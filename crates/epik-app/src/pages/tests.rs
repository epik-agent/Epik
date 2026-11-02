use super::*;
use chrono::TimeZone;
use epik_core::activity::Activity;
use epik_core::hosts::{Host, State};
use epik_core::record::{Issue, Session, SessionRole};
use epik_core::run::RunKey;
use rust_decimal::Decimal;
use std::str::FromStr;

fn time(seconds: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 28, 20, 0, 0).unwrap() + chrono::Duration::seconds(seconds)
}

fn dollars_of(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap()
}

fn issue(number: u64, title: &str, blocked_by: &[u64], status: IssueStatus) -> IssueState {
    IssueState {
        issue: Issue {
            number,
            title: title.to_owned(),
            blocked_by: blocked_by.to_vec(),
        },
        status,
        reason: None,
        pull_request: None,
        started_at: None,
        merge_commit: None,
        duration_ms: None,
        cost_usd: None,
        sessions: Vec::new(),
        activity: None,
    }
}

fn merged(number: u64, title: &str, blocked_by: &[u64], pr: u64) -> IssueState {
    IssueState {
        pull_request: Some(pr),
        duration_ms: Some(185_000),
        cost_usd: Some(dollars_of("0.92")),
        ..issue(number, title, blocked_by, IssueStatus::Merged)
    }
}

fn running(
    number: u64,
    title: &str,
    blocked_by: &[u64],
    action: &str,
    test: Option<&str>,
    last: i64,
) -> IssueState {
    IssueState {
        started_at: Some(time(0)),
        activity: Some(Activity {
            last_action: Some(action.to_owned()),
            last_test: test.map(str::to_owned),
            last_event: Some(time(last)),
            recent: vec!["Reading src/eval.rs".to_owned(), action.to_owned()],
            cost_usd: Some(0.48),
        }),
        ..issue(number, title, blocked_by, IssueStatus::Running)
    }
}

fn summary(status: BuildStatus, start: i64, elapsed_ms: u64, cost: &str) -> BuildSummary {
    BuildSummary {
        build_id: format!("b{start}"),
        status,
        reason: None,
        pull_request: None,
        started_at: time(start),
        ended_at: (status != BuildStatus::Running)
            .then(|| time(start) + chrono::Duration::milliseconds(elapsed_ms as i64)),
        elapsed_ms,
        cost_usd: dollars_of(cost),
    }
}

/// Exponentiation, wpm/epik-test#13: #14 and #15 merged, #16 and #17
/// running, #18 waiting on #16 and #19 on #16 and #17; in its second build.
fn exponentiation() -> RunState {
    let issues = vec![
        merged(14, "Parse the ^ operator", &[], 20),
        merged(15, "Right-associative precedence", &[14], 21),
        running(
            16,
            "Integer power evaluation",
            &[15],
            "Running cargo test",
            Some("23 passed, 1 failed"),
            118,
        ),
        running(
            17,
            "Negative and fractional exponents",
            &[15],
            "Running cargo build",
            None,
            10,
        ),
        issue(18, "Overflow errors", &[16], IssueStatus::Waiting),
        issue(19, "README examples", &[16, 17], IssueStatus::Waiting),
    ];
    RunState {
        run: RunKey::new("wpm/epik-test", 13),
        repo: "wpm/epik-test".to_owned(),
        number: 13,
        title: "Exponentiation".to_owned(),
        kind: Kind::Epic,
        branch: "epic/13-exponentiation".to_owned(),
        disposition: Disposition::Building,
        reason: None,
        pull_request: None,
        review: None,
        open_threads: None,
        issues,
        sessions: Vec::new(),
        builds: vec![
            summary(BuildStatus::Interrupted, -3600, 720_000, "1.01"),
            summary(BuildStatus::Running, 0, 0, "2.06"),
        ],
        running_build: Some("b0".to_owned()),
        started_at: time(-3600),
        latest_at: time(118),
        elapsed_ms: 720_000,
        cost_usd: dollars_of("3.07"),
    }
}

fn now() -> DateTime<Utc> {
    time(120)
}

