//! The builds the app is running.
//!
//! [`Builds`] starts each build on its own thread, says once it has started
//! (its run recorded, its repository cloned), knows which are still running,
//! tells an observer whenever how many are running changes, and cancels and
//! waits for them all when the app shuts down. A build for a remote host is
//! forwarded to that host's app, which runs it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::agent::{AcpClient, Agent, AgentProfile};
use crate::build::{self, Request};
use crate::github::{GhCli, GitHub};
use crate::hosts;
use crate::ipc;
use crate::mirrors::Mirrors;
use crate::reconcile;
use crate::record::{self, Kind};
use crate::repositories;
use crate::run::{self, Disposition, RunKey, RunState};
use crate::settings::{self, Role, Settings};
use crate::setup;

/// A build that has started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Started {
    /// The run the build belongs to.
    pub run: RunKey,
    /// The build's id, unique within its run.
    pub build_id: String,
    pub kind: Kind,
    pub branch: String,
    /// The build's record directory, inside its run's, on the machine
    /// building it.
    pub build_dir: PathBuf,
    /// The remote host building it, or `None` for this computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// The builds the app is running, each on its own thread.
pub struct Builds {
    root: PathBuf,
    github: Arc<dyn GitHub>,
    /// Makes the agent a profile describes.
    make_agent: Arc<MakeAgent>,
    /// Set on shutdown: stops every build, and refuses new ones.
    cancel: Arc<AtomicBool>,
    /// The runs with a build still running.
    active: Arc<Mutex<BTreeSet<RunKey>>>,
    /// Told how many builds are running each time that changes.
    on_active: Arc<dyn Fn(usize) + Send + Sync>,
    /// Every build's thread not yet known to have ended.
    threads: Mutex<Vec<JoinHandle<()>>>,
    /// Checks what Epik needs on the machine, under the root.
    setup: Arc<SetupCheck>,
    /// Reaches the remote hosts, if this app does.
    hosts: Option<Arc<Mirrors>>,
}

/// Checks Epik's setup under a root.
type SetupCheck = dyn Fn(&Path) -> Vec<setup::Check> + Send + Sync;

/// Makes the agent a profile describes.
type MakeAgent = dyn Fn(&AgentProfile) -> Arc<dyn Agent + Send + Sync> + Send + Sync;

impl Builds {
    /// Builds under `root`, normally `~/.epik`, reaching GitHub through
    /// `github` and running each role with the agent `make_agent` makes
    /// from the role's profile.
    pub fn new(
        root: impl Into<PathBuf>,
        github: Arc<dyn GitHub>,
        make_agent: impl Fn(&AgentProfile) -> Arc<dyn Agent + Send + Sync> + Send + Sync + 'static,
    ) -> Self {
        Self {
            root: root.into(),
            github,
            make_agent: Arc::new(make_agent),
            cancel: Arc::new(AtomicBool::new(false)),
            active: Arc::default(),
            on_active: Arc::new(|_| {}),
            threads: Mutex::default(),
            setup: Arc::new(|_| Vec::new()),
            hosts: None,
        }
    }

    /// Forwards builds for a remote host through `mirrors`, which keep the
    /// hosts' mirrors. Without them, a build for a remote host is refused.
    pub fn reaching(mut self, mirrors: Arc<Mirrors>) -> Self {
        self.hosts = Some(mirrors);
        self
    }

    /// Checks Epik's setup with `check` before each build, and for
    /// [`Builds::setup`]. Without one, setup counts as complete.
    pub fn check_setup(
        mut self,
        check: impl Fn(&Path) -> Vec<setup::Check> + Send + Sync + 'static,
    ) -> Self {
        self.setup = Arc::new(check);
        self
    }

    /// Every setup item's state.
    pub fn setup(&self) -> Vec<setup::Check> {
        (self.setup)(&self.root)
    }

    /// Calls `observer` with how many builds are running each time that
    /// changes, in the order the changes happen.
    pub fn on_active(mut self, observer: impl Fn(usize) + Send + Sync + 'static) -> Self {
        self.on_active = Arc::new(observer);
        self
    }

