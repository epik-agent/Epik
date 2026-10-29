//! A build's record: the one place that says how a build went.
//!
//! Each build has a directory inside its run's,
//! `<root>/runs/<owner>/<name>/<number>/<build-id>/`, where the root is
//! normally `~/.epik`. The app appends Epik's own events to `events.jsonl`
//! as they happen, and writes `summary.json` when the build ends. A build's
//! state is the fold of its events, [`BuildState`]; a run's is the fold of
//! its builds' states, in [`crate::run`].

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::activity::Activity;
use crate::run::{self, RunKey};

/// The file a build's events are appended to.
pub const EVENTS_FILE: &str = "events.jsonl";
/// The file a build's folded state is written to when it ends.
pub const SUMMARY_FILE: &str = "summary.json";

/// Epik's root directory, `~/.epik`, or `None` if there is no home directory.
pub fn default_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".epik"))
}

/// The directory holding every run's record, `<root>/runs`.
pub fn runs_dir(root: &Path) -> PathBuf {
    root.join("runs")
}

/// A build's directory, inside its run's.
pub fn build_dir(root: &Path, run: &RunKey, build_id: &str) -> PathBuf {
    run::run_dir(root, run).join(build_id)
}

/// A build's id, `<UTC stamp>-<suffix>`, from the time the build started.
/// The suffix is 4 random lowercase base-36 characters, so builds started
/// in the same second get ids of their own, and ids still sort by time.
pub fn build_id(at: DateTime<Utc>) -> String {
    format!("{}-{}", at.format("%Y%m%dT%H%M%SZ"), suffix())
}

/// 4 random lowercase base-36 characters.
fn suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    // Each `RandomState` is seeded afresh, which is random enough to keep
    // ids apart; `BuildWriter::create_new` retries on the rare collision.
    let mut bits = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (0..4)
        .map(|_| {
            let digit = (bits % 36) as u32;
            bits /= 36;
            char::from_digit(digit, 36).expect("a base-36 digit")
        })
        .collect()
}

/// How many ids [`BuildWriter::create_new`] tries before giving up.
const NEW_BUILD_ATTEMPTS: usize = 5;

/// Whether a build is of an epic or a single issue. An epic's build creates
/// its epic branch and merges each issue into it; a single issue's stops at
/// its pull request into the default branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Epic,
    Issue,
}

/// How an issue or a build ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Failed,
}

/// An issue of a run: an epic sub-issue, or the epic issue of an epic
/// with none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub blocked_by: Vec<u64>,
}

/// Something that happened in a build: one line of `events.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    BuildStarted {
        at: DateTime<Utc>,
        build_id: String,
        repo: String,
        number: u64,
        title: String,
        kind: Kind,
        /// The branch the issues' pull requests merge into.
        branch: String,
        /// Every issue of the run, closed or not.
        issues: Vec<Issue>,
        /// The issues already closed when the build started, which it
        /// skips.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        closed: Vec<u64>,
        /// Epik's Claude Code plugin, which the agents load. A rerun loads
        /// it again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plugin_dir: Option<PathBuf>,
    },
    IssueStarted {
        at: DateTime<Utc>,
        number: u64,
    },
    IssueFinished {
        at: DateTime<Utc>,
        number: u64,
        outcome: Outcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pull_request: Option<u64>,
        duration_ms: u64,
        /// US dollars, as the agent reported them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<Decimal>,
    },
    IssueMerged {
        at: DateTime<Utc>,
        number: u64,
        pull_request: u64,
        /// The merge commit's SHA.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        merge_commit: Option<String>,
    },
    BuildFinished {
        at: DateTime<Utc>,
        outcome: Outcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// The pull request into the default branch: the epic's, or a
        /// single issue's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pull_request: Option<u64>,
    },
    BuildInterrupted {
        at: DateTime<Utc>,
        reason: String,
    },
    /// The build's pull request into the default branch was closed without
    /// merging, as GitHub said after the build ended.
    PullRequestClosed {
        at: DateTime<Utc>,
        pull_request: u64,
    },
}

