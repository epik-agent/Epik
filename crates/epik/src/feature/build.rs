//! A feature build folds a plan into work: as many as four Agents at
//! once, each on its own branch cut from the feature branch as it
//! stands at dispatch, landing through the merge as they finish, until
//! nothing is ready and nothing is running.
//!
//! [`build`] starts the machinery and returns; the caller holds the
//! shared [`Build`] record and watches the build proceed. The ready set
//! fills whatever slots are free — [`CONCURRENCY`] of them — and a
//! finished issue releases its slot at once: there is no round barrier,
//! so a fast issue never waits on a slow sibling. Each issue's branch
//! is cut from the feature branch's tip at the moment it is dispatched,
//! so an issue that starts late already contains everything that landed
//! early. Merges land one at a time through the
//! [`Branch`](super::merge::Branch) the caller established, which is
//! why [`State`] gives Merging a word of its own: an issue can be
//! finished — its Agent gone, its slot released — and still queued
//! behind the merge lock.
//!
//! A failed issue dooms what depended on it: its descendants, and
//! everything transitively blocked by it, become Skipped, and the build
//! finishes the rest. Epik observes; the Agent commits — an Agent that
//! exits cleanly but advances nothing, or leaves uncommitted work, has
//! failed by observation alone. And Agent events never enter any chat
//! transcript: each dispatched issue's [`Run`] rides in the record, and
//! the record is the sink.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use serde::Serialize;

use super::merge::{Branch, Outcome};
use super::{Issue, IssueId, Plan};
use crate::agent::Agent;
use crate::build::{Order, Record, Run, Workspace, launch, provision};
use crate::forge::Forge;

/// The most Agents a feature build runs at once. A constant, not a
/// setting: configuration is its own unbuilt subject, and a number is
/// honest until there is somewhere for a setting to live.
pub const CONCURRENCY: usize = 4;

/// How often a worker re-reads its run while the commit observation is
/// still on its way.
const OBSERVATION_POLL: Duration = Duration::from_millis(20);

/// Where one issue stands: waiting on a slot or a blocker, running
/// under an Agent, merging behind the lock, and three ends — landed,
/// failed in its own right, or skipped because a failure upstream made
/// it unreachable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
    /// Never dispatched: a failure upstream means it can never become
    /// ready.
    Skipped,
}

/// The record of a feature build: the plan, one [`State`] per issue of
/// work, and each dispatched issue's [`Run`] — where its Agent's
/// narration sinks. Shared behind `Arc<Mutex<_>>`: [`build`]'s workers
/// write it, the caller reads it.
#[derive(Clone, Debug)]
pub struct Build {
    pub plan: Plan,
    pub states: BTreeMap<IssueId, State>,
    pub runs: BTreeMap<IssueId, Run>,
}

impl Build {
    /// A record for a plan about to build: one Waiting state per issue
    /// of work — the open leaves with no settled ancestor. What is
    /// already settled, or abandoned under a closed container, is not
    /// work and gets no state.
    fn new(plan: Plan) -> Self {
        let none = BTreeSet::new();
        let states = plan
            .tree
            .leaves()
            .filter(|leaf| {
                plan.tree
                    .find_path(|issue| issue.id == leaf.id)
                    .expect("a leaf is in its own tree")
                    .iter()
                    .all(|node| !super::settled(node, &none))
            })
            .map(|leaf| (leaf.id.clone(), State::Waiting))
            .collect();
        Self {
            plan,
            states,
            runs: BTreeMap::new(),
        }
    }

