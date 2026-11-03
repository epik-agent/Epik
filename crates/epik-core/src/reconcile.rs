//! Reconciling runs with GitHub, which the app does on a timer.
//!
//! GitHub decides when a run is over, and whether a failed issue's pull
//! request merged after all. On each check, for every monitored repository,
//! each failed issue of each of its epic runs, building or not, turns merged
//! if its pull request has; and for each of its runs not building:
//!
//! | GitHub says | The library |
//! |---|---|
//! | The epic issue closed | Posts the closing comment, then deletes the run |
//! | The pull request into the default branch closed without merging | Marks the run as needing attention |
//! | A monitored repository is not found | Marks it as no longer on GitHub |
//!
//! A run is deleted only once its closing comment is posted; the comment
//! goes first because its totals are read from the files the delete
//! removes.

use std::fs;
use std::path::Path;

use chrono::Utc;

use crate::comment::{self, Merge};
use crate::github::{GitHub, PullState, State};
use crate::record::{self, BuildWriter, Event, IssueStatus, Kind};
use crate::repositories;
use crate::run::{self, Disposition, RunKey, RunState};

/// The file beside a run's record that says its closing comment is posted
/// or being posted, so a run whose delete failed isn't commented on twice.
/// It is outside the record, which the delete removes.
fn commented(root: &Path, run: &RunKey) -> std::path::PathBuf {
    run::run_dir(root, run).with_extension("closing-comment-posted")
}

/// What a check did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Runs whose epic issue closed, commented on and deleted.
    pub finished: Vec<RunKey>,
    /// Runs whose pull request was closed without merging.
    pub needs_attention: Vec<RunKey>,
    /// Monitored repositories GitHub didn't find.
    pub missing: Vec<String>,
    /// What went wrong, in words; the rest of the check went on.
    pub errors: Vec<String>,
}

/// Checks GitHub for every run of the monitored repositories under `root`.
/// Of the runs in `building`, it only records failed issues that merged.
pub fn reconcile(root: &Path, github: &dyn GitHub, building: &[RunKey]) -> Report {
    let mut report = Report::default();
    let monitored = match repositories::list(root) {
        Ok(monitored) => monitored,
        Err(error) => {
            report
                .errors
                .push(format!("couldn't read the monitored repositories: {error}"));
            return report;
        }
    };
    let runs = match run::list(root) {
        Ok(runs) => runs,
        Err(error) => {
            report
                .errors
                .push(format!("couldn't list the runs: {error}"));
            return report;
        }
    };
    for repository in monitored {
        let repo = repository.repo;
        match github.repo_exists(&repo) {
            Ok(exists) => {
                if let Err(error) = repositories::set_missing(root, &repo, !exists) {
                    report.errors.push(format!("couldn't mark {repo}: {error}"));
                }
                if !exists {
                    report.missing.push(repo);
                    continue;
                }
            }
            Err(error) => {
                report.errors.push(format!("couldn't read {repo}: {error}"));
                continue;
            }
        }
        for state in runs.iter().filter(|r| r.repo.eq_ignore_ascii_case(&repo)) {
            if state.kind == Kind::Epic {
                record_merged_issues(root, github, state, &mut report);
            }
            if building.contains(&state.run) || state.disposition.is_building() {
                continue;
            }
            if let Err(error) = check(root, github, state, &mut report) {
                report.errors.push(format!("{}: {error}", state.run));
            }
        }
    }
    report
}

/// Records as merged each failed issue of `state`'s latest build whose pull
/// request has merged. An issue only an earlier build has stays failed: its
/// merge can't be recorded in the latest build, whose fold would ignore it.
fn record_merged_issues(root: &Path, github: &dyn GitHub, state: &RunState, report: &mut Report) {
    let builds = match record::list_builds(root, &state.run) {
        Ok(builds) => builds,
        Err(error) => {
            report
                .errors
                .push(format!("{}: couldn't read its builds: {error}", state.run));
            return;
        }
    };
    let Some(latest) = builds.last() else {
        return;
    };
    let failed = latest
        .issues
        .iter()
        .filter(|issue| issue.status == IssueStatus::Failed);
    for issue in failed {
        let number = issue.issue.number;
        if let Err(error) = record_if_merged(root, github, state, &latest.build_id, number) {
            report
                .errors
                .push(format!("{}: #{number}: {error}", state.run));
        }
    }
}