fn waiting_run() -> RunState {
    let mut run = exponentiation();
    run.run = RunKey::new("wpm/epik-test", 9);
    run.number = 9;
    run.title = "Calculator REPL".to_owned();
    run.disposition = Disposition::WaitingForReview;
    run.pull_request = Some(12);
    run.issues = (1..=4).map(|n| merged(n, "Step", &[], n + 100)).collect();
    run.builds = vec![summary(BuildStatus::Succeeded, 0, 860_000, "3.67")];
    run.running_build = None;
    run
}

fn failed_run() -> RunState {
    let mut run = exponentiation();
    run.run = RunKey::new("wpm/epik-test", 7);
    run.number = 7;
    run.title = "Division by zero".to_owned();
    run.kind = Kind::Issue;
    run.disposition = Disposition::NeedsAttention;
    run.reason = Some("CI failed on PR #8".to_owned());
    run.issues = vec![issue(7, "Division by zero", &[], IssueStatus::Failed)];
    run.builds = vec![summary(BuildStatus::Failed, 0, 363_000, "1.76")];
    run.running_build = None;
    run
}

/// `run` on this computer.
fn local(state: RunState) -> HostedRun {
    HostedRun { host: None, state }
}

/// The runs, all on this computer, with no remote hosts.
fn here(runs: impl IntoIterator<Item = RunState>) -> Shown {
    Shown {
        hosts: vec![],
        runs: runs.into_iter().map(local).collect(),
    }
}

/// The host `devbox`, last heard from as `state`.
fn devbox(state: Option<State>) -> Host {
    Host {
        destination: "devbox".to_owned(),
        added_at: time(-7200),
        heard_at: match &state {
            Some(State::Reachable { at, .. }) => Some(*at),
            _ => Some(time(-600)),
        },
        state,
    }
}

/// `devbox`, which answered a second ago.
fn reachable() -> Host {
    devbox(Some(State::Reachable {
        version: "0.4.0".to_owned(),
        at: time(119),
    }))
}

/// `devbox`, which last answered ten minutes ago, and not since.
fn unreachable() -> Host {
    devbox(Some(State::Unreachable {
        error: "ssh: connect to host devbox port 22: Connection refused".to_owned(),
        at: time(118),
    }))
}

/// `state` on `host`.
fn on(host: &Host, state: RunState) -> HostedRun {
    HostedRun {
        host: Some(host.clone()),
        state,
    }
}

/// `devbox` as `host`, and the runs.
fn with(host: &Host, runs: Vec<HostedRun>) -> Shown {
    Shown {
        hosts: vec![host.clone()],
        runs,
    }
}

/// Exponentiation, numbered `number`.
fn numbered(number: u64) -> RunState {
    let mut run = exponentiation();
    run.run = RunKey::new("wpm/epik-test", number);
    run.number = number;
    run
}

#[test]
fn a_building_run_says_what_is_running_next_and_done() {
    let card = run_card(&exponentiation(), now());

    assert_eq!(card.title, "Exponentiation");
    assert_eq!(
        card.epic,
        link(
            "wpm/epik-test#13",
            "https://github.com/wpm/epik-test/issues/13"
        )
    );
    assert_eq!(card.count, "6 issues, 2 merged");
    assert_eq!(card.elapsed, "14m 00s");
    assert_eq!(card.cost, "$3.07");
    assert_eq!(
        card.progress,
        Progress {
            merged: 2,
            running: 2,
            not_started: 2
        }
    );
    let lines: Vec<(String, Option<&str>)> = card
        .now
        .iter()
        .map(|line| (plain(&line.words), line.life.as_deref()))
        .collect();
    assert_eq!(
        lines,
        [
            (
                "#16 Integer power evaluation — running cargo test. Last result: 23 passed, 1 failed.".to_owned(),
                Some("active 2s ago")
            ),
            (
                "#17 Negative and fractional exponents — running cargo build. No tests run yet.".to_owned(),
                Some("quiet 1m 50s")
            ),
        ]
    );
    assert_eq!(
        card.now[0].words[0].url.as_deref(),
        Some("https://github.com/wpm/epik-test/issues/16")
    );
    assert_eq!(
        plain(card.next.as_ref().unwrap()),
        "#18 and #19 start when the issues they depend on are merged."
    );
    assert_eq!(
        plain(card.done.as_ref().unwrap()),
        "#14 and #15 are merged into epic/13-exponentiation (PR #20, PR #21)."
    );
    assert!(
        card.done
            .unwrap()
            .iter()
            .any(|s| s.url.as_deref() == Some("https://github.com/wpm/epik-test/pull/21"))
    );
    assert_eq!(card.sentence, None);
    assert_eq!(
        plain(&card.summary.unwrap()),
        "2 of 6 issues merged. #16 and #17 are in progress."
    );
}

