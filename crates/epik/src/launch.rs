//! The launch tool: a feature handed to the coding agent, mid-chat.
//!
//! One registry entry sets the run machinery to work. Given a feature
//! issue number, the handler preflights the config-derived manifest, then
//! conducts a [`FeatureRun`] on a thread of its own and answers as soon as
//! the run is started — naming the run and the log file it narrates into.
//! Implementation belongs to the coding agent through the
//! [`CodingAgent`] seam: the chat model authors no code and receives no
//! run events, because the run's whole narration goes to its log
//! ([`crate::logs`]) and nowhere near the chat transcript.
//!
//! One run in flight at a time, and refusals are vocabulary: a launch
//! while one is running is a typed [`Error::InFlight`] naming it, and a
//! preflight refusal is the tool's own [`Error::Refused`], rendered for
//! the model like every other tool error. The run's stop token stays here,
//! with the machinery that owns the thread — [`Launcher::stop`] is the one
//! outside reach it answers to, and nothing leaks the token itself.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::{Budget, CodingAgent};
use crate::chat::StopToken;
use crate::git::Git;
use crate::github::Repo;
use crate::keystore::KeyStore;
use crate::logs::{Kind, Logs};
use crate::preflight::{self, CapabilityStatus};
use crate::run::{FeatureRun, Machinery, credentialed};
use crate::tools::{Registry, Tool};

/// What every launched feature shares, handed in once — the
/// [`FeatureRun`] fields that do not depend on which issue, plus the
/// host's read of the token override. Everything handed in, nothing
/// discovered.
#[derive(Clone)]
pub struct Launch {
    pub repo: Repo,
    /// Where the repository is cloned from.
    pub url: String,
    /// The branch the review pull request merges into.
    pub base: String,
    /// What each issue run may spend.
    pub budget: Budget,
    /// How long each issue run's judgment waits for a check still running.
    pub patience: Duration,
    /// Outranks the keystore for the GitHub token: `$EPIK_GITHUB_TOKEN`,
    /// read by the host and handed in — never read here.
    pub token_override: Option<String>,
}

