//! Updating Epik from its releases: a release build checks when it starts
//! and daily, and Check for updates reads "Update to x.y.z" once a newer
//! version is found. Choosing that asks, as Restart does, then installs it
//! and restarts into it. Builds that can't update themselves open the
//! releases page instead.

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_updater::{Update, UpdaterExt};

use epik_core::hosts;

use crate::{main_window, pages, popover, tray};

/// How often a running Epik checks for a newer version.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The newer version found, if any.
#[derive(Default)]
pub struct Found(Mutex<Option<Update>>);

impl Found {
    /// The newer version's number, if one is found.
    pub fn version(&self) -> Option<String> {
        self.update().map(|update| update.version)
    }

    fn update(&self) -> Option<Update> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Whether a build for `os` can update itself: a release build on macOS,
/// Windows, or Linux in an AppImage, which `appimage` says.
pub fn can_update(os: &str, appimage: bool, debug: bool) -> bool {
    !debug
        && match os {
            "macos" | "windows" => true,
            "linux" => appimage,
            _ => false,
        }
}

/// Whether this build can update itself.
fn can_update_here() -> bool {
    let appimage = std::env::var_os("APPIMAGE").is_some_and(|path| !path.is_empty());
    can_update(std::env::consts::OS, appimage, cfg!(debug_assertions))
}

/// What choosing Check for updates does.
#[derive(Debug, PartialEq, Eq)]
pub enum Choice<U> {
    /// Opens the releases page, for a build that can't update itself.
    OpenReleases,
    /// Checks for a newer version now.
    Check,
    /// Offers to install the newer version found.
    Install(U),
}

/// What choosing Check for updates does, in a build that `updatable` or
/// not, with the newer version `found`, if any.
pub fn choice<U>(updatable: bool, found: Option<U>) -> Choice<U> {
    match (updatable, found) {
        (false, _) => Choice::OpenReleases,
        (true, None) => Choice::Check,
        (true, Some(update)) => Choice::Install(update),
    }
}

/// Checks for a newer version now and daily after, if this build can
/// update itself. A failed check is only reported.
pub fn check_daily(handle: &AppHandle) {
    if !can_update_here() {
        return;
    }
    let handle = handle.clone();
    thread::spawn(move || {
        loop {
            if let Err(error) = tauri::async_runtime::block_on(check(&handle)) {
                eprintln!("epik-app: couldn't check for updates: {error}");
            }
            thread::sleep(CHECK_EVERY);
        }
    });
}

/// Checks for a newer version, and remembers the one found, if any.
async fn check(handle: &AppHandle) -> Result<Option<Update>, String> {
    let quit = handle.state::<popover::Quit>().inner().clone();
    let found = handle
        .updater_builder()
        // On Windows the installer ends the app, so the builds stop first.
        .on_before_exit(move || quit.stop_for_installer())
        .build()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?;
    if let Some(update) = &found {
        *handle
            .state::<Found>()
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(update.clone());
        tray::show_update(handle, &update.version);
    }
    Ok(found)
}

/// Does what choosing Check for updates does, with `building` runs here
/// that updating interrupts.
pub fn choose(handle: &AppHandle, building: usize) {
    match choice(can_update_here(), handle.state::<Found>().update()) {
        Choice::OpenReleases => open_releases(),
        Choice::Install(update) => offer(handle, update, building),
        Choice::Check => {
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                match check(&handle).await {
                    Ok(Some(update)) => offer(&handle, update, building),
                    Ok(None) => tell(
                        &handle,
                        pages::UP_TO_DATE,
                        pages::up_to_date(epik_core::version()),
                        MessageDialogKind::Info,
                    ),
                    Err(error) => {
                        eprintln!("epik-app: couldn't check for updates: {error}");
                        tell(
                            &handle,
                            pages::COULDNT_CHECK,
                            pages::couldnt_check(&error),
                            MessageDialogKind::Error,
                        );
                        open_releases();
                    }
                }
            });
        }
    }
}

/// Asks before installing `update`, as Restart does, warning too of the
/// hosts it cuts off; then installs it and restarts into it, or says why
/// it couldn't. If the hosts can't be read, none is warned of.
fn offer(handle: &AppHandle, update: Update, building: usize) {
    let question = pages::update_question(&update.version, building);
    let hosts = hosts::list(&handle.state::<main_window::Window>().root).unwrap_or_default();
    let warning = pages::update_warning(&update.version, building, &hosts);
    popover::ask_to_restart(handle, question, warning, "Update", move |handle| {
        tauri::async_runtime::spawn(async move {
            match update.download_and_install(|_, _| {}, || {}).await {
                Ok(()) => handle.state::<popover::Quit>().restart(),
                Err(error) => {
                    eprintln!("epik-app: couldn't update: {error}");
                    tell(
                        &handle,
                        pages::COULDNT_UPDATE,
                        pages::couldnt_update(&error.to_string()),
                        MessageDialogKind::Error,
                    );
                }
            }
        });
    });
}

fn tell(handle: &AppHandle, title: &str, message: String, kind: MessageDialogKind) {
    handle
        .dialog()
        .message(message)
        .title(title)
        .kind(kind)
        .show(|_| {});
}

fn open_releases() {
    if let Err(error) = open::that_detached(popover::RELEASES) {
        eprintln!("epik-app: couldn't open the releases page: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_and_windows_release_builds_can_update() {
        assert!(can_update("macos", false, false));
        assert!(can_update("windows", false, false));
    }

    #[test]
    fn on_linux_only_an_appimage_can_update() {
        assert!(can_update("linux", true, false));
        assert!(!can_update("linux", false, false));
    }

    #[test]
    fn a_debug_build_cant_update() {
        assert!(!can_update("macos", false, true));
        assert!(!can_update("linux", true, true));
    }

    #[test]
    fn a_build_that_cant_update_opens_the_releases_page() {
        assert_eq!(choice(false, None::<&str>), Choice::OpenReleases);
        assert_eq!(choice(false, Some("0.4.0")), Choice::OpenReleases);
    }

    #[test]
    fn with_no_update_known_choosing_it_checks_now() {
        assert_eq!(choice(true, None::<&str>), Choice::Check);
    }

    #[test]
    fn with_an_update_known_choosing_it_offers_to_install_it() {
        assert_eq!(choice(true, Some("0.4.0")), Choice::Install("0.4.0"));
    }
}
