use epik_ui::words::Words;
use leptos::prelude::*;

mod fixtures;
use fixtures::render;
use fixtures::{code, link, text};

#[test]
fn a_linked_span_is_a_link_to_its_url() {
    let spans = vec![
        text("Merged in "),
        link("PR #20", "https://github.com/acme/widgets/pull/20"),
    ];
    let html = render(view! { <Words spans/> });
    assert!(
        html.contains(r#"<a href="https://github.com/acme/widgets/pull/20">PR #20</a>"#),
        "{html}"
    );
    assert!(html.contains("Merged in "), "{html}");
}

#[test]
fn a_code_span_is_code() {
    let html = render(view! { <Words spans=vec![text("ran "), code("cargo test")]/> });
    assert!(html.contains("<code>cargo test</code>"), "{html}");
    assert!(!html.contains("<a"), "{html}");
}

#[test]
fn a_sentence_is_a_span_with_its_class() {
    let html = render(view! { <Words spans=vec![text("hello")] class="summary"/> });
    assert!(html.starts_with(r#"<span class="summary">"#), "{html}");
    assert!(html.ends_with("</span>"), "{html}");
}

#[test]
fn a_sentence_without_a_class_has_no_class() {
    let html = render(view! { <Words spans=vec![text("hello")]/> });
    assert!(html.starts_with("<span>"), "{html}");
}
