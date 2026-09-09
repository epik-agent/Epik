//! The chat verbs of a feature build: `start_feature` launches one and
//! answers as soon as it is running; `feature_status` reads the record.
//!
//! The record — [`Builds`] — is a map keyed by [`RunId`], and as many
//! features build at once as are asked for: each owns a feature branch
//! nobody else writes to, and the one place they could collide —
//! merging into the default branch — is the human's, never Epik's.
//! Nothing here arbitrates. The guard that remains is git's: a build
//! holds a workspace of its feature branch for as long as it runs, and
//! a branch has one worktree, so a second build of a feature still
//! building is refused — asked of git's worktree listing before the
//! check card is raised, and answered in Epik's words naming the build
//! that holds the branch; `worktree add` beneath refuses whatever slips
//! between. A finished build has retired its workspace, and the same
//! feature builds again.
//! Launching is two-phase all the same: the run id is reserved before
//! the check card is raised, so the build has a name from the moment
//! it is asked for, and the flight fills the reservation once the
//! build is running; the map lock is only ever held for the map
//! itself, never across a card, a network push, or a git command.
//!
//! The check is a precondition, not an instruction: before anything is
//! dispatched, `start_feature` raises the question itself —
//! [`check::detect`] prefills the card, the answered command is the one
//! in force, and a decline is the skip: the build runs on observation
//! alone and the record says the branch is unchecked. Asking is never
//! a thing the persona has to remember.
//!
//! The tool answers when the build is running, not when it finishes:
//! the turn that called it ends while the build proceeds. No Agent
//! event enters any chat transcript — the [`Build`] record is the sink,
//! and `feature_status` is the door for a model. A window has another:
//! the record speaks every change to the host's [`Log`], from the
//! reservation on — [`Reserved`](Change::Reserved) before the card is
//! raised, so a build is visible from the moment it is asked for, and
//! [`Abandoned`](Change::Abandoned) with the reason when a launch fails
//! before the build runs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};

use super::check::{self, Check};
use super::merge::Branch;
use super::{Budget, Build, Feature, Issue, IssueId, Plan, RunId, build};
use crate::agent::Agent;
use crate::chat::{Answer, Ask};
use crate::forge::Forge;
use crate::git::plumbing;
use crate::job::Workspace;
use crate::monitor::{Change, Log};
use crate::tools::Tool;
use crate::tools::arg;

/// One launched feature build, as the record keeps it: the build in
/// flight, and afterwards the build that flew.
struct Flight {
    pub feature: Feature,
    /// The local clone the build runs in.
    repository: String,
    /// The feature branch the merges land on.
    branch: String,
    /// The check command in force — `None` when the user declined: the
    /// branch is unchecked, and the record says so.
    check: Option<String>,
    /// Reads the feature branch's settled tip — [`Branch::tip`], under
    /// the merge lock, so a commit a red check is about to reset away
    /// is never the answer.
    pub(super) tip: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    record: Arc<Mutex<Build>>,
}

/// One entry of the record: a launch under way — reserved before the
/// check card is raised, so status can name it before anyone is asked
/// — or the build itself.
enum Entry {
    /// The reservation: the launch is validating, asking, establishing.
    Starting {
        feature: Feature,
        repository: String,
        branch: String,
    },
    /// The build, running or finished.
    Flight(Flight),
}

/// The record of feature builds, keyed by run id. Finished builds stay
/// readable — status outlives completion. Every change to the record
/// is spoken to `log`, the host's one.
pub struct Builds {
    entries: Mutex<BTreeMap<RunId, Entry>>,
    /// The next run id. A counter rather than the map's maximum, so a
    /// released reservation never recycles its id onto a later build.
    next: std::sync::atomic::AtomicU64,
    log: Arc<Log>,
}

impl Builds {
    /// An empty record whose builds speak to `log`.
    #[must_use]
    pub fn new(log: Arc<Log>) -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            next: std::sync::atomic::AtomicU64::new(1),
            log,
        }
    }

    /// Reserves the next run id for a launch — the insert under the map
    /// lock, held for nothing else. The reservation exists so the build
    /// has a name before any card is raised or any branch established:
    /// Reserved is recorded here, so a watcher sees the build from the
    /// moment it is asked for, and status answers for it meanwhile.
    /// Fill it when the build is running; abandon it, or drop it
    /// unfilled, and the entry goes, though its id is never reused.
    fn reserve(
        self: &Arc<Self>,
        feature: Feature,
        repository: String,
        branch: String,
        base: Option<String>,
    ) -> Reservation {
        let run = {
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            let run = RunId(self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
            entries.insert(
                run,
                Entry::Starting {
                    feature: feature.clone(),
                    repository: repository.clone(),
                    branch: branch.clone(),
                },
            );
            run
        };
        self.log.record(Change::Reserved {
            run,
            feature: feature.0,
            repository,
            branch,
            base,
        });
        Reservation {
            builds: Arc::clone(self),
            run,
            standing: Standing::Held,
        }
    }
}

/// How a reservation ended, if it has.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Standing {
    /// The launch is under way.
    Held,
    /// The build is running: the flight took the reservation's place.
    Filled,
    /// The launch failed, and the log has been told why.
    Abandoned,
}

