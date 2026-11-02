//! Each window's view model survives the trip to JSON and back, as it does
//! between the app and its front end.

use epik_view::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: T) {
    let json = serde_json::to_string(&value).unwrap();
    let back: T = serde_json::from_str(&json).unwrap();
    assert_eq!(back, value, "{json}");
}

fn text(text: &str) -> Span {
    Span {
        text: text.to_owned(),
        url: None,
        code: false,
    }
}

fn link(text: &str, url: &str) -> Span {
    Span {
        text: text.to_owned(),
        url: Some(url.to_owned()),
        code: false,
    }
}

fn code(text: &str) -> Span {
    Span {
        text: text.to_owned(),
        url: None,
        code: true,
    }
}

fn card(disposition: Disposition) -> RunCard {
    RunCard {
        run: "o/r#13".to_owned(),
        title: "Exponentiation".to_owned(),
        place: None,
        epic: link("o/r#13", "https://github.com/o/r/issues/13"),
        count: "2 issues, 1 merged".to_owned(),
        disposition,
        elapsed: "3m 05s".to_owned(),
        cost: "$0.92".to_owned(),
        graph: MiniGraph {
            layout: Layout {
                columns: 2,
                rows: 1,
                places: vec![(14, 0, 0), (15, 1, 0)],
                edges: vec![(14, 15)],
            },
            states: vec![NodeState::Merged, NodeState::Running],
        },
        progress: Progress {
            merged: 1,
            running: 1,
            not_started: 0,
        },
        now: vec![NowLine {
            number: 15,
            words: vec![
                link("#15", "https://github.com/o/r/issues/15"),
                text(" Power"),
            ],
            life: Some("active 2s ago".to_owned()),
            last_event: Some(1_793_000_000_000),
        }],
        next: None,
        done: Some(vec![text("#14 is merged into "), code("epic/13")]),
        sentence: None,
        note: Some("A note".to_owned()),
        summary: Some(vec![text("1 of 2 merged")]),
    }
}

#[test]
fn home() {
    round_trip(Home {
        headline: "One run building.".to_owned(),
        building: vec![card(Disposition::Building)],
        waiting: vec![card(Disposition::WaitingForReview)],
        attention: vec![card(Disposition::NeedsAttention)],
    });
}

#[test]
fn sidebar() {
    round_trip(Sidebar {
        runs: vec![SidebarRun {
            run: "devbox:o/r#13".to_owned(),
            title: "Exponentiation".to_owned(),
            disposition: Disposition::NeedsAttention,
        }],
        missing_repository: true,
        status: "Epik running · up 4h 12m".to_owned(),
    });
}

#[test]
fn popover() {
    round_trip(Popover {
        about: "1 run · 2 agents · up 4h 12m".to_owned(),
        restart: "Interrupt one build?".to_owned(),
        runs: vec![TrayRun {
            run: "devbox:o/r#13".to_owned(),
            title: "Exponentiation".to_owned(),
            place: Some("Building on devbox".to_owned()),
            count: "1/3".to_owned(),
            segments: vec![NodeState::Merged, NodeState::Running, NodeState::Failed],
            line: "building #2 · 2m 00s · o/r#13".to_owned(),
        }],
    });
}

#[test]
fn run_page() {
    round_trip(RunPage {
        run: "o/r#13".to_owned(),
        title: "Exponentiation".to_owned(),
        place: Some("Building on devbox, last heard from 12m 00s ago.".to_owned()),
        epic: link("o/r#13", "https://github.com/o/r/issues/13"),
        about: vec![text("2 issues · merging into "), code("epic/13")],
        sentence: "First build running.".to_owned(),
        disposition: Disposition::Building,
        elapsed: "3m 05s".to_owned(),
        cost: "$0.92".to_owned(),
        also_building: vec![("o/s#2".to_owned(), "Other".to_owned())],
        builds: vec!["The first build is running.".to_owned()],
        columns: 2,
        rows: 1,
        issues: vec![
            IssueCard {
                number: 14,
                url: "https://github.com/o/r/issues/14".to_owned(),
                title: "Square".to_owned(),
                state: NodeState::Merged,
                column: 0,
                row: 0,
                figures: Some("3m 05s · $0.92".to_owned()),
                line: vec![
                    text("Merged in "),
                    link("PR #20", "https://github.com/o/r/pull/20"),
                ],
                life: None,
                last_event: None,
                sentence: vec![text("Merged.")],
                worked: None,
                depends_on: vec![],
                unblocks: vec![link("#15", "https://github.com/o/r/issues/15")],
                pull_request: vec![link("PR #20", "https://github.com/o/r/pull/20")],
                recent: vec!["Ran cargo test".to_owned()],
                log: Some("the log".to_owned()),
            },
            IssueCard {
                number: 15,
                url: "https://github.com/o/r/issues/15".to_owned(),
                title: "Power".to_owned(),
                state: NodeState::NotStarted,
                column: 1,
                row: 0,
                figures: None,
                line: vec![text("Starts when its build reaches it")],
                life: Some("quiet 1m 50s".to_owned()),
                last_event: Some(1_793_000_000_000),
                sentence: vec![],
                worked: Some("2m 40s · $0.48 so far".to_owned()),
                depends_on: vec![link("#14", "https://github.com/o/r/issues/14")],
                unblocks: vec![],
                pull_request: vec![],
                recent: vec![],
                log: None,
            },
        ],
        edges: vec![(14, 15)],
    });
}

