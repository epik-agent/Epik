//! A build, from start to finish: clone the repository, run each issue's
//! agent in dependency order, merge each issue's pull request into the epic
//! branch, and open the epic's pull request, recording the run as it goes.
//!
//! [`start`] reads the issue, clones the repository and writes
//! `run_started`; [`Build::run`] does the rest. [`run`] does both. A build
//! reaches GitHub only through [`GitHub`] and its agents only through
//! [`Agent`], so it is tested with fakes of each.
//!
//! An agent's work ends at an open pull request. For an epic, the build
//! waits for that pull request's checks, merges it into the epic branch and
//! closes the issue, which GitHub does only for merges into the default
//! branch. For a single issue, the pull request targets the default branch
//! and is left for the user to merge.

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use rust_decimal::Decimal;

use crate::agent::{Agent, AgentOutcome, AgentRequest};
use crate::github::{self, Checks, GitHub, PullRequest, State};
use crate::graph::Graph;
use crate::record::{self, Event, Kind, Member, Outcome, RunState, RunWriter};

/// How often a pull request's checks are read while they are pending.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// How often a waiting build looks at the cancel signal.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// What to build.
#[derive(Clone, Debug)]
pub struct Request {
    /// The repository, `owner/name`.
    pub repo: String,
    /// The epic or issue to build.
    pub number: u64,
    /// The branch the issues' pull requests merge into. By default
    /// `epic/<number>-<slug of title>` for an epic, and the repository's
    /// default branch for a single issue.
    pub branch: Option<String>,
    /// Epik's root, normally `~/.epik`. Clones go under `builds`, records
    /// under `runs`.
    pub root: PathBuf,
    /// Epik's Claude Code plugin, which each agent loads.
    pub plugin_dir: PathBuf,
}

/// Why a build couldn't start.
#[derive(Debug)]
pub enum Error {
    /// The number is a pull request's.
    PullRequest,
    /// The issue is already closed.
    Closed,
    GitHub(github::Error),
    /// The run's record couldn't be created.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::PullRequest => f.write_str("it is a pull request, not an issue"),
            Error::Closed => f.write_str("it is closed"),
            Error::GitHub(error) => error.fmt(f),
            Error::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<github::Error> for Error {
    fn from(error: github::Error) -> Self {
        Error::GitHub(error)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Io(error)
    }
}

/// `title` made fit for a branch name, as `slug` in
/// `plugin/server/src/tools/build_start.js` does it: lowercased, each run
/// of anything but ASCII letters and digits made one `-`, cut to 40
/// characters, and trimmed of `-`.
pub fn slug(title: &str) -> String {
    let mut slug = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(40);
    slug.trim_matches('-').to_owned()
}

/// The default branch for epic `number` titled `title`:
/// `epic/<number>-<slug>`, or `epic/<number>` when the title has nothing to
/// slug.
pub fn epic_branch(number: u64, title: &str) -> String {
    match slug(title) {
        tail if tail.is_empty() => format!("epic/{number}"),
        tail => format!("epic/{number}-{tail}"),
    }
}

/// Starts a build and runs it to its end. See [`start`] and [`Build::run`].
pub fn run(
    github: &dyn GitHub,
    agent: &(dyn Agent + Sync),
    request: &Request,
    cancel: &AtomicBool,
) -> Result<RunState, Error> {
    Ok(start(github, agent, request)?.run(cancel))
}

/// Starts a build: reads the issue, picks the branch, for an epic ensures
/// its branch and label and reads its members and their blockers, and
/// clones the repository into `<root>/builds/<run-id>`. Then writes
/// `run_started`, whose members are the open issues the run builds.
///
/// A pull request, a closed issue, or a failure before `run_started` is an
/// error, and leaves no record.
pub fn start<'a>(
    github: &'a dyn GitHub,
    agent: &'a (dyn Agent + Sync),
    request: &Request,
) -> Result<Build<'a>, Error> {
    let repo = request.repo.as_str();
    let number = request.number;
    let issue = github.issue(repo, number)?;
    if issue.is_pull_request {
        return Err(Error::PullRequest);
    }
    if issue.state == State::Closed {
        return Err(Error::Closed);
    }
    let subs = github.sub_issues(repo, number)?;
    let kind = if subs.is_empty() {
        Kind::Issue
    } else {
        Kind::Epic
    };
    let default_branch = github.default_branch(repo)?;
    let branch = match (&request.branch, kind) {
        (Some(branch), _) => branch.clone(),
        (None, Kind::Epic) => epic_branch(number, &issue.title),
        (None, Kind::Issue) => default_branch.clone(),
    };

    let at = Utc::now();
    let mut problems = Vec::new();
    let members = match kind {
        Kind::Issue => vec![Member {
            number,
            title: issue.title.clone(),
            blocked_by: Vec::new(),
        }],
        Kind::Epic => {
            github.ensure_branch(repo, &branch)?;
            github.ensure_epic_label(repo, number)?;
            let (here, elsewhere): (Vec<_>, Vec<_>) = subs.iter().partition(|sub| sub.repo == repo);
            if !elsewhere.is_empty() {
                let elsewhere: Vec<String> = elsewhere
                    .iter()
                    .map(|sub| format!("{}#{}", sub.repo, sub.number))
                    .collect();
                problems.push(format!(
                    "members in another repository: {}",
                    elsewhere.join(", ")
                ));
            }
            let open: Vec<_> = here
                .into_iter()
                .filter(|sub| sub.state == State::Open)
                .collect();
            let numbers: BTreeSet<u64> = open.iter().map(|sub| sub.number).collect();
            open.into_iter()
                .map(|sub| {
                    let blockers = github.blocked_by(repo, sub.number)?;
                    Ok(Member {
                        number: sub.number,
                        title: sub.title.clone(),
                        blocked_by: blockers
                            .into_iter()
                            .filter(|b| numbers.contains(b))
                            .collect(),
                    })
                })
                .collect::<github::Result<_>>()?
        }
    };
    let graph = Graph::new(
        members
            .iter()
            .map(|member| (member.number, member.blocked_by.clone())),
    );
    for cycle in graph.cycles() {
        problems.push(format!("a dependency cycle: {}", issues(&cycle)));
    }

    // Creating the record first reserves the run's id, so no other run
    // shares its record or its clone.
    let writer = RunWriter::create_new(&request.root, repo, number, at)?;
    let run_id = writer.run_id().to_owned();
    let dir = writer.dir().to_owned();
    let clone = request.root.join("builds").join(&run_id);
    if let Err(error) = github.clone(repo, &clone) {
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
        return Err(error.into());
    }
    let mut log = Log {
        writer,
        events: Vec::new(),
    };
    log.append(Event::RunStarted {
        at,
        run_id: run_id.clone(),
        repo: repo.to_owned(),
        number,
        title: issue.title.clone(),
        kind,
        branch: branch.clone(),
        members,
    });
    Ok(Build {
        github,
        agent,
        repo: repo.to_owned(),
        number,
        title: issue.title,
        kind,
        branch,
        default_branch,
        run_id,
        clone,
        dir,
        plugin_dir: request.plugin_dir.clone(),
        graph,
        problems,
        check_interval: CHECK_INTERVAL,
        log: Mutex::new(log),
    })
}

