//! What the stylesheets promise about how an issue's state is drawn.

const APP: &str = include_str!("../app.css");
const POPOVER: &str = include_str!("../popover.css");
const THEME: &str = include_str!("../epik.css");

/// The declarations of the rule for `selector`, alone or among others.
fn rule<'a>(css: &'a str, selector: &str) -> &'a str {
    css.split('}')
        .filter_map(|rule| rule.split_once('{'))
        .find(|(selectors, _)| selectors.split(',').any(|s| s.trim() == selector))
        .map_or("", |(_, declarations)| declarations)
}

fn blinks(css: &str, selector: &str) -> bool {
    rule(css, selector).contains("animation: blink 1s steps(1) infinite")
}

#[test]
fn everything_running_blinks() {
    for selector in [
        "#sidebar .dot.building",
        ".run-table .status .dot.building",
        ".issue.running .dot",
        ".mini-node.running",
        ".progress .running",
    ] {
        assert!(blinks(APP, selector), "{selector}");
    }
    for selector in [".segments .running", ".dot.building"] {
        assert!(blinks(POPOVER, selector), "{selector}");
    }
}

#[test]
fn every_window_has_the_blink() {
    assert!(THEME.contains("@keyframes blink"));
}

#[test]
fn a_dot_for_a_run_that_is_not_building_does_not_blink() {
    for selector in [
        "#sidebar .dot",
        "#sidebar .dot.waiting_for_review",
        "#sidebar .dot.needs_attention",
        ".run-table .status .dot",
        ".run-table .status .dot.waiting_for_review",
        ".run-table .status .dot.needs_attention",
    ] {
        assert!(!blinks(APP, selector), "{selector}");
    }
    assert!(!blinks(POPOVER, ".dot"));
}

#[test]
fn nothing_flashes() {
    assert!(!APP.contains("flash"));
}

#[test]
fn each_state_has_its_own_card() {
    let not_started = rule(APP, ".issue.not_started");
    assert!(not_started.contains("border: 1px dashed var(--strong)"));
    assert!(not_started.contains("background: transparent"));
    let running = rule(APP, ".issue.running");
    assert!(running.contains("border: 1.5px solid var(--accent)"));
    assert!(running.contains("background: var(--surface)"));
    let merged = rule(APP, ".issue.merged");
    assert!(merged.contains("border-color: var(--accent)"));
    assert!(merged.contains("background: var(--accent-muted)"));
    let failed = rule(APP, ".issue.failed");
    assert!(failed.contains("border: 1.5px solid var(--error)"));
    assert!(failed.contains("background: var(--error-muted)"));
}

#[test]
fn nothing_on_a_card_not_attempted_is_green() {
    assert!(rule(APP, ".issue.not_started a").contains("color: var(--muted)"));
}

#[test]
fn a_chosen_running_card_is_not_tinted() {
    assert_eq!(rule(APP, ".issue.running.chosen"), "");
}

#[test]
fn each_state_has_its_own_dot() {
    assert!(rule(APP, ".issue .dot").contains("border: 1.5px solid var(--strong)"));
    assert!(rule(APP, ".issue.running .dot").contains("background: var(--accent-fill)"));
    assert!(rule(APP, ".issue.merged .dot").contains("background: var(--accent)"));
    assert!(rule(APP, ".issue.failed .dot").contains("background: var(--error)"));
}

#[test]
fn the_small_graph_and_the_bars_draw_the_states_in_the_same_colors() {
    assert!(rule(APP, ".mini-node").contains("stroke: var(--strong)"));
    assert!(rule(APP, ".mini-node.running").contains("fill: var(--accent-fill)"));
    assert!(rule(APP, ".mini-node.merged").contains("fill: var(--accent-fill)"));
    assert!(rule(APP, ".mini-node.failed").contains("fill: var(--error)"));
    for (css, bar) in [(APP, ".progress"), (POPOVER, ".segments")] {
        let color = |state: &str| rule(css, &format!("{bar} .{state}")).to_owned();
        assert!(
            color("merged").contains("background: var(--accent-fill)"),
            "{bar}"
        );
        assert!(
            color("running").contains("background: var(--accent-fill)"),
            "{bar}"
        );
        assert!(
            color("failed").contains("background: var(--error)"),
            "{bar}"
        );
        assert!(
            color("not_started").contains("background: var(--strong)"),
            "{bar}"
        );
    }
}
