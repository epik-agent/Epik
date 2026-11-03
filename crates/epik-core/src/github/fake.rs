//! [`GitHub`] in memory, for other modules' tests.
//!
//! A test arranges repositories, issues, branches and pull requests,
//! hands the fake to the code under test, and reads back what the calls
//! left behind with [`FakeGitHub::repo`]. As on GitHub, a repository's
//! issues and pull requests share one sequence of numbers.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};

use super::{
    BranchUpdate, Checks, EPIC_LABEL, EPIC_LABEL_COLOR, EPIC_LABEL_DESCRIPTION, Error, GitHub,
    Issue, MergeMethod, MergeMethods, PullRequest, PullState, PullStatus, Result, Review, State,
    SubIssue, closing_pull_request,
};

/// A label's color and description.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FakeLabel {
    pub color: String,
    pub description: String,
}

/// An issue the fake holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FakeIssue {
    pub title: String,
    pub state: State,
    pub labels: BTreeSet<String>,
    /// Each sub-issue's repository and number, in order.
    pub sub_issues: Vec<(String, u64)>,
    pub blocked_by: Vec<u64>,
    pub comments: Vec<String>,
}

/// A pull request the fake holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FakePull {
    pub head: String,
    pub base: String,
    pub title: String,
    pub body: String,
    pub state: PullState,
    pub checks: Checks,
    /// How it was merged, once it is.
    pub merged_with: Option<MergeMethod>,
    /// Its merge commit, once it is merged: see [`FakeGitHub::merge_commit`].
    pub merge_commit: Option<String>,
    /// The reviews submitted on it, oldest first.
    pub reviews: Vec<Review>,
    /// How many of its review threads are unresolved.
    pub open_review_threads: usize,
    /// How many commits its base has that it doesn't. Each merge into its
    /// base adds one.
    pub behind: u64,
    /// Whether it conflicts with its base, so that it can be neither updated
    /// nor merged.
    pub conflicting: bool,
    /// Whether GitHub says it can merge although it conflicts.
    pub conflict_unnoticed: bool,
    /// How many more reads of it say GitHub is still working out whether it
    /// can merge.
    pub mergeable_unknown_reads: u32,
    /// What the next reads of its checks say, the last staying as `checks`.
    pub check_reads: VecDeque<Checks>,
    /// What [`check_reads`](Self::check_reads) becomes when its branch takes
    /// a push. When empty, its checks stay as they are.
    pub checks_after_push: Vec<Checks>,
    /// How many pushes its branch has taken, which its head commit shows.
    pub pushes: u32,
    /// How many updates of its branch GitHub has started.
    pub updates: u32,
    /// How many reads of it pass before an update lands.
    pub update_lag: u32,
    /// How many more reads of it pass before the update under way lands.
    pub update_landing_in: Option<u32>,
    /// How far behind its base it was, and what its checks said, when it
    /// merged.
    pub merged_on: Option<(u64, Checks)>,
}

impl FakePull {
    /// Takes its base into its branch and pushes, as an update or an agent
    /// does: it is caught up, conflicts no more, and is on a new head.
    pub fn take_base(&mut self) {
        self.behind = 0;
        self.conflicting = false;
        self.conflict_unnoticed = false;
        self.pushes += 1;
        self.check_reads = self.checks_after_push.iter().copied().collect();
    }
}

/// A repository the fake holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FakeRepo {
    pub default_branch: String,
    pub branches: BTreeSet<String>,
    pub labels: BTreeMap<String, FakeLabel>,
    pub issues: BTreeMap<u64, FakeIssue>,
    pub pulls: BTreeMap<u64, FakePull>,
    pub merge_methods: MergeMethods,
}

impl FakeRepo {
    fn next_number(&self) -> u64 {
        let issues = self.issues.keys().next_back().copied().unwrap_or(0);
        let pulls = self.pulls.keys().next_back().copied().unwrap_or(0);
        issues.max(pulls) + 1
    }

    fn issue(&self, number: u64) -> Result<&FakeIssue> {
        self.issues.get(&number).ok_or_else(not_found)
    }

    fn issue_mut(&mut self, number: u64) -> Result<&mut FakeIssue> {
        self.issues.get_mut(&number).ok_or_else(not_found)
    }

    fn pull_mut(&mut self, number: u64) -> Result<&mut FakePull> {
        self.pulls.get_mut(&number).ok_or_else(not_found)
    }
}

