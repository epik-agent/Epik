//! Keeping every remote host's mirror current, each host on a thread of its
//! own.
//!
//! A host is synced with [`mirror::sync`] every [`BUILDING`] while any of
//! its mirrored runs is building, else every [`IDLE`], and at once when
//! asked. A sync that fails records the host as unreachable and leaves its
//! mirror as it was. Each host has one client, which its thread and the
//! requests forwarded to the host share. Nothing on this computer acts on a mirror: sweeping
//! clones, interrupting builds, reconciling and the closing comment use only
//! the local root. The decision record
//! `docs/design-history/2026-10-31-remote-hosts.md` says why.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::hosts::{self, State};
use crate::ipc::{Request, Response};
use crate::mirror;
use crate::remote::{self, Client, Ssh};
use crate::run::{self, Disposition};
use crate::settings;

/// How often a host is synced while any of its mirrored runs is building.
pub const BUILDING: Duration = Duration::from_secs(3);

/// How often a host is synced while none of its mirrored runs is building.
pub const IDLE: Duration = Duration::from_secs(60);

/// How often [`Mirrors::follow_until`] reads the list of hosts again.
const REFRESH: Duration = Duration::from_secs(5);

/// The client that reaches a host's app.
type HostClient = Arc<dyn Client + Send + Sync>;

/// Makes the client that reaches a host's app, by its destination.
type Connect = dyn Fn(&str) -> HostClient + Send + Sync;

/// A thread for each remote host, keeping its mirror current.
pub struct Mirrors {
    root: PathBuf,
    connect: Arc<Connect>,
    /// How often a host is synced while any of its runs is building.
    pub building: Duration,
    /// How often a host is synced otherwise.
    pub idle: Duration,
    /// What wakes each followed host's thread, by destination. Dropping
    /// one ends its thread.
    followed: Mutex<HashMap<String, Sender<()>>>,
    /// Each host's client, by destination, made when first wanted.
    clients: Mutex<HashMap<String, HostClient>>,
}

impl Mirrors {
    /// Mirrors for the hosts listed under `root`, reached with `connect`.
    pub fn new(
        root: impl Into<PathBuf>,
        connect: impl Fn(&str) -> HostClient + Send + Sync + 'static,
    ) -> Self {
        Self {
            root: root.into(),
            connect: Arc::new(connect),
            building: BUILDING,
            idle: IDLE,
            followed: Mutex::new(HashMap::new()),
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// Mirrors for the hosts listed under `root`, reached with `ssh`.
    pub fn real(root: impl Into<PathBuf>) -> Self {
        Self::new(root, |destination| Arc::new(Ssh::new(destination)))
    }

    /// Sends `request` to `host`'s app through the client its mirror uses.
    pub fn request(&self, host: &str, request: &Request) -> io::Result<Response> {
        self.client(host).request(request)
    }

    /// `host`'s client, made if it has none.
    fn client(&self, host: &str) -> HostClient {
        lock(&self.clients)
            .entry(host.to_owned())
            .or_insert_with(|| (self.connect)(host))
            .clone()
    }

    /// Checks `host` at once through its client, as [`check`] does.
    pub fn check(&self, host: &str) -> io::Result<()> {
        check(&self.root, host, &self.client(host), Utc::now())
    }

    /// Starts a thread for each listed host that has none, which syncs it at
    /// once, and ends the thread of each host no longer listed.
    pub fn refresh(&self) -> io::Result<()> {
        let listed: Vec<String> = hosts::list(&self.root)?
            .into_iter()
            .map(|host| host.destination)
            .collect();
        lock(&self.clients).retain(|destination, _| listed.contains(destination));
        let mut followed = lock(&self.followed);
        followed.retain(|destination, _| listed.contains(destination));
        for destination in listed {
            if let Entry::Vacant(entry) = followed.entry(destination) {
                let wake = self.start(entry.key());
                entry.insert(wake);
            }
        }
        Ok(())
    }

    /// Syncs `host` at once, if it is followed. Whether it is.
    pub fn sync_now(&self, host: &str) -> bool {
        lock(&self.followed)
            .get(host)
            .is_some_and(|wake| wake.send(()).is_ok())
    }

    /// Refreshes the hosts every few seconds until `stop` is set, then
    /// ends every host's thread.
    pub fn follow_until(&self, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            if let Err(error) = self.refresh() {
                eprintln!("epik-app: couldn't read the hosts: {error}");
            }
            let until = Instant::now() + REFRESH;
            while Instant::now() < until && !stop.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(200));
            }
        }
        self.shutdown();
    }

    /// Ends every host's thread, once any sync it is in has finished.
    pub fn shutdown(&self) {
        lock(&self.followed).clear();
    }

    /// Starts the thread that syncs `destination`, and returns what wakes it.
    fn start(&self, destination: &str) -> Sender<()> {
        let (wake, woken) = mpsc::channel();
        let root = self.root.clone();
        let destination = destination.to_owned();
        let client = self.client(&destination);
        let (building, idle) = (self.building, self.idle);
        thread::spawn(move || {
            let mirror = hosts::dir(&root, &destination);
            loop {
                if let Err(error) = sync(&root, &destination, &client, Utc::now()) {
                    eprintln!("epik-app: couldn't sync {destination}: {error}");
                }
                let wait = if is_building(&mirror) { building } else { idle };
                match woken.recv_timeout(wait) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                // Asked more than once during the wait: once will do.
                while woken.try_recv().is_ok() {}
            }
            // A sync that was under way when the host was removed may have
            // written its mirror again.
            let listed = hosts::list(&root)
                .is_ok_and(|hosts| hosts.iter().any(|h| h.destination == destination));
            if !listed {
                let _ = fs::remove_dir_all(&mirror);
            }
        });
        wake
    }
}

