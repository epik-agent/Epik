//! The run logs: the third root under Epik's home, and the only one that
//! is not a cache.
//!
//! Every conducted run writes its full event stream — `FeatureEvent`s for
//! a feature run, `RunEvent`s for an issue run — as JSON lines to one file
//! per run, `<logs>/<owner>/<name>/<kind>-<number>-<start>.jsonl`, where
//! `<start>` is the run's wall-clock start in RFC 3339 spelled with `-`
//! for `:`, which some filesystems refuse. The contract is the opposite
//! of [`crate::git`]'s roots: `repos/` and `work/` are caches of GitHub,
//! deletable at the cost of time alone, while the log is the only copy of
//! the run's narration — no Epik code deletes it, and the file is opened
//! to append, so nothing ever truncates what an earlier run wrote.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config;
use crate::github::Repo;
use crate::logging::JsonLines;

/// Where the run logs live under Epik's home.
const LOGS: &str = "logs";

/// Which run a log narrates: the file name's first word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Feature,
    Issue,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Feature => "feature",
            Self::Issue => "issue",
        })
    }
}

/// The log root, held: one value says where the narration lands, and tests
/// root it in a temp directory instead of the real `~/.epik`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Logs {
    root: PathBuf,
}

impl Logs {
    /// The real root: `~/.epik/logs`.
    ///
    /// # Errors
    ///
    /// Returns an error when Epik's home is not discoverable.
    pub fn new() -> Result<Self> {
        Ok(Self::rooted(config::home()?.join(LOGS)))
    }

    /// A root where the caller says — how tests stay out of `~/.epik`.
    #[must_use]
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where a `kind` run of `number` that started at `start` writes its
    /// log, whether or not it exists yet.
    #[must_use]
    pub fn path(&self, repo: &Repo, kind: Kind, number: u64, start: SystemTime) -> PathBuf {
        self.root
            .join(&repo.owner)
            .join(&repo.name)
            .join(format!("{kind}-{number}-{}.jsonl", stamp(start)))
    }

    /// Opens the log for a run starting now: parents made, the file opened
    /// to append. The log is the only copy of the run's narration, so even
    /// a rerun landing inside the same second may add to a file but never
    /// take from one.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or the file cannot be made.
    pub fn create(&self, repo: &Repo, kind: Kind, number: u64) -> Result<JsonLines<File>> {
        let path = self.path(repo, kind, number, SystemTime::now());
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        Ok(JsonLines::new(file))
    }
}

/// `start` as UTC RFC 3339 to the second, with `-` for `:` — colons are
/// illegal in file names on some filesystems, and nothing else about the
/// form needs to bend.
fn stamp(start: SystemTime) -> String {
    // A clock before 1970 is a broken clock; the epoch is as honest a name
    // as any for a start it cannot state.
    let seconds = start
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (hours, minutes, secs) = (seconds / 3600 % 24, seconds / 60 % 60, seconds % 60);
    // Civil-from-days (Howard Hinnant's algorithm) in the 0000-03-01 era;
    // unsigned arithmetic suffices for any post-epoch clock.
    let days = seconds / 86_400 + 719_468;
    let era = days / 146_097;
    let doe = days % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}-{minutes:02}-{secs:02}Z")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::logging::Log;
    use crate::run::{Phase, RunEvent, Verdict};

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn the_path_is_kind_number_and_a_filesystem_safe_start() {
        let logs = Logs::rooted("/epik/logs");
        let repo = Repo::new("epik-agent", "Epik");
        assert_eq!(
            logs.path(&repo, Kind::Feature, 141, at(1_000_000_000)),
            PathBuf::from("/epik/logs/epik-agent/Epik/feature-141-2001-09-09T01-46-40Z.jsonl")
        );
        assert_eq!(
            logs.path(&repo, Kind::Issue, 7, at(0)),
            PathBuf::from("/epik/logs/epik-agent/Epik/issue-7-1970-01-01T00-00-00Z.jsonl")
        );
    }

    #[test]
    fn the_stamp_reads_the_civil_calendar() {
        assert_eq!(stamp(at(1_709_164_800)), "2024-02-29T00-00-00Z");
        assert_eq!(stamp(at(1_786_192_205)), "2026-08-08T12-30-05Z");
        assert_eq!(
            stamp(UNIX_EPOCH - Duration::from_secs(1)),
            "1970-01-01T00-00-00Z",
            "a pre-epoch clock collapses to the epoch rather than lying"
        );
    }

    #[test]
    fn a_created_log_parses_back_into_the_vocabulary_it_carried() {
        let root = TempDir::new().unwrap();
        let logs = Logs::rooted(root.path().join("logs"));
        let repo = Repo::new("epik-agent", "Epik");

        let mut sink = logs.create(&repo, Kind::Issue, 7).unwrap();
        sink.emit(RunEvent::Entered(Phase::Worktree));
        sink.emit(RunEvent::Finished(Verdict::Done));
        drop(sink);

        let events = collected(root.path().join("logs/epik-agent/Epik"));
        assert_eq!(
            events,
            [
                RunEvent::Entered(Phase::Worktree),
                RunEvent::Finished(Verdict::Done)
            ]
        );
    }

    #[test]
    fn a_rerun_takes_nothing_from_an_earlier_runs_log() {
        let root = TempDir::new().unwrap();
        let logs = Logs::rooted(root.path().join("logs"));
        let repo = Repo::new("epik-agent", "Epik");

        let mut first = logs.create(&repo, Kind::Issue, 7).unwrap();
        first.emit(RunEvent::Entered(Phase::Worktree));
        drop(first);
        let mut second = logs.create(&repo, Kind::Issue, 7).unwrap();
        second.emit(RunEvent::Finished(Verdict::Done));
        drop(second);

        // Whether the rerun landed inside the first run's second — one
        // shared file — or the next, every line survives.
        let events = collected(root.path().join("logs/epik-agent/Epik"));
        assert!(events.contains(&RunEvent::Entered(Phase::Worktree)));
        assert!(events.contains(&RunEvent::Finished(Verdict::Done)));
    }

    /// Every line of every log beneath `dir`, parsed, in file order.
    fn collected(dir: PathBuf) -> Vec<RunEvent> {
        let mut files: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        files.sort();
        files
            .iter()
            .flat_map(|path| {
                fs::read_to_string(path)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect::<Vec<RunEvent>>()
            })
            .collect()
    }
}