#[derive(Default)]
struct World {
    repos: BTreeMap<String, FakeRepo>,
    clones: Vec<(String, PathBuf)>,
    /// The error each named operation fails with, for as long as it is
    /// set.
    failures: BTreeMap<String, String>,
}

impl World {
    fn repo(&self, repo: &str) -> Result<&FakeRepo> {
        self.repos.get(repo).ok_or_else(not_found)
    }

    fn repo_mut(&mut self, repo: &str) -> Result<&mut FakeRepo> {
        self.repos.get_mut(repo).ok_or_else(not_found)
    }

    fn check(&self, operation: &str) -> Result<()> {
        match self.failures.get(operation) {
            Some(message) => Err(Error::new(message.clone())),
            None => Ok(()),
        }
    }
}

fn not_found() -> Error {
    Error::new("gh: Not Found (HTTP 404)")
}

/// The name of each [`GitHub`] operation, which [`FakeGitHub::fail`]
/// takes.
const OPERATIONS: [&str; 19] = [
    "issue",
    "sub_issues",
    "blocked_by",
    "default_branch",
    "ensure_branch",
    "ensure_epic_label",
    "clone",
    "find_pull_request",
    "checks",
    "merge_pull_request",
    "behind_by",
    "update_branch",
    "close_issue",
    "open_pull_request",
    "comment",
    "pull_request",
    "repo_exists",
    "reviews",
    "open_review_threads",
];

/// [`GitHub`] in memory.
#[derive(Default)]
pub struct FakeGitHub {
    world: Mutex<World>,
}

impl FakeGitHub {
    pub fn new() -> Self {
        Self::default()
    }