/// A build that has started: its run is recorded and its repository cloned.
pub struct Build<'a> {
    github: &'a dyn GitHub,
    agent: &'a (dyn Agent + Sync),
    repo: String,
    number: u64,
    title: String,
    kind: Kind,
    branch: String,
    default_branch: String,
    run_id: String,
    clone: PathBuf,
    /// The run's record directory.
    dir: PathBuf,
    plugin_dir: PathBuf,
    /// The open members and the members blocking each.
    graph: Graph,
    /// What stops the run before any agent starts.
    problems: Vec<String>,
    check_interval: Duration,
    log: Mutex<Log>,
}

/// How one issue's turn ended.
enum Finished {
    /// Closed, or for a single issue, at an open pull request: this one.
    Done(u64),
    Failed,
    /// Stopped by the cancel signal, without writing anything more.
    Cancelled,
}

impl Build<'_> {
    /// The run's id.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Whether the run builds an epic or a single issue.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The branch the run's pull requests merge into.
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// The run's record directory, `<root>/runs/<run-id>`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The repository's clone, `<root>/builds/<run-id>`.
    pub fn clone_dir(&self) -> &Path {
        &self.clone
    }

    /// How often pending checks are read; [`CHECK_INTERVAL`] by default.
    pub fn check_interval(mut self, interval: Duration) -> Self {
        self.check_interval = interval;
        self
    }

    /// Runs the build to its end and returns its final state, which is
    /// also written to `summary.json`.
    ///
    /// Every ready issue's agent starts at once, each on its own thread,
    /// with the prompt `/epik:issue <number> <branch>`. As each finishes,
    /// its issue is finished, and for an epic merged and closed, and the
    /// issues it unblocks start. When nothing is running or ready, an epic
    /// whose members are all closed gets its pull request into the default
    /// branch. Setting `cancel` stops every agent and ends the run with
    /// `run_interrupted`.
    pub fn run(self, cancel: &AtomicBool) -> RunState {
        if self.problems.is_empty() {
            self.orchestrate(cancel);
        } else {
            self.finish(Err(self.problems.join("; ")));
        }
        let log = self.log.into_inner().unwrap();
        let state = record::fold(&log.events).expect("the run started");
        // The events are the record; a summary that can't be written is
        // rebuilt from them by anything that reads the run.
        let _ = log.writer.write_summary(&state);
        state
    }

    fn orchestrate(&self, cancel: &AtomicBool) {
        let mut closed = BTreeSet::new();
        let mut running = BTreeSet::new();
        let mut failed = BTreeSet::new();
        let mut last_pull_request = None;
        let (done, finished) = mpsc::channel();
        thread::scope(|scope| {
            loop {
                if !cancel.load(Ordering::SeqCst) {
                    for number in self.graph.ready(&closed, &running, &failed) {
                        running.insert(number);
                        self.append(Event::IssueStarted {
                            at: Utc::now(),
                            number,
                        });
                        let done = done.clone();
                        scope.spawn(move || {
                            let _ = done.send((number, self.build_issue(number, cancel)));
                        });
                    }
                }
                if running.is_empty() {
                    break;
                }
                let Ok((number, how)) = finished.recv_timeout(CANCEL_POLL) else {
                    continue;
                };
                running.remove(&number);
                match how {
                    Finished::Done(pr) => {
                        closed.insert(number);
                        last_pull_request = Some(pr);
                    }
                    Finished::Failed => {
                        failed.insert(number);
                    }
                    Finished::Cancelled => {}
                }
            }
        });

        if cancel.load(Ordering::SeqCst) {
            self.append(Event::RunInterrupted {
                at: Utc::now(),
                reason: "cancelled".to_owned(),
            });
            return;
        }
        let stuck = self.graph.stuck(&closed, &failed);
        if !failed.is_empty() || !stuck.is_empty() {
            let mut reason = format!("failed: {}", issues(&failed));
            if !stuck.is_empty() {
                reason.push_str(&format!("; stuck: {}", issues(&stuck)));
            }
            self.finish(Err(reason));
            return;
        }
        match self.kind {
            // The single issue's own pull request.
            Kind::Issue => self.finish(Ok(last_pull_request)),
            Kind::Epic => {
                let body = format!("Closes #{}", self.number);
                let opened = self.github.open_pull_request(
                    &self.repo,
                    &self.branch,
                    &self.default_branch,
                    &self.title,
                    &body,
                );
                self.finish(
                    opened
                        .map(Some)
                        .map_err(|error| format!("couldn't open the epic's pull request: {error}")),
                );
            }
        }
    }

    /// Runs issue `number`'s agent, then finds its pull request, and for an
    /// epic waits for its checks, merges it, and closes the issue.
    fn build_issue(&self, number: u64, cancel: &AtomicBool) -> Finished {
        let dir = &self.dir;
        let request = AgentRequest {
            prompt: format!("/epik:issue {number} {}", self.branch),
            cwd: self.clone.clone(),
            plugin_dir: self.plugin_dir.clone(),
            jsonl_path: dir.join(format!("{number}.jsonl")),
            err_path: dir.join(format!("{number}.err")),
        };
        let outcome = self.agent.run(&request, cancel);
        if cancel.load(Ordering::SeqCst) {
            return Finished::Cancelled;
        }
        let found = if outcome.exited_cleanly {
            match self
                .github
                .find_pull_request(&self.repo, &self.branch, number)
            {
                Ok(Some(pr)) => Ok(pr),
                Ok(None) => Err(format!(
                    "no pull request into {} closes #{number}",
                    self.branch
                )),
                Err(error) => Err(format!("couldn't find its pull request: {error}")),
            }
        } else {
            Err(format!(
                "the agent stopped ({}): {}",
                outcome.stop_reason,
                outcome.error.as_deref().unwrap_or("no error given")
            ))
        };
        let pr = match found {
            Ok(pr) => pr,
            Err(reason) => {
                self.issue_finished(number, &outcome, Outcome::Failed, Some(reason), None);
                return Finished::Failed;
            }
        };
        self.issue_finished(number, &outcome, Outcome::Succeeded, None, Some(pr.number));
        if self.kind == Kind::Issue {
            return Finished::Done(pr.number);
        }
        match self.merge(number, pr, cancel) {
            Ok(()) => {
                self.append(Event::IssueMerged {
                    at: Utc::now(),
                    number,
                    pull_request: pr.number,
                });
                Finished::Done(pr.number)
            }
            Err(Stop::Cancelled) => Finished::Cancelled,
            Err(Stop::Failed(reason)) => {
                // A second `issue_finished` records that the issue failed
                // after its agent succeeded.
                self.issue_finished(
                    number,
                    &outcome,
                    Outcome::Failed,
                    Some(reason),
                    Some(pr.number),
                );
                Finished::Failed
            }
        }
    }

    /// Waits for `pr`'s checks, merges it, and closes issue `number`.
    fn merge(&self, number: u64, pr: PullRequest, cancel: &AtomicBool) -> Result<(), Stop> {
        if !pr.merged {
            self.await_checks(pr.number, cancel)?;
            self.github
                .merge_pull_request(&self.repo, pr.number)
                .map_err(|error| Stop::Failed(format!("couldn't merge #{}: {error}", pr.number)))?;
        }
        let comment = format!("Merged into `{}` by #{}.", self.branch, pr.number);
        self.github
            .close_issue(&self.repo, number, &comment)
            .map_err(|error| {
                Stop::Failed(format!(
                    "merged #{} but couldn't close the issue: {error}",
                    pr.number
                ))
            })
    }

    /// Reads `pr`'s checks until none is pending. No checks count as
    /// passing.
    fn await_checks(&self, pr: u64, cancel: &AtomicBool) -> Result<(), Stop> {
        loop {
            match self.github.checks(&self.repo, pr) {
                Ok(Checks::Passing | Checks::None) => return Ok(()),
                Ok(Checks::Pending) => {}
                Ok(Checks::Failing) => {
                    return Err(Stop::Failed(format!("checks failed on #{pr}")));
                }
                Err(error) => {
                    return Err(Stop::Failed(format!(
                        "couldn't read the checks on #{pr}: {error}"
                    )));
                }
            }
            if !wait(self.check_interval, cancel) {
                return Err(Stop::Cancelled);
            }
        }
    }

    fn issue_finished(
        &self,
        number: u64,
        agent: &AgentOutcome,
        outcome: Outcome,
        reason: Option<String>,
        pull_request: Option<u64>,
    ) {
        self.append(Event::IssueFinished {
            at: Utc::now(),
            number,
            outcome,
            reason,
            pull_request,
            duration_ms: agent.duration_ms,
            cost_usd: agent
                .cost_usd
                .and_then(|cost| Decimal::from_str(&cost.to_string()).ok()),
        });
    }

    /// Ends the run: with its pull request if it succeeded, else with the
    /// reason it failed.
    fn finish(&self, result: Result<Option<u64>, String>) {
        let (outcome, reason, pull_request) = match result {
            Ok(pull_request) => (Outcome::Succeeded, None, pull_request),
            Err(reason) => (Outcome::Failed, Some(reason), None),
        };
        self.append(Event::RunFinished {
            at: Utc::now(),
            outcome,
            reason,
            pull_request,
        });
    }

    fn append(&self, event: Event) {
        self.log.lock().unwrap().append(event);
    }
}

