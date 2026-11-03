//! A build, from start to finish: clone the repository, run each issue's
//! agent in dependency order, merge each issue's pull request into the epic
//! branch, and open the epic's pull request, recording the run as it goes.
//!
//! [`start`] reads the issue, clones the repository and writes
//! `build_started`; [`Build::run`] does the rest. [`run`] does both. A build
//! reaches GitHub only through [`GitHub`] and its agents only through
//! [`Agent`], so it is tested with fakes of each.
//!
//! An agent's work ends at an open pull request. For an epic, the build
//! merges it into the epic branch and closes the issue, which GitHub does
//! only for merges into the default branch. An epic merges one pull request
//! at a time: holding the epic's merge slot, an issue updates a pull request
//! that is behind and waits for its checks, so what merges has passed on the
//! base it merges into. A pull request that conflicts or fails goes back to
//! the coding agent, without the slot, up to [`RESOLVE_SESSIONS`] times. For
//! a single issue, the pull request targets the default branch and is left
//! for the user to merge.
//!
//! With review on, the run's pull request into the default branch, the
//! single issue's or the epic's, is reviewed once its checks pass, and the
//! build finishes with the review's verdict. A review asking for changes
//! has the coding agent address it on the pull request's branch, then the
//! checks and the review run again, up to [`REVIEW_ROUNDS`] reviews. An
//! epic's issues merge with no review between them.

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use rust_decimal::Decimal;

use crate::agent::{Agent, AgentOutcome, AgentRequest};
use crate::cleanup;
use crate::comment;
use crate::github::{
    self, BranchUpdate, Checks, GitHub, PullRequest, PullState, PullStatus, State,
};
use crate::graph::Graph;
use crate::prompt::{self, Work};
use crate::record::{
    self, BuildState, BuildWriter, Event, Issue, Kind, Outcome, ReviewVerdict, SessionRole,
};
use crate::review::{self, Verdict};
use crate::run::{self, RunKey};
use crate::settings::{Role, Settings};

/// How often a pull request's checks are read while they are pending.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// How many times the run's pull request is reviewed before a review that
/// still asks for changes leaves it to the user.
pub const REVIEW_ROUNDS: u32 = 2;

/// How many times a pull request that conflicts or fails its checks goes back
/// to the coding agent before its issue fails.
pub const RESOLVE_SESSIONS: u32 = 2;

/// How many times, a [`CHECK_INTERVAL`] apart, the build reads what GitHub
/// should soon settle before it gives up: whether a pull request can merge,
/// whether its update has landed, whether its new head has checks.
const SETTLE_READS: u32 = 20;

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
    /// The agent and model each role runs with, recorded in
    /// `build_started`. Each issue's agent runs the coding role's model,
    /// and the review, when it is on, the review role's.
    pub roles: Settings,
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

/// `title` made fit for a branch name: lowercased, each run
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
) -> Result<BuildState, Error> {
    Ok(start(github, agent, request)?.run(cancel))
}