    fn world(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds an empty repository whose only branch is `default_branch`.
    pub fn add_repo(&self, repo: &str, default_branch: &str) {
        let fake = FakeRepo {
            default_branch: default_branch.to_owned(),
            branches: BTreeSet::from([default_branch.to_owned()]),
            labels: BTreeMap::new(),
            issues: BTreeMap::new(),
            pulls: BTreeMap::new(),
            merge_methods: MergeMethods::ALL,
        };
        self.world().repos.insert(repo.to_owned(), fake);
    }

    /// Adds branch `branch` to `repo`.
    pub fn add_branch(&self, repo: &str, branch: &str) {
        self.arrange(repo, |fake| {
            fake.branches.insert(branch.to_owned());
        });
    }

    /// Adds an open issue `number` titled `title` to `repo`.
    pub fn add_issue(&self, repo: &str, number: u64, title: &str) {
        let issue = FakeIssue {
            title: title.to_owned(),
            state: State::Open,
            labels: BTreeSet::new(),
            sub_issues: Vec::new(),
            blocked_by: Vec::new(),
            comments: Vec::new(),
        };
        self.arrange(repo, |fake| {
            fake.issues.insert(number, issue);
        });
    }

    /// Makes issue `sub` of `sub_repo` the last sub-issue of `parent`.
    pub fn add_sub_issue(&self, repo: &str, parent: u64, sub_repo: &str, sub: u64) {
        self.arrange_issue(repo, parent, |issue| {
            issue.sub_issues.push((sub_repo.to_owned(), sub));
        });
    }

    /// Records that issue `blocker` blocks issue `number`.
    pub fn add_blocker(&self, repo: &str, number: u64, blocker: u64) {
        self.arrange_issue(repo, number, |issue| issue.blocked_by.push(blocker));
    }

    /// Sets issue `number`'s state.
    pub fn set_state(&self, repo: &str, number: u64, state: State) {
        self.arrange_issue(repo, number, |issue| issue.state = state);
    }

    /// Sets the merge methods `repo` allows, all of them until this is
    /// called.
    pub fn set_merge_methods(&self, repo: &str, methods: MergeMethods) {
        self.arrange(repo, |fake| fake.merge_methods = methods);
    }

    /// Sets pull request `pr`'s checks.
    pub fn set_checks(&self, repo: &str, pr: u64, checks: Checks) {
        self.arrange_pull(repo, pr, |pull| pull.checks = checks);
    }

    /// Submits a review of pull request `pr` with `body` at
    /// `submitted_at`.
    pub fn add_review(&self, repo: &str, pr: u64, body: &str, submitted_at: DateTime<Utc>) {
        let review = Review {
            body: body.to_owned(),
            submitted_at,
        };
        self.arrange_pull(repo, pr, |pull| pull.reviews.push(review));
    }

    /// Sets how many of pull request `pr`'s review threads are unresolved.
    pub fn set_open_review_threads(&self, repo: &str, pr: u64, open: usize) {
        self.arrange_pull(repo, pr, |pull| pull.open_review_threads = open);
    }

    /// The merge commit the fake gives pull request `pr` when it merges
    /// it: 40 hex digits whose first 7 are `a` and `pr` in hex.
    pub fn merge_commit(pr: u64) -> String {
        format!("{:07x}{}", 0xa00_0000 + pr, "0".repeat(33))
    }

    /// Removes `repo`, as when it is deleted or the user loses access.
    pub fn remove_repo(&self, repo: &str) {
        self.world().repos.remove(repo);
    }

    /// Sets pull request `pr`'s state, with a merge commit when merged.
    pub fn set_pull_state(&self, repo: &str, pr: u64, state: PullState) {
        self.arrange_pull(repo, pr, |pull| {
            pull.state = state;
            pull.merge_commit = (state == PullState::Merged).then(|| Self::merge_commit(pr));
        });
    }

    /// Makes `operation`, named as the [`GitHub`] method is, fail with
    /// `message` until [`succeed`](Self::succeed) is called for it.
    pub fn fail(&self, operation: &str, message: &str) {
        assert!(OPERATIONS.contains(&operation), "no operation {operation}");
        let mut world = self.world();
        world
            .failures
            .insert(operation.to_owned(), message.to_owned());
    }

    /// Undoes [`fail`](Self::fail).
    pub fn succeed(&self, operation: &str) {
        self.world().failures.remove(operation);
    }

    /// A copy of `repo` as it stands.
    pub fn repo(&self, repo: &str) -> FakeRepo {
        self.world().repo(repo).expect("repository exists").clone()
    }

    /// Each repository cloned, and where, in order.
    pub fn clones(&self) -> Vec<(String, PathBuf)> {
        self.world().clones.clone()
    }

    fn arrange(&self, repo: &str, change: impl FnOnce(&mut FakeRepo)) {
        let mut world = self.world();
        change(world.repo_mut(repo).expect("repository exists"));
    }

    fn arrange_issue(&self, repo: &str, number: u64, change: impl FnOnce(&mut FakeIssue)) {
        self.arrange(repo, |fake| {
            change(fake.issue_mut(number).expect("issue exists"));
        });
    }

    /// Changes pull request `pr`.
    pub fn arrange_pull(&self, repo: &str, pr: u64, change: impl FnOnce(&mut FakePull)) {
        self.arrange(repo, |fake| {
            change(fake.pull_mut(pr).expect("pull request exists"));
        });
    }

    /// The world, once `operation` is allowed to proceed.
    fn enter(&self, operation: &str) -> Result<MutexGuard<'_, World>> {
        let world = self.world();
        world.check(operation)?;
        Ok(world)
    }
}

impl GitHub for FakeGitHub {
    fn issue(&self, repo: &str, number: u64) -> Result<Issue> {
        let world = self.enter("issue")?;
        let fake = world.repo(repo)?;
        if let Some(pull) = fake.pulls.get(&number) {
            let state = match pull.state {
                PullState::Open => State::Open,
                PullState::Closed | PullState::Merged => State::Closed,
            };
            return Ok(Issue {
                title: pull.title.clone(),
                state,
                is_pull_request: true,
            });
        }
        let issue = fake.issue(number)?;
        Ok(Issue {
            title: issue.title.clone(),
            state: issue.state,
            is_pull_request: false,
        })
    }

    fn sub_issues(&self, repo: &str, number: u64) -> Result<Vec<SubIssue>> {
        let world = self.enter("sub_issues")?;
        let parent = world.repo(repo)?.issue(number)?;
        parent
            .sub_issues
            .iter()
            .map(|(sub_repo, sub)| {
                let issue = world.repo(sub_repo)?.issue(*sub)?;
                Ok(SubIssue {
                    repo: sub_repo.clone(),
                    number: *sub,
                    title: issue.title.clone(),
                    state: issue.state,
                })
            })
            .collect()
    }

    fn blocked_by(&self, repo: &str, number: u64) -> Result<Vec<u64>> {
        let world = self.enter("blocked_by")?;
        let issue = world.repo(repo)?.issue(number)?;
        Ok(issue.blocked_by.clone())
    }

