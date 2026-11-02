//! Every run the window shows: this computer's and each remote host's,
//! each named by the host it builds on.
//!
//! A remote host's runs are read from its mirror, under
//! [`hosts::dir`], by the code that reads this computer's. Only the runs of
//! the repositories monitored here are shown, wherever they build.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::hosts::{self, Host};
use crate::repositories;
use crate::run::{self, RunKey, RunState};

/// A run and the host it builds on, written `owner/name#number` for this
/// computer's and `<host>:owner/name#number` for a remote host's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostedKey {
    /// The remote host, or `None` for this computer.
    pub host: Option<String>,
    pub run: RunKey,
}

impl HostedKey {
    /// `run` on this computer.
    pub fn local(run: RunKey) -> Self {
        Self { host: None, run }
    }

    /// `run` on the remote host `host`.
    pub fn on(host: &str, run: RunKey) -> Self {
        Self {
            host: Some(host.to_owned()),
            run,
        }
    }

    /// The root its records are under: `root` for this computer's, the
    /// host's mirror for a remote host's.
    pub fn root(&self, root: &Path) -> PathBuf {
        match &self.host {
            Some(host) => hosts::dir(root, host),
            None => root.to_owned(),
        }
    }
}

impl fmt::Display for HostedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            Some(host) => write!(f, "{host}:{}", self.run),
            None => write!(f, "{}", self.run),
        }
    }
}

impl FromStr for HostedKey {
    type Err = String;

    /// Reads `owner/name#number` or `<host>:owner/name#number`. A run's
    /// key has no `:`, so the host is everything before the last one.
    fn from_str(text: &str) -> Result<Self, String> {
        match text.rsplit_once(':') {
            Some((host, run)) => {
                if !hosts::is_destination(host) {
                    return Err(format!("{host:?} is not a host"));
                }
                Ok(Self::on(host, run.parse()?))
            }
            None => Ok(Self::local(text.parse()?)),
        }
    }
}

/// A run, and the remote host it builds on.
#[derive(Debug, Clone, PartialEq)]
pub struct HostedRun {
    /// The remote host, as last heard from, or `None` for this computer.
    pub host: Option<Host>,
    pub state: RunState,
}

impl HostedRun {
    /// Its name: its host and `owner/name#number`.
    pub fn key(&self) -> HostedKey {
        HostedKey {
            host: self.host.as_ref().map(|host| host.destination.clone()),
            run: self.state.run.clone(),
        }
    }
}

/// The runs shown, and the remote hosts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shown {
    /// Every remote host, whether or not it has runs shown.
    pub hosts: Vec<Host>,
    /// Newest activity first.
    pub runs: Vec<HostedRun>,
}

impl Shown {
    /// The run named `key`, if it is shown.
    pub fn run(&self, key: &HostedKey) -> Option<&HostedRun> {
        self.runs.iter().find(|run| run.key() == *key)
    }
}

