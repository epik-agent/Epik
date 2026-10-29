//! The builds the app is running.
//!
//! [`Builds`] starts each build on its own thread, says once it has started
//! (its run recorded, its repository cloned), knows which are still running,
//! tells an observer whenever how many are running changes, and cancels and
//! waits for them all when the app shuts down.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::agent::{Agent, ClaudeAcp};
use crate::build::{self, Request};
use crate::github::{GhCli, GitHub};
use crate::record::{self, Kind};
use crate::repositories;
use crate::run::{self, Disposition, RunKey, RunState};

/// A build that has started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Started {
    /// The run the build belongs to.
    pub run: RunKey,
    /// The build's id, unique within its run.
    pub build_id: String,
    pub kind: Kind,
    pub branch: String,
    /// The build's record directory, inside its run's.
    pub build_dir: PathBuf,
}

/// The builds the app is running, each on its own thread.
pub struct Builds {
    root: PathBuf,
    github: Arc<dyn GitHub>,
    agent: Arc<dyn Agent + Send + Sync>,
    /// Set on shutdown: stops every build, and refuses new ones.
    cancel: Arc<AtomicBool>,
    /// The runs with a build still running.
    active: Arc<Mutex<BTreeSet<RunKey>>>,
    /// Told how many builds are running each time that changes.
    on_active: Arc<dyn Fn(usize) + Send + Sync>,
    /// Every build's thread not yet known to have ended.
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Builds {
    /// Builds under `root`, normally `~/.epik`, reaching GitHub through
    /// `github` and running each issue with `agent`.
    pub fn new(
        root: impl Into<PathBuf>,
        github: Arc<dyn GitHub>,
        agent: Arc<dyn Agent + Send + Sync>,
    ) -> Self {
        Self {
            root: root.into(),
            github,
            agent,
            cancel: Arc::new(AtomicBool::new(false)),
            active: Arc::default(),
            on_active: Arc::new(|_| {}),
            threads: Mutex::default(),
        }
    }

    /// Calls `observer` with how many builds are running each time that
    /// changes, in the order the changes happen.
    pub fn on_active(mut self, observer: impl Fn(usize) + Send + Sync + 'static) -> Self {
        self.on_active = Arc::new(observer);
        self
    }

    /// Builds under `root` with [`GhCli`] and [`ClaudeAcp`].
    pub fn real(root: impl Into<PathBuf>) -> Self {
        Self::new(root, Arc::new(GhCli::new()), Arc::new(ClaudeAcp::new()))
    }

    /// Epik's root, where clones and records go.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Starts building `number` in `repo` on a thread of its own, and
    /// returns once the build has written `build_started`, or with why it
    /// couldn't start. A build of an epic issue that already has a run adds
    /// a build to it; while one of its builds is running, another is
    /// refused.
    pub fn start(
        &self,
        repo: &str,
        number: u64,
        branch: Option<String>,
        plugin_dir: PathBuf,
    ) -> Result<Started, String> {
        let request = Request {
            repo: repo.to_owned(),
            number,
            branch,
            root: self.root.clone(),
            plugin_dir,
        };
        let key = RunKey::new(repo, number);
        let (started, starting) = mpsc::channel();
        {
            // Holding the lock while spawning means a shutdown either
            // refuses this build or waits for it.
            let mut threads = lock(&self.threads);
            if self.cancel.load(Ordering::SeqCst) {
                return Err("the Epik app is shutting down".to_owned());
            }
            threads.retain(|thread| !thread.is_finished());
            // Claimed under the threads' lock, so two requests for the same
            // run can't both start.
            {
                let mut active = lock(&self.active);
                if !active.insert(key.clone()) {
                    return Err(format!("a build of {key} is already running"));
                }
                (self.on_active)(active.len());
            }
            let github = self.github.clone();
            let agent = self.agent.clone();
            let cancel = self.cancel.clone();
            let active = self.active.clone();
            let on_active = self.on_active.clone();
            let unclaim = {
                let key = key.clone();
                let active = self.active.clone();
                let on_active = self.on_active.clone();
                move || {
                    change(&active, &*on_active, |active| {
                        active.remove(&key);
                    })
                }
            };
            let thread = thread::Builder::new()
                .name(format!("build {repo}#{number}"))
                .spawn(move || {
                    let build = match build::start(&*github, &*agent, &request) {
                        Ok(build) => build,
                        Err(error) => {
                            change(&active, &*on_active, |active| {
                                active.remove(&key);
                            });
                            let _ = started.send(Err(error.to_string()));
                            return;
                        }
                    };
                    if let Err(error) =
                        repositories::add(&request.root, &request.repo, false, Utc::now())
                    {
                        eprintln!("epik: couldn't monitor {}: {error}", request.repo);
                    }
                    let _ = started.send(Ok(Started {
                        run: key.clone(),
                        build_id: build.build_id().to_owned(),
                        kind: build.kind(),
                        branch: build.branch().to_owned(),
                        build_dir: build.dir().to_owned(),
                    }));
                    build.run(&cancel);
                    change(&active, &*on_active, |active| {
                        active.remove(&key);
                    });
                })
                .map_err(|error| {
                    unclaim();
                    format!("couldn't start a thread for the build: {error}")
                })?;
            threads.push(thread);
        }
        starting
            .recv()
            .unwrap_or_else(|_| Err("the build stopped before it started".to_owned()))
    }

    /// Starts a new build of `run`, which needs attention, as its last
    /// build was started: with the same branch and plugin. Like any build,
    /// it skips the issues already closed.
    pub fn rerun(&self, run: &RunKey) -> Result<Started, String> {
        let state = self.needing_attention(run)?;
        let latest = record::list_builds(&self.root, run)
            .ok()
            .and_then(|builds| builds.into_iter().last())
            .ok_or_else(|| format!("couldn't read {run}'s builds"))?;
        let plugin_dir = latest
            .plugin_dir
            .ok_or_else(|| format!("{run}'s last build didn't record Epik's plugin"))?;
        self.start(&state.repo, state.number, Some(latest.branch), plugin_dir)
    }

    /// Deletes `run`, which needs attention: every build's clone and
    /// record. Refused while one of its builds is running.
    pub fn delete(&self, run: &RunKey) -> Result<(), String> {
        // Held so that no build of the run starts while it is deleted.
        let _threads = lock(&self.threads);
        if lock(&self.active).contains(run) {
            return Err(format!("a build of {run} is running"));
        }
        self.needing_attention(run)?;
        run::delete(&self.root, run).map_err(|error| format!("couldn't delete {run}: {error}"))
    }

    /// `run`'s state, if it needs attention; else why not.
    fn needing_attention(&self, run: &RunKey) -> Result<RunState, String> {
        let state = run::read(&self.root, run).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!("no run {run}"),
            _ => format!("couldn't read {run}: {error}"),
        })?;
        match state.disposition {
            Disposition::NeedsAttention => Ok(state),
            Disposition::Building => Err(format!("a build of {run} is running")),
            Disposition::WaitingForReview => Err(format!(
                "{run} is waiting for review, and leaves by itself when its issue closes"
            )),
        }
    }

    /// The runs with a build still running.
    pub fn active(&self) -> Vec<RunKey> {
        lock(&self.active).iter().cloned().collect()
    }

    /// Cancels every build, waits for each to end, and refuses new ones.
    pub fn shutdown(&self) {
        let threads = {
            let mut threads = lock(&self.threads);
            self.cancel.store(true, Ordering::SeqCst);
            std::mem::take(&mut *threads)
        };
        for thread in threads {
            let _ = thread.join();
        }
    }
}

