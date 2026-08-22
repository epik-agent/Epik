//! A feature build folds a plan into work: as many as four Agents at
//! once, each on its own branch cut from the feature branch as it
//! stands at dispatch, landing through the merge as they finish, until
//! nothing is ready and nothing is running.
//!
//! [`build`] starts the machinery and returns; the caller holds the
//! shared [`Build`] record and watches the build proceed. The ready set
//! fills whatever slots are free — [`CONCURRENCY`] of them, drawn from
//! the [`Budget`] the caller hands in, which a plain build alongside
//! draws on too — and a finished issue releases its slot at once: there
//! is no round barrier, so a fast issue never waits on a slow sibling. Each issue's branch
//! is cut at dispatch from [`Branch::tip`] — the settled tip, read
//! under the merge lock, so a commit a red check is about to reset away
//! is never anyone's base — and an issue that starts late already
//! contains everything that landed early. A standing `issue/<id>`
//! branch is a corpse from an earlier run, kept then on purpose;
//! dispatch deletes it, because a rerun supersedes it. Merges land one
//! at a time through the [`Branch`] the caller established, which is
//! why [`State`] gives Merging a word of its own: an issue can be
//! finished — its Agent gone, its slot released — and still queued
//! behind the merge lock.
//!
//! Every issue of work ends in a terminal state. A failed issue dooms
//! what depended on it, and work that could never become ready — stuck
//! behind an open issue outside the tree, a dangling edge, a cycle —
//! is skipped before anything runs, each naming the blocker it is
//! stuck on; [`Plan::doomed`] is the judgement, [`Plan::problems`]
//! rides in the record, and the build finishes the rest. Epik
//! observes; the Agent commits — an Agent that exits cleanly but
//! advances nothing, or leaves uncommitted work, has failed by
//! observation alone. And Agent events never enter any chat
//! transcript: each dispatched issue's [`Run`] rides in the record,
//! and the record is the sink.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use serde::Serialize;

use super::merge::{Branch, Outcome};
use super::{Issue, IssueId, Plan, Problem};
use crate::agent::Agent;
use crate::build::{Order, Record, Run, Workspace, abandon, launch, provision};
use crate::forge::Forge;
use crate::git::plumbing;

/// The most Agents a feature build runs at once. A constant, not a
/// setting: configuration is its own unbuilt subject, and a number is
/// honest until there is somewhere for a setting to live.
pub const CONCURRENCY: usize = 4;

/// A budget of [`CONCURRENCY`] Agent slots. One per host, so a feature
/// build and a plain build drawing on the same budget never run more
/// than four Agents between them. Each caller keeps its own discipline
/// — a plain build [`claim`](Self::claim)s and refuses, a feature
/// scheduler [`take`](Self::take)s and waits — and both go through the
/// budget's one mutex and condvar, so a release can never fall between
/// a caller's check and its sleep.
#[derive(Debug)]
pub struct Budget {
    free: Mutex<usize>,
    freed: Condvar,
}

impl Budget {
    /// A fresh budget of [`CONCURRENCY`] slots, shared from birth: every
    /// claimant holds the same `Arc`.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            free: Mutex::new(CONCURRENCY),
            freed: Condvar::new(),
        })
    }

    /// One slot, held until the [`Slot`] drops — or `None` when all
    /// four are out. Never waits: the plain build's discipline is to
    /// refuse in words.
    #[must_use]
    pub fn claim(self: &Arc<Self>) -> Option<Slot> {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        if *free == 0 {
            return None;
        }
        *free -= 1;
        Some(Slot(Arc::clone(self)))
    }

    /// One slot, waited for: the feature scheduler's discipline. The
    /// emptiness check and the sleep happen under the budget's own
    /// mutex, and [`release`](Self::release) notifies under the same
    /// one, so a slot freed at any moment is never missed.
    #[must_use]
    pub fn take(self: &Arc<Self>) -> Slot {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        while *free == 0 {
            free = self
                .freed
                .wait(free)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *free -= 1;
        Slot(Arc::clone(self))
    }

    fn release(&self) {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        *free += 1;
        self.freed.notify_all();
    }
}

