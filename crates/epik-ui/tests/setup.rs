use std::collections::BTreeMap;

use epik_ui::setup::{Intro, Item, Items, Setup};
use epik_view::{Checklist, ItemState, Output, Row};
use leptos::prelude::*;

mod fixtures;
use fixtures::render;

fn row(id: &str, state: ItemState, button: Option<&str>) -> Row {
    Row {
        id: id.into(),
        title: "GitHub CLI".into(),
        state,
        detail: "gh 2.80.0".into(),
        button: button.map(Into::into),
        needed: true,
    }
}

fn output(lines: &[&str], running: bool, error: Option<&str>) -> Output {
    Output {
        lines: lines.iter().map(|&line| line.into()).collect(),
        running,
        error: error.map(Into::into),
    }
}

fn checklist(rows: Vec<Row>, complete: bool) -> Checklist {
    Checklist { rows, complete }
}

#[test]
fn a_done_row_has_its_mark_title_and_detail_and_no_button() {
    let html = render(|| view! { <Item row=row("gh", ItemState::Done, None) busy=false/> });
    assert_eq!(
        html,
        r#"<li data-id="gh" class="item done"><span class="mark">✓</span><span class="title">GitHub CLI</span><span class="detail">gh 2.80.0</span></li>"#
    );
}

#[test]
fn a_missing_row_has_its_button() {
    let html = render(
        || view! { <Item row=row("gh", ItemState::Missing, Some("Install a private copy")) busy=false/> },
    );
    assert_eq!(
        html,
        r#"<li data-id="gh" class="item missing"><span class="mark">✕</span><span class="title">GitHub CLI</span><button class="primary">Install a private copy</button><span class="detail">gh 2.80.0</span></li>"#
    );
}

#[test]
fn a_row_not_needed_says_so_and_keeps_its_button() {
    let codex = Row {
        id: "codex-auth".into(),
        title: "Codex sign-in".into(),
        state: ItemState::Missing,
        detail: "not signed in".into(),
        button: Some("Sign in to Codex (opens the browser)".into()),
        needed: false,
    };

    let html = render(move || view! { <Item row=codex busy=false/> });

    assert_eq!(
        html,
        r#"<li data-id="codex-auth" class="item unneeded"><span class="mark">–</span><span class="title">Codex sign-in</span><button class="primary">Sign in to Codex (opens the browser)</button><span class="detail">Not needed by your agents</span></li>"#
    );
}

