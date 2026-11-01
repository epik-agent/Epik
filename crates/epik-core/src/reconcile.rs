//! Reconciling runs with GitHub, which the app does on a timer.
//!
//! GitHub decides when a run is over. On each check, for every monitored
//! repository and each of its runs not building:
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
use crate::record::{BuildWriter, Event};
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

/// Checks GitHub for every run of the monitored repositories under `root`,
/// leaving alone the runs in `building`.
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
            if building.contains(&state.run) || state.disposition == Disposition::Building {
                continue;
            }
            if let Err(error) = check(root, github, state, &mut report) {
                report.errors.push(format!("{}: {error}", state.run));
            }
        }
    }
    report
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
    use crate::record::{Issue, Kind, Outcome};
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
                    duration_ms: 59_000,
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
                },
            ]);
        }
        for event in &events {
            writer.append(event).unwrap();
        }
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
            fs::read_dir(crate::record::runs_dir(root.path()))
                .unwrap()
                .count(),
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