/// Hand-written to keep the secret out: the token override is named as
/// present or absent, never spelled — a `{:?}` must land in no log with a
/// credential in it.
impl fmt::Debug for Launch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Launch")
            .field("repo", &self.repo)
            .field("url", &self.url)
            .field("base", &self.base)
            .field("budget", &self.budget)
            .field("patience", &self.patience)
            .field(
                "token_override",
                &self.token_override.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// The launch machinery: the collaborators a run is provisioned from, and
/// the single run-in-flight slot.
///
/// Cloning shares the slot, so a host can move one clone into the
/// registry's handler and keep another for [`stop`](Self::stop) — the
/// [`StopToken`] pattern, one level up.
#[derive(Clone)]
pub struct Launcher {
    launch: Launch,
    agent: Arc<dyn CodingAgent + Send + Sync>,
    machinery: Arc<dyn Machinery + Send + Sync>,
    git: Git,
    logs: Logs,
    store: Arc<dyn KeyStore + Send + Sync>,
    running: Arc<Mutex<Option<InFlight>>>,
}

/// The run in flight: its name for the refusal that must say it, its stop
/// token, and the thread conducting it — whose being finished is what
/// frees the slot.
#[derive(Debug)]
struct InFlight {
    run: String,
    stop: StopToken,
    /// The conducting thread — `None` while the launch is still
    /// provisioning, because the slot is claimed before the preflight and
    /// the thread arrives when provisioning ends.
    handle: Option<JoinHandle<()>>,
}

impl InFlight {
    /// Still in flight: provisioning, or a thread not yet finished. Only a
    /// finished thread frees the slot.
    fn conducting(&self) -> bool {
        self.handle
            .as_ref()
            .is_none_or(|handle| !handle.is_finished())
    }
}

/// The slot, locked. The state is a plain value, intact whatever became
/// of a previous holder, so a poisoned lock is taken over rather than
/// raised.
fn slot(running: &Mutex<Option<InFlight>>) -> MutexGuard<'_, Option<InFlight>> {
    running.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Launcher {
    /// A launcher over its collaborators: the config-derived agent, the
    /// machinery the run reads and writes GitHub through, the git cache,
    /// the log root, and the keystore the preflight resolves the token
    /// from. All seams, so a test hands in a scripted agent and a loopback
    /// machinery; the real path is `ClaudeCode` from `[worker]` config
    /// over `GitHub`.
    #[must_use]
    pub fn new(
        launch: Launch,
        agent: impl CodingAgent + Send + Sync + 'static,
        machinery: impl Machinery + Send + Sync + 'static,
        git: Git,
        logs: Logs,
        store: impl KeyStore + Send + Sync + 'static,
    ) -> Self {
        Self {
            launch,
            agent: Arc::new(agent),
            machinery: Arc::new(machinery),
            git,
            logs,
            store: Arc::new(store),
            running: Arc::new(Mutex::new(None)),
        }
    }

    /// Sets the run machinery to work on feature `number` and answers as
    /// soon as the run is started: the turn that called ends while the run
    /// proceeds on its own thread, narrating into the named log and never
    /// into the chat. The log file is created before the thread starts, so
    /// the answer can say where the narration lands.
    ///
    /// # Errors
    ///
    /// [`Error::InFlight`] while a run is still conducting — one at a
    /// time, and the refusal names it; [`Error::Refused`] when the
    /// preflight cannot stand; [`Error::Log`] when the run's log cannot be
    /// created, because a run whose narration has nowhere durable to land
    /// must not start.
    pub fn launch(&self, number: u64) -> Result<Launched, Error> {
        // Credentials arrive only after the preflight vouches for them;
        // everything else the run needs is in hand now, and the claim
        // below wants the run's own name.
        let run = FeatureRun {
            repo: self.launch.repo.clone(),
            url: self.launch.url.clone(),
            base: self.launch.base.clone(),
            number,
            env: Vec::new(),
            budget: self.launch.budget,
            patience: self.launch.patience,
        };
        let stop = StopToken::new();
        // The claim, brief: the preflight can spawn processes and consult
        // the keyring — a dialog, on macOS — so the lock covers only the
        // in-flight check and the slot's taking. A claimed slot with no
        // thread yet is a launch mid-provisioning, and it refuses rivals
        // like any conducting run.
        {
            let mut running = slot(&self.running);
            if let Some(inflight) = running.as_ref().filter(|it| it.conducting()) {
                return Err(Error::InFlight {
                    running: inflight.run.clone(),
                });
            }
            *running = Some(InFlight {
                run: run.branch(),
                stop: stop.clone(),
                handle: None,
            });
        }
        self.provisioned(run, stop).inspect_err(|_| {
            // A refused launch frees the slot on its way out — no rival
            // could have taken it while the claim stood.
            *slot(&self.running) = None;
        })
    }

    /// The expensive middle of a launch, outside the slot's lock: the
    /// preflight, the log's creation, and the conducting thread's start.
    /// The claim already stands; the thread lands in it on success.
    fn provisioned(&self, run: FeatureRun, stop: StopToken) -> Result<Launched, Error> {
        let token = preflight::manifest(
            &self.agent.binaries(),
            self.launch.token_override.clone(),
            self.store.as_ref(),
        )
        .map_err(|refusals| Error::Refused { refusals })?;
        let (log, sink) = self
            .logs
            .create(&run.repo, Kind::Feature, run.number)
            .map_err(|error| Error::Log { error })?;
        let name = run.branch();
        let run = FeatureRun {
            env: credentialed(&token),
            ..run
        };
        // The vouched-for token, offered onward to git for the fetches,
        // through the askpass rails.
        let git = self.git.clone().authenticated(token);
        let agent = Arc::clone(&self.agent);
        let machinery = Arc::clone(&self.machinery);
        let handle = thread::spawn(move || {
            // The verdict is the log's last word, and GitHub's record
            // stands beside it; a conduct is infallible, so the thread has
            // nothing to answer for.
            let mut sink = sink;
            run.conduct(&git, machinery.as_ref(), agent.as_ref(), &mut sink, &stop);
        });
        if let Some(inflight) = slot(&self.running).as_mut() {
            inflight.handle = Some(handle);
        }
        Ok(Launched { run: name, log })
    }

    /// The run in flight's name — "feature-7" — while one is still
    /// conducting.
    #[must_use]
    pub fn running(&self) -> Option<String> {
        slot(&self.running)
            .as_ref()
            .filter(|inflight| inflight.conducting())
            .map(|inflight| inflight.run.clone())
    }

    /// Asks the run in flight to wind up, when one is. The stop token
    /// stays with the machinery that owns the thread; this verb is the one
    /// outside reach it answers to.
    pub fn stop(&self) {
        if let Some(inflight) = slot(&self.running).as_ref() {
            inflight.stop.stop();
        }
    }
}

impl fmt::Debug for Launcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Launcher")
            .field("launch", &self.launch)
            .finish_non_exhaustive()
    }
}

