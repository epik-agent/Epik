//! The remote hosts builds can run on.
//!
//! A host is named by its SSH destination as the user typed it (`devbox`,
//! `me@build.example.com`), so `~/.ssh/config` decides how it is reached.
//! This computer is always a host and isn't in the list. The list, which
//! host is the default for builds, and each host's last known state are
//! `<root>/hosts.json`. Each host's mirror of its runs is under
//! `<root>/hosts/`, in [`dir`].

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::setup::Check;

/// The file the list is kept in.
pub const FILE: &str = "hosts.json";

/// The directory the hosts' mirrors are kept in.
pub const DIR: &str = "hosts";

/// A remote host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// The SSH destination, as the user typed it.
    pub destination: String,
    pub added_at: DateTime<Utc>,
    /// What was last heard from it, if anything has been.
    #[serde(default)]
    pub state: Option<State>,
    /// When it last answered as an app of this one's minor version, if it
    /// ever has: when its mirror was last current.
    #[serde(default)]
    pub heard_at: Option<DateTime<Utc>>,
    /// Its setup items, as it last said, if it has been checked.
    #[serde(default)]
    pub setup: Option<Vec<Check>>,
}

/// What was last heard from a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum State {
    /// It answered, and its app is `version`.
    Reachable { version: String, at: DateTime<Utc> },
    /// It couldn't be reached, for `error`.
    Unreachable { error: String, at: DateTime<Utc> },
    /// It answered, but its app is `version`, another minor version than
    /// this one's, so it is refused.
    Mismatch { version: String, at: DateTime<Utc> },
}

/// What `hosts.json` holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Hosts {
    /// The host builds go to; none means this computer.
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    hosts: Vec<Host>,
}

fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

fn read(root: &Path) -> io::Result<Hosts> {
    match fs::read_to_string(path(root)) {
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Hosts::default()),
        Err(error) => Err(error),
    }
}

/// The remote hosts, in the order they were added.
pub fn list(root: &Path) -> io::Result<Vec<Host>> {
    Ok(read(root)?.hosts)
}

/// The host builds go to, or none for this computer.
pub fn default(root: &Path) -> io::Result<Option<String>> {
    Ok(read(root)?.default)
}

/// Changes the hosts with `edit`, saving them when `edit` says they
/// changed. As in [`crate::repositories`], the change holds an exclusive
/// lock on `<root>/hosts.lock`, and the file is written whole through a
/// temporary file.
fn change<T>(root: &Path, edit: impl FnOnce(&mut Hosts) -> io::Result<(bool, T)>) -> io::Result<T> {
    fs::create_dir_all(root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("hosts.lock"))?;
    lock.lock()?;
    let mut hosts = read(root)?;
    let (changed, result) = edit(&mut hosts)?;
    if changed {
        let mut json = serde_json::to_vec_pretty(&hosts)?;
        json.push(b'\n');
        let temporary = root.join(format!("{FILE}.{}", std::process::id()));
        fs::write(&temporary, json)?;
        fs::rename(temporary, path(root))?;
    }
    Ok(result)
}

fn not_a_host(destination: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{destination:?} is not a host"),
    )
}

/// Whether `destination` can be an SSH destination Epik passes to `ssh`:
/// not empty, no whitespace, and not starting with `-`, which `ssh` would
/// take for an option. Nor `.` or `..`, whose mirror would be a directory
/// that is already there.
pub fn is_destination(destination: &str) -> bool {
    !destination.is_empty()
        && !destination.starts_with('-')
        && !destination.contains(char::is_whitespace)
        && destination != "."
        && destination != ".."
}

/// The name of `host`'s mirror: the destination with anything but ASCII
/// letters, digits, `.`, `-` and `_` made `_`, and `.` and `..` made all
/// `_` so the mirror is always under `<root>/hosts`.
fn dir_name(host: &str) -> String {
    let name: String = host
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .collect();
    if name == "." || name == ".." {
        name.replace('.', "_")
    } else {
        name
    }
}

