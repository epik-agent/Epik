use epik_ui::main_window::{HomeBar, RunBar};
use leptos::prelude::*;

mod fixtures;
use fixtures::render;

#[test]
fn the_bar_on_home_has_the_mark_the_name_the_headline_and_the_settings_gear() {
    let html =
        render(|| view! { <HomeBar headline="One run building".into() missing_repository=false/> });
    assert!(
        html.starts_with(r#"<span class="mark"><svg viewBox="0 0 64 64" width="20" height="20""#),
        "{html}"
    );
    assert!(
        html.contains(
            r#"<span class="name">Epik</span><span class="headline">One run building</span>"#
        ),
        "{html}"
    );
    assert!(
        html.ends_with(r#"<button title="Settings" aria-label="Settings" class="gear">⚙</button>"#),
        "{html}"
    );
}

#[test]
fn the_settings_gear_has_a_badge_when_a_repository_is_missing() {
    let html =
        render(|| view! { <HomeBar headline="Nothing building".into() missing_repository=true/> });
    assert!(
        html.ends_with(r#"class="gear">⚙<span class="badge">✕</span></button>"#),
        "{html}"
    );
}

#[test]
fn the_bar_on_a_runs_page_names_the_run_and_links_back_home() {
    let html = render(|| view! { <RunBar title="Exponentiation".into() also_building=vec![]/> });
    assert!(html.starts_with(r#"<span class="mark"><svg"#), "{html}");
    assert!(html.ends_with(r##"<span class="crumbs"><a class="back" href="#/">‹ Runs</a><span class="slash">/</span><span>Exponentiation</span></span>"##), "{html}");
    assert!(!html.contains("gear"), "{html}");
}

#[test]
fn the_bar_on_a_runs_page_links_to_the_other_runs_building() {
    let also = vec![
        ("acme/widgets#40".to_string(), "Logging".to_string()),
        ("acme/tools#2".to_string(), "Lint".to_string()),
    ];
    let html = render(|| view! { <RunBar title="Exponentiation".into() also_building=also/> });
    assert!(html.ends_with(r##"<span class="also">Also building: <a href="#/run/acme/widgets%2340">Logging</a>, <a href="#/run/acme/tools%232">Lint</a></span>"##), "{html}");
}