/// The tool's answer: the run is started, and this is where to find it.
#[derive(Clone, Debug, Serialize)]
pub struct Launched {
    /// The run's name: "feature-7".
    pub run: String,
    /// The log file the run narrates into — the only place its events go.
    pub log: PathBuf,
}

/// Why a launch started no run: the tool's own failure vocabulary,
/// rendered for the model like every other tool error.
#[derive(Debug)]
pub enum Error {
    /// The preflight refused: every capability that could not stand.
    Refused { refusals: Vec<CapabilityStatus> },
    /// One run in flight at a time, and this names the one that is.
    InFlight { running: String },
    /// The run's log could not be created, and a run whose narration has
    /// nowhere durable to land must not start.
    Log { error: anyhow::Error },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused { refusals } => {
                let refusals: Vec<String> = refusals.iter().map(ToString::to_string).collect();
                write!(f, "the preflight refused: {}", refusals.join("; "))
            }
            Self::InFlight { running } => {
                write!(f, "{running} is already in flight; one run at a time")
            }
            Self::Log { error } => write!(f, "the run log could not be created: {error:#}"),
        }
    }
}

impl std::error::Error for Error {}

/// Registers the launch verb into `registry`, conducted by `launcher`.
/// The caller that wants [`Launcher::stop`] keeps a clone; the slot is
/// shared.
///
/// # Panics
///
/// Panics when `registry` already holds a tool by this name, as
/// [`Registry::register`] does.
pub fn register(registry: &mut Registry, launcher: Launcher) {
    registry.register(launch_feature(), move |args: LaunchFeature| {
        launcher.launch(args.number)
    });
}

// The face: prompt engineering, not documentation — a model reads nothing
// else when it decides whether to call.

fn launch_feature() -> Tool {
    Tool::new(
        "launch_feature",
        "Set the coding agent to work implementing a feature issue: every open sub-issue \
         gets implemented and merged into the feature branch, and a review pull request is \
         opened when they are all closed. Answers as soon as the run is started, naming \
         the run and the log file it narrates into — the run then proceeds on its own, and \
         this call never waits for it or reports its progress. One run at a time: \
         launching while one is in flight is refused, naming the run that is running.",
        json!({
            "type": "object",
            "properties": {
                "number": {
                    "type": "integer",
                    "description": "The feature issue's number, without the leading #.",
                },
            },
            "required": ["number"],
        }),
    )
}

#[derive(Deserialize)]
struct LaunchFeature {
    number: u64,
}

