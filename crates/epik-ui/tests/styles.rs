//! What the stylesheets promise about how an issue's state is drawn.

const APP: &str = include_str!("../app.css");

#[test]
fn nothing_flashes() {
    assert!(!APP.contains("flash"));
}

#[test]
fn a_progress_bar_draws_failed_issues_in_red() {
    assert!(APP.contains(".progress .failed { background: var(--error); }"));
}