#[test]
fn settings() {
    round_trip(Settings {
        repositories: vec![RepositoryRow {
            repo: "o/r".to_owned(),
            url: "https://github.com/o/r".to_owned(),
            about: "1 run · added Oct 20".to_owned(),
            runs: vec![("o/r#13".to_owned(), "Exponentiation".to_owned())],
            missing: false,
        }],
        hosts: vec![HostRow {
            destination: Some("devbox".to_owned()),
            name: "devbox".to_owned(),
            status: HostStatus::Incomplete,
            about: "Epik 0.4.0 · setup incomplete · No runs".to_owned(),
            why: None,
            missing: vec![SetupFix {
                title: "Claude sign-in".to_owned(),
                detail: "not signed in".to_owned(),
                commands: vec!["claude auth login".to_owned()],
            }],
            default: true,
            runs: vec![],
        }],
    });
}

#[test]
fn checklist() {
    round_trip(Checklist {
        rows: vec![
            Row {
                id: "gh".to_owned(),
                title: "GitHub CLI".to_owned(),
                state: ItemState::Done,
                detail: "gh 2.80".to_owned(),
                button: None,
                needed: true,
            },
            Row {
                id: "gh-auth".to_owned(),
                title: "Signed in to GitHub".to_owned(),
                state: ItemState::Missing,
                detail: "not signed in".to_owned(),
                button: Some("Sign in to GitHub".to_owned()),
                needed: true,
            },
            Row {
                id: "node".to_owned(),
                title: "Node".to_owned(),
                state: ItemState::Outdated,
                detail: "Node 22".to_owned(),
                button: Some("Update the private copy".to_owned()),
                needed: false,
            },
        ],
        complete: false,
    });
}

#[test]
fn output() {
    round_trip(Output {
        lines: vec!["Installing…".to_owned()],
        running: false,
        error: Some("it failed".to_owned()),
    });
}

#[test]
fn stream() {
    let lines = |lines: &[&str]| lines.iter().map(|line| line.to_string()).collect();
    round_trip(Stream {
        entries: vec![
            StreamEntry {
                at: Some(1_700_000_000_000),
                what: Entry::Narration {
                    text: "I'll read the issue.".to_owned(),
                },
            },
            StreamEntry {
                at: None,
                what: Entry::Command {
                    command: "cargo test".to_owned(),
                    output: Excerpt {
                        head: lines(&["running 40 tests"]),
                        more: 30,
                        tail: lines(&["test result: ok"]),
                    },
                },
            },
            StreamEntry {
                at: None,
                what: Entry::Read {
                    path: "src/eval.rs".to_owned(),
                    lines: Some(LineRange { from: 10, to: None }),
                },
            },
            StreamEntry {
                at: None,
                what: Entry::Search {
                    pattern: "fn eval".to_owned(),
                    within: Some("src".to_owned()),
                    matches: 1,
                    hits: Excerpt {
                        head: lines(&["src/eval.rs:3"]),
                        ..Excerpt::default()
                    },
                },
            },
            StreamEntry {
                at: None,
                what: Entry::Edit {
                    path: "src/eval.rs".to_owned(),
                    changes: Excerpt {
                        head: vec![
                            Change::Removed("a".to_owned()),
                            Change::Added("b".to_owned()),
                        ],
                        ..Excerpt::default()
                    },
                },
            },
            StreamEntry {
                at: None,
                what: Entry::NewFile {
                    path: "src/new.rs".to_owned(),
                    content: Excerpt::default(),
                },
            },
            StreamEntry {
                at: None,
                what: Entry::Other {
                    text: "Updating its plan".to_owned(),
                },
            },
        ],
        working: true,
    });
}

#[test]
fn the_json_is_what_the_windows_read() {
    let json = serde_json::to_value((
        text("plain"),
        Disposition::WaitingForReview,
        Disposition::Reviewing,
        ItemState::Outdated,
        NodeState::NotStarted,
    ))
    .unwrap();
    assert_eq!(
        json,
        serde_json::json!([
            {"text": "plain"},
            "waiting_for_review",
            "reviewing",
            "outdated",
            "not_started"
        ])
    );
}