#[test]
fn a_run_waiting_for_review_links_its_pull_request() {
    let card = run_card(&waiting_run(), now());

    assert_eq!(
        plain(card.sentence.as_ref().unwrap()),
        "all 4 issues merged. PR #12 is ready for your review."
    );
    assert_eq!(
        card.note.as_deref(),
        Some("When it merges, Epik comments the total time and cost on #9 and this run goes away.")
    );
    assert!(card.now.is_empty() && card.next.is_none() && card.done.is_none());
}

#[test]
fn a_run_that_needs_attention_says_why() {
    assert_eq!(
        plain(&run_card(&failed_run(), now()).sentence.unwrap()),
        "stopped: CI failed on PR #8."
    );

    let mut interrupted = exponentiation();
    interrupted.disposition = Disposition::NeedsAttention;
    interrupted.builds[1].status = BuildStatus::Interrupted;
    interrupted.reason = Some("the Epik app stopped before the build ended".to_owned());
    assert_eq!(
        plain(&run_card(&interrupted, now()).sentence.unwrap()),
        "interrupted when Epik quit, with 2 of 6 issues merged."
    );

    let mut closed = waiting_run();
    closed.disposition = Disposition::NeedsAttention;
    assert_eq!(
        plain(&run_card(&closed, now()).sentence.unwrap()),
        "its PR #12 was closed without merging."
    );
}

#[test]
fn home_sorts_runs_by_disposition_and_says_what_is_building() {
    let mut second = exponentiation();
    second.run = RunKey::new("wpm/tic-tac-toe", 4);
    let runs = [exponentiation(), waiting_run(), failed_run(), second];

    let home = home(&here(runs), now());

    let keys = |cards: &[RunCard]| cards.iter().map(|c| c.run.clone()).collect::<Vec<_>>();
    assert_eq!(
        keys(&home.building),
        ["wpm/epik-test#13", "wpm/tic-tac-toe#4"]
    );
    assert_eq!(keys(&home.waiting), ["wpm/epik-test#9"]);
    assert_eq!(keys(&home.attention), ["wpm/epik-test#7"]);
    assert!(
        home.headline
            .starts_with("Two runs building. Keeping this "),
        "{}",
        home.headline
    );
    assert!(home.headline.ends_with(" awake until they finish."));
    assert_eq!(
        super::home(&here([]), now()).headline,
        "Nothing is building."
    );
}

#[test]
fn the_sidebar_lists_runs_needing_attention_then_building_then_waiting() {
    let mut second = exponentiation();
    second.run = RunKey::new("wpm/tic-tac-toe", 4);
    second.title = "Tic-tac-toe".to_owned();
    let runs = [exponentiation(), waiting_run(), failed_run(), second];

    let sidebar = sidebar(&here(runs), &[], time(0), now());

    let listed: Vec<(&str, &str, epik_view::Disposition)> = sidebar
        .runs
        .iter()
        .map(|r| (r.run.as_str(), r.title.as_str(), r.disposition))
        .collect();
    assert_eq!(
        listed,
        [
            (
                "wpm/epik-test#7",
                "Division by zero",
                epik_view::Disposition::NeedsAttention
            ),
            (
                "wpm/epik-test#13",
                "Exponentiation",
                epik_view::Disposition::Building
            ),
            (
                "wpm/tic-tac-toe#4",
                "Tic-tac-toe",
                epik_view::Disposition::Building
            ),
            (
                "wpm/epik-test#9",
                "Calculator REPL",
                epik_view::Disposition::WaitingForReview
            ),
        ]
    );
    assert!(!sidebar.missing_repository);
}

