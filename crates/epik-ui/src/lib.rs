//! The Epik app's windows, as a Leptos app compiled to WebAssembly. There
//! is one app for every window, which draws the window its URL's hash
//! names.
//!
//! The windows draw the view models in `epik-view`, which they get from the
//! app's Tauri commands.

pub mod graph;
pub mod popover;
pub mod tauri;
pub mod words;

/// A window, as its URL's hash names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `#/popover`: the tray's popover.
    Popover,
}

impl Route {
    /// The window at `hash`, if it has been ported here. The setup window
    /// and the main window still load their own HTML files.
    pub fn from_hash(hash: &str) -> Option<Route> {
        match hash {
            "#/popover" => Some(Route::Popover),
            _ => None,
        }
    }
}

/// Draws the window the URL's hash names in the page's body.
#[cfg(feature = "csr")]
pub fn mount() {
    let hash = leptos::prelude::window()
        .location()
        .hash()
        .unwrap_or_default();
    if let Some(Route::Popover) = Route::from_hash(&hash) {
        leptos::mount::mount_to_body(popover::PopoverWindow);
    }
}
