//! The chat verbs of a feature build: `start_feature` launches one and
//! answers as soon as it is running; `feature_status` reads the record.
//!
//! The record — [`Builds`] — is a map keyed by [`RunId`], and
//! [`Builds::reserve`] refuses while a build is in flight. The map
//! shape is deliberate: one feature at a time is policy, visibly the
//! unstable part of the decision, so it lives in one refusal — a line
//! to delete, never a shape to migrate. The refusal is typed,
//! [`Refused`], and names the build in flight. Launching is
//! two-phase: the run id is reserved before the check card is raised —
//! so two concurrent starts can never both ask, and no answered card is
//! ever discarded — and the flight fills the reservation once the build
//! is running; the map lock is only ever held for the map itself, never
//! across a card, a network push, or a git command.
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
//! and `feature_status` is the door.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::{Value, json};

use super::check::{self, Check};
use super::merge::Branch;
use super::{Budget, Build, Feature, Issue, IssueId, Plan, build};
use crate::agent::Agent;
use crate::chat::{Answer, Ask};
use crate::forge::Forge;
use crate::git::plumbing;
use crate::job::Workspace;
use crate::tools::Tool;
use crate::tools::arg;

/// The key of the feature-build record: one per launched build,
/// counting up from 1.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct RunId(u64);

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

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

/// The typed refusal: one feature at a time, and this is the build in
/// flight, named.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Refused {
    pub run: RunId,
    pub feature: Feature,
    branch: String,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a feature build is already in flight: run {} is building feature \
             {} on branch {}; feature_status says how it is going, and a \
             second feature waits for the first to finish",
            self.run, self.feature, self.branch
        )
    }
}

/// One entry of the record: a launch under way — reserved before the
/// check card is raised, so a second start refuses before anyone is
/// asked — or the build itself.
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

impl Entry {
    /// Whether this entry blocks a new start: a reservation always
    /// does; a flight does until its build finishes.
    fn in_flight(&self) -> bool {
        match self {
            Self::Starting { .. } => true,
            Self::Flight(flight) => !flight
                .record
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .finished(),
        }
    }

    const fn named(&self) -> (&Feature, &str) {
        match self {
            Self::Starting {
                feature, branch, ..
            } => (feature, branch.as_str()),
            Self::Flight(flight) => (&flight.feature, flight.branch.as_str()),
        }
    }
}

/// The record of feature builds, keyed by run id. Finished builds stay
/// readable — status outlives completion — and only a launch under way
/// or a build still in flight blocks a reservation.
pub struct Builds {
    entries: Mutex<BTreeMap<RunId, Entry>>,
    /// The next run id. A counter rather than the map's maximum, so a
    /// released reservation never recycles its id onto a later build.
    next: std::sync::atomic::AtomicU64,
}

impl Default for Builds {
    fn default() -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            next: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl Builds {
    /// The refusal a second `start_feature` answers with, when a launch
    /// or a build is in flight.
    #[cfg(test)]
    fn in_flight(&self) -> Option<Refused> {
        refusal(&self.entries.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Reserves the next run id for a launch, or refuses typed while
    /// one is in flight — check and insert under one map lock, held for
    /// nothing else. The reservation is what a second start runs into
    /// from this moment on: before any card is raised, before any
    /// branch is established. Fill it when the build is running; drop
    /// it unfilled and the entry goes, though its id is never reused.
    ///
    /// # Errors
    ///
    /// [`Refused`], naming the build in flight.
    fn reserve(
        self: &Arc<Self>,
        feature: Feature,
        repository: String,
        branch: String,
    ) -> Result<Reservation, Refused> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(refused) = refusal(&entries) {
            return Err(refused);
        }
        let run = RunId(self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        entries.insert(
            run,
            Entry::Starting {
                feature,
                repository,
                branch,
            },
        );
        Ok(Reservation {
            builds: Arc::clone(self),
            run,
            filled: false,
        })
    }
}

/// A run id held while a launch is under way: taken before the check
/// card is raised, filled with the [`Flight`] once the build is
/// running. Dropped unfilled — the launch failed — it releases its
/// entry, and a new start may reserve again.
struct Reservation {
    builds: Arc<Builds>,
    run: RunId,
    filled: bool,
}

impl fmt::Debug for Reservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reservation")
            .field("run", &self.run)
            .field("filled", &self.filled)
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
        self.filled = true;
        self.run
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.filled {
            self.builds
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&self.run);
        }
    }
}