#[test]
fn the_sidebar_marks_settings_while_a_repository_is_missing() {
    let missing = Repository {
        repo: "wpm/calc-old".to_owned(),
        added_at: time(0),
        added_by_user: false,
        missing: true,
    };

    assert!(sidebar(&here([]), &[missing], time(0), now()).missing_repository);
}

#[test]
fn the_status_line_says_how_long_epik_has_been_running() {
    let up = |seconds: i64| sidebar(&here([]), &[], time(0), time(seconds)).status;
    assert_eq!(up(0), "Epik running · up 0s");
    assert_eq!(up(4 * 3600 + 12 * 60 + 30), "Epik running · up 4h 12m");
}

#[test]
fn a_single_issue_run_reads_as_one_issue() {
    let mut run = exponentiation();
    run.kind = Kind::Issue;
    run.issues = vec![running(
        4,
        "Computer opponent",
        &[],
        "Editing tests/ai.rs",
        None,
        115,
    )];

    let card = run_card(&run, now());

    assert_eq!(card.count, "1 issue");
    assert_eq!(card.next, None);
    assert_eq!(card.done, None);
    assert_eq!(
        plain(&card.summary.unwrap()),
        "Editing tests/ai.rs. No tests run yet."
    );
}

#[test]
fn an_agent_is_active_for_a_minute_then_quiet() {
    assert_eq!(life(time(0), time(2)), "active 2s ago");
    assert_eq!(life(time(0), time(60)), "active 1m 00s ago");
    assert_eq!(life(time(0), time(110)), "quiet 1m 50s");
}

#[test]
fn the_tray_menu_counts_the_runs_and_agents_and_says_how_long_epik_has_been_up() {
    let runs = [exponentiation(), waiting_run(), failed_run()];

    let menu = popover(&here(runs), time(0), time(4 * 3600 + 12 * 60 + 30));

    assert_eq!(menu.about, "3 runs · 2 agents · up 4h 12m");
}

#[test]
fn restarting_warns_of_the_builds_it_interrupts() {
    assert_eq!(restart_question(0), "Restart Epik?");
    assert_eq!(restart_question(1), "Interrupt one build?");
    assert_eq!(restart_question(2), "Interrupt two builds?");
    assert_eq!(restart_warning(0), "Epik will quit and start again.");
    assert_eq!(
        restart_warning(1),
        "Restarting interrupts the build running now. You can rerun it from its page."
    );
    assert_eq!(
        restart_warning(3),
        "Restarting interrupts the three builds running now. You can rerun each from its page."
    );
}

#[test]
fn the_tray_menu_asks_before_interrupting_its_building_runs() {
    let runs = [exponentiation(), waiting_run(), failed_run()];
    assert_eq!(
        popover(&here(runs), time(0), time(5)).restart,
        "Interrupt one build?"
    );
    assert_eq!(
        popover(&here([]), time(0), time(5)).restart,
        "Restart Epik?"
    );
}

#[test]
fn the_tray_menus_counts_read_as_english() {
    let mut run = exponentiation();
    run.issues.retain(|i| i.issue.number != 17);
    assert_eq!(
        popover(&here([run]), time(0), time(5)).about,
        "1 run · 1 agent · up 5s"
    );
    assert_eq!(
        popover(&here([]), time(0), time(0)).about,
        "0 runs · 0 agents · up 0s"
    );
}

