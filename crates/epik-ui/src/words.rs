//! A sentence whose issue and pull request numbers link to GitHub.

use epik_view::Span;
use leptos::ev::MouseEvent;
use leptos::prelude::*;
use serde::Serialize;

use crate::tauri;

#[derive(Serialize)]
struct OpenGithub {
    url: String,
}

/// A sentence of spans: a span with a `url` is a link that opens GitHub in
/// the browser, a span with `code` is code, and otherwise it's text.
#[component]
pub fn Words(spans: Vec<Span>, #[prop(optional)] class: Option<&'static str>) -> impl IntoView {
    let spans = spans
        .into_iter()
        .map(|span| match span.url {
            Some(url) => {
                let open = url.clone();
                let click = move |event: MouseEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                    tauri::run("open_github", OpenGithub { url: open.clone() });
                };
                view! { <a href=url on:click=click>{span.text}</a> }.into_any()
            }
            None if span.code => view! { <code>{span.text}</code> }.into_any(),
            None => span.text.into_any(),
        })
        .collect_view();
    match class {
        Some(class) => view! { <span class=class>{spans}</span> }.into_any(),
        None => view! { <span>{spans}</span> }.into_any(),
    }
}
