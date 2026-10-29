//! What a run's window shows, as a pure function of the run's fold.
//!
//! [`view`] lays a run's issues out as a dependency graph: a column per
//! level, left to right, so every blocker is left of what it blocks, and a
//! row per issue within its column. It also words the header and each
//! node's duration and cost, so the page only places and prints what it is
//! given.

use chrono::{DateTime, Utc};
use epik_core::graph::Graph;
use epik_core::record::IssueStatus;
use epik_core::run::{Disposition, RunState};
use epik_core::words::{dollars, duration};
use serde::Serialize;

/// A run's window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct View {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub status: Disposition,
    pub reason: Option<String>,
    /// The epic's pull request.
    pub pull_request: Option<u64>,
    pub elapsed: String,
    pub cost: String,
    /// How many columns the graph spans.
    pub columns: usize,
    /// How many rows its tallest column has.
    pub rows: usize,
    /// In the order of the run's issues.
    pub nodes: Vec<Node>,
    /// In the order of the issues blocked, then of their blockers.
    pub edges: Vec<Edge>,
}

/// An issue, placed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Node {
    pub number: u64,
    pub title: String,
    pub status: IssueStatus,
    pub reason: Option<String>,
    pub pull_request: Option<u64>,
    pub duration: Option<String>,
    pub cost: Option<String>,
    /// Its level: 0 with no blockers, else one right of its deepest
    /// blocker's. Issues on or behind a cycle go in a column after the
    /// rest.
    pub column: usize,
    /// Its place in its column, in the order of the run's issues.
    pub row: usize,
}

/// An arrow from a blocker to the issue it blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Edge {
    pub from: u64,
    pub to: u64,
}

