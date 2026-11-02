use epik_ui::home::{Filter, HomePage};
use epik_view::{Disposition, Home, RunCard};
use leptos::prelude::*;

mod fixtures;
use fixtures::{card, ended, home, link, now, render, text};

fn building(run: &str) -> RunCard {
    RunCard {
        run: run.into(),
        ..card(vec![])
    }
}

/// Two runs building, one waiting for review, one needing attention.
fn runs() -> Home {
    home(
        vec![building("acme/widgets#10"), building("acme/widgets#11")],
        vec![ended("acme/widgets#20", Disposition::WaitingForReview)],
        vec![ended("acme/widgets#30", Disposition::NeedsAttention)],
    )
}

/// The runs `html` has rows for, in order, as their pages' paths.
fn rows(html: &str) -> Vec<String> {
    html.split(r#"<tbody class="run "#)
        .skip(1)
        .map(|row| {
            let at = row.find("#/run/").unwrap() + "#/run/".len();
            let end = at + row[at..].find('"').unwrap();
            row[at..end].to_owned()
        })
        .collect()
}

/// The row of `run`, from its `<tbody>` to its end.
fn row<'a>(html: &'a str, class: &str) -> &'a str {
    let start = html
        .find(&format!(r#"<tbody class="run {class}">"#))
        .unwrap();
    let after = &html[start..];
    &after[..after.find("</tbody>").unwrap() + "</tbody>".len()]
}

#[test]
fn home_has_a_title_and_the_headline() {
    let html = render(|| view! { <HomePage home=runs()/> });
    assert!(
        html.starts_with(
            r#"<div class="page-head"><h1>Runs</h1><p class="headline">One run building</p></div>"#
        ),
        "{html}"
    );
}

#[test]
fn the_tabs_count_the_runs_and_all_is_chosen() {
    let html = render(|| view! { <HomePage home=runs()/> });
    assert!(html.contains(r#"<button role="tab" aria-selected="true" class="tab">All<span class="count">4</span></button>"#), "{html}");
    assert!(html.contains(r#"<button role="tab" aria-selected="false" class="tab">Building<span class="count">2</span></button>"#), "{html}");
    assert!(html.contains(r#"<button role="tab" aria-selected="false" class="tab">Waiting for review<span class="count">1</span></button>"#), "{html}");
    assert!(html.contains(r#"<button role="tab" aria-selected="false" class="tab">Needs attention<span class="count">1</span></button>"#), "{html}");
}

#[test]
fn the_table_has_a_row_for_each_run_attention_first_then_building_then_waiting() {
    let html = render(|| view! { <HomePage home=runs()/> });
    assert!(html.contains(r#"<thead><tr><th></th><th>Run</th><th>Progress</th><th>Status</th><th class="figure">Time</th><th class="figure">Cost</th><th></th></tr></thead>"#), "{html}");
    assert_eq!(
        rows(&html),
        [
            "acme/widgets%2330",
            "acme/widgets%2310",
            "acme/widgets%2311",
            "acme/widgets%2320"
        ],
        "{html}"
    );
}

#[test]
fn a_row_has_the_run_its_progress_status_time_and_cost() {
    let html = render(|| {
        view! { <HomePage home=home(vec![], vec![ended("acme/widgets#13", Disposition::WaitingForReview)], vec![])/> }
    });
    assert!(html.contains(r##"<tbody class="run waiting_for_review"><tr><td class="fold-cell"></td><td class="run-cell"><a href="#/run/acme/widgets%2313" class="title">Exponentiation</a><div class="repo"><a href="https://github.com/acme/widgets/issues/13">acme/widgets#13</a></div></td><td class="progress-cell"><svg viewBox="0 0 32 24""##), "{html}");
    assert!(html.contains(r#"<div class="progress">"#), "{html}");
    assert!(html.contains(r#"<td class="status"><span class="dot waiting_for_review"></span><span>Merged into <a href="https://github.com/acme/widgets/pull/30">PR #30</a></span></td><td class="figure">4m 12s</td><td class="figure">$1.20</td>"#), "{html}");
}

#[test]
fn a_building_row_says_where_it_stands() {
    let html = render(|| view! { <HomePage home=home(vec![card(vec![])], vec![], vec![])/> });
    assert!(html.contains(r#"<td class="status"><span class="dot building"></span><span>1 of 3 merged, <a href="https://github.com/acme/widgets/issues/2">#2</a> running</span></td>"#), "{html}");
}

#[test]
fn a_run_needing_attention_has_rerun_and_delete_and_the_others_open() {
    let html = render(|| view! { <HomePage home=runs()/> });
    assert!(row(&html, "needs_attention").ends_with(r#"<td class="actions"><button class="primary">Rerun</button><button>Delete</button></td></tr></tbody>"#), "{html}");
    assert!(row(&html, "waiting_for_review").ends_with(r##"<td class="actions"><a href="#/run/acme/widgets%2320" class="open">Open ›</a></td></tr></tbody>"##), "{html}");
    assert!(row(&html, "building").contains(r##"<td class="actions"><a href="#/run/acme/widgets%2310" class="open">Open ›</a></td></tr>"##), "{html}");
    assert_eq!(html.matches("Rerun").count(), 1, "{html}");
    assert_eq!(html.matches("Open ›").count(), 3, "{html}");
}

#[test]
fn each_filter_shows_only_its_runs() {
    let shown = |filter: Filter| rows(&render(move || view! { <HomePage home=runs() filter/> }));
    assert_eq!(
        shown(Filter::Building),
        ["acme/widgets%2310", "acme/widgets%2311"]
    );
    assert_eq!(shown(Filter::Waiting), ["acme/widgets%2320"]);
    assert_eq!(shown(Filter::Attention), ["acme/widgets%2330"]);
    let html = render(|| view! { <HomePage home=runs() filter=Filter::Waiting/> });
    assert!(html.contains(r#"<button role="tab" aria-selected="false" class="tab">All<span class="count">4</span></button>"#), "{html}");
    assert!(html.contains(r#"<button role="tab" aria-selected="true" class="tab">Waiting for review<span class="count">1</span></button>"#), "{html}");
}

#[test]
fn a_building_row_folds_and_the_others_dont() {
    let html = render(|| view! { <HomePage home=runs()/> });
    assert!(row(&html, "building").starts_with(r#"<tbody class="run building"><tr><td class="fold-cell"><button aria-label="Unfold" aria-expanded="false" class="fold">▸</button></td>"#), "{html}");
    assert!(
        row(&html, "needs_attention")
            .starts_with(r#"<tbody class="run needs_attention"><tr><td class="fold-cell"></td>"#),
        "{html}"
    );
    assert!(!html.contains(r#"class="lines""#), "{html}");
}

#[test]
fn an_unfolded_building_row_shows_its_now_next_and_done_lines() {
    let mut run = card(vec![
        now(2, Some("active 2s ago"), Some(1)),
        now(3, Some("quiet 1m 50s"), Some(1)),
    ]);
    run.next = Some(vec![link("#5", "https://github.com/acme/widgets/issues/5")]);
    run.done = Some(vec![text("#1")]);
    let html = render(|| {
        view! { <HomePage home=home(vec![run], vec![], vec![]) unfolded=vec!["acme/widgets#13".into()]/> }
    });
    assert!(
        html.contains(r#"<button aria-label="Fold" aria-expanded="true" class="fold">▾</button>"#),
        "{html}"
    );
    assert!(html.contains(r#"</tr><tr class="lines"><td></td><td colspan="6"><div class="line"><span class="label">Now</span><span>#2</span><span class="life">active 2s ago</span></div>"#), "{html}");
    assert!(html.contains(r#"<div class="line"><span class="label">Now</span><span>#3</span><span class="life quiet">quiet 1m 50s</span></div>"#), "{html}");
    assert!(html.contains(r#"<div class="line"><span class="label">Next</span><span><a href="https://github.com/acme/widgets/issues/5">#5</a></span></div>"#), "{html}");
    assert!(html.contains(r#"<div class="line"><span class="label">Done</span><span>#1</span></div></td></tr></tbody>"#), "{html}");
}

#[test]
fn home_without_runs_says_so() {
    let html = render(|| view! { <HomePage home=home(vec![], vec![], vec![])/> });
    assert!(html.contains(r#"<div class="empty"><p>No runs right now.</p><p class="muted">Ask Epik to build an issue or an epic in Claude Code, and it shows up here while it builds.</p></div>"#), "{html}");
    assert!(!html.contains("<table"), "{html}");
}

#[test]
fn a_hosts_run_says_where_it_builds_under_its_repository() {
    let mut remote = building("devbox:acme/widgets#10");
    remote.place = Some(
        "Building on devbox, last heard from 12m 00s ago. The build may still be going on there."
            .into(),
    );
    let html = render(|| view! { <HomePage home=home(vec![remote], vec![], vec![])/> });
    assert_eq!(rows(&html), ["devbox:acme/widgets%2310"], "{html}");
    assert!(
        html.contains(r#"</div><div class="place">Building on devbox, last heard from 12m 00s ago. The build may still be going on there.</div></td>"#),
        "{html}"
    );
}

#[test]
fn a_run_with_nowhere_to_say_says_nothing_of_it() {
    let html = render(|| view! { <HomePage home=runs()/> });
    assert!(!html.contains(r#"class="place""#), "{html}");
}