impl Drop for Mirrors {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Syncs `host`'s mirror under `root` through `client`, and records what
/// was heard from it at `now`: its version, or why it couldn't be reached.
/// A failed sync leaves the mirror as it was.
pub fn sync(root: &Path, host: &str, client: &impl Client, now: DateTime<Utc>) -> io::Result<()> {
    let synced = ping(client).and_then(|version| {
        mirror::sync(client, &hosts::dir(root, host))?;
        Ok(version)
    });
    heard(root, host, synced, now).map(drop)
}

/// Checks `host` through `client`, without syncing its mirror: records
/// what was heard from it at `now`, as [`sync`] does, and its setup items,
/// marked for this computer's roles, which the builds it is sent carry.
/// A host that can't be reached keeps the setup it last said.
pub fn check(root: &Path, host: &str, client: &impl Client, now: DateTime<Utc>) -> io::Result<()> {
    heard(root, host, ping(client), now)?;
    let roles = settings::read(root)?;
    match client.request(&Request::Setup { roles: Some(roles) })? {
        Response::Setup { items } => hosts::set_setup(root, host, items),
        Response::Error { message } => Err(io::Error::other(message)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the host answered setup with {other:?}"),
        )),
    }
}

/// Records `answer`, the version `host` answered with or why it didn't, as
/// heard at `now`, and returns it.
fn heard(
    root: &Path,
    host: &str,
    answer: io::Result<String>,
    now: DateTime<Utc>,
) -> io::Result<String> {
    let state = match &answer {
        Ok(version) => State::Reachable {
            version: version.clone(),
            at: now,
        },
        Err(error) => match remote::mismatched(error) {
            Some(version) => State::Mismatch {
                version: version.to_owned(),
                at: now,
            },
            None => State::Unreachable {
                error: error.to_string(),
                at: now,
            },
        },
    };
    hosts::set_state(root, host, state)?;
    answer
}

/// The version of the app `client` reaches.
fn ping(client: &impl Client) -> io::Result<String> {
    match client.request(&Request::Ping)? {
        Response::Pong { version } => Ok(version),
        Response::Error { message } => Err(io::Error::other(message)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the host answered ping with {other:?}"),
        )),
    }
}

