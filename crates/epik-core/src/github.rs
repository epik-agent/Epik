//! The GitHub operations a build needs.
//!
//! A build reaches GitHub only through the [`GitHub`] trait. [`GhCli`] is
//! the real one, running the `gh` CLI, which is already signed in.
//! [`FakeGitHub`] holds repositories in memory for other modules' tests.
//!
//! Repositories are named `owner/name`.

mod fake;
mod gh;

pub use fake::FakeGitHub;
pub use gh::GhCli;

use std::fmt;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// The `gh` program to run.
pub fn gh_program() -> std::path::PathBuf {
    std::path::PathBuf::from("gh")
}

/// The name of the label an epic carries.
pub const EPIC_LABEL: &str = "epic";
/// The color [`EPIC_LABEL`] is created with.
pub const EPIC_LABEL_COLOR: &str = "5319E7";
/// The description [`EPIC_LABEL`] is created with.
pub const EPIC_LABEL_DESCRIPTION: &str = "A dependency-ordered set of issues";

/// A GitHub operation that failed. For [`GhCli`], the message is `gh`'s
/// stderr.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    pub message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Whether an issue is open or closed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Open,
    Closed,
}

/// An issue, which GitHub also uses for every pull request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Issue {
    pub title: String,
    pub state: State,
    pub is_pull_request: bool,
}

/// One sub-issue of an issue, which may live in another repository.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubIssue {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub state: State,
}

/// A pull request that closes an issue: open, or already merged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PullRequest {
    pub number: u64,
    pub merged: bool,
}

/// Where a pull request stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullState {
    Open,
    Closed,
    Merged,
}

/// The pull request among `candidates` (number, state and body) whose
/// body closes `issue`: the first merged one, else the first open one. A
/// closed one never counts.
pub fn closing_pull_request<'a>(
    candidates: impl IntoIterator<Item = (u64, PullState, &'a str)> + Clone,
    issue: u64,
) -> Option<PullRequest> {
    let first = |wanted: PullState| {
        candidates
            .clone()
            .into_iter()
            .find(|(_, state, body)| *state == wanted && closes(body, issue))
            .map(|(number, _, _)| PullRequest {
                number,
                merged: wanted == PullState::Merged,
            })
    };
    first(PullState::Merged).or_else(|| first(PullState::Open))
}

/// A pull request's state, its merge commit once it has merged, and the
/// branch it is on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullStatus {
    pub state: PullState,
    pub merge_commit: Option<String>,
    pub head: String,
}

/// How a pull request is merged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeMethod {
    /// With a merge commit.
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    /// The REST API's `merge_method`.
    pub fn as_str(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }
}

/// The merge methods a repository allows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MergeMethods {
    pub merge_commit: bool,
    pub squash: bool,
    pub rebase: bool,
}

impl MergeMethods {
    /// Every method, as GitHub allows by default.
    pub const ALL: Self = Self {
        merge_commit: true,
        squash: true,
        rebase: true,
    };

    /// No method at all.
    pub const NONE: Self = Self {
        merge_commit: false,
        squash: false,
        rebase: false,
    };

    /// The first allowed of merge commit, squash and rebase, or an error
    /// naming `repo` when none is.
    pub fn choose(self, repo: &str) -> Result<MergeMethod> {
        [
            (self.merge_commit, MergeMethod::Merge),
            (self.squash, MergeMethod::Squash),
            (self.rebase, MergeMethod::Rebase),
        ]
        .into_iter()
        .find_map(|(allowed, method)| allowed.then_some(method))
        .ok_or_else(|| Error::new(format!("{repo} allows no merge method")))
    }
}

/// What a pull request's checks add up to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Checks {
    /// Every check finished, and none failed.
    Passing,
    /// None failed, and some are still to finish.
    Pending,
    /// At least one failed.
    Failing,
    /// There are no checks at all.
    None,
}

/// A review submitted on a pull request: its summary, and when GitHub
/// says it was submitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Review {
    pub body: String,
    pub submitted_at: DateTime<Utc>,
}

