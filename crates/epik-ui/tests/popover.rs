use epik_ui::popover::{Command, Header, Item, Items, RunRow, Runs, Segments, shortcut};
use epik_view::NodeState;
use leptos::prelude::*;

mod fixtures;
use fixtures::render;
use fixtures::{popover, tray_run};

#[test]
fn the_header_says_epik_is_running_and_what_it_is_doing() {
    let html = render(|| view! { <Header about=popover(vec![]).about/> });
    assert_eq!(
        html,
        r#"<header><div class="running"><span class="dot"></span><span class="name">Epik is running</span></div><div class="about">3 runs · 4 agents · up 4h 12m</div></header>"#
    );
}

#[test]
fn a_run_row_has_its_title_count_segments_and_line() {
    let html = render(|| view! { <RunRow run=tray_run()/> });
    assert!(
        html.starts_with(r#"<button type="button" class="run">"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<div class="head"><span class="title">Exponentiation</span><span class="count">1/3</span></div>"#),
        "{html}"
    );
    assert!(html.contains(r#"<div class="segments">"#), "{html}");
    assert!(
        html.contains(r#"<div class="line">building #2 · 2m 00s · acme/widgets#13</div>"#),
        "{html}"
    );
}

#[test]
fn the_segments_are_one_for_each_issue_in_its_state() {
    let segments = vec![
        NodeState::Merged,
        NodeState::Running,
        NodeState::NotStarted,
        NodeState::Failed,
    ];
    let html = render(|| view! { <Segments segments/> });
    assert_eq!(
        html,
        r#"<div class="segments"><span class="merged"></span><span class="running"></span><span class="not_started"></span><span class="failed"></span></div>"#
    );
}

#[test]
fn the_runs_are_a_row_for_each_active_run_under_their_heading() {
    let html = render(|| view! { <Runs runs=popover(vec![tray_run(), tray_run()]).runs/> });
    assert!(
        html.starts_with(r#"<section class="runs"><h2>Active runs</h2>"#),
        "{html}"
    );
    assert_eq!(
        html.matches(r#"<button type="button" class="run">"#)
            .count(),
        2,
        "{html}"
    );
}

#[test]
fn with_nothing_building_the_runs_are_left_out() {
    let html = render(|| view! { <Runs runs=vec![]/> });
    assert_eq!(html, "");
}

#[test]
fn the_items_are_in_order_with_their_shortcuts() {
    let html = render(|| view! { <Items mac=true/> });
    let labels: Vec<&str> = Item::ALL.iter().map(|item| item.label()).collect();
    assert_eq!(
        labels,
        [
            "Open Dashboard",
            "Restart",
            "Settings…",
            "Check setup…",
            "Check for updates…",
            "Quit Epik",
        ]
    );
    let mut at = 0;
    for item in Item::ALL {
        let found = html[at..]
            .find(&format!(r#"id="{}""#, item.id()))
            .unwrap_or_else(|| panic!("{} is missing or out of order: {html}", item.id()));
        at += found;
    }
    assert!(
        html.contains(
            r#"<span class="label">Open Dashboard</span><span class="shortcut">⌘D</span>"#
        ),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="shortcut">⌘,</span>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="shortcut">⌘Q</span>"#),
        "{html}"
    );
    assert_eq!(html.matches(r#"class="shortcut""#).count(), 3, "{html}");
}

#[test]
fn the_items_are_grouped_as_the_menu_draws_them() {
    let html = render(|| view! { <Items mac=true/> });
    assert_eq!(html.matches(r#"<hr>"#).count(), 2, "{html}");
    let dashboard = html.find("open-dashboard").unwrap();
    let restart = html.find("restart").unwrap();
    let settings = html.find("settings").unwrap();
    let rules: Vec<usize> = html.match_indices("<hr>").map(|(i, _)| i).collect();
    assert!(dashboard < rules[0] && rules[0] < restart, "{html}");
    assert!(restart < rules[1] && rules[1] < settings, "{html}");
}

#[test]
fn elsewhere_the_shortcuts_use_ctrl() {
    let html = render(|| view! { <Items mac=false/> });
    assert!(
        html.contains(r#"<span class="shortcut">Ctrl+D</span>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="shortcut">Ctrl+,</span>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="shortcut">Ctrl+Q</span>"#),
        "{html}"
    );
}

#[test]
fn each_item_calls_its_command() {
    let commands: Vec<Command> = Item::ALL.iter().map(|item| item.command()).collect();
    assert_eq!(
        commands,
        [
            Command::OpenMain("".into()),
            Command::Run("restart"),
            Command::OpenMain("settings".into()),
            Command::Run("check_setup"),
            Command::Run("check_for_updates"),
            Command::Run("quit"),
        ]
    );
}

#[test]
fn a_run_row_opens_its_run_page() {
    assert_eq!(
        Command::open_run("acme/widgets#13"),
        Command::OpenMain("run/acme/widgets#13".into())
    );
}

#[test]
fn the_shortcuts_choose_their_items() {
    assert_eq!(shortcut("d"), Some(Item::OpenDashboard));
    assert_eq!(shortcut("D"), Some(Item::OpenDashboard));
    assert_eq!(shortcut(","), Some(Item::Settings));
    assert_eq!(shortcut("q"), Some(Item::Quit));
    assert_eq!(shortcut("r"), None);
}
