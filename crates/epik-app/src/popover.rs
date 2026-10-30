//! The tray's popover: a small window, opened by clicking the tray icon,
//! that says what is happening and leads into the main window. It hides
//! when it loses focus. Linux doesn't report tray clicks, so there the
//! tray's menu and the main window are the way in.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    AppHandle, Manager, PhysicalPosition, Rect, State, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};

use crate::{main_window, navigation};

/// The popover's label.
const LABEL: &str = "popover";
/// Its size, in logical pixels.
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 520.0;

/// Set to quit the app, as the tray's Quit does.
pub struct Quit(pub Arc<AtomicBool>);

/// Shows the popover under the tray icon at `rect`, or above it when the
/// icon is in the lower half of the screen, as on Windows; or hides it if
/// it is showing.
pub fn toggle(handle: &AppHandle, rect: Rect) -> tauri::Result<()> {
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
                .inner_size(WIDTH, HEIGHT)
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
    let scale = window.scale_factor()?;
    let icon_at = rect.position.to_physical::<f64>(scale);
    let icon_size = rect.size.to_physical::<f64>(scale);
    let (width, height) = (WIDTH * scale, HEIGHT * scale);
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

/// Opens the main window at `route`, from the popover, and hides it.
#[tauri::command]
pub fn open_main(route: String, handle: AppHandle) -> Result<(), String> {
    hide(&handle);
    main_window::open(&handle, &route).map_err(|error| error.to_string())
}

/// Quits Epik, interrupting any build.
#[tauri::command]
pub fn quit(quit: State<'_, Quit>) {
    quit.0.store(true, Ordering::SeqCst);
}