/// A held Agent slot. Dropping it returns the slot to its [`Budget`]
/// and wakes whoever waits there.
#[derive(Debug)]
pub struct Slot(Arc<Budget>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// Where one issue stands: waiting on a slot or a blocker, running
/// under an Agent, merging behind the lock, and three ends — landed,
/// failed in its own right, or skipped because nothing this build can
/// do would ever make it ready. Serialized — `feature_status`'s answer
/// on its way to a model — tagged by its state word.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// Not yet dispatched: blocked, or ready and out of slots.
    Waiting,
    /// An Agent holds a slot and is working the issue's branch.
    Running,
    /// The Agent is done and the slot released; the merge is queued
    /// behind the lock or under way.
    Merging,
    /// Merged onto the feature branch and pushed. `checked` is false
    /// when the build was handed no check — the branch is unchecked,
    /// and the record says so.
    Merged { commit: String, checked: bool },
    /// Failed, with the words to fail it by: the Agent's end, the
    /// observation, a conflict's paths, or a red check's output.
    Failed { report: String },
    /// Never dispatched, and never will be: the blocker it is stuck on,
    /// which this build can never settle — a failure upstream, or a
    /// plan that was never buildable here.
    Skipped { reason: String },
}

/// The record of a feature build: the plan and what is wrong with its
/// shape, one [`State`] per issue of work, and each dispatched issue's
/// [`Run`] — where its Agent's narration sinks. Shared behind
/// `Arc<Mutex<_>>`: [`build`]'s workers write it, the caller reads it.
#[derive(Clone, Debug)]
pub struct Build {
    pub plan: Plan,
    /// [`Plan::problems`], taken once at the start: cycles and dangling
    /// edges are named here, and the work they strand is Skipped rather
    /// than silently left Waiting.
    pub problems: Vec<Problem>,
    pub states: BTreeMap<IssueId, State>,
    pub runs: BTreeMap<IssueId, Run>,
}

impl Build {
    /// A record for a plan about to build: one state per issue of work
    /// — the open leaves with no settled ancestor — Waiting, except
    /// what could never become ready, which is Skipped at once with the
    /// blocker it is stuck on. What is already settled, or abandoned
    /// under a closed container, is not work and gets no state.
    fn new(plan: Plan) -> Self {
        let none = BTreeSet::new();
        let problems = plan.problems();
        let mut states: BTreeMap<IssueId, State> = plan
            .work(&none)
            .into_iter()
            .map(|leaf| (leaf.id.clone(), State::Waiting))
            .collect();
        for (leaf, blocker) in plan.doomed(&none, &none) {
            states.insert(
                leaf,
                State::Skipped {
                    reason: stuck(&blocker),
                },
            );
        }
        Self {
            plan,
            problems,
            states,
            runs: BTreeMap::new(),
        }
    }

    /// The build is over when nothing is ready and nothing is running —
    /// merging included, because a queued merge can still unblock work.
    /// Every state is terminal by then: what could not end any other
    /// way was Skipped when its fate was sealed.
    #[must_use]
    pub fn finished(&self) -> bool {
        !self
            .states
            .values()
            .any(|state| matches!(state, State::Running | State::Merging))
            && self.dispatchable().is_empty()
    }

