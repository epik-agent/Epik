//! What the stylesheets promise about how an issue's state is drawn.

const APP: &str = include_str!("../app.css");

#[test]
fn nothing_flashes() {
    assert!(!APP.contains("flash"));
}
