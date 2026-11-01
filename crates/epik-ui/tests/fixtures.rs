//! Runs and spans for the tests to draw.

#![allow(dead_code)]

use leptos::prelude::{Owner, RenderHtml};

use epik_view::{
    Disposition, Home, IssueCard, Layout, MiniGraph, NodeState, NowLine, Popover, Progress,
    RepositoryRow, RunCard, RunPage, Sidebar, SidebarRun, Span, TrayRun,
};

pub fn text(text: &str) -> Span {
    Span {
        text: text.into(),
        url: None,
        code: false,
    }
}

pub fn link(text: &str, url: &str) -> Span {
    Span {
        text: text.into(),
        url: Some(url.into()),
        code: false,
    }
}

pub fn code(text: &str) -> Span {
    Span {
        text: text.into(),
        url: None,
        code: true,
    }
}

/// Three issues: #1 blocks #2 and #3.
pub fn graph() -> MiniGraph {
    MiniGraph {
        layout: Layout {
            columns: 2,
            rows: 2,
            places: vec![(1, 0, 0), (2, 1, 0), (3, 1, 1)],
            edges: vec![(1, 2), (1, 3)],
        },
        states: vec![NodeState::Merged, NodeState::Running, NodeState::NotStarted],
    }
}

pub fn now(number: u64, life: Option<&str>, last_event: Option<i64>) -> NowLine {
    NowLine {
        number,
        words: vec![text(&format!("#{number}"))],
        life: life.map(Into::into),
        last_event,
    }
}

pub fn card(now: Vec<NowLine>) -> RunCard {
    RunCard {
        run: "acme/widgets#13".into(),
        title: "Exponentiation".into(),
        place: None,
        epic: link(
            "acme/widgets#13",
            "https://github.com/acme/widgets/issues/13",
        ),
        count: "3 issues, 1 merged".into(),
        disposition: Disposition::Building,
        elapsed: "4m 12s".into(),
        cost: "$1.20".into(),
        graph: graph(),
        progress: Progress {
            merged: 1,
            running: 1,
            not_started: 1,
        },
        now,
        next: None,
        done: None,
        sentence: None,
        note: None,
        summary: Some(vec![
            text("1 of 3 merged, "),
            link("#2", "https://github.com/acme/widgets/issues/2"),
            text(" running"),
        ]),
    }
}

/// Exponentiation, building, with three issues: one merged, one running,
/// one waiting.
pub fn tray_run() -> TrayRun {
    TrayRun {
        run: "acme/widgets#13".into(),
        title: "Exponentiation".into(),
        place: None,
        count: "1/3".into(),
        segments: vec![NodeState::Merged, NodeState::Running, NodeState::NotStarted],
        line: "building #2 · 2m 00s · acme/widgets#13".into(),
    }
}

pub fn popover(runs: Vec<TrayRun>) -> Popover {
    Popover {
        about: "3 runs · 4 agents · up 4h 12m".into(),
        restart: "Interrupt one build?".into(),
        runs,
    }
}

/// A run that has ended, waiting for review or needing attention.
pub fn ended(run: &str, disposition: Disposition) -> RunCard {
    RunCard {
        run: run.into(),
        disposition,
        sentence: Some(vec![
            text("Merged into "),
            link("PR #30", "https://github.com/acme/widgets/pull/30"),
        ]),
        ..card(vec![])
    }
}

pub fn home(building: Vec<RunCard>, waiting: Vec<RunCard>, attention: Vec<RunCard>) -> Home {
    Home {
        headline: "One run building".into(),
        building,
        waiting,
        attention,
    }
}

pub fn sidebar_run(run: &str, title: &str, disposition: Disposition) -> SidebarRun {
    SidebarRun {
        run: run.into(),
        title: title.into(),
        disposition,
    }
}

pub fn sidebar(runs: Vec<SidebarRun>) -> Sidebar {
    Sidebar {
        runs,
        missing_repository: false,
        status: "Epik running · up 4h 12m".into(),
    }
}

/// Issue `number` of acme/widgets, in `state`, at `column` and `row`.
pub fn issue(number: u64, state: NodeState, column: usize, row: usize) -> IssueCard {
    IssueCard {
        number,
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        title: "Add a power operator".into(),
        state,
        column,
        row,
        figures: None,
        line: vec![
            text("Starts when "),
            link("#1", "https://github.com/acme/widgets/issues/1"),
            text(" is merged"),
        ],
        life: None,
        last_event: None,
        sentence: vec![text("Waiting for #1")],
        worked: None,
        depends_on: vec![link("#1", "https://github.com/acme/widgets/issues/1")],
        unblocks: vec![text("nothing")],
        pull_request: vec![text("none yet")],
        recent: vec![],
        log: None,
    }
}

/// A run of three issues: #1 blocks #2 and #3.
pub fn run_page() -> RunPage {
    RunPage {
        run: "acme/widgets#13".into(),
        title: "Exponentiation".into(),
        place: None,
        epic: link(
            "acme/widgets#13",
            "https://github.com/acme/widgets/issues/13",
        ),
        about: vec![
            text("3 issues · merging into "),
            code("epic/13-exponentiation"),
        ],
        sentence: "First build running. 1 of 3 issues merged, 1 agent working.".into(),
        disposition: Disposition::Building,
        elapsed: "4m 12s".into(),
        cost: "$1.20".into(),
        also_building: vec![],
        builds: vec!["First build, started 10:02".into()],
        columns: 2,
        rows: 2,
        issues: vec![
            issue(1, NodeState::Merged, 0, 0),
            issue(2, NodeState::Running, 1, 0),
            issue(3, NodeState::NotStarted, 1, 1),
        ],
        edges: vec![(1, 2), (1, 3)],
    }
}

pub fn repository(repo: &str, runs: Vec<(&str, &str)>, missing: bool) -> RepositoryRow {
    RepositoryRow {
        repo: repo.into(),
        url: format!("https://github.com/{repo}"),
        about: "2 runs · added Oct 20".into(),
        runs: runs
            .into_iter()
            .map(|(run, title)| (run.into(), title.into()))
            .collect(),
        missing,
    }
}

/// The HTML of the view `draw` draws, without the markers Leptos leaves
/// for hydration. It draws under a reactive owner, as in a window.
pub fn render<V: RenderHtml>(draw: impl FnOnce() -> V) -> String {
    Owner::new().with(|| draw().to_html().replace("<!>", ""))
}
