//! The app in the menu bar (macOS), notification area (Windows) or system
//! tray (Linux).
//!
//! The tray runs the same socket and builds as `--headless`, with Tauri's
//! event loop on the main thread. Its icon and menu are redrawn from
//! [`menu::menu`] every two seconds, and whenever how many builds are
//! running changes. Where there is no tray, the app runs headless and says
//! so.

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use epik_core::{record, run};
use tauri::image::Image;
use tauri::menu::{IsMenuItem, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, RunEvent, Wry};

use crate::App;
use crate::menu::{self, Item, Menu};
use crate::{main_window, setup_window};

/// How often the menu is redrawn from the run listing.
const REFRESH: Duration = Duration::from_secs(2);

const QUIT: &str = "quit";
const OPEN_RUNS_FOLDER: &str = "open-runs-folder";
const CHECK_SETUP: &str = "check-setup";
const OPEN_EPIK: &str = "open-epik";
/// The prefix of a run's item's id; the run, `owner/name#number`, follows
/// it.
const RUN: &str = "run:";

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

/// Starts Tauri with no window, ready to open runs' windows, and puts the
/// icon in the tray, or says why it can't.
fn open(app: &App) -> Result<(tauri::App, TrayIcon), String> {
    if !has_display() {
        return Err("no display".to_owned());
    }
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut tauri = tauri::Builder::default()
        .manage(main_window::Window {
            root: app.root.clone(),
            builds: app.builds.clone(),
        })
        .manage(setup_window::Setup {
            root: app.root.clone(),
            builds: app.builds.clone(),
            outputs: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            main_window::home,
            main_window::popover,
            main_window::run_page,
            main_window::rerun,
            main_window::delete_run,
            main_window::settings,
            main_window::monitor,
            main_window::stop_monitoring,
            main_window::open_github,
            main_window::open_log,
            setup_window::setup_checklist,
            setup_window::setup_run,
            setup_window::setup_output,
        ])
        .build(tauri::generate_context!())
        .map_err(|error| error.to_string())?;
    // No Dock icon.
    #[cfg(target_os = "macos")]
    tauri.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let shown = menu::menu(&[], &[]);
    let root = app.root.clone();
    let stop = app.stop.clone();
    let tray = TrayIconBuilder::new()
        .icon(icon(shown.building))
        .icon_as_template(TEMPLATE)
        .tooltip("Epik")
        .menu(&native(tauri.handle(), &shown).map_err(|error| error.to_string())?)
        .on_menu_event(move |handle, event| {
            let id = event.id().as_ref();
            if id == QUIT {
                stop.store(true, Ordering::SeqCst);
            } else if id == CHECK_SETUP {
                if let Err(error) = setup_window::open(handle) {
                    eprintln!("epik-app: couldn't open the setup checklist: {error}");
                }
            } else if id == OPEN_RUNS_FOLDER {
                reveal(&record::runs_dir(&root));
            } else if id == OPEN_EPIK {
                if let Err(error) = main_window::open(handle, "") {
                    eprintln!("epik-app: couldn't open Epik's window: {error}");
                }
            } else if let Some(run) = id.strip_prefix(RUN)
                && let Err(error) = main_window::open(handle, &main_window::run_route(run))
            {
                eprintln!("epik-app: couldn't open the page of run {run}: {error}");
            }
        })
        .build(&tauri)
        .map_err(|error| error.to_string())?;
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

/// Serves on one thread and redraws the tray on another while Tauri runs on
/// this one. When serving stops, the builds are stopped, then Tauri.
fn in_tray(app: App, tauri: tauri::App, tray: TrayIcon, changes: Receiver<()>) -> ExitCode {
    let handle = tauri.handle().clone();
    let root = app.root.clone();
    let builds = app.builds.clone();
    let stop = app.stop.clone();
    thread::spawn(move || {
        let mut shown = None;
        while !stop.load(Ordering::SeqCst) {
            let runs = run::list(&root).unwrap_or_default();
            let wanted = menu::menu(&runs, &builds.active());
            if shown.as_ref() != Some(&wanted) {
                if let Err(error) = redraw(&handle, &tray, &wanted) {
                    eprintln!("epik-app: couldn't redraw the tray: {error}");
                }
                shown = Some(wanted);
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
    tauri.run_return(|_, event| {
        // Closing the last window doesn't quit an app that lives in the
        // tray; only Quit does.
        if let RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
        }
    });
    server.join().unwrap_or(ExitCode::FAILURE)
}

fn redraw(handle: &AppHandle, tray: &TrayIcon, shown: &Menu) -> tauri::Result<()> {
    tray.set_menu(Some(native(handle, shown)?))?;
    tray.set_icon(Some(icon(shown.building)))?;
    tray.set_icon_as_template(TEMPLATE)
}

/// Whether the icons are templates, which the menu bar tints: on macOS.
const TEMPLATE: bool = cfg!(target_os = "macos");

/// The idle or building icon, at 2x, which the tray scales to its size. On
/// macOS a one-color template, hollow nodes when idle and solid when
/// building; elsewhere gray when idle and in the mark's colors when
/// building. `icons/make-tray-icons.py` draws them.
fn icon(building: bool) -> Image<'static> {
    let png: &[u8] = match (TEMPLATE, building) {
        (true, false) => include_bytes!("../icons/idle-template@2x.png"),
        (true, true) => include_bytes!("../icons/building-template@2x.png"),
        (false, false) => include_bytes!("../icons/idle@2x.png"),
        (false, true) => include_bytes!("../icons/building@2x.png"),
    };
    Image::from_bytes(png).expect("the tray icons are PNGs")
}

/// The menu Tauri draws for `shown`.
fn native(handle: &AppHandle, shown: &Menu) -> tauri::Result<tauri::menu::Menu<Wry>> {
    let items = shown
        .items
        .iter()
        .map(|item| -> tauri::Result<Box<dyn IsMenuItem<Wry>>> {
            Ok(match item {
                Item::Run { run, label } => Box::new(MenuItem::with_id(
                    handle,
                    format!("{RUN}{run}"),
                    label,
                    true,
                    None::<&str>,
                )?),
                Item::Note(text) => Box::new(MenuItem::new(handle, text, false, None::<&str>)?),
                Item::Separator => Box::new(PredefinedMenuItem::separator(handle)?),
                Item::OpenEpik => Box::new(MenuItem::with_id(
                    handle,
                    OPEN_EPIK,
                    "Open Epik",
                    true,
                    None::<&str>,
                )?),
                Item::CheckSetup => Box::new(MenuItem::with_id(
                    handle,
                    CHECK_SETUP,
                    "Check setup",
                    true,
                    None::<&str>,
                )?),
                Item::OpenRunsFolder => Box::new(MenuItem::with_id(
                    handle,
                    OPEN_RUNS_FOLDER,
                    "Open runs folder",
                    true,
                    None::<&str>,
                )?),
                Item::Quit => {
                    Box::new(MenuItem::with_id(handle, QUIT, "Quit", true, None::<&str>)?)
                }
            })
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let items: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|item| item.as_ref()).collect();
    tauri::menu::Menu::with_items(handle, &items)
}

/// Opens `dir` in the platform's file manager, creating it first so there
/// is something to open.
fn reveal(dir: &Path) {
    let opened = std::fs::create_dir_all(dir).and_then(|()| open::that_detached(dir));
    if let Err(error) = opened {
        eprintln!("epik-app: couldn't open {}: {error}", dir.display());
    }
}
