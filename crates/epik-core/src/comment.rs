//! The comments in which Epik records time and cost on GitHub.
//!
//! GitHub is the archive, so time and cost are written on the issues they
//! were spent on: each epic sub-issue when its pull request merges, and the
//! epic issue when it closes. Each comment is readable, and carries the
//! same figures in a hidden block, `<!-- epik:cost {json} -->`, for an audit
//! tool to parse later. Every comment names the pull request and the merge
//! commit the money bought, and the agent and model of each session that
//! spent it.

use rust_decimal::Decimal;
use serde::Serialize;

use crate::record::{IssueState, Session, SessionRole};
use crate::run::RunState;
use crate::settings::{self, Role};
use crate::words::{dollars, duration};

/// The version of the hidden block's JSON.
const VERSION: u32 = 1;

/// One issue's figures in the hidden block.
#[derive(Serialize)]
struct IssueCost<'a> {
    number: u64,
    duration_ms: u64,
    cost_usd: Decimal,
    pull_request: Option<u64>,
    merge_commit: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sessions: Vec<SessionCost<'a>>,
}

/// One session's figures in the hidden block.
#[derive(Serialize)]
struct SessionCost<'a> {
    role: SessionRole,
    round: u32,
    agent: &'a str,
    model: Option<&'a str>,
    duration_ms: Option<u64>,
    cost_usd: Option<Decimal>,
}

impl<'a> SessionCost<'a> {
    fn of(session: &'a Session) -> Self {
        Self {
            role: session.role,
            round: session.round,
            agent: &session.agent,
            model: session.model.as_deref(),
            duration_ms: session.duration_ms,
            cost_usd: session.cost_usd,
        }
    }
}

/// Each session in a line: "- Coding, round 2: Claude Code with opus, 20s,
/// $0.25". The first round of a role goes without its number.
fn sessions(sessions: &[Session]) -> String {
    sessions
        .iter()
        .map(|session| {
            let role = match session.role {
                SessionRole::Coding => "Coding",
                SessionRole::Review => "Review",
            };
            let round = match session.round {
                1 => String::new(),
                n => format!(", round {n}"),
            };
            let agent = settings::profile(&Role {
                agent: session.agent.clone(),
                model: session.model.clone(),
            })
            .map_or_else(|_| session.agent.clone(), |profile| profile.name);
            let model = session.model.as_deref().unwrap_or("its default model");
            let time = session
                .duration_ms
                .map_or("still running".to_owned(), duration);
            let cost = session
                .cost_usd
                .map_or("cost not reported".to_owned(), dollars);
            format!("- {role}{round}: {agent} with {model}, {time}, {cost}\n")
        })
        .collect()
}

impl<'a> IssueCost<'a> {
    fn of(issue: &'a IssueState) -> Self {
        Self {
            number: issue.issue.number,
            duration_ms: issue.duration_ms.unwrap_or(0),
            cost_usd: issue.cost_usd.unwrap_or(Decimal::ZERO),
            pull_request: issue.pull_request,
            merge_commit: issue.merge_commit.as_deref(),
            sessions: issue.sessions.iter().map(SessionCost::of).collect(),
        }
    }
}

fn hidden(value: &impl Serialize) -> String {
    let json = serde_json::to_string(value).expect("figures serialize");
    format!("<!-- epik:cost {json} -->")
}

/// A commit's short form, as GitHub links it.
fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// The comment on an epic sub-issue whose pull request has merged into the
/// epic branch: its own time and cost, over every build of the run.
pub fn issue_merged(branch: &str, issue: &IssueState) -> String {
    #[derive(Serialize)]
    struct Block<'a> {
        version: u32,
        #[serde(flatten)]
        issue: IssueCost<'a>,
    }
    let cost = IssueCost::of(issue);
    let by = match (cost.pull_request, cost.merge_commit) {
        (Some(pr), Some(sha)) => format!(" by #{pr} ({})", short(sha)),
        (Some(pr), None) => format!(" by #{pr}"),
        _ => String::new(),
    };
    let mut lines = sessions(&issue.sessions);
    if !lines.is_empty() {
        lines.push('\n');
    }
    format!(
        "**Epik: {}, {}.** Merged into `{branch}`{by}.\n\n{lines}{}",
        duration(cost.duration_ms),
        dollars(cost.cost_usd),
        hidden(&Block {
            version: VERSION,
            issue: cost,
        })
    )
}