impl Event {
    /// When the event happened.
    pub fn at(&self) -> DateTime<Utc> {
        match self {
            Event::BuildStarted { at, .. }
            | Event::IssueStarted { at, .. }
            | Event::IssueFinished { at, .. }
            | Event::IssueMerged { at, .. }
            | Event::BuildFinished { at, .. }
            | Event::BuildInterrupted { at, .. }
            | Event::PullRequestClosed { at, .. } => *at,
        }
    }
}

/// Appends a build's events to its `events.jsonl`, one line per event.
pub struct BuildWriter {
    build_id: String,
    dir: PathBuf,
    events: File,
}

impl BuildWriter {
    /// Creates a build's record in its run's directory. The build's
    /// directory must not exist yet: an existing one is
    /// [`io::ErrorKind::AlreadyExists`], never reused.
    pub fn create(root: &Path, run: &RunKey, build_id: &str) -> io::Result<Self> {
        let dir = build_dir(root, run, build_id);
        fs::create_dir_all(run::run_dir(root, run))?;
        fs::create_dir(&dir)?;
        Self::open_in(dir, build_id)
    }

    /// Creates the record of a new build of `run` started `at`, with a
    /// [`build_id`] no other build has, trying another when one is taken.
    pub fn create_new(root: &Path, run: &RunKey, at: DateTime<Utc>) -> io::Result<Self> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            match Self::create(root, run, &build_id(at)) {
                Err(error)
                    if error.kind() == io::ErrorKind::AlreadyExists
                        && attempts < NEW_BUILD_ATTEMPTS => {}
                created => return created,
            }
        }
    }

    /// Opens an existing build's record to append to it.
    pub fn open(root: &Path, run: &RunKey, build_id: &str) -> io::Result<Self> {
        let dir = build_dir(root, run, build_id);
        if !is_build_id(build_id) || !dir.join(EVENTS_FILE).is_file() {
            return Err(no_build(build_id));
        }
        Self::open_in(dir, build_id)
    }

    fn open_in(dir: PathBuf, build_id: &str) -> io::Result<Self> {
        let events = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(EVENTS_FILE))?;
        Ok(BuildWriter {
            build_id: build_id.to_owned(),
            dir,
            events,
        })
    }

    /// The build's id.
    pub fn build_id(&self) -> &str {
        &self.build_id
    }

    /// The build's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Writes the build's `summary.json`, its folded state, beside its events.
    pub fn write_summary(&self, state: &BuildState) -> io::Result<()> {
        let mut json = serde_json::to_vec_pretty(state)?;
        json.push(b'\n');
        fs::write(self.dir.join(SUMMARY_FILE), json)
    }

    /// Appends one event and flushes it, so a reader tailing the file sees
    /// it as it happens. The line goes out in a single write.
    pub fn append(&mut self, event: &Event) -> io::Result<()> {
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        self.events.write_all(&line)?;
        self.events.flush()
    }
}

fn no_build(build_id: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("no build {build_id:?}"))
}

/// Reads a build's events. A final line without its newline is one still
/// being written, and is skipped unless it is already a whole event.
///
/// An id that could name anything but a directory in its run's, such as
/// `../x`, is a build that doesn't exist: [`io::ErrorKind::NotFound`].
pub fn read_events(root: &Path, run: &RunKey, build_id: &str) -> io::Result<Vec<Event>> {
    if !is_build_id(build_id) {
        return Err(no_build(build_id));
    }
    read_events_in(&build_dir(root, run, build_id))
}