    /// The done set fed to [`Plan::ready`]: the leaves this build has
    /// merged. Issues the tracker already closed are settled by the
    /// plan itself.
    fn merged(&self) -> BTreeSet<IssueId> {
        self.states
            .iter()
            .filter(|(_, state)| matches!(state, State::Merged { .. }))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The issues a free slot could take: ready by the plan's judgement
    /// and still Waiting by this build's.
    fn dispatchable(&self) -> Vec<Issue> {
        let done = self.merged();
        self.plan
            .ready(&done)
            .into_iter()
            .filter(|issue| matches!(self.states.get(&issue.id), Some(State::Waiting)))
            .cloned()
            .collect()
    }

    /// Marks `issue` Failed with `report`, and everything the failure
    /// dooms Skipped, each naming the blocker it is stuck on —
    /// [`Plan::doomed`] is the judgement; this only writes it down.
    fn fail(&mut self, issue: &IssueId, report: String) {
        self.states.insert(issue.clone(), State::Failed { report });
        let done = self.merged();
        let lost: BTreeSet<IssueId> = self
            .states
            .iter()
            .filter(|(_, state)| matches!(state, State::Failed { .. } | State::Skipped { .. }))
            .map(|(id, _)| id.clone())
            .collect();
        let doomed = self.plan.doomed(&done, &lost);
        for (leaf, blocker) in doomed {
            self.skip(&leaf, stuck(&blocker));
        }
    }

    /// Skipped, if it was still Waiting: an issue already running,
    /// landed, or failed keeps the state it earned.
    fn skip(&mut self, id: &IssueId, reason: String) {
        if matches!(self.states.get(id), Some(State::Waiting)) {
            self.states.insert(id.clone(), State::Skipped { reason });
        }
    }
}

/// The words a skipped issue carries.
fn stuck(blocker: &IssueId) -> String {
    format!("waits on {blocker}, which this build can never settle")
}

/// The instructions one issue's Agent receives — the order's prompt,
/// which [`Workspace::brief`] wraps in the standing preamble before it
/// reaches the Agent: implement the issue, write tests covering the
/// work, and make them pass.
fn brief(issue: &Issue) -> String {
    format!(
        "Implement issue {}: {}\n\nWrite tests covering the work, and make them pass.",
        issue.id, issue.title
    )
}

/// Starts the feature build and returns; the caller holds the record
/// while the build proceeds, and [`Build::finished`] is how it knows
/// the build is over. `branch` is the feature branch as
/// [`Branch::establish`] left it — shared, so the caller can keep
/// reading [`Branch::tip`] while the build runs — `runner` the agent
/// runner binary, `agents` turns one dispatched issue — with its
/// provisioned workspace and assembled brief — into the Agent that
/// implements it, and `budget` is where the Agent slots come from — the
/// host's one budget, so a plain build running alongside draws on the
/// same four.
pub fn build<F, A, M>(
    plan: Plan,
    branch: Arc<Branch<F>>,
    runner: &Path,
    agents: M,
    budget: Arc<Budget>,
) -> Arc<Mutex<Build>>
where
    F: Forge + Send + Sync + 'static,
    A: Agent + 'static,
    M: Fn(&Issue, &Workspace, &str) -> A + Send + Sync + 'static,
{
    let machinery = Machinery {
        record: Arc::new(Mutex::new(Build::new(plan))),
        signal: Arc::new(Condvar::new()),
        branch,
        runner: runner.to_path_buf(),
        agents: Arc::new(agents),
        budget,
        _agent: PhantomData,
    };
    let record = Arc::clone(&machinery.record);
    std::thread::spawn(move || machinery.schedule());
    record
}

/// Everything a worker thread needs, cheap to clone: the shared record,
/// the condvar that wakes the scheduler, the feature branch, the runner,
/// the Agent factory, and the slot budget.
struct Machinery<F: Forge, A, M> {
    record: Arc<Mutex<Build>>,
    signal: Arc<Condvar>,
    branch: Arc<Branch<F>>,
    runner: PathBuf,
    agents: Arc<M>,
    budget: Arc<Budget>,
    _agent: PhantomData<fn() -> A>,
}

impl<F: Forge, A, M> Clone for Machinery<F, A, M> {
    fn clone(&self) -> Self {
        Self {
            record: Arc::clone(&self.record),
            signal: Arc::clone(&self.signal),
            branch: Arc::clone(&self.branch),
            runner: self.runner.clone(),
            agents: Arc::clone(&self.agents),
            budget: Arc::clone(&self.budget),
            _agent: PhantomData,
        }
    }
}

impl<F, A, M> Machinery<F, A, M>
where
    F: Forge + Send + Sync + 'static,
    A: Agent + 'static,
    M: Fn(&Issue, &Workspace, &str) -> A + Send + Sync + 'static,
{
    /// The slot loop, three steps a turn. First, under the record lock:
    /// wait — on the condvar the workers notify — until something is
    /// dispatchable, or return when the build is over. Second, with no
    /// lock of the record held: [`Budget::take`] a slot, waited for on
    /// the budget's own condvar, so a slot freed by anyone — a sibling
    /// worker, a plain build elsewhere — is never missed. Third, back
    /// under the record lock: marry the slot to the first dispatchable
    /// issue, marked Running under the same lock that judged it ready,
    /// so a slot is never promised twice; the world may have moved while
    /// the slot was waited for, and a slot with no issue left to take it
    /// goes straight back.
    fn schedule(self) {
        loop {
            {
                let mut build = self.record.lock().unwrap_or_else(PoisonError::into_inner);
                loop {
                    if build.finished() {
                        return;
                    }
                    if !build.dispatchable().is_empty() {
                        break;
                    }
                    build = self
                        .signal
                        .wait(build)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
            let slot = self.budget.take();
            let dispatched = {
                let mut build = self.record.lock().unwrap_or_else(PoisonError::into_inner);
                let issue = build.dispatchable().into_iter().next();
                if let Some(issue) = &issue {
                    build.states.insert(issue.id.clone(), State::Running);
                }
                issue
            };
            match dispatched {
                Some(issue) => {
                    let machinery = self.clone();
                    std::thread::spawn(move || machinery.work(&issue, slot));
                }
                None => drop(slot),
            }
        }
    }

    /// One issue, dispatch to its end: however [`attempt`](Self::attempt)
    /// came out, write the state — failing an issue also skips whatever
    /// that dooms — and wake the scheduler. A worker must never hang the
    /// build: the Agent factory is caller code, so a panic anywhere in
    /// the attempt becomes a Failed issue with the panic's words, and
    /// the slot is released like any other end — the attempt owns it,
    /// and every exit drops it.
    fn work(&self, issue: &Issue, slot: Slot) {
        let landed =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.attempt(issue, slot)))
                .unwrap_or_else(|panic| Err(format!("the worker panicked: {}", words(&*panic))));
        {
            let mut build = self.record.lock().unwrap_or_else(PoisonError::into_inner);
            match landed {
                Ok((commit, checked)) => {
                    build
                        .states
                        .insert(issue.id.clone(), State::Merged { commit, checked });
                }
                Err(report) => build.fail(&issue.id, report),
            }
        }
        self.signal.notify_all();
    }

    /// The issue's whole journey: a branch cut from the settled feature
    /// tip at this moment, an Agent launched in a workspace of it, the
    /// commit observation judged, and the branch merged. The slot is
    /// released — Merging said, scheduler woken — before queueing on
    /// the merge lock, so no issue waits on a slower sibling's check;
    /// any earlier exit drops it on the way out.
    fn attempt(&self, issue: &Issue, slot: Slot) -> Result<(String, bool), String> {
        let feature = self.branch.workspace();
        let order = Order {
            prompt: brief(issue),
            repository: feature.repository.clone(),
            branch: format!("issue/{}", issue.id),
            // The settled tip, read under the merge lock — never the
            // feature ref itself, which mid-merge may hold a commit a
            // red check is about to reset away.
            base: Some(self.branch.tip()?),
        };
        // A standing issue branch is a corpse from an earlier run, kept
        // then on purpose; this run supersedes it. One still pinned by
        // a kept worktree refuses in git's words, and the provision
        // below fails the issue with the corpse named.
        let _ = plumbing(&["-C", &order.repository, "branch", "-D", &order.branch]);
        let workspace = provision(&order)?;
        // The factory is caller code. A panic in it is the worker's to
        // report, but the worktree just provisioned is this attempt's to
        // remove first — a failed launch removes its own, and so does
        // a launch that never happened.
        let agent = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.agents)(issue, &workspace, &workspace.brief(&order.prompt))
        }))
        .unwrap_or_else(|panic| {
            abandon(&workspace);
            std::panic::resume_unwind(panic)
        });
        let branch = workspace.branch.clone();
        let run = Record::new(order, workspace);
        // The exit hook stays empty here: a feature issue's slot is
        // released at the Merging transition below, not at the exit.
        let (handle, observer) = launch(&agent, &self.runner, Arc::clone(&run), || {})
            .map_err(|error| format!("could not launch the agent runner: {error}"))?;
        self.record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .runs
            .insert(issue.id.clone(), Arc::clone(&run));
        let exit = handle.wait()?;
        match (exit.code, exit.signal) {
            (Some(0), _) => {}
            (Some(code), _) => return Err(format!("the Agent exited with code {code}")),
            (None, Some(signal)) => return Err(format!("the Agent was killed by signal {signal}")),
            (None, None) => return Err("the Agent ended without saying how".to_owned()),
        }
        // The observation is the last thing the drainer writes; joining
        // it waits exactly as long as that takes, no polling.
        observer
            .join()
            .map_err(|_| "the run's observer died unsettled".to_owned())?;
        let commits = run
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .commits
            .clone()
            .ok_or("the run ended with no commit observation")?;
        if !commits.advanced {
            return Err(
                "the Agent committed nothing: the branch never advanced past its base".to_owned(),
            );
        }
        if !commits.clean {
            return Err(match &commits.kept {
                Some(path) => format!("the Agent left uncommitted work at {}", path.display()),
                None => "the Agent left uncommitted work".to_owned(),
            });
        }
        {
            self.record
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .states
                .insert(issue.id.clone(), State::Merging);
        }
        drop(slot);
        self.signal.notify_all();
        match self.branch.merge(&branch)? {
            Outcome::Merged { commit, checked } => Ok((commit, checked)),
            Outcome::Conflict { paths } => {
                Err(format!("the merge conflicted at: {}", paths.join(", ")))
            }
            Outcome::Red { output } => Err(format!("the merged result failed the check: {output}")),
        }
    }
}

