use leptos::ev;
use leptos::prelude::*;

use crate::ipc;

#[component]
pub fn App() -> impl IntoView {
    // Cmd+, is the platform convention on macOS, Ctrl+, elsewhere; accepting
    // either modifier serves both without asking the OS which one this is.
    let handle = window_event_listener(ev::keydown, move |event| {
        if (event.meta_key() || event.ctrl_key()) && event.key() == "," {
            event.prevent_default();
            ipc::open_settings();
        }
    });
    on_cleanup(move || handle.remove());

    view! {
        <main class="flex h-screen items-center justify-center">
            <h1 class="text-2xl font-semibold text-gray-400">"Epik"</h1>
        </main>
    }
}
