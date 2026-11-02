//! Runs: the work of finishing one epic, across all its builds.
//!
//! A run is identified by its epic issue, `owner/name#number`: a [`RunKey`].
//! Its record is a directory, `<root>/runs/<owner>/<name>/<number>/`,
//! holding one directory per build, and its builds' clones are under
//! `<root>/builds/` at the same keys. A run's state, [`RunState`], is one
//! fold over its builds' states: each issue as it now stands, elapsed time
//! and cost summed, and the builds in order.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::activity;
use crate::cleanup;
use crate::record::{
    self, BuildState, BuildStatus, BuildWriter, Event, IssueState, IssueStatus, Kind, Session,
};

/// A run's identity: its epic issue, written `owner/name#number`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunKey {
    /// The repository, `owner/name`.
    pub repo: String,
    /// The epic issue.
    pub number: u64,
}

impl RunKey {
    /// The run of issue `number` in `repo`. GitHub's names ignore case, so
    /// a key is lowercased: one epic is one run however it is spelled.
    pub fn new(repo: &str, number: u64) -> Self {
        Self {
            repo: repo.to_ascii_lowercase(),
            number,
        }
    }

    /// The repository's owner and name.
    fn parts(&self) -> (&str, &str) {
        self.repo.split_once('/').unwrap_or((&self.repo, ""))
    }

    /// The key's directory under `base`: `<base>/<owner>/<name>/<number>`.
    fn under(&self, base: PathBuf) -> PathBuf {
        let (owner, name) = self.parts();
        base.join(owner).join(name).join(self.number.to_string())
    }
}

impl fmt::Display for RunKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.repo, self.number)
    }
}

/// Whether `part` can be a GitHub owner's or repository's name, and so a
/// directory name that stays where it is put: ASCII letters, digits, `-`,
/// `_` and `.`, not starting with `.`.
fn is_name(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('.')
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl FromStr for RunKey {
    type Err = String;

    /// Reads `owner/name#number`.
    fn from_str(text: &str) -> Result<Self, String> {
        let bad = || format!("{text:?} is not a run: write owner/name#number");
        let (repo, number) = text.split_once('#').ok_or_else(bad)?;
        let (owner, name) = repo.split_once('/').ok_or_else(bad)?;
        if !is_name(owner) || !is_name(name) {
            return Err(bad());
        }
        let number = number.parse().map_err(|_| bad())?;
        Ok(Self::new(repo, number))
    }
}

impl Serialize for RunKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RunKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A run's record directory, `<root>/runs/<owner>/<name>/<number>`.
pub fn run_dir(root: &Path, run: &RunKey) -> PathBuf {
    run.under(record::runs_dir(root))
}

/// Where a build of `run` clones its repository,
/// `<root>/builds/<owner>/<name>/<number>/<build-id>`.
pub fn clone_dir(root: &Path, run: &RunKey, build_id: &str) -> PathBuf {
    run.under(cleanup::builds_dir(root)).join(build_id)
}

/// Where a run stands, as the window sorts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// A build is running.
    Building,
    /// Every issue merged, and its pull request into the default branch
    /// waits for the user.
    WaitingForReview,
    /// Its latest build failed or was interrupted, or its pull request was
    /// closed without merging.
    NeedsAttention,
}

/// One build of a run, in brief.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildSummary {
    pub build_id: String,
    pub status: BuildStatus,
    pub reason: Option<String>,
    pub pull_request: Option<u64>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub elapsed_ms: u64,
    pub cost_usd: Decimal,
}

/// A run, as its builds leave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub run: RunKey,
    pub repo: String,
    pub number: u64,
    /// The epic issue's title, as the latest build read it.
    pub title: String,
    pub kind: Kind,
    pub branch: String,
    pub disposition: Disposition,
    /// Why the run needs attention, if it does.
    pub reason: Option<String>,
    /// The latest build's pull request into the default branch.
    pub pull_request: Option<u64>,
    /// Every issue of the run, including ones merged in earlier builds.
    /// Each one's duration and cost are summed over the builds, and its
    /// sessions are every build's.
    pub issues: Vec<IssueState>,
    /// The run's own agent sessions, such as reviews of its pull request,
    /// over every build, oldest first.
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// Oldest first.
    pub builds: Vec<BuildSummary>,
    /// The running build's id, if one is running.
    pub running_build: Option<String>,
    /// When the first build started.
    pub started_at: DateTime<Utc>,
    /// The latest event of any build.
    pub latest_at: DateTime<Utc>,
    /// Summed over the builds. For a clock that keeps going, use
    /// [`RunState::elapsed`].
    pub elapsed_ms: u64,
    /// Summed over the builds.
    pub cost_usd: Decimal,
}

