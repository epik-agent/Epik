//! A run's small graph and its progress bar.

use epik_view::{NodeState, Progress};
use leptos::prelude::*;

/// The distance between columns, and between rows.
const STEP: usize = 16;
const ROW: usize = 12;
/// A dot's radius.
const R: usize = 4;

fn state_class(state: NodeState) -> &'static str {
    match state {
        NodeState::Merged => "merged",
        NodeState::Running => "running",
        NodeState::NotStarted => "not_started",
        NodeState::Failed => "failed",
    }
}

/// A run's small graph: its issues as dots by state, joined by their
/// dependencies.
#[component]
pub fn MiniGraph(graph: epik_view::MiniGraph) -> impl IntoView {
    let epik_view::MiniGraph { layout, states } = graph;
    let width = layout.columns.max(1) * STEP;
    let height = layout.rows.max(1) * ROW;
    let dots: Vec<(u64, usize, usize, NodeState)> = layout
        .places
        .iter()
        .zip(states)
        .map(|(&(number, column, row), state)| {
            (number, column * STEP + R + 2, row * ROW + 6, state)
        })
        .collect();
    let at = |number: u64| {
        dots.iter()
            .find(|dot| dot.0 == number)
            .map(|dot| (dot.1, dot.2))
    };
    let edges = layout
        .edges
        .iter()
        .filter_map(|&(from, to)| Some((at(from)?, at(to)?)))
        .map(|((x1, y1), (x2, y2))| view! { <line x1=x1 y1=y1 x2=x2 y2=y2 class="mini-edge"/> })
        .collect_view();
    let dots = dots
        .iter()
        .map(|&(_, x, y, state)| {
            view! { <circle cx=x cy=y r=R class=format!("mini-node {}", state_class(state))/> }
        })
        .collect_view();
    view! {
        <svg class="mini-graph" viewBox=format!("0 0 {width} {height}") width=width height=height>
            {edges}
            {dots}
        </svg>
    }
}

/// A bar of merged, running and not-started issues.
#[component]
pub fn ProgressBar(progress: Progress) -> impl IntoView {
    let total = (progress.merged + progress.running + progress.not_started).max(1) as f64;
    let grow = move |count: usize| (count as f64 / total).to_string();
    view! {
        <div class="progress">
            <span class="merged" style:flex-grow=grow(progress.merged)></span>
            <span class="running" style:flex-grow=grow(progress.running)></span>
            <span class="not_started" style:flex-grow=grow(progress.not_started)></span>
        </div>
    }
}
