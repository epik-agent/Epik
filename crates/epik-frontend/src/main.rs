mod app;
mod chat;
mod ipc;
mod settings;

use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    // One bundle, two windows: the backend opens the settings window on
    // this same page with ?window=settings, and the query decides the view.
    let is_settings = window()
        .location()
        .search()
        .is_ok_and(|search| search.contains("window=settings"));
    if is_settings {
        mount_to_body(settings::SettingsPage)
    } else {
        mount_to_body(app::App)
    }
}
