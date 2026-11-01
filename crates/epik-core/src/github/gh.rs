//! [`GitHub`] through the `gh` CLI, mostly `gh api`.
//!
//! Each operation builds `gh`'s arguments and parses what `gh` prints.
//! The tests parse responses recorded from `epik-agent/Epik` in
//! `fixtures/`, trimmed to the fields that matter, and run the operations
//! against a scripted `gh`. The `checks_*` fixtures other than
//! `checks_passing.json` are the recorded shape with other states, as are
//! the marker and pending review in `reviews.json` and the unresolved
//! threads in `review_threads.json`.

use std::path::Path;
use std::process::Command;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::{DeserializeOwned, IgnoredAny};

use super::{
    Checks, EPIC_LABEL, EPIC_LABEL_COLOR, EPIC_LABEL_DESCRIPTION, Error, GitHub, Issue,
    MergeMethods, PullRequest, PullState, PullStatus, Result, Review, State, SubIssue,
    closing_pull_request,
};

/// Runs `gh` with some arguments and returns its stdout.
type Run = dyn Fn(&[&str]) -> Result<String> + Send + Sync;

/// [`GitHub`] through the `gh` CLI, which must be signed in.
pub struct GhCli {
    run: Box<Run>,
}

impl Default for GhCli {
    fn default() -> Self {
        Self::new()
    }
}

impl GhCli {
    /// Runs the `gh` on the `PATH`.
    pub fn new() -> Self {
        Self {
            run: Box::new(run_gh),
        }
    }

    fn gh<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        (self.run)(&args)
    }

    fn json<T: DeserializeOwned, const N: usize>(&self, args: [&str; N]) -> Result<T> {
        parse(&self.gh(args)?)
    }

    /// Every item of the list at REST `path`, which may carry a query,
    /// across all its pages.
    fn list<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        let joint = if path.contains('?') { '&' } else { '?' };
        let path = format!("{path}{joint}per_page=100");
        parse_pages(&self.gh(["api", "--paginate", "--slurp", &path])?)
    }

    fn repo(&self, repo: &str) -> Result<RawRepo> {
        self.json(["api", &format!("repos/{repo}")])
    }

    /// The commit `branch` stands on, if it exists. Reading the refs that
    /// start with its name answers a missing branch with an empty list
    /// rather than an error.
    fn head_of(&self, repo: &str, branch: &str) -> Result<Option<String>> {
        let refs = self.list(&format!("repos/{repo}/git/matching-refs/heads/{branch}"))?;
        Ok(head_among(refs, branch))
    }
}

impl GitHub for GhCli {
    fn issue(&self, repo: &str, number: u64) -> Result<Issue> {
        let raw: RawIssue = self.json(["api", &format!("repos/{repo}/issues/{number}")])?;
        Ok(raw.into())
    }

    fn sub_issues(&self, repo: &str, number: u64) -> Result<Vec<SubIssue>> {
        let raw: Vec<RawIssue> = self.list(&format!("repos/{repo}/issues/{number}/sub_issues"))?;
        raw.into_iter().map(RawIssue::into_sub_issue).collect()
    }

    fn blocked_by(&self, repo: &str, number: u64) -> Result<Vec<u64>> {
        let path = format!("repos/{repo}/issues/{number}/dependencies/blocked_by");
        let raw: Vec<RawIssue> = self.list(&path)?;
        Ok(raw.into_iter().map(|issue| issue.number).collect())
    }

    fn default_branch(&self, repo: &str) -> Result<String> {
        Ok(self.repo(repo)?.default_branch)
    }

    fn ensure_branch(&self, repo: &str, branch: &str) -> Result<()> {
        if self.head_of(repo, branch)?.is_some() {
            return Ok(());
        }
        let default = self.default_branch(repo)?;
        let sha = self
            .head_of(repo, &default)?
            .ok_or_else(|| Error::new(format!("{repo} has no branch {default}")))?;
        self.gh([
            "api",
            "-X",
            "POST",
            &format!("repos/{repo}/git/refs"),
            "-f",
            &format!("ref=refs/heads/{branch}"),
            "-f",
            &format!("sha={sha}"),
        ])?;
        Ok(())
    }

