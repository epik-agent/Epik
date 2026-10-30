use epik_ui::main_window::{Page, RunBar, Sidebar};
use epik_view::Disposition;
use leptos::prelude::*;

mod fixtures;
use fixtures::{render, sidebar, sidebar_run};

fn runs() -> epik_view::Sidebar {
    sidebar(vec![
        sidebar_run(
            "acme/widgets#7",
            "Division by zero",
            Disposition::NeedsAttention,
        ),
        sidebar_run("acme/widgets#13", "Exponentiation", Disposition::Building),
        sidebar_run("acme/tools#2", "Lint", Disposition::WaitingForReview),
    ])
}

#[test]
fn the_sidebar_has_the_mark_and_name_runs_settings_and_the_status_line() {
    let html = render(|| view! { <Sidebar sidebar=runs() page=Page::Home/> });
    assert!(
        html.starts_with(r#"<div class="brand"><span class="mark"><svg viewBox="0 0 64 64""#),
        "{html}"
    );
    assert!(
        html.contains(r#"</span><span class="name">Epik</span></div>"#),
        "{html}"
    );
    assert!(
        html.contains(r##"<a href="#/" class="label">Runs</a><span class="count">3</span>"##),
        "{html}"
    );
    assert!(
        html.ends_with(r##"<div class="foot"><a href="#/settings" class="item settings">Settings</a><div class="status"><span class="dot"></span>Epik running · up 4h 12m</div></div>"##),
        "{html}"
    );
}

#[test]
fn the_sidebar_lists_each_run_with_a_dot_for_its_disposition() {
    let html = render(|| view! { <Sidebar sidebar=runs() page=Page::Home/> });
    assert!(html.contains(r##"<ul class="run-list"><li><a href="#/run/acme/widgets%237" class="item run"><span class="dot needs_attention"></span><span class="title">Division by zero</span></a></li><li><a href="#/run/acme/widgets%2313" class="item run"><span class="dot building"></span><span class="title">Exponentiation</span></a></li><li><a href="#/run/acme/tools%232" class="item run"><span class="dot waiting_for_review"></span><span class="title">Lint</span></a></li></ul>"##), "{html}");
}

#[test]
fn settings_has_a_badge_while_a_repository_is_missing() {
    let mut missing = runs();
    missing.missing_repository = true;
    let html = render(|| view! { <Sidebar sidebar=missing page=Page::Home/> });
    assert!(
        html.contains(
            r##"<a href="#/settings" class="item settings">Settings<span class="badge">✕</span></a>"##
        ),
        "{html}"
    );
}

#[test]
fn the_list_of_runs_folds() {
    let open = render(|| view! { <Sidebar sidebar=runs() page=Page::Home/> });
    assert!(
        open.contains(
            r#"<button aria-label="Fold the runs" aria-expanded="true" class="fold">▾</button>"#
        ),
        "{open}"
    );
    let folded = render(|| view! { <Sidebar sidebar=runs() page=Page::Home folded=true/> });
    assert!(
        folded.contains(
            r#"<button aria-label="Unfold the runs" aria-expanded="false" class="fold">▸</button>"#
        ),
        "{folded}"
    );
    assert!(!folded.contains("run-list"), "{folded}");
}

#[test]
fn the_current_page_is_highlighted() {
    let home = render(|| view! { <Sidebar sidebar=runs() page=Page::Home/> });
    assert!(
        home.contains(r#"<div class="item runs current">"#),
        "{home}"
    );
    assert!(!home.contains("run current"), "{home}");

    let run =
        render(|| view! { <Sidebar sidebar=runs() page=Page::Run("acme/widgets#13".into())/> });
    assert!(run.contains(r#"<div class="item runs">"#), "{run}");
    assert!(
        run.contains(r##"<a href="#/run/acme/widgets%2313" class="item run current">"##),
        "{run}"
    );
    assert_eq!(run.matches("current").count(), 1, "{run}");

    let settings = render(|| view! { <Sidebar sidebar=runs() page=Page::Settings/> });
    assert!(
        settings.contains(r#"<div class="item runs">"#),
        "{settings}"
    );
    assert!(
        settings.contains(r##"<a href="#/settings" class="item settings current">"##),
        "{settings}"
    );
    assert_eq!(settings.matches("current").count(), 1, "{settings}");
}

#[test]
fn the_breadcrumb_on_a_runs_page_names_the_run_and_links_back_home() {
    let html = render(|| view! { <RunBar title="Exponentiation" also_building=vec![]/> });
    assert_eq!(
        html,
        r##"<span class="crumbs"><a class="back" href="#/">‹ Runs</a><span class="slash">/</span><span>Exponentiation</span></span>"##
    );
}

#[test]
fn the_breadcrumb_on_a_runs_page_links_to_the_other_runs_building() {
    let also = vec![
        ("acme/widgets#40".to_string(), "Logging".to_string()),
        ("acme/tools#2".to_string(), "Lint".to_string()),
    ];
    let html = render(|| view! { <RunBar title="Exponentiation" also_building=also/> });
    assert!(html.ends_with(r##"<span class="also">Also building: <a href="#/run/acme/widgets%2340">Logging</a>, <a href="#/run/acme/tools%232">Lint</a></span>"##), "{html}");
}