/// A run id held while a launch is under way: taken before the check
/// card is raised, filled with the [`Flight`] once the build is
/// running. Abandoned — the launch failed — it releases its entry with
/// the reason recorded; dropped unfilled without a word, it records the
/// abandonment with words of its own, so no reservation ever vanishes
/// from the log unexplained.
struct Reservation {
    builds: Arc<Builds>,
    run: RunId,
    standing: Standing,
}

impl fmt::Debug for Reservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reservation")
            .field("run", &self.run)
            .field("standing", &self.standing)
            .finish_non_exhaustive()
    }
}

impl Reservation {
    /// The reserved run id.
    #[cfg(test)]
    const fn run(&self) -> RunId {
        self.run
    }

    /// The build is running: the flight takes the reservation's place
    /// in the record.
    fn fill(mut self, flight: Flight) -> RunId {
        self.builds
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(self.run, Entry::Flight(flight));
        self.standing = Standing::Filled;
        self.run
    }

    /// Fills the reservation with the flight `launch` yields — or, when
    /// the launch fails, abandons it with the launch's own words, so no
    /// path between reserving and running can fail silently. Whatever
    /// else `launch` yields comes back beside the run id.
    fn fill_with<T>(
        self,
        launch: impl FnOnce(RunId) -> Result<(Flight, T), String>,
    ) -> Result<(RunId, T), String> {
        match launch(self.run) {
            Ok((flight, extra)) => Ok((self.fill(flight), extra)),
            Err(words) => {
                self.abandon(words.clone());
                Err(words)
            }
        }
    }

    /// The launch failed: `reason` is recorded, and the entry released.
    fn abandon(mut self, reason: String) {
        self.builds.log.record(Change::Abandoned {
            run: self.run,
            reason,
        });
        self.standing = Standing::Abandoned;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        match self.standing {
            Standing::Filled => return,
            Standing::Abandoned => {}
            Standing::Held => self.builds.log.record(Change::Abandoned {
                run: self.run,
                reason: "the launch ended without saying why".to_owned(),
            }),
        }
        self.builds
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.run);
    }
}

/// The wording of the check card, composed by the machinery — the
/// persona never decides whether or how to ask.
fn asked(feature: &Feature, branch: &str) -> String {
    format!(
        "Feature {feature} builds on branch {branch}, and every merge onto it is judged by \
         one command run in the feature workspace: the merge lands only when the command \
         exits 0. Confirm or edit this repository's own check command, or skip the check \
         to build on observation alone."
    )
}

/// The answer, read: a confirmed command is the one in force; a decline
/// is the skip. An empty command is a decline wearing a keyboard —
/// `sh -c ""` exits 0, a check that checks nothing — and any other
/// answer kind never comes off a check card, so it reads as the skip
/// too.
fn confirmed(answer: &Answer) -> Option<Check> {
    match answer {
        Answer::Check { command } if !command.trim().is_empty() => Some(Check {
            command: command.clone(),
        }),
        _ => None,
    }
}

