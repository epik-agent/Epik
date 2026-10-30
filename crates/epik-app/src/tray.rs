//! The app in the menu bar (macOS), notification area (Windows) or system
//! tray (Linux).
//!
//! The tray runs the same socket and builds as `--headless`, with Tauri's
//! event loop on the main thread. Clicking its icon opens the popover; its
//! menu (a right click, or on Linux, where tray clicks aren't reported, any
//! click) opens Epik's window, checks setup, or quits. The icon shows
//! whether anything is building, and with a red dot, whether any run needs
//! attention. On macOS, Epik is in the Dock and Cmd-Tab only while its main
//! window is open. Where there is no tray, the app runs headless and says so.

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use epik_core::repositories;
use epik_core::run::Disposition;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{RunEvent, WindowEvent};

use crate::App;
use crate::{main_window, popover, setup_window};

/// How often the icon is checked against what is building and what needs
/// attention, besides each time a build changes.
const REFRESH: Duration = Duration::from_secs(2);

const QUIT: &str = "quit";
const CHECK_SETUP: &str = "check-setup";
const OPEN_EPIK: &str = "open-epik";

/// Runs the app in the tray until Quit, SIGINT or SIGTERM, or headless
/// where there is no tray.
pub fn run() -> ExitCode {
    let (changed, changes) = mpsc::channel();
    let app = match App::start(move |_| {
        let _ = changed.send(());
    }) {
        Ok(app) => app,
        Err(code) => return code,
    };
    match panic::catch_unwind(AssertUnwindSafe(|| open(&app))) {
        Ok(Ok((tauri, tray))) => in_tray(app, tauri, tray, changes),
        Ok(Err(reason)) => headless(app, &reason),
        Err(_) => headless(app, "the tray failed to start"),
    }
}

fn headless(app: App, reason: &str) -> ExitCode {
    eprintln!("epik-app: there is no tray ({reason}), so running headless");
    app.serve()
}

/// Starts Tauri with no window and puts the icon in the tray, or says why
/// it can't.
fn open(app: &App) -> Result<(tauri::App, TrayIcon), String> {
    if !has_display() {
        return Err("no display".to_owned());
    }
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut tauri = tauri::Builder::default()
        .manage(main_window::Window {
            root: app.root.clone(),
            builds: app.builds.clone(),
            started: chrono::Utc::now(),
        })
        .manage(setup_window::Setup {
            root: app.root.clone(),
            builds: app.builds.clone(),
            outputs: Default::default(),
        })
        .manage(popover::Quit(app.stop.clone()))
        .invoke_handler(tauri::generate_handler![
            main_window::home,
            main_window::sidebar,
            main_window::popover,
            main_window::run_page,
            main_window::rerun,
            main_window::delete_run,
            main_window::settings,
            main_window::monitor,
            main_window::stop_monitoring,
            main_window::uninstall_plan,
            main_window::uninstall,
            main_window::open_github,
            main_window::agent_stream,
            popover::open_main,
            popover::quit,
            setup_window::setup_checklist,
            setup_window::setup_run,
            setup_window::setup_output,
        ])
        .build(tauri::generate_context!())
        .map_err(|error| error.to_string())?;
    // No Dock icon until the main window opens.
    #[cfg(target_os = "macos")]
    tauri.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let handle = tauri.handle();
    let failed = |error: tauri::Error| error.to_string();
    let menu = Menu::with_items(
        handle,
        &[
            &MenuItem::with_id(handle, OPEN_EPIK, "Open Epik", true, None::<&str>)
                .map_err(failed)?,
            &MenuItem::with_id(handle, CHECK_SETUP, "Check setup", true, None::<&str>)
                .map_err(failed)?,
            &PredefinedMenuItem::separator(handle).map_err(failed)?,
            &MenuItem::with_id(handle, QUIT, "Quit Epik", true, None::<&str>).map_err(failed)?,
        ],
    )
    .map_err(failed)?;
    let stop = app.stop.clone();
    let idle = look(TEMPLATE, false, false);
    let tray = TrayIconBuilder::new()
        .icon(icon(idle))
        .icon_as_template(idle.template)
        .tooltip("Epik")
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "linux"))
        .on_menu_event(move |handle, event| {
            let opened = match event.id().as_ref() {
                QUIT => {
                    stop.store(true, Ordering::SeqCst);
                    Ok(())
                }
                CHECK_SETUP => setup_window::open(handle),
                OPEN_EPIK => main_window::open(handle, ""),
                _ => Ok(()),
            };
            if let Err(error) = opened {
                eprintln!("epik-app: couldn't open the window: {error}");
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
                && let Err(error) = popover::toggle(tray.app_handle(), rect)
            {
                eprintln!("epik-app: couldn't open the popover: {error}");
            }
        })
        .build(&tauri)
        .map_err(failed)?;
    Ok((tauri, tray))
}

