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

use crate::{main_window, navigation, pages, setup_window, tray, update};

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
#[derive(Clone, Default)]
pub struct Quit {
    pub stop: Arc<AtomicBool>,
    pub restart: Arc<AtomicBool>,
    /// Set once serving has stopped.
    pub stopped: Arc<AtomicBool>,
    /// Set when an installer is to end the app, as on Windows, rather
    /// than the app ending itself once serving stops.
    pub installing: Arc<AtomicBool>,
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

    /// Stops the builds and the server and waits until they have, leaving
    /// the app running for an installer to end.
    pub fn stop_for_installer(&self) {
        self.installing.store(true, Ordering::SeqCst);
        self.quit();
        while !self.stopped.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
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

/// Checks for updates or installs the one found, from the menu, and
/// hides it.
#[tauri::command]
pub fn check_for_updates(handle: AppHandle, window: State<'_, main_window::Window>) {
    hide(&handle);
    update::choose(&handle, tray::building_here(&window.runs));
}

/// Asks `question` in a dialog before restarting, warning of the
/// `building` runs it interrupts, and calls `restart` if the user chooses
/// `choice`: for Linux's native tray menu, which can't ask itself, and for
/// updating.
pub fn ask_to_restart(
    handle: &AppHandle,
    question: String,
    building: usize,
    choice: &str,
    restart: impl FnOnce(AppHandle) + Send + 'static,
) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let answered = handle.clone();
    handle
        .dialog()
        .message(pages::restart_warning(building))
        .title(question)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            choice.to_owned(),
            "Cancel".to_owned(),
        ))
        .show(move |chosen| {
            if chosen {
                restart(answered);
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
        let quit = Quit::default();
        quit.restart();
        assert!(quit.stop.load(Ordering::SeqCst));
        assert!(quit.restart.load(Ordering::SeqCst));
    }

    #[test]
    fn quitting_doesnt_restart() {
        let quit = Quit::default();
        quit.quit();
        assert!(quit.stop.load(Ordering::SeqCst));
        assert!(!quit.restart.load(Ordering::SeqCst));
    }

    #[test]
    fn an_installer_stops_serving_and_waits_but_leaves_the_app_to_it() {
        let quit = Quit::default();
        let server = std::thread::spawn({
            let quit = quit.clone();
            move || {
                while !quit.stop.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
                quit.stopped.store(true, Ordering::SeqCst);
            }
        });
        quit.stop_for_installer();
        assert!(quit.stopped.load(Ordering::SeqCst));
        assert!(quit.installing.load(Ordering::SeqCst));
        assert!(!quit.restart.load(Ordering::SeqCst));
        server.join().unwrap();
    }
}