/// Why an epic's issue stopped short of being merged.
enum Stop {
    Cancelled,
    Failed(String),
}

/// A run's events, as written and as kept for the final fold.
struct Log {
    writer: RunWriter,
    events: Vec<Event>,
}

impl Log {
    fn append(&mut self, event: Event) {
        // A record that can't be written mustn't stop the agents.
        let _ = self.writer.append(&event);
        self.events.push(event);
    }
}

/// Sleeps for `interval`, or less if `cancel` is set. Whether it slept
/// uncancelled.
fn wait(interval: Duration, cancel: &AtomicBool) -> bool {
    let until = Instant::now() + interval;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        thread::sleep(left.min(CANCEL_POLL));
    }
}

/// `#1, #2, #3`.
fn issues<'a>(numbers: impl IntoIterator<Item = &'a u64>) -> String {
    numbers
        .into_iter()
        .map(|number| format!("#{number}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{FakeGitHub, MergeMethods, PullState};
    use crate::record::{MemberStatus, RunStatus};
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicUsize;

    const REPO: &str = "owner/repo";
    const EPIC_BRANCH: &str = "epic/1-the-epic";

    /// What the fake agent does for an issue.
    #[derive(Clone, Copy)]
    enum Act {
        /// Opens a pull request closing the issue, with these checks.
        Pull(Checks),
        /// Ends cleanly without a pull request.
        NoPull,
        /// Fails, as an agent that couldn't start does.
        Crash,
        /// Sets the cancel signal, as the user would mid-run.
        Cancel,
        /// Runs until cancelled.
        Hang,
    }

    /// An [`Agent`] that acts on a [`FakeGitHub`] as `/epik:issue` would.
    struct FakeAgent<'a> {
        github: &'a FakeGitHub,
        acts: BTreeMap<u64, Act>,
        requests: Mutex<Vec<AgentRequest>>,
        running: AtomicUsize,
        most_running: AtomicUsize,
    }

    impl<'a> FakeAgent<'a> {
        fn new<const N: usize>(github: &'a FakeGitHub, acts: [(u64, Act); N]) -> Self {
            Self {
                github,
                acts: BTreeMap::from(acts),
                requests: Mutex::new(Vec::new()),
                running: AtomicUsize::new(0),
                most_running: AtomicUsize::new(0),
            }
        }

        fn prompts(&self) -> Vec<String> {
            let requests = self.requests.lock().unwrap();
            requests.iter().map(|r| r.prompt.clone()).collect()
        }

        fn act(&self, prompt: &str, cancel: &AtomicBool) -> Option<String> {
            let words: Vec<&str> = prompt.split(' ').collect();
            let [command, number, branch] = words[..] else {
                panic!("unexpected prompt {prompt}");
            };
            assert_eq!(command, "/epik:issue");
            let number: u64 = number.parse().unwrap();
            match self
                .acts
                .get(&number)
                .copied()
                .unwrap_or(Act::Pull(Checks::Passing))
            {
                Act::Pull(checks) => {
                    let head = format!("issue-{number}");
                    self.github.add_branch(REPO, &head);
                    let title = format!("Issue {number}");
                    let body = format!("Closes #{number}");
                    let pr = self
                        .github
                        .open_pull_request(REPO, &head, branch, &title, &body)
                        .unwrap();
                    self.github.set_checks(REPO, pr, checks);
                    None
                }
                Act::NoPull => None,
                Act::Crash => Some("agent_failed".into()),
                Act::Cancel => {
                    cancel.store(true, Ordering::SeqCst);
                    Some("cancelled".into())
                }
                Act::Hang => {
                    while !cancel.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Some("cancelled".into())
                }
            }
        }
    }

    impl Agent for FakeAgent<'_> {
        fn run(&self, request: &AgentRequest, cancel: &AtomicBool) -> AgentOutcome {
            self.requests.lock().unwrap().push(request.clone());
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_running.fetch_max(now, Ordering::SeqCst);
            // Long enough for issues started together to overlap.
            thread::sleep(Duration::from_millis(50));
            let stopped = self.act(&request.prompt, cancel);
            self.running.fetch_sub(1, Ordering::SeqCst);
            AgentOutcome {
                exited_cleanly: stopped.is_none(),
                error: stopped.as_ref().map(|_| "it broke".to_owned()),
                stop_reason: stopped.unwrap_or_else(|| "end_turn".into()),
                duration_ms: 1_000,
                cost_usd: Some(0.25),
            }
        }
    }

    /// A repository on `main` holding epic 1, "The epic", whose sub-issues
    /// are `members`, each with its blockers.
    fn epic<const N: usize>(members: [(u64, &[u64]); N]) -> FakeGitHub {
        let github = FakeGitHub::new();
        github.add_repo(REPO, "main");
        github.add_issue(REPO, 1, "The epic");
        for (number, blockers) in members {
            github.add_issue(REPO, number, &format!("Issue {number}"));
            github.add_sub_issue(REPO, 1, REPO, number);
            for &blocker in blockers {
                github.add_blocker(REPO, number, blocker);
            }
        }
        github
    }

    fn request(root: &Path, number: u64) -> Request {
        Request {
            repo: REPO.to_owned(),
            number,
            branch: None,
            root: root.to_owned(),
            plugin_dir: PathBuf::from("/plugin"),
        }
    }

    /// Builds `number` to its end, checking pending checks every
    /// millisecond.
    fn build(github: &FakeGitHub, agent: &FakeAgent, root: &Path, number: u64) -> RunState {
        let cancel = AtomicBool::new(false);
        start(github, agent, &request(root, number))
            .unwrap()
            .check_interval(Duration::from_millis(1))
            .run(&cancel)
    }

    /// The pull request from `issue-<number>`.
    fn pr_of(github: &FakeGitHub, number: u64) -> u64 {
        let head = format!("issue-{number}");
        let pulls = github.repo(REPO).pulls;
        let found = pulls.iter().find(|(_, pull)| pull.head == head);
        *found.expect("the issue has a pull request").0
    }

    /// The run's events on disk, each written briefly, without its time.
    fn events(root: &Path, state: &RunState) -> Vec<String> {
        let events = record::read_events(root, &state.run_id).unwrap();
        events.iter().map(brief).collect()
    }

    fn brief(event: &Event) -> String {
        let pr = |pr: &Option<u64>| pr.map_or(String::new(), |pr| format!(" #{pr}"));
        let why = |reason: &Option<String>| match reason {
            Some(reason) => format!(" ({reason})"),
            None => String::new(),
        };
        match event {
            Event::RunStarted {
                kind,
                branch,
                members,
                ..
            } => {
                let members: Vec<String> = members
                    .iter()
                    .map(|m| format!("{}{:?}", m.number, m.blocked_by))
                    .collect();
                format!("run_started {kind:?} {branch} {}", members.join(" "))
            }
            Event::IssueStarted { number, .. } => format!("issue_started {number}"),
            Event::IssueFinished {
                number,
                outcome,
                reason,
                pull_request,
                ..
            } => format!(
                "issue_finished {number} {outcome:?}{}{}",
                pr(pull_request),
                why(reason)
            ),
            Event::IssueMerged {
                number,
                pull_request,
                ..
            } => format!("issue_merged {number} #{pull_request}"),
            Event::RunFinished {
                outcome,
                reason,
                pull_request,
                ..
            } => format!(
                "run_finished {outcome:?}{}{}",
                pr(pull_request),
                why(reason)
            ),
            Event::RunInterrupted { reason, .. } => format!("run_interrupted ({reason})"),
        }
    }

    fn sorted(mut events: Vec<String>) -> Vec<String> {
        events.sort();
        events
    }

    #[test]
    fn slug_follows_build_start() {
        assert_eq!(slug("The Epik app runs builds"), "the-epik-app-runs-builds");
        assert_eq!(slug("  Hello, World!  "), "hello-world");
        assert_eq!(slug("Café au lait"), "caf-au-lait");
        assert_eq!(slug("!!!"), "");
        let long = "a".repeat(39) + " b";
        assert_eq!(slug(&long), "a".repeat(39));
    }

    #[test]
    fn epic_branch_is_named_for_the_epic() {
        assert_eq!(epic_branch(12, "Run builds"), "epic/12-run-builds");
        assert_eq!(epic_branch(12, "???"), "epic/12");
    }

    #[test]
    fn an_epic_with_a_chain_and_a_parallel_pair_merges_in_order() {
        // 2 and 3 run together; 4 needs both; 5 needs 4.
        let github = epic([(2, &[]), (3, &[]), (4, &[2, 3]), (5, &[4])]);
        let agent = FakeAgent::new(&github, [(4, Act::Pull(Checks::None))]);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);

        let pr = |n| pr_of(&github, n);
        let epic_pr = state.pull_request.expect("the epic's pull request");
        let events = events(root.path(), &state);
        assert_eq!(
            events[..3],
            [
                format!("run_started Epic {EPIC_BRANCH} 2[] 3[] 4[2, 3] 5[4]"),
                "issue_started 2".to_owned(),
                "issue_started 3".to_owned(),
            ]
        );
        assert_eq!(
            sorted(events[3..7].to_vec()),
            [
                format!("issue_finished 2 Succeeded #{}", pr(2)),
                format!("issue_finished 3 Succeeded #{}", pr(3)),
                format!("issue_merged 2 #{}", pr(2)),
                format!("issue_merged 3 #{}", pr(3)),
            ]
        );
        assert_eq!(
            events[7..],
            [
                "issue_started 4".to_owned(),
                format!("issue_finished 4 Succeeded #{}", pr(4)),
                format!("issue_merged 4 #{}", pr(4)),
                "issue_started 5".to_owned(),
                format!("issue_finished 5 Succeeded #{}", pr(5)),
                format!("issue_merged 5 #{}", pr(5)),
                format!("run_finished Succeeded #{epic_pr}"),
            ]
        );
        assert_eq!(agent.most_running.load(Ordering::SeqCst), 2);

        let repo = github.repo(REPO);
        assert!(repo.branches.contains(EPIC_BRANCH));
        assert!(repo.issues[&1].labels.contains("epic"));
        for n in 2..=5 {
            assert_eq!(repo.pulls[&pr(n)].state, PullState::Merged);
            assert_eq!(repo.pulls[&pr(n)].base, EPIC_BRANCH);
            let issue = &repo.issues[&n];
            assert_eq!(issue.state, State::Closed);
            let comment = format!("Merged into `{EPIC_BRANCH}` by #{}.", pr(n));
            assert_eq!(issue.comments, [comment]);
        }
        let epic_pull = &repo.pulls[&epic_pr];
        assert_eq!(epic_pull.head, EPIC_BRANCH);
        assert_eq!(epic_pull.base, "main");
        assert_eq!(epic_pull.title, "The epic");
        assert!(epic_pull.body.contains("Closes #1"));
        assert_eq!(epic_pull.state, PullState::Open);
        assert_eq!(repo.issues[&1].state, State::Open);

        assert_eq!(state.status, RunStatus::Succeeded);
        assert_eq!(state.cost_usd, Decimal::from_str("1.00").unwrap());
    }

    #[test]
    fn a_run_clones_the_repository_and_keeps_each_agents_logs() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);

        let clone = root.path().join("builds").join(&state.run_id);
        assert_eq!(github.clones(), [(REPO.to_owned(), clone.clone())]);
        let requests = agent.requests.lock().unwrap();
        let [request] = &requests[..] else {
            panic!("one agent ran");
        };
        let dir = record::run_dir(root.path(), &state.run_id);
        assert_eq!(request.prompt, format!("/epik:issue 2 {EPIC_BRANCH}"));
        assert_eq!(request.cwd, clone);
        assert_eq!(request.plugin_dir, Path::new("/plugin"));
        assert_eq!(request.jsonl_path, dir.join("2.jsonl"));
        assert_eq!(request.err_path, dir.join("2.err"));

        let summary = std::fs::read_to_string(dir.join(record::SUMMARY_FILE)).unwrap();
        let summary: RunState = serde_json::from_str(&summary).unwrap();
        assert_eq!(summary, state);
    }

    #[test]
    fn a_given_branch_is_used() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let mut request = request(root.path(), 1);
        request.branch = Some("daemon".to_owned());
        let state = run(&github, &agent, &request, &cancel).unwrap();
        assert_eq!(state.branch, "daemon");
        assert_eq!(agent.prompts(), ["/epik:issue 2 daemon"]);
        assert_eq!(github.repo(REPO).pulls[&pr_of(&github, 2)].base, "daemon");
    }

    #[test]
    fn a_failed_issue_strands_what_it_blocks() {
        // 2 fails, so 3 and 4 behind it never run; 5 is independent.
        let github = epic([(2, &[]), (3, &[2]), (4, &[3]), (5, &[])]);
        let agent = FakeAgent::new(&github, [(2, Act::Crash)]);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);

        let events = events(root.path(), &state);
        assert_eq!(
            events[..3],
            [
                format!("run_started Epic {EPIC_BRANCH} 2[] 3[2] 4[3] 5[]"),
                "issue_started 2".to_owned(),
                "issue_started 5".to_owned(),
            ]
        );
        assert_eq!(
            sorted(events[3..6].to_vec()),
            [
                "issue_finished 2 Failed (the agent stopped (agent_failed): it broke)".to_owned(),
                format!("issue_finished 5 Succeeded #{}", pr_of(&github, 5)),
                format!("issue_merged 5 #{}", pr_of(&github, 5)),
            ]
        );
        assert_eq!(
            events[6..],
            ["run_finished Failed (failed: #2; stuck: #3, #4)"]
        );
        assert_eq!(agent.prompts().len(), 2);
        let repo = github.repo(REPO);
        assert_eq!(repo.issues[&2].state, State::Open);
        assert_eq!(repo.issues[&5].state, State::Closed);
        assert!(repo.pulls.values().all(|pull| pull.base != "main"));
        assert_eq!(state.member(3).unwrap().status, MemberStatus::Waiting);
    }

    #[test]
    fn an_issue_without_a_pull_request_fails() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, [(2, Act::NoPull)]);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        assert_eq!(
            events(root.path(), &state)[1..],
            [
                "issue_started 2".to_owned(),
                format!("issue_finished 2 Failed (no pull request into {EPIC_BRANCH} closes #2)"),
                "run_finished Failed (failed: #2)".to_owned(),
            ]
        );
    }

    #[test]
    fn failing_checks_fail_the_issue_and_leave_it_open() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, [(2, Act::Pull(Checks::Failing))]);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);

        let pr = pr_of(&github, 2);
        assert_eq!(
            events(root.path(), &state)[1..],
            [
                "issue_started 2".to_owned(),
                format!("issue_finished 2 Succeeded #{pr}"),
                format!("issue_finished 2 Failed #{pr} (checks failed on #{pr})"),
                "run_finished Failed (failed: #2)".to_owned(),
            ]
        );
        let repo = github.repo(REPO);
        assert_eq!(repo.pulls[&pr].state, PullState::Open);
        assert_eq!(repo.issues[&2].state, State::Open);
        let member = state.member(2).unwrap();
        assert_eq!(member.status, MemberStatus::Failed);
        assert_eq!(member.pull_request, Some(pr));
    }

    #[test]
    fn a_failed_merge_fails_the_issue() {
        let github = epic([(2, &[])]);
        github.fail("merge_pull_request", "gh: conflict");
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        let pr = pr_of(&github, 2);
        assert_eq!(
            events(root.path(), &state)[3],
            format!("issue_finished 2 Failed #{pr} (couldn't merge #{pr}: gh: conflict)")
        );
        assert_eq!(github.repo(REPO).issues[&2].state, State::Open);
    }

    #[test]
    fn an_issue_merges_with_a_method_the_repository_allows() {
        let github = epic([(2, &[])]);
        let squash_only = MergeMethods {
            squash: true,
            ..MergeMethods::NONE
        };
        github.set_merge_methods(REPO, squash_only);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        let pr = pr_of(&github, 2);
        assert_eq!(
            events(root.path(), &state)[3],
            format!("issue_merged 2 #{pr}")
        );
        let repo = github.repo(REPO);
        assert_eq!(repo.issues[&2].state, State::Closed);
    }

    #[test]
    fn a_cycle_fails_the_run_before_any_agent_starts() {
        let github = epic([(2, &[3]), (3, &[2]), (4, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        assert_eq!(
            events(root.path(), &state),
            [
                format!("run_started Epic {EPIC_BRANCH} 2[3] 3[2] 4[]"),
                "run_finished Failed (a dependency cycle: #2, #3)".to_owned(),
            ]
        );
        assert!(agent.prompts().is_empty());
    }

    #[test]
    fn a_member_in_another_repository_fails_the_run() {
        let github = epic([(2, &[])]);
        github.add_repo("other/repo", "main");
        github.add_issue("other/repo", 9, "Elsewhere");
        github.add_sub_issue(REPO, 1, "other/repo", 9);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        assert_eq!(
            events(root.path(), &state)[1..],
            ["run_finished Failed (members in another repository: other/repo#9)"]
        );
        assert!(agent.prompts().is_empty());
    }

    #[test]
    fn closed_members_are_skipped() {
        // Building again after 2 merged.
        let github = epic([(2, &[]), (3, &[2])]);
        github.set_state(REPO, 2, State::Closed);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        let events = events(root.path(), &state);
        assert_eq!(events[0], format!("run_started Epic {EPIC_BRANCH} 3[]"));
        assert_eq!(events[1], "issue_started 3");
        assert_eq!(agent.prompts(), [format!("/epik:issue 3 {EPIC_BRANCH}")]);
        assert_eq!(state.status, RunStatus::Succeeded);
    }

    #[test]
    fn a_single_issue_stops_at_its_open_pull_request() {
        let github = FakeGitHub::new();
        github.add_repo(REPO, "main");
        github.add_issue(REPO, 7, "One thing");
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 7);

        let pr = pr_of(&github, 7);
        assert_eq!(
            events(root.path(), &state),
            [
                "run_started Issue main 7[]".to_owned(),
                "issue_started 7".to_owned(),
                format!("issue_finished 7 Succeeded #{pr}"),
                format!("run_finished Succeeded #{pr}"),
            ]
        );
        assert_eq!(agent.prompts(), ["/epik:issue 7 main"]);
        let repo = github.repo(REPO);
        assert_eq!(repo.pulls[&pr].state, PullState::Open);
        assert_eq!(repo.pulls[&pr].base, "main");
        assert_eq!(repo.issues[&7].state, State::Open);
        assert!(repo.issues[&7].labels.is_empty());
        assert_eq!(
            repo.branches,
            BTreeSet::from(["main".into(), "issue-7".into()])
        );
        assert_eq!(state.kind, Kind::Issue);
        assert_eq!(state.member(7).unwrap().status, MemberStatus::Succeeded);
    }

    #[test]
    fn cancelling_stops_every_agent_and_interrupts_the_run() {
        // 2 is cancelled by the user mid-run while 3 is still working.
        let github = epic([(2, &[]), (3, &[]), (4, &[2])]);
        let agent = FakeAgent::new(&github, [(2, Act::Cancel), (3, Act::Hang)]);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        assert_eq!(
            events(root.path(), &state)[1..],
            [
                "issue_started 2".to_owned(),
                "issue_started 3".to_owned(),
                "run_interrupted (cancelled)".to_owned(),
            ]
        );
        assert_eq!(agent.prompts().len(), 2);
        assert_eq!(state.status, RunStatus::Interrupted);
        assert!(github.repo(REPO).pulls.is_empty());
    }

    #[test]
    fn cancelling_while_checks_are_pending_interrupts_the_run() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, [(2, Act::Pull(Checks::Pending))]);
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let build = start(&github, &agent, &request(root.path(), 1))
            .unwrap()
            .check_interval(Duration::from_secs(3600));
        let state = thread::scope(|scope| {
            let running = scope.spawn(|| build.run(&cancel));
            while github.repo(REPO).pulls.is_empty() {
                thread::sleep(Duration::from_millis(5));
            }
            cancel.store(true, Ordering::SeqCst);
            running.join().unwrap()
        });
        let pr = pr_of(&github, 2);
        assert_eq!(
            events(root.path(), &state)[1..],
            [
                "issue_started 2".to_owned(),
                format!("issue_finished 2 Succeeded #{pr}"),
                "run_interrupted (cancelled)".to_owned(),
            ]
        );
        assert_eq!(github.repo(REPO).pulls[&pr].state, PullState::Open);
    }

    #[test]
    fn a_failed_clone_leaves_no_record() {
        let github = epic([(2, &[])]);
        github.fail("clone", "gh: no space left");
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let error = start(&github, &agent, &request(root.path(), 1)).err();
        assert!(matches!(error, Some(Error::GitHub(_))));
        let runs = std::fs::read_dir(record::runs_dir(root.path())).unwrap();
        assert_eq!(runs.count(), 0);
    }

    #[test]
    fn two_builds_of_one_issue_get_their_own_records_and_clones() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, [(2, Act::Hang)]);
        let root = tempfile::tempdir().unwrap();
        let first = start(&github, &agent, &request(root.path(), 1)).unwrap();
        let second = start(&github, &agent, &request(root.path(), 1)).unwrap();
        assert_ne!(first.run_id(), second.run_id());
        assert_ne!(first.dir(), second.dir());
        assert_ne!(first.clone_dir(), second.clone_dir());
    }

    #[test]
    fn a_pull_request_or_closed_issue_cannot_be_built() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        github.add_branch(REPO, "topic");
        let pr = github
            .open_pull_request(REPO, "topic", "main", "A change", "")
            .unwrap();
        let error = start(&github, &agent, &request(root.path(), pr)).err();
        assert!(matches!(error, Some(Error::PullRequest)));
        github.set_state(REPO, 2, State::Closed);
        let error = start(&github, &agent, &request(root.path(), 2)).err();
        assert!(matches!(error, Some(Error::Closed)));
        assert!(github.clones().is_empty());
        assert!(record::list_runs(root.path()).unwrap().is_empty());
    }
}