#[test]
fn the_tray_menu_lists_each_building_run_with_its_progress() {
    let runs = [exponentiation(), waiting_run(), failed_run()];

    let menu = popover(&here(runs), time(0), now());

    assert_eq!(menu.runs.len(), 1);
    let run = &menu.runs[0];
    assert_eq!(run.run, "wpm/epik-test#13");
    assert_eq!(run.title, "Exponentiation");
    assert_eq!(run.count, "2/6");
    assert_eq!(
        run.segments,
        [
            NodeState::Merged,
            NodeState::Merged,
            NodeState::Running,
            NodeState::Running,
            NodeState::NotStarted,
            NodeState::NotStarted,
        ]
    );
    assert_eq!(run.line, "building #16 and #17 · 2m 00s · wpm/epik-test#13");
}

#[test]
fn a_tray_menu_row_times_its_longest_running_issue() {
    let mut run = exponentiation();
    run.issues[2].started_at = Some(time(60));
    run.issues[3].started_at = Some(time(30));

    let menu = popover(&here([run]), time(0), now());

    assert_eq!(
        menu.runs[0].line,
        "building #16 and #17 · 1m 30s · wpm/epik-test#13"
    );
}

#[test]
fn a_tray_menu_row_with_nothing_running_is_starting() {
    let mut run = exponentiation();
    run.issues.retain(|i| i.status != IssueStatus::Running);

    let menu = popover(&here([run]), time(0), now());

    assert_eq!(menu.runs[0].line, "starting · wpm/epik-test#13");
}

#[test]
fn the_tray_menu_lists_no_runs_when_nothing_is_building() {
    let menu = popover(&here([waiting_run(), failed_run()]), time(0), now());

    assert!(menu.runs.is_empty());
}

#[test]
fn the_run_page_says_where_the_run_stands_and_draws_each_issue() {
    let zone = FixedOffset::east_opt(0).unwrap();
    let merged_by_build = [vec![14], vec![15]];
    let dir = std::path::Path::new("/runs/wpm/epik-test/13/b0");

    let page = run_page(
        &local(exponentiation()),
        &here([waiting_run()]),
        &merged_by_build,
        Some(dir),
        now(),
        &zone,
    );

    assert_eq!(
        page.sentence,
        "Second build running. 2 of 6 issues merged, 2 agents working."
    );
    assert_eq!(
        plain(&page.about),
        "6 issues · merging into epic/13-exponentiation"
    );
    assert_eq!(
        (page.elapsed.as_str(), page.cost.as_str()),
        ("14m 00s", "$3.07")
    );
    assert_eq!(
        page.builds,
        [
            "Build 2, running since 20:00, has merged #15 so far",
            "Build 1, Oct 28, merged #14, then was interrupted · 12m 00s · $1.01",
        ]
    );
    assert_eq!((page.columns, page.rows), (4, 2));
    let card = |n: u64| page.issues.iter().find(|c| c.number == n).unwrap();
    assert_eq!(plain(&card(14).line), "Merged in PR #20");
    assert_eq!(card(14).figures.as_deref(), Some("3m 05s · $0.92"));
    assert_eq!(
        plain(&card(16).line),
        "cargo test · last: 23 passed, 1 failed"
    );
    assert_eq!(card(16).life.as_deref(), Some("active 2s ago"));
    assert_eq!(plain(&card(17).line), "cargo build · no tests yet");
    assert_eq!(plain(&card(19).line), "Starts when #16 and #17 are merged");
    assert_eq!(plain(&card(18).line), "Starts when #16 is merged");
    let detail = card(16);
    assert_eq!(
        plain(&detail.sentence),
        "Running cargo test. Last result: 23 passed, 1 failed. Its agent is active 2s ago."
    );
    assert_eq!(detail.worked.as_deref(), Some("2m 00s · $0.48 so far"));
    assert_eq!(plain(&detail.depends_on), "#15");
    assert_eq!(plain(&detail.unblocks), "#18 and #19");
    assert_eq!(plain(&detail.pull_request), "Opens when the tests pass");
    assert_eq!(detail.recent, ["Reading src/eval.rs", "Running cargo test"]);
    let log = dir.join("16.jsonl").display().to_string();
    assert_eq!(detail.log.as_deref(), Some(log.as_str()));
    assert_eq!(card(14).log, None);
    assert_eq!(page.also_building, Vec::<(String, String)>::new());
}

