//! The GitHub calls of a feature build: the branch a feature is built
//! on, the pull requests into and out of it, the wait for check
//! verdicts, the merge, and the closing of an issue.
//!
//! None of these launches an agent, so none takes a slot. Each call
//! goes to the blocking pool for its own length, and the wait between
//! two polls is an async sleep.
//!
//! An issue's steps are [`Feature`]'s methods, run in order inside one
//! node: [`open_issue_pull`](Feature::open_issue_pull) returns the
//! [`Pull`] whose number [`merge`](Feature::merge) takes. A step that is
//! a whole node is a [`Task`]: [`CreateBranch`] and [`OpenPull`].

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::time::{Instant, sleep};

use super::briefly;
use super::task::Task;
use crate::github::{Check, Error, GitHub, Merge, Pull, Repo, State};

/// The branch feature `number` is built on.
#[must_use]
pub fn feature_branch(number: u64) -> String {
    format!("feature-{number}")
}

/// A feature under construction: the issue `number` in `repo`, whose
/// members merge into its [`branch`](Self::branch).
#[derive(Clone, Debug)]
pub struct Feature {
    pub github: GitHub,
    pub repo: Repo,
    pub number: u64,
}

/// How a wait for check verdicts is bounded, and what it expects.
#[derive(Clone, Copy, Debug)]
pub struct CheckWait {
    /// Whether the repository runs checks on pull requests into a
    /// feature branch. Stated by the build.
    pub expected: bool,
    /// The sleep between two polls.
    pub every: Duration,
    /// How long the wait lasts before it fails.
    pub bound: Duration,
    /// How many consecutive polls must read the same green list before
    /// it is the verdict. GitHub creates a job's check run when the job
    /// is queued, so a job waiting on another has no run while that one
    /// goes: a single poll can land on a list that is green only because
    /// it is still filling. Epik cannot know a repository's job names,
    /// so it waits for the list to settle.
    pub settles: NonZeroUsize,
}

/// What a wait that succeeded saw.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// Every check run concluded green, and there was at least one.
    Green,
    /// No check run, on a repository expecting none: nothing is claimed.
    Unclaimed,
}

/// One poll's check runs, read together.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Reading {
    Empty,
    /// The names of the runs still going; none is red.
    Pending(String),
    /// The names of the runs that concluded red.
    Red(String),
    /// The names of every run, sorted: the set two polls compare. A
    /// green list that gained or lost a run between them has not
    /// settled.
    Green(BTreeSet<String>),
}

impl Reading {
    fn of(checks: &[Check]) -> Self {
        let names = |which: fn(&Check) -> bool| {
            let names: Vec<_> = checks
                .iter()
                .filter(|check| which(check))
                .map(|check| check.name.as_str())
                .collect();
            names.join(", ")
        };
        let (red, pending) = (names(Check::red), names(Check::pending));
        if !red.is_empty() {
            Self::Red(red)
        } else if !pending.is_empty() {
            Self::Pending(pending)
        } else if checks.is_empty() {
            Self::Empty
        } else {
            Self::Green(checks.iter().map(|check| check.name.clone()).collect())
        }
    }
}

impl Feature {
    /// The branch the feature is built on.
    #[must_use]
    pub fn branch(&self) -> String {
        feature_branch(self.number)
    }

    /// The feature branch created at the default branch's tip. A branch
    /// of that name already standing is adopted.
    ///
    /// # Errors
    ///
    /// GitHub's failure, or a default branch with no commit.
    pub async fn create_branch(&self) -> Result<()> {
        let branch = self.branch();
        self.ask(move |github, repo| {
            let default = github.default_branch(repo)?;
            let tip = github
                .branch_sha(repo, &default)?
                .with_context(|| format!("{repo}'s default branch {default} has no commit"))?;
            match github.create_branch(repo, &branch, &tip) {
                Err(Error::Refused { .. }) if github.branch_sha(repo, &branch)?.is_some() => Ok(()),
                created => Ok(created?),
            }
        })
        .await
    }

    /// The pull request of member `issue`: `head`, the issue's branch,
    /// into the feature branch.
    ///
    /// # Errors
    ///
    /// GitHub's failure.
    pub async fn open_issue_pull(&self, issue: u64, head: &str) -> Result<Pull> {
        let (head, base) = (head.to_owned(), self.branch());
        self.ask(move |github, repo| closing_pull(github, repo, issue, &head, &base))
            .await
    }