/// Whether `build_id` can be a build's id: ASCII letters, digits, `-`, `_`
/// and `.`, not starting with `.`.
fn is_build_id(build_id: &str) -> bool {
    !build_id.is_empty()
        && !build_id.starts_with('.')
        && build_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn read_events_in(dir: &Path) -> io::Result<Vec<Event>> {
    parse_events(&fs::read_to_string(dir.join(EVENTS_FILE))?)
}

fn parse_events(text: &str) -> io::Result<Vec<Event>> {
    let (complete, partial) = match text.rfind('\n') {
        Some(end) => text.split_at(end + 1),
        None => ("", text),
    };
    let mut events = complete
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(io::Error::from))
        .collect::<io::Result<Vec<Event>>>()?;
    if let Ok(event) = serde_json::from_str(partial) {
        events.push(event);
    }
    Ok(events)
}

/// Where an issue stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    Waiting,
    Running,
    /// Its agent opened a pull request.
    Succeeded,
    Merged,
    Failed,
    /// Closed before the build started, by an earlier build or by hand.
    Closed,
}

/// Where a build stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

impl From<Outcome> for IssueStatus {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Succeeded => IssueStatus::Succeeded,
            Outcome::Failed => IssueStatus::Failed,
        }
    }
}

impl From<Outcome> for BuildStatus {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Succeeded => BuildStatus::Succeeded,
            Outcome::Failed => BuildStatus::Failed,
        }
    }
}

/// An issue, as a build's events leave it, or as a run's builds do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueState {
    #[serde(flatten)]
    pub issue: Issue,
    pub status: IssueStatus,
    pub reason: Option<String>,
    pub pull_request: Option<u64>,
    /// When its agent last started, if it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// Its pull request's merge commit, once Epik has merged it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_commit: Option<String>,
    pub duration_ms: Option<u64>,
    pub cost_usd: Option<Decimal>,
    /// What its agent is doing, while it runs. Read from the agent's log
    /// by [`crate::run::read`] and [`crate::run::list`], never recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<Activity>,
}

/// A build, as its events leave it. `summary.json` is this, serialized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildState {
    pub build_id: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub kind: Kind,
    pub branch: String,
    /// Epik's plugin, as the build was started with it.
    #[serde(default)]
    pub plugin_dir: Option<PathBuf>,
    pub status: BuildStatus,
    pub reason: Option<String>,
    /// The pull request into the default branch: the epic's, or a single
    /// issue's.
    pub pull_request: Option<u64>,
    /// Whether that pull request was closed without merging.
    #[serde(default)]
    pub pull_request_closed: bool,
    pub issues: Vec<IssueState>,
    pub started_at: DateTime<Utc>,
    /// When the build finished or was interrupted.
    pub ended_at: Option<DateTime<Utc>>,
    /// The time of its latest event.
    pub latest_at: DateTime<Utc>,
    /// From the start to the end, or to the latest event while running. For
    /// a clock that keeps going between events, use [`BuildState::elapsed`].
    pub elapsed_ms: u64,
    /// The sum of the issues' costs so far.
    pub cost_usd: Decimal,
}

impl BuildState {
    /// The issue with this number, if the build has one.
    pub fn issue(&self, number: u64) -> Option<&IssueState> {
        self.issues.iter().find(|m| m.issue.number == number)
    }

    fn issue_mut(&mut self, number: u64) -> Option<&mut IssueState> {
        self.issues.iter_mut().find(|m| m.issue.number == number)
    }

    /// The run the build belongs to.
    pub fn run(&self) -> RunKey {
        RunKey::new(&self.repo, self.number)
    }

    /// How long the build has taken: to its end, or to `now` while it runs.
    pub fn elapsed(&self, now: DateTime<Utc>) -> chrono::Duration {
        self.ended_at.unwrap_or(now) - self.started_at
    }
}

