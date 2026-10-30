use epik_ui::graph::{MiniGraph, ProgressBar};
use epik_view::Progress;
use leptos::prelude::*;

mod fixtures;
use fixtures::render;

#[test]
fn a_mini_graph_is_sized_by_its_columns_and_rows() {
    let html = render(|| view! { <MiniGraph graph=fixtures::graph()/> });
    assert!(html.contains(r#"class="mini-graph""#), "{html}");
    assert!(html.contains(r#"viewBox="0 0 32 24""#), "{html}");
    assert!(html.contains(r#"width="32""#), "{html}");
    assert!(html.contains(r#"height="24""#), "{html}");
}

#[test]
fn a_mini_graph_has_a_dot_for_each_issue_by_state() {
    let html = render(|| view! { <MiniGraph graph=fixtures::graph()/> });
    for dot in [
        r#"<circle cx="6" cy="6" r="4" class="mini-node merged"></circle>"#,
        r#"<circle cx="22" cy="6" r="4" class="mini-node running"></circle>"#,
        r#"<circle cx="22" cy="18" r="4" class="mini-node not_started"></circle>"#,
    ] {
        assert!(html.contains(dot), "{dot} in {html}");
    }
}

#[test]
fn a_mini_graph_joins_its_dots_by_their_edges_under_them() {
    let html = render(|| view! { <MiniGraph graph=fixtures::graph()/> });
    let edges = [
        r#"<line x1="6" y1="6" x2="22" y2="6" class="mini-edge"></line>"#,
        r#"<line x1="6" y1="6" x2="22" y2="18" class="mini-edge"></line>"#,
    ];
    for edge in edges {
        assert!(html.contains(edge), "{edge} in {html}");
    }
    assert!(
        html.find(edges[1]).unwrap() < html.find("<circle").unwrap(),
        "{html}"
    );
}

#[test]
fn an_empty_mini_graph_is_one_step_square() {
    let mut graph = fixtures::graph();
    graph.layout.columns = 0;
    graph.layout.rows = 0;
    let html = render(|| view! { <MiniGraph graph/> });
    assert!(html.contains(r#"viewBox="0 0 16 12""#), "{html}");
}

#[test]
fn a_progress_bar_has_a_segment_for_each_part_sized_by_count() {
    let progress = Progress {
        merged: 1,
        running: 1,
        not_started: 2,
    };
    let html = render(|| view! { <ProgressBar progress/> });
    assert!(html.starts_with(r#"<div class="progress">"#), "{html}");
    let merged = html.find(r#"<span class="merged" style="flex-grow:0.25;"></span>"#);
    let running = html.find(r#"<span class="running" style="flex-grow:0.25;"></span>"#);
    let not_started = html.find(r#"<span class="not_started" style="flex-grow:0.5;"></span>"#);
    assert!(
        merged.is_some() && running.is_some() && not_started.is_some(),
        "{html}"
    );
    assert!(merged < running && running < not_started, "{html}");
}

#[test]
fn an_empty_progress_bar_has_empty_segments() {
    let progress = Progress {
        merged: 0,
        running: 0,
        not_started: 0,
    };
    let html = render(|| view! { <ProgressBar progress/> });
    assert_eq!(html.matches(r#"style="flex-grow:0;""#).count(), 3, "{html}");
}