    /// Builds under `root` with [`GhCli`] and each role's agent through
    /// [`AcpClient`], checking the machine's setup before each.
    pub fn real(root: impl Into<PathBuf>) -> Self {
        Self::new(root, Arc::new(GhCli::new()), |profile: &AgentProfile| {
            Arc::new(AcpClient::new(profile.clone()))
        })
        .check_setup(|root| {
            let latest = crate::install::Cached {
                root: root.to_owned(),
                sources: crate::install::Sources::from_env(),
            };
            setup::check_all(root, &setup::System, &latest)
        })
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
    ///
    /// The build runs with `roles`, or without them, with the settings.
    /// Before anything is recorded, the coding agent, and the review agent
    /// when review is on, are checked: an agent that can't run unattended
    /// with its model refuses the build.
    pub fn start(
        &self,
        repo: &str,
        number: u64,
        branch: Option<String>,
        roles: Option<Settings>,
    ) -> Result<Started, String> {
        if !repositories::is_repo(repo) {
            return Err(format!("{repo:?} is not a repository: write owner/name"));
        }
        if let Some(refusal) = setup::refusal(&self.setup()) {
            return Err(refusal);
        }
        let roles = match roles {
            Some(roles) => roles,
            None => settings::read(&self.root)
                .map_err(|error| format!("couldn't read the settings: {error}"))?,
        };
        let agent = self.checked_agent("coding", &roles.coding)?;
        let reviewer = match &roles.review {
            Some(review) => self.checked_agent("review", review)?,
            None => agent.clone(),
        };
        let request = Request {
            repo: repo.to_owned(),
            number,
            branch,
            root: self.root.clone(),
            roles,
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
                        Ok(build) => build.reviewer(&*reviewer),
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
                        host: None,
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

    /// Starts building `number` in `repo` on `host`, or on the default host
    /// for none, and returns once the build has started, or with why it
    /// couldn't, as a sentence. A run on one host isn't built on another.
    ///
    /// On this computer it is [`Builds::start`]. A remote host is sent the
    /// build with `roles`, or this computer's settings, since it has
    /// settings of its own; it refuses a build it can't run. Once it has
    /// started, the repository is monitored here too, and the host's mirror
    /// synced.
    pub fn build(
        &self,
        host: Option<&str>,
        repo: &str,
        number: u64,
        branch: Option<String>,
        roles: Option<Settings>,
    ) -> Result<Started, String> {
        let host = match host {
            Some(host) => Some(host.to_owned()),
            None => hosts::default(&self.root)
                .map_err(|error| format!("couldn't build {repo}#{number}: {error}"))?,
        };
        let failed = |why: String| match &host {
            Some(host) => format!("couldn't build {repo}#{number} on {host}: {why}"),
            None => format!("couldn't build {repo}#{number}: {why}"),
        };
        if let Some(host) = &host {
            let listed = hosts::list(&self.root)
                .map_err(|error| failed(format!("couldn't read the hosts: {error}")))?;
            if !listed.iter().any(|listed| &listed.destination == host) {
                return Err(format!(
                    "couldn't build {repo}#{number}: {host:?} is not a host: add it in Settings"
                ));
            }
        }
        if repositories::is_repo(repo)
            && let Some(elsewhere) = self.host_of(&RunKey::new(repo, number), host.as_deref())
        {
            let key = RunKey::new(repo, number);
            return Err(failed(format!(
                "{key} is on {}: rerun or delete it there",
                elsewhere.as_deref().unwrap_or("this computer")
            )));
        }
        match &host {
            None => self.start(repo, number, branch, roles).map_err(failed),
            Some(host) => self
                .forward(host, repo, number, branch, roles)
                .map_err(failed),
        }
    }

    /// The host other than `building_on` that has a run `key`, `None` within
    /// for this computer, if any does.
    fn host_of(&self, key: &RunKey, building_on: Option<&str>) -> Option<Option<String>> {
        let has = |root: &Path| run::run_dir(root, key).is_dir();
        if building_on.is_some() && has(&self.root) {
            return Some(None);
        }
        let listed = hosts::list(&self.root).unwrap_or_default();
        listed
            .into_iter()
            .map(|host| host.destination)
            .filter(|host| Some(host.as_str()) != building_on)
            .find(|host| has(&hosts::dir(&self.root, host)))
            .map(Some)
    }

    /// Sends the build to `host`, with `roles` or this computer's settings.
    fn forward(
        &self,
        host: &str,
        repo: &str,
        number: u64,
        branch: Option<String>,
        roles: Option<Settings>,
    ) -> Result<Started, String> {
        let roles = match roles {
            Some(roles) => roles,
            None => settings::read(&self.root)
                .map_err(|error| format!("couldn't read the settings: {error}"))?,
        };
        let request = ipc::Request::Build {
            repo: repo.to_owned(),
            number,
            branch,
            roles: Some(roles),
            host: None,
        };
        let started = match self.ask(host, &request)? {
            ipc::Response::Started(started) => started,
            ipc::Response::Error { message } => {
                return Err(refusal(message, &format!("{repo}#{number}"), "build"));
            }
            other => return Err(format!("{host} answered the build with {other:?}")),
        };
        if let Err(error) = repositories::add(&self.root, repo, false, Utc::now()) {
            eprintln!("epik: couldn't monitor {repo}: {error}");
        }
        self.sync(host);
        Ok(Started {
            host: Some(host.to_owned()),
            ..started
        })
    }

    /// Sends `request` to the remote `host`'s app, which must be listed,
    /// and returns its answer, or why it couldn't be asked.
    pub fn ask(&self, host: &str, request: &ipc::Request) -> Result<ipc::Response, String> {
        self.reaching_host(host)?
            .request(host, request)
            .map_err(|error| error.to_string())
    }

    /// Checks the remote `host`, which must be listed: records its version,
    /// or why it couldn't be reached, and its setup items.
    pub fn check_host(&self, host: &str) -> Result<(), String> {
        self.reaching_host(host)?
            .check(host)
            .map_err(|error| error.to_string())
    }

    /// The mirrors that reach the remote `host`, once it is known to be
    /// listed.
    fn reaching_host(&self, host: &str) -> Result<&Mirrors, String> {
        let listed =
            hosts::list(&self.root).map_err(|error| format!("couldn't read the hosts: {error}"))?;
        if !listed.iter().any(|listed| listed.destination == host) {
            return Err(format!("{host:?} is not a host: add it in Settings"));
        }
        self.hosts
            .as_deref()
            .ok_or_else(|| "this Epik app doesn't reach other hosts".to_owned())
    }

    /// Syncs `host`'s mirror at once, since something changed there.
    fn sync(&self, host: &str) {
        if let Some(mirrors) = &self.hosts {
            mirrors.sync_now(host);
        }
    }

    /// Reruns `run` on `host`, or on this computer for none, and returns
    /// once the build has started, or with why it couldn't. A remote host
    /// reruns it as [`Builds::rerun`] does, and its mirror is synced.
    pub fn rerun_on(&self, host: Option<&str>, run: &RunKey) -> Result<Started, String> {
        let Some(host) = host else {
            return self.rerun(run);
        };
        let request = ipc::Request::Rerun {
            run: run.clone(),
            host: None,
        };
        match self.ask(host, &request)? {
            ipc::Response::Started(started) => {
                self.sync(host);
                Ok(Started {
                    host: Some(host.to_owned()),
                    ..started
                })
            }
            ipc::Response::Error { message } => Err(refusal(message, &run.to_string(), "rerun")),
            other => Err(format!("{host} answered the rerun with {other:?}")),
        }
    }

    /// Deletes `run` on `host`, or on this computer for none, or says why
    /// it couldn't. A remote host deletes it as [`Builds::delete`] does,
    /// and its mirror is synced, which removes the run here too.
    pub fn delete_on(&self, host: Option<&str>, run: &RunKey) -> Result<(), String> {
        let Some(host) = host else {
            return self.delete(run);
        };
        let request = ipc::Request::Delete {
            run: run.clone(),
            host: None,
        };
        match self.ask(host, &request)? {
            ipc::Response::Deleted { .. } => {
                self.sync(host);
                Ok(())
            }
            ipc::Response::Error { message } => Err(refusal(message, &run.to_string(), "delete")),
            other => Err(format!("{host} answered the deletion with {other:?}")),
        }
    }

    /// Starts a new build of `run`, which needs attention, as its last
    /// build was started: with the same branch and roles, whatever the
    /// settings now say. Like any build, it skips the issues already closed.
    pub fn rerun(&self, run: &RunKey) -> Result<Started, String> {
        let state = self.needing_attention(run)?;
        let latest = record::list_builds(&self.root, run)
            .ok()
            .and_then(|builds| builds.into_iter().last())
            .ok_or_else(|| format!("couldn't read {run}'s builds"))?;
        self.start(
            &state.repo,
            state.number,
            Some(latest.branch),
            Some(latest.roles),
        )
    }

    /// The agent for `role`, called `name`, once it has checked that it can
    /// run unattended with the role's model.
    fn checked_agent(
        &self,
        name: &str,
        role: &Role,
    ) -> Result<Arc<dyn Agent + Send + Sync>, String> {
        let agent = (self.make_agent)(&settings::profile(role)?);
        agent
            .check(&self.root, role.model.as_deref())
            .map_err(|why| format!("the {name} agent can't run: {why}"))?;
        Ok(agent)
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
            Disposition::Building | Disposition::Reviewing => {
                Err(format!("a build of {run} is running"))
            }
            Disposition::WaitingForReview => Err(format!(
                "{run} is waiting for review, and leaves by itself when its issue closes"
            )),
        }
    }

    /// Checks GitHub for every run not building: see [`reconcile`].
    pub fn reconcile(&self) -> reconcile::Report {
        reconcile::reconcile(&self.root, &*self.github, &self.active())
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

/// Why a remote host couldn't `verb` `what`, from its `message`, which
/// says what it couldn't do as well.
fn refusal(message: String, what: &str, verb: &str) -> String {
    match message.strip_prefix(&format!("couldn't {verb} {what}: ")) {
        Some(why) => why.to_owned(),
        None => message,
    }
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

    fn check(&self, _cwd: &std::path::Path, _model: Option<&str>) -> Result<(), String> {
        Ok(())
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
    use crate::hosts;
    use crate::mirrors::Mirrors;
    use crate::record::{self, BuildStatus};

    fn builds(root: &Path, github: crate::github::FakeGitHub) -> Builds {
        Builds::new(root, Arc::new(github), |_: &AgentProfile| {
            Arc::new(HangingAgent)
        })
    }

    #[test]
    fn start_returns_once_the_build_has_started_and_shutdown_cancels_it() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());

        let started = builds.start("owner/repo", 7, None, None).unwrap();

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

        builds.start("owner/repo", 7, None, None).unwrap();
        builds.start("owner/repo", 8, None, None).unwrap();
        assert_eq!(*heard.lock().unwrap(), [1, 2]);

        builds.shutdown();
        assert_eq!(*heard.lock().unwrap(), [1, 2, 1, 0]);
    }

    #[test]
    fn start_takes_the_branch_it_is_given() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());

        let started = builds
            .start("owner/repo", 7, Some("topic".to_owned()), None)
            .unwrap();

        assert_eq!(started.branch, "topic");
    }

    #[test]
    fn a_build_that_cannot_start_says_why_and_is_not_active() {
        let root = tempfile::tempdir().unwrap();
        let github = fake_github();
        github.set_state("owner/repo", 7, State::Closed);
        let builds = builds(root.path(), github);

        let error = builds.start("owner/repo", 7, None, None).unwrap_err();

        assert_eq!(error, "it is closed");
        assert!(builds.active().is_empty());
    }

    #[test]
    fn starting_a_build_in_an_unlisted_repository_monitors_it() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());

