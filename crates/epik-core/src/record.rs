//! A run's record: the one place that says how a build went.
//!
//! Each run has a directory, `<root>/runs/<run-id>/`, where the root is
//! normally `~/.epik`. The app appends Epik's own events to `events.jsonl`
//! as they happen, and writes `summary.json` when the run ends. Everything
//! that shows a run (the tray, the run window, the socket) folds the events
//! into a [`RunState`], so the fold is the one place that decides what a run
//! looks like.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// The file a run's events are appended to.
pub const EVENTS_FILE: &str = "events.jsonl";
/// The file a run's folded state is written to when it ends.
pub const SUMMARY_FILE: &str = "summary.json";

/// Epik's root directory, `~/.epik`, or `None` if there is no home directory.
pub fn default_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".epik"))
}

/// The directory holding every run, `<root>/runs`.
pub fn runs_dir(root: &Path) -> PathBuf {
    root.join("runs")
}

/// A run's directory, `<root>/runs/<run-id>`.
pub fn run_dir(root: &Path, run_id: &str) -> PathBuf {
    runs_dir(root).join(run_id)
}

/// A run's id, `<owner>-<name>-<number>-<UTC stamp>`, from a repository
/// written `owner/name`, the number of the issue or epic built, and the time
/// the run started.
pub fn run_id(repo: &str, number: u64, at: DateTime<Utc>) -> String {
    format!(
        "{}-{number}-{}",
        repo.replace('/', "-"),
        at.format("%Y%m%dT%H%M%SZ")
    )
}

/// Whether a run builds an epic or a single issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Epic,
    Issue,
}

/// How an issue or a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Failed,
}

/// An issue a run builds. A single issue is a run whose only member is
/// itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub number: u64,
    pub title: String,
    pub blocked_by: Vec<u64>,
}

/// Something that happened in a run: one line of `events.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    RunStarted {
        at: DateTime<Utc>,
        run_id: String,
        repo: String,
        number: u64,
        title: String,
        kind: Kind,
        /// The branch the issues' pull requests merge into.
        branch: String,
        members: Vec<Member>,
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
    },
    RunFinished {
        at: DateTime<Utc>,
        outcome: Outcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// The epic's pull request.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pull_request: Option<u64>,
    },
    RunInterrupted {
        at: DateTime<Utc>,
        reason: String,
    },
}

impl Event {
    /// When the event happened.
    pub fn at(&self) -> DateTime<Utc> {
        match self {
            Event::RunStarted { at, .. }
            | Event::IssueStarted { at, .. }
            | Event::IssueFinished { at, .. }
            | Event::IssueMerged { at, .. }
            | Event::RunFinished { at, .. }
            | Event::RunInterrupted { at, .. } => *at,
        }
    }
}

/// Appends a run's events to its `events.jsonl`, one line per event.
pub struct RunWriter {
    dir: PathBuf,
    events: File,
}

impl RunWriter {
    /// Opens a run's record for appending, creating its directory if needed.
    pub fn create(root: &Path, run_id: &str) -> io::Result<Self> {
        let dir = run_dir(root, run_id);
        fs::create_dir_all(&dir)?;
        let events = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(EVENTS_FILE))?;
        Ok(RunWriter { dir, events })
    }

    /// The run's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Writes the run's `summary.json`, its folded state, beside its events.
    pub fn write_summary(&self, state: &RunState) -> io::Result<()> {
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

/// Reads a run's events. A final line without its newline is one still
/// being written, and is skipped unless it is already a whole event.
///
/// An id that could name anything but a directory in `<root>/runs`, such as
/// `../x`, is a run that doesn't exist: [`io::ErrorKind::NotFound`].
pub fn read_events(root: &Path, run_id: &str) -> io::Result<Vec<Event>> {
    if !is_run_id(run_id) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no run {run_id:?}"),
        ));
    }
    read_events_in(&run_dir(root, run_id))
}

/// Whether `run_id` can be a run's id: ASCII letters, digits, `-`, `_` and
/// `.`, not starting with `.`.
fn is_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && !run_id.starts_with('.')
        && run_id
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

/// Where an issue in a run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    Waiting,
    Running,
    Succeeded,
    Merged,
    Failed,
}

/// Where a run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

impl From<Outcome> for MemberStatus {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Succeeded => MemberStatus::Succeeded,
            Outcome::Failed => MemberStatus::Failed,
        }
    }
}

impl From<Outcome> for RunStatus {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Succeeded => RunStatus::Succeeded,
            Outcome::Failed => RunStatus::Failed,
        }
    }
}

/// An issue in a run, as its events leave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemberState {
    #[serde(flatten)]
    pub member: Member,
    pub status: MemberStatus,
    pub reason: Option<String>,
    pub pull_request: Option<u64>,
    pub duration_ms: Option<u64>,
    pub cost_usd: Option<Decimal>,
}

