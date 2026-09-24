//! The GitHub calls of a feature build: the branch a feature is built
//! on, the pull requests into and out of it, the wait for check
//! verdicts, the merge, and the closing of an issue.
//!
//! None of these launches an agent, so none takes a slot. Each call
//! goes to the blocking pool for its own length, and the wait between
//! two polls is an async sleep.
//!
//! What the wait waits for is what the repository requires: the checks
//! of the policy read at launch
//! ([`plan::policy`](crate::plan::policy)), matched by name and by the
//! app a rule names. A repository requiring none is waited on for no
//! time at all.
//!
//! An issue's steps are [`Feature`]'s methods, run in order inside one
//! node: [`open_issue_pull`](Feature::open_issue_pull) returns the
//! [`Pull`] whose number [`merge`](Feature::merge) takes. A step that is
//! a whole node is a [`Task`]: [`CreateBranch`] and [`OpenPull`].

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::{Instant, sleep};

use super::briefly;
use super::stream::{Happened, Sink};
use super::task::Task;
use crate::github::{Check, Error, GitHub, Merge, Pull, Repo, Required, State};
pub use crate::happening::Verdict;

/// The branch feature `number` is built on.
#[must_use]
pub fn feature_branch(number: u64) -> String {
    format!("feature-{number}")
}

/// What a refused push or merge adds to GitHub's own words. The policy
/// is read from rulesets, which is everything
/// `GET /repos/{owner}/{repo}/rules/branches/{branch}` returns; classic
/// branch protection needs a wider permission than a build asks for and
/// goes unread, while GitHub enforces it all the same.
pub const CLASSIC: &str =
    "a classic branch protection rule, which Epik does not read, may be the cause";

/// A feature under construction: the issue `number` in `repo`, whose
/// members merge into its [`branch`](Self::branch).
#[derive(Clone, Debug)]
pub struct Feature {
    pub github: GitHub,
    pub repo: Repo,
    pub number: u64,
}

/// How a wait for check verdicts is paced and bounded.
#[derive(Clone, Copy, Debug)]
pub struct CheckWait {
    /// The sleep between two polls.
    pub every: Duration,
    /// How long the wait lasts before it fails.
    pub bound: Duration,
}

/// What asking GitHub to merge a pull request came to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Merged {
    /// GitHub merged it.
    Done,
    /// GitHub answered that the branch is not mergeable — a conflict, a
    /// ruleset, a red required check — in its own words, with
    /// [`CLASSIC`] after them.
    Refused(String),
}

/// Whether `check` is a run the rule `required` asks for: the name the
/// rule names, reported by the app it names when it names one. A rule
/// silent on the app takes a run of that name from anywhere.
fn answers(required: &Required, check: &Check) -> bool {
    check.name == required.context && required.app.is_none_or(|app| check.app == Some(app))
}

/// One poll of the required checks, read together. Every required check
/// green is [`Green`](Self::Green); any red is [`Red`](Self::Red); and a
/// required check still running, or yet to report at all, is
/// [`Waiting`](Self::Waiting) — GitHub's own "Expected — Waiting for
/// status to be reported".
#[derive(Clone, Debug, Eq, PartialEq)]
enum Reading {
    /// The names of the required checks yet to conclude.
    Waiting(String),
    /// The names of the required checks that concluded red.
    Red(String),
    Green,
}

impl Reading {
    /// What `checks`, the runs on the commit, say of the rules in
    /// `required`. A rule no run answers is waited for like one still
    /// going: both are a verdict that has not arrived.
    fn of(required: &[Required], checks: &[Check]) -> Self {
        let names = |which: fn(&[&Check]) -> bool| {
            let names: Vec<_> = required
                .iter()
                .filter(|rule| {
                    let answering: Vec<_> =
                        checks.iter().filter(|check| answers(rule, check)).collect();
                    which(&answering)
                })
                .map(|rule| rule.context.as_str())
                .collect();
            names.join(", ")
        };
        let red = names(|answering| answering.iter().any(|check| check.red()));
        let waiting = names(|answering| {
            answering.is_empty() || answering.iter().any(|check| check.pending())
        });
        if !red.is_empty() {
            Self::Red(red)
        } else if !waiting.is_empty() {
            Self::Waiting(waiting)
        } else {
            Self::Green
        }
    }
}