    /// The feature's own pull request: the feature branch into the
    /// default branch, opened and left for a human.
    ///
    /// # Errors
    ///
    /// GitHub's failure.
    pub async fn open_pull(&self) -> Result<Pull> {
        let (feature, head) = (self.number, self.branch());
        self.ask(move |github, repo| {
            let base = github.default_branch(repo)?;
            closing_pull(github, repo, feature, &head, &base)
        })
        .await
    }

    /// Waits for the check runs on commit `sha` to conclude green and to
    /// stay that way for `wait.settles` consecutive polls: one read,
    /// then a sleep of `wait.every`. `sha` is the commit Epik pushed, so
    /// a push racing the wait is never judged in its place.
    ///
    /// # Errors
    ///
    /// A red check run, by name; the bound reached with runs still
    /// going, with a green list still moving, or, with checks expected,
    /// with none seen; GitHub's failure.
    pub async fn await_checks(&self, sha: &str, wait: CheckWait) -> Result<Verdict> {
        let deadline = Instant::now() + wait.bound;
        // A reading and how many consecutive polls have read it. Before
        // the first poll that is the empty list, read zero times.
        let mut held = (Reading::Empty, 0usize);
        loop {
            let polled = sha.to_owned();
            let checks = self
                .ask(move |github, repo| Ok(github.check_conclusions(repo, &polled)?))
                .await?;
            let bound = wait.bound;
            let out_of_time = Instant::now() + wait.every > deadline;
            let reading = Reading::of(&checks);
            held = match held {
                (same, times) if same == reading => (same, times + 1),
                _ => (reading.clone(), 1),
            };
            let settled = held.1 >= wait.settles.get();
            match reading {
                Reading::Green(_) if settled => return Ok(Verdict::Green),
                Reading::Empty if !wait.expected => return Ok(Verdict::Unclaimed),
                Reading::Red(names) => bail!("checks failed on {sha}: {names}"),
                Reading::Pending(names) if out_of_time => {
                    bail!("checks still running on {sha} after {bound:?}: {names}")
                }
                Reading::Green(names) if out_of_time => {
                    let names = Vec::from_iter(names).join(", ");
                    bail!("green on {sha} never settled within {bound:?}: {names}")
                }
                Reading::Empty if out_of_time => return Err(self.no_check_run(sha, bound).await),
                Reading::Green(_) | Reading::Pending(_) | Reading::Empty => {
                    sleep(wait.every).await;
                }
            }
        }
    }

    /// Why a wait ran out having seen no check run: one read of the
    /// commit's statuses says whether the repository's CI posts those
    /// instead.
    async fn no_check_run(&self, sha: &str, bound: Duration) -> anyhow::Error {
        let polled = sha.to_owned();
        let statuses = self
            .ask(move |github, repo| Ok(github.status_count(repo, &polled)?))
            .await;
        match statuses {
            Ok(0) => anyhow::anyhow!("no check run appeared on {sha} within {bound:?}"),
            Ok(count) => anyhow::anyhow!(
                "no check run appeared on {sha} within {bound:?}: the commit carries \
                 {count} commit statuses, and Epik reads verdicts from check runs only"
            ),
            Err(error) => error.context(format!(
                "no check run appeared on {sha} within {bound:?}; reading its commit statuses"
            )),
        }
    }

    /// Merges pull request `pull`, whose head must still stand on
    /// `judged` — the commit [`await_checks`](Self::await_checks) read.
    /// A push that moved the branch in between is GitHub's 409, so only
    /// the commit the checks judged is merged. A refusal — a conflict,
    /// a ruleset, a red required check — is [`Error::Refused`] in
    /// GitHub's own words.
    ///
    /// # Errors
    ///
    /// GitHub's failure.
    pub async fn merge(&self, pull: u64, judged: &str, method: Merge) -> Result<()> {
        let judged = judged.to_owned();
        self.ask(move |github, repo| Ok(github.merge_pull(repo, pull, Some(&judged), method)?))
            .await
    }

    /// Closes member `issue`. A closing keyword fires only on a merge
    /// into the default branch, so a merge into the feature branch
    /// leaves the closing to this.
    ///
    /// # Errors
    ///
    /// GitHub's failure.
    pub async fn close(&self, issue: u64) -> Result<()> {
        self.ask(move |github, repo| Ok(github.close_issue(repo, issue).map(drop)?))
            .await
    }