/// The build in flight, when one is: the newest entry still going,
/// named for the refusal.
fn refusal(entries: &BTreeMap<RunId, Entry>) -> Option<Refused> {
    entries
        .iter()
        .rev()
        .find(|(_, entry)| entry.in_flight())
        .map(|(run, entry)| {
            let (feature, branch) = entry.named();
            Refused {
                run: *run,
                feature: feature.clone(),
                branch: branch.to_owned(),
            }
        })
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
         Returns as soon as the build is running; it then proceeds on its own for a long \
         time, so tell the user the branch and run id and end your turn — the user will ask \
         how it is going, and feature_status answers. One feature build runs at a time.",
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
            // The reservation: from here a second start refuses — before
            // any card is raised — and any failure below releases it on
            // the way out.
            let reservation = builds
                .reserve(feature.clone(), repository.clone(), branch.clone())
                .map_err(|refused| refused.to_string())?;
            let plan = plan(repo, &feature)?;
            let problems = plan.problems();
            let issues = plan.work(&BTreeSet::new()).len();
            let forge = forge(repo)?;
            let agents = agents()?;
            // The check is a precondition: raised here, before anything
            // is dispatched, with detection's proposal prefilled.
            let proposal = check::detect(Path::new(&repository)).map(|check| check.command);
            let answer = asker(Ask::Check {
                prompt: asked(&feature, &branch),
                proposal,
            });
            let check = confirmed(&answer);
            let command = check.as_ref().map(|check| check.command.clone());
            // Establishing pushes over the network and the build starts
            // machinery: both happen outside every map lock — the
            // reservation is what keeps the door shut meanwhile.
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
            let record = build(plan, established, agents, Arc::clone(&budget));
            let run = reservation.fill(Flight {
                feature: feature.clone(),
                repository: repository.clone(),
                branch: branch.clone(),
                check: command.clone(),
                tip: Arc::new(tip),
                record,
            });
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
        "How the feature build started by start_feature is going — or went: each issue's \
         title and state (waiting, running, merging, merged, failed with its report, skipped \
         with its reason), the feature branch's tip, the check in force (null means the \
         branch is unchecked), and any problems with the plan's shape. Call it once when the \
         user asks and report what it says; never poll it in a loop — the build runs on its \
         own. Pass run to read an earlier build; the latest answers when it is omitted.",
        json!({
            "type": "object",
            "properties": {
                "run": {
                    "type": "integer",
                    "description": "The run id start_feature answered with; the latest when omitted.",
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
    use super::super::fixtures::{id, node, plan, seven_holding_eight};
    use super::*;
    use crate::testing::{Local, Scratch, seeded};
    use crate::tools::Registry;

    /// An Agent that never starts: its program does not exist, so every
    /// dispatch fails at the start — which is exactly what lets the tool
    /// tests run without an engine. The end-to-end build rides in the
    /// feature build's own tests.
    fn unreachable(_: &Issue, _: &Workspace, _: &str) -> Result<Agent, String> {
        Agent::new(vec!["/nonexistent/agent".to_owned()], "/", [])
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
            problems: plan.problems(),
            plan,
            states: BTreeMap::new(),
            runs: BTreeMap::new(),
        };
        for (number, state) in states {
            build.states.insert(id(*number), state.clone());
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
    /// plan, a bare-directory forge, an Agent that never starts, and an
    /// asker the test scripts.
    fn registry(
        builds: &Arc<Builds>,
        the_plan: Plan,
        remote: String,
        asker: impl Fn(Ask) -> Answer + 'static,
    ) -> Registry {
        let mut registry = Registry::default();
        registry.register(start_feature(
            Arc::clone(builds),
            Budget::new(),
            move |_, _| Ok(the_plan.clone()),
            move |_| Ok(Local(remote.clone())),
            || Ok(unreachable),
            asker,
        ));
        registry.register(feature_status(Arc::clone(builds)));
        registry
    }

    fn eventually_finished(registry: &Registry) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let status = registry.dispatch("feature_status", "{}").unwrap();
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
    fn reserve(builds: &Arc<Builds>, feature: u64) -> Result<Reservation, Refused> {
        builds.reserve(
            Feature(id(feature)),
            "/r".to_owned(),
            format!("feature-{feature}"),
        )
    }

    /// The refusal a start meets while run `run` builds `feature`.
    fn refusal(run: u64, feature: u64) -> Refused {
        Refused {
            run: RunId(run),
            feature: Feature(id(feature)),
            branch: format!("feature-{feature}"),
        }
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

    #[test]
    fn the_refusal_names_the_build_in_flight() {
        let refused = refusal(3, 144);
        let words = refused.to_string();
        assert!(words.contains("run 3"), "{words}");
        assert!(words.contains("feature 144"), "{words}");
        assert!(words.contains("feature-144"), "{words}");
        assert!(words.contains("feature_status"), "{words}");
    }

    /// Reserves for a fixture flight and fills at once — the shorthand
    /// most map tests want.
    fn admitted(builds: &Arc<Builds>, the_flight: Flight) -> RunId {
        builds
            .reserve(
                the_flight.feature.clone(),
                the_flight.repository.clone(),
                the_flight.branch.clone(),
            )
            .unwrap()
            .fill(the_flight)
    }

    #[test]
    fn a_reservation_or_a_running_build_refuses_the_next_start_typed() {
        let builds = Arc::new(Builds::default());
        let first = admitted(
            &builds,
            flight(7, "feature-7", "/r", &[(8, State::Running)]),
        );
        assert_eq!(first, RunId(1));

        let refused = reserve(&builds, 9).unwrap_err();
        assert_eq!(refused, refusal(1, 7));
        assert_eq!(builds.in_flight(), Some(refused));

        // The build ends; the record stays; reservation reopens with
        // the next id.
        {
            let entries = builds.entries.lock().unwrap();
            let Entry::Flight(flight) = &entries[&RunId(1)] else {
                panic!("the filled entry is a flight");
            };
            flight.record.lock().unwrap().states.insert(
                id(8),
                State::Failed {
                    report: "boom".to_owned(),
                },
            );
        }
        assert_eq!(builds.in_flight(), None);
        let second = admitted(&builds, flight(9, "feature-9", "/r", &[]));
        assert_eq!(second, RunId(2));
    }

    /// The reservation itself is the refusal: taken before any card is
    /// raised, a second start runs into it even though no flight has
    /// filled it yet — so two starts can never both ask.
    #[test]
    fn an_unfilled_reservation_already_refuses_the_next_start() {
        let builds = Arc::new(Builds::default());
        let reservation = reserve(&builds, 7).unwrap();
        assert_eq!(reservation.run(), RunId(1));

        let refused = reserve(&builds, 9).unwrap_err();
        assert_eq!(refused, refusal(1, 7));

        // The launch failed: the dropped reservation releases the door,
        // and the spent id is never reused.
        drop(reservation);
        assert!(builds.entries.lock().unwrap().is_empty());
        let next = reserve(&builds, 9).unwrap();
        assert_eq!(next.run(), RunId(2));
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
        let builds = Arc::new(Builds::default());
        let registry = registry(&builds, seven_holding_eight(), remote, |_| Answer::Declined);

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

        let status = eventually_finished(&registry);
        assert_eq!(status["branch"], "feature-7");
        assert_eq!(
            status["tip"],
            json!(main_tip),
            "nothing landed: an Agent that never starts"
        );
        tidy(&work);
    }

    #[test]
    fn the_check_card_is_prefilled_from_detection_and_the_answer_is_in_force() {
        let scratch = Scratch::new("prefill");
        let (work, remote) = seeded(&scratch);
        cargo_crate(&work);
        let asked = Arc::new(Mutex::new(None::<Ask>));
        let builds = Arc::new(Builds::default());
        let registry = registry(&builds, seven_holding_eight(), remote, {
            let asked = Arc::clone(&asked);
            move |question| {
                *asked.lock().unwrap() = Some(question);
                Answer::Check {
                    command: "cargo test --workspace".to_owned(),
                }
            }
        });

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

        let status = eventually_finished(&registry);
        assert_eq!(status["check"], json!("cargo test --workspace"));
        tidy(&work);
    }

    #[test]
    fn a_decline_skips_the_check_and_the_record_says_unchecked() {
        let scratch = Scratch::new("decline");
        let (work, remote) = seeded(&scratch);
        cargo_crate(&work);
        let builds = Arc::new(Builds::default());
        let registry = registry(&builds, seven_holding_eight(), remote, |_| Answer::Declined);

        let started = start(&registry, &work, 7).unwrap();
        assert_eq!(started["check"], Value::Null, "unchecked, and said so");

        let status = eventually_finished(&registry);
        assert_eq!(status["check"], Value::Null);
        tidy(&work);
    }

    #[test]
    fn a_second_start_while_one_is_in_flight_is_refused_naming_it() {
        let scratch = Scratch::new("inflight");
        let (work, remote) = seeded(&scratch);
        let builds = Arc::new(Builds::default());
        admitted(
            &builds,
            flight(7, "feature-7", &work, &[(8, State::Running)]),
        );
        let registry = registry(
            &builds,
            plan(9, vec![node(9, false, &[], &[])]),
            remote,
            |_| panic!("no card is raised for a refused start"),
        );

        let refused = start(&registry, &work, 9).unwrap_err();
        assert!(refused.contains("run 1"), "{refused}");
        assert!(refused.contains("feature 7"), "{refused}");
        assert!(refused.contains("feature-7"), "{refused}");
    }

    #[test]
    fn what_is_not_a_repository_is_refused_before_anyone_is_asked() {
        let builds = Arc::new(Builds::default());
        let registry = registry(
            &builds,
            plan(7, vec![node(7, false, &[], &[])]),
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
        let builds = Arc::new(Builds::default());
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
        let registry = status_registry(&Arc::new(Builds::default()));
        let status = registry.dispatch("feature_status", "{}").unwrap();
        assert_eq!(status["run"], Value::Null);
        assert_eq!(status["note"], json!("no feature build has been started"));
    }

    /// A launch still under way — the reservation phase, the card up or
    /// the branch establishing — answers status without blocking on
    /// anything the launch holds.
    #[test]
    fn status_of_a_reserved_launch_says_it_has_not_started() {
        let builds = Arc::new(Builds::default());
        let reservation = reserve(&builds, 7).unwrap();
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