/// A run, as its events leave it. `summary.json` is this, serialized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub run_id: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub kind: Kind,
    pub branch: String,
    pub status: RunStatus,
    pub reason: Option<String>,
    /// The epic's pull request.
    pub pull_request: Option<u64>,
    pub members: Vec<MemberState>,
    pub started_at: DateTime<Utc>,
    /// When the run finished or was interrupted.
    pub ended_at: Option<DateTime<Utc>>,
    /// From the start to the end, or to the latest event while running. For
    /// a clock that keeps going between events, use [`RunState::elapsed`].
    pub elapsed_ms: u64,
    /// The sum of the members' costs so far.
    pub cost_usd: Decimal,
}

impl RunState {
    /// The member with this number, if the run has one.
    pub fn member(&self, number: u64) -> Option<&MemberState> {
        self.members.iter().find(|m| m.member.number == number)
    }

    fn member_mut(&mut self, number: u64) -> Option<&mut MemberState> {
        self.members.iter_mut().find(|m| m.member.number == number)
    }

    /// How long the run has taken: to its end, or to `now` while it runs.
    pub fn elapsed(&self, now: DateTime<Utc>) -> chrono::Duration {
        self.ended_at.unwrap_or(now) - self.started_at
    }
}

/// Folds a run's events into its state, or `None` if they don't start with
/// `run_started`. Events about issues that aren't members are ignored.
pub fn fold(events: &[Event]) -> Option<RunState> {
    let (first, rest) = events.split_first()?;
    let Event::RunStarted {
        at,
        run_id,
        repo,
        number,
        title,
        kind,
        branch,
        members,
    } = first
    else {
        return None;
    };
    let mut state = RunState {
        run_id: run_id.clone(),
        repo: repo.clone(),
        number: *number,
        title: title.clone(),
        kind: *kind,
        branch: branch.clone(),
        status: RunStatus::Running,
        reason: None,
        pull_request: None,
        members: members
            .iter()
            .map(|member| MemberState {
                member: member.clone(),
                status: MemberStatus::Waiting,
                reason: None,
                pull_request: None,
                duration_ms: None,
                cost_usd: None,
            })
            .collect(),
        started_at: *at,
        ended_at: None,
        elapsed_ms: 0,
        cost_usd: Decimal::ZERO,
    };
    let mut latest = *at;
    for event in rest {
        latest = latest.max(event.at());
        match event {
            Event::RunStarted { .. } => {}
            Event::IssueStarted { number, .. } => {
                if let Some(member) = state.member_mut(*number) {
                    member.status = MemberStatus::Running;
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
                if let Some(member) = state.member_mut(*number) {
                    member.status = (*outcome).into();
                    member.reason = reason.clone();
                    member.pull_request = *pull_request;
                    member.duration_ms = Some(*duration_ms);
                    member.cost_usd = *cost_usd;
                }
            }
            Event::IssueMerged {
                number,
                pull_request,
                ..
            } => {
                if let Some(member) = state.member_mut(*number) {
                    member.status = MemberStatus::Merged;
                    member.pull_request = Some(*pull_request);
                }
            }
            Event::RunFinished {
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
            Event::RunInterrupted { at, reason } => {
                state.status = RunStatus::Interrupted;
                state.reason = Some(reason.clone());
                state.ended_at = Some(*at);
            }
        }
    }
    let end = state.ended_at.unwrap_or(latest);
    state.elapsed_ms = u64::try_from((end - state.started_at).num_milliseconds()).unwrap_or(0);
    state.cost_usd = state.members.iter().filter_map(|m| m.cost_usd).sum();
    Some(state)
}

/// Every run under the root whose events can be read and folded, newest
/// first. A run that can't be read is skipped, so one bad run doesn't hide
/// the rest.
///
/// Known cost, left for now: this reads and folds every run's events on each
/// call, and the tray calls it every two seconds, so the work grows with
/// every run ever kept. Runs' events are small, so it doesn't matter yet.
/// When it does, read `summary.json` for runs that have ended and fold
/// only the rest; cleanup of old runs also bounds it.
pub fn list_runs(root: &Path) -> io::Result<Vec<RunState>> {
    let entries = match fs::read_dir(runs_dir(root)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut runs = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(state) = read_events_in(&entry.path())
            .ok()
            .and_then(|events| fold(&events))
        {
            runs.push(state);
        }
    }
    runs.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| b.run_id.cmp(&a.run_id))
    });
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::str::FromStr;

    fn time(seconds: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 28, 17, 8, 0).unwrap()
            + chrono::Duration::seconds(seconds.into())
    }

    fn dollars(text: &str) -> Decimal {
        Decimal::from_str(text).unwrap()
    }

    fn member(number: u64, blocked_by: &[u64]) -> Member {
        Member {
            number,
            title: format!("Issue {number}"),
            blocked_by: blocked_by.to_vec(),
        }
    }

    fn epic_started(run_id: &str, at: DateTime<Utc>) -> Event {
        Event::RunStarted {
            at,
            run_id: run_id.to_owned(),
            repo: "epik-agent/Epik".to_owned(),
            number: 12,
            title: "The epic".to_owned(),
            kind: Kind::Epic,
            branch: "epic-12".to_owned(),
            members: vec![member(13, &[]), member(14, &[13])],
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
            epic_started("run", time(0)),
            Event::IssueStarted {
                at: time(1),
                number: 13,
            },
            finished(10, 13, Outcome::Succeeded, "0.10"),
            Event::IssueMerged {
                at: time(11),
                number: 13,
                pull_request: 113,
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
            },
            Event::RunFinished {
                at: time(30),
                outcome: Outcome::Succeeded,
                reason: None,
                pull_request: Some(200),
            },
        ]
    }

    #[test]
    fn run_id_joins_repository_number_and_utc_stamp() {
        let at = Utc.with_ymd_and_hms(2026, 10, 28, 17, 8, 39).unwrap();
        assert_eq!(
            run_id("epik-agent/Epik", 12, at),
            "epik-agent-Epik-12-20261028T170839Z"
        );
    }

    #[test]
    fn run_directory_is_under_the_root() {
        let root = Path::new("/home/someone/.epik");
        assert_eq!(
            run_dir(root, "a-b-1-20261028T170839Z"),
            root.join("runs").join("a-b-1-20261028T170839Z")
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
        let mut writer = RunWriter::create(root.path(), "run").unwrap();
        for event in &events {
            writer.append(event).unwrap();
        }
        assert_eq!(read_events(root.path(), "run").unwrap(), events);
    }

    #[test]
    fn each_appended_event_is_a_whole_line_on_disk() {
        let root = tempfile::tempdir().unwrap();
        let mut writer = RunWriter::create(root.path(), "run").unwrap();
        writer.append(&epic_started("run", time(0))).unwrap();
        let text = fs::read_to_string(writer.dir().join(EVENTS_FILE)).unwrap();
        assert!(text.ends_with('\n'));
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn reader_skips_a_final_partial_line() {
        let root = tempfile::tempdir().unwrap();
        let mut writer = RunWriter::create(root.path(), "run").unwrap();
        writer.append(&epic_started("run", time(0))).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(writer.dir().join(EVENTS_FILE))
            .unwrap();
        file.write_all(br#"{"type":"issue_started","at":"2026-10"#)
            .unwrap();
        assert_eq!(
            read_events(root.path(), "run").unwrap(),
            vec![epic_started("run", time(0))]
        );
    }

    #[test]
    fn reader_rejects_a_malformed_complete_line() {
        let error = parse_events("not json\n").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn fold_of_a_run_just_started_has_every_member_waiting() {
        let state = fold(&[epic_started("run", time(0))]).unwrap();
        assert_eq!(state.status, RunStatus::Running);
        assert!(
            state
                .members
                .iter()
                .all(|m| m.status == MemberStatus::Waiting)
        );
        assert_eq!(state.cost_usd, Decimal::ZERO);
        assert_eq!(state.elapsed_ms, 0);
    }

    #[test]
    fn fold_of_a_run_in_progress_counts_cost_and_time_so_far() {
        let events = &successful_epic()[..5];
        let state = fold(events).unwrap();
        assert_eq!(state.status, RunStatus::Running);
        assert_eq!(state.member(13).unwrap().status, MemberStatus::Merged);
        assert_eq!(state.member(14).unwrap().status, MemberStatus::Running);
        assert_eq!(state.cost_usd, dollars("0.10"));
        assert_eq!(state.elapsed_ms, 12_000);
        assert_eq!(state.ended_at, None);
    }

    #[test]
    fn fold_of_an_epic_that_succeeds() {
        let state = fold(&successful_epic()).unwrap();
        assert_eq!(state.status, RunStatus::Succeeded);
        assert_eq!(state.pull_request, Some(200));
        assert_eq!(state.kind, Kind::Epic);
        assert_eq!(state.branch, "epic-12");
        let issue = state.member(14).unwrap();
        assert_eq!(issue.status, MemberStatus::Merged);
        assert_eq!(issue.pull_request, Some(114));
        assert_eq!(issue.duration_ms, Some(1_000));
        assert_eq!(issue.cost_usd, Some(dollars("0.20")));
        assert_eq!(issue.member.blocked_by, vec![13]);
        assert_eq!(state.cost_usd, dollars("0.30"));
        assert_eq!(state.elapsed_ms, 30_000);
        assert_eq!(state.ended_at, Some(time(30)));
    }

    #[test]
    fn fold_of_an_epic_with_a_failed_issue() {
        let events = vec![
            epic_started("run", time(0)),
            Event::IssueStarted {
                at: time(1),
                number: 13,
            },
            finished(10, 13, Outcome::Failed, "0.05"),
            Event::RunFinished {
                at: time(11),
                outcome: Outcome::Failed,
                reason: Some("#13 failed".to_owned()),
                pull_request: None,
            },
        ];
        let state = fold(&events).unwrap();
        assert_eq!(state.status, RunStatus::Failed);
        assert_eq!(state.reason.as_deref(), Some("#13 failed"));
        let failed = state.member(13).unwrap();
        assert_eq!(failed.status, MemberStatus::Failed);
        assert_eq!(failed.reason.as_deref(), Some("tests failed"));
        assert_eq!(state.member(14).unwrap().status, MemberStatus::Waiting);
        assert_eq!(state.cost_usd, dollars("0.05"));
    }

    #[test]
    fn fold_of_an_interrupted_run() {
        let events = vec![
            epic_started("run", time(0)),
            Event::IssueStarted {
                at: time(1),
                number: 13,
            },
            Event::RunInterrupted {
                at: time(5),
                reason: "the app quit".to_owned(),
            },
        ];
        let state = fold(&events).unwrap();
        assert_eq!(state.status, RunStatus::Interrupted);
        assert_eq!(state.reason.as_deref(), Some("the app quit"));
        assert_eq!(state.member(13).unwrap().status, MemberStatus::Running);
        assert_eq!(state.elapsed_ms, 5_000);
        assert_eq!(state.elapsed(time(60)), chrono::Duration::seconds(5));
    }

    #[test]
    fn elapsed_keeps_counting_while_a_run_is_running() {
        let state = fold(&successful_epic()[..2]).unwrap();
        assert_eq!(state.elapsed_ms, 1_000);
        assert_eq!(state.elapsed(time(60)), chrono::Duration::seconds(60));
    }

    #[test]
    fn fold_of_a_single_issue_run() {
        let events = vec![Event::RunStarted {
            at: time(0),
            run_id: "run".to_owned(),
            repo: "epik-agent/Epik".to_owned(),
            number: 7,
            title: "One issue".to_owned(),
            kind: Kind::Issue,
            branch: "main".to_owned(),
            members: vec![member(7, &[])],
        }];
        let state = fold(&events).unwrap();
        assert_eq!(state.kind, Kind::Issue);
        assert_eq!(state.members.len(), 1);
        assert_eq!(state.member(7).unwrap().status, MemberStatus::Waiting);
    }

    #[test]
    fn fold_needs_a_run_started_event_first() {
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
        RunWriter::create(root.path(), "run")
            .unwrap()
            .write_summary(&state)
            .unwrap();
        let text = fs::read_to_string(run_dir(root.path(), "run").join(SUMMARY_FILE)).unwrap();
        let read: RunState = serde_json::from_str(&text).unwrap();
        assert_eq!(read, state);
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["status"], "succeeded");
        assert_eq!(json["cost_usd"], 0.3);
        assert_eq!(json["elapsed_ms"], 30_000);
        assert_eq!(json["members"][0]["number"], 13);
        assert_eq!(json["members"][0]["duration_ms"], 1_000);
    }

    #[test]
    fn listing_shows_runs_newest_first() {
        let root = tempfile::tempdir().unwrap();
        for (id, at) in [
            ("older", time(0)),
            ("newest", time(120)),
            ("middle", time(60)),
        ] {
            let mut writer = RunWriter::create(root.path(), id).unwrap();
            writer.append(&epic_started(id, at)).unwrap();
        }
        fs::create_dir_all(run_dir(root.path(), "empty")).unwrap();
        let corrupt = run_dir(root.path(), "corrupt");
        fs::create_dir_all(&corrupt).unwrap();
        fs::write(corrupt.join(EVENTS_FILE), "not json\n").unwrap();
        let ids: Vec<String> = list_runs(root.path())
            .unwrap()
            .into_iter()
            .map(|run| run.run_id)
            .collect();
        assert_eq!(ids, ["newest", "middle", "older"]);
    }

    #[test]
    fn listing_an_empty_root_is_empty() {
        let root = tempfile::tempdir().unwrap();
        assert!(list_runs(root.path()).unwrap().is_empty());
    }

    #[test]
    fn an_id_that_leaves_the_runs_directory_is_no_run() {
        let root = tempfile::tempdir().unwrap();
        // `../x` would reach this run outside `runs`.
        fs::create_dir(root.path().join("x")).unwrap();
        fs::write(root.path().join("x").join(EVENTS_FILE), "").unwrap();

        for run_id in ["..", "../x", "a/b", "a\\b", ".hidden", ""] {
            let error = read_events(root.path(), run_id).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::NotFound, "{run_id}");
        }
    }
}