/// The root of `host`'s mirror: `<root>/hosts/<host>`, with anything in
/// the destination but ASCII letters, digits, `.`, `-` and `_` made `_`.
pub fn dir(root: &Path, host: &str) -> PathBuf {
    root.join(DIR).join(dir_name(host))
}

/// Adds `destination`, unless it already is a host. Whether it was added.
/// A destination whose mirror would be another host's, the same but for
/// case or for the characters made `_`, is refused.
pub fn add(root: &Path, destination: &str, now: DateTime<Utc>) -> io::Result<bool> {
    if !is_destination(destination) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{destination:?} is not an SSH destination: write it as you would to ssh, \
                 such as devbox or me@build.example.com"
            ),
        ));
    }
    change(root, |hosts| {
        if hosts.hosts.iter().any(|h| h.destination == destination) {
            return Ok((false, false));
        }
        let name = dir_name(destination);
        if let Some(other) = hosts
            .hosts
            .iter()
            .find(|h| dir_name(&h.destination).eq_ignore_ascii_case(&name))
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{destination:?} is too like the host {:?}: remove that one first",
                    other.destination
                ),
            ));
        }
        hosts.hosts.push(Host {
            destination: destination.to_owned(),
            added_at: now,
            state: None,
            heard_at: None,
            setup: None,
        });
        Ok((true, true))
    })
}

