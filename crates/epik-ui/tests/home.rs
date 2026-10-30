use epik_ui::home::{BuildingCard, EndedCard, HomePage};
use epik_view::{Disposition, RunCard};
use leptos::prelude::*;

mod fixtures;
use fixtures::{card, ended, home, link, now, render, text};

#[test]
fn a_building_card_has_its_graph_title_epic_figures_and_progress() {
    let html = render(|| view! { <BuildingCard card=card(vec![])/> });
    assert!(
        html.starts_with(
            r#"<article class="card building"><div class="card-head"><svg viewBox="0 0 32 24""#
        ),
        "{html}"
    );
    assert!(html.contains(r##"<div class="titles"><a href="#/run/acme/widgets%2313" class="title">Exponentiation</a><div class="sub"><a href="https://github.com/acme/widgets/issues/13">acme/widgets#13</a> · 3 issues, 1 merged</div></div>"##), "{html}");
    assert!(html.contains(r##"<div class="figures"><span class="time">4m 12s</span><span class="cost">$1.20</span><a href="#/run/acme/widgets%2313" class="open">Open ›</a></div></div>"##), "{html}");
    assert!(html.contains(r#"<div class="progress">"#), "{html}");
    assert!(
        html.ends_with(r#"<div class="rows"></div></article>"#),
        "{html}"
    );
}

#[test]
fn a_building_card_has_its_now_next_and_done_rows() {
    let mut run = card(vec![
        now(2, Some("active 2s ago"), Some(1)),
        now(3, Some("quiet 1m 50s"), Some(1)),
        now(4, None, None),
    ]);
    run.next = Some(vec![link("#5", "https://github.com/acme/widgets/issues/5")]);
    run.done = Some(vec![text("#1")]);
    let html = render(|| view! { <BuildingCard card=run/> });
    assert!(html.contains(r#"<div class="rows"><div class="line"><span class="label">Now</span><span>#2</span><span class="life">active 2s ago</span></div>"#), "{html}");
    assert!(html.contains(r#"<div class="line"><span class="label">Now</span><span>#3</span><span class="life quiet">quiet 1m 50s</span></div>"#), "{html}");
    assert!(html.contains(r#"<div class="line"><span class="label">Now</span><span>#4</span><span class="life"></span></div>"#), "{html}");
    assert!(html.contains(r#"<div class="line"><span class="label">Next</span><span><a href="https://github.com/acme/widgets/issues/5">#5</a></span></div>"#), "{html}");
    assert!(html.ends_with(r#"<div class="line"><span class="label">Done</span><span>#1</span></div></div></article>"#), "{html}");
}

#[test]
fn a_card_waiting_for_review_says_where_it_stands() {
    let html = render(
        || view! { <EndedCard card=ended("acme/widgets#13", Disposition::WaitingForReview) attention=false/> },
    );
    assert!(
        html.starts_with(r#"<article class="card waiting"><div class="card-head"><svg"#),
        "{html}"
    );
    assert!(html.contains(r##"<div class="titles"><a href="#/run/acme/widgets%2313" class="title">Exponentiation</a><div class="sub said good"><a href="https://github.com/acme/widgets/issues/13">acme/widgets#13</a> · <span>Merged into <a href="https://github.com/acme/widgets/pull/30">PR #30</a></span></div></div>"##), "{html}");
    assert!(html.ends_with(r#"<div class="figures"><span class="time">4m 12s</span><span class="cost">$1.20</span></div></div></article>"#), "{html}");
    assert!(!html.contains("progress"), "{html}");
    assert!(!html.contains("button"), "{html}");
}

#[test]
fn a_card_needing_attention_has_rerun_and_delete_buttons() {
    let mut run = ended("acme/widgets#13", Disposition::NeedsAttention);
    run.note = Some("Interrupted at 10:14".into());
    let html = render(|| view! { <EndedCard card=run attention=true/> });
    assert!(
        html.starts_with(r#"<article class="card attention">"#),
        "{html}"
    );
    assert!(html.contains(r#"<div class="sub said bad">"#), "{html}");
    assert!(
        html.contains(r#"</div><div class="note">Interrupted at 10:14</div></div>"#),
        "{html}"
    );
    assert!(html.ends_with(r#"<span class="cost">$1.20</span><button class="primary">Rerun</button><button>Delete</button></div></div></article>"#), "{html}");
}

#[test]
fn home_has_a_section_for_each_kind_of_run() {
    let html = render(|| {
        view! {
            <HomePage home=home(
                vec![card(vec![])],
                vec![ended("acme/widgets#20", Disposition::WaitingForReview)],
                vec![ended("acme/widgets#30", Disposition::NeedsAttention)],
            )/>
        }
    });
    let building = html.find(r#"<h2 class="section">Building</h2><article class="card building">"#);
    let waiting = html
        .find(r#"<h2 class="section">Waiting for your review</h2><article class="card waiting">"#);
    let attention = html
        .find(r#"<h2 class="section bad">Needs attention</h2><article class="card attention">"#);
    assert!(
        building.is_some() && waiting.is_some() && attention.is_some(),
        "{html}"
    );
    assert!(building < waiting && waiting < attention, "{html}");
    assert!(!html.contains("No runs right now."), "{html}");
}

#[test]
fn home_leaves_out_an_empty_section() {
    let other = RunCard {
        run: "acme/widgets#40".into(),
        ..card(vec![])
    };
    let html =
        render(|| view! { <HomePage home=home(vec![card(vec![]), other], vec![], vec![])/> });
    assert_eq!(
        html.matches(r#"<article class="card building">"#).count(),
        2,
        "{html}"
    );
    assert!(!html.contains("Waiting for your review"), "{html}");
    assert!(!html.contains("Needs attention"), "{html}");
}

#[test]
fn home_without_runs_says_so() {
    let html = render(|| view! { <HomePage home=home(vec![], vec![], vec![])/> });
    assert_eq!(
        html,
        r#"<div class="empty"><p>No runs right now.</p><p class="muted">Ask Epik to build an issue or an epic in Claude Code, and it shows up here while it builds.</p></div>"#
    );
}