    /// The build is over when nothing is ready and nothing is running —
    /// merging included, because a queued merge can still unblock work.
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
    /// dooms Skipped: the issue's descendants, and everything
    /// transitively blocked by it — containers included, since a
    /// container with a lost leaf beneath it can never settle. A
    /// fixpoint, because each skip can strand someone further out.
    fn fail(&mut self, issue: &IssueId, report: String) {
        self.states.insert(issue.clone(), State::Failed { report });
        let descendants: Vec<IssueId> = self
            .plan
            .tree
            .find_path(|node| &node.id == issue)
            .map(|path| {
                path.last()
                    .expect("a found path reaches its match")
                    .leaves()
                    .filter(|leaf| &leaf.id != issue)
                    .map(|leaf| leaf.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        for id in descendants {
            self.skip(&id);
        }
        loop {
            let stranded: Vec<IssueId> = self
                .states
                .iter()
                .filter(|(_, state)| matches!(state, State::Waiting))
                .filter(|(id, _)| {
                    self.plan
                        .blocking
                        .iter()
                        .filter(|edge| &edge.issue == *id)
                        .any(|edge| self.hopeless(&edge.blocker))
                })
                .map(|(id, _)| id.clone())
                .collect();
            if stranded.is_empty() {
                break;
            }
            for id in stranded {
                self.skip(&id);
            }
        }
    }

    /// Skipped, if it was still Waiting: an issue already running,
    /// landed, or failed keeps the state it earned.
    fn skip(&mut self, id: &IssueId) {
        if matches!(self.states.get(id), Some(State::Waiting)) {
            self.states.insert(id.clone(), State::Skipped);
        }
    }

    /// A blocker this build can never settle: a node of the tree with a
    /// failed or skipped leaf beneath it. An issue outside the tree is
    /// judged by its own state, never by ours.
    fn hopeless(&self, blocker: &IssueId) -> bool {
        self.plan
            .tree
            .find_path(|node| &node.id == blocker)
            .is_some_and(|path| {
                path.last()
                    .expect("a found path reaches its match")
                    .leaves()
                    .any(|leaf| {
                        matches!(
                            self.states.get(&leaf.id),
                            Some(State::Failed { .. } | State::Skipped)
                        )
                    })
            })
    }
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
/// [`Branch::establish`] left it, `runner` the agent runner binary, and
/// `agents` turns one dispatched issue — with its provisioned workspace
/// and assembled brief — into the Agent that implements it.
pub fn build<F, A, M>(plan: Plan, branch: Branch<F>, runner: &Path, agents: M) -> Arc<Mutex<Build>>
where
    F: Forge + Send + Sync + 'static,
    A: Agent + 'static,
    M: Fn(&Issue, &Workspace, &str) -> A + Send + Sync + 'static,
{
    let machinery = Machinery {
        record: Arc::new(Mutex::new(Build::new(plan))),
        signal: Arc::new(Condvar::new()),
        branch: Arc::new(branch),
        runner: runner.to_path_buf(),
        agents: Arc::new(agents),
        _agent: PhantomData,
    };
    let record = Arc::clone(&machinery.record);
    std::thread::spawn(move || machinery.schedule());
    record
}

/// Everything a worker thread needs, cheap to clone: the shared record,
/// the condvar that wakes the scheduler, the feature branch, the runner,
/// and the Agent factory.
struct Machinery<F: Forge, A, M> {
    record: Arc<Mutex<Build>>,
    signal: Arc<Condvar>,
    branch: Arc<Branch<F>>,
    runner: PathBuf,
    agents: Arc<M>,
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
    /// The slot loop: fill whatever slots are free from the ready set,
    /// sleep when nothing can move, return when nothing is ready and
    /// nothing is running. Issues are marked Running under the same
    /// lock that read the ready set, so a slot is never promised twice.
    fn schedule(self) {
        loop {
            let batch = {
                let mut build = self.record.lock().unwrap_or_else(PoisonError::into_inner);
                loop {
                    let running = build
                        .states
                        .values()
                        .filter(|state| matches!(state, State::Running))
                        .count();
                    let batch: Vec<Issue> = build
                        .dispatchable()
                        .into_iter()
                        .take(CONCURRENCY.saturating_sub(running))
                        .collect();
                    if !batch.is_empty() {
                        for issue in &batch {
                            build.states.insert(issue.id.clone(), State::Running);
                        }
                        break batch;
                    }
                    if build.finished() {
                        return;
                    }
                    build = self
                        .signal
                        .wait(build)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            };
            for issue in batch {
                let machinery = self.clone();
                std::thread::spawn(move || machinery.work(&issue));
            }
        }
    }

    /// One issue, dispatch to its end: however [`attempt`](Self::attempt)
    /// came out, write the state — failing an issue also skips whatever
    /// that dooms — and wake the scheduler.
    fn work(&self, issue: &Issue) {
        let landed = self.attempt(issue);
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

    /// The issue's whole journey: a branch cut from the feature tip at
    /// this moment, an Agent launched in a workspace of it, the commit
    /// observation judged, and the branch merged. The slot is released —
    /// Merging said, scheduler woken — before queueing on the merge
    /// lock, so no issue waits on a slower sibling's check.
    fn attempt(&self, issue: &Issue) -> Result<(String, bool), String> {
        let feature = self.branch.workspace();
        let order = Order {
            prompt: brief(issue),
            repository: feature.repository.clone(),
            branch: format!("issue/{}", issue.id),
            base: Some(feature.branch.clone()),
        };
        let workspace = provision(&order)?;
        let agent = (self.agents)(issue, &workspace, &workspace.brief(&order.prompt));
        let branch = workspace.branch.clone();
        let run = Record::new(order, workspace);
        let handle = launch(&agent, &self.runner, Arc::clone(&run))
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
        // The observation is the last thing the drainer writes, just
        // after the exit already seen — a short wait, never a long one.
        let commits = loop {
            let observed = run
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .commits
                .clone();
            if let Some(commits) = observed {
                break commits;
            }
            std::thread::sleep(OBSERVATION_POLL);
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::Node;

    fn id(number: u64) -> IssueId {
        IssueId::from(number)
    }

    fn issue(number: u64, closed: bool) -> Issue {
        Issue {
            id: id(number),
            title: format!("issue {number}"),
            closed,
        }
    }

    fn node(number: u64, closed: bool, children: &[u64], blockers: &[(u64, bool)]) -> Node {
        Node {
            issue: issue(number, closed),
            children: children.iter().copied().map(id).collect(),
            blockers: blockers
                .iter()
                .map(|&(number, closed)| issue(number, closed))
                .collect(),
        }
    }

    fn plan(feature: u64, nodes: Vec<Node>) -> Plan {
        Plan::descend(&id(feature), |asked| {
            nodes
                .iter()
                .find(|node| &node.issue.id == asked)
                .cloned()
                .ok_or_else(|| format!("no fixture for {asked}"))
        })
        .unwrap()
    }

    fn states(build: &Build) -> Vec<(&str, &State)> {
        build
            .states
            .iter()
            .map(|(id, state)| (id.0.as_str(), state))
            .collect()
    }

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
    }

    #[test]
    fn a_failure_skips_its_dependents_transitively_and_spares_the_rest() {
        let mut build = Build::new(plan(
            1,
            vec![
                node(1, false, &[2, 3, 4, 5], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
                node(4, false, &[], &[]),
                node(5, false, &[], &[(3, false)]),
            ],
        ));
        build.fail(&id(2), "the wumpus got in".to_owned());
        assert_eq!(
            states(&build),
            [
                (
                    "2",
                    &State::Failed {
                        report: "the wumpus got in".to_owned()
                    }
                ),
                ("3", &State::Skipped),
                ("4", &State::Waiting),
                ("5", &State::Skipped),
            ],
            "3 waited on 2, 5 waited on 3; 4 waited on no one"
        );
    }

    #[test]
    fn a_failure_under_a_container_strands_whoever_waits_on_the_container() {
        let mut build = Build::new(plan(
            1,
            vec![
                node(1, false, &[7, 9], &[]),
                node(7, false, &[2, 8], &[]),
                node(2, false, &[], &[]),
                node(8, false, &[], &[]),
                node(9, false, &[], &[(7, false)]),
            ],
        ));
        build.fail(&id(2), "boom".to_owned());
        assert_eq!(build.states[&id(9)], State::Skipped, "7 can never settle");
        assert_eq!(
            build.states[&id(8)],
            State::Waiting,
            "8 is still work; the build finishes the rest"
        );
    }

    #[test]
    fn skipping_never_overwrites_a_state_an_issue_earned() {
        let mut build = Build::new(plan(
            1,
            vec![
                node(1, false, &[2, 3], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
            ],
        ));
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
        let mut build = Build::new(plan(
            1,
            vec![
                node(1, false, &[2, 3], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
            ],
        ));
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
}
