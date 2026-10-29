//! The Epik app's windows, as a Leptos app compiled to WebAssembly. There
//! is one app for every window, which draws the window its URL's hash
//! names.
//!
//! The windows draw the view models in `epik-view`, which they get from the
//! app's Tauri commands.

pub mod graph;
pub mod popover;
pub mod setup;
pub mod tauri;
pub mod words;

/// A window, as its URL's hash names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `#/popover`: the tray's popover.
    Popover,
    /// `#/setup`: the Set up Epik checklist.
    Setup,
}

impl Route {
    /// The window at `hash`, if it has been ported here. The main window
    /// still loads its own HTML file.
    pub fn from_hash(hash: &str) -> Option<Route> {
        match hash {
            "#/popover" => Some(Route::Popover),
            "#/setup" => Some(Route::Setup),
            _ => None,
        }
    }

    /// The window's own stylesheet. Each window styles the page's `body`,
    /// `header` and `main` its own way, so only its own is loaded.
    pub fn stylesheet(self) -> &'static str {
        match self {
            Route::Popover => "popover.css",
            Route::Setup => "setup.css",
        }
    }

    /// The window's title.
    pub fn title(self) -> &'static str {
        match self {
            Route::Popover => "Epik",
            Route::Setup => "Set up Epik",
        }
    }
}

/// Draws the window the URL's hash names in the page's body.
#[cfg(feature = "csr")]
pub fn mount() {
    use leptos::prelude::{document, window};

    let hash = window().location().hash().unwrap_or_default();
    let Some(route) = Route::from_hash(&hash) else {
        return;
    };
    let document = document();
    document.set_title(route.title());
    if let (Some(head), Ok(link)) = (document.head(), document.create_element("link")) {
        let _ = link.set_attribute("rel", "stylesheet");
        let _ = link.set_attribute("href", route.stylesheet());
        let _ = head.append_child(&link);
    }
    match route {
        Route::Popover => leptos::mount::mount_to_body(popover::PopoverWindow),
        Route::Setup => leptos::mount::mount_to_body(setup::SetupWindow),
    }
}