    fn ensure_epic_label(&self, repo: &str, number: u64) -> Result<()> {
        let labels: Vec<RawLabel> = self.list(&format!("repos/{repo}/labels"))?;
        // Label names are case-insensitive on GitHub.
        let present = labels
            .iter()
            .any(|label| label.name.eq_ignore_ascii_case(EPIC_LABEL));
        if !present {
            self.gh([
                "api",
                "-X",
                "POST",
                &format!("repos/{repo}/labels"),
                "-f",
                &format!("name={EPIC_LABEL}"),
                "-f",
                &format!("color={EPIC_LABEL_COLOR}"),
                "-f",
                &format!("description={EPIC_LABEL_DESCRIPTION}"),
            ])?;
        }
        self.gh([
            "api",
            "-X",
            "POST",
            &format!("repos/{repo}/issues/{number}/labels"),
            "-f",
            &format!("labels[]={EPIC_LABEL}"),
        ])?;
        Ok(())
    }

    fn clone(&self, repo: &str, dir: &Path) -> Result<()> {
        let dir = dir
            .to_str()
            .ok_or_else(|| Error::new(format!("{} is not UTF-8", dir.display())))?;
        self.gh(["repo", "clone", repo, dir])?;
        Ok(())
    }

    fn find_pull_request(&self, repo: &str, base: &str, issue: u64) -> Result<Option<PullRequest>> {
        let pulls: Vec<RawPull> =
            self.list(&format!("repos/{repo}/pulls?base={base}&state=all"))?;
        let candidates = pulls.iter().map(|pull| {
            (
                pull.number,
                pull.state(),
                pull.body.as_deref().unwrap_or_default(),
            )
        });
        Ok(closing_pull_request(candidates, issue))
    }

    fn checks(&self, repo: &str, pr: u64) -> Result<Checks> {
        let rollup: Rollup = self.json([
            "pr",
            "view",
            &pr.to_string(),
            "--repo",
            repo,
            "--json",
            "statusCheckRollup",
        ])?;
        Ok(rollup.checks())
    }

    fn merge_pull_request(&self, repo: &str, pr: u64) -> Result<String> {
        let pull: RawPull = self.json(["api", &format!("repos/{repo}/pulls/{pr}")])?;
        if pull.state() == PullState::Merged {
            return pull
                .merge_commit_sha
                .ok_or_else(|| Error::new(format!("#{pr} is merged but has no merge commit")));
        }
        let method = self.repo(repo)?.merge_methods().choose(repo)?;
        let merged: Merged = self.json([
            "api",
            "-X",
            "PUT",
            &format!("repos/{repo}/pulls/{pr}/merge"),
            "-f",
            &format!("merge_method={}", method.as_str()),
        ])?;
        Ok(merged.sha)
    }

    fn comment(&self, repo: &str, number: u64, body: &str) -> Result<()> {
        self.gh([
            "api",
            "-X",
            "POST",
            &format!("repos/{repo}/issues/{number}/comments"),
            "-f",
            &format!("body={body}"),
        ])?;
        Ok(())
    }

    fn pull_request(&self, repo: &str, pr: u64) -> Result<PullStatus> {
        let pull: RawPull = self.json(["api", &format!("repos/{repo}/pulls/{pr}")])?;
        Ok(PullStatus {
            state: pull.state(),
            merge_commit: pull.merge_commit_sha.filter(|_| pull.merged_at.is_some()),
        })
    }

    fn repo_exists(&self, repo: &str) -> Result<bool> {
        match self.repo(repo) {
            Ok(_) => Ok(true),
            Err(error) if error.message.contains("(HTTP 404)") => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn close_issue(&self, repo: &str, number: u64, comment: &str) -> Result<()> {
        self.gh([
            "api",
            "-X",
            "POST",
            &format!("repos/{repo}/issues/{number}/comments"),
            "-f",
            &format!("body={comment}"),
        ])?;
        self.gh([
            "api",
            "-X",
            "PATCH",
            &format!("repos/{repo}/issues/{number}"),
            "-f",
            "state=closed",
            "-f",
            "state_reason=completed",
        ])?;
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
        let owner = repo.split('/').next().unwrap_or_default();
        let path = format!("repos/{repo}/pulls?head={owner}:{head}&base={base}&state=open");
        let open: Vec<RawPull> = self.list(&path)?;
        if let Some(pull) = open.first() {
            return Ok(pull.number);
        }
        let created: RawPull = self.json([
            "api",
            "-X",
            "POST",
            &format!("repos/{repo}/pulls"),
            "-f",
            &format!("head={head}"),
            "-f",
            &format!("base={base}"),
            "-f",
            &format!("title={title}"),
            "-f",
            &format!("body={body}"),
        ])?;
        Ok(created.number)
    }

    fn reviews(&self, repo: &str, pr: u64) -> Result<Vec<Review>> {
        let raw: Vec<RawReview> = self.list(&format!("repos/{repo}/pulls/{pr}/reviews"))?;
        Ok(raw.into_iter().filter_map(RawReview::submitted).collect())
    }

    fn open_review_threads(&self, repo: &str, pr: u64) -> Result<usize> {
        let (owner, name) = repo
            .split_once('/')
            .ok_or_else(|| Error::new(format!("{repo} is not owner/name")))?;
        let pages: Vec<ThreadsPage> = self.json([
            "api",
            "graphql",
            "--paginate",
            "--slurp",
            "-f",
            &format!("query={REVIEW_THREADS_QUERY}"),
            "-F",
            &format!("owner={owner}"),
            "-F",
            &format!("name={name}"),
            "-F",
            &format!("number={pr}"),
        ])?;
        Ok(unresolved(pages))
    }
}

/// Runs the `gh` on the `PATH`, failing with its stderr.
fn run_gh(args: &[&str]) -> Result<String> {
    let output = Command::new(super::gh_program())
        .args(args)
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::new("gh is not installed. Install it from https://cli.github.com/")
            } else {
                Error::new(format!("gh could not start: {error}"))
            }
        })?;
    if output.status.success() {
        return String::from_utf8(output.stdout)
            .map_err(|_| Error::new(format!("gh {} printed non-UTF-8", args.join(" "))));
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(Error::new(if stderr.is_empty() {
        format!("gh {} failed with {}", args.join(" "), output.status)
    } else {
        stderr
    }))
}