        builds.start("owner/repo", 7, None, None).unwrap();

        let monitored = repositories::list(root.path()).unwrap();
        assert_eq!(monitored.len(), 1);
        assert_eq!(monitored[0].repo, "owner/repo");
        assert!(!monitored[0].added_by_user);
    }

    #[test]
    fn a_build_is_refused_up_front_when_setup_is_incomplete() {
        let root = tempfile::tempdir().unwrap();
        let no_node = setup::tests::Recorded::complete().answer("node --version", false, "");
        let builds = builds(root.path(), fake_github())
            .check_setup(move |root| setup::check_all(root, &no_node, &setup::Unknown));

        let error = builds.start("owner/repo", 7, None, None).unwrap_err();

        assert_eq!(
            error,
            "Epik's setup is incomplete: Node. Choose Check setup in the Epik menu."
        );
        assert!(run::list(root.path()).unwrap().is_empty());
        assert_eq!(builds.setup().len(), 6);
    }

    #[test]
    fn a_repository_that_isnt_owner_slash_name_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());

        for bad in ["github.com/owner/repo", "owner", "../owner/repo"] {
            let error = builds.start(bad, 7, None, None).unwrap_err();
            assert!(
                error.ends_with("is not a repository: write owner/name"),
                "{error}"
            );
        }
        assert!(run::list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn one_epic_spelled_two_ways_is_one_run() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());
        builds.start("owner/repo", 7, None, None).unwrap();

        let error = builds.start("Owner/Repo", 7, None, None).unwrap_err();

        assert_eq!(error, "a build of owner/repo#7 is already running");
    }

    #[test]
    fn a_second_build_of_a_running_run_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());
        builds.start("owner/repo", 7, None, None).unwrap();

        let error = builds.start("owner/repo", 7, None, None).unwrap_err();

        assert_eq!(error, "a build of owner/repo#7 is already running");
        assert_eq!(builds.active().len(), 1);
    }

    #[test]
    fn building_a_run_again_adds_a_build_to_it() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());
        let first = builds.start("owner/repo", 7, None, None).unwrap();
        builds.shutdown();
        let builds =
            super::Builds::new(root.path(), Arc::new(fake_github()), |_: &AgentProfile| {
                Arc::new(HangingAgent)
            });

        let second = builds.start("owner/repo", 7, None, None).unwrap();

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
            .start("owner/repo", 7, Some("topic".to_owned()), None)
            .unwrap();
        builds.shutdown();
        started
    }

    #[test]
    fn rerunning_adds_a_build_with_the_same_branch() {
        let root = tempfile::tempdir().unwrap();
        let first = interrupted(root.path());
        let builds = builds(root.path(), fake_github());

        let second = builds.rerun(&first.run).unwrap();

        assert_eq!(second.run, first.run);
        assert_eq!(second.branch, "topic");
        let latest = record::list_builds(root.path(), &first.run).unwrap();
        assert_eq!(latest.len(), 2);
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
        let started = first.start("owner/repo", 1, None, None).unwrap();
        first.shutdown();

        let second = builds(root.path(), github());
        second.rerun(&started.run).unwrap();

        // `rerun` returns once the build has started; its thread starts issue 3 a moment later.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let run = loop {
            let run = run::read(root.path(), &started.run).unwrap();
            let running = run
                .issue(3)
                .is_some_and(|issue| issue.status == record::IssueStatus::Running);
            if running || std::time::Instant::now() >= deadline {
                break run;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(run.builds.len(), 2);
        assert_eq!(run.issue(2).unwrap().status, record::IssueStatus::Closed);
        assert_eq!(run.issue(3).unwrap().status, record::IssueStatus::Running);
    }

    #[test]
    fn a_running_or_unknown_run_is_not_rerun_or_deleted() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());
        let started = builds.start("owner/repo", 7, None, None).unwrap();

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

    /// What a [`Recording`] agent was asked, in order.
    #[derive(Debug, Default)]
    struct Asked {
        /// The profile of each agent made.
        made: Vec<String>,
        /// The model of each check.
        checked: Vec<Option<String>>,
        /// The model of each run.
        ran: Vec<Option<String>>,
    }

    /// An [`Agent`] that keeps what it is asked, refuses one model in its
    /// check, and runs until cancelled.
    struct Recording {
        asked: Arc<Mutex<Asked>>,
        refused: Option<String>,
    }

    impl Agent for Recording {
        fn run(
            &self,
            request: &crate::agent::AgentRequest,
            cancel: &AtomicBool,
        ) -> crate::agent::AgentOutcome {
            self.asked.lock().unwrap().ran.push(request.model.clone());
            HangingAgent.run(request, cancel)
        }

        fn check(&self, _cwd: &Path, model: Option<&str>) -> Result<(), String> {
            self.asked
                .lock()
                .unwrap()
                .checked
                .push(model.map(str::to_owned));
            match model {
                Some(model) if Some(model) == self.refused.as_deref() => Err(format!(
                    "Claude Code doesn't offer the model `{model}`; its models are: sonnet, opus"
                )),
                _ => Ok(()),
            }
        }
    }

    /// Builds under `root` whose agents are [`Recording`]s refusing
    /// `refused`, and what they are asked.
    fn recording(root: &Path, refused: Option<&str>) -> (Builds, Arc<Mutex<Asked>>) {
        let asked = Arc::new(Mutex::new(Asked::default()));
        let builds = Builds::new(root, Arc::new(fake_github()), {
            let asked = asked.clone();
            let refused = refused.map(str::to_owned);
            move |profile: &AgentProfile| {
                asked.lock().unwrap().made.push(profile.id.clone());
                Arc::new(Recording {
                    asked: asked.clone(),
                    refused: refused.clone(),
                })
            }
        });
        (builds, asked)
    }

    fn roles(coding: Option<&str>, review: Option<Option<&str>>) -> Settings {
        let role = |model: Option<&str>| settings::Role {
            agent: "claude".to_owned(),
            model: model.map(str::to_owned),
        };
        Settings {
            coding: role(coding),
            review: review.map(role),
        }
    }

    /// The models the agents ran, once one has.
    fn ran(asked: &Mutex<Asked>) -> Vec<Option<String>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let ran = asked.lock().unwrap().ran.clone();
            if !ran.is_empty() || std::time::Instant::now() >= deadline {
                return ran;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The roles `started` recorded.
    fn recorded(root: &Path, started: &Started) -> Settings {
        let events = record::read_events(root, &started.run, &started.build_id).unwrap();
        record::fold(&events).unwrap().roles
    }

    #[test]
    fn a_build_without_roles_uses_the_settings() {
        let root = tempfile::tempdir().unwrap();
        let settings = roles(Some("opus"), None);
        settings::write(root.path(), &settings).unwrap();
        let (builds, asked) = recording(root.path(), None);

        let started = builds.start("owner/repo", 7, None, None).unwrap();

        assert_eq!(ran(&asked), [Some("opus".to_owned())]);
        let asked = asked.lock().unwrap();
        assert_eq!(asked.made, ["claude"]);
        assert_eq!(asked.checked, [Some("opus".to_owned())]);
        assert_eq!(recorded(root.path(), &started), settings);
    }

    #[test]
    fn a_build_with_roles_uses_those() {
        let root = tempfile::tempdir().unwrap();
        settings::write(root.path(), &roles(Some("opus"), None)).unwrap();
        let (builds, asked) = recording(root.path(), None);
        let given = roles(Some("sonnet"), Some(Some("haiku")));

        let started = builds
            .start("owner/repo", 7, None, Some(given.clone()))
            .unwrap();

        assert_eq!(ran(&asked), [Some("sonnet".to_owned())]);
        let asked = asked.lock().unwrap();
        assert_eq!(asked.made, ["claude", "claude"]);
        assert_eq!(
            asked.checked,
            [Some("sonnet".to_owned()), Some("haiku".to_owned())]
        );
        assert_eq!(recorded(root.path(), &started), given);
    }

    #[test]
    fn a_rerun_uses_the_roles_its_last_build_recorded() {
        let root = tempfile::tempdir().unwrap();
        let first_roles = roles(Some("opus"), Some(None));
        settings::write(root.path(), &first_roles).unwrap();
        let (builds, _) = recording(root.path(), None);
        let first = builds.start("owner/repo", 7, None, None).unwrap();
        builds.shutdown();
        settings::write(root.path(), &roles(Some("sonnet"), None)).unwrap();
        let (builds, asked) = recording(root.path(), None);

        let second = builds.rerun(&first.run).unwrap();

        assert_eq!(ran(&asked), [Some("opus".to_owned())]);
        assert_eq!(
            asked.lock().unwrap().checked,
            [Some("opus".to_owned()), None]
        );
        assert_eq!(recorded(root.path(), &second), first_roles);
    }

    #[test]
    fn an_agent_that_fails_its_check_refuses_the_build() {
        let root = tempfile::tempdir().unwrap();
        let (builds, asked) = recording(root.path(), Some("gpt-5"));

        for (given, role) in [
            (roles(Some("gpt-5"), None), "coding"),
            (roles(None, Some(Some("gpt-5"))), "review"),
        ] {
            let error = builds
                .start("owner/repo", 7, None, Some(given))
                .unwrap_err();

            assert_eq!(
                error,
                format!(
                    "the {role} agent can't run: Claude Code doesn't offer the model `gpt-5`; \
                     its models are: sonnet, opus"
                )
            );
        }
        assert!(asked.lock().unwrap().ran.is_empty());
        assert!(builds.active().is_empty());
        assert!(run::list(root.path()).unwrap().is_empty());
        assert!(!crate::cleanup::builds_dir(root.path()).exists());
    }

    #[test]
    fn a_role_naming_an_agent_epik_does_not_know_refuses_the_build() {
        let root = tempfile::tempdir().unwrap();
        let (builds, asked) = recording(root.path(), None);
        let mut given = roles(None, None);
        given.coding.agent = "aider".to_owned();

        let error = builds
            .start("owner/repo", 7, None, Some(given))
            .unwrap_err();

        assert_eq!(
            error,
            "\"aider\" is not an agent Epik knows: the agents are claude, codex"
        );
        assert!(asked.lock().unwrap().made.is_empty());
        assert!(run::list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn no_build_starts_after_shutdown() {
        let root = tempfile::tempdir().unwrap();
        let builds = builds(root.path(), fake_github());
        builds.shutdown();

        let error = builds.start("owner/repo", 7, None, None).unwrap_err();

        assert_eq!(error, "the Epik app is shutting down");
        assert!(!root.path().join("runs").exists());
    }

    /// A host's app that records the builds, reruns, deletions and status
    /// it is asked for, and refuses the first three with `refusal` when
    /// there is one. It answers the rest from the records under `root`.
    struct Remote {
        root: tempfile::TempDir,
        refusal: Option<String>,
        asked: Mutex<Vec<crate::ipc::Request>>,
        syncs: std::sync::atomic::AtomicUsize,
    }

    impl Remote {
        fn new(refusal: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                root: tempfile::tempdir().unwrap(),
                refusal: refusal.map(str::to_owned),
                asked: Mutex::default(),
                syncs: Default::default(),
            })
        }

        fn asked(&self) -> Vec<crate::ipc::Request> {
            self.asked.lock().unwrap().clone()
        }

        fn syncs(&self) -> usize {
            self.syncs.load(Ordering::SeqCst)
        }

        /// Waits up to five seconds for the host to have been synced
        /// `times`, and says how many times it was.
        fn synced(&self, times: usize) -> usize {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while self.syncs() < times && std::time::Instant::now() < until {
                thread::sleep(std::time::Duration::from_millis(10));
            }
            self.syncs()
        }

        /// The refusal to `verb` `run`, the way an app words it, if this
        /// host refuses.
        fn refuses(&self, verb: &str, run: &RunKey) -> Option<crate::ipc::Response> {
            self.refusal
                .as_ref()
                .map(|why| crate::ipc::Response::Error {
                    message: format!("couldn't {verb} {run}: {why}"),
                })
        }
    }

    impl crate::remote::Client for Remote {
        fn request(&self, request: &crate::ipc::Request) -> std::io::Result<crate::ipc::Response> {
            use crate::ipc::{Request, Response};
            let started = |run: RunKey| {
                Response::Started(Started {
                    run,
                    build_id: "20261101T120000Z-abcd".to_owned(),
                    kind: Kind::Issue,
                    branch: "main".to_owned(),
                    build_dir: "/home/me/.epik/runs/owner/repo/7/20261101T120000Z-abcd".into(),
                    host: None,
                })
            };
            Ok(match request {
                Request::Build { repo, number, .. } => {
                    self.asked.lock().unwrap().push(request.clone());
                    let run = RunKey::new(repo, *number);
                    self.refuses("build", &run).unwrap_or_else(|| started(run))
                }
                Request::Rerun { run, .. } => {
                    self.asked.lock().unwrap().push(request.clone());
                    self.refuses("rerun", run)
                        .unwrap_or_else(|| started(run.clone()))
                }
                Request::Delete { run, .. } => {
                    self.asked.lock().unwrap().push(request.clone());
                    self.refuses("delete", run)
                        .unwrap_or_else(|| Response::Deleted { run: run.clone() })
                }
                Request::Status { run, .. } => {
                    self.asked.lock().unwrap().push(request.clone());
                    crate::ipc::status(self.root.path(), run)
                }
                Request::Ping => Response::Pong {
                    version: crate::version().to_owned(),
                },
                Request::Setup => Response::Setup { items: Vec::new() },
                Request::Runs => {
                    self.syncs.fetch_add(1, Ordering::SeqCst);
                    crate::ipc::records(self.root.path(), request.clone())
                }
                other => crate::ipc::records(self.root.path(), other.clone()),
            })
        }
    }

    /// Builds under `root`, which lists the host `devbox`, reached as
    /// `remote`.
    fn on_hosts(root: &Path, remote: &Arc<Remote>) -> (Builds, Arc<Mirrors>) {
        hosts::add(root, "devbox", Utc::now()).unwrap();
        let mut mirrors = Mirrors::new(root, {
            let remote = remote.clone();
            move |_| remote.clone()
        });
        mirrors.idle = std::time::Duration::from_secs(3600);
        let mirrors = Arc::new(mirrors);
        let builds = builds(root, fake_github()).reaching(mirrors.clone());
        (builds, mirrors)
    }

    fn opus() -> Settings {
        Settings {
            coding: Role {
                agent: "claude".to_owned(),
                model: Some("opus".to_owned()),
            },
            review: None,
        }
    }

    /// The request a build of `owner/repo#7` forwarded with `roles` is.
    fn forwarded(roles: Settings) -> crate::ipc::Request {
        crate::ipc::Request::Build {
            repo: "owner/repo".to_owned(),
            number: 7,
            branch: None,
            roles: Some(roles),
            host: None,
        }
    }

    #[test]
    fn a_listed_host_is_checked_through_its_mirrors_client() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);

        builds.check_host("devbox").unwrap();
        let refused = builds.check_host("elsewhere").unwrap_err();

        let host = &hosts::list(root.path()).unwrap()[0];
        assert!(
            matches!(host.state, Some(hosts::State::Reachable { .. })),
            "{host:?}"
        );
        assert_eq!(host.setup, Some(Vec::new()));
        assert_eq!(refused, "\"elsewhere\" is not a host: add it in Settings");
    }

    #[test]
    fn a_build_on_a_host_is_forwarded_with_this_computers_roles() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);
        settings::write(root.path(), &opus()).unwrap();

        let started = builds
            .build(Some("devbox"), "owner/repo", 7, None, None)
            .unwrap();

        assert_eq!(remote.asked(), [forwarded(opus())]);
        assert_eq!(started.host.as_deref(), Some("devbox"));
        assert_eq!(started.run, RunKey::new("owner/repo", 7));
        assert_eq!(started.build_id, "20261101T120000Z-abcd");
        assert!(builds.active().is_empty(), "nothing builds here");
        assert!(run::keys(root.path()).unwrap().is_empty());
    }

    #[test]
    fn a_build_with_its_own_roles_forwards_those() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);

        builds
            .build(Some("devbox"), "owner/repo", 7, None, Some(opus()))
            .unwrap();

        assert_eq!(remote.asked(), [forwarded(opus())]);
    }

    #[test]
    fn a_build_without_a_host_goes_to_the_default_host() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);
        hosts::set_default(root.path(), Some("devbox")).unwrap();

        let started = builds.build(None, "owner/repo", 7, None, None).unwrap();

        assert_eq!(remote.asked(), [forwarded(Settings::default())]);
        assert_eq!(started.host.as_deref(), Some("devbox"));
    }

    #[test]
    fn a_build_without_a_host_or_a_default_builds_here() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);

        let started = builds.build(None, "owner/repo", 7, None, None).unwrap();

        assert_eq!(started.host, None);
        assert!(remote.asked().is_empty());
        assert_eq!(builds.active(), [RunKey::new("owner/repo", 7)]);
    }

    #[test]
    fn a_build_on_a_host_monitors_its_repository_here_and_syncs_the_host() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, mirrors) = on_hosts(root.path(), &remote);
        mirrors.refresh().unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while remote.syncs() < 1 && std::time::Instant::now() < until {
            thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(remote.syncs(), 1);

        builds
            .build(Some("devbox"), "owner/repo", 7, None, None)
            .unwrap();

        let monitored = repositories::list(root.path()).unwrap();
        assert_eq!(monitored.len(), 1);
        assert_eq!(monitored[0].repo, "owner/repo");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while remote.syncs() < 2 && std::time::Instant::now() < until {
            thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(remote.syncs(), 2, "synced at once");
    }

    #[test]
    fn a_hosts_refusal_is_the_error() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(Some("the coding agent can't run: no model opus"));
        let (builds, _mirrors) = on_hosts(root.path(), &remote);

        let error = builds
            .build(Some("devbox"), "owner/repo", 7, None, Some(opus()))
            .unwrap_err();

        assert_eq!(
            error,
            "couldn't build owner/repo#7 on devbox: the coding agent can't run: no model opus"
        );
        assert!(repositories::list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn a_host_that_isnt_listed_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);

        let error = builds
            .build(Some("elsewhere"), "owner/repo", 7, None, None)
            .unwrap_err();

        assert_eq!(
            error,
            "couldn't build owner/repo#7: \"elsewhere\" is not a host: add it in Settings"
        );
        assert!(remote.asked().is_empty());
    }

    /// Records a finished build of `owner/repo#7` under `root`.
    fn recorded_run(root: &Path) {
        let at = Utc::now();
        let key = RunKey::new("owner/repo", 7);
        let mut writer = record::BuildWriter::create_new(root, &key, at).unwrap();
        writer
            .append(&record::Event::BuildStarted {
                at,
                build_id: writer.build_id().to_owned(),
                repo: "owner/repo".to_owned(),
                number: 7,
                title: "An issue".to_owned(),
                kind: Kind::Issue,
                branch: "main".to_owned(),
                issues: Vec::new(),
                closed: Vec::new(),
                roles: Settings::default(),
            })
            .unwrap();
    }

    #[test]
    fn a_run_on_this_computer_is_not_built_on_a_host() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);
        recorded_run(root.path());

        let error = builds
            .build(Some("devbox"), "Owner/Repo", 7, None, None)
            .unwrap_err();

        assert_eq!(
            error,
            "couldn't build Owner/Repo#7 on devbox: owner/repo#7 is on this computer: \
             rerun or delete it there"
        );
        assert!(remote.asked().is_empty());
    }

    #[test]
    fn a_run_on_a_host_is_built_only_there() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);
        hosts::add(root.path(), "other", Utc::now()).unwrap();
        recorded_run(&hosts::dir(root.path(), "devbox"));

        let here = builds.build(None, "owner/repo", 7, None, None).unwrap_err();
        let other = builds
            .build(Some("other"), "owner/repo", 7, None, None)
            .unwrap_err();
        let there = builds.build(Some("devbox"), "owner/repo", 7, None, None);

        assert_eq!(
            here,
            "couldn't build owner/repo#7: owner/repo#7 is on devbox: rerun or delete it there"
        );
        assert_eq!(
            other,
            "couldn't build owner/repo#7 on other: owner/repo#7 is on devbox: \
             rerun or delete it there"
        );
        assert!(there.is_ok(), "{there:?}");
        assert!(builds.active().is_empty());
    }

    /// Builds under `root` reaching `devbox` as `remote`, with its mirror
    /// followed and synced once.
    fn followed(root: &Path, remote: &Arc<Remote>) -> (Builds, Arc<Mirrors>) {
        let (builds, mirrors) = on_hosts(root, remote);
        mirrors.refresh().unwrap();
        assert_eq!(remote.synced(1), 1);
        (builds, mirrors)
    }

    #[test]
    fn a_rerun_on_a_host_is_forwarded_and_the_host_synced() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = followed(root.path(), &remote);
        let run = RunKey::new("owner/repo", 7);

        let started = builds.rerun_on(Some("devbox"), &run).unwrap();

        assert_eq!(
            remote.asked(),
            [crate::ipc::Request::Rerun {
                run: run.clone(),
                host: None
            }]
        );
        assert_eq!(started.host.as_deref(), Some("devbox"));
        assert_eq!(started.run, run);
        assert!(builds.active().is_empty(), "nothing builds here");
        assert_eq!(remote.synced(2), 2, "synced at once");
    }

    #[test]
    fn a_delete_on_a_host_is_forwarded_and_the_host_synced() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = followed(root.path(), &remote);
        let run = RunKey::new("owner/repo", 7);

        builds.delete_on(Some("devbox"), &run).unwrap();

        assert_eq!(
            remote.asked(),
            [crate::ipc::Request::Delete {
                run: run.clone(),
                host: None
            }]
        );
        assert_eq!(remote.synced(2), 2, "synced at once");
    }

    #[test]
    fn a_hosts_refusal_to_rerun_or_delete_is_the_error() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(Some("a build of owner/repo#7 is running"));
        let (builds, _mirrors) = followed(root.path(), &remote);
        let run = RunKey::new("owner/repo", 7);

        let rerun = builds.rerun_on(Some("devbox"), &run).unwrap_err();
        let delete = builds.delete_on(Some("devbox"), &run).unwrap_err();

        assert_eq!(rerun, "a build of owner/repo#7 is running");
        assert_eq!(delete, "a build of owner/repo#7 is running");
        thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(remote.syncs(), 1, "nothing changed there");
    }

    #[test]
    fn a_rerun_or_delete_on_a_host_that_isnt_listed_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);
        let run = RunKey::new("owner/repo", 7);

        let rerun = builds.rerun_on(Some("elsewhere"), &run).unwrap_err();
        let delete = builds.delete_on(Some("elsewhere"), &run).unwrap_err();

        let refused = "\"elsewhere\" is not a host: add it in Settings";
        assert_eq!(rerun, refused);
        assert_eq!(delete, refused);
        assert!(remote.asked().is_empty());
    }

    #[test]
    fn a_rerun_or_delete_without_a_host_is_done_here() {
        let root = tempfile::tempdir().unwrap();
        let first = interrupted(root.path());
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);
        hosts::set_default(root.path(), Some("devbox")).unwrap();

        let second = builds.rerun_on(None, &first.run).unwrap();
        builds.shutdown();
        builds.delete_on(None, &first.run).unwrap();

        assert_eq!(second.host, None);
        assert!(remote.asked().is_empty(), "not the default host");
        assert!(run::list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn a_status_on_a_host_is_the_hosts_answer() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(None);
        let (builds, _mirrors) = on_hosts(root.path(), &remote);
        recorded_run(remote.root.path());
        let run = RunKey::new("owner/repo", 7);
        let request = crate::ipc::Request::Status {
            run: run.clone(),
            host: None,
        };

        let reply = builds.ask("devbox", &request).unwrap();

        let crate::ipc::Response::Run(state) = reply else {
            panic!("{reply:?}");
        };
        assert_eq!(state.run, run);
        assert_eq!(remote.asked(), [request]);
    }
}