/// A panic payload's words, when it has any.
fn words(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no words came with it")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::fixtures::{
        a_chain_beside_a_loner, id, issue, nine_waiting_on_container_seven, node, plan,
        two_and_three_in_a_cycle, two_then_three,
    };

    #[test]
    fn a_new_build_states_only_the_open_reachable_leaves() {
        let build = Build::new(plan(
            1,
            vec![
                node(1, false, &[2, 3, 6, 8], &[]),
                node(2, true, &[], &[]),
                node(3, false, &[4, 5], &[]),
                node(4, false, &[], &[]),
                node(5, false, &[], &[]),
                node(6, true, &[7], &[]),
                node(7, false, &[], &[]),
                node(8, false, &[], &[]),
            ],
        ));
        let work: Vec<&str> = build.states.keys().map(|id| id.0.as_str()).collect();
        assert_eq!(
            work,
            ["4", "5", "8"],
            "no containers, no closed leaves, no abandoned subtrees"
        );
        assert!(build.states.values().all(|s| *s == State::Waiting));
        assert!(build.problems.is_empty());
    }

    #[test]
    fn what_could_never_become_ready_is_skipped_before_anything_runs() {
        let build = Build::new(plan(
            1,
            vec![
                node(1, false, &[2, 3], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(55, false)]),
            ],
        ));
        assert_eq!(build.states[&id(2)], State::Waiting);
        let State::Skipped { reason } = &build.states[&id(3)] else {
            panic!("an open outside blocker is permanent: {:?}", build.states);
        };
        assert!(reason.contains("waits on 55"), "{reason}");
    }

    #[test]
    fn a_cycle_is_skipped_at_the_start_and_named_in_the_problems() {
        let build = Build::new(two_and_three_in_a_cycle());
        assert!(
            build
                .states
                .values()
                .all(|state| matches!(state, State::Skipped { .. })),
            "{:?}",
            build.states
        );
        assert!(
            build
                .problems
                .iter()
                .any(|problem| matches!(problem, Problem::Cycle(_))),
            "{:?}",
            build.problems
        );
        assert!(build.finished(), "no non-terminal state is left behind");
    }

    #[test]
    fn a_failure_skips_its_dependents_transitively_and_spares_the_rest() {
        let mut build = Build::new(a_chain_beside_a_loner());
        build.fail(&id(2), "the wumpus got in".to_owned());
        assert_eq!(
            build.states[&id(2)],
            State::Failed {
                report: "the wumpus got in".to_owned()
            }
        );
        let State::Skipped { reason } = &build.states[&id(3)] else {
            panic!("3 waited on 2: {:?}", build.states);
        };
        assert!(reason.contains("waits on 2"), "{reason}");
        let State::Skipped { reason } = &build.states[&id(5)] else {
            panic!("5 waited on 3: {:?}", build.states);
        };
        assert!(
            reason.contains("waits on 3"),
            "doom is transitive: {reason}"
        );
        assert_eq!(build.states[&id(4)], State::Waiting, "4 waited on no one");
    }

    #[test]
    fn a_failure_under_a_container_strands_whoever_waits_on_the_container() {
        let mut build = Build::new(nine_waiting_on_container_seven());
        build.fail(&id(2), "boom".to_owned());
        let State::Skipped { reason } = &build.states[&id(9)] else {
            panic!("7 can never settle: {:?}", build.states);
        };
        assert!(reason.contains("waits on 7"), "{reason}");
        assert_eq!(
            build.states[&id(8)],
            State::Waiting,
            "8 is still work; the build finishes the rest"
        );
    }

    #[test]
    fn skipping_never_overwrites_a_state_an_issue_earned() {
        let mut build = Build::new(two_then_three());
        build.states.insert(
            id(3),
            State::Merged {
                commit: "abc".to_owned(),
                checked: true,
            },
        );
        build.fail(&id(2), "late".to_owned());
        assert!(matches!(build.states[&id(3)], State::Merged { .. }));
    }

    #[test]
    fn the_build_ends_when_nothing_is_ready_and_nothing_is_running() {
        let mut build = Build::new(two_then_three());
        assert!(!build.finished(), "2 is ready");
        build.states.insert(id(2), State::Running);
        assert!(!build.finished(), "an Agent is out");
        build.states.insert(id(2), State::Merging);
        assert!(!build.finished(), "a queued merge can still unblock work");
        build.states.insert(
            id(2),
            State::Merged {
                commit: "abc".to_owned(),
                checked: false,
            },
        );
        assert!(!build.finished(), "2 landing made 3 ready");
        build.states.insert(id(3), State::Running);
        build.fail(&id(3), "boom".to_owned());
        assert!(build.finished(), "nothing ready, nothing running");
    }

    #[test]
    fn a_plan_with_no_work_is_finished_before_it_starts() {
        let build = Build::new(plan(
            1,
            vec![node(1, false, &[2], &[]), node(2, true, &[], &[])],
        ));
        assert!(build.states.is_empty());
        assert!(build.finished());
        assert_eq!(build.problems, [Problem::NoWork]);
    }

    #[test]
    fn the_brief_names_the_issue_and_demands_tests() {
        let brief = brief(&issue(7, false));
        assert!(brief.contains("Implement issue 7: issue 7"), "{brief}");
        assert!(
            brief.contains("Write tests covering the work, and make them pass."),
            "{brief}"
        );
    }

    #[test]
    fn a_panics_words_are_read_from_either_payload() {
        assert_eq!(words(&"static words"), "static words");
        assert_eq!(words(&"owned words".to_owned()), "owned words");
        assert_eq!(words(&7_u64), "no words came with it");
    }

    #[test]
    fn the_budget_yields_exactly_four_slots_and_a_drop_gives_one_back() {
        let budget = Budget::new();
        let held: Vec<Slot> = std::iter::from_fn(|| budget.claim()).collect();
        assert_eq!(held.len(), CONCURRENCY);
        assert!(budget.claim().is_none(), "the fifth claim is refused");
        drop(held);
        assert!(budget.claim().is_some(), "a dropped slot comes back");
    }

    /// The waited claim: `take` on an exhausted budget blocks and comes
    /// back the moment a slot is released. The emptiness check and the
    /// sleep share the budget's own mutex, so this holds whether the
    /// release lands before, during, or after the taker's arrival —
    /// there is no window for a lost wakeup.
    #[test]
    fn take_waits_out_an_exhausted_budget_and_is_woken_by_a_release() {
        let budget = Budget::new();
        let mut held: Vec<Slot> = std::iter::from_fn(|| budget.claim()).collect();
        assert_eq!(held.len(), CONCURRENCY);

        let taker = std::thread::spawn({
            let budget = Arc::clone(&budget);
            move || budget.take()
        });
        drop(held.pop());
        let taken = taker.join().unwrap();

        assert!(budget.claim().is_none(), "the taken slot is really held");
        drop(taken);
        assert!(budget.claim().is_some());
    }
}