/// Appends `issue_merged` to build `build_id` if issue `number`'s pull request
/// into the run's branch has merged.
fn record_if_merged(
    root: &Path,
    github: &dyn GitHub,
    state: &RunState,
    build_id: &str,
    number: u64,
) -> Result<(), String> {
    let found = github
        .find_pull_request(&state.repo, &state.branch, number)
        .map_err(|error| format!("couldn't find its pull request: {error}"))?;
    let Some(pr) = found.filter(|pr| pr.merged) else {
        return Ok(());
    };
    let pull = github
        .pull_request(&state.repo, pr.number)
        .map_err(|error| format!("couldn't read #{}: {error}", pr.number))?;
    BuildWriter::open(root, &state.run, build_id)
        .and_then(|mut writer| {
            writer.append(&Event::IssueMerged {
                at: Utc::now(),
                number,
                pull_request: pr.number,
                merge_commit: pull.merge_commit,
            })
        })
        .map_err(|error| format!("couldn't record that #{} merged: {error}", pr.number))
}

/// Checks one run that isn't building.
fn check(
    root: &Path,
    github: &dyn GitHub,
    state: &RunState,
    report: &mut Report,
) -> Result<(), String> {
    let issue = github
        .issue(&state.repo, state.number)
        .map_err(|error| format!("couldn't read the epic issue: {error}"))?;
    if issue.state == State::Closed {
        finish(root, github, state)?;
        report.finished.push(state.run.clone());
        return Ok(());
    }
    if state.disposition == Disposition::WaitingForReview
        && let Some(pr) = state.pull_request
    {
        let pull = github
            .pull_request(&state.repo, pr)
            .map_err(|error| format!("couldn't read #{pr}: {error}"))?;
        if pull.state == PullState::Closed {
            let latest = state.builds.last().ok_or("the run has no builds")?;
            BuildWriter::open(root, &state.run, &latest.build_id)
                .and_then(|mut writer| {
                    writer.append(&Event::PullRequestClosed {
                        at: Utc::now(),
                        pull_request: pr,
                    })
                })
                .map_err(|error| format!("couldn't record that #{pr} closed: {error}"))?;
            report.needs_attention.push(state.run.clone());
        }
    }
    Ok(())
}

