//! The chat verbs of a feature build: `start_feature` launches one and
//! answers as soon as it is running; `feature_status` reads the record.
//!
//! The record — [`Builds`] — is a map keyed by [`RunId`], and
//! [`Builds::admit`] refuses while a build is in flight. The map shape
//! is deliberate: one feature at a time is policy, visibly the unstable
//! part of the decision, so it lives in one refusal — a line to delete,
//! never a shape to migrate. The refusal is typed, [`Refused::InFlight`],
//! and names the build in flight.
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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::{Value, json};

use super::merge::Branch;
use super::{Budget, Build, Issue, IssueId, Plan, build};
use crate::agent::Agent;
use crate::build::Workspace;
use crate::chat::{Answer, Ask};
use crate::check::{self, Check};
use crate::forge::Forge;
use crate::git::plumbing;
use crate::tools::Tool;

/// The key of the feature-build record: one per launched build,
/// counting up from 1.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RunId(pub u64);

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One launched feature build, as the record keeps it: the build in
/// flight, and afterwards the build that flew.
pub struct Flight {
    pub feature: IssueId,
    /// The local clone the build runs in.
    pub repository: String,
    /// The feature branch the merges land on.
    pub branch: String,
    /// The check command in force — `None` when the user declined: the
    /// branch is unchecked, and the record says so.
    pub check: Option<String>,
    pub record: Arc<Mutex<Build>>,
}

/// Why a feature build did not start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refused {
    /// One feature at a time: the build in flight, named.
    InFlight {
        run: RunId,
        feature: IssueId,
        branch: String,
    },
    /// The launch itself failed, in its own words.
    Launch(String),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InFlight {
                run,
                feature,
                branch,
            } => write!(
                f,
                "a feature build is already in flight: run {run} is building feature \
                 {feature} on branch {branch}; feature_status says how it is going, and a \
                 second feature waits for the first to finish"
            ),
            Self::Launch(words) => write!(f, "{words}"),
        }
    }
}

/// The record of feature builds, keyed by run id. Finished builds stay
/// readable — status outlives completion — and only a build still in
/// flight blocks admission.
#[derive(Default)]
pub struct Builds {
    flights: Mutex<BTreeMap<RunId, Flight>>,
}