/// The `start_feature` tool over the host's seams: `plan` reads a
/// feature's shape out of the tracker, `forge` names where the feature
/// branch's pushes go, `agents` yields the per-issue Agent factory one
/// build uses, and
/// `asker` is the whole question modality — it takes the check card and
/// comes back with the answer, however long that takes. Each is fallible
/// where the host's world is: every refusal is words the model reads.
pub fn start_feature<F, M>(
    builds: Arc<Builds>,
    budget: Arc<Budget>,
    plan: impl Fn(&str, &Feature) -> Result<Plan, String> + 'static,
    forge: impl Fn(&str) -> Result<F, String> + 'static,
    agents: impl Fn() -> Result<M, String> + 'static,
    asker: impl Fn(Ask) -> Answer + 'static,
) -> Tool
where
    F: Forge + Send + Sync + 'static,
    M: Fn(&Issue, &Workspace, &str) -> Result<Agent, String> + Send + Sync + 'static,
{
    Tool::new(
        "start_feature",
        "Builds a whole feature — an issue and the sub-issues beneath it — in a local clone: \
         up to four coding agents work the ready issues at once, each finished issue merges \
         onto one feature branch, and the branch is pushed as work lands. Asks the user, in \
         the window, for the check command every merge must pass — never ask that yourself. \
         Returns as soon as the build is running, with the run id that names this build; \
         several feature builds may run at once, each with its own run id, and one feature \
         builds once at a time. The build then proceeds on its own for a long time, so tell \
         the user the branch and run id and end your turn — the user will ask how it is \
         going, and feature_status answers for the run.",
        json!({
            "type": "object",
            "properties": {
                "repo": {
                    "type": "string",
                    "description": "The repository as the tracker names it: one owner/name string on GitHub.",
                },
                "repository": {
                    "type": "string",
                    "description": "The absolute path of the local clone to build in (git_clone one first if none exists).",
                },
                "feature": {
                    "type": "integer",
                    "description": "The feature issue's number.",
                },
                "branch": {
                    "type": "string",
                    "description": "The feature branch's name; feature-<number> when omitted.",
                },
                "base": {
                    "type": "string",
                    "description": "The branch or commit the feature branch starts from; the repository's default branch when omitted.",
                },
            },
            "required": ["repo", "repository", "feature"],
        }),
        Box::new(move |arguments| {
            let repo = arg::non_empty(arguments, "repo")?;
            let repository = arg::non_empty(arguments, "repository")?.to_owned();
            let feature = Feature(IssueId::from(arg::number(arguments, "feature")?));
            let branch = match arg::optional(arguments, "branch")? {
                Some(branch) => branch.to_owned(),
                None => feature.branch(),
            };
            let named_base = arg::optional(arguments, "base")?.map(str::to_owned);
            // The clone is read before anyone is asked anything —
            // detection and the default base both need it to be real —
            // under provisioning's own rule, so a relative path gets
            // exactly start_build's refusal.
            crate::job::locate(&repository)?;
            let base = match named_base {
                Some(base) => base,
                None => plumbing(&["-C", &repository, "symbolic-ref", "--short", "HEAD"])
                    .map_err(|words| format!("the repository has no default branch: {words}"))?
                    .trim()
                    .to_owned(),
            };
            // The reservation: from here the build has a name — before
            // any card is raised — and any failure below abandons it,
            // with its words, on the way out.
            let reservation = builds.reserve(
                feature.clone(),
                repository.clone(),
                branch.clone(),
                Some(base.clone()),
            );
            let (run, (problems, issues, command)) = reservation.fill_with(|run| {
                // Git is the guard — a feature branch has one worktree,
                // and establishing beneath would be refused — but asked
                // here first, so a second build of a held branch is
                // refused in Epik's words, naming the holder, before
                // anyone is asked anything.
                if let Some(holder) = crate::job::held_by(&repository, &branch)? {
                    return Err(format!(
                        "feature branch {branch:?} is already checked out by a build at {}; \
                         one build owns a feature branch at a time",
                        holder.display()
                    ));
                }
                let plan = plan(repo, &feature)?;
                let problems = plan.problems();
                let issues = plan.work(&BTreeSet::new()).len();
                let forge = forge(repo)?;
                let agents = agents()?;
                // The check is a precondition: raised here, before
                // anything is dispatched, with detection's proposal
                // prefilled.
                let proposal = check::detect(Path::new(&repository)).map(|check| check.command);
                let answer = asker(Ask::Check {
                    prompt: asked(&feature, &branch),
                    proposal,
                });
                let check = confirmed(&answer);
                let command = check.as_ref().map(|check| check.command.clone());
                // Establishing pushes over the network and the build
                // starts machinery: both happen outside every map lock.
                // A branch taken between the question above and here is
                // refused by git inside establishing, in its words.
                let established = Arc::new(Branch::establish(
                    &repository,
                    &branch,
                    &base,
                    forge,
                    check,
                )?);
                let tip = {
                    let established = Arc::clone(&established);
                    move || established.tip()
                };
                let record = build(
                    run,
                    Arc::clone(&builds.log),
                    plan,
                    established,
                    agents,
                    Arc::clone(&budget),
                );
                let flight = Flight {
                    feature: feature.clone(),
                    repository: repository.clone(),
                    branch: branch.clone(),
                    check: command.clone(),
                    tip: Arc::new(tip),
                    record,
                };
                Ok((flight, (problems, issues, command)))
            })?;
            Ok(json!({
                "started": true,
                "run": run,
                "feature": feature,
                "repository": repository,
                "branch": branch,
                "base": base,
                "check": command,
                "issues": issues,
                "problems": problems,
            }))
        }),
    )
}