/// Whether there is a desktop to put a tray on. Linux without X or Wayland
/// has none, and Tauri would panic looking for one.
fn has_display() -> bool {
    !cfg!(target_os = "linux")
        || ["DISPLAY", "WAYLAND_DISPLAY"]
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// Serves on one thread and keeps the icon current on another while Tauri
/// runs on this one. When serving stops, the builds are stopped, then
/// Tauri.
fn in_tray(app: App, tauri: tauri::App, tray: TrayIcon, changes: Receiver<()>) -> ExitCode {
    let (root, builds) = (app.root.clone(), app.builds.clone());
    let stop = app.stop.clone();
    thread::spawn(move || {
        let mut shown = look(TEMPLATE, false, false);
        while !stop.load(Ordering::SeqCst) {
            let building = !builds.active().is_empty();
            let now = look(TEMPLATE, building, needs_attention(&root));
            if shown != now {
                let drawn = tray
                    .set_icon(Some(icon(now)))
                    .and_then(|()| tray.set_icon_as_template(now.template));
                if let Err(error) = drawn {
                    eprintln!("epik-app: couldn't redraw the tray: {error}");
                }
                shown = now;
            }
            if let Err(RecvTimeoutError::Disconnected) = changes.recv_timeout(REFRESH) {
                break;
            }
        }
    });
    thread::spawn({
        let handle = tauri.handle().clone();
        let (root, builds) = (app.root.clone(), app.builds.clone());
        move || {
            // Not every Linux desktop shows tray icons, so there the
            // window is the way in.
            if cfg!(target_os = "linux") {
                let opener = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    if let Err(error) = main_window::open(&opener, "") {
                        eprintln!("epik-app: couldn't open Epik's window: {error}");
                    }
                });
            }
            if setup_window::opens_at_start(&root, &builds.setup()) {
                let opener = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    if let Err(error) = setup_window::open(&opener) {
                        eprintln!("epik-app: couldn't open the setup checklist: {error}");
                    }
                });
            }
        }
    });
    let handle = tauri.handle().clone();
    let server = thread::spawn(move || {
        let code = app.serve();
        handle.exit(0);
        code
    });
    tauri.run_return(|handle, event| match event {
        // Closing the last window doesn't quit an app that lives in the
        // tray; only Quit does.
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        // Closing the main window takes Epik out of the Dock.
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Destroyed,
            ..
        } => main_window::follow(handle, &label, main_window::Change::Closed),
        // Clicking the Dock icon opens the main window.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            if let Err(error) = main_window::open(handle, "") {
                eprintln!("epik-app: couldn't open Epik's window: {error}");
            }
        }
        _ => {}
    });
    server.join().unwrap_or(ExitCode::FAILURE)
}

/// Whether any run of a monitored repository needs attention. If the runs
/// can't be read, none is said to.
fn needs_attention(root: &Path) -> bool {
    repositories::monitored_runs(root).is_ok_and(|runs| {
        runs.iter()
            .any(|run| run.disposition == Disposition::NeedsAttention)
    })
}

/// Whether the icons are templates, which the menu bar tints: on macOS.
const TEMPLATE: bool = cfg!(target_os = "macos");

/// Which icon the tray shows, and whether it is a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Look {
    name: &'static str,
    template: bool,
}

/// The icon for whether the tray takes templates, whether anything is
/// building, and whether any run needs attention, which a red dot shows. A
/// template is one color, so it can't carry the dot: then the icon is drawn
/// in gray instead, which reads on light and dark menu bars.
fn look(template: bool, building: bool, attention: bool) -> Look {
    let name = match (template, building, attention) {
        (true, false, false) => "idle-template",
        (true, true, false) => "building-template",
        (true, false, true) => "idle-attention-gray",
        (true, true, true) => "building-attention-gray",
        (false, false, false) => "idle",
        (false, true, false) => "building",
        (false, false, true) => "idle-attention",
        (false, true, true) => "building-attention",
    };
    Look {
        name,
        template: template && !attention,
    }
}

/// An icon, at 2x, which the tray scales to its size. On macOS the
/// templates have hollow nodes when idle and solid when building; elsewhere
/// the icons are gray when idle and in the mark's colors when building.
/// `icons/make-tray-icons.py` draws them.
fn icon(look: Look) -> Image<'static> {
    Image::from_bytes(png(look.name)).expect("the tray icons are PNGs")
}

fn png(name: &str) -> &'static [u8] {
    match name {
        "idle-template" => include_bytes!("../icons/idle-template@2x.png"),
        "building-template" => include_bytes!("../icons/building-template@2x.png"),
        "idle-attention-gray" => include_bytes!("../icons/idle-attention-gray@2x.png"),
        "building-attention-gray" => include_bytes!("../icons/building-attention-gray@2x.png"),
        "idle" => include_bytes!("../icons/idle@2x.png"),
        "building" => include_bytes!("../icons/building@2x.png"),
        "idle-attention" => include_bytes!("../icons/idle-attention@2x.png"),
        "building-attention" => include_bytes!("../icons/building-attention@2x.png"),
        _ => panic!("there is no tray icon {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_show_whether_anything_is_building() {
        assert_eq!(
            look(true, false, false),
            Look {
                name: "idle-template",
                template: true
            }
        );
        assert_eq!(
            look(true, true, false),
            Look {
                name: "building-template",
                template: true
            }
        );
    }

    #[test]
    fn a_template_cant_be_red_so_attention_is_drawn_in_gray() {
        assert_eq!(
            look(true, false, true),
            Look {
                name: "idle-attention-gray",
                template: false
            }
        );
        assert_eq!(
            look(true, true, true),
            Look {
                name: "building-attention-gray",
                template: false
            }
        );
    }

    #[test]
    fn colored_icons_show_building_and_attention() {
        let names: Vec<_> = [(false, false), (true, false), (false, true), (true, true)]
            .into_iter()
            .map(|(building, attention)| look(false, building, attention))
            .inspect(|look| assert!(!look.template))
            .map(|look| look.name)
            .collect();
        assert_eq!(
            names,
            ["idle", "building", "idle-attention", "building-attention"]
        );
    }

    #[test]
    fn every_look_has_an_icon() {
        for template in [false, true] {
            for building in [false, true] {
                for attention in [false, true] {
                    let _ = png(look(template, building, attention).name);
                }
            }
        }
    }
}