impl Builds {
    /// The refusal a second `start_feature` answers with, when a build
    /// is in flight.
    #[must_use]
    pub fn in_flight(&self) -> Option<Refused> {
        refusal(&self.flights.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Admits `launch`'s build into the record under a fresh id. The
    /// in-flight check and the launch run under one map lock — the
    /// launch starts machinery and returns, so nothing here blocks long
    /// — which is what makes it impossible for a build to run
    /// unrecorded or for two launches to interleave.
    ///
    /// # Errors
    ///
    /// [`Refused::InFlight`] while a build is running, or the launch's
    /// own words.
    pub fn admit(&self, launch: impl FnOnce() -> Result<Flight, String>) -> Result<RunId, Refused> {
        let mut flights = self.flights.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(refused) = refusal(&flights) {
            return Err(refused);
        }
        let run = RunId(flights.keys().next_back().map_or(1, |RunId(last)| last + 1));
        flights.insert(run, launch().map_err(Refused::Launch)?);
        Ok(run)
    }
}

/// The build in flight, when one is: the newest unfinished flight,
/// named for the refusal.
fn refusal(flights: &BTreeMap<RunId, Flight>) -> Option<Refused> {
    flights
        .iter()
        .rev()
        .find(|(_, flight)| {
            !flight
                .record
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .finished()
        })
        .map(|(run, flight)| Refused::InFlight {
            run: *run,
            feature: flight.feature.clone(),
            branch: flight.branch.clone(),
        })
}

/// The wording of the check card, composed by the machinery — the
/// persona never decides whether or how to ask.
fn asked(feature: &IssueId, branch: &str) -> String {
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

fn string<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, String> {
    arguments[name]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("the {name} argument must be a non-empty string"))
}

fn number(arguments: &Value, name: &str) -> Result<u64, String> {
    arguments[name]
        .as_u64()
        .ok_or_else(|| format!("the {name} argument must be a whole number"))
}

/// The `start_feature` tool over the host's seams: `plan` reads a
/// feature's shape out of the tracker, `forge` names where the feature
/// branch's pushes go, `runner` locates the agent runner binary,
/// `agents` yields the per-issue Agent factory one build uses, and
/// `asker` is the whole question modality — it takes the check card and
/// comes back with the answer, however long that takes. Each is fallible
/// where the host's world is: every refusal is words the model reads.
pub fn start_feature<F, A, M>(
    builds: Arc<Builds>,
    budget: Arc<Budget>,
    runner: impl Fn() -> Result<PathBuf, String> + 'static,
    plan: impl Fn(&str, &IssueId) -> Result<Plan, String> + 'static,
    forge: impl Fn(&str) -> Result<F, String> + 'static,
    agents: impl Fn() -> Result<M, String> + 'static,
    asker: impl Fn(Ask) -> Answer + 'static,
) -> Tool
where
    F: Forge + Send + Sync + 'static,
    A: Agent + 'static,
    M: Fn(&Issue, &Workspace, &str) -> A + Send + Sync + 'static,
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
            let repo = string(arguments, "repo")?;
            let repository = string(arguments, "repository")?.to_owned();
            let feature = IssueId::from(number(arguments, "feature")?);
            if let Some(refused) = builds.in_flight() {
                return Err(refused.to_string());
            }
            // The clone is read before anyone is asked anything:
            // detection and the default base both need it to be real.
            plumbing(&["-C", &repository, "rev-parse", "--absolute-git-dir"])
                .map_err(|words| format!("{repository} is not a git repository: {words}"))?;
            let branch = arguments["branch"]
                .as_str()
                .map_or_else(|| format!("feature-{feature}"), str::to_owned);
            let base = match arguments["base"].as_str() {
                Some(base) => base.to_owned(),
                None => plumbing(&["-C", &repository, "symbolic-ref", "--short", "HEAD"])
                    .map_err(|words| format!("the repository has no default branch: {words}"))?
                    .trim()
                    .to_owned(),
            };
            let plan = plan(repo, &feature)?;
            let problems = plan.problems();
            let issues = plan.work(&BTreeSet::new()).len();
            let forge = forge(repo)?;
            let runner = runner()?;
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
            let run = builds
                .admit(|| {
                    let established = Branch::establish(&repository, &branch, &base, forge, check)?;
                    let record = build(plan, established, &runner, agents, Arc::clone(&budget));
                    Ok(Flight {
                        feature: feature.clone(),
                        repository: repository.clone(),
                        branch: branch.clone(),
                        check: command.clone(),
                        record,
                    })
                })
                .map_err(|refused| refused.to_string())?;
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
            // Cloned out under the map lock; git runs after it is gone.
            let (run, feature, repository, branch, check, build) = {
                let flights = builds
                    .flights
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let (run, flight) = match arguments["run"].as_u64() {
                    Some(asked) => flights
                        .get_key_value(&RunId(asked))
                        .ok_or_else(|| format!("no feature build has run id {asked}"))?,
                    None => match flights.iter().next_back() {
                        Some(latest) => latest,
                        None => {
                            return Ok(json!({
                                "run": Value::Null,
                                "note": "no feature build has been started",
                            }));
                        }
                    },
                };
                (
                    *run,
                    flight.feature.clone(),
                    flight.repository.clone(),
                    flight.branch.clone(),
                    flight.check.clone(),
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
            let tip = plumbing(&[
                "-C",
                &repository,
                "rev-parse",
                &format!("refs/heads/{branch}"),
            ])
            .map(|tip| Value::String(tip.trim().to_owned()))
            .unwrap_or(Value::Null);
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
    use super::super::fixtures::{id, node, plan};
    use super::*;
    use crate::agent::Task;
    use crate::testing::Scratch;
    use crate::tools::Registry;

    /// A forge for tests: a bare directory, no credentials.
    struct Local(String);

    impl Forge for Local {
        fn remote(&self) -> String {
            self.0.clone()
        }

        fn credentials(&self) -> Option<crate::forge::Credentials> {
            None
        }
    }

    /// An Agent that is never reached in these tests: the runner path is
    /// bogus, so every dispatch fails at launch — which is exactly what
    /// lets the tool tests run with no runner binary. The end-to-end
    /// build rides in crates/epik-agent's integration tests.
    struct Unreachable;

    impl Agent for Unreachable {
        fn task(&self) -> Task {
            Task {
                argv: vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()],
                env: Vec::new(),
                cwd: "/".to_owned(),
                stdin: None,
            }
        }
    }

    fn git(args: &[&str]) -> String {
        plumbing(args).unwrap()
    }

    /// A working repository with one commit on main, and a bare remote.
    fn seeded(scratch: &Scratch) -> (String, String) {
        let work = scratch.join("work");
        let remote = scratch.join("remote.git");
        git(&["init", "--initial-branch=main", &work]);
        git(&["-C", &work, "config", "user.name", "Test"]);
        git(&["-C", &work, "config", "user.email", "test@example.com"]);
        git(&["-C", &work, "config", "commit.gpgsign", "false"]);
        std::fs::write(std::path::Path::new(&work).join("hello.txt"), "hello\n").unwrap();
        git(&["-C", &work, "add", "hello.txt"]);
        git(&["-C", &work, "commit", "-m", "the first commit"]);
        git(&["init", "--bare", "--initial-branch=main", &remote]);
        (work, remote)
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
            feature: id(feature),
            repository: repository.to_owned(),
            branch: branch.to_owned(),
            check: None,
            record: Arc::new(Mutex::new(build)),
        }
    }

    /// The registry a turn would carry, over injected seams: a fixture
    /// plan, a bare-directory forge, a bogus runner, and an asker the
    /// test scripts.
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
            || Ok(PathBuf::from("/nonexistent/epik-agent")),
            move |_, _| Ok(the_plan.clone()),
            move |_| Ok(Local(remote.clone())),
            || {
                Ok(|_: &Issue, workspace: &Workspace, _: &str| {
                    let _ = workspace;
                    Unreachable
                })
            },
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

    #[test]
    fn the_refusal_names_the_build_in_flight() {
        let refused = Refused::InFlight {
            run: RunId(3),
            feature: id(144),
            branch: "feature-144".to_owned(),
        };
        let words = refused.to_string();
        assert!(words.contains("run 3"), "{words}");
        assert!(words.contains("feature 144"), "{words}");
        assert!(words.contains("feature-144"), "{words}");
        assert!(words.contains("feature_status"), "{words}");
    }

    #[test]
    fn admission_is_refused_typed_while_a_build_is_in_flight() {
        let builds = Builds::default();
        let first = builds
            .admit(|| Ok(flight(7, "feature-7", "/r", &[(8, State::Running)])))
            .unwrap();
        assert_eq!(first, RunId(1));

        let refused = builds
            .admit(|| panic!("the launch must not run"))
            .unwrap_err();
        assert_eq!(
            refused,
            Refused::InFlight {
                run: RunId(1),
                feature: id(7),
                branch: "feature-7".to_owned(),
            }
        );
        assert_eq!(builds.in_flight(), Some(refused));

        // The build ends; the record stays; admission reopens with the
        // next id.
        {
            let flights = builds.flights.lock().unwrap();
            flights[&RunId(1)].record.lock().unwrap().states.insert(
                id(8),
                State::Failed {
                    report: "boom".to_owned(),
                },
            );
        }
        assert_eq!(builds.in_flight(), None);
        let second = builds
            .admit(|| Ok(flight(9, "feature-9", "/r", &[])))
            .unwrap();
        assert_eq!(second, RunId(2));
    }

    #[test]
    fn a_failed_launch_is_words_and_admits_nothing() {
        let builds = Builds::default();
        let refused = builds
            .admit(|| Err("the forge is gone".to_owned()))
            .unwrap_err();
        assert_eq!(refused, Refused::Launch("the forge is gone".to_owned()));
        assert!(builds.flights.lock().unwrap().is_empty());
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
        let registry = registry(
            &builds,
            plan(7, vec![node(7, false, &[8], &[]), node(8, false, &[], &[])]),
            remote,
            |_| Answer::Declined,
        );

        let started = registry
            .dispatch(
                "start_feature",
                &json!({ "repo": "o/r", "repository": work, "feature": 7 }).to_string(),
            )
            .unwrap();
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
            "nothing landed: a bogus runner"
        );
        tidy(&work);
    }