#[test]
fn a_running_issues_log_is_its_latest_sessions() {
    let mut run = exponentiation();
    let session = |build_id: &str, log: &str| Session {
        build_id: build_id.to_owned(),
        role: SessionRole::Coding,
        round: 1,
        agent: "claude".to_owned(),
        model: None,
        log: log.to_owned(),
        started_at: time(0),
        duration_ms: None,
        cost_usd: None,
    };
    run.issues[3].sessions = vec![
        session("b-3600", "17.jsonl"),
        session("b0", "17.jsonl"),
        session("b0", "17-coding-2.jsonl"),
    ];
    let dir = std::path::Path::new("/home/me/.epik/runs/wpm/epik-test/13/b0");

    let page = run_page(
        &local(run),
        &here([]),
        &[],
        Some(dir),
        now(),
        &FixedOffset::east_opt(0).unwrap(),
    );

    let card = |n: u64| page.issues.iter().find(|c| c.number == n).unwrap();
    let log = dir.join("17-coding-2.jsonl").display().to_string();
    assert_eq!(card(17).log.as_deref(), Some(log.as_str()));
    let old = dir.join("16.jsonl").display().to_string();
    assert_eq!(card(16).log.as_deref(), Some(old.as_str()), "no sessions");
}

#[test]
fn issues_go_in_columns_by_level_and_rows_in_order() {
    let issues = vec![
        issue(2, "a", &[], IssueStatus::Waiting),
        issue(3, "b", &[], IssueStatus::Waiting),
        issue(4, "c", &[2], IssueStatus::Waiting),
        issue(5, "d", &[4, 2], IssueStatus::Waiting),
        issue(6, "e", &[3, 99], IssueStatus::Waiting),
    ];

    let layout = layout(&issues);

    assert_eq!(
        layout.places,
        [(2, 0, 0), (3, 0, 1), (4, 1, 0), (5, 2, 0), (6, 1, 1)]
    );
    assert_eq!((layout.columns, layout.rows), (3, 2));
    assert_eq!(layout.edges, [(2, 4), (4, 5), (2, 5), (3, 6)]);
}

#[test]
fn settings_lists_each_repository_with_its_runs_and_when_it_was_added() {
    let zone = FixedOffset::east_opt(0).unwrap();
    let repository = |repo: &str, by_user, missing| Repository {
        repo: repo.to_owned(),
        added_at: time(0),
        added_by_user: by_user,
        missing,
    };
    let repositories = [
        repository("wpm/epik-test", false, false),
        repository("epik-agent/Epik", true, false),
        repository("wpm/calc-old", false, true),
    ];

    let settings = settings(&repositories, &[exponentiation(), waiting_run()], &zone);

    let rows: Vec<(&str, &str, usize, bool)> = settings
        .repositories
        .iter()
        .map(|r| (r.repo.as_str(), r.about.as_str(), r.runs.len(), r.missing))
        .collect();
    assert_eq!(
        rows,
        [
            ("wpm/epik-test", "2 runs · added Oct 28", 2, false),
            (
                "epik-agent/Epik",
                "No runs · added by you, Oct 28",
                0,
                false
            ),
            ("wpm/calc-old", "No runs · added Oct 28", 0, true),
        ]
    );
    assert_eq!(
        settings.repositories[0].runs[1],
        ("wpm/epik-test#9".to_owned(), "Calculator REPL".to_owned())
    );
    assert_eq!(
        settings.repositories[0].url,
        "https://github.com/wpm/epik-test"
    );
}

#[test]
fn stopping_monitoring_a_repository_lists_only_its_runs() {
    let zone = FixedOffset::east_opt(0).unwrap();
    let repository = |repo: &str| Repository {
        repo: repo.to_owned(),
        added_at: time(0),
        added_by_user: false,
        missing: false,
    };
    let mut elsewhere = failed_run();
    elsewhere.run = RunKey::new("wpm/calc", 7);
    elsewhere.repo = "wpm/calc".to_owned();

    let settings = settings(
        &[repository("wpm/epik-test"), repository("wpm/calc")],
        &[exponentiation(), elsewhere, waiting_run()],
        &zone,
    );

    assert_eq!(
        settings.repositories[0].runs,
        [
            ("wpm/epik-test#13".to_owned(), "Exponentiation".to_owned()),
            ("wpm/epik-test#9".to_owned(), "Calculator REPL".to_owned()),
        ]
    );
    assert_eq!(
        settings.repositories[1].runs,
        [("wpm/calc#7".to_owned(), "Division by zero".to_owned())]
    );
}