/// The runs of the repositories monitored under `root`, this computer's
/// and each remote host's, newest activity first. A host whose mirror
/// can't be read shows none.
pub fn monitored(root: &Path) -> io::Result<Shown> {
    let hosts = hosts::list(root)?;
    let mut runs: Vec<HostedRun> = repositories::monitored_runs(root)?
        .into_iter()
        .map(|state| HostedRun { host: None, state })
        .collect();
    let monitored = repositories::list(root)?;
    for host in &hosts {
        let Ok(mirrored) = run::list(&hosts::dir(root, &host.destination)) else {
            continue;
        };
        runs.extend(
            mirrored
                .into_iter()
                .filter(|state| {
                    monitored
                        .iter()
                        .any(|r| r.repo.eq_ignore_ascii_case(&state.repo))
                })
                .map(|state| HostedRun {
                    host: Some(host.clone()),
                    state,
                }),
        );
    }
    runs.sort_by(|a, b| {
        b.state
            .latest_at
            .cmp(&a.state.latest_at)
            .then_with(|| a.key().cmp(&b.key()))
    });
    Ok(Shown { hosts, runs })
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::*;
    use crate::hosts::State;
    use crate::record::{BuildWriter, Event, Kind};

    fn time(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, 12, minute, 0).unwrap()
    }

    /// Records a build of `repo#number` under `root` that started at
    /// `minute`.
    fn write_run(root: &Path, repo: &str, number: u64, minute: u32) {
        let key = RunKey::new(repo, number);
        let mut writer = BuildWriter::create(root, &key, "b").unwrap();
        writer
            .append(&Event::BuildStarted {
                at: time(minute),
                build_id: "b".to_owned(),
                repo: repo.to_owned(),
                number,
                title: "A run".to_owned(),
                kind: Kind::Issue,
                branch: "main".to_owned(),
                issues: vec![],
                closed: vec![],
                roles: Default::default(),
            })
            .unwrap();
    }

    fn keys(shown: &Shown) -> Vec<String> {
        shown.runs.iter().map(|run| run.key().to_string()).collect()
    }

    #[test]
    fn a_key_names_its_host_before_the_run() {
        let local = HostedKey::local(RunKey::new("acme/widgets", 12));
        let remote = HostedKey::on("me@build.example.com", RunKey::new("acme/widgets", 12));

        assert_eq!(local.to_string(), "acme/widgets#12");
        assert_eq!(remote.to_string(), "me@build.example.com:acme/widgets#12");
        assert_eq!("acme/widgets#12".parse(), Ok(local));
        assert_eq!("me@build.example.com:acme/widgets#12".parse(), Ok(remote));
    }

    #[test]
    fn a_host_with_a_colon_is_read_to_its_last() {
        let key: HostedKey = "ssh://me@devbox:2222:acme/widgets#12".parse().unwrap();

        assert_eq!(key.host.as_deref(), Some("ssh://me@devbox:2222"));
        assert_eq!(key.run, RunKey::new("acme/widgets", 12));
    }

    #[test]
    fn a_key_with_no_host_or_no_run_is_refused() {
        for bad in [
            ":acme/widgets#12",
            "devbox:",
            "devbox:acme",
            "-x:acme/widgets#1",
        ] {
            assert!(bad.parse::<HostedKey>().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_keys_records_are_on_its_host() {
        let root = Path::new("/home/me/.epik");
        let run = RunKey::new("acme/widgets", 12);

        assert_eq!(HostedKey::local(run.clone()).root(root), root);
        assert_eq!(
            HostedKey::on("devbox", run).root(root),
            hosts::dir(root, "devbox")
        );
    }

    #[test]
    fn every_hosts_runs_are_shown_newest_first_each_with_its_host() {
        let root = tempfile::tempdir().unwrap();
        repositories::add(root.path(), "acme/widgets", true, time(0)).unwrap();
        hosts::add(root.path(), "devbox", time(0)).unwrap();
        hosts::add(root.path(), "idle", time(0)).unwrap();
        write_run(root.path(), "acme/widgets", 1, 1);
        write_run(&hosts::dir(root.path(), "devbox"), "acme/widgets", 2, 3);
        write_run(&hosts::dir(root.path(), "devbox"), "acme/widgets", 1, 2);

        let shown = monitored(root.path()).unwrap();

        assert_eq!(
            keys(&shown),
            [
                "devbox:acme/widgets#2",
                "devbox:acme/widgets#1",
                "acme/widgets#1"
            ]
        );
        assert_eq!(shown.hosts.len(), 2, "a host with no runs is still a host");
        assert_eq!(shown.runs[0].host, Some(shown.hosts[0].clone()));
        assert_eq!(shown.runs[2].host, None);
    }

    #[test]
    fn a_hosts_runs_are_shown_only_for_the_repositories_monitored_here() {
        let root = tempfile::tempdir().unwrap();
        repositories::add(root.path(), "acme/widgets", true, time(0)).unwrap();
        hosts::add(root.path(), "devbox", time(0)).unwrap();
        write_run(&hosts::dir(root.path(), "devbox"), "acme/widgets", 1, 1);
        write_run(&hosts::dir(root.path(), "devbox"), "acme/gadgets", 2, 2);

        assert_eq!(
            keys(&monitored(root.path()).unwrap()),
            ["devbox:acme/widgets#1"]
        );
    }

    #[test]
    fn a_hosts_run_carries_what_was_last_heard_from_it() {
        let root = tempfile::tempdir().unwrap();
        repositories::add(root.path(), "acme/widgets", true, time(0)).unwrap();
        hosts::add(root.path(), "devbox", time(0)).unwrap();
        let unreachable = State::Unreachable {
            error: "Connection refused".to_owned(),
            at: time(5),
        };
        hosts::set_state(root.path(), "devbox", unreachable.clone()).unwrap();
        write_run(&hosts::dir(root.path(), "devbox"), "acme/widgets", 1, 1);

        let shown = monitored(root.path()).unwrap();

        let host = shown.runs[0].host.as_ref().unwrap();
        assert_eq!(host.state, Some(unreachable));
        let key = HostedKey::on("devbox", RunKey::new("acme/widgets", 1));
        assert_eq!(shown.run(&key), Some(&shown.runs[0]));
        assert_eq!(shown.run(&HostedKey::local(key.run.clone())), None);
    }
}