/// The GitHub operations a build needs. Synchronous: a build calls them
/// from its own thread.
pub trait GitHub: Send + Sync {
    /// Issue `number`'s title and state, and whether it is a pull request.
    fn issue(&self, repo: &str, number: u64) -> Result<Issue>;

    /// Issue `number`'s sub-issues, in GitHub's order.
    fn sub_issues(&self, repo: &str, number: u64) -> Result<Vec<SubIssue>>;

    /// The numbers of the issues blocking issue `number`.
    fn blocked_by(&self, repo: &str, number: u64) -> Result<Vec<u64>>;

    /// The repository's default branch.
    fn default_branch(&self, repo: &str) -> Result<String>;

    /// Creates `branch` from the default branch's head if it is missing.
    fn ensure_branch(&self, repo: &str, branch: &str) -> Result<()>;

    /// Creates the [`EPIC_LABEL`] if the repository lacks it, never
    /// changing an existing one, and adds it to issue `number`.
    fn ensure_epic_label(&self, repo: &str, number: u64) -> Result<()>;

    /// Clones the repository into `dir`.
    fn clone(&self, repo: &str, dir: &Path) -> Result<()>;

    /// The open or merged pull request into `base` whose body closes
    /// `issue`, preferring a merged one. GitHub links pull requests to
    /// issues only for the default branch, so the body is the link.
    fn find_pull_request(&self, repo: &str, base: &str, issue: u64) -> Result<Option<PullRequest>>;

    /// What pull request `pr`'s checks add up to.
    fn checks(&self, repo: &str, pr: u64) -> Result<Checks>;

    /// Merges pull request `pr` with the first method the repository
    /// allows of merge commit, squash and rebase, failing when it allows
    /// none, and returns the merge commit's SHA. An already-merged pull
    /// request is success.
    fn merge_pull_request(&self, repo: &str, pr: u64) -> Result<String>;

    /// Comments `body` on issue `number`.
    fn comment(&self, repo: &str, number: u64, body: &str) -> Result<()>;

    /// Pull request `pr`'s state and merge commit.
    fn pull_request(&self, repo: &str, pr: u64) -> Result<PullStatus>;

    /// Whether the repository exists for the user. GitHub answers "not
    /// found" both for a repository that is gone and for one the user can
    /// no longer reach, so this can't tell them apart.
    fn repo_exists(&self, repo: &str) -> Result<bool>;

    /// Comments `comment` on issue `number` and closes it.
    fn close_issue(&self, repo: &str, number: u64, comment: &str) -> Result<()>;