#[test]
fn a_done_row_not_needed_is_shown_done() {
    let gh = Row {
        needed: false,
        ..row("gh", ItemState::Done, None)
    };

    let html = render(move || view! { <Item row=gh busy=false/> });

    assert!(
        html.starts_with(r#"<li data-id="gh" class="item done"><span class="mark">✓</span>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="detail">gh 2.80.0</span>"#),
        "{html}"
    );
}

#[test]
fn an_outdated_row_has_its_mark() {
    let html = render(
        || view! { <Item row=row("node", ItemState::Outdated, Some("Update the private copy")) busy=false/> },
    );
    assert!(
        html.starts_with(r#"<li data-id="node" class="item outdated"><span class="mark">!</span>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<button class="primary">Update the private copy</button>"#),
        "{html}"
    );
}

#[test]
fn a_row_shows_its_kept_output_and_failure_after_its_detail() {
    let kept = output(
        &["Downloading gh", "Unpacking"],
        false,
        Some("gh exited with 1"),
    );
    let html = render(|| {
        view! {
            <Item row=row("gh", ItemState::Missing, Some("Install")) busy=false output=Some(kept)/>
        }
    });
    assert!(
        html.ends_with(
            "<span class=\"detail\">gh 2.80.0</span><pre class=\"log\">Downloading gh\nUnpacking</pre><span class=\"failed\">gh exited with 1</span></li>"
        ),
        "{html}"
    );
}

#[test]
fn a_row_whose_step_succeeded_keeps_its_output_without_a_failure() {
    let kept = output(&["Signed in"], false, None);
    let html = render(
        || view! { <Item row=row("gh-auth", ItemState::Done, None) busy=false output=Some(kept)/> },
    );
    assert!(
        html.contains(r#"<pre class="log">Signed in</pre>"#),
        "{html}"
    );
    assert!(!html.contains("failed"), "{html}");
}

#[test]
fn a_busy_rows_button_is_disabled() {
    let html =
        render(|| view! { <Item row=row("gh", ItemState::Missing, Some("Install")) busy=true/> });
    assert!(
        html.contains(r#"<button disabled class="primary">Install</button>"#),
        "{html}"
    );
}

#[test]
fn while_a_step_runs_every_button_is_disabled() {
    let mut setup = Setup::default();
    setup.fetched(Ok(checklist(
        vec![
            row("gh", ItemState::Missing, Some("Install")),
            row("node", ItemState::Outdated, Some("Update")),
        ],
        false,
    )));
    setup.started("gh");
    let html = render(|| view! { <Items setup/> });
    assert_eq!(html.matches("<button").count(), 2, "{html}");
    assert_eq!(html.matches("<button disabled").count(), 2, "{html}");
    // The running step's log, empty until it prints.
    assert!(html.contains(r#"<pre class="log"> </pre>"#), "{html}");
}

#[test]
fn the_complete_message_shows_when_the_checklist_is_complete() {
    let mut setup = Setup::default();
    setup.fetched(Ok(checklist(vec![row("gh", ItemState::Done, None)], true)));
    let html = render(|| view! { <Items setup/> });
    assert!(
        html.contains("<section id=\"complete\"><h2>Epik is set up.</h2><p>Start Claude Code and say “Hello, Epik.”</p></section>"),
        "{html}"
    );
}

#[test]
fn the_complete_message_is_hidden_until_then() {
    let html = render(|| view! { <Items setup=Setup::default()/> });
    assert!(html.contains(r#"<section id="complete" hidden>"#), "{html}");
    let mut setup = Setup::default();
    setup.fetched(Ok(checklist(
        vec![row("gh", ItemState::Missing, None)],
        false,
    )));
    let html = render(|| view! { <Items setup/> });
    assert!(html.contains(r#"<section id="complete" hidden>"#), "{html}");
}

#[test]
fn a_failed_fetch_is_shown_in_the_error_line() {
    let mut setup = Setup::default();
    setup.fetched(Err("gh: command not found".into()));
    let html = render(|| view! { <Items setup/> });
    assert!(
        html.ends_with(r#"<p id="error">Couldn't check setup: gh: command not found</p>"#),
        "{html}"
    );
}

#[test]
fn a_failed_fetch_keeps_the_checklist_and_the_next_one_clears_the_error() {
    let list = checklist(vec![row("gh", ItemState::Done, None)], false);
    let mut setup = Setup::default();
    setup.fetched(Ok(list.clone()));
    setup.fetched(Err("boom".into()));
    assert_eq!(setup.checklist, Some(list.clone()));
    setup.fetched(Ok(list));
    assert_eq!(setup.error, "");
}

#[test]
fn a_refused_step_is_shown_in_the_error_line_and_starts_nothing() {
    let mut setup = Setup::default();
    setup.refused("GitHub CLI is already being set up".into());
    assert_eq!(setup.error, "GitHub CLI is already being set up");
    assert!(!setup.running());
}

#[test]
fn starting_a_step_clears_its_previous_output_and_failure() {
    let mut setup = Setup::default();
    setup.followed("gh", output(&["old"], false, Some("failed")));
    setup.started("gh");
    assert_eq!(setup.outputs["gh"], output(&[], true, None));
    assert!(setup.running());
}

#[test]
fn a_step_is_followed_until_it_stops_running_and_its_output_is_kept() {
    let mut setup = Setup::default();
    setup.started("gh");
    assert!(setup.followed("gh", output(&["one"], true, None)));
    assert!(setup.running());
    assert!(!setup.followed("gh", output(&["one", "two"], false, Some("bad"))));
    assert!(!setup.running());
    setup.fetched(Ok(checklist(
        vec![row("gh", ItemState::Missing, Some("Install"))],
        false,
    )));
    assert_eq!(
        setup.outputs,
        BTreeMap::from([("gh".to_owned(), output(&["one", "two"], false, Some("bad")))])
    );
}

#[test]
fn the_intro_names_no_operating_system() {
    let html = render(|| view! { <Intro/> });
    assert!(
        html.starts_with("<p>Epik needs these on this computer. Each button says"),
        "{html}"
    );
}
