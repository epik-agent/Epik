//! The window's root component.
//!
//! Components are called by the `view!` macro, never by code that could
//! sensibly discard the result, so `#[must_use]` on each one is noise.
#![allow(clippy::must_use_candidate)]

use leptos::prelude::*;

/// The whole window. Its first occupant — a chat pane — arrives with the
/// chat core; for now it proves the toolchain end to end.
#[component]
pub fn App() -> impl IntoView {
    view! {
        <main>
            <h1>"Epik"</h1>
            <p>"You say it. We make it."</p>
        </main>
    }
}
