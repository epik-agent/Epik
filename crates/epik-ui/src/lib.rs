//! The Epik app's windows, as a Leptos app compiled to WebAssembly. There
//! is one app for every window, which draws the window its URL's hash
//! names.
//!
//! The windows draw the view models in `epik-view`, which they get from the
//! app's Tauri commands.

pub mod flash;
pub mod graph;
pub mod home;
pub mod live;
pub mod main_window;
pub mod popover;
pub mod run_page;
pub mod settings;
pub mod setup;
pub mod stream;
pub mod tauri;
pub mod words;

/// A window, as its URL's hash names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `#/popover`: the tray's popover.
    Popover,
    /// `#/setup`: the Set up Epik checklist.
    Setup,
    /// Any other hash: the main window, whose page the hash names.
    Main,
}

impl Route {
    /// The window at `hash`.
    pub fn from_hash(hash: &str) -> Route {
        match hash {
            "#/popover" => Route::Popover,
            "#/setup" => Route::Setup,
            _ => Route::Main,
        }
    }

    /// The window's own stylesheet. Each window styles the page's `body`,
    /// `header` and `main` its own way, so only its own is loaded.
    pub fn stylesheet(self) -> &'static str {
        match self {
            Route::Popover => "popover.css",
            Route::Setup => "setup.css",
            Route::Main => "app.css",
        }
    }

    /// The window's title.
    pub fn title(self) -> &'static str {
        match self {
            Route::Popover => "Epik",
            Route::Setup => "Set up Epik",
            Route::Main => "Epik",
        }
    }
}

/// Draws the window the URL's hash names in the page's body.
#[cfg(feature = "csr")]
pub fn mount() {
    use leptos::prelude::{document, window};

    let hash = window().location().hash().unwrap_or_default();
    let route = Route::from_hash(&hash);
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
        Route::Main => leptos::mount::mount_to_body(main_window::MainWindow),
    }
}