/// Folds a build's events into its state, or `None` if they don't start
/// with `build_started`. Events about issues the build doesn't have are
/// ignored.
pub fn fold(events: &[Event]) -> Option<BuildState> {
    let (first, rest) = events.split_first()?;
    let Event::BuildStarted {
        at,
        build_id,
        repo,
        number,
        title,
        kind,
        branch,
        issues,
        closed,
        plugin_dir,
    } = first
    else {
        return None;
    };
    let mut state = BuildState {
        build_id: build_id.clone(),
        repo: repo.clone(),
        number: *number,
        title: title.clone(),
        kind: *kind,
        branch: branch.clone(),
        plugin_dir: plugin_dir.clone(),
        status: BuildStatus::Running,
        reason: None,
        pull_request: None,
        pull_request_closed: false,
        issues: issues
            .iter()
            .map(|issue| IssueState {
                issue: issue.clone(),
                status: if closed.contains(&issue.number) {
                    IssueStatus::Closed
                } else {
                    IssueStatus::Waiting
                },
                reason: None,
                pull_request: None,
                started_at: None,
                merge_commit: None,
                duration_ms: None,
                cost_usd: None,
                activity: None,
            })
            .collect(),
        started_at: *at,
        ended_at: None,
        latest_at: *at,
        elapsed_ms: 0,
        cost_usd: Decimal::ZERO,
    };
    for event in rest {
        state.latest_at = state.latest_at.max(event.at());
        match event {
            Event::BuildStarted { .. } => {}
            Event::IssueStarted { at, number } => {
                if let Some(issue) = state.issue_mut(*number) {
                    issue.status = IssueStatus::Running;
                    issue.started_at = Some(*at);
                }
            }
            Event::IssueFinished {
                number,
                outcome,
                reason,
                pull_request,
                duration_ms,
                cost_usd,
                ..
            } => {
                if let Some(issue) = state.issue_mut(*number) {
                    issue.status = (*outcome).into();
                    issue.reason = reason.clone();
                    issue.pull_request = *pull_request;
                    issue.duration_ms = Some(*duration_ms);
                    issue.cost_usd = *cost_usd;
                }
            }
            Event::IssueMerged {
                number,
                pull_request,
                merge_commit,
                ..
            } => {
                if let Some(issue) = state.issue_mut(*number) {
                    issue.status = IssueStatus::Merged;
                    issue.pull_request = Some(*pull_request);
                    issue.merge_commit = merge_commit.clone();
                }
            }
            Event::BuildFinished {
                at,
                outcome,
                reason,
                pull_request,
            } => {
                state.status = (*outcome).into();
                state.reason = reason.clone();
                state.pull_request = *pull_request;
                state.ended_at = Some(*at);
            }
            Event::BuildInterrupted { at, reason } => {
                state.status = BuildStatus::Interrupted;
                state.reason = Some(reason.clone());
                state.ended_at = Some(*at);
            }
            Event::PullRequestClosed { pull_request, .. } => {
                if state.pull_request == Some(*pull_request) {
                    state.pull_request_closed = true;
                }
            }
        }
    }
    let end = state.ended_at.unwrap_or(state.latest_at);
    state.elapsed_ms = u64::try_from((end - state.started_at).num_milliseconds()).unwrap_or(0);
    state.cost_usd = state.issues.iter().filter_map(|m| m.cost_usd).sum();
    Some(state)
}