    #[test]
    fn the_check_card_is_prefilled_from_detection_and_the_answer_is_in_force() {
        let scratch = Scratch::new("prefill");
        let (work, remote) = seeded(&scratch);
        std::fs::write(
            std::path::Path::new(&work).join("Cargo.toml"),
            "[package]\nname = \"wumpus\"\n",
        )
        .unwrap();
        let asked = Arc::new(Mutex::new(None::<Ask>));
        let builds = Arc::new(Builds::default());
        let registry = registry(
            &builds,
            plan(7, vec![node(7, false, &[8], &[]), node(8, false, &[], &[])]),
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

        let status = eventually_finished(&registry);
        assert_eq!(status["check"], json!("cargo test --workspace"));
        tidy(&work);
    }

    #[test]
    fn a_decline_skips_the_check_and_the_record_says_unchecked() {
        let scratch = Scratch::new("decline");
        let (work, remote) = seeded(&scratch);
        std::fs::write(
            std::path::Path::new(&work).join("Cargo.toml"),
            "[package]\nname = \"wumpus\"\n",
        )
        .unwrap();
        let builds = Arc::new(Builds::default());
        let registry = registry(
            &builds,
            plan(7, vec![node(7, false, &[8], &[]), node(8, false, &[], &[])]),
            remote,
            |_| Answer::Declined,
        );

        let started = registry
            .dispatch(
                "start_feature",
                &json!({ "repo": "o/r", "repository": work, "feature": 7 }).to_string(),
            )
            .unwrap();
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
        builds
            .admit(|| Ok(flight(7, "feature-7", &work, &[(8, State::Running)])))
            .unwrap();
        let registry = registry(
            &builds,
            plan(9, vec![node(9, false, &[], &[])]),
            remote,
            |_| panic!("no card is raised for a refused start"),
        );

        let refused = registry
            .dispatch(
                "start_feature",
                &json!({ "repo": "o/r", "repository": work, "feature": 9 }).to_string(),
            )
            .unwrap_err();
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
        let error = registry
            .dispatch(
                "start_feature",
                &json!({ "repo": "o/r", "repository": "/nonexistent/clone", "feature": 7 })
                    .to_string(),
            )
            .unwrap_err();
        assert!(error.contains("not a git repository"), "{error}");
        assert!(builds.flights.lock().unwrap().is_empty());
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
        builds
            .admit(|| {
                let mut flight = flight(
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
                flight.check = Some("cargo test".to_owned());
                Ok(flight)
            })
            .unwrap();

        let mut registry = Registry::default();
        registry.register(feature_status(Arc::clone(&builds)));
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
        let mut registry = Registry::default();
        registry.register(feature_status(Arc::new(Builds::default())));
        let status = registry.dispatch("feature_status", "{}").unwrap();
        assert_eq!(status["run"], Value::Null);
        assert_eq!(status["note"], json!("no feature build has been started"));
    }
}