#[test]
fn uninstalling_reports_each_step_by_its_title_with_any_error() {
    use epik_core::uninstall::{Outcome, Step};
    let outcomes = [
        Outcome {
            step: Step::Plugin,
            result: Err("disk full".to_owned()),
        },
        Outcome {
            step: Step::Runs,
            result: Ok(()),
        },
    ];

    assert_eq!(
        uninstall_steps(&outcomes),
        [
            UninstallStep {
                title: "The Epik plugin".to_owned(),
                error: Some("disk full".to_owned()),
            },
            UninstallStep {
                title: "Run history".to_owned(),
                error: None,
            },
        ]
    );
}

#[test]
fn with_no_hosts_where_a_run_builds_goes_without_saying() {
    let home = home(&here([exponentiation()]), now());

    assert_eq!(home.building[0].place, None);
}

#[test]
fn a_hosts_run_says_it_builds_there() {
    let shown = with(&reachable(), vec![on(&reachable(), exponentiation())]);

    let home = home(&shown, now());

    assert_eq!(home.building[0].run, "devbox:wpm/epik-test#13");
    assert_eq!(
        home.building[0].place.as_deref(),
        Some("Building on devbox")
    );
}

#[test]
fn with_a_host_this_computers_runs_say_so() {
    let shown = with(
        &reachable(),
        vec![local(exponentiation()), local(waiting_run())],
    );

    let home = home(&shown, now());

    let building = home.building[0].place.as_deref().unwrap();
    assert!(building.starts_with("Building on this "), "{building}");
    let waiting = home.waiting[0].place.as_deref().unwrap();
    assert!(waiting.starts_with("Built on this "), "{waiting}");
}

#[test]
fn a_finished_run_on_a_host_was_built_there() {
    let shown = with(&reachable(), vec![on(&reachable(), failed_run())]);

    let home = home(&shown, now());

    assert_eq!(home.attention[0].place.as_deref(), Some("Built on devbox"));
}

#[test]
fn an_unreachable_hosts_run_says_when_it_was_last_heard_from() {
    let shown = with(
        &unreachable(),
        vec![
            on(&unreachable(), exponentiation()),
            on(&unreachable(), waiting_run()),
        ],
    );

    let home = home(&shown, now());

    assert_eq!(
        home.building[0].place.as_deref(),
        Some(
            "Building on devbox, last heard from 12m 00s ago. \
             The build may still be going on there."
        )
    );
    assert_eq!(
        home.waiting[0].place.as_deref(),
        Some("Built on devbox, last heard from 12m 00s ago.")
    );
}

#[test]
fn a_host_never_heard_from_says_so() {
    let mut host = unreachable();
    host.heard_at = None;
    let shown = with(&host, vec![on(&host, exponentiation())]);

    let home = home(&shown, now());

    assert_eq!(
        home.building[0].place.as_deref(),
        Some(
            "Building on devbox, which hasn't answered yet. \
             The build may still be going on there."
        )
    );
}

