//! The tray menu: a small window, opened by clicking the tray icon, that
//! says whether Epik is running and what is building, and leads into the
//! main window. It hides when it loses focus. Linux doesn't report tray
//! clicks, so there the tray has a native menu of the same items instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    AppHandle, Manager, PhysicalPosition, Rect, State, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};

use crate::{main_window, navigation, pages, setup_window};

/// The menu's label.
pub(crate) const LABEL: &str = "popover";
/// Its width, in logical pixels.
const WIDTH: f64 = 330.0;
/// The most runs it makes room for; it scrolls to show more.
const MOST_RUNS: usize = 5;

/// Where Epik's releases are, which Check for updates opens.
pub const RELEASES: &str = "https://github.com/epik-agent/Epik/releases";

/// Set to quit the app, as the tray's Quit does; and to start it again
/// once it has quit, as Restart does.
pub struct Quit {
    pub stop: Arc<AtomicBool>,
    pub restart: Arc<AtomicBool>,
}

impl Quit {
    /// Stops the builds and the server, and so the app.
    pub fn quit(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Quits, then starts Epik again.
    pub fn restart(&self) {
        self.restart.store(true, Ordering::SeqCst);
        self.quit();
    }
}

/// The menu's height, in logical pixels, with `building` runs: the header
/// and items, and the active runs under their heading, if any.
fn height(building: usize) -> f64 {
    let runs = match building.min(MOST_RUNS) {
        0 => 0.0,
        n => 34.0 + 62.0 * n as f64,
    };
    272.0 + runs
}

/// Shows the popover under the tray icon at `rect`, or above it when the
/// icon is in the lower half of the screen, as on Windows; or hides it if
/// it is showing.
/// It is as tall as `building` runs need.
pub fn toggle(handle: &AppHandle, rect: Rect, building: usize) -> tauri::Result<()> {
    let (width, height) = (WIDTH, height(building));
    let window = match handle.get_webview_window(LABEL) {
        Some(window) => {
            if window.is_visible()? {
                return window.hide();
            }
            window
        }
        None => {
            let url = WebviewUrl::App("index.html#/popover".into());
            let window = WebviewWindowBuilder::new(handle, LABEL, url)
                .title("Epik")
                .inner_size(width, height)
                .resizable(false)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .visible(false)
                .on_navigation(navigation::stay_in_app)
                .build()?;
            let hide = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::Focused(false) = event {
                    let _ = hide.hide();
                }
            });
            window
        }
    };
    window.set_size(tauri::LogicalSize::new(width, height))?;
    let scale = window.scale_factor()?;
    let icon_at = rect.position.to_physical::<f64>(scale);
    let icon_size = rect.size.to_physical::<f64>(scale);
    let (width, height) = (width * scale, height * scale);
    let screen_height = window
        .current_monitor()?
        .map_or(f64::MAX, |monitor| f64::from(monitor.size().height));
    let x = icon_at.x + icon_size.width / 2.0 - width / 2.0;
    let y = if icon_at.y > screen_height / 2.0 {
        icon_at.y - height - 4.0
    } else {
        icon_at.y + icon_size.height + 4.0
    };
    window.set_position(PhysicalPosition::new(x.max(0.0), y.max(0.0)))?;
    window.show()?;
    window.set_focus()
}

fn hide(handle: &AppHandle) {
    if let Some(window) = handle.get_webview_window(LABEL) {
        let _ = window.hide();
    }
}

/// Opens the main window at `route`, from the menu, and hides it.
#[tauri::command]
pub fn open_main(route: String, handle: AppHandle) -> Result<(), String> {
    hide(&handle);
    main_window::open(&handle, &route).map_err(|error| error.to_string())
}

/// Opens the setup checklist, from the menu, and hides it.
#[tauri::command]
pub fn check_setup(handle: AppHandle) -> Result<(), String> {
    hide(&handle);
    setup_window::open(&handle).map_err(|error| error.to_string())
}

/// Opens Epik's releases in the browser, from the menu, and hides it.
#[tauri::command]
pub fn check_for_updates(handle: AppHandle) -> Result<(), String> {
    hide(&handle);
    open::that_detached(RELEASES).map_err(|error| error.to_string())
}

/// Asks in a dialog before restarting, warning of the `building` runs it
/// interrupts: for Linux's native tray menu, which can't ask itself.
pub fn ask_to_restart(handle: &AppHandle, building: usize) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let answered = handle.clone();
    handle
        .dialog()
        .message(pages::restart_warning(building))
        .title(pages::restart_question(building))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Restart".to_owned(),
            "Cancel".to_owned(),
        ))
        .show(move |restart| {
            if restart {
                answered.state::<Quit>().restart();
            }
        });
}

/// Quits Epik, interrupting any build, and starts it again. The tray menu
/// asks first.
#[tauri::command]
pub fn restart(quit: State<'_, Quit>) {
    quit.restart();
}

/// Quits Epik, interrupting any build.
#[tauri::command]
pub fn quit(quit: State<'_, Quit>) {
    quit.quit();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_makes_room_for_each_run_up_to_five() {
        let room = |n| height(n) - height(0);
        assert_eq!(room(0), 0.0);
        assert_eq!(room(1), 96.0);
        assert_eq!(room(2) - room(1), 62.0);
        assert_eq!(room(5), room(9));
    }

    #[test]
    fn restarting_quits_first() {
        let quit = Quit {
            stop: Arc::default(),
            restart: Arc::default(),
        };
        quit.restart();
        assert!(quit.stop.load(Ordering::SeqCst));
        assert!(quit.restart.load(Ordering::SeqCst));
    }

    #[test]
    fn quitting_doesnt_restart() {
        let quit = Quit {
            stop: Arc::default(),
            restart: Arc::default(),
        };
        quit.quit();
        assert!(quit.stop.load(Ordering::SeqCst));
        assert!(!quit.restart.load(Ordering::SeqCst));
    }
}