/// Removes `destination`, and its mirror. When it was the default, builds
/// go to this computer again.
pub fn remove(root: &Path, destination: &str) -> io::Result<()> {
    change(root, |hosts| {
        let before = hosts.hosts.len();
        hosts.hosts.retain(|h| h.destination != destination);
        if hosts.hosts.len() == before {
            return Ok((false, ()));
        }
        if hosts.default.as_deref() == Some(destination) {
            hosts.default = None;
        }
        match fs::remove_dir_all(dir(root, destination)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        Ok((true, ()))
    })
}

/// Records what was last heard from `destination`, and when it last
/// answered if it did.
pub fn set_state(root: &Path, destination: &str, state: State) -> io::Result<()> {
    change(root, |hosts| {
        let host = hosts
            .hosts
            .iter_mut()
            .find(|h| h.destination == destination)
            .ok_or_else(|| not_a_host(destination))?;
        let changed = host.state.as_ref() != Some(&state);
        if let State::Reachable { at, .. } = &state {
            host.heard_at = Some(*at);
        }
        host.state = Some(state);
        Ok((changed, ()))
    })
}

/// Records `destination`'s setup items, as it said them.
pub fn set_setup(root: &Path, destination: &str, items: Vec<Check>) -> io::Result<()> {
    change(root, |hosts| {
        let host = hosts
            .hosts
            .iter_mut()
            .find(|h| h.destination == destination)
            .ok_or_else(|| not_a_host(destination))?;
        let changed = host.setup.as_ref() != Some(&items);
        host.setup = Some(items);
        Ok((changed, ()))
    })
}

/// Makes `destination` the host builds go to, or this computer for none.
pub fn set_default(root: &Path, destination: Option<&str>) -> io::Result<()> {
    change(root, |hosts| {
        if let Some(destination) = destination
            && !hosts.hosts.iter().any(|h| h.destination == destination)
        {
            return Err(not_a_host(destination));
        }
        let changed = hosts.default.as_deref() != destination;
        hosts.default = destination.map(str::to_owned);
        Ok((changed, ()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::{self, Item};
    use chrono::TimeZone;

    fn time(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0).unwrap()
    }

    fn destinations(root: &Path) -> Vec<String> {
        list(root)
            .unwrap()
            .into_iter()
            .map(|host| host.destination)
            .collect()
    }

    #[test]
    fn there_are_no_hosts_at_first() {
        let root = tempfile::tempdir().unwrap();

        assert!(list(root.path()).unwrap().is_empty());
        assert_eq!(default(root.path()).unwrap(), None);
    }

    #[test]
    fn hosts_are_added_once_in_order() {
        let root = tempfile::tempdir().unwrap();

        assert!(add(root.path(), "devbox", time(1)).unwrap());
        assert!(add(root.path(), "me@build.example.com", time(2)).unwrap());
        assert!(!add(root.path(), "devbox", time(3)).unwrap());

        assert_eq!(
            list(root.path()).unwrap(),
            [
                Host {
                    destination: "devbox".to_owned(),
                    added_at: time(1),
                    state: None,
                    heard_at: None,
                    setup: None,
                },
                Host {
                    destination: "me@build.example.com".to_owned(),
                    added_at: time(2),
                    state: None,
                    heard_at: None,
                    setup: None,
                },
            ]
        );
    }

    #[test]
    fn changes_made_at_once_are_all_kept() {
        let root = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for n in 0..16 {
                let root = root.path();
                scope.spawn(move || add(root, &format!("host{n}"), time(1)).unwrap());
            }
        });

        assert_eq!(list(root.path()).unwrap().len(), 16);
    }

    #[test]
    fn a_destination_ssh_would_misread_is_refused() {
        let root = tempfile::tempdir().unwrap();
        for bad in [
            "",
            "-oProxyCommand=x",
            "dev box",
            " devbox",
            "devbox\n",
            "a\tb",
            ".",
            "..",
        ] {
            let error = add(root.path(), bad, time(1)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
        }
        assert!(list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn a_host_whose_mirror_would_be_another_hosts_is_refused() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "me@devbox", time(1)).unwrap();

        for clash in ["me_devbox", "ME@devbox"] {
            let error = add(root.path(), clash, time(2)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "{clash}");
        }
        assert_eq!(destinations(root.path()), ["me@devbox"]);
    }

    #[test]
    fn a_host_is_removed_with_its_mirror() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "one", time(1)).unwrap();
        add(root.path(), "two", time(1)).unwrap();
        fs::create_dir_all(dir(root.path(), "one").join("runs")).unwrap();
        fs::create_dir_all(dir(root.path(), "two").join("runs")).unwrap();

        remove(root.path(), "one").unwrap();

        assert_eq!(destinations(root.path()), ["two"]);
        assert!(!dir(root.path(), "one").exists());
        assert!(dir(root.path(), "two").exists());
    }

    #[test]
    fn removing_a_host_without_a_mirror_is_fine() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "one", time(1)).unwrap();

        remove(root.path(), "one").unwrap();
        remove(root.path(), "never-added").unwrap();

        assert!(list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn removing_what_isnt_a_host_leaves_a_mirror_of_the_same_name() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "me@devbox", time(1)).unwrap();
        fs::create_dir_all(dir(root.path(), "me@devbox")).unwrap();

        remove(root.path(), "me_devbox").unwrap();

        assert_eq!(destinations(root.path()), ["me@devbox"]);
        assert!(dir(root.path(), "me@devbox").exists());
    }

    #[test]
    fn the_last_state_heard_is_kept() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "devbox", time(1)).unwrap();
        let reachable = State::Reachable {
            version: "0.4.0".to_owned(),
            at: time(2),
        };
        let unreachable = State::Unreachable {
            error: "ssh: Could not resolve hostname devbox".to_owned(),
            at: time(3),
        };

        set_state(root.path(), "devbox", reachable.clone()).unwrap();
        assert_eq!(list(root.path()).unwrap()[0].state, Some(reachable));
        set_state(root.path(), "devbox", unreachable.clone()).unwrap();
        assert_eq!(list(root.path()).unwrap()[0].state, Some(unreachable));
    }

    #[test]
    fn a_host_was_last_heard_from_when_it_last_answered() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "devbox", time(1)).unwrap();
        assert_eq!(list(root.path()).unwrap()[0].heard_at, None);

        let reachable = State::Reachable {
            version: "0.4.0".to_owned(),
            at: time(2),
        };
        set_state(root.path(), "devbox", reachable).unwrap();
        let unreachable = State::Unreachable {
            error: "ssh: connect to host devbox port 22: Connection refused".to_owned(),
            at: time(3),
        };
        set_state(root.path(), "devbox", unreachable).unwrap();

        assert_eq!(list(root.path()).unwrap()[0].heard_at, Some(time(2)));
    }

    #[test]
    fn a_host_of_another_version_was_last_heard_from_when_it_was_the_same() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "devbox", time(1)).unwrap();
        let reachable = State::Reachable {
            version: "0.4.0".to_owned(),
            at: time(2),
        };
        let mismatch = State::Mismatch {
            version: "0.3.2".to_owned(),
            at: time(3),
        };

        set_state(root.path(), "devbox", reachable).unwrap();
        set_state(root.path(), "devbox", mismatch.clone()).unwrap();

        let host = &list(root.path()).unwrap()[0];
        assert_eq!(host.state, Some(mismatch));
        assert_eq!(host.heard_at, Some(time(2)));
    }

    #[test]
    fn a_hosts_setup_is_kept_until_checked_again() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "devbox", time(1)).unwrap();
        assert_eq!(list(root.path()).unwrap()[0].setup, None);
        let items = vec![Check {
            item: Item::GhAuth,
            title: "GitHub sign-in".to_owned(),
            state: setup::State::Missing,
            detail: "not signed in".to_owned(),
            usable: false,
        }];

        set_setup(root.path(), "devbox", items.clone()).unwrap();
        let reachable = State::Reachable {
            version: "0.4.0".to_owned(),
            at: time(2),
        };
        set_state(root.path(), "devbox", reachable).unwrap();

        assert_eq!(list(root.path()).unwrap()[0].setup, Some(items));
        let error = set_setup(root.path(), "other", vec![]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn the_state_of_an_unknown_host_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let state = State::Unreachable {
            error: "no".to_owned(),
            at: time(1),
        };

        let error = set_state(root.path(), "devbox", state).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn the_default_survives_a_reload() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "devbox", time(1)).unwrap();

        set_default(root.path(), Some("devbox")).unwrap();
        let text = fs::read_to_string(path(root.path())).unwrap();
        fs::remove_file(path(root.path())).unwrap();
        fs::write(path(root.path()), text).unwrap();

        assert_eq!(default(root.path()).unwrap().as_deref(), Some("devbox"));
        set_default(root.path(), None).unwrap();
        assert_eq!(default(root.path()).unwrap(), None);
    }

    #[test]
    fn only_a_host_can_be_the_default() {
        let root = tempfile::tempdir().unwrap();

        let error = set_default(root.path(), Some("devbox")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(default(root.path()).unwrap(), None);
    }

    #[test]
    fn the_default_clears_when_its_host_goes() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "one", time(1)).unwrap();
        add(root.path(), "two", time(1)).unwrap();
        set_default(root.path(), Some("two")).unwrap();

        remove(root.path(), "one").unwrap();
        assert_eq!(default(root.path()).unwrap().as_deref(), Some("two"));
        remove(root.path(), "two").unwrap();
        assert_eq!(default(root.path()).unwrap(), None);
    }

    #[test]
    fn a_mirror_is_named_for_its_destination() {
        let root = Path::new("/home/me/.epik");
        for (host, name) in [
            ("devbox", "devbox"),
            ("me@build.example.com", "me_build.example.com"),
            ("build-1_x", "build-1_x"),
            ("me@[::1]:22", "me____1__22"),
            ("ssh://me@host/x", "ssh___me_host_x"),
            ("..", "__"),
            ("dévbox", "d_vbox"),
        ] {
            assert_eq!(dir(root, host), root.join("hosts").join(name), "{host}");
        }
    }
}