    fn default_branch(&self, repo: &str) -> Result<String> {
        let world = self.enter("default_branch")?;
        Ok(world.repo(repo)?.default_branch.clone())
    }

    fn ensure_branch(&self, repo: &str, branch: &str) -> Result<()> {
        let mut world = self.enter("ensure_branch")?;
        world.repo_mut(repo)?.branches.insert(branch.to_owned());
        Ok(())
    }

    fn ensure_epic_label(&self, repo: &str, number: u64) -> Result<()> {
        let mut world = self.enter("ensure_epic_label")?;
        let fake = world.repo_mut(repo)?;
        fake.issue_mut(number)?;
        fake.labels
            .entry(EPIC_LABEL.to_owned())
            .or_insert_with(|| FakeLabel {
                color: EPIC_LABEL_COLOR.to_owned(),
                description: EPIC_LABEL_DESCRIPTION.to_owned(),
            });
        fake.issue_mut(number)?.labels.insert(EPIC_LABEL.to_owned());
        Ok(())
    }

    fn clone(&self, repo: &str, dir: &Path) -> Result<()> {
        let mut world = self.enter("clone")?;
        world.repo(repo)?;
        std::fs::create_dir_all(dir).map_err(|error| Error::new(error.to_string()))?;
        world.clones.push((repo.to_owned(), dir.to_owned()));
        Ok(())
    }

    fn find_pull_request(&self, repo: &str, base: &str, issue: u64) -> Result<Option<PullRequest>> {
        let world = self.enter("find_pull_request")?;
        // Newest first, as GitHub lists them.
        let candidates = world.repo(repo)?.pulls.iter().rev();
        let candidates = candidates
            .filter(|(_, pull)| pull.base == base)
            .map(|(number, pull)| (*number, pull.state, pull.body.as_str()));
        Ok(closing_pull_request(candidates, issue))
    }

    fn checks(&self, repo: &str, pr: u64) -> Result<Checks> {
        let mut world = self.enter("checks")?;
        let pull = world.repo_mut(repo)?.pull_mut(pr)?;
        if let Some(next) = pull.check_reads.pop_front() {
            pull.checks = next;
        }
        Ok(pull.checks)
    }

    fn merge_pull_request(&self, repo: &str, pr: u64) -> Result<String> {
        let mut world = self.enter("merge_pull_request")?;
        let fake = world.repo_mut(repo)?;
        let methods = fake.merge_methods;
        let pull = fake.pull_mut(pr)?;
        match pull.state {
            PullState::Closed => Err(Error::new("gh: Pull Request is not mergeable (HTTP 405)")),
            PullState::Merged => Ok(pull.merge_commit.clone().unwrap_or_default()),
            PullState::Open if pull.conflicting => {
                Err(Error::new("gh: Pull Request is not mergeable (HTTP 405)"))
            }
            PullState::Open => {
                pull.merged_with = Some(methods.choose(repo)?);
                pull.state = PullState::Merged;
                pull.merged_on = Some((pull.behind, pull.checks));
                let sha = FakeGitHub::merge_commit(pr);
                pull.merge_commit = Some(sha.clone());
                let base = pull.base.clone();
                for other in fake.pulls.values_mut() {
                    if other.base == base && other.state == PullState::Open {
                        other.behind += 1;
                    }
                }
                Ok(sha)
            }
        }
    }

    fn behind_by(&self, repo: &str, base: &str, head: &str) -> Result<u64> {
        let world = self.enter("behind_by")?;
        let mut pulls = world.repo(repo)?.pulls.values();
        let pull = pulls
            .find(|pull| pull.head == head && pull.base == base)
            .ok_or_else(not_found)?;
        Ok(pull.behind)
    }

    fn update_branch(&self, repo: &str, pr: u64) -> Result<BranchUpdate> {
        let mut world = self.enter("update_branch")?;
        let pull = world.repo_mut(repo)?.pull_mut(pr)?;
        if pull.conflicting {
            return Ok(BranchUpdate::Conflict);
        }
        pull.updates += 1;
        if pull.update_lag == 0 {
            pull.take_base();
        } else {
            pull.update_landing_in = Some(pull.update_lag);
        }
        Ok(BranchUpdate::Started)
    }

    fn comment(&self, repo: &str, number: u64, body: &str) -> Result<()> {
        let mut world = self.enter("comment")?;
        let issue = world.repo_mut(repo)?.issue_mut(number)?;
        issue.comments.push(body.to_owned());
        Ok(())
    }