#[cfg(test)]
// Test scaffolding is entitled to panic; the allow-unwrap-in-tests clippy
// setting only covers #[test] functions, not the helpers here.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::Instant;

    use tempfile::TempDir;

    use super::*;
    use crate::agent::Scripted;
    use crate::git::testing::World;
    use crate::github::{self, Check, Issue, IssueGraph, Pull, State};
    use crate::keystore::InMemory;
    use crate::preflight::Capability;
    use crate::run::{Evidence, FeatureEvent, FeatureVerdict};

    /// The littlest GitHub a launch can be judged against: one feature
    /// issue with no sub-issues — the shortest conductable run — and, when
    /// gated, a graph read that blocks until the test says, holding the
    /// run mid-conduct while the tool's answer is examined.
    struct Fake {
        gate: Option<Mutex<Receiver<()>>>,
    }

    impl Fake {
        fn instant() -> Self {
            Self { gate: None }
        }

        fn gated() -> (Self, Sender<()>) {
            let (open, gate) = mpsc::channel();
            (
                Self {
                    gate: Some(Mutex::new(gate)),
                },
                open,
            )
        }
    }

    impl Evidence for Fake {
        fn pull(&self, _: &Repo, _: &str) -> Result<Option<Pull>, github::Error> {
            unimplemented!("a sub-issueless feature run never consults evidence")
        }

        fn checks(&self, _: &Repo, _: &str) -> Result<Vec<Check>, github::Error> {
            unimplemented!("a sub-issueless feature run never consults evidence")
        }

        fn issue(&self, _: &Repo, _: u64) -> Result<Issue, github::Error> {
            unimplemented!("a sub-issueless feature run never consults evidence")
        }
    }

    impl Machinery for Fake {
        fn graph(&self, _: &Repo, number: u64) -> Result<IssueGraph, github::Error> {
            if let Some(gate) = &self.gate {
                // A dropped sender opens the gate too: the run winds up
                // rather than outliving the test that staged it.
                let _ = gate.lock().unwrap().recv();
            }
            Ok(IssueGraph {
                issue: Issue {
                    number,
                    title: format!("feature {number}"),
                    body: "the plan".to_owned(),
                    state: State::Open,
                },
                sub_issues: Vec::new(),
                blocked_by: Vec::new(),
            })
        }

        fn branch(&self, _: &Repo, _: &str) -> Result<Option<String>, github::Error> {
            unimplemented!("a sub-issueless feature fails before the branch phase")
        }

        fn create_branch(&self, _: &Repo, _: &str, _: &str) -> Result<(), github::Error> {
            unimplemented!("a sub-issueless feature fails before the branch phase")
        }

        fn open_pull(
            &self,
            _: &Repo,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Pull, github::Error> {
            unimplemented!("a sub-issueless feature fails before the review phase")
        }
    }

    /// A launcher over the world's git, a temp log root, and the stated
    /// token override — the preflight's keystore rail deliberately empty,
    /// so the override is the whole credential story.
    fn launcher(world: &World, logs: &Path, machinery: Fake, token: Option<&str>) -> Launcher {
        Launcher::new(
            Launch {
                repo: world.repo.clone(),
                url: world.url(),
                base: "main".to_owned(),
                budget: Budget {
                    max_tokens: None,
                    max_cost: None,
                    stall: Duration::from_mins(1),
                },
                patience: Duration::ZERO,
                token_override: token.map(str::to_owned),
            },
            Scripted::playing(Vec::new()),
            machinery,
            world.git.clone(),
            Logs::rooted(logs),
            InMemory::default(),
        )
    }

    /// Waits for the run in flight to finish — the test's rendezvous with
    /// a thread it cannot join.
    fn finished(launcher: &Launcher) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while launcher.running().is_some() {
            assert!(Instant::now() < deadline, "the run never finished");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Every line of the run's log, parsed back into the vocabulary.
    fn narrated(log: &Path) -> Vec<FeatureEvent> {
        fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn the_answer_arrives_while_the_run_conducts_and_the_log_grows_after() {
        let w = World::new();
        let root = TempDir::new().unwrap();
        let (fake, open) = Fake::gated();
        let launcher = launcher(&w, root.path(), fake, Some("ghp-test"));

        let started = launcher.launch(7).unwrap();

        assert_eq!(started.run, "feature-7");
        assert!(
            started.log.is_file(),
            "the log exists before the answer: {started:?}"
        );
        let name = started
            .log
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(name.starts_with("feature-7-"), "{name}");
        // The run is still conducting — held at the gate — so a second
        // launch is refused, naming the run in flight.
        let error = launcher.launch(9).unwrap_err();
        assert!(
            matches!(&error, Error::InFlight { running } if running == "feature-7"),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "feature-7 is already in flight; one run at a time"
        );
        assert_eq!(launcher.running().as_deref(), Some("feature-7"));
        let at_answer = narrated(&started.log).len();

        open.send(()).unwrap();
        finished(&launcher);

        let events = narrated(&started.log);
        assert!(
            events.len() > at_answer,
            "the log grew after the tool answered: {at_answer} then {}",
            events.len()
        );
        assert!(
            matches!(
                events.last(),
                Some(FeatureEvent::Finished(FeatureVerdict::Failed { report }))
                    if report.contains("no sub-issues")
            ),
            "the narration ends with the run's own verdict: {events:?}"
        );
    }

    #[test]
    fn a_finished_run_frees_the_slot_for_the_next_launch() {
        let w = World::new();
        let root = TempDir::new().unwrap();
        let launcher = launcher(&w, root.path(), Fake::instant(), Some("ghp-test"));

        let first = launcher.launch(7).unwrap();
        finished(&launcher);

        let second = launcher.launch(9).unwrap();
        assert_eq!(second.run, "feature-9");
        assert_ne!(first.log, second.log, "every run gets a file of its own");
        finished(&launcher);
    }

    #[test]
    fn a_missing_capability_is_the_tools_typed_refusal_and_no_run_starts() {
        let w = World::new();
        let root = TempDir::new().unwrap();
        // No override and an empty keystore: the token layer cannot stand.
        let launcher = launcher(&w, root.path(), Fake::instant(), None);

        let error = launcher.launch(7).unwrap_err();

        let Error::Refused { refusals } = &error else {
            panic!("a failed preflight refuses: {error:?}");
        };
        assert!(
            refusals
                .iter()
                .any(|refusal| refusal.capability == Capability::GithubToken),
            "{refusals:?}"
        );
        assert!(
            error.to_string().contains("the GitHub token is absent"),
            "rendered like every other tool error: {error}"
        );
        assert_eq!(launcher.running(), None, "no thread started");
        assert!(
            !root.path().join(&w.repo.owner).exists(),
            "a refused launch leaves no log file saying otherwise"
        );
        let again = launcher.launch(7).unwrap_err();
        assert!(
            matches!(again, Error::Refused { .. }),
            "a refused launch frees the slot rather than wedging it: {again:?}"
        );
    }

    #[test]
    fn debugging_a_launcher_never_spells_the_token() {
        let launch = Launch {
            repo: Repo::new("epik-agent", "Epik"),
            url: "https://github.com/epik-agent/Epik.git".to_owned(),
            base: "main".to_owned(),
            budget: Budget {
                max_tokens: None,
                max_cost: None,
                stall: Duration::from_mins(1),
            },
            patience: Duration::ZERO,
            token_override: Some("ghp-secret".to_owned()),
        };

        let debugged = format!("{launch:?}");

        assert!(
            !debugged.contains("ghp-secret"),
            "the secret must land in no log: {debugged}"
        );
        assert!(
            debugged.contains("token_override: Some"),
            "presence is still said, value never: {debugged}"
        );
    }

    #[test]
    fn the_registry_carries_the_answer_and_the_refusals_to_the_model() {
        let w = World::new();
        let root = TempDir::new().unwrap();
        let (fake, open) = Fake::gated();
        let launcher = launcher(&w, root.path(), fake, Some("ghp-test"));
        let mut registry = Registry::new();
        register(&mut registry, launcher.clone());

        let answer = registry.call("launch_feature", r#"{"number": 7}"#).unwrap();
        assert_eq!(answer["run"], "feature-7");
        assert!(
            answer["log"].as_str().unwrap().contains("feature-7-"),
            "{answer}"
        );

        let error = registry
            .call("launch_feature", r#"{"number": 9}"#)
            .unwrap_err();
        let crate::tools::Error::Failed { error, .. } = error else {
            panic!("the launcher's refusal is the tool's own failure: {error}");
        };
        assert!(
            matches!(
                error.downcast_ref::<Error>(),
                Some(Error::InFlight { running }) if running == "feature-7"
            ),
            "the vocabulary survives the trip through the registry: {error}"
        );

        open.send(()).unwrap();
        finished(&launcher);
    }

    #[test]
    fn the_tool_is_worded_for_a_model_deciding_whether_to_call() {
        let tool = launch_feature();
        assert_eq!(tool.name, "launch_feature");
        assert!(tool.description.contains("as soon as the run is started"));
        assert_eq!(tool.schema["required"], json!(["number"]));
        assert!(
            !tool.schema["properties"]["number"]["description"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn stopping_with_nothing_in_flight_is_a_quiet_no_op() {
        let w = World::new();
        let root = TempDir::new().unwrap();
        let launcher = launcher(&w, root.path(), Fake::instant(), Some("ghp-test"));
        launcher.stop();
        assert_eq!(launcher.running(), None);
    }
}