fn parse<T: DeserializeOwned>(json: &str) -> Result<T> {
    serde_json::from_str(json).map_err(|error| Error::new(format!("unexpected gh output: {error}")))
}

/// The items of `gh api --paginate --slurp`'s array of pages.
fn parse_pages<T: DeserializeOwned>(json: &str) -> Result<Vec<T>> {
    let pages: Vec<Vec<T>> = parse(json)?;
    Ok(pages.into_iter().flatten().collect())
}

/// The commit `branch` stands on, from the refs whose names start with
/// it.
fn head_among(refs: Vec<RawRef>, branch: &str) -> Option<String> {
    let wanted = format!("refs/heads/{branch}");
    refs.into_iter()
        .find(|found| found.name == wanted)
        .map(|found| found.object.sha)
}

/// What merging a pull request answers.
#[derive(Deserialize)]
struct Merged {
    sha: String,
}

#[derive(Deserialize)]
struct RawIssue {
    number: u64,
    title: String,
    state: State,
    #[serde(default)]
    repository_url: String,
    /// Present only on a pull request.
    pull_request: Option<IgnoredAny>,
}

impl From<RawIssue> for Issue {
    fn from(raw: RawIssue) -> Self {
        Issue {
            title: raw.title,
            state: raw.state,
            is_pull_request: raw.pull_request.is_some(),
        }
    }
}

impl RawIssue {
    fn into_sub_issue(self) -> Result<SubIssue> {
        Ok(SubIssue {
            repo: repo_of(&self.repository_url)?,
            number: self.number,
            title: self.title,
            state: self.state,
        })
    }
}

/// `owner/name` from a repository's API URL, which ends with them.
fn repo_of(url: &str) -> Result<String> {
    let mut segments = url.trim_end_matches('/').rsplit('/');
    match (segments.next(), segments.next()) {
        (Some(name), Some(owner)) if !name.is_empty() && !owner.is_empty() => {
            Ok(format!("{owner}/{name}"))
        }
        _ => Err(Error::new(format!("unexpected repository URL: {url}"))),
    }
}

/// A repository as the REST API returns it. GitHub reports the merge
/// methods only to those who may merge; when it doesn't, they take
/// GitHub's default, allowed.
#[derive(Deserialize)]
struct RawRepo {
    default_branch: String,
    #[serde(default = "allowed")]
    allow_merge_commit: bool,
    #[serde(default = "allowed")]
    allow_squash_merge: bool,
    #[serde(default = "allowed")]
    allow_rebase_merge: bool,
}

fn allowed() -> bool {
    true
}

impl RawRepo {
    fn merge_methods(&self) -> MergeMethods {
        MergeMethods {
            merge_commit: self.allow_merge_commit,
            squash: self.allow_squash_merge,
            rebase: self.allow_rebase_merge,
        }
    }
}

#[derive(Deserialize)]
struct RawRef {
    #[serde(rename = "ref")]
    name: String,
    object: RawObject,
}

#[derive(Deserialize)]
struct RawObject {
    sha: String,
}

#[derive(Deserialize)]
struct RawLabel {
    name: String,
}