    fn pull_request(&self, repo: &str, pr: u64) -> Result<PullStatus> {
        let mut world = self.enter("pull_request")?;
        let pull = world.repo_mut(repo)?.pull_mut(pr)?;
        let status = PullStatus {
            state: pull.state,
            merge_commit: pull.merge_commit.clone(),
            head: pull.head.clone(),
            head_sha: format!("{}-{}", pull.head, pull.pushes),
            mergeable: if pull.state != PullState::Open || pull.mergeable_unknown_reads > 0 {
                None
            } else {
                Some(!pull.conflicting || pull.conflict_unnoticed)
            },
        };
        pull.mergeable_unknown_reads = pull.mergeable_unknown_reads.saturating_sub(1);
        match pull.update_landing_in {
            Some(1) => {
                pull.update_landing_in = None;
                pull.take_base();
            }
            Some(left) => pull.update_landing_in = Some(left - 1),
            None => {}
        }
        Ok(status)
    }

    fn repo_exists(&self, repo: &str) -> Result<bool> {
        let world = self.enter("repo_exists")?;
        Ok(world.repos.contains_key(repo))
    }

    fn close_issue(&self, repo: &str, number: u64, comment: &str) -> Result<()> {
        let mut world = self.enter("close_issue")?;
        let issue = world.repo_mut(repo)?.issue_mut(number)?;
        issue.comments.push(comment.to_owned());
        issue.state = State::Closed;
        Ok(())
    }