/// Posts the run's closing comment on its epic issue, then deletes the
/// run. If the comment can't be posted, the run stays.
pub fn finish(root: &Path, github: &dyn GitHub, state: &RunState) -> Result<(), String> {
    let marker = commented(root, &state.run);
    if !marker.exists() {
        let merge = match state.pull_request {
            Some(pr) => {
                let pull = github
                    .pull_request(&state.repo, pr)
                    .map_err(|error| format!("couldn't read #{pr}: {error}"))?;
                (pull.state == PullState::Merged).then_some(Merge {
                    pull_request: pr,
                    merge_commit: pull.merge_commit,
                })
            }
            None => None,
        };
        let body = comment::closing(state, merge.as_ref());
        // Marked first: a comment that can't be marked isn't posted.
        fs::write(&marker, "")
            .map_err(|error| format!("couldn't mark the closing comment: {error}"))?;
        if let Err(error) = github.comment(&state.repo, state.number, &body) {
            let _ = fs::remove_file(&marker);
            return Err(format!("couldn't post the closing comment: {error}"));
        }
    }
    run::delete(root, &state.run).map_err(|error| format!("couldn't delete the run: {error}"))?;
    let _ = fs::remove_file(&marker);
    // Prunes the directories the marker kept.
    let _ = run::delete(root, &state.run);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::FakeGitHub;
    use crate::record::{Issue, Outcome};
    use chrono::{DateTime, TimeZone};
    use rust_decimal::Decimal;

    const REPO: &str = "wpm/epik-test";

    fn time(seconds: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap() + chrono::Duration::seconds(seconds)
    }

    /// GitHub holding open epic 1 with sub-issue 2, and the epic's pull
    /// request 3 from `epic/1` into `main`.
    fn github() -> FakeGitHub {
        let github = FakeGitHub::new();
        github.add_repo(REPO, "main");
        github.add_branch(REPO, "epic/1");
        github.add_issue(REPO, 1, "The epic");
        github.add_issue(REPO, 2, "The issue");
        let pr = github
            .open_pull_request(REPO, "epic/1", "main", "The epic", "Closes #1")
            .unwrap();
        assert_eq!(pr, 3);
        github
    }

    fn key() -> RunKey {
        RunKey::new(REPO, 1)
    }

    /// A run of epic 1 whose one build succeeded, waiting for review of
    /// pull request 3; or, if `ended` is false, still building.
    fn record(root: &Path, ended: bool) {
        repositories::add(root, REPO, false, time(0)).unwrap();
        let mut writer = BuildWriter::create(root, &key(), "b").unwrap();
        let mut events = vec![
            Event::BuildStarted {
                at: time(0),
                build_id: "b".to_owned(),
                repo: REPO.to_owned(),
                number: 1,
                title: "The epic".to_owned(),
                kind: Kind::Epic,
                branch: "epic/1".to_owned(),
                issues: vec![Issue {
                    number: 2,
                    title: "The issue".to_owned(),
                    blocked_by: vec![],
                }],
                closed: vec![],
                roles: Default::default(),
            },
            Event::IssueStarted {
                at: time(1),
                number: 2,
            },
        ];
        if ended {
            events.extend([
                Event::IssueFinished {
                    at: time(60),
                    number: 2,
                    outcome: Outcome::Succeeded,
                    reason: None,
                    pull_request: Some(4),
                    duration_ms: Some(59_000),
                    cost_usd: Some(Decimal::new(125, 2)),
                },
                Event::IssueMerged {
                    at: time(61),
                    number: 2,
                    pull_request: 4,
                    merge_commit: Some("4444444abc".to_owned()),
                },
                Event::BuildFinished {
                    at: time(62),
                    outcome: Outcome::Succeeded,
                    reason: None,
                    pull_request: Some(3),
                    review: None,
                    open_threads: None,
                },
            ]);
        }
        for event in &events {
            writer.append(event).unwrap();
        }
    }

    /// A build of a run of `kind` with `issues`, started at `at`. Each of
    /// `failed` failed; the build failed too, unless `ended` is false.
    fn record_failed(
        root: &Path,
        build_id: &str,
        at: DateTime<Utc>,
        kind: Kind,
        issues: &[u64],
        failed: &[u64],
        ended: bool,
    ) {
        repositories::add(root, REPO, false, time(0)).unwrap();
        let mut writer = BuildWriter::create(root, &key(), build_id).unwrap();
        let started = Event::BuildStarted {
            at,
            build_id: build_id.to_owned(),
            repo: REPO.to_owned(),
            number: 1,
            title: "The epic".to_owned(),
            kind,
            branch: "epic/1".to_owned(),
            issues: issues
                .iter()
                .map(|number| Issue {
                    number: *number,
                    title: format!("Issue {number}"),
                    blocked_by: vec![],
                })
                .collect(),
            closed: vec![],
            roles: Default::default(),
        };
        writer.append(&started).unwrap();
        for number in failed {
            writer
                .append(&Event::IssueFinished {
                    at,
                    number: *number,
                    outcome: Outcome::Failed,
                    reason: Some("the merge was refused".to_owned()),
                    pull_request: None,
                    duration_ms: None,
                    cost_usd: None,
                })
                .unwrap();
        }
        if ended {
            writer
                .append(&Event::BuildFinished {
                    at,
                    outcome: Outcome::Failed,
                    reason: Some("an issue failed".to_owned()),
                    pull_request: None,
                    review: None,
                    open_threads: None,
                })
                .unwrap();
        }
    }

    /// A run of epic 1 whose build `b` failed issue 2; or, if `ended` is
    /// false, is still building.
    fn record_failed_issue(root: &Path, ended: bool) {
        record_failed(root, "b", time(0), Kind::Epic, &[2], &[2], ended);
    }

    /// Opens a pull request into `epic/1` that closes `issue`.
    fn open_issue_pull(github: &FakeGitHub, issue: u64) -> u64 {
        let head = format!("issue-{issue}");
        github.add_branch(REPO, &head);
        github
            .open_pull_request(
                REPO,
                &head,
                "epic/1",
                "The fix",
                &format!("Closes #{issue}"),
            )
            .unwrap()
    }

    /// How many `issue_merged` events build `build_id` holds.
    fn merges_recorded(root: &Path, build_id: &str) -> usize {
        record::read_events(root, &key(), build_id)
            .unwrap()
            .iter()
            .filter(|event| matches!(event, Event::IssueMerged { .. }))
            .count()
    }

    fn assert_merged(root: &Path, issue: u64, pull_request: u64) {
        let run = run::read(root, &key()).unwrap();
        let issue = run.issue(issue).unwrap();
        assert_eq!(issue.status, IssueStatus::Merged);
        assert_eq!(issue.reason, None);
        assert_eq!(issue.pull_request, Some(pull_request));
        assert_eq!(
            issue.merge_commit,
            Some(FakeGitHub::merge_commit(pull_request))
        );
    }

    fn assert_failed(root: &Path, issue: u64) {
        let run = run::read(root, &key()).unwrap();
        let issue = run.issue(issue).unwrap();
        assert_eq!(issue.status, IssueStatus::Failed);
        assert_eq!(issue.reason.as_deref(), Some("the merge was refused"));
    }

    #[test]
    fn a_failed_issue_whose_pull_request_merged_turns_merged_in_an_ended_run() {
        let root = tempfile::tempdir().unwrap();
        record_failed_issue(root.path(), true);
        let github = github();
        let pr = open_issue_pull(&github, 2);
        github.set_pull_state(REPO, pr, PullState::Merged);

        let report = reconcile(root.path(), &github, &[]);

        assert_eq!(report, Report::default());
        assert_merged(root.path(), 2, pr);
        let run = run::read(root.path(), &key()).unwrap();
        assert_eq!(
            run.disposition,
            Disposition::NeedsAttention,
            "the run still needs a rerun"
        );
    }

    #[test]
    fn a_failed_issue_whose_pull_request_merged_turns_merged_in_a_building_run() {
        let root = tempfile::tempdir().unwrap();
        record_failed_issue(root.path(), false);
        let github = github();
        let pr = open_issue_pull(&github, 2);
        github.set_pull_state(REPO, pr, PullState::Merged);
        github.set_state(REPO, 1, State::Closed);

        let report = reconcile(root.path(), &github, &[key()]);

        assert_eq!(report, Report::default(), "the rest of the check skips it");
        assert_merged(root.path(), 2, pr);
        assert_eq!(
            run::read(root.path(), &key()).unwrap().disposition,
            Disposition::Building
        );
    }

    #[test]
    fn a_failed_issue_whose_pull_request_is_open_or_absent_stays_failed() {
        let root = tempfile::tempdir().unwrap();
        record_failed(
            root.path(),
            "b",
            time(0),
            Kind::Epic,
            &[2, 5],
            &[2, 5],
            true,
        );
        let github = github();
        github.add_issue(REPO, 5, "Another issue");
        open_issue_pull(&github, 2);

        let report = reconcile(root.path(), &github, &[]);

        assert_eq!(report, Report::default());
        assert_failed(root.path(), 2);
        assert_failed(root.path(), 5);
        assert_eq!(merges_recorded(root.path(), "b"), 0);
    }

    #[test]
    fn a_second_check_doesnt_record_the_merge_again() {
        let root = tempfile::tempdir().unwrap();
        record_failed_issue(root.path(), true);
        let github = github();
        let pr = open_issue_pull(&github, 2);
        github.set_pull_state(REPO, pr, PullState::Merged);

        reconcile(root.path(), &github, &[]);
        reconcile(root.path(), &github, &[]);

        assert_eq!(merges_recorded(root.path(), "b"), 1);
    }

    #[test]
    fn a_failed_issue_the_latest_build_doesnt_have_isnt_recorded_on_every_check() {
        let root = tempfile::tempdir().unwrap();
        record_failed(root.path(), "a", time(0), Kind::Epic, &[2], &[2], true);
        record_failed(root.path(), "b", time(100), Kind::Epic, &[5], &[], true);
        let github = github();
        let pr = open_issue_pull(&github, 2);
        github.set_pull_state(REPO, pr, PullState::Merged);

        reconcile(root.path(), &github, &[]);

        assert_eq!(merges_recorded(root.path(), "a"), 0);
        assert_eq!(merges_recorded(root.path(), "b"), 0);
    }

    #[test]
    fn a_github_error_on_a_failed_issue_is_reported_and_the_check_goes_on() {
        let root = tempfile::tempdir().unwrap();
        record_failed(
            root.path(),
            "b",
            time(0),
            Kind::Epic,
            &[2, 5],
            &[2, 5],
            true,
        );
        let github = github();
        github.add_issue(REPO, 5, "Another issue");
        github.fail("find_pull_request", "gh: rate limited");
        github.set_state(REPO, 1, State::Closed);

        let report = reconcile(root.path(), &github, &[]);

        let [first, second] = &report.errors[..] else {
            panic!("an error for each issue: {report:?}");
        };
        assert!(
            first.contains("#2") && first.contains("rate limited"),
            "{first}"
        );
        assert!(
            second.contains("#5") && second.contains("rate limited"),
            "{second}"
        );
        assert_eq!(report.finished, [key()], "the rest of the check went on");
    }

    #[test]
    fn a_merge_commit_that_cant_be_read_is_recorded_on_a_later_check() {
        let root = tempfile::tempdir().unwrap();
        record_failed_issue(root.path(), true);
        let github = github();
        let pr = open_issue_pull(&github, 2);
        github.set_pull_state(REPO, pr, PullState::Merged);
        github.fail("pull_request", "gh: rate limited");

        let report = reconcile(root.path(), &github, &[]);

        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert_failed(root.path(), 2);
        github.succeed("pull_request");
        reconcile(root.path(), &github, &[]);
        assert_merged(root.path(), 2, pr);
    }

    #[test]
    fn a_single_issue_run_stays_failed() {
        let root = tempfile::tempdir().unwrap();
        record_failed(root.path(), "b", time(0), Kind::Issue, &[1], &[1], true);
        let github = github();
        let pr = open_issue_pull(&github, 1);
        github.set_pull_state(REPO, pr, PullState::Merged);

        reconcile(root.path(), &github, &[]);

        assert_eq!(merges_recorded(root.path(), "b"), 0);
    }

    #[test]
    fn a_closed_epic_issue_gets_its_comment_and_the_run_is_deleted() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), true);
        let github = github();
        github.set_pull_state(REPO, 3, PullState::Merged);
        github.set_state(REPO, 1, State::Closed);

        let report = reconcile(root.path(), &github, &[]);

        assert_eq!(report.finished, [key()]);
        assert!(report.errors.is_empty(), "{report:?}");
        let comments = github.repo(REPO).issues[&1].comments.clone();
        let [comment] = &comments[..] else {
            panic!("one comment: {comments:?}");
        };
        let merged = &FakeGitHub::merge_commit(3)[..7];
        assert!(
            comment.starts_with("**Epik: built in 1 build, 1m 02s, $1.25**"),
            "{comment}"
        );
        assert!(
            comment.contains(&format!("Merged by #3 ({merged}).")),
            "{comment}"
        );
        assert!(
            comment.contains("| #2 The issue | 59s | $1.25 | #4 | 4444444 |"),
            "{comment}"
        );
        assert!(!run::run_dir(root.path(), &key()).exists());
    }

    #[test]
    fn a_hosts_mirrored_run_is_neither_commented_on_nor_deleted() {
        let root = tempfile::tempdir().unwrap();
        let mirror = crate::hosts::dir(root.path(), "devbox");
        record(&mirror, true);
        repositories::add(root.path(), REPO, false, time(0)).unwrap();
        let github = github();
        github.set_pull_state(REPO, 3, PullState::Closed);
        github.set_state(REPO, 1, State::Closed);

        let report = reconcile(root.path(), &github, &[]);

        assert_eq!(report, Report::default());
        assert!(github.repo(REPO).issues[&1].comments.is_empty());
        assert_eq!(
            run::read(&mirror, &key()).unwrap().disposition,
            Disposition::WaitingForReview
        );
    }

    #[test]
    fn a_closed_epic_issue_ends_a_run_that_needs_attention_too() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), true);
        let github = github();
        github.set_pull_state(REPO, 3, PullState::Closed);
        reconcile(root.path(), &github, &[]);
        assert_eq!(
            run::read(root.path(), &key()).unwrap().disposition,
            Disposition::NeedsAttention
        );
        github.set_state(REPO, 1, State::Closed);

        let report = reconcile(root.path(), &github, &[]);

        assert_eq!(report.finished, [key()]);
        assert!(run::list(root.path()).unwrap().is_empty());
        let comment = &github.repo(REPO).issues[&1].comments[0];
        assert!(!comment.contains("Merged by"), "{comment}");
    }

    #[test]
    fn a_pull_request_closed_without_merging_needs_attention() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), true);
        let github = github();
        github.set_pull_state(REPO, 3, PullState::Closed);

        let report = reconcile(root.path(), &github, &[]);

        assert_eq!(report.needs_attention, [key()]);
        let run = run::read(root.path(), &key()).unwrap();
        assert_eq!(run.disposition, Disposition::NeedsAttention);
        assert_eq!(
            run.reason.as_deref(),
            Some("PR #3 was closed without merging")
        );
        let again = reconcile(root.path(), &github, &[]);
        assert!(again.needs_attention.is_empty(), "noted once");
    }

    #[test]
    fn an_open_pull_request_leaves_the_run_waiting() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), true);

        let report = reconcile(root.path(), &github(), &[]);

        assert_eq!(report, Report::default());
        let run = run::read(root.path(), &key()).unwrap();
        assert_eq!(run.disposition, Disposition::WaitingForReview);
    }

    #[test]
    fn a_monitored_repository_not_found_is_marked_and_comes_back() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), true);
        let github = github();
        github.remove_repo(REPO);

        let report = reconcile(root.path(), &github, &[]);

        assert_eq!(report.missing, [REPO]);
        assert!(repositories::list(root.path()).unwrap()[0].missing);
        assert!(run::read(root.path(), &key()).is_ok(), "its runs stay");
        github.add_repo(REPO, "main");
        reconcile(root.path(), &github, &[]);
        assert!(!repositories::list(root.path()).unwrap()[0].missing);
    }

    #[test]
    fn a_run_is_deleted_only_once_its_comment_is_posted() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), true);
        let github = github();
        github.set_state(REPO, 1, State::Closed);
        github.fail("comment", "gh: rate limited");

        let report = reconcile(root.path(), &github, &[]);

        assert!(report.finished.is_empty());
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert!(run::read(root.path(), &key()).is_ok());
        github.succeed("comment");
        assert_eq!(reconcile(root.path(), &github, &[]).finished, [key()]);
        assert_eq!(github.repo(REPO).issues[&1].comments.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_run_whose_delete_fails_isnt_commented_on_twice() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        record(root.path(), true);
        let github = github();
        github.set_state(REPO, 1, State::Closed);
        let parent = run::run_dir(root.path(), &key())
            .parent()
            .unwrap()
            .to_owned();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();

        let failed = reconcile(root.path(), &github, &[]);

        assert!(failed.finished.is_empty(), "{failed:?}");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        let finished = reconcile(root.path(), &github, &[]);
        assert_eq!(finished.finished, [key()]);
        assert_eq!(github.repo(REPO).issues[&1].comments.len(), 1);
        assert!(!commented(root.path(), &key()).exists());
        assert_eq!(
            fs::read_dir(record::runs_dir(root.path())).unwrap().count(),
            0
        );
    }

    #[test]
    fn building_runs_and_unmonitored_repositories_are_left_alone() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), false);
        let github = github();
        github.set_state(REPO, 1, State::Closed);

        let report = reconcile(root.path(), &github, &[]);
        assert!(report.finished.is_empty(), "the record says it is building");
        let report = reconcile(root.path(), &github, &[key()]);
        assert!(report.finished.is_empty());

        repositories::remove(root.path(), REPO).unwrap();
        let report = reconcile(root.path(), &github, &[]);
        assert_eq!(report, Report::default());
        assert!(run::read(root.path(), &key()).is_ok());
    }
}