/// A pull request as the REST API returns it.
#[derive(Deserialize)]
struct RawPull {
    number: u64,
    state: State,
    /// Set only once the pull request is merged.
    merged_at: Option<IgnoredAny>,
    #[serde(default)]
    merge_commit_sha: Option<String>,
    body: Option<String>,
}

impl RawPull {
    fn state(&self) -> PullState {
        match (self.state, &self.merged_at) {
            (State::Open, _) => PullState::Open,
            (State::Closed, Some(_)) => PullState::Merged,
            (State::Closed, None) => PullState::Closed,
        }
    }
}

/// A review as the REST API returns it. One still pending has no
/// submission time.
#[derive(Deserialize)]
struct RawReview {
    #[serde(default)]
    body: String,
    submitted_at: Option<DateTime<Utc>>,
}

impl RawReview {
    fn submitted(self) -> Option<Review> {
        Some(Review {
            body: self.body,
            submitted_at: self.submitted_at?,
        })
    }
}

/// A pull request's review threads, a page at a time: `gh api graphql
/// --paginate` fills in `$endCursor` from each page's `pageInfo`.
const REVIEW_THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $endCursor: String) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { reviewThreads(first: 100, after: $endCursor) { nodes { isResolved } pageInfo { hasNextPage endCursor } } } } }";

/// One page of [`REVIEW_THREADS_QUERY`]'s reply.
#[derive(Deserialize)]
struct ThreadsPage {
    data: ThreadsData,
}