/// Every build of `run` whose events can be read and folded, oldest first.
/// A build that can't be read is skipped, so one bad build doesn't hide
/// the rest.
///
/// Known cost, left for now: this reads and folds every build's events on
/// each call, and the app calls it every few seconds. Builds' events are
/// small and finished runs are deleted, so it doesn't matter yet. When it
/// does, read `summary.json` for builds that have ended.
pub fn list_builds(root: &Path, run: &RunKey) -> io::Result<Vec<BuildState>> {
    let entries = match fs::read_dir(run::run_dir(root, run)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut builds = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(state) = read_events_in(&entry.path())
            .ok()
            .and_then(|events| fold(&events))
        {
            builds.push(state);
        }
    }
    builds.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.build_id.cmp(&b.build_id))
    });
    Ok(builds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::collections::BTreeSet;
    use std::str::FromStr;

    fn time(seconds: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 28, 17, 8, 0).unwrap()
            + chrono::Duration::seconds(seconds.into())
    }

    fn dollars(text: &str) -> Decimal {
        Decimal::from_str(text).unwrap()
    }

    fn run() -> RunKey {
        RunKey::new("epik-agent/Epik", 12)
    }

    fn issue(number: u64, blocked_by: &[u64]) -> Issue {
        Issue {
            number,
            title: format!("Issue {number}"),
            blocked_by: blocked_by.to_vec(),
        }
    }

    fn epic_started(build_id: &str, at: DateTime<Utc>) -> Event {
        Event::BuildStarted {
            at,
            build_id: build_id.to_owned(),
            repo: "epik-agent/Epik".to_owned(),
            number: 12,
            title: "The epic".to_owned(),
            kind: Kind::Epic,
            branch: "epic-12".to_owned(),
            issues: vec![issue(13, &[]), issue(14, &[13])],
            closed: Vec::new(),
            plugin_dir: None,
        }
    }

    fn finished(at: u32, number: u64, outcome: Outcome, cost: &str) -> Event {
        Event::IssueFinished {
            at: time(at),
            number,
            outcome,
            reason: (outcome == Outcome::Failed).then(|| "tests failed".to_owned()),
            pull_request: (outcome == Outcome::Succeeded).then_some(number + 100),
            duration_ms: 1_000,
            cost_usd: Some(dollars(cost)),
        }
    }

    fn successful_epic() -> Vec<Event> {
        vec![
            epic_started("build", time(0)),
            Event::IssueStarted {
                at: time(1),
                number: 13,
            },
            finished(10, 13, Outcome::Succeeded, "0.10"),
            Event::IssueMerged {
                at: time(11),
                number: 13,
                pull_request: 113,
                merge_commit: None,
            },
            Event::IssueStarted {
                at: time(12),
                number: 14,
            },
            finished(20, 14, Outcome::Succeeded, "0.20"),
            Event::IssueMerged {
                at: time(21),
                number: 14,
                pull_request: 114,
                merge_commit: None,
            },
            Event::BuildFinished {
                at: time(30),
                outcome: Outcome::Succeeded,
                reason: None,
                pull_request: Some(200),
            },
        ]
    }

    #[test]
    fn build_id_joins_a_utc_stamp_and_a_suffix() {
        let at = Utc.with_ymd_and_hms(2026, 10, 28, 17, 8, 39).unwrap();
        let id = build_id(at);
        let suffix = id
            .strip_prefix("20261028T170839Z-")
            .unwrap_or_else(|| panic!("{id}"));
        assert_eq!(suffix.len(), 4, "{id}");
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "{id}"
        );
        assert!(is_build_id(&id));
    }

    #[test]
    fn build_ids_made_at_the_same_instant_differ() {
        let at = time(0);
        let ids: BTreeSet<String> = (0..100).map(|_| build_id(at)).collect();
        assert!(ids.len() > 90, "{ids:?}");
    }

    #[test]
    fn a_new_build_never_reuses_an_existing_builds_directory() {
        let root = tempfile::tempdir().unwrap();
        let first = BuildWriter::create_new(root.path(), &run(), time(0)).unwrap();
        let second = BuildWriter::create_new(root.path(), &run(), time(0)).unwrap();
        assert_ne!(first.build_id(), second.build_id());
        assert_ne!(first.dir(), second.dir());
        assert!(first.dir().is_dir() && second.dir().is_dir());
        assert_eq!(
            first.dir(),
            build_dir(root.path(), &run(), first.build_id())
        );
    }

    #[test]
    fn creating_a_record_refuses_an_existing_directory() {
        let root = tempfile::tempdir().unwrap();
        BuildWriter::create(root.path(), &run(), "build").unwrap();
        let error = BuildWriter::create(root.path(), &run(), "build")
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn a_build_directory_is_inside_its_runs() {
        let root = Path::new("/home/someone/.epik");
        assert_eq!(
            build_dir(root, &run(), "20261028T170839Z-k3f9"),
            root.join("runs")
                .join("epik-agent")
                .join("epik")
                .join("12")
                .join("20261028T170839Z-k3f9")
        );
    }

    #[test]
    fn events_are_tagged_by_type_in_snake_case() {
        let line = serde_json::to_value(finished(10, 13, Outcome::Failed, "0.25")).unwrap();
        assert_eq!(
            line,
            serde_json::json!({
                "type": "issue_finished",
                "at": "2026-10-28T17:08:10Z",
                "number": 13,
                "outcome": "failed",
                "reason": "tests failed",
                "duration_ms": 1000,
                "cost_usd": 0.25,
            })
        );
    }

    #[test]
    fn events_round_trip_through_the_record() {
        let root = tempfile::tempdir().unwrap();
        let events = successful_epic();
        let mut writer = BuildWriter::create(root.path(), &run(), "build").unwrap();
        for event in &events {
            writer.append(event).unwrap();
        }
        assert_eq!(read_events(root.path(), &run(), "build").unwrap(), events);
    }

    #[test]
    fn an_existing_build_can_be_appended_to() {
        let root = tempfile::tempdir().unwrap();
        let events = successful_epic();
        let mut writer = BuildWriter::create(root.path(), &run(), "build").unwrap();
        writer.append(&events[0]).unwrap();
        drop(writer);

        let mut writer = BuildWriter::open(root.path(), &run(), "build").unwrap();
        writer.append(&events[1]).unwrap();

        assert_eq!(
            read_events(root.path(), &run(), "build").unwrap(),
            events[..2]
        );
        for missing in ["other", "../build", ""] {
            let error = BuildWriter::open(root.path(), &run(), missing).err();
            assert_eq!(error.unwrap().kind(), io::ErrorKind::NotFound, "{missing}");
        }
    }

    #[test]
    fn each_appended_event_is_a_whole_line_on_disk() {
        let root = tempfile::tempdir().unwrap();
        let mut writer = BuildWriter::create(root.path(), &run(), "build").unwrap();
        writer.append(&epic_started("build", time(0))).unwrap();
        let text = fs::read_to_string(writer.dir().join(EVENTS_FILE)).unwrap();
        assert!(text.ends_with('\n'));
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn reader_skips_a_final_partial_line() {
        let root = tempfile::tempdir().unwrap();
        let mut writer = BuildWriter::create(root.path(), &run(), "build").unwrap();
        writer.append(&epic_started("build", time(0))).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(writer.dir().join(EVENTS_FILE))
            .unwrap();
        file.write_all(br#"{"type":"issue_started","at":"2026-10"#)
            .unwrap();
        assert_eq!(
            read_events(root.path(), &run(), "build").unwrap(),
            vec![epic_started("build", time(0))]
        );
    }

    #[test]
    fn reader_rejects_a_malformed_complete_line() {
        let error = parse_events("not json\n").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn fold_of_a_build_just_started_has_every_issue_waiting() {
        let state = fold(&[epic_started("build", time(0))]).unwrap();
        assert_eq!(state.status, BuildStatus::Running);
        assert!(
            state
                .issues
                .iter()
                .all(|m| m.status == IssueStatus::Waiting)
        );
        assert_eq!(state.cost_usd, Decimal::ZERO);
        assert_eq!(state.elapsed_ms, 0);
        assert_eq!(state.run(), run());
    }

    #[test]
    fn fold_marks_the_issues_closed_when_the_build_started() {
        let Event::BuildStarted {
            at,
            build_id,
            repo,
            number,
            title,
            kind,
            branch,
            issues,
            ..
        } = epic_started("build", time(0))
        else {
            unreachable!()
        };
        let started = Event::BuildStarted {
            at,
            build_id,
            repo,
            number,
            title,
            kind,
            branch,
            issues,
            closed: vec![13],
            plugin_dir: None,
        };
        let state = fold(&[started]).unwrap();
        assert_eq!(state.issue(13).unwrap().status, IssueStatus::Closed);
        assert_eq!(state.issue(14).unwrap().status, IssueStatus::Waiting);
    }

    #[test]
    fn fold_of_a_build_in_progress_counts_cost_and_time_so_far() {
        let events = &successful_epic()[..5];
        let state = fold(events).unwrap();
        assert_eq!(state.status, BuildStatus::Running);
        assert_eq!(state.issue(13).unwrap().status, IssueStatus::Merged);
        assert_eq!(state.issue(14).unwrap().status, IssueStatus::Running);
        assert_eq!(state.issue(14).unwrap().started_at, Some(time(12)));
        assert_eq!(state.cost_usd, dollars("0.10"));
        assert_eq!(state.elapsed_ms, 12_000);
        assert_eq!(state.ended_at, None);
        assert_eq!(state.latest_at, time(12));
    }

    #[test]
    fn fold_of_an_epic_that_succeeds() {
        let state = fold(&successful_epic()).unwrap();
        assert_eq!(state.status, BuildStatus::Succeeded);
        assert_eq!(state.pull_request, Some(200));
        assert!(!state.pull_request_closed);
        assert_eq!(state.kind, Kind::Epic);
        assert_eq!(state.branch, "epic-12");
        let issue = state.issue(14).unwrap();
        assert_eq!(issue.status, IssueStatus::Merged);
        assert_eq!(issue.pull_request, Some(114));
        assert_eq!(issue.duration_ms, Some(1_000));
        assert_eq!(issue.cost_usd, Some(dollars("0.20")));
        assert_eq!(issue.issue.blocked_by, vec![13]);
        assert_eq!(state.cost_usd, dollars("0.30"));
        assert_eq!(state.elapsed_ms, 30_000);
        assert_eq!(state.ended_at, Some(time(30)));
    }

    #[test]
    fn fold_notes_a_pull_request_closed_without_merging() {
        let mut events = successful_epic();
        events.push(Event::PullRequestClosed {
            at: time(100),
            pull_request: 200,
        });
        let state = fold(&events).unwrap();
        assert!(state.pull_request_closed);
        assert_eq!(state.status, BuildStatus::Succeeded);
        assert_eq!(state.elapsed_ms, 30_000, "the build itself ended at 30s");
    }

    #[test]
    fn fold_of_an_epic_with_a_failed_issue() {
        let events = vec![
            epic_started("build", time(0)),
            Event::IssueStarted {
                at: time(1),
                number: 13,
            },
            finished(10, 13, Outcome::Failed, "0.05"),
            Event::BuildFinished {
                at: time(11),
                outcome: Outcome::Failed,
                reason: Some("#13 failed".to_owned()),
                pull_request: None,
            },
        ];
        let state = fold(&events).unwrap();
        assert_eq!(state.status, BuildStatus::Failed);
        assert_eq!(state.reason.as_deref(), Some("#13 failed"));
        let failed = state.issue(13).unwrap();
        assert_eq!(failed.status, IssueStatus::Failed);
        assert_eq!(failed.reason.as_deref(), Some("tests failed"));
        assert_eq!(state.issue(14).unwrap().status, IssueStatus::Waiting);
        assert_eq!(state.cost_usd, dollars("0.05"));
    }

    #[test]
    fn fold_of_an_interrupted_build() {
        let events = vec![
            epic_started("build", time(0)),
            Event::IssueStarted {
                at: time(1),
                number: 13,
            },
            Event::BuildInterrupted {
                at: time(5),
                reason: "the app quit".to_owned(),
            },
        ];
        let state = fold(&events).unwrap();
        assert_eq!(state.status, BuildStatus::Interrupted);
        assert_eq!(state.reason.as_deref(), Some("the app quit"));
        assert_eq!(state.issue(13).unwrap().status, IssueStatus::Running);
        assert_eq!(state.elapsed_ms, 5_000);
        assert_eq!(state.elapsed(time(60)), chrono::Duration::seconds(5));
    }

    #[test]
    fn elapsed_keeps_counting_while_a_build_is_running() {
        let state = fold(&successful_epic()[..2]).unwrap();
        assert_eq!(state.elapsed_ms, 1_000);
        assert_eq!(state.elapsed(time(60)), chrono::Duration::seconds(60));
    }

    #[test]
    fn fold_of_a_single_issue_build() {
        let events = vec![Event::BuildStarted {
            at: time(0),
            build_id: "build".to_owned(),
            repo: "epik-agent/Epik".to_owned(),
            number: 7,
            title: "One issue".to_owned(),
            kind: Kind::Issue,
            branch: "main".to_owned(),
            issues: vec![issue(7, &[])],
            closed: Vec::new(),
            plugin_dir: None,
        }];
        let state = fold(&events).unwrap();
        assert_eq!(state.kind, Kind::Issue);
        assert_eq!(state.issues.len(), 1);
        assert_eq!(state.issue(7).unwrap().status, IssueStatus::Waiting);
    }

    #[test]
    fn fold_needs_a_build_started_event_first() {
        assert_eq!(fold(&[]), None);
        assert_eq!(
            fold(&[Event::IssueStarted {
                at: time(0),
                number: 1
            }]),
            None
        );
    }

    #[test]
    fn summary_is_the_fold_serialized() {
        let root = tempfile::tempdir().unwrap();
        let state = fold(&successful_epic()).unwrap();
        BuildWriter::create(root.path(), &run(), "build")
            .unwrap()
            .write_summary(&state)
            .unwrap();
        let path = build_dir(root.path(), &run(), "build").join(SUMMARY_FILE);
        let text = fs::read_to_string(path).unwrap();
        let read: BuildState = serde_json::from_str(&text).unwrap();
        assert_eq!(read, state);
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["status"], "succeeded");
        assert_eq!(json["cost_usd"], 0.3);
        assert_eq!(json["elapsed_ms"], 30_000);
        assert_eq!(json["issues"][0]["number"], 13);
        assert_eq!(json["issues"][0]["duration_ms"], 1_000);
    }

    #[test]
    fn a_runs_builds_are_listed_oldest_first() {
        let root = tempfile::tempdir().unwrap();
        for (id, at) in [("b", time(60)), ("c", time(120)), ("a", time(0))] {
            let mut writer = BuildWriter::create(root.path(), &run(), id).unwrap();
            writer.append(&epic_started(id, at)).unwrap();
        }
        fs::create_dir_all(build_dir(root.path(), &run(), "empty")).unwrap();
        let corrupt = build_dir(root.path(), &run(), "corrupt");
        fs::create_dir_all(&corrupt).unwrap();
        fs::write(corrupt.join(EVENTS_FILE), "not json\n").unwrap();
        let other = RunKey::new("epik-agent/Epik", 13);
        let mut writer = BuildWriter::create(root.path(), &other, "d").unwrap();
        writer.append(&epic_started("d", time(5))).unwrap();

        let ids: Vec<String> = list_builds(root.path(), &run())
            .unwrap()
            .into_iter()
            .map(|build| build.build_id)
            .collect();

        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn a_run_with_no_record_has_no_builds() {
        let root = tempfile::tempdir().unwrap();
        assert!(list_builds(root.path(), &run()).unwrap().is_empty());
    }

    #[test]
    fn an_id_that_leaves_its_runs_directory_is_no_build() {
        let root = tempfile::tempdir().unwrap();
        // `../13/x` would reach a build of another run.
        let other = RunKey::new("epik-agent/Epik", 13);
        BuildWriter::create(root.path(), &other, "x").unwrap();

        for build_id in ["..", "../13/x", "a/b", "a\\b", ".hidden", ""] {
            let error = read_events(root.path(), &run(), build_id).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::NotFound, "{build_id}");
        }
    }
}
