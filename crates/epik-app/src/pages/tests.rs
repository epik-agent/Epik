use super::*;
use chrono::TimeZone;
use epik_core::activity::Activity;
use epik_core::record::Issue;
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
        issues,
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
    let missing = Repository {
        repo: "wpm/calc-old".to_owned(),
        added_at: time(0),
        added_by_user: false,
        missing: true,
    };

    let home = home(&runs, &[missing], now());

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
    assert!(home.missing_repository);
    assert_eq!(
        super::home(&[], &[], now()).headline,
        "Nothing is building."
    );
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
fn the_popover_lists_building_runs_and_the_rest_in_sentences() {
    let popover = popover(&[exponentiation(), waiting_run(), failed_run()], now());

    assert_eq!(popover.headline, "One run building");
    assert_eq!(popover.building.len(), 1);
    let sentences: Vec<String> = popover.waiting.iter().map(|(_, _, w)| plain(w)).collect();
    assert_eq!(
        sentences,
        [
            "Calculator REPL is done. PR #12 is ready for your review.",
            "Division by zero stopped: CI failed on PR #8.",
        ]
    );
}

#[test]
fn the_run_page_says_where_the_run_stands_and_draws_each_issue() {
    let zone = FixedOffset::east_opt(0).unwrap();
    let merged_by_build = [vec![14], vec![15]];
    let dir = std::path::Path::new("/runs/wpm/epik-test/13/b0");

    let page = run_page(
        &exponentiation(),
        &[waiting_run()],
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
