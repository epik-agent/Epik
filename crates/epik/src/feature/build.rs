//! A feature build folds a plan into work: as many as four Agents at
//! once, each a [`job`](crate::job) on its own branch cut from the
//! feature branch as it stands at dispatch, landing through the merge
//! as they finish, until nothing is ready and nothing is running.
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
//!
//! Every movement of the record is also spoken: [`Build::set`] is the
//! one writer of the states, and each write is a
//! [`Moved`](Change::Moved) in the host's [`Log`]; [`build`] records
//! [`Started`](Change::Started) — the plan, its problems, and the
//! initial states, so those never ride as a stream of moves — before
//! the scheduler exists, and [`Finished`](Change::Finished) exactly
//! once, where the scheduler observes [`Build::finished`] — and then
//! retires the feature workspace, so the branch a build held while it
//! ran is free for a rebuild. The log is how a window watches a build
//! without asking after it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use super::merge::{Branch, Outcome};
use super::{Issue, IssueId, Plan, Problem, RunId, State};
use crate::agent::{Agent, Exit};
use crate::forge::Forge;
use crate::git::plumbing;
use crate::job::{Order, Phase, Record, Run, Workspace, abandon, launch, provision};
use crate::monitor::{Change, Log};

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
    fn take(self: &Arc<Self>) -> Slot {
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

/// The record of a feature build: the plan and what is wrong with its
/// shape, one [`State`] per issue of work, and each dispatched issue's
/// [`Run`] — where its Agent's narration sinks. Shared behind
/// `Arc<Mutex<_>>`: [`build`]'s workers write it, the caller reads it,
/// and every write is spoken to the log as the run's own.
#[derive(Clone, Debug)]
pub struct Build {
    pub(super) run: RunId,
    pub(super) log: Arc<Log>,
    pub(super) plan: Plan,
    /// [`Plan::problems`], taken once at the start: cycles and dangling
    /// edges are named here, and the work they strand is Skipped rather
    /// than silently left Waiting.
    pub(super) problems: Vec<Problem>,
    /// Written through [`set`](Self::set) and nothing else.
    pub(super) states: BTreeMap<IssueId, State>,
    pub(super) runs: BTreeMap<IssueId, Run>,
}

impl Build {
    /// A record for a plan about to build: one state per issue of work
    /// — the open leaves with no settled ancestor — Waiting, except
    /// what could never become ready, which is Skipped at once with the
    /// blocker it is stuck on. What is already settled, or abandoned
    /// under a closed container, is not work and gets no state. Nothing
    /// is recorded here: the initial map rides in Started, whole.
    fn new(run: RunId, log: Arc<Log>, plan: Plan) -> Self {
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
            run,
            log,
            plan,
            problems,
            states,
            runs: BTreeMap::new(),
        }
    }

    /// The one writer: `issue` moves to `state`, in the record and in
    /// the log, under whatever lock the caller holds on the record — so
    /// a run's moves are recorded in the order they were made.
    pub(super) fn set(&mut self, issue: &IssueId, state: State) {
        self.states.insert(issue.clone(), state.clone());
        self.log.record(Change::Moved {
            run: self.run,
            issue: issue.clone(),
            state,
        });
    }

    /// What Started says of this record: the plan, its problems in
    /// words, the initial states, and the check in force.
    fn started(&self, check: Option<String>) -> Change {
        Change::Started {
            run: self.run,
            plan: self.plan.clone(),
            problems: self.problems.iter().map(ToString::to_string).collect(),
            states: self.states.clone(),
            check,
        }
    }

    /// The build is over when nothing is ready and nothing is running —
    /// merging included, because a queued merge can still unblock work.
    /// Every state is terminal by then: what could not end any other
    /// way was Skipped when its fate was sealed.
    #[must_use]
    pub(super) fn finished(&self) -> bool {
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
        self.set(issue, State::Failed { report });
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
            self.set(id, State::Skipped { reason });
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
/// the build is over — or the log's Finished, which the scheduler
/// records the moment it sees the same. `run` is the id the build goes
/// by in the log, `branch` is the feature branch as
/// [`Branch::establish`] left it — shared, so the caller can keep
/// reading [`Branch::tip`] while the build runs; the check it holds is
/// what Started reports — `agents` starts, for one dispatched issue —
/// with its provisioned workspace and assembled brief — the Agent that
/// implements it, or refuses in words, and `budget` is where the Agent
/// slots come from — the host's one budget, so a plain build running
/// alongside draws on the same four. Started is recorded before the
/// scheduler exists, so no Moved can precede it.
pub fn build<F, M>(
    run: RunId,
    log: Arc<Log>,
    plan: Plan,
    branch: Arc<Branch<F>>,
    agents: M,
    budget: Arc<Budget>,
) -> Arc<Mutex<Build>>
where
    F: Forge + Send + Sync + 'static,
    M: Fn(&Issue, &Workspace, &str) -> Result<Agent, String> + Send + Sync + 'static,
{
    let build = Build::new(run, Arc::clone(&log), plan);
    log.record(build.started(branch.check().map(|check| check.command.clone())));
    let machinery = Machinery {
        record: Arc::new(Mutex::new(build)),
        signal: Arc::new(Condvar::new()),
        branch,
        agents: Arc::new(agents),
        budget,
    };
    let record = Arc::clone(&machinery.record);
    std::thread::spawn(move || machinery.schedule());
    record
}

/// Everything a worker thread needs, cheap to clone: the shared record,
/// the condvar that wakes the scheduler, the feature branch, the Agent
/// factory, and the slot budget.
struct Machinery<F: Forge, M> {
    record: Arc<Mutex<Build>>,
    signal: Arc<Condvar>,
    branch: Arc<Branch<F>>,
    agents: Arc<M>,
    budget: Arc<Budget>,
}

impl<F: Forge, M> Clone for Machinery<F, M> {
    fn clone(&self) -> Self {
        Self {
            record: Arc::clone(&self.record),
            signal: Arc::clone(&self.signal),
            branch: Arc::clone(&self.branch),
            agents: Arc::clone(&self.agents),
            budget: Arc::clone(&self.budget),
        }
    }
}

impl<F, M> Machinery<F, M>
where
    F: Forge + Send + Sync + 'static,
    M: Fn(&Issue, &Workspace, &str) -> Result<Agent, String> + Send + Sync + 'static,
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
    /// goes straight back. The end is observed here and nowhere else,
    /// so Finished is recorded exactly once — under the same lock as
    /// the last move, and at once for a plan with nothing to do — and
    /// the feature workspace is retired after it, with no lock held:
    /// the branch is free for a rebuild one git command after the log
    /// says the build is over.
    fn schedule(self) {
        loop {
            {
                let mut build = self.record.lock().unwrap_or_else(PoisonError::into_inner);
                loop {
                    if build.finished() {
                        build.log.record(Change::Finished { run: build.run });
                        drop(build);
                        self.branch.retire();
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
                    build.set(&issue.id, State::Running);
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
                Ok((commit, checked)) => build.set(&issue.id, State::Merged { commit, checked }),
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
        // The factory is caller code. A panic in it, or a refusal, is
        // the worker's to report, but the worktree just provisioned is
        // this attempt's to remove first — an Agent that never started
        // leaves nothing behind.
        let agent = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.agents)(issue, &workspace, &workspace.brief(&order.prompt))
        }))
        .unwrap_or_else(|panic| {
            abandon(&workspace);
            std::panic::resume_unwind(panic)
        })
        .inspect_err(|_| abandon(&workspace))?;
        let branch = workspace.branch.clone();
        let run = Record::new(order, workspace);
        // The exit hook stays empty here: a feature issue's slot is
        // released at the Merging transition below, not at the exit.
        let observer = launch(agent, Arc::clone(&run), || {});
        self.record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .runs
            .insert(issue.id.clone(), Arc::clone(&run));
        // The observation is the last thing the drainer writes; joining
        // it waits exactly as long as that takes, no polling.
        observer
            .join()
            .map_err(|_| "the run's observer died unsettled".to_owned())?;
        let (phase, commits) = {
            let run = run.lock().unwrap_or_else(PoisonError::into_inner);
            (run.phase.clone(), run.commits.clone())
        };
        match phase {
            Phase::Finished(Exit::Code(0)) => {}
            Phase::Finished(Exit::Code(code)) => {
                return Err(format!("the Agent exited with code {code}"));
            }
            Phase::Finished(Exit::Signal(signal)) => {
                return Err(format!("the Agent was killed by signal {signal}"));
            }
            Phase::Lost(words) => return Err(words),
            Phase::Running => return Err("the run ended without an exit".to_owned()),
        }
        let commits = commits.ok_or("the run ended with no commit observation")?;
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
        self.record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set(&issue.id, State::Merging);
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
        a_chain_beside_a_loner, committing, id, issue, nine_waiting_on_container_seven, node, plan,
        two_and_three_in_a_cycle, two_then_three,
    };
    use crate::monitor::{Entry, Stage, folded};
    use crate::testing::{Local, Scratch, seeded};

    /// A record for `plan` as run 1, on a log of its own.
    fn fresh(plan: Plan) -> Build {
        Build::new(RunId(1), Arc::new(Log::new()), plan)
    }

    /// The Moved entries of `entries`, as (issue, state) pairs.
    fn moves(entries: &[Entry]) -> Vec<(IssueId, State)> {
        entries
            .iter()
            .filter_map(|entry| match &entry.change {
                Change::Moved { issue, state, .. } => Some((issue.clone(), state.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_new_build_states_only_the_open_reachable_leaves() {
        let build = fresh(plan(
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
        let build = fresh(plan(
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
        let build = fresh(two_and_three_in_a_cycle());
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
        let mut build = fresh(a_chain_beside_a_loner());
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
        let mut build = fresh(nine_waiting_on_container_seven());
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
        let mut build = fresh(two_then_three());
        build.set(
            &id(3),
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
        let mut build = fresh(two_then_three());
        assert!(!build.finished(), "2 is ready");
        build.set(&id(2), State::Running);
        assert!(!build.finished(), "an Agent is out");
        build.set(&id(2), State::Merging);
        assert!(!build.finished(), "a queued merge can still unblock work");
        build.set(
            &id(2),
            State::Merged {
                commit: "abc".to_owned(),
                checked: false,
            },
        );
        assert!(!build.finished(), "2 landing made 3 ready");
        build.set(&id(3), State::Running);
        build.fail(&id(3), "boom".to_owned());
        assert!(build.finished(), "nothing ready, nothing running");
    }

    #[test]
    fn a_plan_with_no_work_is_finished_before_it_starts() {
        let build = fresh(plan(
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

    /// A new record speaks nothing — its map rides in Started — and
    /// every write after that is one Moved, the skips a failure causes
    /// included.
    #[test]
    fn every_write_is_one_moved_and_the_initial_map_is_none() {
        let mut build = fresh(a_chain_beside_a_loner());
        assert!(build.log.since(0).is_empty());
        build.fail(&id(2), "the wumpus got in".to_owned());
        let moved = moves(&build.log.since(0));
        assert_eq!(moved.len(), 3, "2 failed, 3 and 5 skipped: {moved:?}");
        assert_eq!(
            moved[0],
            (
                id(2),
                State::Failed {
                    report: "the wumpus got in".to_owned()
                }
            )
        );
        for (issue, state) in &moved {
            assert_eq!(&build.states[issue], state);
        }
    }

    /// Removes every linked worktree a build left behind, so a scratch
    /// drop is enough.
    fn tidy(repository: &str) {
        let listed = plumbing(&["-C", repository, "worktree", "list", "--porcelain"]).unwrap();
        for line in listed.lines() {
            if let Some(path) = line.strip_prefix("worktree ")
                && path != repository
            {
                let _ = plumbing(&[
                    "-C", repository, "worktree", "remove", "--force", "--", path,
                ]);
            }
        }
    }

    /// Waits for `run`'s Finished to land in the log — the last thing
    /// its scheduler records — bounded, never by a fixed sleep.
    fn eventually_finished(log: &Log, run: RunId) -> Vec<Entry> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let mut cursor = 0;
        loop {
            let heard = log.wait_after(cursor, std::time::Duration::from_secs(1));
            if heard
                .iter()
                .any(|entry| entry.change == Change::Finished { run })
            {
                return log.since(0);
            }
            cursor += heard.len() as u64;
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the build to finish: {:?}",
                log.since(0)
            );
        }
    }

    /// Launches `plan` as run `run` for feature `feature`, the way the
    /// tools would: Reserved spoken, the feature branch `branch`
    /// established in `work` over `remote`, and the machinery started
    /// against the committing Agent on `budget` and `log` — both shared
    /// by whoever else is launched beside it.
    fn launched(
        run: u64,
        feature: u64,
        plan: Plan,
        (work, remote): (&str, &str),
        branch: &str,
        budget: &Arc<Budget>,
        log: &Arc<Log>,
    ) -> Arc<Mutex<Build>> {
        let established = Arc::new(
            Branch::establish(work, branch, "main", Local(remote.to_owned()), None).unwrap(),
        );
        // The reservation is the tools' to speak; here the test speaks
        // it, so the fold has a run to hang Started on.
        log.record(Change::Reserved {
            run: RunId(run),
            feature: id(feature),
            repository: work.to_owned(),
            branch: branch.to_owned(),
            base: Some("main".to_owned()),
        });
        build(
            RunId(run),
            Arc::clone(log),
            plan,
            established,
            committing(None),
            Arc::clone(budget),
        )
    }

    /// The build is over with every leaf in `leaves` Merged, and each
    /// leaf's file is in the tree at `branch`'s tip.
    fn landed(work: &str, branch: &str, record: &Arc<Mutex<Build>>, leaves: &[u64]) {
        let build = record.lock().unwrap().clone();
        assert!(build.finished());
        let tree = git2::Repository::open(work).unwrap();
        let tip = tree
            .find_branch(branch, git2::BranchType::Local)
            .unwrap()
            .get()
            .peel_to_tree()
            .unwrap();
        for leaf in leaves {
            let id = id(*leaf);
            assert!(
                matches!(build.states[&id], State::Merged { .. }),
                "{branch} {id}: {:?}",
                build.states
            );
            assert!(
                tip.get_name(&format!("{id}.txt")).is_some(),
                "{branch} {id}"
            );
        }
    }

    /// The runs the log says Finished, in order — each once, if the
    /// scheduler keeps its word.
    fn finished(entries: &[Entry]) -> Vec<RunId> {
        entries
            .iter()
            .filter_map(|entry| match entry.change {
                Change::Finished { run } => Some(run),
                _ => None,
            })
            .collect()
    }

    /// Feature 4 holds 5 and 6, 6 waiting on 5: `two_then_three`'s
    /// shape on ids that collide with none of its own, so the two can
    /// build in one repository — issue branches are named for the id.
    fn five_then_six() -> Plan {
        plan(
            4,
            vec![
                node(4, false, &[5, 6], &[]),
                node(5, false, &[], &[]),
                node(6, false, &[], &[(5, false)]),
            ],
        )
    }

    /// The whole machinery against a scripted Agent that commits one
    /// file and exits: 2 lands, which readies 3, which lands — every
    /// leaf Merged. The log tells the same story: one Moved per
    /// terminal state, agreeing issue for issue with the record, and
    /// the fold of the log is the record.
    #[test]
    fn a_scripted_agent_merges_every_leaf_and_the_log_is_the_record() {
        let scratch = Scratch::new("feature-build");
        let (work, remote) = seeded(&scratch);
        let log = Arc::new(Log::new());
        let record = launched(
            1,
            1,
            two_then_three(),
            (&work, &remote),
            "feature-1",
            &Budget::new(),
            &log,
        );

        let entries = eventually_finished(&log, RunId(1));
        landed(&work, "feature-1", &record, &[2, 3]);
        let build = record.lock().unwrap().clone();

        // Reserved, then Started before any move; Finished closes, once.
        assert!(matches!(entries[0].change, Change::Reserved { .. }));
        assert!(matches!(entries[1].change, Change::Started { .. }));
        assert!(matches!(
            entries.last().unwrap().change,
            Change::Finished { .. }
        ));
        assert_eq!(finished(&entries), [RunId(1)]);
        // One Moved per terminal state, each the record's own.
        let moved = moves(&entries);
        let terminal: Vec<_> = moved
            .iter()
            .filter(|(_, state)| matches!(state, State::Merged { .. }))
            .collect();
        assert_eq!(terminal.len(), 2, "{moved:?}");
        for (issue, state) in &terminal {
            assert_eq!(&build.states[issue], state);
        }
        // Each issue's own story, in order.
        for id in [id(2), id(3)] {
            let story: Vec<&State> = moved
                .iter()
                .filter(|(issue, _)| *issue == id)
                .map(|(_, state)| state)
                .collect();
            assert_eq!(story.len(), 3, "{id}: {story:?}");
            assert_eq!(story[0], &State::Running);
            assert_eq!(story[1], &State::Merging);
        }
        // The fold reconstructs the record.
        let progress = folded(&log);
        let watch = progress.watch(RunId(1)).expect("Started names the run");
        assert_eq!(*watch.states(), build.states);
        assert_eq!(*watch.stage(), Stage::Finished);
        assert_eq!(watch.counts().merged, 2);

        tidy(&work);
    }

    /// When the build is over its workspace is retired: the feature
    /// branch has no worktree, the tip still answers from the
    /// repository, and the same feature establishes again — the rebuild
    /// git would otherwise refuse.
    #[test]
    fn a_finished_build_retires_its_workspace_and_the_feature_can_be_rebuilt() {
        let scratch = Scratch::new("retire");
        let (work, remote) = seeded(&scratch);
        let log = Arc::new(Log::new());
        let branch = Arc::new(
            Branch::establish(&work, "feature-1", "main", Local(remote.clone()), None).unwrap(),
        );
        let workspace = branch.workspace().directory.clone();
        let record = build(
            RunId(1),
            Arc::clone(&log),
            two_then_three(),
            Arc::clone(&branch),
            committing(None),
            Budget::new(),
        );
        eventually_finished(&log, RunId(1));
        landed(&work, "feature-1", &record, &[2, 3]);

        // Retirement follows Finished by the width of one git command.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while crate::job::held_by(&work, "feature-1").unwrap().is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "the workspace was never retired"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!workspace.exists());
        let tip = branch.tip().unwrap();
        assert_eq!(
            tip,
            plumbing(&["-C", &work, "rev-parse", "refs/heads/feature-1"])
                .unwrap()
                .trim()
        );

        let again = Branch::establish(&work, "feature-1", "main", Local(remote), None).unwrap();
        assert_eq!(again.workspace().base_commit, tip, "as the build left it");
        tidy(&work);
    }

    /// Two feature builds in one repository, off the same base, at the
    /// same time — one budget, one log, the clone carrying both builds'
    /// worktree adds and removes and ref updates — and both complete
    /// with every leaf Merged, the log saying Finished once for each.
    #[test]
    fn two_features_in_one_repository_build_at_once() {
        let scratch = Scratch::new("two-features");
        let (work, remote) = seeded(&scratch);
        let budget = Budget::new();
        let log = Arc::new(Log::new());
        let first = launched(
            1,
            1,
            two_then_three(),
            (&work, &remote),
            "feature-1",
            &budget,
            &log,
        );
        let second = launched(
            2,
            4,
            five_then_six(),
            (&work, &remote),
            "feature-2",
            &budget,
            &log,
        );

        eventually_finished(&log, RunId(1));
        let entries = eventually_finished(&log, RunId(2));
        landed(&work, "feature-1", &first, &[2, 3]);
        landed(&work, "feature-2", &second, &[5, 6]);
        let mut runs = finished(&entries);
        runs.sort_unstable();
        assert_eq!(runs, [RunId(1), RunId(2)]);

        tidy(&work);
    }

    /// Two feature builds in two repositories at the same time, sharing
    /// nothing but the host's budget and log, and both complete.
    #[test]
    fn two_features_in_two_repositories_build_at_once() {
        let one = Scratch::new("repository-one");
        let two = Scratch::new("repository-two");
        let (work_one, remote_one) = seeded(&one);
        let (work_two, remote_two) = seeded(&two);
        let budget = Budget::new();
        let log = Arc::new(Log::new());
        let first = launched(
            1,
            1,
            two_then_three(),
            (&work_one, &remote_one),
            "feature-1",
            &budget,
            &log,
        );
        let second = launched(
            2,
            1,
            two_then_three(),
            (&work_two, &remote_two),
            "feature-1",
            &budget,
            &log,
        );

        eventually_finished(&log, RunId(1));
        let entries = eventually_finished(&log, RunId(2));
        landed(&work_one, "feature-1", &first, &[2, 3]);
        landed(&work_two, "feature-1", &second, &[2, 3]);
        let mut runs = finished(&entries);
        runs.sort_unstable();
        assert_eq!(runs, [RunId(1), RunId(2)]);

        tidy(&work_one);
        tidy(&work_two);
    }
}