impl Drop for Builds {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Changes the running builds with `edit`, and tells `observer` how many
/// there now are. The lock is held while it's told, so it hears of the
/// changes in order.
fn change(
    active: &Mutex<BTreeSet<RunKey>>,
    observer: &(dyn Fn(usize) + Send + Sync),
    edit: impl FnOnce(&mut BTreeSet<RunKey>),
) {
    let mut active = lock(active);
    edit(&mut active);
    observer(active.len());
}

/// A lock that a panicked build thread doesn't spoil.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An [`Agent`] for tests: runs until cancelled.
#[cfg(test)]
pub(crate) struct HangingAgent;

#[cfg(test)]
impl Agent for HangingAgent {
    fn run(
        &self,
        _request: &crate::agent::AgentRequest,
        cancel: &AtomicBool,
    ) -> crate::agent::AgentOutcome {
        while !cancel.load(Ordering::SeqCst) {
            thread::sleep(std::time::Duration::from_millis(5));
        }
        crate::agent::AgentOutcome {
            exited_cleanly: false,
            stop_reason: "cancelled".to_owned(),
            duration_ms: 0,
            cost_usd: None,
            error: None,
        }
    }
}

/// A repository `owner/repo` on `main` holding open issue 7.
#[cfg(test)]
pub(crate) fn fake_github() -> crate::github::FakeGitHub {
    let github = crate::github::FakeGitHub::new();
    github.add_repo("owner/repo", "main");
    github.add_issue("owner/repo", 7, "An issue");
    github
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::State;
    use crate::record::{self, BuildStatus};

    fn builds(root: &Path, github: crate::github::FakeGitHub) -> Builds {
        Builds::new(root, Arc::new(github), Arc::new(HangingAgent))
    }

    #[test]
    fn start_returns_once_the_build_has_started_and_shutdown_cancels_it() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());