#[derive(Deserialize)]
struct ThreadsData {
    repository: ThreadsRepository,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsRepository {
    pull_request: ThreadsPullRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsPullRequest {
    review_threads: Threads,
}

#[derive(Deserialize)]
struct Threads {
    nodes: Vec<Thread>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Thread {
    is_resolved: bool,
}

/// How many threads across `pages` are unresolved.
fn unresolved(pages: Vec<ThreadsPage>) -> usize {
    pages
        .iter()
        .flat_map(|page| &page.data.repository.pull_request.review_threads.nodes)
        .filter(|thread| !thread.is_resolved)
        .count()
}

#[derive(Deserialize)]
struct Rollup {
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Vec<RollupItem>,
}

/// One check run or commit status, as `gh pr view --json
/// statusCheckRollup` prints it.
#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum RollupItem {
    CheckRun {
        #[serde(default)]
        status: String,
        #[serde(default)]
        conclusion: String,
    },
    StatusContext {
        #[serde(default)]
        state: String,
    },
}

impl RollupItem {
    fn checks(&self) -> Checks {
        match self {
            RollupItem::CheckRun { status, conclusion } => match status.as_str() {
                "COMPLETED" => match conclusion.as_str() {
                    "SUCCESS" | "NEUTRAL" | "SKIPPED" => Checks::Passing,
                    _ => Checks::Failing,
                },
                _ => Checks::Pending,
            },
            RollupItem::StatusContext { state } => match state.as_str() {
                "SUCCESS" => Checks::Passing,
                "PENDING" | "EXPECTED" => Checks::Pending,
                _ => Checks::Failing,
            },
        }
    }
}

impl Rollup {
    fn checks(&self) -> Checks {
        let each: Vec<Checks> = self
            .status_check_rollup
            .iter()
            .map(RollupItem::checks)
            .collect();
        if each.is_empty() {
            Checks::None
        } else if each.contains(&Checks::Failing) {
            Checks::Failing
        } else if each.contains(&Checks::Pending) {
            Checks::Pending
        } else {
            Checks::Passing
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::*;

    const REPO: &str = "epik-agent/Epik";

    fn fixture(name: &str) -> String {
        let path = format!("{}/src/github/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
    }

    /// A `gh` that expects exactly `script`'s calls, in order, answering
    /// each with its reply, and the calls it received.
    fn scripted(
        script: Vec<(Vec<&'static str>, Result<String>)>,
    ) -> (GhCli, Arc<Mutex<Vec<String>>>) {
        let script = Mutex::new(VecDeque::from(script));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        let run = move |args: &[&str]| {
            seen.lock().unwrap().push(args.join(" "));
            let (expected, reply) = script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected gh {}", args.join(" ")));
            assert_eq!(args, expected.as_slice());
            reply
        };
        let gh = GhCli { run: Box::new(run) };
        (gh, calls)
    }

    fn refs(json: &str) -> Vec<RawRef> {
        parse_pages(json).unwrap()
    }

    fn ok(json: &str) -> Result<String> {
        Ok(json.to_owned())
    }

    #[test]
    fn parses_an_issue() {
        let issue: Issue = parse::<RawIssue>(&fixture("issue.json")).unwrap().into();
        assert_eq!(
            issue,
            Issue {
                title: "GitHub operations for the Epik app, through gh".into(),
                state: State::Open,
                is_pull_request: false,
            }
        );
    }

    #[test]
    fn parses_a_pull_request_read_as_an_issue() {
        let raw = fixture("issue_pull_request.json");
        let issue: Issue = parse::<RawIssue>(&raw).unwrap().into();
        assert_eq!(issue.state, State::Closed);
        assert!(issue.is_pull_request);
    }

    #[test]
    fn parses_sub_issues() {
        let raw: Vec<RawIssue> = parse_pages(&fixture("sub_issues.json")).unwrap();
        let subs: Vec<SubIssue> = raw
            .into_iter()
            .map(|raw| raw.into_sub_issue().unwrap())
            .collect();
        assert_eq!(subs.len(), 10);
        assert_eq!(
            subs[0],
            SubIssue {
                repo: REPO.into(),
                number: 357,
                title: "Rust workspace and CI for the Epik app".into(),
                state: State::Closed,
            }
        );
        assert_eq!(subs[9].number, 366);
        assert_eq!(subs[9].state, State::Open);
    }

    #[test]
    fn parses_blockers() {
        let raw: Vec<RawIssue> = parse_pages(&fixture("blocked_by.json")).unwrap();
        let numbers: Vec<u64> = raw.iter().map(|issue| issue.number).collect();
        assert_eq!(numbers, [358, 359, 360, 361]);
    }

    #[test]
    fn pages_are_flattened_in_order() {
        let numbers: Vec<u64> = parse_pages("[[1, 2], [3]]").unwrap();
        assert_eq!(numbers, [1, 2, 3]);
    }

    #[test]
    fn parses_a_default_branch() {
        let repo: RawRepo = parse(&fixture("repo.json")).unwrap();
        assert_eq!(repo.default_branch, "main");
    }

    #[test]
    fn parses_a_branch_head() {
        let head = head_among(refs(&fixture("matching_refs.json")), "daemon");
        assert_eq!(
            head.as_deref(),
            Some("dd1288cc693368c025b69618edc4bc603c07d301")
        );
    }

    #[test]
    fn a_branch_only_prefixing_another_has_no_head() {
        assert_eq!(
            head_among(refs(&fixture("matching_refs.json")), "dae"),
            None
        );
        assert_eq!(head_among(refs("[[]]"), "daemon"), None);
    }

    #[test]
    fn parses_labels() {
        let labels: Vec<RawLabel> = parse_pages(&fixture("labels.json")).unwrap();
        assert!(labels.iter().any(|label| label.name == "epic"));
    }

    #[test]
    fn parses_a_pull_request() {
        let pull: RawPull = parse(&fixture("pull.json")).unwrap();
        assert_eq!(pull.number, 367);
        assert_eq!(pull.state(), PullState::Merged);
    }

    #[test]
    fn parses_pull_requests_into_a_base() {
        let pulls: Vec<RawPull> = parse_pages(&fixture("pulls.json")).unwrap();
        let states: Vec<(u64, PullState)> = pulls
            .iter()
            .map(|pull| (pull.number, pull.state()))
            .collect();
        assert_eq!(
            states,
            [
                (369, PullState::Open),
                (368, PullState::Merged),
                (367, PullState::Merged)
            ]
        );
    }

    #[test]
    fn an_unmerged_closed_pull_request_is_closed() {
        let pull: RawPull =
            parse(r#"{"number": 1, "state": "closed", "merged_at": null, "body": null}"#).unwrap();
        assert_eq!(pull.state(), PullState::Closed);
    }

    #[test]
    fn checks_add_up() {
        let checks = |name| parse::<Rollup>(&fixture(name)).unwrap().checks();
        assert_eq!(checks("checks_passing.json"), Checks::Passing);
        assert_eq!(checks("checks_pending.json"), Checks::Pending);
        assert_eq!(checks("checks_failing.json"), Checks::Failing);
        assert_eq!(checks("checks_status_error.json"), Checks::Failing);
        assert_eq!(checks("checks_none.json"), Checks::None);
    }

    #[test]
    fn repo_comes_from_the_repository_url() {
        assert_eq!(
            repo_of("https://api.github.com/repos/epik-agent/Epik").unwrap(),
            REPO
        );
        assert!(repo_of("Epik").is_err());
    }

    #[test]
    fn reads_an_issue_through_gh_api() {
        let (gh, _) = scripted(vec![(
            vec!["api", "repos/epik-agent/Epik/issues/359"],
            ok(&fixture("issue.json")),
        )]);
        assert_eq!(gh.issue(REPO, 359).unwrap().state, State::Open);
    }

    #[test]
    fn errors_carry_ghs_stderr() {
        let (gh, _) = scripted(vec![(
            vec!["api", "repos/epik-agent/Epik/issues/1"],
            Err(Error::new("gh: Not Found (HTTP 404)")),
        )]);
        let error = gh.issue(REPO, 1).unwrap_err();
        assert_eq!(error.message, "gh: Not Found (HTTP 404)");
    }

    #[test]
    fn reads_every_page_of_sub_issues() {
        let (gh, _) = scripted(vec![(
            vec![
                "api",
                "--paginate",
                "--slurp",
                "repos/epik-agent/Epik/issues/356/sub_issues?per_page=100",
            ],
            ok(&fixture("sub_issues.json")),
        )]);
        assert_eq!(gh.sub_issues(REPO, 356).unwrap().len(), 10);
    }

    #[test]
    fn an_existing_branch_is_left_alone() {
        let (gh, calls) = scripted(vec![(
            vec![
                "api",
                "--paginate",
                "--slurp",
                "repos/epik-agent/Epik/git/matching-refs/heads/daemon?per_page=100",
            ],
            ok(&fixture("matching_refs.json")),
        )]);
        gh.ensure_branch(REPO, "daemon").unwrap();
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_missing_branch_is_created_from_the_default_branchs_head() {
        let main_ref = fixture("matching_refs.json").replace("daemon", "main");
        let (gh, _) = scripted(vec![
            (
                vec![
                    "api",
                    "--paginate",
                    "--slurp",
                    "repos/epik-agent/Epik/git/matching-refs/heads/epic/356-x?per_page=100",
                ],
                ok("[[]]"),
            ),
            (
                vec!["api", "repos/epik-agent/Epik"],
                ok(&fixture("repo.json")),
            ),
            (
                vec![
                    "api",
                    "--paginate",
                    "--slurp",
                    "repos/epik-agent/Epik/git/matching-refs/heads/main?per_page=100",
                ],
                ok(&main_ref),
            ),
            (
                vec![
                    "api",
                    "-X",
                    "POST",
                    "repos/epik-agent/Epik/git/refs",
                    "-f",
                    "ref=refs/heads/epic/356-x",
                    "-f",
                    "sha=dd1288cc693368c025b69618edc4bc603c07d301",
                ],
                ok("{}"),
            ),
        ]);
        gh.ensure_branch(REPO, "epic/356-x").unwrap();
    }

    fn add_epic_label() -> (Vec<&'static str>, Result<String>) {
        (
            vec![
                "api",
                "-X",
                "POST",
                "repos/epik-agent/Epik/issues/356/labels",
                "-f",
                "labels[]=epic",
            ],
            ok("[]"),
        )
    }

    #[test]
    fn an_existing_epic_label_is_only_added() {
        let (gh, _) = scripted(vec![
            (
                vec![
                    "api",
                    "--paginate",
                    "--slurp",
                    "repos/epik-agent/Epik/labels?per_page=100",
                ],
                ok(&fixture("labels.json")),
            ),
            add_epic_label(),
        ]);
        gh.ensure_epic_label(REPO, 356).unwrap();
    }

    #[test]
    fn a_missing_epic_label_is_created_then_added() {
        let (gh, _) = scripted(vec![
            (
                vec![
                    "api",
                    "--paginate",
                    "--slurp",
                    "repos/epik-agent/Epik/labels?per_page=100",
                ],
                ok(r#"[[{"name": "bug"}]]"#),
            ),
            (
                vec![
                    "api",
                    "-X",
                    "POST",
                    "repos/epik-agent/Epik/labels",
                    "-f",
                    "name=epic",
                    "-f",
                    "color=5319E7",
                    "-f",
                    "description=A dependency-ordered set of issues",
                ],
                ok("{}"),
            ),
            add_epic_label(),
        ]);
        gh.ensure_epic_label(REPO, 356).unwrap();
    }

    #[test]
    fn an_already_merged_pull_request_merges() {
        let (gh, calls) = scripted(vec![(
            vec!["api", "repos/epik-agent/Epik/pulls/367"],
            ok(&fixture("pull.json")),
        )]);
        let sha = gh.merge_pull_request(REPO, 367).unwrap();
        assert_eq!(sha, "dd1288cc693368c025b69618edc4bc603c07d301");
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn reads_a_pull_requests_state_and_merge_commit() {
        let (gh, _) = scripted(vec![
            (
                vec!["api", "repos/epik-agent/Epik/pulls/367"],
                ok(&fixture("pull.json")),
            ),
            (
                vec!["api", "repos/epik-agent/Epik/pulls/367"],
                ok(&open_pull()),
            ),
        ]);
        let merged = gh.pull_request(REPO, 367).unwrap();
        assert_eq!(merged.state, PullState::Merged);
        assert_eq!(
            merged.merge_commit.as_deref(),
            Some("dd1288cc693368c025b69618edc4bc603c07d301")
        );
        let open = gh.pull_request(REPO, 367).unwrap();
        assert_eq!(open.state, PullState::Open);
        assert_eq!(open.merge_commit, None);
    }

    #[test]
    fn comments_on_an_issue() {
        let (gh, _) = scripted(vec![(
            vec![
                "api",
                "-X",
                "POST",
                "repos/epik-agent/Epik/issues/13/comments",
                "-f",
                "body=**Epik**",
            ],
            ok("{}"),
        )]);
        gh.comment(REPO, 13, "**Epik**").unwrap();
    }

    #[test]
    fn a_repository_not_found_doesnt_exist() {
        let not_found = || Err(Error::new("gh: Not Found (HTTP 404)"));
        let (gh, _) = scripted(vec![
            (
                vec!["api", "repos/epik-agent/Epik"],
                ok(&fixture("repo.json")),
            ),
            (vec!["api", "repos/epik-agent/Epik"], not_found()),
            (
                vec!["api", "repos/epik-agent/Epik"],
                Err(Error::new("gh: error connecting to api.github.com")),
            ),
        ]);
        assert_eq!(gh.repo_exists(REPO), Ok(true));
        assert_eq!(gh.repo_exists(REPO), Ok(false));
        assert!(gh.repo_exists(REPO).is_err());
    }

    fn open_pull() -> String {
        fixture("pull.json")
            .replace("\"closed\"", "\"open\"")
            .replace(
                "\"merged_at\": \"2026-10-28T21:00:43Z\"",
                "\"merged_at\": null",
            )
    }

    #[test]
    fn parses_the_allowed_merge_methods() {
        let repo: RawRepo = parse(&fixture("repo.json")).unwrap();
        let squash_only = MergeMethods {
            squash: true,
            ..MergeMethods::NONE
        };
        assert_eq!(repo.merge_methods(), squash_only);
    }

    #[test]
    fn unreported_merge_methods_are_allowed() {
        let repo: RawRepo = parse(r#"{"default_branch": "main"}"#).unwrap();
        assert_eq!(repo.merge_methods(), MergeMethods::ALL);
    }

    #[test]
    fn an_open_pull_request_merges_with_a_method_the_repository_allows() {
        let (gh, _) = scripted(vec![
            (
                vec!["api", "repos/epik-agent/Epik/pulls/367"],
                ok(&open_pull()),
            ),
            (
                vec!["api", "repos/epik-agent/Epik"],
                ok(&fixture("repo.json")),
            ),
            (
                vec![
                    "api",
                    "-X",
                    "PUT",
                    "repos/epik-agent/Epik/pulls/367/merge",
                    "-f",
                    "merge_method=squash",
                ],
                ok(r#"{"sha": "6dcb09b5b57875f334f61aebed695e2e4193db5e", "merged": true}"#),
            ),
        ]);
        let sha = gh.merge_pull_request(REPO, 367).unwrap();
        assert_eq!(sha, "6dcb09b5b57875f334f61aebed695e2e4193db5e");
    }

    #[test]
    fn a_repository_allowing_no_merge_method_is_not_merged_into() {
        let none = r#"{
            "default_branch": "main",
            "allow_merge_commit": false,
            "allow_squash_merge": false,
            "allow_rebase_merge": false
        }"#;
        let (gh, _) = scripted(vec![
            (
                vec!["api", "repos/epik-agent/Epik/pulls/367"],
                ok(&open_pull()),
            ),
            (vec!["api", "repos/epik-agent/Epik"], ok(none)),
        ]);
        assert!(gh.merge_pull_request(REPO, 367).is_err());
    }

    #[test]
    fn an_existing_pull_request_is_returned_instead_of_opened() {
        let (gh, calls) = scripted(vec![(
            vec![
                "api",
                "--paginate",
                "--slurp",
                "repos/epik-agent/Epik/pulls?head=epik-agent:epic/1&base=main&state=open&per_page=100",
            ],
            ok(&format!("[[{}]]", fixture("pull.json"))),
        )]);
        let number = gh
            .open_pull_request(REPO, "epic/1", "main", "Title", "Body")
            .unwrap();
        assert_eq!(number, 367);
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_new_pull_request_is_opened() {
        let (gh, _) = scripted(vec![
            (
                vec![
                    "api",
                    "--paginate",
                    "--slurp",
                    "repos/epik-agent/Epik/pulls?head=epik-agent:epic/1&base=main&state=open&per_page=100",
                ],
                ok("[[]]"),
            ),
            (
                vec![
                    "api",
                    "-X",
                    "POST",
                    "repos/epik-agent/Epik/pulls",
                    "-f",
                    "head=epic/1",
                    "-f",
                    "base=main",
                    "-f",
                    "title=Title",
                    "-f",
                    "body=Body",
                ],
                ok(&fixture("pull.json")),
            ),
        ]);
        let number = gh
            .open_pull_request(REPO, "epic/1", "main", "Title", "Body")
            .unwrap();
        assert_eq!(number, 367);
    }

    #[test]
    fn finds_a_pull_request_among_every_one_into_the_base() {
        let (gh, _) = scripted(vec![(
            vec![
                "api",
                "--paginate",
                "--slurp",
                "repos/epik-agent/Epik/pulls?base=daemon&state=all&per_page=100",
            ],
            ok(&fixture("pulls.json")),
        )]);
        let found = gh.find_pull_request(REPO, "daemon", 357).unwrap();
        let merged = PullRequest {
            number: 367,
            merged: true,
        };
        assert_eq!(found, Some(merged));
    }

    #[test]
    fn closing_an_issue_comments_then_closes() {
        let (gh, _) = scripted(vec![
            (
                vec![
                    "api",
                    "-X",
                    "POST",
                    "repos/epik-agent/Epik/issues/359/comments",
                    "-f",
                    "body=Merged in #370.",
                ],
                ok("{}"),
            ),
            (
                vec![
                    "api",
                    "-X",
                    "PATCH",
                    "repos/epik-agent/Epik/issues/359",
                    "-f",
                    "state=closed",
                    "-f",
                    "state_reason=completed",
                ],
                ok("{}"),
            ),
        ]);
        gh.close_issue(REPO, 359, "Merged in #370.").unwrap();
    }

    #[test]
    fn reads_the_submitted_reviews_through_gh_api() {
        let (gh, _) = scripted(vec![(
            vec![
                "api",
                "--paginate",
                "--slurp",
                "repos/epik-agent/Epik/pulls/471/reviews?per_page=100",
            ],
            ok(&fixture("reviews.json")),
        )]);
        let reviews = gh.reviews(REPO, 471).unwrap();
        assert_eq!(reviews.len(), 3);
        assert_eq!(reviews[0].body, "");
        assert_eq!(
            reviews[0].submitted_at,
            "2026-10-30T23:54:08Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(
            reviews[2].body,
            "Two concerns, inline.\n\n<!-- epik-review: changes round=1 -->"
        );
    }

    #[test]
    fn counts_the_unresolved_review_threads_through_graphql() {
        let query: &'static str =
            Box::leak(format!("query={REVIEW_THREADS_QUERY}").into_boxed_str());
        let (gh, _) = scripted(vec![(
            vec![
                "api",
                "graphql",
                "--paginate",
                "--slurp",
                "-f",
                query,
                "-F",
                "owner=epik-agent",
                "-F",
                "name=Epik",
                "-F",
                "number=471",
            ],
            ok(&fixture("review_threads.json")),
        )]);
        assert_eq!(gh.open_review_threads(REPO, 471).unwrap(), 3);
    }

    #[test]
    fn review_threads_are_counted_across_pages() {
        let page = |resolved: &[bool]| {
            let nodes: Vec<String> = resolved
                .iter()
                .map(|resolved| format!(r#"{{"isResolved": {resolved}}}"#))
                .collect();
            format!(
                r#"{{"data": {{"repository": {{"pullRequest": {{"reviewThreads": {{"nodes": [{}]}}}}}}}}}}"#,
                nodes.join(", ")
            )
        };
        let json = format!("[{}, {}]", page(&[false, true]), page(&[false]));
        let pages: Vec<ThreadsPage> = parse(&json).unwrap();
        assert_eq!(unresolved(pages), 2);
    }

    #[test]
    fn a_repository_name_without_an_owner_has_no_review_threads_to_count() {
        let (gh, calls) = scripted(vec![]);
        assert!(gh.open_review_threads("Epik", 471).is_err());
        assert!(calls.lock().unwrap().is_empty());
    }
}
