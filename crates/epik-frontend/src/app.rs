//! The window: a tab strip over the chat and a pane per feature build.
//!
//! The chat is the first tab and stays mounted whichever tab is open —
//! hidden, not unmounted, so its transcript and draft survive a
//! switch. The feature tabs and panes draw the [`View`] fold, fed from
//! the moment the window opens: the listener first, the replay after.

use leptos::ev;
use leptos::prelude::*;

use crate::chat::Chat;
use crate::ipc;
use crate::monitor::{self, Tab, View};
use crate::pane::{FeaturePane, TabStrip};

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

    let view = RwSignal::new(View::default());
    monitor::attach(ipc::Monitor, move |step| {
        view.update(|view| {
            view.step(step);
        });
    });

    view! {
        <div class="flex h-screen flex-col bg-neutral-50 dark:bg-neutral-900">
            <TabStrip view />
            <div class="min-h-0 flex-1" class:hidden=move || view.with(|view| view.tab() != Tab::Chat)>
                <Chat />
            </div>
            {move || view.with(View::pane).map(|pane| view! { <FeaturePane view pane /> })}
        </div>
    }
}