        let started = builds
            .start("owner/repo", 7, None, PathBuf::from("/plugin"))
            .unwrap();

        assert_eq!(started.run, RunKey::new("owner/repo", 7));
        assert!(started.build_id.starts_with("20"), "{started:?}");
        assert_eq!(started.kind, Kind::Issue);
        assert_eq!(started.branch, "main");
        assert_eq!(
            started.build_dir,
            record::build_dir(root.path(), &started.run, &started.build_id)
        );
        let events = record::read_events(root.path(), &started.run, &started.build_id).unwrap();
        assert!(matches!(events[0], record::Event::BuildStarted { .. }));
        assert_eq!(builds.active(), vec![started.run.clone()]);

        builds.shutdown();

        assert!(builds.active().is_empty());
        let events = record::read_events(root.path(), &started.run, &started.build_id).unwrap();
        let state = record::fold(&events).unwrap();
        assert_eq!(state.status, BuildStatus::Interrupted);
    }

    #[test]
    fn the_observer_hears_how_many_builds_are_running() {
        let root = tempfile::tempdir().unwrap();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let github = fake_github();
        github.add_issue("owner/repo", 8, "Another issue");
        let builds = builds(root.path(), github).on_active({
            let heard = heard.clone();
            move |n| heard.lock().unwrap().push(n)
        });

        builds
            .start("owner/repo", 7, None, "/plugin".into())
            .unwrap();
        builds
            .start("owner/repo", 8, None, "/plugin".into())
            .unwrap();
        assert_eq!(*heard.lock().unwrap(), [1, 2]);

        builds.shutdown();
        assert_eq!(*heard.lock().unwrap(), [1, 2, 1, 0]);
    }

    #[test]
    fn start_takes_the_branch_it_is_given() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());

        let started = builds
            .start("owner/repo", 7, Some("topic".to_owned()), "/plugin".into())
            .unwrap();

        assert_eq!(started.branch, "topic");
    }

    #[test]
    fn a_build_that_cannot_start_says_why_and_is_not_active() {
        let root = tempfile::tempdir().unwrap();
        let github = fake_github();
        github.set_state("owner/repo", 7, State::Closed);
        let builds = builds(root.path(), github);

        let error = builds
            .start("owner/repo", 7, None, "/plugin".into())
            .unwrap_err();

        assert_eq!(error, "it is closed");
        assert!(builds.active().is_empty());
    }

    #[test]
    fn starting_a_build_in_an_unlisted_repository_monitors_it() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());

        builds
            .start("owner/repo", 7, None, "/plugin".into())
            .unwrap();

        let monitored = repositories::list(root.path()).unwrap();
        assert_eq!(monitored.len(), 1);
        assert_eq!(monitored[0].repo, "owner/repo");
        assert!(!monitored[0].added_by_user);
    }

    #[test]
    fn a_second_build_of_a_running_run_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());
        builds
            .start("owner/repo", 7, None, "/plugin".into())
            .unwrap();

        let error = builds
            .start("owner/repo", 7, None, "/plugin".into())
            .unwrap_err();

        assert_eq!(error, "a build of owner/repo#7 is already running");
        assert_eq!(builds.active().len(), 1);
    }

    #[test]
    fn building_a_run_again_adds_a_build_to_it() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());
        let first = builds
            .start("owner/repo", 7, None, "/plugin".into())
            .unwrap();
        builds.shutdown();
        let builds =
            super::Builds::new(root.path(), Arc::new(fake_github()), Arc::new(HangingAgent));

        let second = builds
            .start("owner/repo", 7, None, "/plugin".into())
            .unwrap();

        assert_eq!(first.run, second.run);
        assert_ne!(first.build_id, second.build_id);
        let run = crate::run::read(root.path(), &first.run).unwrap();
        let ids: Vec<&str> = run.builds.iter().map(|b| b.build_id.as_str()).collect();
        assert_eq!(ids, [first.build_id.as_str(), second.build_id.as_str()]);
        assert_eq!(run.disposition, crate::run::Disposition::Building);
    }

    /// A build of `owner/repo#7` that was interrupted, under `root`.
    fn interrupted(root: &Path) -> Started {
        let builds = builds(root, fake_github());
        let started = builds
            .start("owner/repo", 7, Some("topic".to_owned()), "/plugin".into())
            .unwrap();
        builds.shutdown();
        started
    }

    #[test]
    fn rerunning_adds_a_build_with_the_same_branch_and_plugin() {
        let root = tempfile::tempdir().unwrap();
        let first = interrupted(root.path());
        let builds = builds(root.path(), fake_github());

        let second = builds.rerun(&first.run).unwrap();

        assert_eq!(second.run, first.run);
        assert_eq!(second.branch, "topic");
        let latest = record::list_builds(root.path(), &first.run).unwrap();
        assert_eq!(latest.len(), 2);
        assert_eq!(latest[1].plugin_dir, Some(PathBuf::from("/plugin")));
        assert_eq!(builds.active(), std::slice::from_ref(&first.run));
    }

    #[test]
    fn rerunning_an_interrupted_epic_skips_its_merged_issues() {
        let root = tempfile::tempdir().unwrap();
        let github = || {
            let github = fake_github();
            github.add_issue("owner/repo", 1, "The epic");
            for sub in [2, 3] {
                github.add_issue("owner/repo", sub, "A sub-issue");
                github.add_sub_issue("owner/repo", 1, "owner/repo", sub);
            }
            github.set_state("owner/repo", 2, State::Closed);
            github
        };
        let first = builds(root.path(), github());
        let started = first
            .start("owner/repo", 1, None, "/plugin".into())
            .unwrap();
        first.shutdown();

        let second = builds(root.path(), github());
        second.rerun(&started.run).unwrap();

        let run = run::read(root.path(), &started.run).unwrap();
        assert_eq!(run.builds.len(), 2);
        assert_eq!(run.issue(2).unwrap().status, record::IssueStatus::Closed);
        assert_eq!(run.issue(3).unwrap().status, record::IssueStatus::Running);
    }

    #[test]
    fn a_running_or_unknown_run_is_not_rerun_or_deleted() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());
        let started = builds
            .start("owner/repo", 7, None, "/plugin".into())
            .unwrap();

        let rerun = builds.rerun(&started.run).unwrap_err();
        let delete = builds.delete(&started.run).unwrap_err();

        assert_eq!(rerun, "a build of owner/repo#7 is running");
        assert_eq!(delete, "a build of owner/repo#7 is running");
        let unknown = RunKey::new("owner/repo", 9);
        assert_eq!(builds.rerun(&unknown).unwrap_err(), "no run owner/repo#9");
        assert_eq!(builds.delete(&unknown).unwrap_err(), "no run owner/repo#9");
    }

    #[test]
    fn deleting_a_run_that_needs_attention_leaves_nothing_of_it() {
        let root = tempfile::tempdir().unwrap();
        let started = interrupted(root.path());
        let builds = builds(root.path(), fake_github());

        builds.delete(&started.run).unwrap();

        assert!(run::list(root.path()).unwrap().is_empty());
        assert!(!run::run_dir(root.path(), &started.run).exists());
        let clones = crate::cleanup::builds_dir(root.path());
        assert!(!clones.exists() || std::fs::read_dir(clones).unwrap().count() == 0);
    }

    #[test]
    fn no_build_starts_after_shutdown() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());
        builds.shutdown();

        let error = builds
            .start("owner/repo", 7, None, "/plugin".into())
            .unwrap_err();

        assert_eq!(error, "the Epik app is shutting down");
        assert!(!root.path().join("runs").exists());
    }
}