#[test]
fn the_headline_keeps_awake_only_for_builds_here() {
    let host = reachable();
    let remote = |n| on(&host, numbered(n));

    let only_remote = home(&with(&host, vec![remote(1), remote(2)]), now());
    assert!(
        only_remote
            .headline
            .starts_with("Two runs building on devbox. This "),
        "{}",
        only_remote.headline
    );
    assert!(only_remote.headline.ends_with(" can sleep."));

    let one_remote = home(&with(&host, vec![remote(1)]), now());
    assert!(
        one_remote
            .headline
            .starts_with("One run building on devbox. This "),
        "{}",
        one_remote.headline
    );

    let elsewhere = Host {
        destination: "me@build.example.com".to_owned(),
        ..reachable()
    };
    let two_hosts = Shown {
        hosts: vec![host.clone(), elsewhere.clone()],
        runs: vec![remote(1), on(&elsewhere, numbered(2))],
    };
    assert!(
        home(&two_hosts, now())
            .headline
            .starts_with("Two runs building on other hosts. This "),
    );

    let both = home(
        &with(&host, vec![remote(1), remote(2), local(numbered(3))]),
        now(),
    );
    assert!(
        both.headline
            .starts_with("Three runs building. Keeping this "),
        "{}",
        both.headline
    );
    assert!(
        both.headline
            .ends_with(" awake until the one here finishes."),
        "{}",
        both.headline
    );

    let two_here = home(
        &with(
            &host,
            vec![remote(1), local(numbered(2)), local(numbered(3))],
        ),
        now(),
    );
    assert!(
        two_here
            .headline
            .ends_with(" awake until the two here finish."),
        "{}",
        two_here.headline
    );
}

#[test]
fn a_run_on_a_host_and_one_here_are_listed_apart() {
    let shown = with(
        &reachable(),
        vec![on(&reachable(), exponentiation()), local(exponentiation())],
    );

    let sidebar = sidebar(&shown, &[], time(0), now());

    let listed: Vec<&str> = sidebar.runs.iter().map(|r| r.run.as_str()).collect();
    assert_eq!(listed, ["devbox:wpm/epik-test#13", "wpm/epik-test#13"]);
}

#[test]
fn the_tray_menu_says_where_each_run_builds() {
    let shown = with(
        &unreachable(),
        vec![on(&unreachable(), exponentiation()), local(numbered(4))],
    );

    let menu = popover(&shown, time(0), now());

    assert_eq!(menu.runs[0].run, "devbox:wpm/epik-test#13");
    assert_eq!(
        menu.runs[0].place.as_deref(),
        Some(
            "Building on devbox, last heard from 12m 00s ago. \
             The build may still be going on there."
        )
    );
    let here = menu.runs[1].place.as_deref().unwrap();
    assert!(here.starts_with("Building on this "), "{here}");
}

#[test]
fn restarting_interrupts_only_the_builds_here() {
    let host = reachable();
    let shown = with(&host, vec![on(&host, numbered(1)), local(numbered(2))]);
    assert_eq!(
        popover(&shown, time(0), now()).restart,
        "Interrupt one build?"
    );

    let only_remote = with(&host, vec![on(&host, numbered(1))]);
    assert_eq!(
        popover(&only_remote, time(0), now()).restart,
        "Restart Epik?"
    );
}

#[test]
fn a_hosts_run_page_says_where_it_builds_and_links_to_the_others_by_host() {
    let zone = FixedOffset::east_opt(0).unwrap();
    let host = unreachable();
    let run = on(&host, exponentiation());
    let shown = with(&host, vec![run.clone(), on(&host, numbered(4))]);

    let page = run_page(&run, &shown, &[], None, now(), &zone);

    assert_eq!(page.run, "devbox:wpm/epik-test#13");
    assert_eq!(
        page.place.as_deref(),
        Some(
            "Building on devbox, last heard from 12m 00s ago. \
             The build may still be going on there."
        )
    );
    assert_eq!(
        page.also_building,
        [(
            "devbox:wpm/epik-test#4".to_owned(),
            "Exponentiation".to_owned()
        )]
    );
}

#[test]
fn a_run_here_doesnt_count_a_hosts_run_of_the_same_epic_as_itself() {
    let zone = FixedOffset::east_opt(0).unwrap();
    let host = reachable();
    let here = local(exponentiation());
    let shown = with(&host, vec![here.clone(), on(&host, exponentiation())]);

    let page = run_page(&here, &shown, &[], None, now(), &zone);

    assert_eq!(
        page.also_building,
        [(
            "devbox:wpm/epik-test#13".to_owned(),
            "Exponentiation".to_owned()
        )]
    );
}