impl RunState {
    /// The issue with this number, if the run has one.
    pub fn issue(&self, number: u64) -> Option<&IssueState> {
        self.issues.iter().find(|m| m.issue.number == number)
    }

    /// How long the run's builds have taken, counting a running build to
    /// `now`.
    pub fn elapsed(&self, now: DateTime<Utc>) -> chrono::Duration {
        self.builds
            .iter()
            .map(|build| match build.ended_at {
                Some(_) => chrono::Duration::milliseconds(build.elapsed_ms as i64),
                None => now - build.started_at,
            })
            .sum()
    }
}

/// Folds a run's builds, oldest first, into the run's state, or `None`
/// with no builds.
///
/// Each issue takes its status from the latest build, except that an
/// issue a build found already closed and an earlier build merged stays
/// merged.
pub fn fold(builds: &[BuildState]) -> Option<RunState> {
    let first = builds.first()?;
    let latest = builds.last()?;
    let mut issues: BTreeMap<u64, IssueState> = BTreeMap::new();
    for build in builds {
        for now in &build.issues {
            let issue = issues
                .entry(now.issue.number)
                .or_insert_with(|| IssueState {
                    duration_ms: None,
                    cost_usd: None,
                    sessions: Vec::new(),
                    ..now.clone()
                });
            issue.issue = now.issue.clone();
            // Only a merge is worth keeping over a later build's "closed":
            // an issue that failed and was then closed by hand is closed.
            let merged_before = issue.status == IssueStatus::Merged;
            if now.status != IssueStatus::Closed || !merged_before {
                issue.status = now.status;
                issue.reason = now.reason.clone();
                issue.pull_request = now.pull_request;
                issue.merge_commit = now.merge_commit.clone();
                issue.started_at = now.started_at.or(issue.started_at);
            }
            issue.duration_ms = add(issue.duration_ms, now.duration_ms);
            issue.cost_usd = add(issue.cost_usd, now.cost_usd);
            issue.sessions.extend(now.sessions.iter().cloned());
        }
    }
    let mut order: Vec<u64> = latest.issues.iter().map(|m| m.issue.number).collect();
    for build in builds.iter().rev() {
        for issue in &build.issues {
            if !order.contains(&issue.issue.number) {
                order.push(issue.issue.number);
            }
        }
    }
    let (disposition, reason) = match latest.status {
        BuildStatus::Running => (Disposition::Building, None),
        BuildStatus::Succeeded if !latest.pull_request_closed => {
            (Disposition::WaitingForReview, None)
        }
        BuildStatus::Succeeded => (
            Disposition::NeedsAttention,
            Some(format!(
                "PR #{} was closed without merging",
                latest.pull_request.unwrap_or_default()
            )),
        ),
        BuildStatus::Failed | BuildStatus::Interrupted => {
            (Disposition::NeedsAttention, latest.reason.clone())
        }
    };
    Some(RunState {
        run: latest.run(),
        repo: latest.repo.clone(),
        number: latest.number,
        title: latest.title.clone(),
        kind: latest.kind,
        branch: latest.branch.clone(),
        disposition,
        reason,
        pull_request: latest.pull_request,
        issues: order
            .into_iter()
            .filter_map(|number| issues.remove(&number))
            .collect(),
        sessions: builds
            .iter()
            .flat_map(|build| build.sessions.iter().cloned())
            .collect(),
        builds: builds
            .iter()
            .map(|build| BuildSummary {
                build_id: build.build_id.clone(),
                status: build.status,
                reason: build.reason.clone(),
                pull_request: build.pull_request,
                started_at: build.started_at,
                ended_at: build.ended_at,
                elapsed_ms: build.elapsed_ms,
                cost_usd: build.cost_usd,
            })
            .collect(),
        running_build: (latest.status == BuildStatus::Running).then(|| latest.build_id.clone()),
        started_at: first.started_at,
        latest_at: builds.iter().map(|build| build.latest_at).max()?,
        elapsed_ms: builds.iter().map(|build| build.elapsed_ms).sum(),
        cost_usd: builds.iter().map(|build| build.cost_usd).sum(),
    })
}

