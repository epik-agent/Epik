use epik_ui::flash::Flashes;
use epik_ui::popover::{Card, Runs, latest};
use epik_view::Disposition;
use leptos::prelude::*;

mod fixtures;
use fixtures::render;
use fixtures::{card, link, now, popover, text};

#[test]
fn a_card_has_its_graph_title_time_and_summary() {
    let html = render(|| view! { <Card run=card(vec![]) flash=false/> });
    assert!(html.starts_with(r#"<article class="card">"#), "{html}");
    assert!(html.contains(r#"<div class="head"><svg viewBox="0 0 32 24" width="32" height="24" class="mini-graph""#), "{html}");
    assert!(
        html.contains(r##"<a href="#" class="title">Exponentiation</a>"##),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="time">4m 12s</span>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="summary">1 of 3 merged, "#),
        "{html}"
    );
}

#[test]
fn a_card_without_a_life_line_has_none() {
    let html = render(|| view! { <Card run=card(vec![now(2, None, None)]) flash=false/> });
    assert!(!html.contains("life"), "{html}");
}

#[test]
fn a_card_has_its_latest_agents_life_line() {
    let run = card(vec![
        now(2, Some("active 9s ago"), Some(100)),
        now(3, Some("active 2s ago"), Some(200)),
    ]);
    let html = render(|| view! { <Card run flash=false/> });
    assert!(
        html.contains(r#"<div class="life">active 2s ago</div>"#),
        "{html}"
    );
    assert!(!html.contains("active 9s ago"), "{html}");
}

#[test]
fn a_quiet_life_line_is_quiet() {
    let html = render(
        || view! { <Card run=card(vec![now(2, Some("quiet 1m 50s"), Some(100))]) flash=false/> },
    );
    assert!(
        html.contains(r#"<div class="life quiet">quiet 1m 50s</div>"#),
        "{html}"
    );
}

#[test]
fn a_flashing_card_has_the_flash_class() {
    let html = render(|| view! { <Card run=card(vec![]) flash=true/> });
    assert!(
        html.starts_with(r#"<article class="card flash">"#),
        "{html}"
    );
}

#[test]
fn a_card_has_a_progress_bar_when_it_has_more_than_one_issue() {
    let html = render(|| view! { <Card run=card(vec![]) flash=false/> });
    assert!(html.contains(r#"<div class="progress">"#), "{html}");
    let mut one = card(vec![]);
    one.graph.layout.places.truncate(1);
    let html = render(|| view! { <Card run=one flash=false/> });
    assert!(!html.contains("progress"), "{html}");
}

#[test]
fn the_runs_are_a_card_for_each_building_run() {
    let html =
        render(|| view! { <Runs popover=popover(vec![card(vec![]), card(vec![])], vec![])/> });
    assert_eq!(
        html.matches(r#"<article class="card">"#).count(),
        2,
        "{html}"
    );
    assert!(!html.contains("Waiting for you"), "{html}");
    assert!(!html.contains("Nothing is building."), "{html}");
}

#[test]
fn the_runs_list_those_waiting_for_the_user() {
    let waiting = vec![
        (
            Disposition::WaitingForReview,
            "acme/widgets#13".into(),
            vec![
                link("Exponentiation", "https://github.com/acme/widgets/pull/30"),
                text(" is ready for review"),
            ],
        ),
        (
            Disposition::NeedsAttention,
            "acme/widgets#40".into(),
            vec![text("Logging failed")],
        ),
    ];
    let html = render(|| view! { <Runs popover=popover(vec![], waiting)/> });
    assert!(html.contains("<h2>Waiting for you</h2>"), "{html}");
    assert!(html.contains(r#"<ul class="waiting"><li class="waiting_for_review"><span class="marker"></span><span><a href="https://github.com/acme/widgets/pull/30">Exponentiation</a>"#), "{html}");
    assert!(html.contains(r#"<li class="needs_attention"><span class="marker"></span><span>Logging failed</span></li></ul>"#), "{html}");
    assert!(!html.contains("Nothing is building."), "{html}");
}

#[test]
fn the_runs_say_nothing_is_building_when_there_is_nothing_to_show() {
    let html = render(|| view! { <Runs popover=popover(vec![], vec![])/> });
    assert_eq!(html, r#"<p class="quiet-note">Nothing is building.</p>"#);
}

#[test]
fn the_latest_agent_is_the_first_with_the_greatest_last_event() {
    let lines = [
        now(1, Some("a"), Some(5)),
        now(2, Some("b"), Some(9)),
        now(3, Some("c"), Some(9)),
    ];
    assert_eq!(latest(&lines).map(|line| line.number), Some(2));
}

#[test]
fn there_is_no_latest_agent_until_one_has_logged() {
    let lines = [now(1, Some("a"), None), now(2, Some("b"), Some(0))];
    assert_eq!(latest(&lines), None);
    assert_eq!(latest(&[]), None);
}

#[test]
fn a_run_flashes_when_its_last_event_changes() {
    let mut flashes = Flashes::default();
    assert!(
        !flashes.changed("acme/widgets#13", Some(1)),
        "the first sighting"
    );
    assert!(!flashes.changed("acme/widgets#13", Some(1)), "no change");
    assert!(flashes.changed("acme/widgets#13", Some(2)));
    assert!(!flashes.changed("acme/widgets#13", None), "no event");
    assert!(flashes.changed("acme/widgets#13", Some(3)));
    assert!(!flashes.changed("acme/widgets#40", Some(3)), "another run");
}
