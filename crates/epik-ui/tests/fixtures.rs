//! Runs and spans for the tests to draw.

#![allow(dead_code)]

use leptos::prelude::RenderHtml;

use epik_view::{
    Disposition, Layout, MiniGraph, NodeState, NowLine, Popover, Progress, RunCard, Span,
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

pub fn popover(building: Vec<RunCard>, waiting: Vec<(Disposition, String, Vec<Span>)>) -> Popover {
    Popover {
        headline: "One run building".into(),
        building,
        waiting,
    }
}

/// The HTML of `view`, without the markers Leptos leaves for hydration.
pub fn render(view: impl RenderHtml) -> String {
    view.to_html().replace("<!>", "")
}