    /// `call` on the blocking pool, for its own length.
    async fn ask<R: Send + 'static>(
        &self,
        call: impl FnOnce(&GitHub, &Repo) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let Self { github, repo, .. } = self.clone();
        briefly(move || call(&github, &repo)).await
    }
}

/// A pull request titled as issue `number` is, its body the keyword
/// that closes that issue: what fills GitHub's Development field. An
/// open pull request `head` already produced is adopted, the way a
/// standing feature branch is; a closed one is history, and a fresh
/// pull request is opened over it.
fn closing_pull(github: &GitHub, repo: &Repo, number: u64, head: &str, base: &str) -> Result<Pull> {
    let standing = github.pull_for(repo, head)?;
    if let Some(pull) = standing.filter(|pull| pull.state == State::Open) {
        return Ok(pull);
    }
    let title = github.issue(repo, number)?.title;
    Ok(github.open_pull(repo, &title, head, base, &format!("Closes #{number}"))?)
}

/// The node a feature's plan starts from: its branch.
#[derive(Clone, Debug)]
pub struct CreateBranch(pub Feature);

impl Task for CreateBranch {
    async fn run(&self) -> Result<()> {
        self.0.create_branch().await
    }
}

/// The node a feature's plan ends on: its pull request, left open.
#[derive(Clone, Debug)]
pub struct OpenPull(pub Feature);

impl Task for OpenPull {
    async fn run(&self) -> Result<()> {
        self.0.open_pull().await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::{Dag, Node};
    use crate::github::fake::{DEFAULT_BRANCH, FakeGitHub, Request, TIP, check, check_runs};
    use crate::github::{Conclusion, State};
    use crate::plan::AgentSlots;
    use serde_json::{Value, json};
    use std::num::NonZeroUsize;
    use tokio::time::timeout;

    /// Long enough for anything here that makes progress at all.
    const PATIENCE: Duration = Duration::from_secs(30);

    /// The commit Epik pushed, in these tests.
    const PUSHED: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// A short wait: a few polls, then the bound.
    fn wait(expected: bool) -> CheckWait {
        CheckWait {
            expected,
            every: Duration::from_millis(5),
            bound: Duration::from_millis(40),
            settles: NonZeroUsize::new(2).unwrap(),
        }
    }

    /// Feature 7 on `fake`, with member issue 3 on a pushed branch.
    fn feature(fake: &FakeGitHub) -> Feature {
        let mut world = fake.world();
        world.open_issue(7, "Build the thing");
        world.open_issue(3, "Lay the first stone");
        world
            .branches
            .insert("issue-3".to_owned(), PUSHED.to_owned());
        Feature {
            github: fake.github(),
            repo: FakeGitHub::repo(),
            number: 7,
        }
    }

    fn sent(method: &str, path: &str, body: Value) -> Request {
        Request::new(method, &format!("/repos/fake/repo{path}"), body)
    }

    fn get(path: &str) -> Request {
        sent("GET", path, Value::Null)
    }

    /// The look for the pull request `head` already produced.
    fn standing(head: &str) -> Request {
        get(&format!("/pulls?state=all&per_page=1&head=fake:{head}"))
    }

    #[test]
    fn the_feature_branch_is_named_for_the_features_number() {
        assert_eq!(feature_branch(208), "feature-208");
    }

    #[tokio::test]
    async fn the_feature_branch_is_created_at_the_default_branchs_tip() {
        let fake = FakeGitHub::spawn();
        feature(&fake).create_branch().await.unwrap();
        assert_eq!(
            fake.requests(),
            [
                get(""),
                get("/git/ref/heads/main"),
                sent(
                    "POST",
                    "/git/refs",
                    json!({ "ref": "refs/heads/feature-7", "sha": TIP })
                ),
            ]
        );
        assert_eq!(fake.world().branches["feature-7"], TIP);
    }

    #[tokio::test]
    async fn a_feature_branch_already_standing_is_adopted() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        fake.world()
            .branches
            .insert("feature-7".to_owned(), PUSHED.to_owned());
        feature.create_branch().await.unwrap();
        assert_eq!(fake.world().branches["feature-7"], PUSHED, "left as found");
    }

