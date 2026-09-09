//! The page: a tab strip over the chat and a pane per feature build.
//!
//! One bundle, two surfaces, decided once as the page mounts: in the
//! window, Tauri's bridge is on `window` and the feed is its event
//! channel; in a browser, neither is, and the feed is the event stream
//! from the origin the page came from. The chat is the window's first
//! tab and stays mounted whichever tab is open — hidden, not unmounted,
//! so its transcript and draft survive a switch. A browser has no
//! backend to send to, so it has no chat tab: the page is the monitor
//! alone. The feature tabs and panes draw the [`View`] fold, fed from
//! the moment the page opens: the listener first, the replay after.

use leptos::ev;
use leptos::prelude::*;

use crate::chat::Chat;
use crate::monitor::{self, Surface, Tab, View};
use crate::pane::{FeaturePane, TabStrip};
use crate::{http, ipc};

#[component]
pub fn App() -> impl IntoView {
    let surface = if ipc::in_window() {
        Surface::Window
    } else {
        Surface::Browser
    };
    let view = RwSignal::new(View::new(surface));
    let apply = move |step| {
        view.update(|view| {
            view.step(step);
        });
    };
    match surface {
        Surface::Window => monitor::attach(ipc::Monitor, apply),
        Surface::Browser => monitor::attach(http::Monitor, apply),
    }

    let chat = view.with_untracked(View::has_chat).then(|| {
        // Cmd+, is the platform convention on macOS, Ctrl+, elsewhere;
        // accepting either modifier serves both without asking the OS
        // which one this is.
        let handle = window_event_listener(ev::keydown, move |event| {
            if (event.meta_key() || event.ctrl_key()) && event.key() == "," {
                event.prevent_default();
                ipc::open_settings();
            }
        });
        on_cleanup(move || handle.remove());
        view! {
            <div class="min-h-0 flex-1" class:hidden=move || view.with(|view| view.tab() != Tab::Chat)>
                <Chat />
            </div>
        }
    });

    view! {
        <div class="flex h-screen flex-col bg-neutral-50 dark:bg-neutral-900">
            <TabStrip view />
            {chat}
            {move || view.with(View::pane).map(|pane| view! { <FeaturePane view pane /> })}
        </div>
    }
}