/// The window for `run` at `now`.
pub fn view(run: &RunState, now: DateTime<Utc>) -> View {
    let graph = Graph::new(
        run.issues
            .iter()
            .map(|m| (m.issue.number, m.issue.blocked_by.iter().copied())),
    );
    let levels = graph.levels();
    let level = |number: u64| levels.get(&number).copied().flatten();
    let after = run
        .issues
        .iter()
        .filter_map(|m| level(m.issue.number))
        .max()
        .map_or(0, |deepest| deepest + 1);

    let mut heights: Vec<usize> = Vec::new();
    let nodes: Vec<Node> = run
        .issues
        .iter()
        .map(|m| {
            let column = level(m.issue.number).unwrap_or(after);
            if heights.len() <= column {
                heights.resize(column + 1, 0);
            }
            let row = heights[column];
            heights[column] += 1;
            Node {
                number: m.issue.number,
                title: m.issue.title.clone(),
                status: m.status,
                reason: m.reason.clone(),
                pull_request: m.pull_request,
                duration: m.duration_ms.map(duration),
                cost: m.cost_usd.map(dollars),
                column,
                row,
            }
        })
        .collect();

    let is_issue = |number: u64| run.issue(number).is_some();
    let edges = run
        .issues
        .iter()
        .flat_map(|m| {
            m.issue
                .blocked_by
                .iter()
                .filter(|&&blocker| is_issue(blocker))
                .map(|&blocker| Edge {
                    from: blocker,
                    to: m.issue.number,
                })
        })
        .collect();

    let elapsed_ms = u64::try_from(run.elapsed(now).num_milliseconds()).unwrap_or(0);
    View {
        repo: run.repo.clone(),
        number: run.number,
        title: run.title.clone(),
        status: run.disposition,
        reason: run.reason.clone(),
        pull_request: run.pull_request,
        elapsed: duration(elapsed_ms),
        cost: dollars(run.cost_usd),
        columns: heights.len(),
        rows: heights.iter().copied().max().unwrap_or(0),
        nodes,
        edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use epik_core::record::{BuildStatus, Issue, IssueState, Kind};
    use epik_core::run::{BuildSummary, RunKey};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn time(seconds: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap() + chrono::Duration::seconds(seconds)
    }

    fn issue(number: u64, blocked_by: &[u64]) -> IssueState {
        IssueState {
            issue: Issue {
                number,
                title: format!("Issue {number}"),
                blocked_by: blocked_by.to_vec(),
            },
            status: IssueStatus::Waiting,
            reason: None,
            pull_request: None,
            merge_commit: None,
            duration_ms: None,
            cost_usd: None,
            activity: None,
        }
    }

    /// A run of epic 1 whose one build is running, 10 seconds in.
    fn run(issues: Vec<IssueState>) -> RunState {
        RunState {
            run: RunKey::new("o/r", 1),
            repo: "o/r".to_owned(),
            number: 1,
            title: "The epic".to_owned(),
            kind: Kind::Epic,
            branch: "epic".to_owned(),
            disposition: Disposition::Building,
            reason: None,
            pull_request: None,
            issues,
            builds: vec![BuildSummary {
                build_id: "20261028T000000Z-a1b2".to_owned(),
                status: BuildStatus::Running,
                reason: None,
                pull_request: None,
                started_at: time(0),
                ended_at: None,
                elapsed_ms: 10_000,
                cost_usd: Decimal::ZERO,
            }],
            running_build: Some("20261028T000000Z-a1b2".to_owned()),
            started_at: time(0),
            latest_at: time(10),
            elapsed_ms: 10_000,
            cost_usd: Decimal::ZERO,
        }
    }

    /// Each node's number, column and row.
    fn places(view: &View) -> Vec<(u64, usize, usize)> {
        view.nodes
            .iter()
            .map(|node| (node.number, node.column, node.row))
            .collect()
    }

    fn edge(from: u64, to: u64) -> Edge {
        Edge { from, to }
    }

    #[test]
    fn issues_go_in_columns_by_level_and_rows_in_issue_order() {
        // 2 and 3 need nothing; 4 needs 2; 5 needs 2 and 4; 6 needs 3.
        let run = run(vec![
            issue(2, &[]),
            issue(3, &[]),
            issue(4, &[2]),
            issue(5, &[4, 2]),
            issue(6, &[3]),
        ]);

        let view = view(&run, time(10));

        assert_eq!(
            places(&view),
            [(2, 0, 0), (3, 0, 1), (4, 1, 0), (5, 2, 0), (6, 1, 1)]
        );
        assert_eq!((view.columns, view.rows), (3, 2));
        assert_eq!(view.edges, [edge(2, 4), edge(4, 5), edge(2, 5), edge(3, 6)]);
    }

    #[test]
    fn blockers_that_arent_issues_are_ignored() {
        let view = view(&run(vec![issue(2, &[99]), issue(3, &[2])]), time(0));

        assert_eq!(places(&view), [(2, 0, 0), (3, 1, 0)]);
        assert_eq!(view.edges, [edge(2, 3)]);
    }

    #[test]
    fn issues_on_or_behind_a_cycle_go_in_a_column_after_the_rest() {
        let run = run(vec![
            issue(2, &[]),
            issue(3, &[4]),
            issue(4, &[3]),
            issue(5, &[3]),
        ]);

        let view = view(&run, time(0));

        assert_eq!(places(&view), [(2, 0, 0), (3, 1, 0), (4, 1, 1), (5, 1, 2)]);
        assert_eq!((view.columns, view.rows), (2, 3));
    }

    #[test]
    fn a_run_with_no_issues_has_an_empty_graph() {
        let view = view(&run(vec![]), time(0));

        assert_eq!((view.columns, view.rows), (0, 0));
        assert!(view.nodes.is_empty());
    }

    #[test]
    fn a_node_shows_its_state_pull_request_duration_and_cost_once_known() {
        let mut done = issue(2, &[]);
        done.status = IssueStatus::Merged;
        done.pull_request = Some(102);
        done.duration_ms = Some(185_000);
        done.cost_usd = Some(Decimal::from_str("1.5").unwrap());
        let run = run(vec![done, issue(3, &[2])]);

        let view = view(&run, time(0));

        let [merged, waiting] = &view.nodes[..] else {
            panic!("two nodes");
        };
        assert_eq!(merged.status, IssueStatus::Merged);
        assert_eq!(merged.pull_request, Some(102));
        assert_eq!(merged.duration.as_deref(), Some("3m 05s"));
        assert_eq!(merged.cost.as_deref(), Some("$1.50"));
        assert_eq!(waiting.status, IssueStatus::Waiting);
        assert_eq!(
            (waiting.duration.as_ref(), waiting.cost.as_ref()),
            (None, None)
        );
    }

    #[test]
    fn the_header_shows_the_run_and_its_clock_keeps_going_while_it_runs() {
        let mut run = run(vec![]);
        run.cost_usd = Decimal::from_str("0.125").unwrap();

        let view = view(&run, time(75));

        assert_eq!(
            (view.repo.as_str(), view.number, view.title.as_str()),
            ("o/r", 1, "The epic")
        );
        assert_eq!(view.status, Disposition::Building);
        assert_eq!(view.elapsed, "1m 15s");
        assert_eq!(view.cost, "$0.13");
    }

    #[test]
    fn a_finished_run_shows_its_recorded_time() {
        let mut run = run(vec![]);
        run.disposition = Disposition::WaitingForReview;
        run.pull_request = Some(50);
        run.builds[0].status = BuildStatus::Succeeded;
        run.builds[0].ended_at = Some(time(10));

        let view = view(&run, time(1000));

        assert_eq!(view.status, Disposition::WaitingForReview);
        assert_eq!(view.pull_request, Some(50));
        assert_eq!(view.elapsed, "10s");
    }
}