/// The conclusions of the runs no rule in `required` asks for, named
/// and sorted: what the stream says of the checks that decide nothing.
fn unrequired(required: &[Required], checks: &[Check]) -> String {
    let spare: BTreeSet<_> = checks
        .iter()
        .filter(|check| !required.iter().any(|rule| answers(rule, check)))
        .map(|check| {
            let conclusion = match check.conclusion {
                Some(conclusion) if check.red() => format!("{conclusion:?}").to_lowercase(),
                Some(_) => "green".to_owned(),
                None => "running".to_owned(),
            };
            format!("{} ({conclusion})", check.name)
        })
        .collect();
    Vec::from_iter(spare).join(", ")
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

    /// Waits for each check in `required` to conclude on commit `sha`:
    /// one read, then a sleep of `wait.every`, until the last of them
    /// has a verdict. `sha` is the commit Epik pushed, so a push racing
    /// the wait is never judged in its place.
    ///
    /// A required check that concluded red ends the wait as a
    /// [red verdict](Verdict::Red), which is a value for whoever asked
    /// to branch on. A policy requiring nothing is
    /// [`Unclaimed`](Verdict::Unclaimed) before any read: there is
    /// nothing to wait for.
    ///
    /// A run no rule asks for decides nothing. Its conclusion goes to
    /// `said` as [`Happened::Ran`], red included, and the verdict goes
    /// there too, so what the checks came to is the issue's own fact.
    ///
    /// # Errors
    ///
    /// The bound reached with a required check yet to conclude, which
    /// names the ones still waited on; GitHub's failure.
    pub async fn await_checks(
        &self,
        sha: &str,
        required: &[Required],
        wait: CheckWait,
        said: &Sink<Happened>,
    ) -> Result<Verdict> {
        if required.is_empty() {
            said.emit(Happened::Checked(Verdict::Unclaimed));
            return Ok(Verdict::Unclaimed);
        }
        let deadline = Instant::now() + wait.bound;
        loop {
            let polled = sha.to_owned();
            let checks = self
                .ask(move |github, repo| Ok(github.check_conclusions(repo, &polled)?))
                .await?;
            let bound = wait.bound;
            let out_of_time = Instant::now() + wait.every > deadline;
            let spare = unrequired(required, &checks);
            let verdict = match Reading::of(required, &checks) {
                Reading::Green => Verdict::Green,
                Reading::Red(names) => Verdict::Red(names),
                Reading::Waiting(names) if out_of_time => {
                    return Err(self
                        .still_waiting(sha, bound, &names, checks.is_empty())
                        .await);
                }
                Reading::Waiting(_) => {
                    sleep(wait.every).await;
                    continue;
                }
            };
            if !spare.is_empty() {
                said.emit(Happened::Ran(spare));
            }
            said.emit(Happened::Checked(verdict.clone()));
            return Ok(verdict);
        }
    }

    /// Why a wait ran out with `names` still waited on. A commit that
    /// drew no check run at all is `bare`, and one read of its statuses
    /// says whether the repository's CI posts those instead.
    async fn still_waiting(
        &self,
        sha: &str,
        bound: Duration,
        names: &str,
        bare: bool,
    ) -> anyhow::Error {
        let ran_out =
            format!("the required checks on {sha} had not concluded after {bound:?}: {names}");
        if !bare {
            return anyhow::anyhow!(ran_out);
        }
        let polled = sha.to_owned();
        let statuses = self
            .ask(move |github, repo| Ok(github.status_count(repo, &polled)?))
            .await;
        match statuses {
            Ok(0) => anyhow::anyhow!("{ran_out}; no check run appeared at all"),
            Ok(count) => anyhow::anyhow!(
                "{ran_out}; no check run appeared at all, and the commit carries {count} \
                 commit statuses, which Epik reads no verdict from"
            ),
            Err(error) => error.context(format!("{ran_out}; reading its commit statuses")),
        }
    }

    /// Merges pull request `pull`, whose head must still stand on
    /// `judged` — the commit [`await_checks`](Self::await_checks) read.
    /// A push that moved the branch in between is GitHub's 409, so only
    /// the commit the checks judged is merged.
    ///
    /// A branch GitHub calls not mergeable — a conflict, a ruleset, a
    /// red required check — is its 405, and comes back as
    /// [`Merged::Refused`] in GitHub's own words, with
    /// [`CLASSIC`]: a value for whoever asked to branch on.
    ///
    /// # Errors
    ///
    /// GitHub's failure, a moved head among it.
    pub async fn merge(&self, pull: u64, judged: &str, method: Merge) -> Result<Merged> {
        let judged = judged.to_owned();
        self.ask(
            move |github, repo| match github.merge_pull(repo, pull, Some(&judged), method) {
                Err(Error::Refused {
                    status: 405,
                    message,
                }) => Ok(Merged::Refused(format!("{message} ({CLASSIC})"))),
                merged => Ok(merged.map(|()| Merged::Done)?),
            },
        )
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
    use crate::github::fake::{
        DEFAULT_BRANCH, FakeGitHub, Request, TIP, check, check_by, check_runs, required,
        required_by,
    };
    use crate::github::{Conclusion, State};
    use crate::plan::AgentSlots;
    use crate::plan::stream::received;
    use serde_json::{Value, json};
    use std::num::NonZeroUsize;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::time::timeout;

    /// Long enough for anything here that makes progress at all.
    const PATIENCE: Duration = Duration::from_secs(30);

    /// The commit Epik pushed, in these tests.
    const PUSHED: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// The app a ruleset names in these tests, and another that also
    /// reports checks.
    const NAMED: u64 = 15368;
    const OTHER: u64 = 99;

    /// A short wait: a few polls, then the bound.
    const fn wait() -> CheckWait {
        CheckWait {
            every: Duration::from_millis(5),
            bound: Duration::from_millis(40),
        }
    }

    /// A wait long enough to reach whatever verdict is coming.
    const fn patient() -> CheckWait {
        CheckWait {
            bound: PATIENCE,
            ..wait()
        }
    }

    /// A sink and what the Checks step said into it.
    fn said() -> (Sink<Happened>, UnboundedReceiver<Happened>) {
        Sink::channel()
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
    fn pull_sought(head: &str) -> Request {
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
                pull_sought("issue-3"),
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
                pull_sought("feature-7"),
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
        let merged = feature
            .merge(pull.number, PUSHED, Merge::Squash)
            .await
            .unwrap();
        assert_eq!(merged, Merged::Done);
        assert_eq!(
            fake.requests().last(),
            Some(&sent(
                "PUT",
                "/pulls/1/merge",
                json!({ "merge_method": "squash", "sha": PUSHED })
            ))
        );
        assert!(fake.world().pulls[0].merged, "the pull request merged");
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
    async fn a_merge_refused_as_not_mergeable_is_an_outcome_in_githubs_words() {
        let fake = FakeGitHub::spawn();
        let feature = feature(&fake);
        feature.create_branch().await.unwrap();
        let pull = feature.open_issue_pull(3, "issue-3").await.unwrap();
        fake.world().merge_refusal = Some("Pull Request is not mergeable".to_owned());
        let merged = feature
            .merge(pull.number, PUSHED, Merge::Squash)
            .await
            .unwrap();
        let Merged::Refused(words) = merged else {
            panic!("{merged:?}");
        };
        assert!(
            words.starts_with("Pull Request is not mergeable"),
            "{words}"
        );
        assert!(
            words.contains("classic branch protection"),
            "a rule the launch never read may be the cause: {words}"
        );
        assert!(
            !fake.world().pulls[0].merged,
            "the pull request stands open"
        );
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
    fn a_reading_is_green_only_when_every_required_check_concluded_green() {
        let rules = [required("build"), required("docs")];
        let green = || check("build", Some(Conclusion::Success));
        assert_eq!(
            Reading::of(&rules, &[green(), check("docs", Some(Conclusion::Skipped))]),
            Reading::Green
        );
        assert_eq!(
            Reading::of(&rules, &[green(), check("docs", None)]),
            Reading::Waiting("docs".to_owned()),
            "a required check still running is neither green nor red"
        );
        assert_eq!(
            Reading::of(&rules, &[green()]),
            Reading::Waiting("docs".to_owned()),
            "a required check yet to report is waited for the same way"
        );
        assert_eq!(
            Reading::of(&rules, &[green(), check("docs", Some(Conclusion::Other))]),
            Reading::Red("docs".to_owned())
        );
    }

    #[test]
    fn a_rule_naming_an_app_is_answered_by_that_apps_run_alone() {
        let rules = [required_by("CI", NAMED)];
        assert_eq!(
            Reading::of(&rules, &[check_by("CI", Some(Conclusion::Success), NAMED)]),
            Reading::Green
        );
        assert_eq!(
            Reading::of(&rules, &[check_by("CI", Some(Conclusion::Success), OTHER)]),
            Reading::Waiting("CI".to_owned()),
            "the named app has still not reported"
        );
        assert_eq!(
            Reading::of(&rules, &[check("CI", Some(Conclusion::Success))]),
            Reading::Waiting("CI".to_owned()),
            "a run outside any app is not the named app's"
        );
        assert_eq!(
            Reading::of(
                &[required("CI")],
                &[check_by("CI", Some(Conclusion::Success), OTHER)]
            ),
            Reading::Green,
            "a rule silent on the app takes a run of that name from anywhere"
        );
    }

    #[test]
    fn the_runs_no_rule_asks_for_are_named_with_what_they_came_to() {
        let checks = [
            check("build", Some(Conclusion::Success)),
            check("lint", None),
            check("audit", Some(Conclusion::Failure)),
        ];
        assert_eq!(
            unrequired(&[required("build")], &checks),
            "audit (failure), lint (running)"
        );
        assert_eq!(unrequired(&[], &checks[..1]), "build (green)");
        assert_eq!(unrequired(&[required("build")], &checks[..1]), "");
    }

    #[tokio::test]
    async fn a_policy_requiring_no_check_claims_no_verdict_without_waiting() {
        let fake = FakeGitHub::spawn();
        let (sink, mut heard) = said();
        let verdict = feature(&fake)
            .await_checks(PUSHED, &[], wait(), &sink)
            .await;
        assert_eq!(verdict.unwrap(), Verdict::Unclaimed);
        assert_eq!(fake.requests(), [], "nothing was waited for");
        assert_eq!(
            received(&mut heard),
            [Happened::Checked(Verdict::Unclaimed)]
        );
    }

    #[tokio::test]
    async fn a_green_required_check_is_green_though_an_unrequired_one_still_runs() {
        let fake = FakeGitHub::spawn();
        let runs = vec![
            check("CI", Some(Conclusion::Success)),
            check("coverage", None),
        ];
        fake.world().checks.insert(PUSHED.to_owned(), runs);
        let (sink, mut heard) = said();
        let verdict = feature(&fake)
            .await_checks(PUSHED, &[required("CI")], wait(), &sink)
            .await
            .unwrap();
        assert_eq!(verdict, Verdict::Green);
        assert_eq!(fake.requests().len(), 1, "the required check settles it");
        assert_eq!(
            received(&mut heard),
            [
                Happened::Ran("coverage (running)".to_owned()),
                Happened::Checked(Verdict::Green),
            ]
        );
    }

    #[tokio::test]
    async fn a_red_check_no_rule_asks_for_is_reported_and_blocks_nothing() {
        let fake = FakeGitHub::spawn();
        let runs = vec![
            check("CI", Some(Conclusion::Success)),
            check("audit", Some(Conclusion::Failure)),
        ];
        fake.world().checks.insert(PUSHED.to_owned(), runs);
        let (sink, mut heard) = said();
        let verdict = feature(&fake)
            .await_checks(PUSHED, &[required("CI")], wait(), &sink)
            .await
            .unwrap();
        assert_eq!(verdict, Verdict::Green);
        assert_eq!(
            received(&mut heard),
            [
                Happened::Ran("audit (failure)".to_owned()),
                Happened::Checked(Verdict::Green),
            ]
        );
    }

    #[tokio::test]
    async fn a_required_check_reported_by_another_app_does_not_satisfy_the_rule() {
        let fake = FakeGitHub::spawn();
        let runs = vec![check_by("CI", Some(Conclusion::Success), OTHER)];
        fake.world().checks.insert(PUSHED.to_owned(), runs);
        let (sink, mut heard) = said();
        let error = feature(&fake)
            .await_checks(PUSHED, &[required_by("CI", NAMED)], wait(), &sink)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("had not concluded"), "{error}");
        assert!(error.to_string().contains("CI"), "{error}");
        assert_eq!(
            received(&mut heard),
            [],
            "a wait that ran out claims no verdict"
        );
    }

    #[tokio::test]
    async fn a_required_check_that_never_reports_fails_the_wait_naming_it() {
        let fake = FakeGitHub::spawn();
        let runs = vec![check("CI", Some(Conclusion::Success))];
        fake.world().checks.insert(PUSHED.to_owned(), runs);
        let rules = [required("CI"), required("Docs")];
        let (sink, _heard) = said();
        let error = feature(&fake)
            .await_checks(PUSHED, &rules, wait(), &sink)
            .await
            .unwrap_err();
        let words = error.to_string();
        assert!(words.contains("had not concluded"), "{words}");
        assert!(words.contains("Docs"), "{words}");
        assert!(
            !words.contains("CI"),
            "the one that reported is not named: {words}"
        );
        assert!(
            fake.requests()
                .iter()
                .all(|poll| *poll == check_runs(PUSHED)),
            "every poll is the pushed commit's: {:?}",
            fake.requests()
        );
    }

    #[tokio::test]
    async fn a_commit_that_drew_no_check_run_at_all_says_so_and_reads_its_statuses() {
        let fake = FakeGitHub::spawn();
        fake.world().statuses.insert(PUSHED.to_owned(), 2);
        let (sink, _heard) = said();
        let error = feature(&fake)
            .await_checks(PUSHED, &[required("CI")], wait(), &sink)
            .await
            .unwrap_err();
        let words = error.to_string();
        assert!(words.contains("no check run appeared at all"), "{words}");
        assert!(words.contains("2 commit statuses"), "{words}");
        let requests = fake.requests();
        let (last, polls) = requests.split_last().unwrap();
        assert!(polls.len() > 1, "the wait continued: {polls:?}");
        assert_eq!(
            *last,
            get(&format!("/commits/{PUSHED}/status?per_page=1")),
            "one read of the combined status, at the end"
        );
    }

    #[tokio::test]
    async fn a_red_required_check_is_the_verdict_naming_the_rules_it_broke() {
        let fake = FakeGitHub::spawn();
        let runs = vec![
            check("build", Some(Conclusion::Success)),
            check("test", Some(Conclusion::Failure)),
            check("audit", Some(Conclusion::Other)),
        ];
        fake.world().checks.insert(PUSHED.to_owned(), runs);
        let rules = ["build", "test", "audit"].map(required);
        let (sink, mut heard) = said();
        let verdict = feature(&fake)
            .await_checks(PUSHED, &rules, wait(), &sink)
            .await
            .unwrap();
        assert_eq!(verdict, Verdict::Red("test, audit".to_owned()));
        assert_eq!(fake.requests().len(), 1, "red is read once");
        assert_eq!(
            received(&mut heard),
            [Happened::Checked(Verdict::Red("test, audit".to_owned()))]
        );
    }

    #[tokio::test]
    async fn a_required_check_whose_run_appears_late_is_waited_for() {
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
        let rules = ["build", "test"].map(required);
        let (sink, _heard) = said();
        let verdict = feature(&fake)
            .await_checks(PUSHED, &rules, patient(), &sink)
            .await
            .unwrap();
        assert_eq!(verdict, Verdict::Red("test".to_owned()));
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
        let (sink, _heard) = said();
        let verdict = feature
            .await_checks(PUSHED, &[required("test")], patient(), &sink)
            .await
            .unwrap();
        assert_eq!(verdict, Verdict::Green);
        assert!(
            fake.requests()
                .iter()
                .all(|poll| *poll == check_runs(PUSHED)),
            "every poll is the pushed commit's: {:?}",
            fake.requests()
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
        let (sink, _heard) = said();
        let waiting = tokio::spawn(async move {
            feature
                .await_checks(PUSHED, &[required("test")], patient(), &sink)
                .await
        });
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