/// How the run's last pull request merged, as GitHub says when the epic
/// issue closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge {
    pub pull_request: u64,
    pub merge_commit: Option<String>,
}

/// The closing comment on a run's epic issue: the run's total time and
/// cost, the number of builds, the run's own sessions, and a breakdown by
/// issue. For a single
/// issue's run, the issue is the epic issue, and `merge` gives its pull
/// request and merge commit. For an epic, `merge` is the epic's pull
/// request into the default branch, if it merged.
pub fn closing(run: &RunState, merge: Option<&Merge>) -> String {
    let single = run.issues.len() == 1 && run.issues[0].issue.number == run.number;
    let mut issues: Vec<IssueState> = run.issues.clone();
    if single && let Some(merge) = merge {
        issues[0].pull_request = Some(merge.pull_request);
        issues[0].merge_commit = merge.merge_commit.clone();
    }
    let builds = match run.builds.len() {
        1 => "1 build".to_owned(),
        n => format!("{n} builds"),
    };
    let mut text = format!(
        "**Epik: built in {builds}, {}, {}**\n\n",
        duration(run.elapsed_ms),
        dollars(run.cost_usd)
    );
    if !single && let Some(merge) = merge {
        let sha = merge
            .merge_commit
            .as_deref()
            .map(|sha| format!(" ({})", short(sha)))
            .unwrap_or_default();
        text.push_str(&format!("Merged by #{}{sha}.\n\n", merge.pull_request));
    }
    // A single issue's sessions are the run's: its comment is this one.
    let mut own: Vec<Session> = Vec::new();
    if single {
        own.extend(issues[0].sessions.iter().cloned());
    }
    own.extend(run.sessions.iter().cloned());
    if !own.is_empty() {
        text.push_str(&sessions(&own));
        text.push('\n');
    }
    text.push_str("| Issue | Time | Cost | Pull request | Merge commit |\n");
    text.push_str("|---|---|---|---|---|\n");
    let dash = || "—".to_owned();
    for issue in &issues {
        text.push_str(&format!(
            "| #{} {} | {} | {} | {} | {} |\n",
            issue.issue.number,
            issue.issue.title.replace('|', "\\|"),
            issue.duration_ms.map(duration).unwrap_or_else(dash),
            issue.cost_usd.map(dollars).unwrap_or_else(dash),
            issue
                .pull_request
                .map(|pr| format!("#{pr}"))
                .unwrap_or_else(dash),
            issue
                .merge_commit
                .as_deref()
                .map(|sha| short(sha).to_owned())
                .unwrap_or_else(dash),
        ));
    }
    #[derive(Serialize)]
    struct Block<'a> {
        version: u32,
        run: String,
        builds: usize,
        duration_ms: u64,
        cost_usd: Decimal,
        pull_request: Option<u64>,
        merge_commit: Option<&'a str>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        sessions: Vec<SessionCost<'a>>,
        issues: Vec<IssueCost<'a>>,
    }
    text.push('\n');
    text.push_str(&hidden(&Block {
        version: VERSION,
        run: run.run.to_string(),
        builds: run.builds.len(),
        duration_ms: run.elapsed_ms,
        cost_usd: run.cost_usd,
        pull_request: merge.map(|merge| merge.pull_request),
        merge_commit: merge.and_then(|merge| merge.merge_commit.as_deref()),
        sessions: run.sessions.iter().map(SessionCost::of).collect(),
        issues: issues.iter().map(IssueCost::of).collect(),
    }));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{BuildStatus, Issue, IssueStatus, Kind, Session, SessionRole};
    use crate::run::{BuildSummary, Disposition, RunKey};
    use chrono::{TimeZone, Utc};
    use std::str::FromStr;

    fn issue(number: u64, title: &str, ms: u64, cost: &str, pr: u64, sha: &str) -> IssueState {
        IssueState {
            issue: Issue {
                number,
                title: title.to_owned(),
                blocked_by: vec![],
            },
            status: IssueStatus::Merged,
            reason: None,
            pull_request: Some(pr),
            merge_commit: Some(sha.to_owned()),
            started_at: None,
            duration_ms: Some(ms),
            cost_usd: Some(Decimal::from_str(cost).unwrap()),
            sessions: Vec::new(),
            activity: None,
        }
    }

    fn session(
        role: SessionRole,
        round: u32,
        model: Option<&str>,
        ms: u64,
        cost: Option<&str>,
    ) -> Session {
        Session {
            build_id: "b".to_owned(),
            role,
            round,
            agent: "claude".to_owned(),
            model: model.map(str::to_owned),
            log: "14.jsonl".to_owned(),
            started_at: Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap(),
            duration_ms: Some(ms),
            cost_usd: cost.map(|cost| Decimal::from_str(cost).unwrap()),
        }
    }

    fn summary(id: &str) -> BuildSummary {
        BuildSummary {
            build_id: id.to_owned(),
            status: BuildStatus::Succeeded,
            reason: None,
            pull_request: None,
            started_at: Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap(),
            ended_at: None,
            elapsed_ms: 0,
            cost_usd: Decimal::ZERO,
        }
    }

    fn run(number: u64, issues: Vec<IssueState>, builds: usize) -> RunState {
        let at = Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap();
        RunState {
            run: RunKey::new("wpm/epik-test", number),
            repo: "wpm/epik-test".to_owned(),
            number,
            title: "Exponentiation".to_owned(),
            kind: Kind::Epic,
            branch: "epic/13".to_owned(),
            disposition: Disposition::WaitingForReview,
            reason: None,
            pull_request: Some(30),
            review: None,
            open_threads: None,
            cost_usd: issues.iter().filter_map(|i| i.cost_usd).sum(),
            issues,
            sessions: Vec::new(),
            builds: (0..builds).map(|n| summary(&n.to_string())).collect(),
            running_build: None,
            started_at: at,
            latest_at: at,
            elapsed_ms: 1_312_000,
        }
    }

    #[test]
    fn a_merged_sub_issue_gets_its_time_cost_pull_request_and_commit() {
        let parse = issue(14, "Parse ^", 185_000, "0.92", 20, "1a2b3c4d5e6f");

        let text = issue_merged("epic/13", &parse);

        assert_eq!(
            text,
            "**Epik: 3m 05s, $0.92.** Merged into `epic/13` by #20 (1a2b3c4).\n\n\
             <!-- epik:cost {\"version\":1,\"number\":14,\"duration_ms\":185000,\
             \"cost_usd\":0.92,\"pull_request\":20,\"merge_commit\":\"1a2b3c4d5e6f\"} -->"
        );
    }

    #[test]
    fn a_merged_sub_issue_names_the_agent_and_model_of_each_session() {
        let mut parse = issue(14, "Parse ^", 185_000, "0.92", 20, "1a2b3c4d5e6f");
        parse.sessions = vec![
            session(SessionRole::Coding, 1, Some("opus"), 125_000, Some("0.92")),
            session(SessionRole::Review, 1, None, 40_000, None),
            session(SessionRole::Coding, 2, Some("opus"), 20_000, None),
        ];

        let text = issue_merged("epic/13", &parse);

        let (readable, block) = text.split_once("<!-- epik:cost ").unwrap();
        assert_eq!(
            readable,
            "**Epik: 3m 05s, $0.92.** Merged into `epic/13` by #20 (1a2b3c4).\n\n\
             - Coding: Claude Code with opus, 2m 05s, $0.92\n\
             - Review: Claude Code with its default model, 40s, cost not reported\n\
             - Coding, round 2: Claude Code with opus, 20s, cost not reported\n\n"
        );
        let json: serde_json::Value =
            serde_json::from_str(block.strip_suffix(" -->").unwrap()).unwrap();
        assert_eq!(
            json["sessions"][1],
            serde_json::json!({
                "role": "review",
                "round": 1,
                "agent": "claude",
                "model": null,
                "duration_ms": 40000,
                "cost_usd": null,
            })
        );
    }

    #[test]
    fn the_closing_comment_names_the_runs_own_sessions_and_a_single_issues() {
        let mut only = issue(7, "One thing", 60_000, "0.50", 8, "abcdef0123");
        only.sessions = vec![session(SessionRole::Coding, 1, None, 60_000, Some("0.50"))];
        let mut run = run(7, vec![only], 1);
        run.sessions = vec![session(
            SessionRole::Review,
            1,
            Some("sonnet"),
            30_000,
            Some("0.10"),
        )];

        let text = closing(&run, None);

        assert!(
            text.contains(
                "\n\n- Coding: Claude Code with its default model, 1m 00s, $0.50\n\
                 - Review: Claude Code with sonnet, 30s, $0.10\n\n| Issue |"
            ),
            "{text}"
        );
    }

    #[test]
    fn the_closing_comment_totals_the_run_and_breaks_it_down_by_issue() {
        let issues = vec![
            issue(
                14,
                "Parse the ^ operator",
                185_000,
                "0.92",
                20,
                "1a2b3c4aaaa",
            ),
            issue(15, "Right | left", 252_000, "1.14", 21, "5d6e7f8bbbb"),
        ];
        let merge = Merge {
            pull_request: 30,
            merge_commit: Some("9999999cccc".to_owned()),
        };

        let text = closing(&run(13, issues, 2), Some(&merge));

        let (table, block) = text.split_once("<!-- epik:cost ").unwrap();
        assert_eq!(
            table,
            "**Epik: built in 2 builds, 21m 52s, $2.06**\n\n\
             Merged by #30 (9999999).\n\n\
             | Issue | Time | Cost | Pull request | Merge commit |\n\
             |---|---|---|---|---|\n\
             | #14 Parse the ^ operator | 3m 05s | $0.92 | #20 | 1a2b3c4 |\n\
             | #15 Right \\| left | 4m 12s | $1.14 | #21 | 5d6e7f8 |\n\n"
        );
        let json: serde_json::Value =
            serde_json::from_str(block.strip_suffix(" -->").unwrap()).unwrap();
        assert_eq!(json["run"], "wpm/epik-test#13");
        assert_eq!(json["builds"], 2);
        assert_eq!(json["cost_usd"], 2.06);
        assert_eq!(json["merge_commit"], "9999999cccc");
        assert_eq!(json["issues"][1]["merge_commit"], "5d6e7f8bbbb");
    }

    #[test]
    fn a_single_issues_run_gets_its_own_pull_request_and_commit() {
        let mut only = issue(7, "One thing", 60_000, "0.50", 0, "");
        only.pull_request = Some(8);
        only.merge_commit = None;
        let merge = Merge {
            pull_request: 8,
            merge_commit: Some("abcdef0123".to_owned()),
        };

        let text = closing(&run(7, vec![only], 1), Some(&merge));

        assert!(text.starts_with("**Epik: built in 1 build, "), "{text}");
        assert!(!text.contains("Merged by"), "{text}");
        assert!(
            text.contains("| #7 One thing | 1m 00s | $0.50 | #8 | abcdef0 |"),
            "{text}"
        );
    }

    #[test]
    fn missing_figures_are_dashes() {
        let mut closed = issue(14, "Closed by hand", 0, "0", 0, "");
        closed.status = IssueStatus::Closed;
        closed.pull_request = None;
        closed.merge_commit = None;
        closed.duration_ms = None;
        closed.cost_usd = None;

        let text = closing(&run(13, vec![closed], 1), None);

        assert!(
            text.contains("| #14 Closed by hand | — | — | — | — |"),
            "{text}"
        );
    }
}