/// Whether any run in the mirror at `mirror_root` is building. A mirror
/// that can't be read has none.
pub fn is_building(mirror_root: &Path) -> bool {
    run::list(mirror_root).is_ok_and(|runs| {
        runs.iter()
            .any(|run| run.disposition == Disposition::Building)
    })
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use chrono::TimeZone;
    use tempfile::TempDir;

    use super::*;
    use crate::ipc;
    use crate::record::{BuildWriter, Event, Kind, Outcome};
    use crate::remote::Mismatch;
    use crate::run::RunKey;
    use crate::settings::Settings;
    use crate::setup::{self, Check, Item};

    fn time(seconds: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap() + chrono::Duration::seconds(seconds)
    }

    fn key(number: u64) -> RunKey {
        RunKey::new("owner/repo", number)
    }

    /// Records a build of issue `number` under `root`, still running unless
    /// `ended`.
    fn build(root: &Path, number: u64, ended: bool) {
        let at = time(i64::try_from(number).unwrap());
        let mut writer = BuildWriter::create_new(root, &key(number), at).unwrap();
        writer
            .append(&Event::BuildStarted {
                at,
                build_id: writer.build_id().to_owned(),
                repo: "owner/repo".to_owned(),
                number,
                title: format!("Issue {number}"),
                kind: Kind::Issue,
                branch: "main".to_owned(),
                issues: Vec::new(),
                closed: Vec::new(),
                roles: Default::default(),
            })
            .unwrap();
        if ended {
            writer
                .append(&Event::BuildFinished {
                    at,
                    outcome: Outcome::Failed,
                    reason: Some("stopped".to_owned()),
                    pull_request: None,
                    review: None,
                    open_threads: None,
                })
                .unwrap();
        }
    }

    /// A host's app, answering from the records under its own root, or
    /// failing every request while `down`.
    struct Host {
        root: TempDir,
        down: AtomicBool,
        /// Whether its app is another minor version than this one's.
        mismatched: AtomicBool,
        /// How many times it has been asked for its runs.
        syncs: AtomicUsize,
        /// The roles it was asked to mark its setup for, each time.
        setups: Mutex<Vec<Option<Settings>>>,
    }

    impl Host {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                root: TempDir::new().unwrap(),
                down: AtomicBool::new(false),
                mismatched: AtomicBool::new(false),
                syncs: AtomicUsize::new(0),
                setups: Mutex::new(Vec::new()),
            })
        }

        fn syncs(&self) -> usize {
            self.syncs.load(Ordering::SeqCst)
        }
    }

    impl Client for Host {
        fn request(&self, request: &Request) -> io::Result<Response> {
            if self.down.load(Ordering::SeqCst) {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "couldn't reach devbox: Connection refused",
                ));
            }
            if self.mismatched.load(Ordering::SeqCst) {
                return Err(Mismatch {
                    destination: "devbox".to_owned(),
                    version: "0.3.2".to_owned(),
                }
                .into());
            }
            Ok(match request {
                Request::Ping => Response::Pong {
                    version: "0.4.1".to_owned(),
                },
                Request::Setup { roles } => {
                    self.setups.lock().unwrap().push(roles.clone());
                    Response::Setup {
                        items: vec![gh_signed_out()],
                    }
                }
                Request::Runs => {
                    self.syncs.fetch_add(1, Ordering::SeqCst);
                    ipc::records(self.root.path(), request.clone())
                }
                other => ipc::records(self.root.path(), other.clone()),
            })
        }
    }

    /// The GitHub sign-in, missing.
    fn gh_signed_out() -> Check {
        Check {
            item: Item::GhAuth,
            title: "GitHub sign-in".to_owned(),
            state: setup::State::Missing,
            detail: "not signed in".to_owned(),
            usable: false,
            needed: true,
        }
    }

    /// A local root listing `devbox` as a host.
    fn local() -> TempDir {
        let root = TempDir::new().unwrap();
        hosts::add(root.path(), "devbox", time(0)).unwrap();
        root
    }

    fn state(root: &Path) -> Option<State> {
        hosts::list(root).unwrap()[0].state.clone()
    }

    /// Mirrors of the hosts under `root`, each reached as `host`, synced
    /// every `building` while building and hourly otherwise.
    fn mirrors(root: &Path, host: &Arc<Host>, building: Duration) -> Mirrors {
        let host = host.clone();
        let mut mirrors = Mirrors::new(root, move |_| host.clone());
        mirrors.building = building;
        mirrors.idle = Duration::from_secs(3600);
        mirrors
    }

    /// Waits up to five seconds for `done`.
    fn eventually(done: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if done() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        done()
    }

    #[test]
    fn a_mirror_is_building_while_any_of_its_runs_is() {
        let mirror = TempDir::new().unwrap();
        assert!(!is_building(mirror.path()), "an empty mirror");

        build(mirror.path(), 3, true);
        assert!(!is_building(mirror.path()));

        build(mirror.path(), 4, false);
        assert!(is_building(mirror.path()));
    }

    #[test]
    fn a_sync_copies_the_runs_and_marks_the_host_reachable() {
        let root = local();
        let host = Host::new();
        build(host.root.path(), 3, false);

        sync(root.path(), "devbox", &host, time(5)).unwrap();

        let mirror = hosts::dir(root.path(), "devbox");
        assert_eq!(run::keys(&mirror).unwrap(), [key(3)]);
        assert_eq!(
            state(root.path()),
            Some(State::Reachable {
                version: "0.4.1".to_owned(),
                at: time(5),
            })
        );
    }

    #[test]
    fn a_failed_sync_marks_the_host_unreachable_and_keeps_its_mirror() {
        let root = local();
        let host = Host::new();
        build(host.root.path(), 3, false);
        sync(root.path(), "devbox", &host, time(5)).unwrap();

        host.down.store(true, Ordering::SeqCst);
        let error = sync(root.path(), "devbox", &host, time(9)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(
            state(root.path()),
            Some(State::Unreachable {
                error: "couldn't reach devbox: Connection refused".to_owned(),
                at: time(9),
            })
        );
        let mirror = hosts::dir(root.path(), "devbox");
        assert_eq!(run::keys(&mirror).unwrap(), [key(3)]);
        assert!(is_building(&mirror));
    }

    #[test]
    fn a_sync_of_a_host_of_another_version_records_its_version() {
        let root = local();
        let host = Host::new();
        host.mismatched.store(true, Ordering::SeqCst);

        let error = sync(root.path(), "devbox", &host, time(5)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(
            state(root.path()),
            Some(State::Mismatch {
                version: "0.3.2".to_owned(),
                at: time(5),
            })
        );
    }

    #[test]
    fn a_check_records_the_hosts_version_and_setup_without_mirroring_it() {
        let root = local();
        let host = Host::new();
        build(host.root.path(), 3, false);

        check(root.path(), "devbox", &host, time(5)).unwrap();

        let listed = &hosts::list(root.path()).unwrap()[0];
        assert_eq!(
            listed.state,
            Some(State::Reachable {
                version: "0.4.1".to_owned(),
                at: time(5),
            })
        );
        assert_eq!(listed.setup, Some(vec![gh_signed_out()]));
        assert_eq!(host.syncs(), 0);
    }

    #[test]
    fn a_hosts_setup_is_marked_for_this_computers_roles() {
        let root = local();
        let host = Host::new();
        let codex = Settings {
            coding: settings::Role {
                agent: "codex".to_owned(),
                model: None,
            },
            review: None,
        };
        settings::write(root.path(), &codex).unwrap();

        check(root.path(), "devbox", &host, time(5)).unwrap();

        assert_eq!(*host.setups.lock().unwrap(), [Some(codex)]);
    }

    #[test]
    fn a_check_of_an_unreachable_host_records_why_and_keeps_its_setup() {
        let root = local();
        let host = Host::new();
        check(root.path(), "devbox", &host, time(5)).unwrap();

        host.down.store(true, Ordering::SeqCst);
        let error = check(root.path(), "devbox", &host, time(9)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
        let listed = &hosts::list(root.path()).unwrap()[0];
        assert_eq!(
            listed.state,
            Some(State::Unreachable {
                error: "couldn't reach devbox: Connection refused".to_owned(),
                at: time(9),
            })
        );
        assert_eq!(listed.setup, Some(vec![gh_signed_out()]));
    }

    #[test]
    fn mirrors_check_a_host_through_its_client() {
        let root = local();
        let host = Host::new();
        let mirrors = mirrors(root.path(), &host, Duration::from_secs(3600));

        mirrors.check("devbox").unwrap();

        assert_eq!(
            state(root.path()).map(|s| matches!(s, State::Reachable { .. })),
            Some(true)
        );
    }

    #[test]
    fn a_host_with_a_building_run_is_synced_often() {
        let root = local();
        let host = Host::new();
        build(host.root.path(), 3, false);
        let mirrors = mirrors(root.path(), &host, Duration::from_millis(20));

        mirrors.refresh().unwrap();

        assert!(eventually(|| host.syncs() >= 3), "{}", host.syncs());
    }

    #[test]
    fn a_host_with_nothing_building_waits_until_asked() {
        let root = local();
        let host = Host::new();
        build(host.root.path(), 3, true);
        let mirrors = mirrors(root.path(), &host, Duration::from_millis(20));

        mirrors.refresh().unwrap();
        assert!(eventually(|| host.syncs() == 1));
        assert!(eventually(|| state(root.path()).is_some()));
        thread::sleep(Duration::from_millis(200));
        assert_eq!(host.syncs(), 1, "not synced again on its own");

        assert!(mirrors.sync_now("devbox"));
        assert!(eventually(|| host.syncs() == 2), "{}", host.syncs());
        assert!(!mirrors.sync_now("elsewhere"), "not a host");
    }

    #[test]
    fn the_interval_follows_the_hosts_building_runs() {
        let root = local();
        let host = Host::new();
        build(host.root.path(), 3, true);
        let mirrors = mirrors(root.path(), &host, Duration::from_millis(20));
        mirrors.refresh().unwrap();
        assert!(eventually(|| host.syncs() == 1));

        build(host.root.path(), 4, false);
        mirrors.sync_now("devbox");

        assert!(eventually(|| host.syncs() >= 5), "{}", host.syncs());
    }

    #[test]
    fn a_hosts_failure_is_recorded_by_its_thread() {
        let root = local();
        let host = Host::new();
        host.down.store(true, Ordering::SeqCst);
        let mirrors = mirrors(root.path(), &host, Duration::from_millis(20));

        mirrors.refresh().unwrap();

        assert!(eventually(|| matches!(
            state(root.path()),
            Some(State::Unreachable { .. })
        )));
    }

    #[test]
    fn a_removed_host_is_no_longer_synced_and_leaves_no_mirror() {
        let root = local();
        let host = Host::new();
        build(host.root.path(), 3, false);
        let mirrors = mirrors(root.path(), &host, Duration::from_millis(20));
        mirrors.refresh().unwrap();
        let mirror = hosts::dir(root.path(), "devbox");
        assert!(eventually(
            || run::keys(&mirror).is_ok_and(|keys| !keys.is_empty())
        ));

        hosts::remove(root.path(), "devbox").unwrap();
        mirrors.refresh().unwrap();

        assert!(!mirrors.sync_now("devbox"));
        assert!(eventually(|| !mirror.exists()));
        let after = host.syncs();
        thread::sleep(Duration::from_millis(200));
        assert!(host.syncs() <= after + 1, "{after} then {}", host.syncs());
    }

    #[test]
    fn a_request_to_a_host_shares_its_mirrors_client() {
        let root = local();
        let host = Host::new();
        let connected = Arc::new(AtomicUsize::new(0));
        let mut mirrors = Mirrors::new(root.path(), {
            let host = host.clone();
            let connected = connected.clone();
            move |_| {
                connected.fetch_add(1, Ordering::SeqCst);
                host.clone()
            }
        });
        mirrors.idle = Duration::from_secs(3600);
        mirrors.refresh().unwrap();
        assert!(eventually(|| host.syncs() == 1));

        let reply = mirrors.request("devbox", &Request::Ping).unwrap();

        assert_eq!(
            reply,
            Response::Pong {
                version: "0.4.1".to_owned()
            }
        );
        assert_eq!(
            connected.load(Ordering::SeqCst),
            1,
            "one client for the host"
        );
    }
}
