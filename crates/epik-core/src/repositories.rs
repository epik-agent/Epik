//! The repositories Epik monitors.
//!
//! Reconciling runs with GitHub works over this list, and the window shows
//! only the runs of monitored repositories. A repository is added when Epik
//! starts a build in it or checks an epic's graph there, and the user can
//! add and remove repositories. The list is `<root>/repositories.json`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::run::{self, RunKey, RunState};

/// The file the list is kept in.
pub const FILE: &str = "repositories.json";

/// A monitored repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    /// `owner/name`.
    pub repo: String,
    pub added_at: DateTime<Utc>,
    /// Whether the user added it, rather than Epik.
    pub added_by_user: bool,
    /// Whether GitHub last answered "not found" for it: deleted, renamed,
    /// or no longer reachable by the user. GitHub doesn't say which.
    #[serde(default)]
    pub missing: bool,
}

fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// The monitored repositories, in the order they were added.
pub fn list(root: &Path) -> io::Result<Vec<Repository>> {
    match fs::read_to_string(path(root)) {
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// Changes the list with `edit`, saving it when `edit` says it changed.
/// The change holds an exclusive lock on `<root>/repositories.lock`, so
/// changes made at once, by this process's threads or by `epik-app mcp`,
/// each see the others'. The list is written whole, through a temporary
/// file, so a reader never sees half of it.
fn change<T>(root: &Path, edit: impl FnOnce(&mut Vec<Repository>) -> (bool, T)) -> io::Result<T> {
    fs::create_dir_all(root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("repositories.lock"))?;
    lock.lock()?;
    let mut repositories = list(root)?;
    let (changed, result) = edit(&mut repositories);
    if changed {
        let mut json = serde_json::to_vec_pretty(&repositories)?;
        json.push(b'\n');
        let temporary = root.join(format!("{FILE}.{}", std::process::id()));
        fs::write(&temporary, json)?;
        fs::rename(temporary, path(root))?;
    }
    Ok(result)
}

/// Whether `repo` can be a repository's name, `owner/name`.
pub fn is_repo(repo: &str) -> bool {
    format!("{repo}#1").parse::<RunKey>().is_ok()
}

/// Monitors `repo`, added by the user or not, unless it already is.
/// Whether it was added.
pub fn add(root: &Path, repo: &str, by_user: bool, now: DateTime<Utc>) -> io::Result<bool> {
    if !is_repo(repo) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{repo:?} is not a repository: write owner/name"),
        ));
    }
    change(root, |repositories| {
        if repositories
            .iter()
            .any(|r| r.repo.eq_ignore_ascii_case(repo))
        {
            return (false, false);
        }
        repositories.push(Repository {
            repo: repo.to_owned(),
            added_at: now,
            added_by_user: by_user,
            missing: false,
        });
        (true, true)
    })
}

/// Stops monitoring `repo`. Its runs leave the window; their builds go on.
pub fn remove(root: &Path, repo: &str) -> io::Result<()> {
    change(root, |repositories| {
        repositories.retain(|r| !r.repo.eq_ignore_ascii_case(repo));
        (true, ())
    })
}

/// Records whether GitHub said `repo` was not found.
pub fn set_missing(root: &Path, repo: &str, missing: bool) -> io::Result<()> {
    change(root, |repositories| {
        let mut changed = false;
        for r in repositories.iter_mut() {
            if r.repo.eq_ignore_ascii_case(repo) && r.missing != missing {
                r.missing = missing;
                changed = true;
            }
        }
        (changed, ())
    })
}

/// The runs of `repo`: what would leave the window if it were no longer
/// monitored.
pub fn runs_of(root: &Path, repo: &str) -> io::Result<Vec<RunState>> {
    let mut runs = run::list(root)?;
    runs.retain(|run| run.repo.eq_ignore_ascii_case(repo));
    Ok(runs)
}

/// The runs of the monitored repositories, as [`run::list`] orders them.
pub fn monitored_runs(root: &Path) -> io::Result<Vec<RunState>> {
    let repositories = list(root)?;
    let mut runs = run::list(root)?;
    runs.retain(|run| {
        repositories
            .iter()
            .any(|r| r.repo.eq_ignore_ascii_case(&run.repo))
    });
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{BuildWriter, Event, Kind};
    use chrono::TimeZone;

    fn time(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0).unwrap()
    }

    fn write_run(root: &Path, repo: &str, number: u64) {
        let key = RunKey::new(repo, number);
        let mut writer = BuildWriter::create(root, &key, "b").unwrap();
        writer
            .append(&Event::BuildStarted {
                at: time(1),
                build_id: "b".to_owned(),
                repo: repo.to_owned(),
                number,
                title: "A run".to_owned(),
                kind: Kind::Issue,
                branch: "main".to_owned(),
                issues: vec![],
                closed: vec![],
            })
            .unwrap();
    }

    #[test]
    fn repositories_are_added_once_in_order() {
        let root = tempfile::tempdir().unwrap();

        assert!(add(root.path(), "wpm/epik-test", false, time(1)).unwrap());
        assert!(add(root.path(), "epik-agent/Epik", true, time(2)).unwrap());
        assert!(!add(root.path(), "WPM/epik-test", true, time(3)).unwrap());

        assert_eq!(
            list(root.path()).unwrap(),
            [
                Repository {
                    repo: "wpm/epik-test".to_owned(),
                    added_at: time(1),
                    added_by_user: false,
                    missing: false,
                },
                Repository {
                    repo: "epik-agent/Epik".to_owned(),
                    added_at: time(2),
                    added_by_user: true,
                    missing: false,
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
                scope.spawn(move || add(root, &format!("o/r{n}"), false, time(1)).unwrap());
            }
        });

        assert_eq!(list(root.path()).unwrap().len(), 16);
    }

    #[test]
    fn a_name_that_isnt_owner_slash_name_is_refused() {
        let root = tempfile::tempdir().unwrap();
        for bad in ["epik", "a/b/c", "../x", "a/", ""] {
            let error = add(root.path(), bad, true, time(1)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{bad}");
        }
        assert!(list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn a_repository_is_removed_and_marked_missing() {
        let root = tempfile::tempdir().unwrap();
        add(root.path(), "a/one", false, time(1)).unwrap();
        add(root.path(), "a/two", false, time(1)).unwrap();

        set_missing(root.path(), "a/two", true).unwrap();
        assert!(list(root.path()).unwrap()[1].missing);
        set_missing(root.path(), "a/two", false).unwrap();
        assert!(!list(root.path()).unwrap()[1].missing);
        remove(root.path(), "a/one").unwrap();

        let left: Vec<String> = list(root.path())
            .unwrap()
            .into_iter()
            .map(|r| r.repo)
            .collect();
        assert_eq!(left, ["a/two"]);
    }

    #[test]
    fn only_the_runs_of_monitored_repositories_are_shown() {
        let root = tempfile::tempdir().unwrap();
        write_run(root.path(), "a/one", 1);
        write_run(root.path(), "a/one", 2);
        write_run(root.path(), "a/two", 3);
        add(root.path(), "a/one", false, time(1)).unwrap();

        let shown: Vec<u64> = monitored_runs(root.path())
            .unwrap()
            .iter()
            .map(|run| run.number)
            .collect();
        let going: Vec<u64> = runs_of(root.path(), "a/one")
            .unwrap()
            .iter()
            .map(|run| run.number)
            .collect();

        assert_eq!(shown, [1, 2]);
        assert_eq!(going, [1, 2]);
    }
}