/// `a + b`, where a missing amount counts as nothing unless both are.
fn add<T: std::ops::Add<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    }
}

/// The run `run`'s state, with what each running agent is doing, or
/// [`io::ErrorKind::NotFound`] if it has no builds.
pub fn read(root: &Path, run: &RunKey) -> io::Result<RunState> {
    fold(&record::list_builds(root, run)?)
        .map(|state| with_activity(root, state))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no run {run}")))
}

/// `state` with each running issue's [`Activity`], read from its latest
/// session's log in the running build, or from `<issue>.jsonl` in a record
/// from before sessions. A log that can't be read gives none.
fn with_activity(root: &Path, mut state: RunState) -> RunState {
    let Some(build_id) = state.running_build.clone() else {
        return state;
    };
    let dir = record::build_dir(root, &state.run, &build_id);
    for issue in &mut state.issues {
        if issue.status == IssueStatus::Running {
            issue.activity = activity::read(&dir.join(issue.log(&build_id))).ok();
        }
    }
    state
}

/// Every run with a record under `root`, by the directories under
/// `<root>/runs`.
pub fn keys(root: &Path) -> io::Result<Vec<RunKey>> {
    let mut keys = Vec::new();
    for owner in subdirectories(&record::runs_dir(root))? {
        for name in subdirectories(&owner)? {
            for number in subdirectories(&name)? {
                let key = format!(
                    "{}/{}#{}",
                    file_name(&owner),
                    file_name(&name),
                    file_name(&number)
                );
                if let Ok(key) = key.parse() {
                    keys.push(key);
                }
            }
        }
    }
    Ok(keys)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The directories in `dir`, or none if it doesn't exist.
fn subdirectories(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

/// Every run under `root`, newest activity first. A run none of whose
/// builds can be read is skipped.
pub fn list(root: &Path) -> io::Result<Vec<RunState>> {
    let mut runs = Vec::new();
    for key in keys(root)? {
        if let Some(run) = fold(&record::list_builds(root, &key)?) {
            runs.push(with_activity(root, run));
        }
    }
    runs.sort_by(|a, b| {
        b.latest_at
            .cmp(&a.latest_at)
            .then_with(|| a.run.cmp(&b.run))
    });
    Ok(runs)
}

/// Removes everything of `run` under `root`: every build's clone, with its
/// worktrees, and every build's record. The caller makes sure none of its
/// builds is running.
pub fn delete(root: &Path, run: &RunKey) -> io::Result<()> {
    let clones = run.under(cleanup::builds_dir(root));
    for clone in subdirectories(&clones)? {
        cleanup::remove_clone(&clone)?;
    }
    for dir in [clones, run_dir(root, run)] {
        match fs::remove_dir_all(&dir) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        prune(&dir, root);
    }
    Ok(())
}

/// Removes the empty directories above `dir`, up to but not including
/// `<root>/runs` or `<root>/builds`.
fn prune(dir: &Path, root: &Path) {
    let stops = [record::runs_dir(root), cleanup::builds_dir(root)];
    let mut dir = dir.parent();
    while let Some(parent) = dir {
        if stops.iter().any(|stop| stop == parent) || fs::remove_dir(parent).is_err() {
            break;
        }
        dir = parent.parent();
    }
}

/// Records every build that is still running as interrupted. Called when
/// the app starts, before any build can run, so a running build is one an
/// app that stopped was running.
pub fn interrupt_unfinished(root: &Path) -> io::Result<()> {
    for key in keys(root)? {
        for build in record::list_builds(root, &key)? {
            if build.status == BuildStatus::Running {
                let mut writer = BuildWriter::open(root, &key, &build.build_id)?;
                let now = Utc::now();
                let ran =
                    |at: DateTime<Utc>| u64::try_from((now - at).num_milliseconds()).unwrap_or(0);
                // Their agents stopped with the app, and what they spent
                // isn't known.
                let sessions = build
                    .issues
                    .iter()
                    .map(|issue| (issue.issue.number, &issue.sessions))
                    .chain([(build.number, &build.sessions)]);
                for (number, sessions) in sessions {
                    for session in sessions.iter().filter(|s| s.duration_ms.is_none()) {
                        writer.append(&Event::SessionFinished {
                            at: now,
                            number,
                            role: session.role,
                            round: session.round,
                            duration_ms: ran(session.started_at),
                            cost_usd: None,
                        })?;
                    }
                }
                for issue in &build.issues {
                    if issue.status == IssueStatus::Running {
                        // A record from before sessions keeps the agent's
                        // time on the issue.
                        let duration_ms = issue
                            .sessions
                            .is_empty()
                            .then(|| issue.started_at.map_or(0, ran));
                        writer.append(&Event::IssueFinished {
                            at: now,
                            number: issue.issue.number,
                            outcome: crate::record::Outcome::Failed,
                            reason: Some("interrupted".to_owned()),
                            pull_request: None,
                            duration_ms,
                            cost_usd: None,
                        })?;
                    }
                }
                writer.append(&Event::BuildInterrupted {
                    at: now,
                    reason: "the Epik app stopped before the build ended".to_owned(),
                })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Issue, Outcome, SessionRole};
    use chrono::TimeZone;

    fn time(seconds: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 28, 17, 0, 0).unwrap() + chrono::Duration::seconds(seconds)
    }

    fn key() -> RunKey {
        RunKey::new("wpm/epik-test", 13)
    }

    fn dollars(text: &str) -> Decimal {
        Decimal::from_str(text).unwrap()
    }

    /// The events of a build of epic 13, whose issues are 14 and 15 (which
    /// needs 14), started at `start` seconds, with `closed` already closed.
    fn started(build_id: &str, start: i64, closed: &[u64]) -> Event {
        Event::BuildStarted {
            at: time(start),
            build_id: build_id.to_owned(),
            repo: "wpm/epik-test".to_owned(),
            number: 13,
            title: "Exponentiation".to_owned(),
            kind: Kind::Epic,
            branch: "epic/13".to_owned(),
            issues: vec![
                Issue {
                    number: 14,
                    title: "Parse ^".to_owned(),
                    blocked_by: vec![],
                },
                Issue {
                    number: 15,
                    title: "Precedence".to_owned(),
                    blocked_by: vec![14],
                },
            ],
            closed: closed.to_vec(),
            roles: Default::default(),
        }
    }

    fn issue_started(at: i64, number: u64) -> Event {
        Event::IssueStarted {
            at: time(at),
            number,
        }
    }

    fn issue_finished(at: i64, number: u64, outcome: Outcome, cost: &str) -> Event {
        Event::IssueFinished {
            at: time(at),
            number,
            outcome,
            reason: None,
            pull_request: Some(number + 100),
            duration_ms: Some(60_000),
            cost_usd: Some(dollars(cost)),
        }
    }

    fn session_started(at: i64, number: u64, role: SessionRole, round: u32, log: &str) -> Event {
        Event::SessionStarted {
            at: time(at),
            number,
            role,
            round,
            agent: "claude".to_owned(),
            model: None,
            log: log.to_owned(),
        }
    }

    fn session_finished(at: i64, number: u64, role: SessionRole, round: u32, cost: &str) -> Event {
        Event::SessionFinished {
            at: time(at),
            number,
            role,
            round,
            duration_ms: 10_000,
            cost_usd: Some(dollars(cost)),
        }
    }

    /// Build 2 of [`finished`], with 15 coded in two sessions and the
    /// epic's pull request reviewed in one, still running.
    fn with_sessions() -> Vec<Event> {
        let finished = Event::IssueFinished {
            at: time(1060),
            number: 15,
            outcome: Outcome::Succeeded,
            reason: None,
            pull_request: Some(115),
            duration_ms: None,
            cost_usd: None,
        };
        vec![
            started("20261028T171640Z-bbbb", 1000, &[14]),
            issue_started(1001, 15),
            session_started(1001, 15, SessionRole::Coding, 1, "15.jsonl"),
            session_finished(1030, 15, SessionRole::Coding, 1, "0.50"),
            session_started(1031, 15, SessionRole::Coding, 2, "15-coding-2.jsonl"),
            session_finished(1059, 15, SessionRole::Coding, 2, "0.25"),
            finished,
            merged(1061, 15),
            session_started(1062, 13, SessionRole::Review, 1, "13-review-1.jsonl"),
        ]
    }

    fn merged(at: i64, number: u64) -> Event {
        Event::IssueMerged {
            at: time(at),
            number,
            pull_request: number + 100,
            merge_commit: None,
        }
    }

    /// Build 1: 14 merges, then the app quits while 15 runs.
    fn interrupted() -> Vec<Event> {
        vec![
            started("20261028T170000Z-aaaa", 0, &[]),
            issue_started(1, 14),
            issue_finished(60, 14, Outcome::Succeeded, "0.90"),
            merged(61, 14),
            issue_started(62, 15),
            Event::BuildInterrupted {
                at: time(100),
                reason: "the app quit".to_owned(),
            },
        ]
    }

    /// Build 2, started at 1000 seconds: 15 merges and the epic's pull
    /// request opens.
    fn finished() -> Vec<Event> {
        vec![
            started("20261028T171640Z-bbbb", 1000, &[14]),
            issue_started(1001, 15),
            issue_finished(1060, 15, Outcome::Succeeded, "1.10"),
            merged(1061, 15),
            Event::BuildFinished {
                at: time(1070),
                outcome: Outcome::Succeeded,
                reason: None,
                pull_request: Some(200),
            },
        ]
    }

    fn write(root: &Path, events: &[Event]) {
        let Event::BuildStarted { build_id, .. } = &events[0] else {
            unreachable!()
        };
        let mut writer = BuildWriter::create(root, &key(), build_id).unwrap();
        for event in events {
            writer.append(event).unwrap();
        }
    }

    fn build(events: &[Event]) -> BuildState {
        record::fold(events).unwrap()
    }

    #[test]
    fn a_key_is_written_owner_name_number() {
        assert_eq!(key().to_string(), "wpm/epik-test#13");
        assert_eq!("wpm/epik-test#13".parse::<RunKey>(), Ok(key()));
        assert_eq!(
            serde_json::to_value(key()).unwrap(),
            serde_json::json!("wpm/epik-test#13")
        );
        let read: RunKey = serde_json::from_str(r#""wpm/epik-test#13""#).unwrap();
        assert_eq!(read, key());
    }

    #[test]
    fn a_key_ignores_case_as_github_does() {
        assert_eq!(RunKey::new("WPM/Epik-Test", 13), key());
        assert_eq!("WPM/Epik-Test#13".parse::<RunKey>(), Ok(key()));
    }

    #[test]
    fn a_key_that_could_leave_its_directory_is_refused() {
        for text in [
            "wpm/epik-test",
            "wpm#13",
            "../x#1",
            "wpm/..#1",
            "a/b/c#1",
            "wpm/epik-test#x",
            "/b#1",
            "a\\b/c#1",
        ] {
            assert!(text.parse::<RunKey>().is_err(), "{text}");
        }
    }

    #[test]
    fn a_runs_record_and_clones_are_keyed_by_its_epic_issue() {
        let root = Path::new("/home/me/.epik");
        assert_eq!(
            run_dir(root, &key()),
            Path::new("/home/me/.epik/runs/wpm/epik-test/13")
        );
        assert_eq!(
            clone_dir(root, &key(), "b"),
            Path::new("/home/me/.epik/builds/wpm/epik-test/13/b")
        );
    }

    #[test]
    fn a_run_of_one_build_is_that_build() {
        let only = build(&finished());

        let run = fold(std::slice::from_ref(&only)).unwrap();

        assert_eq!(run.run, key());
        assert_eq!(run.disposition, Disposition::WaitingForReview);
        assert_eq!(run.pull_request, Some(200));
        assert_eq!(run.builds.len(), 1);
        assert_eq!(run.elapsed_ms, only.elapsed_ms);
        assert_eq!(run.cost_usd, dollars("1.10"));
        assert_eq!(run.running_build, None);
        assert_eq!(run.issue(14).unwrap().status, IssueStatus::Closed);
    }

    #[test]
    fn a_run_of_several_builds_sums_their_time_and_cost() {
        let builds = [build(&interrupted()), build(&finished())];

        let run = fold(&builds).unwrap();

        assert_eq!(run.elapsed_ms, 100_000 + 70_000);
        assert_eq!(run.cost_usd, dollars("2.00"));
        assert_eq!(run.started_at, time(0));
        assert_eq!(run.latest_at, time(1070));
        let ids: Vec<&str> = run.builds.iter().map(|b| b.build_id.as_str()).collect();
        assert_eq!(ids, ["20261028T170000Z-aaaa", "20261028T171640Z-bbbb"]);
        assert_eq!(run.builds[0].status, BuildStatus::Interrupted);
        assert_eq!(run.disposition, Disposition::WaitingForReview);
    }

    #[test]
    fn an_issue_merged_in_an_earlier_build_stays_merged() {
        let builds = [build(&interrupted()), build(&finished())];

        let run = fold(&builds).unwrap();

        let earlier = run.issue(14).unwrap();
        assert_eq!(earlier.status, IssueStatus::Merged);
        assert_eq!(earlier.pull_request, Some(114));
        assert_eq!(earlier.cost_usd, Some(dollars("0.90")));
        assert_eq!(earlier.duration_ms, Some(60_000));
        let later = run.issue(15).unwrap();
        assert_eq!(later.status, IssueStatus::Merged);
        let numbers: Vec<u64> = run.issues.iter().map(|m| m.issue.number).collect();
        assert_eq!(numbers, [14, 15]);
    }

    #[test]
    fn an_issue_tried_in_two_builds_adds_up_both() {
        let mut first = interrupted();
        first.truncate(5);
        first.push(issue_finished(90, 15, Outcome::Failed, "0.50"));
        first.push(Event::BuildFinished {
            at: time(100),
            outcome: Outcome::Failed,
            reason: Some("failed: #15".to_owned()),
            pull_request: None,
        });
        let builds = [build(&first), build(&finished())];

        let run = fold(&builds).unwrap();

        let issue = run.issue(15).unwrap();
        assert_eq!(issue.status, IssueStatus::Merged);
        assert_eq!(issue.cost_usd, Some(dollars("1.60")));
        assert_eq!(issue.duration_ms, Some(120_000));
    }

    #[test]
    fn an_issue_tried_in_two_builds_keeps_every_session_and_adds_them_up() {
        let builds = [build(&interrupted()), build(&with_sessions())];

        let run = fold(&builds).unwrap();

        let issue = run.issue(15).unwrap();
        let logs: Vec<(&str, &str)> = issue
            .sessions
            .iter()
            .map(|s| (s.build_id.as_str(), s.log.as_str()))
            .collect();
        assert_eq!(
            logs,
            [
                ("20261028T171640Z-bbbb", "15.jsonl"),
                ("20261028T171640Z-bbbb", "15-coding-2.jsonl")
            ]
        );
        assert_eq!(issue.duration_ms, Some(20_000));
        assert_eq!(issue.cost_usd, Some(dollars("0.75")));
        let reviews: Vec<&str> = run.sessions.iter().map(|s| s.log.as_str()).collect();
        assert_eq!(reviews, ["13-review-1.jsonl"]);
        assert_eq!(run.cost_usd, dollars("0.90") + dollars("0.75"));
    }

    #[test]
    fn an_issue_closed_by_hand_after_failing_is_closed() {
        let mut first = interrupted();
        first.truncate(5);
        first.push(issue_finished(90, 15, Outcome::Failed, "0.50"));
        first.push(Event::BuildFinished {
            at: time(100),
            outcome: Outcome::Failed,
            reason: Some("failed: #15".to_owned()),
            pull_request: None,
        });
        let mut second = finished();
        second.truncate(1);
        let Event::BuildStarted { closed, .. } = &mut second[0] else {
            unreachable!()
        };
        closed.push(15);
        let builds = [build(&first), build(&second)];

        let run = fold(&builds).unwrap();

        assert_eq!(run.issue(15).unwrap().status, IssueStatus::Closed);
        assert_eq!(run.issue(15).unwrap().cost_usd, Some(dollars("0.50")));
        assert_eq!(run.issue(14).unwrap().status, IssueStatus::Merged);
    }

    #[test]
    fn a_running_build_makes_the_run_building() {
        let builds = [build(&interrupted()), build(&finished()[..2])];

        let run = fold(&builds).unwrap();

        assert_eq!(run.disposition, Disposition::Building);
        assert_eq!(run.running_build.as_deref(), Some("20261028T171640Z-bbbb"));
        assert_eq!(run.issue(15).unwrap().status, IssueStatus::Running);
        assert_eq!(run.elapsed(time(1031)), chrono::Duration::seconds(100 + 31));
    }

    #[test]
    fn a_failed_or_interrupted_run_needs_attention_and_says_why() {
        let run = fold(&[build(&interrupted())]).unwrap();

        assert_eq!(run.disposition, Disposition::NeedsAttention);
        assert_eq!(run.reason.as_deref(), Some("the app quit"));
    }

    #[test]
    fn a_pull_request_closed_without_merging_needs_attention() {
        let mut events = finished();
        events.push(Event::PullRequestClosed {
            at: time(2000),
            pull_request: 200,
        });

        let run = fold(&[build(&events)]).unwrap();

        assert_eq!(run.disposition, Disposition::NeedsAttention);
        assert_eq!(
            run.reason.as_deref(),
            Some("PR #200 was closed without merging")
        );
    }

    #[test]
    fn no_builds_is_no_run() {
        assert_eq!(fold(&[]), None);
    }

    #[test]
    fn runs_are_read_and_listed_newest_activity_first() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &interrupted());
        write(root.path(), &finished());
        let other = RunKey::new("wpm/other", 2);
        let mut writer = BuildWriter::create(root.path(), &other, "b").unwrap();
        let Event::BuildStarted { at, .. } = started("b", 500, &[]) else {
            unreachable!()
        };
        writer
            .append(&Event::BuildStarted {
                at,
                build_id: "b".to_owned(),
                repo: "wpm/other".to_owned(),
                number: 2,
                title: "Other".to_owned(),
                kind: Kind::Issue,
                branch: "main".to_owned(),
                issues: vec![],
                closed: vec![],
                roles: Default::default(),
            })
            .unwrap();
        std::fs::create_dir_all(run_dir(root.path(), &RunKey::new("wpm/empty", 1))).unwrap();

        let runs = list(root.path()).unwrap();

        let keys: Vec<String> = runs.iter().map(|run| run.run.to_string()).collect();
        assert_eq!(keys, ["wpm/epik-test#13", "wpm/other#2"]);
        assert_eq!(runs[0].builds.len(), 2);
        assert_eq!(read(root.path(), &key()).unwrap(), runs[0]);
        let missing = read(root.path(), &RunKey::new("wpm/empty", 1)).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_running_issue_reports_what_its_agent_is_doing() {
        let root = tempfile::tempdir().unwrap();
        let running = &finished()[..2];
        write(root.path(), running);
        let log = format!("{}/src/fixtures/agent.jsonl", env!("CARGO_MANIFEST_DIR"));
        let dir = record::build_dir(root.path(), &key(), "20261028T171640Z-bbbb");
        fs::copy(log, dir.join("15.jsonl")).unwrap();

        let run = read(root.path(), &key()).unwrap();

        let activity = run.issue(15).unwrap().activity.as_ref().unwrap();
        assert_eq!(activity.last_action.as_deref(), Some("Running cargo build"));
        assert_eq!(activity.last_test.as_deref(), Some("25 passed, 1 failed"));
        assert!(activity.last_event.is_some());
        assert_eq!(run.issue(14).unwrap().activity, None);
        assert_eq!(list(root.path()).unwrap()[0], run);
    }

    #[test]
    fn a_running_issue_reports_what_its_latest_session_is_doing() {
        let root = tempfile::tempdir().unwrap();
        let mut running = with_sessions();
        running.truncate(5);
        write(root.path(), &running);
        let log = format!("{}/src/fixtures/agent.jsonl", env!("CARGO_MANIFEST_DIR"));
        let dir = record::build_dir(root.path(), &key(), "20261028T171640Z-bbbb");
        fs::write(dir.join("15.jsonl"), "").unwrap();
        fs::copy(log, dir.join("15-coding-2.jsonl")).unwrap();

        let run = read(root.path(), &key()).unwrap();

        let activity = run.issue(15).unwrap().activity.as_ref().unwrap();
        assert_eq!(activity.last_action.as_deref(), Some("Running cargo build"));
    }

    #[test]
    fn deleting_a_run_leaves_nothing_of_it() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &interrupted());
        write(root.path(), &finished());
        for build in ["20261028T170000Z-aaaa", "20261028T171640Z-bbbb"] {
            crate::cleanup::tests::repository(&clone_dir(root.path(), &key(), build));
        }
        let other = RunKey::new("wpm/epik-test", 99);
        BuildWriter::create(root.path(), &other, "b").unwrap();
        fs::create_dir_all(clone_dir(root.path(), &other, "b")).unwrap();

        delete(root.path(), &key()).unwrap();

        assert!(!run_dir(root.path(), &key()).exists());
        assert!(!key().under(cleanup::builds_dir(root.path())).exists());
        assert!(run_dir(root.path(), &other).exists(), "other runs stay");
        assert!(clone_dir(root.path(), &other, "b").exists());
        assert_eq!(keys(root.path()).unwrap(), std::slice::from_ref(&other));

        delete(root.path(), &other).unwrap();
        let runs = fs::read_dir(record::runs_dir(root.path())).unwrap();
        assert_eq!(runs.count(), 0, "empty parents are pruned");
        delete(root.path(), &other).unwrap();
    }

    #[test]
    fn a_mirrors_running_build_is_not_interrupted() {
        let root = tempfile::tempdir().unwrap();
        let mirror = crate::hosts::dir(root.path(), "devbox");
        write(&mirror, &interrupted()[..5]);
        let before = record::list_builds(&mirror, &key()).unwrap();

        interrupt_unfinished(root.path()).unwrap();

        assert_eq!(record::list_builds(&mirror, &key()).unwrap(), before);
        assert_eq!(
            read(&mirror, &key()).unwrap().disposition,
            Disposition::Building
        );
    }

    #[test]
    fn unfinished_builds_are_recorded_as_interrupted() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &interrupted());
        write(root.path(), &finished()[..2]);

        interrupt_unfinished(root.path()).unwrap();

        let run = read(root.path(), &key()).unwrap();
        assert_eq!(run.disposition, Disposition::NeedsAttention);
        assert_eq!(
            run.reason.as_deref(),
            Some("the Epik app stopped before the build ended")
        );
        let issue = run.issue(15).unwrap();
        assert_eq!(
            issue.status,
            IssueStatus::Failed,
            "its agent stopped with the app"
        );
        assert_eq!(issue.reason.as_deref(), Some("interrupted"));
        assert_eq!(run.builds[0].reason.as_deref(), Some("the app quit"));
    }

    #[test]
    fn an_interrupted_session_keeps_the_time_it_ran() {
        let root = tempfile::tempdir().unwrap();
        let mut running = with_sessions();
        running.truncate(5);
        write(root.path(), &running);

        interrupt_unfinished(root.path()).unwrap();

        let issue = read(root.path(), &key())
            .unwrap()
            .issue(15)
            .unwrap()
            .clone();
        assert_eq!(issue.status, IssueStatus::Failed);
        let [first, second] = &issue.sessions[..] else {
            panic!("two sessions: {:?}", issue.sessions);
        };
        assert_eq!(first.duration_ms, Some(10_000));
        let ran = second.duration_ms.expect("finished when interrupted");
        assert!(ran > 0, "it ran from its start to the interruption");
        assert_eq!(second.cost_usd, None);
        assert_eq!(issue.duration_ms, Some(10_000 + ran));
        assert_eq!(issue.cost_usd, Some(dollars("0.50")));
    }
}