/// Starts a build: reads the issue, picks the branch, for an epic ensures
/// its branch and label and reads its issues and their blockers, and
/// clones the repository into the build's clone directory. Then writes
/// `build_started`, with every issue of the run and which are closed; the
/// build works on the open ones.
///
/// A pull request, a closed issue, or a failure before `build_started` is an
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
    let mut closed = BTreeSet::new();
    let issues = match kind {
        Kind::Issue => vec![Issue {
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
                    "sub-issues in another repository: {}",
                    elsewhere.join(", ")
                ));
            }
            let numbers: BTreeSet<u64> = here.iter().map(|sub| sub.number).collect();
            closed.extend(
                here.iter()
                    .filter(|sub| sub.state == State::Closed)
                    .map(|sub| sub.number),
            );
            here.into_iter()
                .map(|sub| {
                    let blockers = github.blocked_by(repo, sub.number)?;
                    Ok(Issue {
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
    let numbers = issues.iter().map(|issue| issue.number).collect();
    let graph = Graph::new(
        issues
            .iter()
            .map(|issue| (issue.number, issue.blocked_by.clone())),
    );
    for cycle in graph.cycles() {
        problems.push(format!("a dependency cycle: {}", numbered(&cycle)));
    }

    // Creating the record first reserves the build's id, so no other build
    // shares its record or its clone.
    let key = RunKey::new(repo, number);
    let writer = BuildWriter::create_new(&request.root, &key, at)?;
    let build_id = writer.build_id().to_owned();
    let dir = writer.dir().to_owned();
    let clone = run::clone_dir(&request.root, &key, &build_id);
    if let Err(error) = github.clone(repo, &clone) {
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = cleanup::remove_clone(&clone);
        // The run's record, if this was its first build, is now empty.
        if record::list_builds(&request.root, &key).is_ok_and(|builds| builds.is_empty()) {
            let _ = run::delete(&request.root, &key);
        }
        return Err(error.into());
    }
    let mut log = Log {
        writer,
        events: Vec::new(),
    };
    log.append(Event::BuildStarted {
        at,
        build_id: build_id.clone(),
        repo: repo.to_owned(),
        number,
        title: issue.title.clone(),
        kind,
        branch: branch.clone(),
        issues,
        closed: closed.iter().copied().collect(),
        roles: request.roles.clone(),
    });
    Ok(Build {
        github,
        agent,
        reviewer: agent,
        root: request.root.clone(),
        key,
        repo: repo.to_owned(),
        number,
        title: issue.title,
        kind,
        branch,
        default_branch,
        build_id,
        clone,
        dir,
        issues: numbers,
        graph,
        closed,
        problems,
        roles: request.roles.clone(),
        check_interval: CHECK_INTERVAL,
        merge_slot: Mutex::new(()),
        log: Mutex::new(log),
    })
}

/// A build that has started: its run is recorded and its repository cloned.
pub struct Build<'a> {
    github: &'a dyn GitHub,
    agent: &'a (dyn Agent + Sync),
    /// The agent that reviews the run's pull request, when review is on.
    reviewer: &'a (dyn Agent + Sync),
    /// Epik's root, holding the run's earlier builds.
    root: PathBuf,
    key: RunKey,
    repo: String,
    number: u64,
    title: String,
    kind: Kind,
    branch: String,
    default_branch: String,
    build_id: String,
    clone: PathBuf,
    /// The run's record directory.
    dir: PathBuf,
    /// The run's issues, closed or not.
    issues: Vec<u64>,
    /// The run's issues and the issues blocking each.
    graph: Graph,
    /// The issues closed when the build started.
    closed: BTreeSet<u64>,
    /// What stops the run before any agent starts.
    problems: Vec<String>,
    roles: Settings,
    check_interval: Duration,
    /// Held by the one issue merging its pull request.
    merge_slot: Mutex<()>,
    log: Mutex<Log>,
}

/// How one try at merging a pull request ended.
enum MergeAttempt {
    /// It merged, with this merge commit.
    Merged(String),
    /// It needs the coding agent, for this reason.
    HandBack(String),
}

/// How one issue's turn ended.
enum Finished {
    /// Closed, or for a single issue, at an open pull request: this one.
    Done(u64),
    Failed,
    /// Stopped by the cancel signal, without writing anything more.
    Cancelled,
}

impl<'a> Build<'a> {
    /// The run the build belongs to.
    pub fn run_key(&self) -> &RunKey {
        &self.key
    }

    /// The build's id, unique within its run.
    pub fn build_id(&self) -> &str {
        &self.build_id
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

    /// The agent that reviews the run's pull request when review is on;
    /// by default the agent that codes.
    pub fn reviewer(mut self, reviewer: &'a (dyn Agent + Sync)) -> Self {
        self.reviewer = reviewer;
        self
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
    /// with a prompt saying what the work is, Epik's rules and the procedure
    /// for implementing an issue. As each finishes,
    /// its issue is finished, and for an epic merged and closed, and the
    /// issues it unblocks start. When nothing is running or ready, an epic
    /// whose issues are all closed gets its pull request into the default
    /// branch. With review on, that pull request, or a single issue's, is
    /// reviewed once its checks pass. Setting `cancel` stops every agent
    /// and ends the run with `build_interrupted`.
    pub fn run(self, cancel: &AtomicBool) -> BuildState {
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
        // The clone is scratch space: what the run made is on GitHub or in
        // its record. Failing to remove it doesn't change how the run went.
        if let Err(error) = cleanup::remove_clone(&self.clone) {
            eprintln!(
                "epik: couldn't remove {}'s clone at {}: {error}",
                self.build_id,
                self.clone.display()
            );
        }
        state
    }

    fn orchestrate(&self, cancel: &AtomicBool) {
        let mut closed = self.closed.clone();
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
            self.interrupt();
            return;
        }
        let stuck = self.graph.stuck(&closed, &failed);
        if !failed.is_empty() || !stuck.is_empty() {
            let mut reason = format!("failed: {}", numbered(&failed));
            if !stuck.is_empty() {
                reason.push_str(&format!("; stuck: {}", numbered(&stuck)));
            }
            self.finish(Err(reason));
            return;
        }
        let pull_request = match self.kind {
            // The single issue's own pull request.
            Kind::Issue => last_pull_request,
            Kind::Epic => {
                let body = format!("Closes #{}", self.number);
                let opened = self.github.open_pull_request(
                    &self.repo,
                    &self.branch,
                    &self.default_branch,
                    &self.title,
                    &body,
                );
                match opened {
                    Ok(pr) => Some(pr),
                    Err(error) => {
                        self.finish(Err(format!(
                            "couldn't open the epic's pull request: {error}"
                        )));
                        return;
                    }
                }
            }
        };
        match (pull_request, &self.roles.review) {
            (Some(pr), Some(role)) => self.review(pr, role, cancel),
            _ => self.finish(Ok(pull_request)),
        }
    }

    /// Waits for the run's pull request `pr`'s checks, reviews it with
    /// `role`'s agent, and finishes the build with the review's verdict and
    /// the threads left open. When the review asks for changes, the coding
    /// agent addresses it on the pull request's branch and the checks and
    /// the review run again, up to [`REVIEW_ROUNDS`] reviews; changes asked
    /// for after the last leave the run needing attention. Failing checks
    /// fail the build.
    fn review(&self, pr: u64, role: &Role, cancel: &AtomicBool) {
        let number = self.number;
        let sub_issues = match self.kind {
            Kind::Epic => self.issues.clone(),
            Kind::Issue => Vec::new(),
        };
        let base = match self.kind {
            Kind::Epic => &self.default_branch,
            Kind::Issue => &self.branch,
        };
        let open_threads = || self.github.open_review_threads(&self.repo, pr).ok();
        // The verdict the pull request stands on, once a review has run.
        let mut review = None;
        let mut round = 1;
        loop {
            match self.await_checks(pr, cancel) {
                Ok(()) => {}
                Err(Stop::Cancelled) => return self.interrupt(),
                Err(Stop::Failed(reason)) => {
                    return self.end(Outcome::Failed, Some(reason), Some(pr), review, None);
                }
            }
            let request = AgentRequest {
                prompt: prompt::review_prompt(&self.repo, number, &sub_issues, pr, round),
                issue: number,
                branch: base.clone(),
                cwd: self.clone.clone(),
                jsonl_path: self.dir.join(format!("{number}-review-{round}.jsonl")),
                err_path: self.dir.join(format!("{number}-review-{round}.err")),
                model: role.model.clone(),
            };
            self.session_started(role, SessionRole::Review, round, &request);
            let outcome = self.reviewer.run(&request, cancel);
            if cancel.load(Ordering::SeqCst) {
                self.session_finished(SessionRole::Review, round, &request, &outcome, None);
                return self.interrupt();
            }
            // The verdict is GitHub's, whatever the agent said or however it
            // stopped: a review that can't be read is no verdict.
            let verdict = self
                .github
                .reviews(&self.repo, pr)
                .ok()
                .and_then(|reviews| review::verdict(&reviews, round));
            review = Some(match verdict {
                Some(Verdict::Approve) => ReviewVerdict::Approve,
                Some(Verdict::Changes) => ReviewVerdict::Changes,
                None => ReviewVerdict::NoVerdict,
            });
            self.session_finished(SessionRole::Review, round, &request, &outcome, review);
            if review != Some(ReviewVerdict::Changes) {
                return self.end(Outcome::Succeeded, None, Some(pr), review, open_threads());
            }
            if round == REVIEW_ROUNDS {
                let reason = format!(
                    "the review still asks for changes after {REVIEW_ROUNDS} rounds; see #{pr}"
                );
                return self.end(
                    Outcome::Failed,
                    Some(reason),
                    Some(pr),
                    review,
                    open_threads(),
                );
            }
            round += 1;
            if let Err(stop) = self.address(pr, round, base, cancel) {
                return match stop {
                    Stop::Cancelled => self.interrupt(),
                    Stop::Failed(reason) => self.end(
                        Outcome::Failed,
                        Some(reason),
                        Some(pr),
                        review,
                        open_threads(),
                    ),
                };
            }
        }
    }

    /// Runs the coding agent on the run's pull request `pr` into `base` as
    /// session `round`, to address the review on the pull request's branch.
    fn address(&self, pr: u64, round: u32, base: &str, cancel: &AtomicBool) -> Result<(), Stop> {
        let head = self.read_pull(pr)?.head;
        let work = Work::Address {
            issue: self.number,
            epic: self.kind == Kind::Epic,
            pull_request: pr,
            head: &head,
        };
        self.coding_session(
            self.number,
            round,
            work,
            base,
            "addressing the review",
            cancel,
        )
    }

    /// Runs the coding agent on `pr`, issue `number`'s pull request into the
    /// run's branch, as session `round`, to resolve what keeps it from
    /// merging. It holds no merge slot.
    fn resolve(&self, number: u64, pr: u64, round: u32, cancel: &AtomicBool) -> Result<(), Stop> {
        let head = self.read_pull(pr)?.head;
        let work = Work::Resolve {
            issue: number,
            pull_request: pr,
            head: &head,
        };
        let doing = format!("resolving #{pr}");
        self.coding_session(number, round, work, &self.branch, &doing, cancel)
    }

    /// Runs the coding agent on `work` for `number` as session `round`, on a
    /// pull request into `base`. `doing` says what for, if it stops.
    fn coding_session(
        &self,
        number: u64,
        round: u32,
        work: Work,
        base: &str,
        doing: &str,
        cancel: &AtomicBool,
    ) -> Result<(), Stop> {
        let request = AgentRequest {
            prompt: prompt::prompt(&self.repo, work, base),
            issue: number,
            branch: base.to_owned(),
            cwd: self.clone.clone(),
            jsonl_path: self.dir.join(format!("{number}-coding-{round}.jsonl")),
            err_path: self.dir.join(format!("{number}-coding-{round}.err")),
            model: self.roles.coding.model.clone(),
        };
        let coding = &self.roles.coding;
        let outcome = self.session(
            self.agent,
            coding,
            SessionRole::Coding,
            round,
            &request,
            cancel,
        );
        if cancel.load(Ordering::SeqCst) {
            return Err(Stop::Cancelled);
        }
        if !outcome.exited_cleanly {
            return Err(Stop::Failed(format!(
                "the agent {doing} stopped ({}): {}",
                outcome.stop_reason,
                outcome.error.as_deref().unwrap_or("no error given")
            )));
        }
        Ok(())
    }

    /// Runs `agent` on `request` as session `round` of `role`, with the
    /// agent and model of `with`, recording when it starts and finishes.
    fn session(
        &self,
        agent: &(dyn Agent + Sync),
        with: &Role,
        role: SessionRole,
        round: u32,
        request: &AgentRequest,
        cancel: &AtomicBool,
    ) -> AgentOutcome {
        self.session_started(with, role, round, request);
        let outcome = agent.run(request, cancel);
        self.session_finished(role, round, request, &outcome, None);
        outcome
    }

    /// Records that session `round` of `role` has started on `request`,
    /// with the agent and model of `with`.
    fn session_started(&self, with: &Role, role: SessionRole, round: u32, request: &AgentRequest) {
        let log = request
            .jsonl_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.append(Event::SessionStarted {
            at: Utc::now(),
            number: request.issue,
            role,
            round,
            agent: with.agent.clone(),
            model: with.model.clone(),
            log,
        });
    }

    /// Records that session `round` of `role` on `request` has finished with
    /// `outcome`, and for a review, its `verdict`.
    fn session_finished(
        &self,
        role: SessionRole,
        round: u32,
        request: &AgentRequest,
        outcome: &AgentOutcome,
        verdict: Option<ReviewVerdict>,
    ) {
        self.append(Event::SessionFinished {
            at: Utc::now(),
            number: request.issue,
            role,
            round,
            duration_ms: outcome.duration_ms,
            cost_usd: outcome
                .cost_usd
                .and_then(|cost| Decimal::from_str(&cost.to_string()).ok()),
            verdict,
        });
    }

    /// Runs issue `number`'s agent, then finds its pull request, and for an
    /// epic merges it and closes the issue.
    fn build_issue(&self, number: u64, cancel: &AtomicBool) -> Finished {
        let dir = &self.dir;
        let request = AgentRequest {
            prompt: prompt::prompt(&self.repo, Work::Issue(number), &self.branch),
            issue: number,
            branch: self.branch.clone(),
            cwd: self.clone.clone(),
            jsonl_path: dir.join(format!("{number}.jsonl")),
            err_path: dir.join(format!("{number}.err")),
            model: self.roles.coding.model.clone(),
        };
        let coding = &self.roles.coding;
        let outcome = self.session(self.agent, coding, SessionRole::Coding, 1, &request, cancel);
        if cancel.load(Ordering::SeqCst) {
            // What the agent spent before it was stopped still counts: its
            // session has it.
            let reason = Some("interrupted".to_owned());
            self.issue_finished(number, Outcome::Failed, reason, None);
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
                self.issue_finished(number, Outcome::Failed, Some(reason), None);
                return Finished::Failed;
            }
        };
        self.issue_finished(number, Outcome::Succeeded, None, Some(pr.number));
        if self.kind == Kind::Issue {
            return Finished::Done(pr.number);
        }
        let closed = self.merge(number, pr, cancel).and_then(|sha| {
            self.append(Event::IssueMerged {
                at: Utc::now(),
                number,
                pull_request: pr.number,
                merge_commit: Some(sha),
            });
            self.close(number, pr.number)
        });
        match closed {
            Ok(()) => Finished::Done(pr.number),
            Err(Stop::Cancelled) => Finished::Cancelled,
            Err(Stop::Failed(reason)) => {
                // A second `issue_finished` records that the issue failed
                // after its agent succeeded.
                self.issue_finished(number, Outcome::Failed, Some(reason), Some(pr.number));
                Finished::Failed
            }
        }
    }

    /// Merges `pr`, issue `number`'s pull request, unless it has merged
    /// already, and returns its merge commit. Each try holds the merge slot;
    /// between tries the coding agent resolves what stopped it, up to
    /// [`RESOLVE_SESSIONS`] times.
    fn merge(&self, number: u64, pr: PullRequest, cancel: &AtomicBool) -> Result<String, Stop> {
        let pr = match pr {
            PullRequest {
                merged: true,
                number,
            } => return self.merge_now(number),
            PullRequest { number, .. } => number,
        };
        let mut had_checks = false;
        let mut sessions = 0;
        loop {
            let slot = self.take_merge_slot(cancel)?;
            let attempt = self.try_merge(pr, &mut had_checks, cancel)?;
            drop(slot);
            let stuck = match attempt {
                MergeAttempt::Merged(sha) => return Ok(sha),
                MergeAttempt::HandBack(stuck) => stuck,
            };
            if sessions == RESOLVE_SESSIONS {
                return Err(Stop::Failed(format!(
                    "{stuck} after {RESOLVE_SESSIONS} resolve sessions"
                )));
            }
            sessions += 1;
            // The issue's first coding session was round 1.
            self.resolve(number, pr, sessions + 1, cancel)?;
        }
    }

    /// Waits for the merge slot, until cancelled.
    fn take_merge_slot(&self, cancel: &AtomicBool) -> Result<MutexGuard<'_, ()>, Stop> {
        loop {
            match self.merge_slot.try_lock() {
                Ok(slot) => return Ok(slot),
                Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) => {}
            }
            if !wait(CANCEL_POLL, cancel) {
                return Err(Stop::Cancelled);
            }
        }
    }

    /// One try at merging `pr`, holding the merge slot: a conflict hands it
    /// back to the agent, a pull request behind is updated, and once its
    /// checks pass on its current head, it merges. `had_checks` remembers
    /// whether it has had checks, so that a new head's having none yet isn't
    /// passing. One no longer open has nothing to wait for.
    fn try_merge(
        &self,
        pr: u64,
        had_checks: &mut bool,
        cancel: &AtomicBool,
    ) -> Result<MergeAttempt, Stop> {
        let status = self.await_pull(
            pr,
            |status| status.state != PullState::Open || status.mergeable.is_some(),
            format!("GitHub never said whether #{pr} can merge"),
            cancel,
        )?;
        if status.state == PullState::Open {
            *had_checks |= self.read_checks(pr)? != Checks::None;
            let conflicts = || MergeAttempt::HandBack(format!("#{pr} still conflicts"));
            if status.mergeable == Some(false) {
                return Ok(conflicts());
            }
            let behind = self
                .github
                .behind_by(&self.repo, &self.branch, &status.head)
                .map_err(|error| Stop::Failed(format!("couldn't compare #{pr}: {error}")))?;
            if behind > 0 {
                let update = self
                    .github
                    .update_branch(&self.repo, pr)
                    .map_err(|error| Stop::Failed(format!("couldn't update #{pr}: {error}")))?;
                if update == BranchUpdate::Conflict {
                    return Ok(conflicts());
                }
                // Until the head moves, the checks are the old head's.
                self.await_pull(
                    pr,
                    |now| now.head_sha != status.head_sha,
                    format!("the update of #{pr}'s branch never landed"),
                    cancel,
                )?;
            }
            if !self.checks_pass(pr, *had_checks, cancel)? {
                let failing = format!("checks still fail on #{pr}");
                return Ok(MergeAttempt::HandBack(failing));
            }
        }
        self.merge_now(pr).map(MergeAttempt::Merged)
    }

    /// Merges `pr` and returns its merge commit.
    fn merge_now(&self, pr: u64) -> Result<String, Stop> {
        self.github
            .merge_pull_request(&self.repo, pr)
            .map_err(|error| Stop::Failed(format!("couldn't merge #{pr}: {error}")))
    }

    fn read_pull(&self, pr: u64) -> Result<PullStatus, Stop> {
        self.github
            .pull_request(&self.repo, pr)
            .map_err(|error| Stop::Failed(format!("couldn't read #{pr}: {error}")))
    }

    /// Reads `pr` until `settled` says of it, failing with `never` after
    /// [`SETTLE_READS`] reads.
    fn await_pull(
        &self,
        pr: u64,
        settled: impl Fn(&PullStatus) -> bool,
        never: String,
        cancel: &AtomicBool,
    ) -> Result<PullStatus, Stop> {
        for _ in 0..SETTLE_READS {
            let status = self.read_pull(pr)?;
            if settled(&status) {
                return Ok(status);
            }
            if !wait(self.check_interval, cancel) {
                return Err(Stop::Cancelled);
            }
        }
        Err(Stop::Failed(never))
    }

    /// Closes issue `number`, merged by `pr`, with a comment of its time and
    /// cost over every build of the run.
    fn close(&self, number: u64, pr: u64) -> Result<(), Stop> {
        let issue = run::read(&self.root, &self.key)
            .ok()
            .and_then(|run| run.issue(number).cloned())
            .or_else(|| {
                let log = self.log.lock().unwrap();
                record::fold(&log.events).and_then(|build| build.issue(number).cloned())
            });
        let comment = match issue {
            Some(issue) => comment::issue_merged(&self.branch, &issue),
            None => format!("Merged into `{}` by #{pr}.", self.branch),
        };
        self.github
            .close_issue(&self.repo, number, &comment)
            .map_err(|error| {
                Stop::Failed(format!(
                    "merged #{pr} but couldn't close the issue: {error}"
                ))
            })
    }

    /// Reads `pr`'s checks until none is pending, and fails if they failed.
    /// No checks count as passing.
    fn await_checks(&self, pr: u64, cancel: &AtomicBool) -> Result<(), Stop> {
        if self.checks_pass(pr, false, cancel)? {
            Ok(())
        } else {
            Err(Stop::Failed(format!("checks failed on #{pr}")))
        }
    }

    /// Reads `pr`'s checks until none is pending, and says whether they
    /// passed. No checks count as passing, unless `expected`: then they are
    /// pending, for up to [`SETTLE_READS`] reads.
    fn checks_pass(&self, pr: u64, expected: bool, cancel: &AtomicBool) -> Result<bool, Stop> {
        let mut empty_reads = 0;
        loop {
            match self.read_checks(pr)? {
                Checks::Passing => return Ok(true),
                Checks::Failing => return Ok(false),
                Checks::None if !expected => return Ok(true),
                Checks::None => {
                    empty_reads += 1;
                    if empty_reads == SETTLE_READS {
                        return Err(Stop::Failed(format!(
                            "no checks appeared on #{pr}'s new head"
                        )));
                    }
                }
                Checks::Pending => {}
            }
            if !wait(self.check_interval, cancel) {
                return Err(Stop::Cancelled);
            }
        }
    }

    fn read_checks(&self, pr: u64) -> Result<Checks, Stop> {
        self.github
            .checks(&self.repo, pr)
            .map_err(|error| Stop::Failed(format!("couldn't read the checks on #{pr}: {error}")))
    }

    fn issue_finished(
        &self,
        number: u64,
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
            duration_ms: None,
            cost_usd: None,
        });
    }

    /// Ends the run unreviewed: with its pull request if it succeeded, else
    /// with the reason it failed.
    fn finish(&self, result: Result<Option<u64>, String>) {
        match result {
            Ok(pull_request) => self.end(Outcome::Succeeded, None, pull_request, None, None),
            Err(reason) => self.end(Outcome::Failed, Some(reason), None, None, None),
        }
    }

    fn end(
        &self,
        outcome: Outcome,
        reason: Option<String>,
        pull_request: Option<u64>,
        review: Option<ReviewVerdict>,
        open_threads: Option<usize>,
    ) {
        self.append(Event::BuildFinished {
            at: Utc::now(),
            outcome,
            reason,
            pull_request,
            review,
            open_threads,
        });
    }

    /// Ends the run as cancelled.
    fn interrupt(&self) {
        self.append(Event::BuildInterrupted {
            at: Utc::now(),
            reason: "cancelled".to_owned(),
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
    writer: BuildWriter,
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
fn numbered<'a>(numbers: impl IntoIterator<Item = &'a u64>) -> String {
    numbers
        .into_iter()
        .map(|number| format!("#{number}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{FakeGitHub, FakePull, MergeMethods, PullState};
    use crate::record::{BuildStatus, IssueStatus, ReviewVerdict};
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;

    const REPO: &str = "owner/repo";
    const EPIC_BRANCH: &str = "epic/1-the-epic";

    /// What the fake agent does for an issue.
    #[derive(Clone, Copy)]
    enum Act {
        /// Opens a pull request closing the issue, with these checks.
        Pull(Checks),
        /// Opens a pull request closing the issue, with passing checks, and
        /// arranges it so.
        PullArranged(fn(&mut FakePull)),
        /// Ends cleanly without a pull request.
        NoPull,
        /// Fails, as an agent that couldn't start does.
        Crash,
        /// Sets the cancel signal, as the user would mid-run.
        Cancel,
        /// Runs until cancelled.
        Hang,
    }

    /// What the fake agent does in a resolve session.
    #[derive(Clone, Copy)]
    enum Resolve {
        /// Merges the target branch, fixes what broke and pushes, after which
        /// the checks pass.
        Fixes,
        /// Ends cleanly, having changed nothing.
        Leaves,
        /// Fails, as an agent that couldn't start does.
        Crashes,
        /// Sets the cancel signal, as the user would mid-run.
        Cancels,
        /// Merges the pull request itself, as the user might.
        Merges,
        /// Pushes a head whose checks appear only after some reads.
        PushesChecksToCome,
        /// Fixes it, but only once issue 3's pull request has merged.
        FixesOnce3Merges,
    }

    /// An [`Agent`] that acts on a [`FakeGitHub`] as an agent following the
    /// issue procedure would. Addressing a review, it pushes to the pull
    /// request it was given, which the fake doesn't see, and opens none.
    struct FakeAgent<'a> {
        github: &'a FakeGitHub,
        acts: BTreeMap<u64, Act>,
        /// Whether addressing a review fails, as an agent that couldn't
        /// start does.
        address_crashes: bool,
        /// What each resolve session does in turn. The rest fix it.
        resolves: Mutex<VecDeque<Resolve>>,
        /// What agents wait at once they have acted, so that they finish
        /// together.
        finish_together: Option<Barrier>,
        requests: Mutex<Vec<AgentRequest>>,
        running: AtomicUsize,
        most_running: AtomicUsize,
    }

    impl<'a> FakeAgent<'a> {
        fn new<const N: usize>(github: &'a FakeGitHub, acts: [(u64, Act); N]) -> Self {
            Self {
                github,
                acts: BTreeMap::from(acts),
                address_crashes: false,
                resolves: Mutex::new(VecDeque::new()),
                finish_together: None,
                requests: Mutex::new(Vec::new()),
                running: AtomicUsize::new(0),
                most_running: AtomicUsize::new(0),
            }
        }

        /// Makes the resolve sessions do `resolves`, in turn.
        fn resolving(self, resolves: &[Resolve]) -> Self {
            *self.resolves.lock().unwrap() = resolves.iter().copied().collect();
            self
        }

        /// How many resolve sessions ran.
        fn resolve_sessions(&self) -> usize {
            let requests = self.requests.lock().unwrap();
            requests.iter().filter(|r| resolves(r)).count()
        }

        /// Opens a pull request from `issue-<number>` into `branch` closing
        /// issue `number`.
        fn open(&self, number: u64, branch: &str) -> u64 {
            let head = format!("issue-{number}");
            self.github.add_branch(REPO, &head);
            let title = format!("Issue {number}");
            let body = format!("Closes #{number}");
            self.github
                .open_pull_request(REPO, &head, branch, &title, &body)
                .unwrap()
        }

        /// Pushes to issue `number`'s pull request what it needs to merge,
        /// with these checks to come.
        fn push(&self, number: u64, checks: Vec<Checks>) {
            let pr = pr_of(self.github, number);
            self.github.arrange_pull(REPO, pr, |pull| {
                pull.checks_after_push = checks;
                pull.take_base();
            });
        }

        /// Runs a resolve session on issue `number`'s pull request.
        fn resolve(&self, number: u64, cancel: &AtomicBool) -> Option<String> {
            let next = self.resolves.lock().unwrap().pop_front();
            match next.unwrap_or(Resolve::Fixes) {
                Resolve::Fixes => self.push(number, vec![Checks::Passing]),
                Resolve::PushesChecksToCome => {
                    let checks = [Checks::None, Checks::None, Checks::None, Checks::Passing];
                    self.push(number, checks.to_vec());
                }
                Resolve::Merges => {
                    let pr = pr_of(self.github, number);
                    self.github.set_pull_state(REPO, pr, PullState::Merged);
                }
                Resolve::FixesOnce3Merges => {
                    let merged = || {
                        let pulls = self.github.repo(REPO).pulls;
                        pulls
                            .values()
                            .any(|pull| pull.head == "issue-3" && pull.state == PullState::Merged)
                    };
                    let until = Instant::now() + Duration::from_secs(2);
                    while !merged() {
                        if Instant::now() > until {
                            return Some("issue 3 never merged".into());
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    self.push(number, vec![Checks::Passing]);
                }
                Resolve::Leaves => {}
                Resolve::Crashes => return Some("agent_failed".into()),
                Resolve::Cancels => {
                    cancel.store(true, Ordering::SeqCst);
                    return Some("cancelled".into());
                }
            }
            None
        }
        /// Each agent's issue and target branch, in the order they ran.
        fn issues(&self) -> Vec<(u64, String)> {
            let requests = self.requests.lock().unwrap();
            requests
                .iter()
                .map(|r| (r.issue, r.branch.clone()))
                .collect()
        }

        fn act(&self, request: &AgentRequest, cancel: &AtomicBool) -> Option<String> {
            let (number, branch) = (request.issue, request.branch.as_str());
            if resolves(request) {
                return self.resolve(number, cancel);
            }
            if addresses(request) {
                return self.address_crashes.then(|| "agent_failed".into());
            }
            match self
                .acts
                .get(&number)
                .copied()
                .unwrap_or(Act::Pull(Checks::Passing))
            {
                Act::Pull(checks) => {
                    let pr = self.open(number, branch);
                    self.github.set_checks(REPO, pr, checks);
                    None
                }
                Act::PullArranged(arrange) => {
                    let pr = self.open(number, branch);
                    self.github.set_checks(REPO, pr, Checks::Passing);
                    self.github.arrange_pull(REPO, pr, arrange);
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

    /// Whether `request` is a resolve session.
    fn resolves(request: &AgentRequest) -> bool {
        request.prompt.starts_with("Resolve pull request")
    }

    /// Whether `request` addresses a review: a coding session after the
    /// first, whose log is `<n>-coding-<round>.jsonl`.
    fn addresses(request: &AgentRequest) -> bool {
        let log = request.jsonl_path.file_name().unwrap().to_string_lossy();
        log.contains("-coding-")
    }

    impl Agent for FakeAgent<'_> {
        fn run(&self, request: &AgentRequest, cancel: &AtomicBool) -> AgentOutcome {
            self.requests.lock().unwrap().push(request.clone());
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_running.fetch_max(now, Ordering::SeqCst);
            // Long enough for issues started together to overlap.
            thread::sleep(Duration::from_millis(50));
            let stopped = self.act(request, cancel);
            if let Some(barrier) = &self.finish_together {
                barrier.wait();
            }
            self.running.fetch_sub(1, Ordering::SeqCst);
            AgentOutcome {
                exited_cleanly: stopped.is_none(),
                error: stopped.as_ref().map(|_| "it broke".to_owned()),
                stop_reason: stopped.unwrap_or_else(|| "end_turn".into()),
                duration_ms: 1_000,
                cost_usd: Some(0.25),
            }
        }

        fn check(&self, _cwd: &std::path::Path, _model: Option<&str>) -> Result<(), String> {
            Ok(())
        }
    }

    /// An [`Agent`] that reviews a run's pull request into `main` as an
    /// agent following the review procedure would: each round it posts one
    /// review ending with that round's marker, or with no marker, and
    /// leaves `open_threads` unresolved.
    struct FakeReviewer<'a> {
        github: &'a FakeGitHub,
        root: PathBuf,
        /// Each round's marker. Rounds past the last repeat it.
        markers: Vec<Option<&'static str>>,
        open_threads: usize,
        requests: Mutex<Vec<AgentRequest>>,
        /// The pull request's checks and the run's disposition, as each
        /// review saw them.
        seen: Mutex<Vec<(Checks, run::Disposition)>>,
    }

    impl<'a> FakeReviewer<'a> {
        fn new(github: &'a FakeGitHub, root: &Path, marker: Option<&'static str>) -> Self {
            Self {
                github,
                root: root.to_owned(),
                markers: vec![marker],
                open_threads: 0,
                requests: Mutex::new(Vec::new()),
                seen: Mutex::new(Vec::new()),
            }
        }

        /// Marks the next round's review with `marker`.
        fn then(mut self, marker: Option<&'static str>) -> Self {
            self.markers.push(marker);
            self
        }
    }

    impl Agent for FakeReviewer<'_> {
        fn run(&self, request: &AgentRequest, _cancel: &AtomicBool) -> AgentOutcome {
            let round = {
                let mut requests = self.requests.lock().unwrap();
                requests.push(request.clone());
                requests.len()
            };
            let pulls = self.github.repo(REPO).pulls;
            let (&pr, _) = pulls
                .iter()
                .find(|(_, pull)| pull.base == "main" && pull.state == PullState::Open)
                .expect("the run's pull request is open");
            let run = run::read(&self.root, &RunKey::new(REPO, request.issue)).unwrap();
            let checks = self.github.checks(REPO, pr).unwrap();
            self.seen.lock().unwrap().push((checks, run.disposition));
            let marker = self.markers[round.min(self.markers.len()) - 1];
            let body = match marker {
                Some(verdict) => {
                    format!("Read it.\n\n<!-- epik-review: {verdict} round={round} -->")
                }
                None => "Read it.".to_owned(),
            };
            self.github.add_review(REPO, pr, &body, Utc::now());
            self.github
                .set_open_review_threads(REPO, pr, self.open_threads);
            AgentOutcome {
                exited_cleanly: true,
                error: None,
                stop_reason: "end_turn".into(),
                duration_ms: 2_000,
                cost_usd: Some(0.5),
            }
        }

        fn check(&self, _cwd: &std::path::Path, _model: Option<&str>) -> Result<(), String> {
            Ok(())
        }
    }

    /// The review role, with a model.
    fn reviewing() -> Option<Role> {
        Some(Role {
            agent: "claude".to_owned(),
            model: Some("sonnet".to_owned()),
        })
    }

    /// A repository on `main` holding open issue 7, "One thing".
    fn single() -> FakeGitHub {
        let github = FakeGitHub::new();
        github.add_repo(REPO, "main");
        github.add_issue(REPO, 7, "One thing");
        github
    }

    /// Builds `number` to its end with `review` as the review role and
    /// `reviewer` as its agent.
    fn build_reviewed(
        github: &FakeGitHub,
        agent: &FakeAgent,
        reviewer: &FakeReviewer,
        root: &Path,
        number: u64,
        review: Option<Role>,
    ) -> BuildState {
        let mut request = request(root, number);
        request.roles.review = review;
        start(github, agent, &request)
            .unwrap()
            .reviewer(reviewer)
            .check_interval(Duration::from_millis(1))
            .run(&AtomicBool::new(false))
    }

    /// Every event on disk, each written briefly, without its time.
    fn all_events(root: &Path, state: &BuildState) -> Vec<String> {
        let events = record::read_events(root, &state.run(), &state.build_id).unwrap();
        events.iter().map(brief).collect()
    }

    /// A repository on `main` holding epic 1, "The epic", whose sub-issues
    /// are `issues`, each with its blockers.
    fn epic<const N: usize>(issues: [(u64, &[u64]); N]) -> FakeGitHub {
        let github = FakeGitHub::new();
        github.add_repo(REPO, "main");
        github.add_issue(REPO, 1, "The epic");
        for (number, blockers) in issues {
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
            roles: Settings::default(),
        }
    }

    /// Builds `number` to its end, checking pending checks every
    /// millisecond.
    fn build(github: &FakeGitHub, agent: &FakeAgent, root: &Path, number: u64) -> BuildState {
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

    /// The run's events on disk, each written briefly, without its time,
    /// leaving out its sessions'.
    fn events(root: &Path, state: &BuildState) -> Vec<String> {
        let events = record::read_events(root, &state.run(), &state.build_id).unwrap();
        events
            .iter()
            .filter(|event| {
                !matches!(
                    event,
                    Event::SessionStarted { .. } | Event::SessionFinished { .. }
                )
            })
            .map(brief)
            .collect()
    }

    fn brief(event: &Event) -> String {
        let pr = |pr: &Option<u64>| pr.map_or(String::new(), |pr| format!(" #{pr}"));
        let why = |reason: &Option<String>| match reason {
            Some(reason) => format!(" ({reason})"),
            None => String::new(),
        };
        match event {
            Event::BuildStarted {
                kind,
                branch,
                issues,
                closed,
                ..
            } => {
                let issues: Vec<String> = issues
                    .iter()
                    .map(|m| {
                        let mark = if closed.contains(&m.number) { "x" } else { "" };
                        format!("{mark}{}{:?}", m.number, m.blocked_by)
                    })
                    .collect();
                format!("build_started {kind:?} {branch} {}", issues.join(" "))
            }
            Event::IssueStarted { number, .. } => format!("issue_started {number}"),
            Event::SessionStarted {
                number,
                role,
                round,
                agent,
                model,
                log,
                ..
            } => format!("session_started {number} {role:?} {round} {agent} {model:?} {log}"),
            Event::SessionFinished {
                number,
                role,
                round,
                duration_ms,
                cost_usd,
                ..
            } => format!(
                "session_finished {number} {role:?} {round} {duration_ms} {}",
                cost_usd.map_or("-".to_owned(), |cost| cost.to_string())
            ),
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
            Event::BuildFinished {
                outcome,
                reason,
                pull_request,
                review,
                open_threads,
                ..
            } => {
                let review = review.map_or(String::new(), |review| format!(" {review:?}"));
                let open = open_threads.map_or(String::new(), |open| format!(" {open} open"));
                format!(
                    "build_finished {outcome:?}{}{review}{open}{}",
                    pr(pull_request),
                    why(reason)
                )
            }
            Event::BuildInterrupted { reason, .. } => format!("build_interrupted ({reason})"),
            Event::PullRequestClosed { pull_request, .. } => {
                format!("pull_request_closed #{pull_request}")
            }
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
                format!("build_started Epic {EPIC_BRANCH} 2[] 3[] 4[2, 3] 5[4]"),
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
                format!("build_finished Succeeded #{epic_pr}"),
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
            let [comment] = &issue.comments[..] else {
                panic!("one comment on #{n}");
            };
            let sha = &FakeGitHub::merge_commit(pr(n))[..7];
            let merged = format!(
                "**Epik: 1s, $0.25.** Merged into `{EPIC_BRANCH}` by #{} ({sha}).",
                pr(n)
            );
            assert!(comment.starts_with(&merged), "{comment}");
            assert!(comment.contains("<!-- epik:cost {"), "{comment}");
        }
        let epic_pull = &repo.pulls[&epic_pr];
        assert_eq!(epic_pull.head, EPIC_BRANCH);
        assert_eq!(epic_pull.base, "main");
        assert_eq!(epic_pull.title, "The epic");
        assert!(epic_pull.body.contains("Closes #1"));
        assert_eq!(epic_pull.state, PullState::Open);
        assert_eq!(repo.issues[&1].state, State::Open);

        assert_eq!(state.status, BuildStatus::Succeeded);
        assert_eq!(state.cost_usd, Decimal::from_str("1.00").unwrap());
    }

    #[test]
    fn each_issues_agent_runs_as_a_coding_session_with_the_coding_role() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let mut request = request(root.path(), 1);
        request.roles.coding.model = Some("opus".to_owned());
        let state = start(&github, &agent, &request)
            .unwrap()
            .check_interval(Duration::from_millis(1))
            .run(&AtomicBool::new(false));

        let events = record::read_events(root.path(), &state.run(), &state.build_id).unwrap();
        let briefly: Vec<String> = events.iter().map(brief).collect();
        let pr = pr_of(&github, 2);
        assert_eq!(
            briefly[1..5],
            [
                "issue_started 2".to_owned(),
                "session_started 2 Coding 1 claude Some(\"opus\") 2.jsonl".to_owned(),
                "session_finished 2 Coding 1 1000 0.25".to_owned(),
                format!("issue_finished 2 Succeeded #{pr}"),
            ]
        );
        let finished = events
            .iter()
            .find(|event| matches!(event, Event::IssueFinished { .. }));
        let Some(Event::IssueFinished {
            duration_ms,
            cost_usd,
            ..
        }) = finished
        else {
            unreachable!()
        };
        assert_eq!(
            (duration_ms, cost_usd),
            (&None, &None),
            "the session has them"
        );

        let issue = state.issue(2).unwrap();
        assert_eq!(issue.sessions.len(), 1);
        assert_eq!(issue.duration_ms, Some(1_000));
        assert_eq!(issue.cost_usd, Some(Decimal::from_str("0.25").unwrap()));
    }

    #[test]
    fn a_build_clones_the_repository_and_keeps_each_agents_logs() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);

        let clone = run::clone_dir(root.path(), &state.run(), &state.build_id);
        assert_eq!(github.clones(), [(REPO.to_owned(), clone.clone())]);
        let requests = agent.requests.lock().unwrap();
        let [request] = &requests[..] else {
            panic!("one agent ran");
        };
        let dir = record::build_dir(root.path(), &state.run(), &state.build_id);
        assert_eq!(request.issue, 2);
        assert_eq!(request.branch, EPIC_BRANCH);
        assert_eq!(
            request.prompt,
            prompt::prompt(REPO, Work::Issue(2), EPIC_BRANCH)
        );
        assert_eq!(request.cwd, clone);
        assert_eq!(request.jsonl_path, dir.join("2.jsonl"));
        assert_eq!(request.err_path, dir.join("2.err"));

        let summary = std::fs::read_to_string(dir.join(record::SUMMARY_FILE)).unwrap();
        let summary: BuildState = serde_json::from_str(&summary).unwrap();
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
        assert_eq!(agent.issues(), [(2, "daemon".to_owned())]);
        assert_eq!(github.repo(REPO).pulls[&pr_of(&github, 2)].base, "daemon");
    }

    #[test]
    fn each_agent_runs_the_coding_model_and_the_build_records_its_roles() {
        let github = epic([(2, &[]), (3, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let roles = Settings {
            coding: Role {
                agent: "claude".to_owned(),
                model: Some("opus".to_owned()),
            },
            review: Some(Role {
                agent: "claude".to_owned(),
                model: None,
            }),
        };
        let mut request = request(root.path(), 1);
        request.roles = roles.clone();

        let state = run(&github, &agent, &request, &cancel).unwrap();

        let models: Vec<Option<String>> = agent
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.issue != 1)
            .map(|request| request.model.clone())
            .collect();
        assert_eq!(models, [Some("opus".to_owned()), Some("opus".to_owned())]);
        assert_eq!(state.roles, roles);
        let events = record::read_events(root.path(), &state.run(), &state.build_id).unwrap();
        let Event::BuildStarted {
            roles: recorded, ..
        } = &events[0]
        else {
            panic!("the build starts with build_started");
        };
        assert_eq!(*recorded, roles);
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
                format!("build_started Epic {EPIC_BRANCH} 2[] 3[2] 4[3] 5[]"),
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
            ["build_finished Failed (failed: #2; stuck: #3, #4)"]
        );
        assert_eq!(agent.issues().len(), 2);
        let repo = github.repo(REPO);
        assert_eq!(repo.issues[&2].state, State::Open);
        assert_eq!(repo.issues[&5].state, State::Closed);
        assert!(repo.pulls.values().all(|pull| pull.base != "main"));
        assert_eq!(state.issue(3).unwrap().status, IssueStatus::Waiting);
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
                "build_finished Failed (failed: #2)".to_owned(),
            ]
        );
    }

    #[test]
    fn checks_still_failing_after_two_resolve_sessions_fail_the_issue_and_leave_it_open() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, [(2, Act::Pull(Checks::Failing))])
            .resolving(&[Resolve::Leaves, Resolve::Leaves]);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);

        let pr = pr_of(&github, 2);
        assert_eq!(
            events(root.path(), &state)[1..],
            [
                "issue_started 2".to_owned(),
                format!("issue_finished 2 Succeeded #{pr}"),
                format!(
                    "issue_finished 2 Failed #{pr} (checks still fail on #{pr} \
                     after 2 resolve sessions)"
                ),
                "build_finished Failed (failed: #2)".to_owned(),
            ]
        );
        assert_eq!(agent.resolve_sessions(), 2);
        let repo = github.repo(REPO);
        assert_eq!(repo.pulls[&pr].state, PullState::Open);
        assert_eq!(repo.issues[&2].state, State::Open);
        let issue = state.issue(2).unwrap();
        assert_eq!(issue.status, IssueStatus::Failed);
        assert_eq!(issue.pull_request, Some(pr));
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

    /// Builds epic 1 of issue 2 alone, whose agent arranges its pull request
    /// with `arrange` and resolves it as `resolves` say. The build, the pull
    /// request, and how many resolve sessions ran.
    fn build_arranged(
        github: &FakeGitHub,
        arrange: fn(&mut FakePull),
        resolves: &[Resolve],
        root: &Path,
    ) -> (BuildState, u64, usize) {
        let agent = FakeAgent::new(github, [(2, Act::PullArranged(arrange))]).resolving(resolves);
        let state = build(github, &agent, root, 1);
        (state, pr_of(github, 2), agent.resolve_sessions())
    }

    #[test]
    fn a_conflicting_pull_request_gets_a_resolve_session_then_merges() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let conflicting: fn(&mut FakePull) = |pull| pull.conflicting = true;
        let agent = FakeAgent::new(&github, [(2, Act::PullArranged(conflicting))]);
        let state = build(&github, &agent, root.path(), 1);

        let pr = pr_of(&github, 2);
        assert_eq!(agent.resolve_sessions(), 1);
        let requests = agent.requests.lock().unwrap();
        let resolve = &requests[1];
        assert_eq!(resolve.branch, EPIC_BRANCH);
        assert!(resolve.prompt.contains(&format!("#{pr}")));
        assert!(resolve.prompt.contains("`issue-2`"));
        let events = all_events(root.path(), &state);
        assert!(
            events.contains(&"session_started 2 Coding 2 claude None 2-coding-2.jsonl".to_owned()),
            "{events:?}"
        );
        assert!(
            events.contains(&format!("issue_merged 2 #{pr}")),
            "{events:?}"
        );
        assert_eq!(state.status, BuildStatus::Succeeded);
        assert_eq!(github.repo(REPO).pulls[&pr].state, PullState::Merged);
    }

    #[test]
    fn a_pull_request_still_conflicting_after_two_resolve_sessions_fails_its_issue() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, pr, sessions) = build_arranged(
            &github,
            |pull| pull.conflicting = true,
            &[Resolve::Leaves, Resolve::Leaves],
            root.path(),
        );
        assert_eq!(sessions, 2);
        assert_eq!(
            events(root.path(), &state)[3],
            format!(
                "issue_finished 2 Failed #{pr} (#{pr} still conflicts after 2 resolve sessions)"
            )
        );
        assert_eq!(github.repo(REPO).pulls[&pr].state, PullState::Open);
    }

    #[test]
    fn an_agent_that_fails_to_resolve_fails_the_issue() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, pr, _) = build_arranged(
            &github,
            |pull| pull.conflicting = true,
            &[Resolve::Crashes],
            root.path(),
        );
        assert_eq!(
            events(root.path(), &state)[3],
            format!(
                "issue_finished 2 Failed #{pr} \
                 (the agent resolving #{pr} stopped (agent_failed): it broke)"
            )
        );
    }

    #[test]
    fn cancelling_a_resolve_session_interrupts_the_build() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, pr, _) = build_arranged(
            &github,
            |pull| pull.conflicting = true,
            &[Resolve::Cancels],
            root.path(),
        );
        assert_eq!(state.status, BuildStatus::Interrupted);
        assert_eq!(github.repo(REPO).pulls[&pr].state, PullState::Open);
    }

    #[test]
    fn a_pull_request_behind_is_updated_its_new_checks_awaited_then_merged() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, pr, sessions) = build_arranged(
            &github,
            |pull| {
                pull.behind = 3;
                pull.checks_after_push = vec![Checks::Pending, Checks::Passing];
            },
            &[],
            root.path(),
        );
        assert_eq!(sessions, 0);
        assert_eq!(state.status, BuildStatus::Succeeded);
        let repo = github.repo(REPO);
        assert_eq!(repo.pulls[&pr].updates, 1);
        assert_eq!(repo.pulls[&pr].merged_on, Some((0, Checks::Passing)));
    }

    #[test]
    fn an_update_not_yet_landed_is_waited_for_before_the_checks_count() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (_, pr, _) = build_arranged(
            &github,
            |pull| {
                pull.behind = 1;
                pull.update_lag = 3;
                pull.checks_after_push = vec![Checks::Pending, Checks::Passing];
            },
            &[],
            root.path(),
        );
        let repo = github.repo(REPO);
        assert_eq!(repo.pulls[&pr].updates, 1);
        assert_eq!(repo.pulls[&pr].merged_on, Some((0, Checks::Passing)));
    }

    #[test]
    fn an_update_that_never_lands_fails_the_issue() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, pr, _) = build_arranged(
            &github,
            |pull| {
                pull.behind = 1;
                pull.update_lag = u32::MAX;
            },
            &[],
            root.path(),
        );
        assert_eq!(
            events(root.path(), &state)[3],
            format!("issue_finished 2 Failed #{pr} (the update of #{pr}'s branch never landed)")
        );
        assert_eq!(github.repo(REPO).pulls[&pr].state, PullState::Open);
    }

    #[test]
    fn checks_failing_after_an_update_go_back_to_the_agent() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, pr, sessions) = build_arranged(
            &github,
            |pull| {
                pull.behind = 1;
                pull.checks_after_push = vec![Checks::Pending, Checks::Failing];
            },
            &[],
            root.path(),
        );
        assert_eq!(sessions, 1);
        assert_eq!(state.status, BuildStatus::Succeeded);
        let repo = github.repo(REPO);
        assert_eq!(repo.pulls[&pr].updates, 1);
        assert_eq!(repo.pulls[&pr].merged_on, Some((0, Checks::Passing)));
    }

    #[test]
    fn a_conflict_github_finds_only_on_update_goes_back_to_the_agent() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, pr, sessions) = build_arranged(
            &github,
            |pull| {
                pull.behind = 1;
                pull.conflicting = true;
                pull.conflict_unnoticed = true;
            },
            &[],
            root.path(),
        );
        assert_eq!(sessions, 1);
        assert_eq!(state.status, BuildStatus::Succeeded);
        assert_eq!(github.repo(REPO).pulls[&pr].updates, 0);
    }

    #[test]
    fn two_issues_merge_one_at_a_time_and_the_second_is_retested_on_the_first() {
        let slow_checks: fn(&mut FakePull) = |pull| {
            pull.check_reads = VecDeque::from([Checks::Pending, Checks::Pending, Checks::Passing]);
            pull.checks_after_push = vec![Checks::Pending, Checks::Passing];
        };
        let github = epic([(2, &[]), (3, &[])]);
        let acts = [
            (2, Act::PullArranged(slow_checks)),
            (3, Act::PullArranged(slow_checks)),
        ];
        let mut agent = FakeAgent::new(&github, acts);
        agent.finish_together = Some(Barrier::new(2));
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);

        assert_eq!(state.status, BuildStatus::Succeeded);
        let repo = github.repo(REPO);
        let (first, second) = (
            &repo.pulls[&pr_of(&github, 2)],
            &repo.pulls[&pr_of(&github, 3)],
        );
        let mut updates = [first.updates, second.updates];
        updates.sort();
        assert_eq!(updates, [0, 1]);
        for pull in [first, second] {
            assert_eq!(pull.merged_on, Some((0, Checks::Passing)));
        }
    }

    #[test]
    fn a_resolve_session_does_not_hold_the_merge_slot() {
        let github = epic([(2, &[]), (3, &[])]);
        let conflicting: fn(&mut FakePull) = |pull| pull.conflicting = true;
        let agent = FakeAgent::new(&github, [(2, Act::PullArranged(conflicting))])
            .resolving(&[Resolve::FixesOnce3Merges]);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        assert_eq!(state.status, BuildStatus::Succeeded);
        let pr = pr_of(&github, 2);
        assert_eq!(
            github.repo(REPO).pulls[&pr].merged_on,
            Some((0, Checks::Passing))
        );
    }

    #[test]
    fn a_new_head_with_no_checks_yet_is_not_passing() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (_, pr, _) = build_arranged(
            &github,
            |pull| {
                pull.behind = 1;
                pull.checks_after_push = vec![Checks::None, Checks::None, Checks::Passing];
            },
            &[],
            root.path(),
        );
        assert_eq!(
            github.repo(REPO).pulls[&pr].merged_on,
            Some((0, Checks::Passing))
        );
    }

    #[test]
    fn checks_that_never_appear_on_a_new_head_fail_the_issue() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, pr, sessions) = build_arranged(
            &github,
            |pull| {
                pull.behind = 1;
                pull.checks_after_push = vec![Checks::None];
            },
            &[],
            root.path(),
        );
        assert_eq!(sessions, 0);
        assert_eq!(
            events(root.path(), &state)[3],
            format!("issue_finished 2 Failed #{pr} (no checks appeared on #{pr}'s new head)")
        );
        assert_eq!(github.repo(REPO).pulls[&pr].state, PullState::Open);
    }

    #[test]
    fn a_new_head_from_a_resolve_session_with_no_checks_yet_is_not_passing() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (_, pr, sessions) = build_arranged(
            &github,
            |pull| pull.conflicting = true,
            &[Resolve::PushesChecksToCome],
            root.path(),
        );
        assert_eq!(sessions, 1);
        assert_eq!(
            github.repo(REPO).pulls[&pr].merged_on,
            Some((0, Checks::Passing))
        );
    }

    #[test]
    fn a_repository_with_no_checks_still_merges_after_an_update() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(
            &github,
            [(
                2,
                Act::PullArranged(|pull| {
                    pull.checks = Checks::None;
                    pull.behind = 1;
                }),
            )],
        );
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        assert_eq!(state.status, BuildStatus::Succeeded);
        let pr = pr_of(&github, 2);
        assert_eq!(
            github.repo(REPO).pulls[&pr].merged_on,
            Some((0, Checks::None))
        );
    }

    #[test]
    fn an_unknown_mergeable_is_read_again_not_a_conflict() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, _, sessions) = build_arranged(
            &github,
            |pull| pull.mergeable_unknown_reads = 3,
            &[],
            root.path(),
        );
        assert_eq!(sessions, 0);
        assert_eq!(state.status, BuildStatus::Succeeded);
    }

    #[test]
    fn a_mergeable_never_known_fails_the_issue() {
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, pr, sessions) = build_arranged(
            &github,
            |pull| pull.mergeable_unknown_reads = u32::MAX,
            &[],
            root.path(),
        );
        assert_eq!(sessions, 0);
        assert_eq!(
            events(root.path(), &state)[3],
            format!("issue_finished 2 Failed #{pr} (GitHub never said whether #{pr} can merge)")
        );
    }

    #[test]
    fn a_pull_request_merged_meanwhile_counts_as_merged() {
        // GitHub never says whether a merged pull request can merge.
        let github = epic([(2, &[])]);
        let root = tempfile::tempdir().unwrap();
        let (state, pr, sessions) = build_arranged(
            &github,
            |pull| pull.conflicting = true,
            &[Resolve::Merges],
            root.path(),
        );
        assert_eq!(sessions, 1);
        assert_eq!(state.status, BuildStatus::Succeeded);
        let events = events(root.path(), &state);
        assert!(
            events.contains(&format!("issue_merged 2 #{pr}")),
            "{events:?}"
        );
    }

    #[test]
    fn cancelling_stops_an_issue_waiting_for_the_merge_slot() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let build = start(&github, &agent, &request(root.path(), 1)).unwrap();
        let _held = build.merge_slot.lock().unwrap();
        let cancel = AtomicBool::new(false);
        thread::scope(|scope| {
            let waiting = scope.spawn(|| build.take_merge_slot(&cancel).map(drop));
            thread::sleep(Duration::from_millis(20));
            assert!(!waiting.is_finished());
            cancel.store(true, Ordering::SeqCst);
            assert!(matches!(waiting.join().unwrap(), Err(Stop::Cancelled)));
        });
    }

    #[test]
    fn a_cycle_fails_the_build_before_any_agent_starts() {
        let github = epic([(2, &[3]), (3, &[2]), (4, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        assert_eq!(
            events(root.path(), &state),
            [
                format!("build_started Epic {EPIC_BRANCH} 2[3] 3[2] 4[]"),
                "build_finished Failed (a dependency cycle: #2, #3)".to_owned(),
            ]
        );
        assert!(agent.issues().is_empty());
    }

    #[test]
    fn a_issue_in_another_repository_fails_the_build() {
        let github = epic([(2, &[])]);
        github.add_repo("other/repo", "main");
        github.add_issue("other/repo", 9, "Elsewhere");
        github.add_sub_issue(REPO, 1, "other/repo", 9);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        assert_eq!(
            events(root.path(), &state)[1..],
            ["build_finished Failed (sub-issues in another repository: other/repo#9)"]
        );
        assert!(agent.issues().is_empty());
    }

    #[test]
    fn closed_issues_are_skipped() {
        // Building again after 2 merged.
        let github = epic([(2, &[]), (3, &[2])]);
        github.set_state(REPO, 2, State::Closed);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        let events = events(root.path(), &state);
        assert_eq!(
            events[0],
            format!("build_started Epic {EPIC_BRANCH} x2[] 3[2]")
        );
        assert_eq!(events[1], "issue_started 3");
        assert_eq!(agent.issues(), [(3, EPIC_BRANCH.to_owned())]);
        assert_eq!(state.status, BuildStatus::Succeeded);
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
                "build_started Issue main 7[]".to_owned(),
                "issue_started 7".to_owned(),
                format!("issue_finished 7 Succeeded #{pr}"),
                format!("build_finished Succeeded #{pr}"),
            ]
        );
        assert_eq!(agent.issues(), [(7, "main".to_owned())]);
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
        assert_eq!(state.issue(7).unwrap().status, IssueStatus::Succeeded);
    }

    #[test]
    fn cancelling_stops_every_agent_and_interrupts_the_build() {
        // 2 is cancelled by the user mid-run while 3 is still working.
        let github = epic([(2, &[]), (3, &[]), (4, &[2])]);
        let agent = FakeAgent::new(&github, [(2, Act::Cancel), (3, Act::Hang)]);
        let root = tempfile::tempdir().unwrap();
        let state = build(&github, &agent, root.path(), 1);
        let events = events(root.path(), &state);
        assert_eq!(events[1..3], ["issue_started 2", "issue_started 3"]);
        assert_eq!(
            sorted(events[3..5].to_vec()),
            [
                "issue_finished 2 Failed (interrupted)",
                "issue_finished 3 Failed (interrupted)",
            ]
        );
        assert_eq!(events[5..], ["build_interrupted (cancelled)"]);
        assert_eq!(agent.issues().len(), 2);
        assert_eq!(state.status, BuildStatus::Interrupted);
        assert_eq!(state.cost_usd, Decimal::from_str("0.50").unwrap());
        assert!(github.repo(REPO).pulls.is_empty());
    }

    #[test]
    fn cancelling_while_checks_are_pending_interrupts_the_build() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, [(2, Act::Pull(Checks::Pending))]);
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let build = start(&github, &agent, &request(root.path(), 1))
            .unwrap()
            .check_interval(Duration::from_secs(3600));
        let state = thread::scope(|scope| {
            let running = scope.spawn(|| build.run(&cancel));
            // Its agent has finished, so the build is waiting on the checks.
            let finished = || {
                run::read(root.path(), &RunKey::new(REPO, 1))
                    .ok()
                    .and_then(|run| run.issue(2).map(|issue| issue.status))
                    == Some(IssueStatus::Succeeded)
            };
            while !finished() {
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
                "build_interrupted (cancelled)".to_owned(),
            ]
        );
        assert_eq!(github.repo(REPO).pulls[&pr].state, PullState::Open);
    }

    #[test]
    fn the_clone_is_removed_when_the_build_ends_and_the_record_kept() {
        for act in [Act::Pull(Checks::Passing), Act::Crash, Act::Cancel] {
            let github = epic([(2, &[])]);
            let agent = FakeAgent::new(&github, [(2, act)]);
            let root = tempfile::tempdir().unwrap();
            let state = build(&github, &agent, root.path(), 1);
            let clone = run::clone_dir(root.path(), &state.run(), &state.build_id);
            assert!(!clone.exists(), "{:?}", state.status);
            let dir = record::build_dir(root.path(), &state.run(), &state.build_id);
            assert!(dir.join(record::SUMMARY_FILE).is_file());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_clone_that_cant_be_removed_leaves_the_outcome_as_it_was() {
        use std::os::unix::fs::PermissionsExt;
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let build = start(&github, &agent, &request(root.path(), 1)).unwrap();
        let clone = build.clone_dir().to_owned();
        let parent = clone.parent().unwrap().to_owned();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();

        let state = build.check_interval(Duration::from_millis(1)).run(&cancel);

        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(clone.exists());
        assert_eq!(state.status, BuildStatus::Succeeded);
    }

    #[test]
    fn building_an_interrupted_epic_again_adds_a_build_to_its_run() {
        // 2 merges, then the user cancels while 3 runs; the second build
        // finds 2 closed and builds only 3.
        let github = epic([(2, &[]), (3, &[2])]);
        let root = tempfile::tempdir().unwrap();
        let first = FakeAgent::new(&github, [(3, Act::Cancel)]);
        let interrupted = build(&github, &first, root.path(), 1);
        assert_eq!(interrupted.status, BuildStatus::Interrupted);

        let second = FakeAgent::new(&github, []);
        let finished = build(&github, &second, root.path(), 1);

        assert_eq!(second.issues(), [(3, EPIC_BRANCH.to_owned())]);
        let run = run::read(root.path(), &RunKey::new(REPO, 1)).unwrap();
        assert_eq!(run.builds.len(), 2);
        assert_eq!(run.disposition, run::Disposition::WaitingForReview);
        assert_eq!(run.elapsed_ms, interrupted.elapsed_ms + finished.elapsed_ms);
        assert_eq!(run.cost_usd, interrupted.cost_usd + finished.cost_usd);
        assert_eq!(run.cost_usd, Decimal::from_str("0.75").unwrap());
        assert_eq!(run.issue(2).unwrap().status, IssueStatus::Merged);
        assert_eq!(run.issue(3).unwrap().status, IssueStatus::Merged);
    }

    #[test]
    fn a_failed_clone_leaves_no_record() {
        let github = epic([(2, &[])]);
        github.fail("clone", "gh: no space left");
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let error = start(&github, &agent, &request(root.path(), 1)).err();
        assert!(matches!(error, Some(Error::GitHub(_))));
        assert!(run::list(root.path()).unwrap().is_empty());
        let runs = std::fs::read_dir(record::runs_dir(root.path())).unwrap();
        assert_eq!(runs.count(), 0, "nothing of the run is left");
    }

    #[test]
    fn two_builds_of_one_issue_get_their_own_records_and_clones() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, [(2, Act::Hang)]);
        let root = tempfile::tempdir().unwrap();
        let first = start(&github, &agent, &request(root.path(), 1)).unwrap();
        let second = start(&github, &agent, &request(root.path(), 1)).unwrap();
        assert_ne!(first.build_id(), second.build_id());
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
        assert!(run::list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn with_review_off_no_review_runs() {
        let github = single();
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let reviewer = FakeReviewer::new(&github, root.path(), Some("approve"));
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 7, None);

        assert!(reviewer.requests.lock().unwrap().is_empty());
        let pr = pr_of(&github, 7);
        assert_eq!(
            all_events(root.path(), &state).last().unwrap(),
            &format!("build_finished Succeeded #{pr}")
        );
        assert_eq!((state.review, state.open_threads), (None, None));
    }

    #[test]
    fn an_approved_single_issue_finishes_with_its_pull_request_and_the_verdict() {
        let github = single();
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let reviewer = FakeReviewer::new(&github, root.path(), Some("approve"));
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 7, reviewing());

        let pr = pr_of(&github, 7);
        assert_eq!(
            all_events(root.path(), &state)[5..],
            [
                "session_started 7 Review 1 claude Some(\"sonnet\") 7-review-1.jsonl".to_owned(),
                "session_finished 7 Review 1 2000 0.5".to_owned(),
                format!("build_finished Succeeded #{pr} Approve 0 open"),
            ]
        );
        let requests = reviewer.requests.lock().unwrap();
        let [request] = &requests[..] else {
            panic!("one review ran");
        };
        let dir = record::build_dir(root.path(), &state.run(), &state.build_id);
        assert_eq!(request.prompt, prompt::review_prompt(REPO, 7, &[], pr, 1));
        assert_eq!(request.issue, 7);
        assert_eq!(request.model, Some("sonnet".to_owned()));
        assert_eq!(request.jsonl_path, dir.join("7-review-1.jsonl"));
        assert_eq!(request.err_path, dir.join("7-review-1.err"));
        assert_eq!(state.status, BuildStatus::Succeeded);
        assert_eq!(state.pull_request, Some(pr));
        assert_eq!(state.review, Some(ReviewVerdict::Approve));
        assert_eq!(state.open_threads, Some(0));
        assert_eq!(github.repo(REPO).pulls[&pr].state, PullState::Open);
    }

    #[test]
    fn changes_are_addressed_on_the_pull_requests_branch_and_reviewed_again() {
        let github = single();
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let reviewer =
            FakeReviewer::new(&github, root.path(), Some("changes")).then(Some("approve"));
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 7, reviewing());

        let pr = pr_of(&github, 7);
        assert_eq!(
            all_events(root.path(), &state)[5..],
            [
                "session_started 7 Review 1 claude Some(\"sonnet\") 7-review-1.jsonl".to_owned(),
                "session_finished 7 Review 1 2000 0.5".to_owned(),
                "session_started 7 Coding 2 claude None 7-coding-2.jsonl".to_owned(),
                "session_finished 7 Coding 2 1000 0.25".to_owned(),
                "session_started 7 Review 2 claude Some(\"sonnet\") 7-review-2.jsonl".to_owned(),
                "session_finished 7 Review 2 2000 0.5".to_owned(),
                format!("build_finished Succeeded #{pr} Approve 0 open"),
            ]
        );
        let requests = agent.requests.lock().unwrap();
        let [_, request] = &requests[..] else {
            panic!("the issue's agent, then one addressing the review");
        };
        let dir = record::build_dir(root.path(), &state.run(), &state.build_id);
        let work = Work::Address {
            issue: 7,
            epic: false,
            pull_request: pr,
            head: "issue-7",
        };
        assert_eq!(request.prompt, prompt::prompt(REPO, work, "main"));
        assert_eq!(request.issue, 7);
        assert_eq!(request.branch, "main");
        assert_eq!(request.model, None);
        assert_eq!(request.jsonl_path, dir.join("7-coding-2.jsonl"));
        assert_eq!(request.err_path, dir.join("7-coding-2.err"));
        let reviews = reviewer.requests.lock().unwrap();
        assert_eq!(
            reviews[1].prompt,
            prompt::review_prompt(REPO, 7, &[], pr, 2)
        );
        assert_eq!(github.repo(REPO).pulls.len(), 1);
        assert_eq!(state.status, BuildStatus::Succeeded);
        assert_eq!(state.review, Some(ReviewVerdict::Approve));
        let run = run::read(root.path(), &state.run()).unwrap();
        assert_eq!(run.disposition, run::Disposition::WaitingForReview);
    }

    #[test]
    fn changes_after_the_last_round_leave_the_pull_request_for_the_user_needing_attention() {
        let github = single();
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let mut reviewer = FakeReviewer::new(&github, root.path(), Some("changes"));
        reviewer.open_threads = 2;
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 7, reviewing());

        let pr = pr_of(&github, 7);
        assert_eq!(reviewer.requests.lock().unwrap().len(), 2);
        assert_eq!(agent.requests.lock().unwrap().len(), 2);
        let reason = format!("the review still asks for changes after 2 rounds; see #{pr}");
        assert_eq!(state.status, BuildStatus::Failed);
        assert_eq!(state.reason, Some(reason.clone()));
        assert_eq!(state.pull_request, Some(pr));
        assert_eq!(state.review, Some(ReviewVerdict::Changes));
        assert_eq!(state.open_threads, Some(2));
        assert_eq!(github.repo(REPO).pulls[&pr].state, PullState::Open);
        let run = run::read(root.path(), &state.run()).unwrap();
        assert_eq!(run.disposition, run::Disposition::NeedsAttention);
        assert_eq!(run.reason, Some(reason));
        assert_eq!(run.pull_request, Some(pr));
        assert_eq!(run.review, Some(ReviewVerdict::Changes));
        assert_eq!(run.open_threads, Some(2));
    }

    #[test]
    fn an_epics_review_is_addressed_on_the_epic_branch() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let reviewer =
            FakeReviewer::new(&github, root.path(), Some("changes")).then(Some("approve"));
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 1, reviewing());

        let epic_pr = state.pull_request.expect("the epic's pull request");
        let requests = agent.requests.lock().unwrap();
        let request = requests.last().unwrap();
        let work = Work::Address {
            issue: 1,
            epic: true,
            pull_request: epic_pr,
            head: EPIC_BRANCH,
        };
        assert_eq!(request.prompt, prompt::prompt(REPO, work, "main"));
        assert_eq!(request.branch, "main");
        let sessions: Vec<_> = state
            .sessions
            .iter()
            .map(|s| (s.role, s.round, s.log.as_str()))
            .collect();
        assert_eq!(
            sessions,
            [
                (SessionRole::Review, 1, "1-review-1.jsonl"),
                (SessionRole::Coding, 2, "1-coding-2.jsonl"),
                (SessionRole::Review, 2, "1-review-2.jsonl"),
            ]
        );
        let verdicts: Vec<_> = state.sessions.iter().map(|s| s.verdict).collect();
        assert_eq!(
            verdicts,
            [
                Some(ReviewVerdict::Changes),
                None,
                Some(ReviewVerdict::Approve)
            ]
        );
        assert_eq!(state.review, Some(ReviewVerdict::Approve));
    }

    #[test]
    fn an_agent_that_fails_to_address_the_review_fails_the_build() {
        let github = single();
        let mut agent = FakeAgent::new(&github, []);
        agent.address_crashes = true;
        let root = tempfile::tempdir().unwrap();
        let reviewer = FakeReviewer::new(&github, root.path(), Some("changes"));
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 7, reviewing());

        let pr = pr_of(&github, 7);
        assert_eq!(reviewer.requests.lock().unwrap().len(), 1);
        assert_eq!(state.status, BuildStatus::Failed);
        assert_eq!(
            state.reason.as_deref(),
            Some("the agent addressing the review stopped (agent_failed): it broke")
        );
        assert_eq!(state.pull_request, Some(pr));
        assert_eq!(state.review, Some(ReviewVerdict::Changes));
    }

    #[test]
    fn a_review_with_no_marker_is_no_verdict() {
        let github = single();
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let reviewer = FakeReviewer::new(&github, root.path(), None);
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 7, reviewing());

        assert_eq!(state.status, BuildStatus::Succeeded);
        assert_eq!(state.review, Some(ReviewVerdict::NoVerdict));
    }

    #[test]
    fn the_review_waits_for_the_pull_requests_checks() {
        let github = single();
        let agent = FakeAgent::new(&github, [(7, Act::Pull(Checks::Pending))]);
        let root = tempfile::tempdir().unwrap();
        let reviewer = FakeReviewer::new(&github, root.path(), Some("approve"));
        let state = thread::scope(|scope| {
            let running = scope
                .spawn(|| build_reviewed(&github, &agent, &reviewer, root.path(), 7, reviewing()));
            while github.repo(REPO).pulls.is_empty() {
                thread::sleep(Duration::from_millis(5));
            }
            thread::sleep(Duration::from_millis(50));
            assert!(reviewer.requests.lock().unwrap().is_empty());
            github.set_checks(REPO, pr_of(&github, 7), Checks::Passing);
            running.join().unwrap()
        });

        let seen = reviewer.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, Checks::Passing);
        assert_eq!(state.review, Some(ReviewVerdict::Approve));
    }

    #[test]
    fn failing_checks_fail_the_build_without_a_review() {
        let github = single();
        let agent = FakeAgent::new(&github, [(7, Act::Pull(Checks::Failing))]);
        let root = tempfile::tempdir().unwrap();
        let reviewer = FakeReviewer::new(&github, root.path(), Some("approve"));
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 7, reviewing());

        let pr = pr_of(&github, 7);
        assert!(reviewer.requests.lock().unwrap().is_empty());
        assert_eq!(state.status, BuildStatus::Failed);
        assert_eq!(state.reason, Some(format!("checks failed on #{pr}")));
        assert_eq!(state.pull_request, Some(pr));
        assert_eq!(state.review, None);
    }

    #[test]
    fn an_epics_issues_merge_without_review_and_its_pull_request_is_reviewed_once() {
        let github = epic([(2, &[]), (3, &[2])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let reviewer = FakeReviewer::new(&github, root.path(), Some("approve"));
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 1, reviewing());

        let epic_pr = state.pull_request.expect("the epic's pull request");
        let events = events(root.path(), &state);
        assert_eq!(
            events[1..],
            [
                "issue_started 2".to_owned(),
                format!("issue_finished 2 Succeeded #{}", pr_of(&github, 2)),
                format!("issue_merged 2 #{}", pr_of(&github, 2)),
                "issue_started 3".to_owned(),
                format!("issue_finished 3 Succeeded #{}", pr_of(&github, 3)),
                format!("issue_merged 3 #{}", pr_of(&github, 3)),
                format!("build_finished Succeeded #{epic_pr} Approve 0 open"),
            ]
        );
        let requests = reviewer.requests.lock().unwrap();
        let [request] = &requests[..] else {
            panic!("one review ran");
        };
        assert_eq!(
            request.prompt,
            prompt::review_prompt(REPO, 1, &[2, 3], epic_pr, 1)
        );
        let [session] = &state.sessions[..] else {
            panic!("the run has one session of its own");
        };
        assert_eq!((session.role, session.round), (SessionRole::Review, 1));
        assert_eq!(session.log, "1-review-1.jsonl");
    }

    #[test]
    fn while_the_review_runs_the_run_is_reviewing() {
        for number in [1, 7] {
            let github = epic([(2, &[])]);
            github.add_issue(REPO, 7, "One thing");
            let agent = FakeAgent::new(&github, []);
            let root = tempfile::tempdir().unwrap();
            let reviewer = FakeReviewer::new(&github, root.path(), Some("approve"));
            build_reviewed(&github, &agent, &reviewer, root.path(), number, reviewing());

            let seen = reviewer.seen.lock().unwrap();
            assert_eq!(seen[0].1, run::Disposition::Reviewing, "#{number}");
        }
    }

    #[test]
    fn the_builds_time_and_cost_include_the_review() {
        let github = epic([(2, &[])]);
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let reviewer = FakeReviewer::new(&github, root.path(), Some("approve"));
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 1, reviewing());

        assert_eq!(state.cost_usd, Decimal::from_str("0.75").unwrap());
        assert_eq!(state.sessions[0].duration_ms, Some(2_000));
        let run = run::read(root.path(), &state.run()).unwrap();
        assert_eq!(run.cost_usd, Decimal::from_str("0.75").unwrap());
        assert_eq!(run.sessions.len(), 1);

        let github = single();
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let reviewer = FakeReviewer::new(&github, root.path(), Some("approve"));
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 7, reviewing());

        let issue = state.issue(7).unwrap();
        assert_eq!(issue.sessions.len(), 2);
        assert_eq!(issue.duration_ms, Some(3_000));
        assert_eq!(state.cost_usd, Decimal::from_str("0.75").unwrap());

        let github = single();
        let agent = FakeAgent::new(&github, []);
        let root = tempfile::tempdir().unwrap();
        let reviewer =
            FakeReviewer::new(&github, root.path(), Some("changes")).then(Some("approve"));
        let state = build_reviewed(&github, &agent, &reviewer, root.path(), 7, reviewing());

        let issue = state.issue(7).unwrap();
        assert_eq!(issue.sessions.len(), 4);
        assert_eq!(issue.duration_ms, Some(6_000));
        assert_eq!(state.cost_usd, Decimal::from_str("1.5").unwrap());
        let run = run::read(root.path(), &state.run()).unwrap();
        assert_eq!(run.cost_usd, Decimal::from_str("1.5").unwrap());
    }
}