    #[tokio::test]
    async fn a_default_branch_with_no_commit_is_the_failure() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        fake.world().branches.remove(DEFAULT_BRANCH);
        let error = feature.create_branch().await.unwrap_err();
        assert!(error.to_string().contains("has no commit"), "{error}");
    }

    #[tokio::test]
    async fn an_issues_pull_request_goes_into_the_feature_branch_and_closes_the_issue() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        feature.create_branch().await.unwrap();
        let before = fake.requests().len();
        let pull = feature.open_issue_pull(3, "issue-3").await.unwrap();
        assert_eq!(
            fake.requests()[before..],
            [
                standing("issue-3"),
                get("/issues/3"),
                sent(
                    "POST",
                    "/pulls",
                    json!({
                        "title": "Lay the first stone",
                        "head": "issue-3",
                        "base": "feature-7",
                        "body": "Closes #3",
                    })
                ),
            ]
        );
        assert_eq!((pull.number, pull.head.sha.as_str()), (1, PUSHED));
    }

    #[tokio::test]
    async fn the_features_pull_request_goes_into_the_default_branch() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        feature.create_branch().await.unwrap();
        let before = fake.requests().len();
        feature.open_pull().await.unwrap();
        assert_eq!(
            fake.requests()[before..],
            [
                get(""),
                standing("feature-7"),
                get("/issues/7"),
                sent(
                    "POST",
                    "/pulls",
                    json!({
                        "title": "Build the thing",
                        "head": "feature-7",
                        "base": "main",
                        "body": "Closes #7",
                    })
                ),
            ]
        );
    }

    #[tokio::test]
    async fn a_merge_is_a_put_naming_its_method_and_the_commit_judged() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        feature.create_branch().await.unwrap();
        let pull = feature.open_issue_pull(3, "issue-3").await.unwrap();
        feature
            .merge(pull.number, PUSHED, Merge::Squash)
            .await
            .unwrap();
        assert_eq!(
            fake.requests().last(),
            Some(&sent(
                "PUT",
                "/pulls/1/merge",
                json!({ "merge_method": "squash", "sha": PUSHED })
            ))
        );
        assert!(fake.world().pulls[0].merged);
    }

    #[tokio::test]
    async fn a_merge_is_refused_when_the_head_has_moved_off_the_judged_commit() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        feature.create_branch().await.unwrap();
        let pull = feature.open_issue_pull(3, "issue-3").await.unwrap();
        // A push lands between the wait and the merge.
        let raced = "cccccccccccccccccccccccccccccccccccccccc";
        {
            let mut world = fake.world();
            world
                .branches
                .insert("issue-3".to_owned(), raced.to_owned());
            world.pulls[0].head.sha = raced.to_owned();
        }
        let error = feature
            .merge(pull.number, PUSHED, Merge::Squash)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Head branch was modified"),
            "{error}"
        );
        assert!(!fake.world().pulls[0].merged, "the unjudged commit stands");
    }

    #[tokio::test]
    async fn an_open_pull_request_for_the_head_is_adopted() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        feature.create_branch().await.unwrap();
        let first = feature.open_issue_pull(3, "issue-3").await.unwrap();
        let before = fake.requests().len();
        let again = feature.open_issue_pull(3, "issue-3").await.unwrap();
        assert_eq!(again, first, "the standing pull request, as it is");
        assert_eq!(fake.world().pulls.len(), 1, "nothing opened twice");
        let posted = fake.requests()[before..]
            .iter()
            .any(|request| request.method == "POST");
        assert!(!posted, "{:?}", &fake.requests()[before..]);
    }

    #[tokio::test]
    async fn a_closed_pull_request_for_the_head_is_not_adopted() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        feature.create_branch().await.unwrap();
        let first = feature.open_issue_pull(3, "issue-3").await.unwrap();
        fake.world().pulls[0].state = State::Closed;
        let again = feature.open_issue_pull(3, "issue-3").await.unwrap();
        assert_ne!(again.number, first.number, "a fresh pull request");
        assert_eq!(fake.world().pulls.len(), 2);
    }

    #[tokio::test]
    async fn the_features_own_pull_request_is_adopted_too() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        feature.create_branch().await.unwrap();
        let first = feature.open_pull().await.unwrap();
        let again = feature.open_pull().await.unwrap();
        assert_eq!(again, first);
        assert_eq!(fake.world().pulls.len(), 1);
    }

    #[tokio::test]
    async fn a_refused_merge_carries_githubs_words() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        feature.create_branch().await.unwrap();
        let pull = feature.open_issue_pull(3, "issue-3").await.unwrap();
        fake.world().merge_refusal = Some("Pull Request is not mergeable".to_owned());
        let error = feature
            .merge(pull.number, PUSHED, Merge::Squash)
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref(),
            Some(&Error::Refused {
                status: 405,
                message: "Pull Request is not mergeable".to_owned()
            })
        );
        assert!(
            error.to_string().contains("Pull Request is not mergeable"),
            "{error}"
        );
        assert!(!fake.world().pulls[0].merged);
    }

    #[tokio::test]
    async fn an_issue_merged_into_the_feature_branch_is_closed_and_reads_closed() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        feature.create_branch().await.unwrap();
        let pull = feature.open_issue_pull(3, "issue-3").await.unwrap();
        feature
            .merge(pull.number, PUSHED, Merge::Squash)
            .await
            .unwrap();
        assert_eq!(
            feature.github.issue(&feature.repo, 3).unwrap().state,
            State::Open,
            "a merge into a feature branch closes nothing"
        );
        feature.close(3).await.unwrap();
        assert_eq!(
            fake.requests().last(),
            Some(&sent("PATCH", "/issues/3", json!({ "state": "closed" })))
        );
        assert_eq!(
            feature.github.issue(&feature.repo, 3).unwrap().state,
            State::Closed
        );
    }

    #[test]
    fn a_reading_is_green_only_when_every_run_concluded_green() {
        let green = || check("build", Some(Conclusion::Success));
        assert_eq!(Reading::of(&[]), Reading::Empty);
        assert_eq!(
            Reading::of(&[green(), check("docs", Some(Conclusion::Skipped))]),
            Reading::Green(["build".to_owned(), "docs".to_owned()].into())
        );
        assert_eq!(
            Reading::of(&[green(), check("lint", None)]),
            Reading::Pending("lint".to_owned()),
            "a check still running is neither green nor red"
        );
        assert_eq!(
            Reading::of(&[
                check("lint", None),
                check("test", Some(Conclusion::Failure)),
                check("audit", Some(Conclusion::Other)),
            ]),
            Reading::Red("test, audit".to_owned())
        );
    }

    #[tokio::test]
    async fn an_empty_check_list_with_checks_expected_is_never_green() {
        let fake = FakeGitHub::spawn();
        let error = feature(&fake)
            .await_checks(PUSHED, wait(true))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no check run"), "{error}");
        let requests = fake.requests();
        let (last, polls) = requests.split_last().unwrap();
        assert!(polls.len() > 1, "the wait continued: {polls:?}");
        assert!(polls.iter().all(|poll| *poll == check_runs(PUSHED)));
        assert_eq!(
            *last,
            get(&format!("/commits/{PUSHED}/status?per_page=1")),
            "one read of the combined status, at the end"
        );
    }

    #[tokio::test]
    async fn an_empty_check_list_with_none_expected_claims_no_verdict() {
        let fake = FakeGitHub::spawn();
        let verdict = feature(&fake).await_checks(PUSHED, wait(false)).await;
        assert_eq!(verdict.unwrap(), Verdict::Unclaimed);
        assert_eq!(fake.requests(), [check_runs(PUSHED)]);
    }

    #[tokio::test]
    async fn a_timeout_on_a_commit_carrying_statuses_names_them_as_the_reason() {
        let fake = FakeGitHub::spawn();
        fake.world().statuses.insert(PUSHED.to_owned(), 2);
        let error = feature(&fake)
            .await_checks(PUSHED, wait(true))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("2 commit statuses"), "{error}");
    }

    #[tokio::test]
    async fn a_red_check_fails_the_wait_by_name() {
        let fake = FakeGitHub::spawn();
        let runs = vec![
            check("build", Some(Conclusion::Success)),
            check("test", Some(Conclusion::Failure)),
        ];
        fake.world().checks.insert(PUSHED.to_owned(), runs);
        let error = feature(&fake)
            .await_checks(PUSHED, wait(true))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("failed"), "{error}");
        assert!(error.to_string().contains("test"), "{error}");
        assert_eq!(fake.requests().len(), 1, "red is final");
    }

    #[tokio::test]
    async fn checks_still_running_at_the_bound_are_named() {
        let fake = FakeGitHub::spawn();
        let runs = vec![check("test", None)];
        fake.world().checks.insert(PUSHED.to_owned(), runs);
        let error = feature(&fake)
            .await_checks(PUSHED, wait(true))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("still running"), "{error}");
        assert!(
            fake.requests()
                .iter()
                .all(|poll| *poll == check_runs(PUSHED))
        );
    }

    #[tokio::test]
    async fn a_run_that_appears_after_a_green_reading_is_still_judged() {
        let fake = FakeGitHub::spawn();
        {
            // `test` needs `build`, so GitHub creates its run only once
            // `build` has concluded.
            let mut world = fake.world();
            let built = vec![check("build", Some(Conclusion::Success))];
            world.checks.insert(PUSHED.to_owned(), built);
            let red = check("test", Some(Conclusion::Failure));
            world.appearing.push((PUSHED.to_owned(), 1, red));
        }
        let patient = CheckWait {
            bound: PATIENCE,
            ..wait(true)
        };
        let error = feature(&fake)
            .await_checks(PUSHED, patient)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("failed"), "{error}");
        assert!(error.to_string().contains("test"), "{error}");
    }

    #[tokio::test]
    async fn a_green_reading_is_the_verdict_once_it_has_settled() {
        let fake = FakeGitHub::spawn();
        let green = vec![check("build", Some(Conclusion::Success))];
        fake.world().checks.insert(PUSHED.to_owned(), green);
        let settling = CheckWait {
            settles: NonZeroUsize::new(3).unwrap(),
            bound: PATIENCE,
            ..wait(true)
        };
        let verdict = feature(&fake).await_checks(PUSHED, settling).await.unwrap();
        assert_eq!(verdict, Verdict::Green);
        assert_eq!(fake.requests().len(), 3, "green held across three reads");
    }

    #[tokio::test]
    async fn the_checks_read_are_those_of_the_pushed_sha() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        // A push races the wait: the branch moves to a red commit.
        let raced = "cccccccccccccccccccccccccccccccccccccccc";
        {
            let mut world = fake.world();
            world
                .branches
                .insert("issue-3".to_owned(), raced.to_owned());
            let red = vec![check("test", Some(Conclusion::Failure))];
            world.checks.insert(raced.to_owned(), red);
            let green = vec![check("test", Some(Conclusion::Success))];
            world.checks.insert(PUSHED.to_owned(), green);
        }
        let patient = CheckWait {
            bound: PATIENCE,
            ..wait(true)
        };
        let verdict = feature.await_checks(PUSHED, patient).await.unwrap();
        assert_eq!(verdict, Verdict::Green);
        assert!(
            fake.requests()
                .iter()
                .all(|poll| *poll == check_runs(PUSHED))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn with_one_slot_an_agent_runs_to_completion_while_a_wait_is_pending() {
        use crate::process::Process;

        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        let running = vec![check("test", None)];
        fake.world().checks.insert(PUSHED.to_owned(), running);
        let patient = CheckWait {
            bound: PATIENCE,
            ..wait(true)
        };
        let waiting = tokio::spawn(async move { feature.await_checks(PUSHED, patient).await });
        while fake.requests().is_empty() {
            sleep(Duration::from_millis(1)).await;
        }

        let slots = AgentSlots::new(NonZeroUsize::MIN);
        let process = slots.run(|| {
            let arguments = vec!["-c".to_owned(), "true".to_owned()];
            Process::new("sh", arguments, "/", [], Some(PATIENCE))?.wait()
        });
        let exit = timeout(PATIENCE, process).await.unwrap().unwrap();
        assert!(exit.success(), "{exit:?}");
        assert!(!waiting.is_finished(), "the check is still running");

        let green = vec![check("test", Some(Conclusion::Success))];
        fake.world().checks.insert(PUSHED.to_owned(), green);
        let verdict = timeout(PATIENCE, waiting).await.unwrap().unwrap();
        assert_eq!(verdict.unwrap(), Verdict::Green);
    }

    /// The two nodes of a feature's plan that are one GitHub call each.
    enum Ends {
        Branch(CreateBranch),
        Pull(OpenPull),
    }

    impl Task for Ends {
        async fn run(&self) -> Result<()> {
            match self {
                Self::Branch(task) => task.run().await,
                Self::Pull(task) => task.run().await,
            }
        }
    }

    #[tokio::test]
    async fn the_features_pull_request_is_open_and_unmerged_when_the_plan_ends() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        let plan = Dag::new(vec![
            Node::new(Ends::Pull(OpenPull(feature.clone())), [1]),
            Node::new(Ends::Branch(CreateBranch(feature.clone())), []),
        ])
        .unwrap();
        plan.execute().await.unwrap();

        let pull = feature.github.pull(&feature.repo, 1).unwrap();
        assert_eq!((pull.state, pull.merged), (State::Open, false));
        assert_eq!(
            (pull.head.name.as_str(), pull.base.name.as_str()),
            ("feature-7", "main")
        );
    }
}
