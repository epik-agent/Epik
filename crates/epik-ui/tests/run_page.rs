use epik_ui::run_page::{Detail, Edges, Issue, RunView};
use epik_view::NodeState;
use leptos::prelude::*;

mod fixtures;
use fixtures::{issue, link, render, run_page};

#[test]
fn an_issue_card_is_placed_by_its_column_and_row() {
    let html =
        render(|| view! { <Issue issue=issue(3, NodeState::NotStarted, 1, 1) chosen=false/> });
    assert!(html.starts_with(r##"<button title="#3 Add a power operator" class="issue not_started" style="left:256px;top:94px;width:200px;height:76px;">"##), "{html}");
    assert!(html.contains(r#"<div class="top"><span class="dot"></span><a href="https://github.com/acme/widgets/issues/3">#3</a></div>"#), "{html}");
    assert!(html.ends_with(r#"<div class="name">Add a power operator</div><span class="card-line">Starts when <a href="https://github.com/acme/widgets/issues/1">#1</a> is merged</span></button>"#), "{html}");
}

#[test]
fn a_merged_issue_card_has_its_figures() {
    let mut merged = issue(1, NodeState::Merged, 0, 0);
    merged.figures = Some("3m 05s · $0.92".into());
    let html = render(|| view! { <Issue issue=merged chosen=false/> });
    assert!(
        html.contains(r##"class="issue merged" style="left:0px;top:0px;"##),
        "{html}"
    );
    assert!(
        html.contains(r#"#1</a><span class="figures-small">3m 05s · $0.92</span></div>"#),
        "{html}"
    );
}

#[test]
fn a_running_issue_card_has_its_life_line_rather_than_its_figures() {
    let mut running = issue(2, NodeState::Running, 1, 0);
    running.figures = Some("1m 00s · $0.20".into());
    running.life = Some("active 2s ago".into());
    let html = render(|| view! { <Issue issue=running.clone() chosen=false/> });
    assert!(html.contains(r#"class="issue running""#), "{html}");
    assert!(
        html.contains(r#"#2</a><span class="life">active 2s ago</span></div>"#),
        "{html}"
    );
    assert!(!html.contains("figures-small"), "{html}");
    running.life = Some("quiet 1m 50s".into());
    let html = render(|| view! { <Issue issue=running chosen=false/> });
    assert!(
        html.contains(r#"<span class="life quiet">quiet 1m 50s</span>"#),
        "{html}"
    );
}

#[test]
fn a_failed_issue_card_is_failed() {
    let html = render(|| view! { <Issue issue=issue(2, NodeState::Failed, 1, 0) chosen=false/> });
    assert!(html.contains(r#"class="issue failed""#), "{html}");
}

#[test]
fn the_chosen_issue_card_is_chosen() {
    let html = render(|| view! { <Issue issue=issue(2, NodeState::Running, 1, 0) chosen=true/> });
    assert!(html.contains(r#"class="issue running chosen""#), "{html}");
}

#[test]
fn the_edges_join_a_blocker_to_what_it_blocks() {
    let page = run_page();
    let html = render(|| view! { <Edges issues=page.issues edges=page.edges/> });
    assert_eq!(
        html,
        concat!(
            r#"<path d="M200,38 C228,38 228,38 256,38" class="edge"></path>"#,
            r#"<path d="M200,38 C228,38 228,132 256,132" class="edge"></path>"#,
        )
    );
}

#[test]
fn an_edge_across_columns_bends_halfway() {
    let issues = vec![
        issue(1, NodeState::Merged, 0, 0),
        issue(2, NodeState::Running, 2, 0),
    ];
    let html = render(|| view! { <Edges issues edges=vec![(1, 2)]/> });
    assert_eq!(
        html,
        r#"<path d="M200,38 C356,38 356,38 512,38" class="edge"></path>"#
    );
}

#[test]
fn the_detail_panel_has_the_issues_facts_and_activity() {
    let mut running = issue(2, NodeState::Running, 1, 0);
    running.worked = Some("2m 40s · $0.48 so far".into());
    running.recent = vec!["Read src/lib.rs".into(), "Ran cargo test".into()];
    running.pull_request = vec![link("PR #31", "https://github.com/acme/widgets/pull/31")];
    let html = render(|| view! { <Detail issue=running/> });
    assert!(html.starts_with(r#"<section class="detail"><div class="facts"><h3><a href="https://github.com/acme/widgets/issues/2">#2</a> Add a power operator</h3><span class="state running">Waiting for #1</span>"#), "{html}");
    assert!(html.contains(r#"<dl><dt>Working for</dt><dd>2m 40s · $0.48 so far</dd><dt>Depends on</dt><dd><span><a href="https://github.com/acme/widgets/issues/1">#1</a></span></dd><dt>Unblocks</dt><dd><span>nothing</span></dd><dt>Pull request</dt><dd><span><a href="https://github.com/acme/widgets/pull/31">PR #31</a></span></dd></dl></div>"#), "{html}");
    assert!(html.ends_with(r#"<div class="doing"><div class="doing-head"><span class="label">What the agent has done</span></div><ol class="recent"><li>Read src/lib.rs</li><li>Ran cargo test</li></ol></div></section>"#), "{html}");
}

#[test]
fn the_detail_panel_links_to_the_full_log() {
    let mut running = issue(2, NodeState::Running, 1, 0);
    running.log = Some("/Users/me/.epik/runs/acme/widgets/13/1/2.log".into());
    let html = render(|| view! { <Detail issue=running/> });
    assert!(html.contains(r##"<span class="label">What the agent has done</span><a href="#" class="full-log">Full log</a></div>"##), "{html}");
}

#[test]
fn the_detail_panel_of_an_ended_issue_says_how_long_it_took() {
    let mut merged = issue(1, NodeState::Merged, 0, 0);
    merged.worked = Some("3m 05s · $0.92".into());
    let html = render(|| view! { <Detail issue=merged/> });
    assert!(html.contains(r#"<span class="state merged">"#), "{html}");
    assert!(
        html.contains(r#"<dl><dt>Took</dt><dd>3m 05s · $0.92</dd>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<ol class="recent"><li class="muted">Nothing yet.</li></ol>"#),
        "{html}"
    );
}

#[test]
fn the_detail_panel_of_an_issue_not_started_has_no_time() {
    let html = render(|| view! { <Detail issue=issue(3, NodeState::NotStarted, 1, 1)/> });
    assert!(
        html.contains(r#"<span class="state not_started">"#),
        "{html}"
    );
    assert!(html.contains(r#"<dl><dt>Depends on</dt>"#), "{html}");
}

#[test]
fn a_runs_page_has_its_head_builds_graph_and_detail() {
    let mut page = run_page();
    page.builds.push("Last build, failed at 9:40".into());
    let html = render(|| view! { <RunView page chosen=Some(2)/> });
    assert!(html.starts_with(r#"<section class="run-head"><div><div class="about"><a href="https://github.com/acme/widgets/issues/13">acme/widgets#13</a> · <span>3 issues · merging into <code>epic/13-exponentiation</code></span></div><h1>Exponentiation</h1><p class="sentence building">First build running. 1 of 3 issues merged, 1 agent working.</p></div><dl class="totals"><dt>Elapsed</dt><dd>4m 12s</dd><dt>Cost so far</dt><dd>$1.20</dd></dl></section>"#), "{html}");
    assert!(html.contains(r#"<div class="builds"><span class="label">Builds</span><span class="build current">First build, started 10:02</span><span class="build">Last build, failed at 9:40</span></div>"#), "{html}");
    assert!(html.contains(r#"<div class="graph-scroll"><div class="graph" style="width:456px;height:170px;"><svg width="456" height="170"><path d="#), "{html}");
    assert_eq!(html.matches(r#"<button title="#).count(), 3, "{html}");
    assert!(html.contains(r#"class="issue running chosen""#), "{html}");
    assert!(html.ends_with(r#"</ol></div></section>"#), "{html}");
    assert!(html.contains(r#"<section class="detail"><div class="facts"><h3><a href="https://github.com/acme/widgets/issues/2">#2</a>"#), "{html}");
}

#[test]
fn a_runs_page_without_a_chosen_issue_has_no_detail() {
    let html = render(|| view! { <RunView page=run_page() chosen=None/> });
    assert!(!html.contains("detail"), "{html}");
    assert!(!html.contains("chosen"), "{html}");
}

#[test]
fn a_run_needing_attention_says_so_in_red() {
    let mut page = run_page();
    page.disposition = epik_view::Disposition::NeedsAttention;
    page.sentence = "The build failed.".into();
    let html = render(|| view! { <RunView page chosen=None/> });
    assert!(
        html.contains(r#"<p class="sentence needs_attention">The build failed.</p>"#),
        "{html}"
    );
}