/// The `feature_status` tool over the record: each issue's state, the
/// feature branch's tip, and the report of whatever failed.
pub fn feature_status(builds: Arc<Builds>) -> Tool {
    Tool::new(
        "feature_status",
        "How a feature build started by start_feature is going — or went: each issue's \
         title and state (waiting, running, merging, merged, failed with its report, skipped \
         with its reason), the feature branch's tip, the check in force (null means the \
         branch is unchecked), and any problems with the plan's shape. Several feature \
         builds may run at once: pass run — the run id start_feature answered with — to \
         read a particular build; with run omitted, the most recently started build answers, \
         whichever feature it is. Call it once when the user asks and report what it says; \
         never poll it in a loop — the build runs on its own.",
        json!({
            "type": "object",
            "properties": {
                "run": {
                    "type": "integer",
                    "description": "The run id start_feature answered with, selecting that build; the most recently started build when omitted.",
                },
            },
        }),
        Box::new(move |arguments| {
            // Cloned out under the map lock; the tip read — which waits
            // on the merge lock — runs after it is gone.
            let (run, feature, repository, branch, check, tip, build) = {
                let entries = builds
                    .entries
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let (run, entry) = match arguments["run"].as_u64() {
                    Some(asked) => entries
                        .get_key_value(&RunId(asked))
                        .ok_or_else(|| format!("no feature build has run id {asked}"))?,
                    None => match entries.iter().next_back() {
                        Some(latest) => latest,
                        None => {
                            return Ok(json!({
                                "run": Value::Null,
                                "note": "no feature build has been started",
                            }));
                        }
                    },
                };
                let flight = match entry {
                    Entry::Starting {
                        feature,
                        repository,
                        branch,
                    } => {
                        return Ok(json!({
                            "run": run,
                            "feature": feature,
                            "repository": repository,
                            "branch": branch,
                            "finished": false,
                            "note": "the build has not started: the check card is \
                                     unanswered or the feature branch is being established",
                        }));
                    }
                    Entry::Flight(flight) => flight,
                };
                (
                    *run,
                    flight.feature.clone(),
                    flight.repository.clone(),
                    flight.branch.clone(),
                    flight.check.clone(),
                    Arc::clone(&flight.tip),
                    flight
                        .record
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone(),
                )
            };
            let titles: BTreeMap<&IssueId, &str> = build
                .plan
                .tree
                .nodes()
                .map(|issue| (&issue.id, issue.title.as_str()))
                .collect();
            let issues: Vec<Value> = build
                .states
                .iter()
                .map(|(id, state)| {
                    let mut entry =
                        serde_json::to_value(state).expect("a state serializes to an object");
                    if let Value::Object(entry) = &mut entry {
                        entry.insert("id".to_owned(), json!(id));
                        entry.insert("title".to_owned(), json!(titles.get(id)));
                    }
                    entry
                })
                .collect();
            // The settled tip, through the merge lock — never a raw read
            // of a ref a red check may be about to reset.
            let tip = tip().map(Value::String).unwrap_or(Value::Null);
            Ok(json!({
                "run": run,
                "feature": feature,
                "repository": repository,
                "branch": branch,
                "tip": tip,
                "check": check,
                "finished": build.finished(),
                "problems": build.problems,
                "issues": issues,
            }))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::super::State;
    use super::super::fixtures::{committing, id, node, plan, seven_holding_eight};
    use super::*;
    use crate::monitor::{Stage, folded};
    use crate::testing::{Local, Scratch, seeded};
    use crate::tools::Registry;

    /// An empty record on a fresh log.
    fn builds() -> Arc<Builds> {
        Arc::new(Builds::new(Arc::new(Log::new())))
    }

    /// An Agent that never starts: its program does not exist, so every
    /// dispatch fails at the start — which is exactly what lets the tool
    /// tests run without an engine. The end-to-end build rides in the
    /// feature build's own tests.
    fn unreachable(_: &Issue, _: &Workspace, _: &str) -> Result<Agent, String> {
        Agent::new(vec!["/nonexistent/agent".to_owned()], "/", [], None)
            .map_err(|error| format!("{error:#}"))
    }

    fn git(args: &[&str]) -> String {
        plumbing(args).unwrap()
    }

    /// Removes every linked worktree a build left behind, so a scratch
    /// drop is enough.
    fn tidy(repository: &str) {
        let listed = git(&["-C", repository, "worktree", "list", "--porcelain"]);
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

    /// A flight over a hand-built record, for exercising the map without
    /// any machinery.
    fn flight(feature: u64, branch: &str, repository: &str, states: &[(u64, State)]) -> Flight {
        let plan = plan(
            feature,
            vec![
                node(feature, false, &[feature + 1], &[]),
                node(feature + 1, false, &[], &[]),
            ],
        );
        let mut build = Build {
            run: RunId(0),
            log: Arc::new(Log::new()),
            problems: plan.problems(),
            plan,
            states: BTreeMap::new(),
            runs: BTreeMap::new(),
        };
        for (number, state) in states {
            build.set(&id(*number), state.clone());
        }
        Flight {
            feature: Feature(id(feature)),
            repository: repository.to_owned(),
            branch: branch.to_owned(),
            check: None,
            tip: Arc::new(|| Err("no branch was established".to_owned())),
            record: Arc::new(Mutex::new(build)),
        }
    }

    /// The registry a turn would carry, over injected seams: a fixture
    /// plan per feature number, a bare-directory forge, the Agent
    /// factory `agents`, and an asker the test scripts.
    fn registry<M>(
        builds: &Arc<Builds>,
        plans: impl IntoIterator<Item = (u64, Plan)>,
        agents: M,
        remote: String,
        asker: impl Fn(Ask) -> Answer + 'static,
    ) -> Registry
    where
        M: Fn(&Issue, &Workspace, &str) -> Result<Agent, String> + Clone + Send + Sync + 'static,
    {
        let plans: BTreeMap<Feature, Plan> = plans
            .into_iter()
            .map(|(feature, plan)| (Feature(id(feature)), plan))
            .collect();
        let mut registry = Registry::default();
        registry.register(start_feature(
            Arc::clone(builds),
            Budget::new(),
            move |_, feature| {
                plans
                    .get(feature)
                    .cloned()
                    .ok_or_else(|| format!("no fixture plan for feature {feature}"))
            },
            move |_| Ok(Local(remote.clone())),
            move || Ok(agents.clone()),
            asker,
        ));
        registry.register(feature_status(Arc::clone(builds)));
        registry
    }

    /// Polls `feature_status` for run `run` until it says finished.
    fn eventually_finished(registry: &Registry, run: u64) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let arguments = json!({ "run": run }).to_string();
        loop {
            let status = registry.dispatch("feature_status", &arguments).unwrap();
            if status["finished"] == json!(true) {
                return status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the build to finish: {status}"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    /// A reservation for feature `feature`, named as a start names it.
    fn reserve(builds: &Arc<Builds>, feature: u64) -> Reservation {
        builds.reserve(
            Feature(id(feature)),
            "/r".to_owned(),
            format!("feature-{feature}"),
            None,
        )
    }

    /// The run ids the record holds, in order.
    fn runs(builds: &Builds) -> Vec<RunId> {
        builds.entries.lock().unwrap().keys().copied().collect()
    }

    /// `start_feature` with only its required arguments.
    fn start(registry: &Registry, repository: &str, feature: u64) -> Result<Value, String> {
        registry.dispatch(
            "start_feature",
            &json!({ "repo": "o/r", "repository": repository, "feature": feature }).to_string(),
        )
    }

    /// A Cargo.toml at `work`, so detection has a check to propose.
    fn cargo_crate(work: &str) {
        std::fs::write(
            Path::new(work).join("Cargo.toml"),
            "[package]\nname = \"wumpus\"\n",
        )
        .unwrap();
    }

    /// A registry carrying only `feature_status` over `builds`.
    fn status_registry(builds: &Arc<Builds>) -> Registry {
        let mut registry = Registry::default();
        registry.register(feature_status(Arc::clone(builds)));
        registry
    }

    /// Reserves for a fixture flight and fills at once — the shorthand
    /// most map tests want.
    fn admitted(builds: &Arc<Builds>, the_flight: Flight) -> RunId {
        builds
            .reserve(
                the_flight.feature.clone(),
                the_flight.repository.clone(),
                the_flight.branch.clone(),
                None,
            )
            .fill(the_flight)
    }

    /// A second flight while the first is still running takes the next
    /// id, and the two coexist in the record: each readable by its run
    /// id, the latest answering when none is named.
    #[test]
    fn a_second_flight_joins_the_first_in_the_record() {
        let builds = builds();
        let first = admitted(
            &builds,
            flight(7, "feature-7", "/r", &[(8, State::Running)]),
        );
        let second = admitted(
            &builds,
            flight(9, "feature-9", "/r", &[(10, State::Running)]),
        );
        assert_eq!((first, second), (RunId(1), RunId(2)));
        assert_eq!(runs(&builds), [RunId(1), RunId(2)]);

        let registry = status_registry(&builds);
        let status = registry.dispatch("feature_status", r#"{"run":1}"#).unwrap();
        assert_eq!(status["feature"], json!("7"));
        assert_eq!(status["finished"], json!(false));
        let latest = registry.dispatch("feature_status", "{}").unwrap();
        assert_eq!(latest["run"], json!(2));
        assert_eq!(latest["feature"], json!("9"));
    }

    /// A reservation holds nothing shut: while one is held, the next
    /// start reserves the next id. A dropped reservation releases its
    /// own entry and no other, and its id is never reused.
    #[test]
    fn reservations_coexist_and_a_dropped_one_never_recycles_its_id() {
        let builds = builds();
        let first = reserve(&builds, 7);
        let second = reserve(&builds, 9);
        assert_eq!((first.run(), second.run()), (RunId(1), RunId(2)));
        assert_eq!(runs(&builds), [RunId(1), RunId(2)]);

        drop(first);
        assert_eq!(runs(&builds), [RunId(2)]);
        assert_eq!(reserve(&builds, 7).run(), RunId(3));
    }

    /// A reservation is spoken from the moment it is taken, and its end
    /// is spoken too: abandoned with the launch's words, or — dropped
    /// without any — with words of the reservation's own. Either way
    /// the fold's stage is Abandoned with the reason.
    #[test]
    fn a_reservation_is_recorded_and_its_abandonment_says_why() {
        let builds = builds();
        let reservation = reserve(&builds, 7);
        let entries = builds.log.since(0);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].change,
            Change::Reserved {
                run: RunId(1),
                feature: id(7),
                repository: "/r".to_owned(),
                branch: "feature-7".to_owned(),
                base: None,
            }
        );
        reservation.abandon("the tracker is down".to_owned());
        assert!(builds.entries.lock().unwrap().is_empty(), "released");
        let progress = folded(&builds.log);
        assert_eq!(
            *progress.watch(RunId(1)).unwrap().stage(),
            Stage::Abandoned {
                reason: "the tracker is down".to_owned()
            }
        );

        drop(reserve(&builds, 7));
        let progress = folded(&builds.log);
        let Stage::Abandoned { reason } = progress.watch(RunId(2)).unwrap().stage() else {
            panic!("a dropped reservation is abandoned too");
        };
        assert!(reason.contains("without saying why"), "{reason}");
    }

    /// The launch that fails after reserving — here the plan cannot be
    /// read — leaves Reserved then Abandoned with the failure's words,
    /// and nothing else: no Started, no card raised.
    #[test]
    fn a_launch_that_fails_after_reserving_is_abandoned_with_its_words() {
        let scratch = Scratch::new("abandoned");
        let (work, remote) = seeded(&scratch);
        let builds = builds();
        let mut registry = Registry::default();
        registry.register(start_feature(
            Arc::clone(&builds),
            Budget::new(),
            |_, _| Err("the tracker is down".to_owned()),
            move |_| Ok(Local(remote.clone())),
            || Ok(unreachable),
            |_| panic!("no card is raised when the plan cannot be read"),
        ));

        let error = start(&registry, &work, 7).unwrap_err();
        assert_eq!(error, "the tracker is down");
        assert!(builds.entries.lock().unwrap().is_empty(), "released");

        let changes: Vec<&str> = builds
            .log
            .since(0)
            .iter()
            .map(|entry| match &entry.change {
                Change::Reserved { base, .. } => {
                    assert_eq!(base.as_deref(), Some("main"));
                    "reserved"
                }
                Change::Abandoned { reason, .. } => {
                    assert_eq!(reason, "the tracker is down");
                    "abandoned"
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(changes, ["reserved", "abandoned"]);
        assert_eq!(
            *folded(&builds.log).watch(RunId(1)).unwrap().stage(),
            Stage::Abandoned {
                reason: "the tracker is down".to_owned()
            }
        );

        // The spent id is never reused: the next reservation takes the
        // next one.
        assert_eq!(reserve(&builds, 9).run(), RunId(2));
    }

    #[test]
    fn a_confirmed_command_is_the_one_in_force_and_anything_else_is_the_skip() {
        assert_eq!(
            confirmed(&Answer::Check {
                command: "cargo test".to_owned()
            }),
            Some(Check {
                command: "cargo test".to_owned()
            })
        );
        assert_eq!(
            confirmed(&Answer::Check {
                command: "   ".to_owned()
            }),
            None,
            "an empty command is a check that checks nothing"
        );
        assert_eq!(confirmed(&Answer::Declined), None);
    }

    #[test]
    fn the_unnamed_branch_is_feature_n_and_the_unnamed_base_is_the_default_branch() {
        let scratch = Scratch::new("defaults");
        let (work, remote) = seeded(&scratch);
        let main_tip = git(&["-C", &work, "rev-parse", "refs/heads/main"])
            .trim()
            .to_owned();
        let builds = builds();
        let registry = registry(
            &builds,
            [(7, seven_holding_eight())],
            unreachable,
            remote,
            |_| Answer::Declined,
        );

        let started = start(&registry, &work, 7).unwrap();
        assert_eq!(started["started"], json!(true));
        assert_eq!(started["run"], json!(1));
        assert_eq!(started["branch"], "feature-7", "named for the issue");
        assert_eq!(started["base"], "main", "the repository's default branch");
        assert_eq!(started["issues"], json!(1));
        assert_eq!(
            git(&["-C", &work, "rev-parse", "refs/heads/feature-7"]).trim(),
            main_tip,
            "the feature branch was cut at the base"
        );

        let status = eventually_finished(&registry, 1);
        assert_eq!(status["branch"], "feature-7");
        assert_eq!(
            status["tip"],
            json!(main_tip),
            "nothing landed: an Agent that never starts"
        );

        // The log told the whole story: Reserved before the card,
        // Started, one Moved per state 8 passed through — Running, then
        // its Failed end — and Finished once. The fold is the record.
        let record = {
            let entries = builds.entries.lock().unwrap();
            let Entry::Flight(flight) = &entries[&RunId(1)] else {
                panic!("the filled entry is a flight");
            };
            flight.record.lock().unwrap().clone()
        };
        let entries = wait_for_finished(&builds.log);
        let words: Vec<&str> = entries
            .iter()
            .map(|entry| match &entry.change {
                Change::Reserved { .. } => "reserved",
                Change::Started { states, check, .. } => {
                    assert_eq!(states[&id(8)], State::Waiting);
                    assert_eq!(*check, None);
                    "started"
                }
                Change::Moved { issue, state, .. } => {
                    assert_eq!(*issue, id(8));
                    match state {
                        State::Running => "running",
                        State::Failed { .. } => {
                            assert_eq!(&record.states[&id(8)], state);
                            "failed"
                        }
                        other => panic!("{other:?}"),
                    }
                }
                Change::Finished { .. } => "finished",
                Change::Abandoned { .. } => "abandoned",
            })
            .collect();
        assert_eq!(
            words,
            ["reserved", "started", "running", "failed", "finished"]
        );
        let progress = folded(&builds.log);
        let watch = progress.watch(RunId(1)).unwrap();
        assert_eq!(*watch.states(), record.states);
        assert_eq!(*watch.stage(), Stage::Finished);
        assert_eq!(watch.branch(), "feature-7");
        assert_eq!(watch.base(), Some("main"));
        tidy(&work);
    }

    /// The log once the run's Finished has landed — which follows the
    /// record's own `finished` by the width of the scheduler's wakeup,
    /// so status saying finished is not yet the log saying so.
    fn wait_for_finished(log: &Log) -> Vec<crate::monitor::Entry> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut cursor = 0;
        loop {
            let heard = log.wait_after(cursor, std::time::Duration::from_secs(1));
            if heard
                .iter()
                .any(|entry| matches!(entry.change, Change::Finished { .. }))
            {
                return log.since(0);
            }
            cursor += heard.len() as u64;
            assert!(
                std::time::Instant::now() < deadline,
                "the log never said finished: {:?}",
                log.since(0)
            );
        }
    }

    #[test]
    fn the_check_card_is_prefilled_from_detection_and_the_answer_is_in_force() {
        let scratch = Scratch::new("prefill");
        let (work, remote) = seeded(&scratch);
        cargo_crate(&work);
        let asked = Arc::new(Mutex::new(None::<Ask>));
        let builds = builds();
        let registry = registry(
            &builds,
            [(7, seven_holding_eight())],
            unreachable,
            remote,
            {
                let asked = Arc::clone(&asked);
                move |question| {
                    *asked.lock().unwrap() = Some(question);
                    Answer::Check {
                        command: "cargo test --workspace".to_owned(),
                    }
                }
            },
        );

        let started = registry
            .dispatch(
                "start_feature",
                &json!({
                    "repo": "o/r",
                    "repository": work,
                    "feature": 7,
                    "branch": "wumpus",
                    "base": "main",
                })
                .to_string(),
            )
            .unwrap();
        assert_eq!(started["branch"], "wumpus", "a named branch is kept");
        assert_eq!(
            started["check"],
            json!("cargo test --workspace"),
            "the answered command is the one in force, not the proposal"
        );

        let Some(Ask::Check { prompt, proposal }) = asked.lock().unwrap().clone() else {
            panic!("the check card was never raised");
        };
        assert_eq!(
            proposal,
            Some("cargo test".to_owned()),
            "detection prefilled"
        );
        assert!(prompt.contains("Feature 7"), "{prompt}");
        assert!(prompt.contains("wumpus"), "{prompt}");

        let status = eventually_finished(&registry, 1);
        assert_eq!(status["check"], json!("cargo test --workspace"));
        tidy(&work);
    }

    #[test]
    fn a_decline_skips_the_check_and_the_record_says_unchecked() {
        let scratch = Scratch::new("decline");
        let (work, remote) = seeded(&scratch);
        cargo_crate(&work);
        let builds = builds();
        let registry = registry(
            &builds,
            [(7, seven_holding_eight())],
            unreachable,
            remote,
            |_| Answer::Declined,
        );

        let started = start(&registry, &work, 7).unwrap();
        assert_eq!(started["check"], Value::Null, "unchecked, and said so");

        let status = eventually_finished(&registry, 1);
        assert_eq!(status["check"], Value::Null);
        tidy(&work);
    }

    /// Feature 9 holding 10: a second feature whose ids collide with
    /// none of `seven_holding_eight`'s, so the two build in one clone.
    fn nine_holding_ten() -> Plan {
        plan(
            9,
            vec![node(9, false, &[10], &[]), node(10, false, &[], &[])],
        )
    }

    /// The log's story of run `run`, one word per change.
    fn story(log: &Log, run: RunId) -> Vec<&'static str> {
        log.since(0)
            .iter()
            .filter(|entry| entry.change.run() == run)
            .map(|entry| match entry.change {
                Change::Reserved { .. } => "reserved",
                Change::Started { .. } => "started",
                Change::Moved { .. } => "moved",
                Change::Finished { .. } => "finished",
                Change::Abandoned { .. } => "abandoned",
            })
            .collect()
    }

    /// Two starts on one record, in one clone, while the first is still
    /// running: both succeed, each with its own run id; both are in the
    /// log as Reserved before either has Finished; both answer status
    /// by run id; and both complete with their leaf Merged.
    #[test]
    fn two_features_start_and_finish_side_by_side() {
        let scratch = Scratch::new("side-by-side");
        let (work, remote) = seeded(&scratch);
        let gate = Path::new(scratch.path()).join("release");
        let builds = builds();
        let registry = registry(
            &builds,
            [(7, seven_holding_eight()), (9, nine_holding_ten())],
            committing(Some(gate.clone())),
            remote,
            |_| Answer::Declined,
        );

        let first = start(&registry, &work, 7).unwrap();
        let second = start(&registry, &work, 9).unwrap();
        assert_eq!((&first["run"], &second["run"]), (&json!(1), &json!(2)));
        assert_eq!(second["branch"], "feature-9");

        // Both held at Running behind the gate: two reservations, two
        // starts, and not a Finished between them.
        assert_eq!(story(&builds.log, RunId(1))[..2], ["reserved", "started"]);
        assert_eq!(story(&builds.log, RunId(2))[..2], ["reserved", "started"]);
        for run in [1, 2] {
            let status = registry
                .dispatch("feature_status", &json!({ "run": run }).to_string())
                .unwrap();
            assert_eq!(status["run"], json!(run));
            assert_eq!(status["finished"], json!(false), "{status}");
        }

        std::fs::write(&gate, "").unwrap();
        for (run, leaf) in [(1, "8"), (2, "10")] {
            let status = eventually_finished(&registry, run);
            let issues = status["issues"].as_array().unwrap();
            assert_eq!(issues.len(), 1, "{status}");
            assert_eq!(issues[0]["id"], json!(leaf));
            assert_eq!(issues[0]["state"], json!("merged"), "{status}");
        }
        tidy(&work);
    }

    /// A second start of a feature whose branch a build still holds is
    /// refused before anyone is asked: git's worktree listing is the
    /// guard, and the refusal is Epik's sentence, naming the branch
    /// and the workspace that holds it. The log says Reserved then
    /// Abandoned with that sentence for the second run; the first run
    /// is untouched and finishes.
    #[test]
    fn a_second_start_of_a_held_feature_branch_is_refused_before_anyone_is_asked() {
        let scratch = Scratch::new("twice");
        let (work, remote) = seeded(&scratch);
        let gate = Path::new(scratch.path()).join("release");
        let builds = builds();
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let registry = registry(
            &builds,
            [(7, seven_holding_eight())],
            committing(Some(gate.clone())),
            remote,
            {
                let asked = Arc::clone(&asked);
                move |_| {
                    assert_eq!(
                        asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                        0,
                        "the second start asks nobody"
                    );
                    Answer::Declined
                }
            },
        );

        assert_eq!(start(&registry, &work, 7).unwrap()["run"], json!(1));
        let holder = crate::job::held_by(&work, "feature-7").unwrap().unwrap();
        let refused = start(&registry, &work, 7).unwrap_err();
        assert!(
            refused
                .starts_with("feature branch \"feature-7\" is already checked out by a build at "),
            "{refused}"
        );
        assert!(refused.contains(&holder.display().to_string()), "{refused}");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);

        assert_eq!(runs(&builds), [RunId(1)], "run 2 released its entry");
        assert_eq!(story(&builds.log, RunId(2)), ["reserved", "abandoned"]);
        let progress = folded(&builds.log);
        assert_eq!(
            *progress.watch(RunId(2)).unwrap().stage(),
            Stage::Abandoned { reason: refused }
        );
        assert_eq!(*progress.watch(RunId(1)).unwrap().stage(), Stage::Building);
        let status = registry.dispatch("feature_status", r#"{"run":1}"#).unwrap();
        assert_eq!(status["finished"], json!(false), "{status}");

        std::fs::write(&gate, "").unwrap();
        let status = eventually_finished(&registry, 1);
        assert_eq!(status["issues"][0]["state"], json!("merged"), "{status}");
        tidy(&work);
    }

    #[test]
    fn what_is_not_a_repository_is_refused_before_anyone_is_asked() {
        let builds = builds();
        let registry = registry(
            &builds,
            [(7, plan(7, vec![node(7, false, &[], &[])]))],
            unreachable,
            "/nonexistent/remote.git".to_owned(),
            |_| panic!("no card is raised for a bad repository"),
        );
        let error = start(&registry, "/nonexistent/clone", 7).unwrap_err();
        assert!(error.contains("not a git repository"), "{error}");
        assert!(builds.entries.lock().unwrap().is_empty());

        // A relative path gets provisioning's own refusal, not a
        // cwd-dependent build.
        let error = start(&registry, "relative/clone", 7).unwrap_err();
        assert!(error.contains("absolute"), "{error}");

        // An empty optional argument is a worded refusal, not a card.
        let error = registry
            .dispatch(
                "start_feature",
                &json!({ "repo": "o/r", "repository": "/nonexistent/clone", "feature": 7, "branch": " " })
                    .to_string(),
            )
            .unwrap_err();
        assert!(error.contains("branch"), "{error}");
        assert!(error.contains("non-empty"), "{error}");
        assert!(builds.entries.lock().unwrap().is_empty());
    }

    #[test]
    fn status_reads_states_titles_the_tip_and_the_failure_reports() {
        let scratch = Scratch::new("status");
        let (work, _) = seeded(&scratch);
        git(&["-C", &work, "branch", "feature-100", "main"]);
        let tip = git(&["-C", &work, "rev-parse", "refs/heads/feature-100"])
            .trim()
            .to_owned();
        let builds = builds();
        let mut settled = flight(
            100,
            "feature-100",
            &work,
            &[(
                101,
                State::Failed {
                    report: "the wumpus got in".to_owned(),
                },
            )],
        );
        settled.check = Some("cargo test".to_owned());
        settled.tip = Arc::new({
            let tip = tip.clone();
            move || Ok(tip.clone())
        });
        admitted(&builds, settled);

        let registry = status_registry(&builds);
        let status = registry.dispatch("feature_status", "{}").unwrap();
        assert_eq!(status["run"], json!(1));
        assert_eq!(status["feature"], json!("100"));
        assert_eq!(status["branch"], "feature-100");
        assert_eq!(status["tip"], json!(tip));
        assert_eq!(status["check"], json!("cargo test"));
        assert_eq!(status["finished"], json!(true));
        let issues = status["issues"].as_array().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0]["id"], json!("101"));
        assert_eq!(issues[0]["title"], json!("issue 101"));
        assert_eq!(issues[0]["state"], json!("failed"));
        assert_eq!(issues[0]["report"], json!("the wumpus got in"));

        let named = registry.dispatch("feature_status", r#"{"run":1}"#).unwrap();
        assert_eq!(named["run"], json!(1));
        let missing = registry
            .dispatch("feature_status", r#"{"run":9}"#)
            .unwrap_err();
        assert!(missing.contains("run id 9"), "{missing}");
    }

    #[test]
    fn status_with_no_builds_says_so_in_an_ok_answer() {
        let registry = status_registry(&builds());
        let status = registry.dispatch("feature_status", "{}").unwrap();
        assert_eq!(status["run"], Value::Null);
        assert_eq!(status["note"], json!("no feature build has been started"));
    }

    /// A launch still under way — the reservation phase, the card up or
    /// the branch establishing — answers status without blocking on
    /// anything the launch holds.
    #[test]
    fn status_of_a_reserved_launch_says_it_has_not_started() {
        let builds = builds();
        let reservation = reserve(&builds, 7);
        let registry = status_registry(&builds);

        let status = registry.dispatch("feature_status", "{}").unwrap();
        assert_eq!(status["run"], json!(1));
        assert_eq!(status["feature"], json!("7"));
        assert_eq!(status["branch"], "feature-7");
        assert_eq!(status["finished"], json!(false));
        assert!(
            status["note"].as_str().unwrap().contains("not started"),
            "{status}"
        );
        drop(reservation);
    }
}