    fn open_pull_request(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<u64> {
        let mut world = self.enter("open_pull_request")?;
        let fake = world.repo_mut(repo)?;
        let standing = fake.pulls.iter().find(|(_, pull)| {
            pull.head == head && pull.base == base && pull.state == PullState::Open
        });
        if let Some((number, _)) = standing {
            return Ok(*number);
        }
        if !fake.branches.contains(head) || !fake.branches.contains(base) {
            return Err(Error::new("gh: Validation Failed (HTTP 422)"));
        }
        let number = fake.next_number();
        let pull = FakePull {
            head: head.to_owned(),
            base: base.to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            state: PullState::Open,
            checks: Checks::None,
            merged_with: None,
            merge_commit: None,
            reviews: Vec::new(),
            open_review_threads: 0,
            behind: 0,
            conflicting: false,
            conflict_unnoticed: false,
            mergeable_unknown_reads: 0,
            check_reads: VecDeque::new(),
            checks_after_push: Vec::new(),
            pushes: 0,
            updates: 0,
            update_lag: 0,
            update_landing_in: None,
            merged_on: None,
        };
        fake.pulls.insert(number, pull);
        Ok(number)
    }

    fn reviews(&self, repo: &str, pr: u64) -> Result<Vec<Review>> {
        let world = self.enter("reviews")?;
        let pull = world.repo(repo)?.pulls.get(&pr).ok_or_else(not_found)?;
        Ok(pull.reviews.clone())
    }

    fn open_review_threads(&self, repo: &str, pr: u64) -> Result<usize> {
        let world = self.enter("open_review_threads")?;
        let pull = world.repo(repo)?.pulls.get(&pr).ok_or_else(not_found)?;
        Ok(pull.open_review_threads)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO: &str = "owner/repo";

    /// A repository on `main` holding open issue 1, an epic, with
    /// sub-issues 2 and 3, 3 blocked by 2.
    fn epic() -> FakeGitHub {
        let github = FakeGitHub::new();
        github.add_repo(REPO, "main");
        github.add_issue(REPO, 1, "Epic");
        github.add_issue(REPO, 2, "First");
        github.add_issue(REPO, 3, "Second");
        github.add_sub_issue(REPO, 1, REPO, 2);
        github.add_sub_issue(REPO, 1, REPO, 3);
        github.add_blocker(REPO, 3, 2);
        github.add_branch(REPO, "epic/1");
        github.add_branch(REPO, "issue-2");
        github
    }

    #[test]
    fn reads_the_graph() {
        let github = epic();
        let subs = github.sub_issues(REPO, 1).unwrap();
        let numbers: Vec<u64> = subs.iter().map(|sub| sub.number).collect();
        assert_eq!(numbers, [2, 3]);
        assert_eq!(subs[0].title, "First");
        assert_eq!(subs[0].repo, REPO);
        assert_eq!(github.blocked_by(REPO, 3).unwrap(), [2]);
        assert_eq!(github.blocked_by(REPO, 2).unwrap(), [] as [u64; 0]);
    }

    #[test]
    fn a_sub_issue_may_live_in_another_repository() {
        let github = epic();
        github.add_repo("other/repo", "trunk");
        github.add_issue("other/repo", 9, "Elsewhere");
        github.add_sub_issue(REPO, 1, "other/repo", 9);
        let subs = github.sub_issues(REPO, 1).unwrap();
        assert_eq!(subs[2].repo, "other/repo");
        assert_eq!(subs[2].title, "Elsewhere");
    }

    #[test]
    fn a_missing_issue_is_not_found() {
        let github = epic();
        assert!(github.issue(REPO, 99).is_err());
        assert!(github.issue("no/repo", 1).is_err());
    }

    #[test]
    fn issues_and_pull_requests_share_numbers() {
        let github = epic();
        let pr = github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Closes #2")
            .unwrap();
        assert_eq!(pr, 4);
        let issue = github.issue(REPO, pr).unwrap();
        assert!(issue.is_pull_request);
        assert_eq!(issue.state, State::Open);
    }

    #[test]
    fn ensure_branch_creates_a_missing_branch_once() {
        let github = epic();
        github.ensure_branch(REPO, "epic/7").unwrap();
        github.ensure_branch(REPO, "epic/7").unwrap();
        assert!(github.repo(REPO).branches.contains("epic/7"));
    }

    #[test]
    fn the_epic_label_is_created_and_added() {
        let github = epic();
        github.ensure_epic_label(REPO, 1).unwrap();
        let repo = github.repo(REPO);
        assert_eq!(repo.labels["epic"].color, "5319E7");
        assert!(repo.issues[&1].labels.contains("epic"));
    }

    #[test]
    fn an_existing_epic_label_is_never_changed() {
        let github = epic();
        let mine = FakeLabel {
            color: "000000".into(),
            description: "Mine".into(),
        };
        github.arrange(REPO, |fake| {
            fake.labels.insert("epic".into(), mine.clone());
        });
        github.ensure_epic_label(REPO, 1).unwrap();
        assert_eq!(github.repo(REPO).labels["epic"], mine);
    }

    #[test]
    fn an_existing_open_pull_request_is_returned() {
        let github = epic();
        let first = github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Closes #2")
            .unwrap();
        let again = github
            .open_pull_request(REPO, "issue-2", "epic/1", "Again", "")
            .unwrap();
        assert_eq!(again, first);
        assert_eq!(github.repo(REPO).pulls.len(), 1);
    }

    #[test]
    fn a_pull_request_needs_both_branches() {
        let github = epic();
        let result = github.open_pull_request(REPO, "missing", "epic/1", "T", "");
        assert!(result.is_err());
    }

    #[test]
    fn finds_the_pull_request_closing_an_issue_by_its_body() {
        let github = epic();
        let pr = github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Fixes #2")
            .unwrap();
        let found = github.find_pull_request(REPO, "epic/1", 2).unwrap();
        assert_eq!(
            found,
            Some(PullRequest {
                number: pr,
                merged: false
            })
        );
        assert_eq!(github.find_pull_request(REPO, "main", 2).unwrap(), None);
        assert_eq!(github.find_pull_request(REPO, "epic/1", 3).unwrap(), None);
    }

    #[test]
    fn a_merged_pull_request_is_found_before_an_open_one() {
        let github = epic();
        let merged = github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Closes #2")
            .unwrap();
        github.merge_pull_request(REPO, merged).unwrap();
        github.add_branch(REPO, "issue-2-again");
        github
            .open_pull_request(REPO, "issue-2-again", "epic/1", "Again", "Closes #2")
            .unwrap();
        let found = github.find_pull_request(REPO, "epic/1", 2).unwrap();
        assert_eq!(
            found,
            Some(PullRequest {
                number: merged,
                merged: true
            })
        );
    }

    #[test]
    fn a_closed_pull_request_is_not_found() {
        let github = epic();
        let pr = github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Closes #2")
            .unwrap();
        github.set_pull_state(REPO, pr, PullState::Closed);
        assert_eq!(github.find_pull_request(REPO, "epic/1", 2).unwrap(), None);
        assert!(github.merge_pull_request(REPO, pr).is_err());
    }

    #[test]
    fn merging_twice_succeeds() {
        let github = epic();
        let pr = github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Closes #2")
            .unwrap();
        github.merge_pull_request(REPO, pr).unwrap();
        github.merge_pull_request(REPO, pr).unwrap();
        assert_eq!(github.repo(REPO).pulls[&pr].state, PullState::Merged);
        assert_eq!(github.issue(REPO, pr).unwrap().state, State::Closed);
    }

    /// The method a fresh pull request in a repository allowing `methods`
    /// merges with, or the merge's error.
    fn merged_with(methods: MergeMethods) -> Result<MergeMethod> {
        let github = epic();
        github.set_merge_methods(REPO, methods);
        let pr = github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Closes #2")
            .unwrap();
        github.merge_pull_request(REPO, pr)?;
        Ok(github.repo(REPO).pulls[&pr].merged_with.unwrap())
    }

    #[test]
    fn merges_with_a_squash_by_default() {
        assert_eq!(merged_with(MergeMethods::ALL), Ok(MergeMethod::Squash));
    }

    #[test]
    fn merges_with_a_method_the_repository_allows() {
        let rebase = MergeMethods {
            rebase: true,
            ..MergeMethods::NONE
        };
        assert_eq!(merged_with(rebase), Ok(MergeMethod::Rebase));
        assert!(merged_with(MergeMethods::NONE).is_err());
    }

    #[test]
    fn a_merged_pull_request_is_merged_whatever_is_allowed() {
        let github = epic();
        let pr = github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Closes #2")
            .unwrap();
        github.merge_pull_request(REPO, pr).unwrap();
        github.set_merge_methods(REPO, MergeMethods::NONE);
        github.merge_pull_request(REPO, pr).unwrap();
    }

    #[test]
    fn checks_start_as_none_and_can_be_set() {
        let github = epic();
        let pr = github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Closes #2")
            .unwrap();
        assert_eq!(github.checks(REPO, pr).unwrap(), Checks::None);
        github.set_checks(REPO, pr, Checks::Passing);
        assert_eq!(github.checks(REPO, pr).unwrap(), Checks::Passing);
    }

    /// Opens a pull request from `issue-2` into `epic/1` in [`epic`].
    fn opened(github: &FakeGitHub) -> u64 {
        github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Closes #2")
            .unwrap()
    }

    #[test]
    fn a_pull_request_falls_behind_when_another_merges_into_its_base() {
        let github = epic();
        let pr = opened(&github);
        github.add_branch(REPO, "issue-3");
        let other = github
            .open_pull_request(REPO, "issue-3", "epic/1", "Second", "Closes #3")
            .unwrap();
        assert_eq!(github.behind_by(REPO, "epic/1", "issue-2").unwrap(), 0);
        github.merge_pull_request(REPO, other).unwrap();
        assert_eq!(github.behind_by(REPO, "epic/1", "issue-2").unwrap(), 1);
        let merged_on = |pr| github.repo(REPO).pulls[&pr].merged_on;
        assert_eq!(merged_on(other), Some((0, Checks::None)));
        github.merge_pull_request(REPO, pr).unwrap();
        assert_eq!(merged_on(pr), Some((1, Checks::None)));
    }

    #[test]
    fn updating_a_branch_catches_it_up_on_a_new_head_with_new_checks() {
        let github = epic();
        let pr = opened(&github);
        github.set_checks(REPO, pr, Checks::Passing);
        github.arrange_pull(REPO, pr, |pull| {
            pull.behind = 2;
            pull.checks_after_push = vec![Checks::None, Checks::Pending];
        });
        let before = github.pull_request(REPO, pr).unwrap().head_sha;
        assert_eq!(github.update_branch(REPO, pr), Ok(BranchUpdate::Started));
        assert_eq!(github.behind_by(REPO, "epic/1", "issue-2").unwrap(), 0);
        assert_ne!(github.pull_request(REPO, pr).unwrap().head_sha, before);
        assert_eq!(github.checks(REPO, pr).unwrap(), Checks::None);
        assert_eq!(github.checks(REPO, pr).unwrap(), Checks::Pending);
        assert_eq!(github.checks(REPO, pr).unwrap(), Checks::Pending);
    }

    #[test]
    fn an_update_lands_after_some_reads_of_the_pull_request() {
        let github = epic();
        let pr = opened(&github);
        github.arrange_pull(REPO, pr, |pull| {
            pull.behind = 1;
            pull.update_lag = 2;
        });
        let before = github.pull_request(REPO, pr).unwrap().head_sha;
        github.update_branch(REPO, pr).unwrap();
        let head = || github.pull_request(REPO, pr).unwrap().head_sha;
        assert_eq!(head(), before);
        assert_eq!(head(), before);
        assert_ne!(head(), before);
    }

    #[test]
    fn a_conflicting_pull_request_cant_be_updated_or_merged() {
        let github = epic();
        let pr = opened(&github);
        github.arrange_pull(REPO, pr, |pull| pull.conflicting = true);
        let read = github.pull_request(REPO, pr).unwrap();
        assert_eq!(read.mergeable, Some(false));
        assert_eq!(github.update_branch(REPO, pr), Ok(BranchUpdate::Conflict));
        assert!(github.merge_pull_request(REPO, pr).is_err());
        github.arrange_pull(REPO, pr, FakePull::take_base);
        assert_eq!(github.pull_request(REPO, pr).unwrap().mergeable, Some(true));
        github.merge_pull_request(REPO, pr).unwrap();
    }

    #[test]
    fn a_conflict_can_go_unnoticed_until_an_update_is_refused() {
        let github = epic();
        let pr = opened(&github);
        github.arrange_pull(REPO, pr, |pull| {
            pull.conflicting = true;
            pull.conflict_unnoticed = true;
        });
        let read = github.pull_request(REPO, pr).unwrap();
        assert_eq!(read.mergeable, Some(true));
        assert_eq!(github.update_branch(REPO, pr), Ok(BranchUpdate::Conflict));
    }

    #[test]
    fn mergeable_is_unknown_for_some_reads_and_once_closed() {
        let github = epic();
        let pr = opened(&github);
        github.arrange_pull(REPO, pr, |pull| pull.mergeable_unknown_reads = 1);
        let mergeable = || github.pull_request(REPO, pr).unwrap().mergeable;
        assert_eq!(mergeable(), None);
        assert_eq!(mergeable(), Some(true));
        github.merge_pull_request(REPO, pr).unwrap();
        assert_eq!(mergeable(), None);
    }

    #[test]
    fn closing_an_issue_comments_and_closes_it() {
        let github = epic();
        github.close_issue(REPO, 2, "Merged in #4.").unwrap();
        let issue = &github.repo(REPO).issues[&2];
        assert_eq!(issue.state, State::Closed);
        assert_eq!(issue.comments, ["Merged in #4."]);
    }

    #[test]
    fn a_failing_operation_fails_until_told_to_succeed() {
        let github = epic();
        github.fail("default_branch", "gh: boom");
        let error = github.default_branch(REPO).unwrap_err();
        assert_eq!(error.message, "gh: boom");
        github.succeed("default_branch");
        assert_eq!(github.default_branch(REPO).unwrap(), "main");
    }

    #[test]
    #[should_panic(expected = "no operation merge")]
    fn only_a_real_operation_can_fail() {
        epic().fail("merge", "gh: boom");
    }

    #[test]
    fn cloning_creates_the_directory_and_records_it() {
        let github = epic();
        let dir = std::env::temp_dir()
            .join(format!("epik-fake-clone-{}", std::process::id()))
            .join("repo");
        github.clone(REPO, &dir).unwrap();
        assert!(dir.is_dir());
        assert_eq!(github.clones(), [(REPO.to_owned(), dir.clone())]);
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn reviews_and_open_threads_start_empty_and_can_be_added() {
        let github = epic();
        let pr = github
            .open_pull_request(REPO, "issue-2", "epic/1", "First", "Closes #2")
            .unwrap();
        assert_eq!(github.reviews(REPO, pr).unwrap(), []);
        assert_eq!(github.open_review_threads(REPO, pr).unwrap(), 0);
        let first: DateTime<Utc> = "2026-11-01T12:00:00Z".parse().unwrap();
        let second: DateTime<Utc> = "2026-11-01T12:05:00Z".parse().unwrap();
        github.add_review(REPO, pr, "First look.", first);
        github.add_review(REPO, pr, "Second look.", second);
        github.set_open_review_threads(REPO, pr, 2);
        let bodies: Vec<String> = github
            .reviews(REPO, pr)
            .unwrap()
            .into_iter()
            .map(|review| review.body)
            .collect();
        assert_eq!(bodies, ["First look.", "Second look."]);
        assert_eq!(github.open_review_threads(REPO, pr).unwrap(), 2);
        assert!(github.reviews(REPO, 99).is_err());
        assert!(github.open_review_threads(REPO, 99).is_err());
    }
}