    /// Opens a pull request from `head` into `base` and returns its
    /// number; when an open one from `head` into `base` already exists,
    /// returns that one.
    fn open_pull_request(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<u64>;

    /// The reviews submitted on pull request `pr`, oldest first. A review
    /// still pending, which only its author can see, is not among them.
    fn reviews(&self, repo: &str, pr: u64) -> Result<Vec<Review>>;

    /// How many of pull request `pr`'s review threads are unresolved.
    fn open_review_threads(&self, repo: &str, pr: u64) -> Result<usize>;
}

/// The keywords by which a pull request's body closes an issue.
const CLOSING_KEYWORDS: [&str; 9] = [
    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
];

/// Whether `body` closes issue `issue` of the pull request's own
/// repository: a closing keyword, in any case, then an optional colon,
/// whitespace, and `#<issue>`.
pub fn closes(body: &str, issue: u64) -> bool {
    let body = body.to_ascii_lowercase();
    let wanted = issue.to_string();
    let word_ends = |at: usize| {
        body[..at]
            .chars()
            .next_back()
            .is_none_or(|before| !before.is_alphanumeric())
    };
    CLOSING_KEYWORDS.iter().any(|keyword| {
        body.match_indices(keyword).any(|(at, _)| {
            if !word_ends(at) {
                return false;
            }
            let rest = &body[at + keyword.len()..];
            let rest = rest.strip_prefix(':').unwrap_or(rest);
            let separated = rest.trim_start();
            if separated.len() == rest.len() {
                return false;
            }
            separated
                .strip_prefix('#')
                .and_then(|rest| rest.strip_prefix(wanted.as_str()))
                .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_digit()))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_keyword_closes() {
        for keyword in CLOSING_KEYWORDS {
            assert!(closes(&format!("{keyword} #12"), 12), "{keyword}");
        }
    }

    #[test]
    fn keywords_are_case_insensitive() {
        assert!(closes("Closes #12", 12));
        assert!(closes("FIXES #12", 12));
        assert!(closes("ReSoLvEd #12", 12));
    }

    #[test]
    fn the_keyword_may_be_anywhere_in_the_body() {
        assert!(closes("Some summary.\n\nCloses #12\n\nMore.", 12));
        assert!(closes("This fixes #12.", 12));
        assert!(closes("(resolves #12)", 12));
    }

    #[test]
    fn a_colon_may_follow_the_keyword() {
        assert!(closes("Closes: #12", 12));
    }

    #[test]
    fn another_issue_number_does_not_close() {
        assert!(!closes("Closes #13", 12));
        assert!(!closes("Closes #123", 12));
        assert!(!closes("Closes #1", 12));
    }

    #[test]
    fn a_mention_without_a_keyword_does_not_close() {
        assert!(!closes("See #12", 12));
        assert!(!closes("#12", 12));
        assert!(!closes("", 12));
    }

    #[test]
    fn a_keyword_inside_another_word_does_not_close() {
        assert!(!closes("prefixes #12", 12));
        assert!(!closes("unresolved #12", 12));
        assert!(!closes("closest #12", 12));
    }

    #[test]
    fn the_keyword_and_number_must_be_separated() {
        assert!(!closes("closes#12", 12));
        assert!(!closes("closes 12", 12));
    }

    #[test]
    fn a_merged_pull_request_is_preferred_to_an_open_one() {
        let candidates = [
            (2, PullState::Open, "Fixes #9"),
            (1, PullState::Merged, "Fixes #9"),
        ];
        let found = closing_pull_request(candidates, 9);
        let merged = PullRequest {
            number: 1,
            merged: true,
        };
        assert_eq!(found, Some(merged));
    }

    #[test]
    fn an_open_pull_request_is_found_without_a_merged_one() {
        let candidates = [
            (3, PullState::Open, "Fixes #8"),
            (2, PullState::Open, "Fixes #9"),
        ];
        let found = closing_pull_request(candidates, 9);
        let open = PullRequest {
            number: 2,
            merged: false,
        };
        assert_eq!(found, Some(open));
    }

    #[test]
    fn a_closed_pull_request_does_not_count() {
        let candidates = [(1, PullState::Closed, "Fixes #9")];
        assert_eq!(closing_pull_request(candidates, 9), None);
    }

    #[test]
    fn a_merge_commit_is_preferred_to_squash_and_rebase() {
        assert_eq!(MergeMethods::ALL.choose("o/r"), Ok(MergeMethod::Merge));
    }

    #[test]
    fn squash_is_preferred_to_rebase() {
        let both = MergeMethods {
            squash: true,
            rebase: true,
            ..MergeMethods::NONE
        };
        assert_eq!(both.choose("o/r"), Ok(MergeMethod::Squash));
    }

    #[test]
    fn each_method_allowed_alone_is_chosen() {
        let merge_commit = MergeMethods {
            merge_commit: true,
            ..MergeMethods::NONE
        };
        let squash = MergeMethods {
            squash: true,
            ..MergeMethods::NONE
        };
        let rebase = MergeMethods {
            rebase: true,
            ..MergeMethods::NONE
        };
        assert_eq!(merge_commit.choose("o/r"), Ok(MergeMethod::Merge));
        assert_eq!(squash.choose("o/r"), Ok(MergeMethod::Squash));
        assert_eq!(rebase.choose("o/r"), Ok(MergeMethod::Rebase));
    }

    #[test]
    fn a_repository_allowing_no_method_is_an_error() {
        let error = MergeMethods::NONE.choose("o/r").unwrap_err();
        assert_eq!(error.message, "o/r allows no merge method");
    }

    #[test]
    fn any_of_several_references_closes() {
        assert!(closes("Closes #11, closes #12", 12));
        assert!(closes("See #12. Fixes #12", 12));
    }
}
