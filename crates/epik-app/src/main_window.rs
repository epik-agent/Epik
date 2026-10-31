//! The Epik app's main window: its sidebar, Home, a run's page, and
//! Settings.
//!
//! The window, drawn by `epik-ui` at `index.html#/`, asks these commands
//! for what to show every second, and draws it. The window notifies; its only controls are Rerun
//! and Delete on a run that needs attention, and Settings, which can also
//! uninstall Epik. Every command
//! goes through `epik-core`; nothing here touches `~/.epik` itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Local, Utc};
use epik_core::builds::Builds;
use epik_core::record::{self, IssueStatus};
use epik_core::repositories;
use epik_core::run::{self, RunKey};
use epik_core::{setup, uninstall};
use epik_view::{Home, Popover, RunPage, Settings, Sidebar, Stream, Uninstall, UninstallStep};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::runs::Runs;
use crate::{navigation, pages};

/// What the window's commands reach.
pub struct Window {
    pub root: PathBuf,
    pub builds: Arc<Builds>,
    /// The runs of the monitored repositories, which the tray reads too.
    pub runs: Arc<Runs>,
    /// When this Epik started, for the sidebar's status line and the tray
    /// menu.
    pub started: DateTime<Utc>,
}

/// The main window's label.
pub const LABEL: &str = "main";

fn key(run: &str) -> Result<RunKey, String> {
    run.parse()
}

fn failed(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
pub fn home(window: State<'_, Window>) -> Result<Home, String> {
    let runs = window.runs.monitored().map_err(failed)?;
    Ok(pages::home(&runs, Utc::now()))
}

#[tauri::command]
pub fn sidebar(window: State<'_, Window>) -> Result<Sidebar, String> {
    let runs = window.runs.monitored().map_err(failed)?;
    let monitored = repositories::list(&window.root).map_err(failed)?;
    Ok(pages::sidebar(
        &runs,
        &monitored,
        window.started,
        Utc::now(),
    ))
}

#[tauri::command]
pub fn popover(window: State<'_, Window>) -> Result<Popover, String> {
    let runs = window.runs.monitored().map_err(failed)?;
    Ok(pages::popover(&runs, window.started, Utc::now()))
}

#[tauri::command]
pub fn run_page(run: String, window: State<'_, Window>) -> Result<RunPage, String> {
    let key = key(&run)?;
    let state = run::read(&window.root, &key).map_err(failed)?;
    let others = window.runs.monitored().map_err(failed)?;
    let builds = record::list_builds(&window.root, &key).map_err(failed)?;
    let merged_by_build: Vec<Vec<u64>> = builds
        .iter()
        .map(|build| {
            build
                .issues
                .iter()
                .filter(|issue| issue.status == IssueStatus::Merged)
                .map(|issue| issue.issue.number)
                .collect()
        })
        .collect();
    let log_dir = state
        .running_build
        .as_deref()
        .map(|build| record::build_dir(&window.root, &key, build));
    Ok(pages::run_page(
        &state,
        &others,
        &merged_by_build,
        log_dir.as_deref(),
        Utc::now(),
        Local::now().offset(),
    ))
}

/// Runs `work` on the builds away from the main thread, which draws every
/// window: a rerun waits for its clone.
async fn off_main(
    window: State<'_, Window>,
    work: impl FnOnce(&Builds) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let builds = window.builds.clone();
    tauri::async_runtime::spawn_blocking(move || work(&builds))
        .await
        .map_err(failed)?
}

#[tauri::command]
pub async fn rerun(run: String, window: State<'_, Window>) -> Result<(), String> {
    let key = key(&run)?;
    let runs = window.runs.clone();
    let rerun = off_main(window, move |builds| builds.rerun(&key).map(drop)).await;
    runs.forget();
    rerun
}

#[tauri::command]
pub async fn delete_run(run: String, window: State<'_, Window>) -> Result<(), String> {
    let key = key(&run)?;
    let runs = window.runs.clone();
    let deleted = off_main(window, move |builds| builds.delete(&key)).await;
    runs.forget();
    deleted
}

#[tauri::command]
pub fn settings(window: State<'_, Window>) -> Result<Settings, String> {
    let monitored = repositories::list(&window.root).map_err(failed)?;
    let runs = run::list(&window.root).map_err(failed)?;
    Ok(pages::settings(&monitored, &runs, Local::now().offset()))
}

#[tauri::command]
pub fn monitor(repo: String, window: State<'_, Window>) -> Result<(), String> {
    let repo = repo.trim();
    let repo = repo.strip_prefix("https://github.com/").unwrap_or(repo);
    let added = repositories::add(&window.root, repo.trim_end_matches('/'), true, Utc::now());
    window.runs.forget();
    added.map(drop).map_err(failed)
}

#[tauri::command]
pub fn stop_monitoring(repo: String, window: State<'_, Window>) -> Result<(), String> {
    let removed = repositories::remove(&window.root, &repo);
    window.runs.forget();
    removed.map_err(failed)
}

/// Why Epik can't be uninstalled while a build runs.
const BUILDING: &str = "Wait for the running build to finish, or stop it first.";

/// What the confirmation to uninstall Epik says: how many runs' history
/// goes, and whether a build is running.
#[tauri::command]
pub fn uninstall_plan(window: State<'_, Window>) -> Result<Uninstall, String> {
    Ok(Uninstall {
        runs: run::list(&window.root).map_err(failed)?.len(),
        building: !window.builds.active().is_empty(),
    })
}

/// Removes what Epik put on the machine, unless a build is running, and
/// says how each step went. The app keeps running until the user quits it.
#[tauri::command]
pub async fn uninstall(window: State<'_, Window>) -> Result<Vec<UninstallStep>, String> {
    if !window.builds.active().is_empty() {
        return Err(BUILDING.to_owned());
    }
    let root = window.root.clone();
    let outcomes =
        tauri::async_runtime::spawn_blocking(move || uninstall::uninstall(&root, &setup::System))
            .await
            .map_err(failed)?;
    Ok(pages::uninstall_steps(&outcomes))
}

/// Opens a GitHub page in the browser. Only GitHub's pages: the window
/// links nowhere else.
#[tauri::command]
pub fn open_github(url: String) -> Result<(), String> {
    if !url.starts_with("https://github.com/") {
        return Err(format!("{url} isn't a GitHub page"));
    }
    open::that_detached(&url).map_err(failed)
}

/// The stream in the agent log at `path`, which must be one of Epik's
/// records. A log not written yet has nothing in it.
fn stream(root: &Path, path: &str) -> Result<Stream, String> {
    let path = Path::new(path);
    let refused = || format!("{} isn't an agent's log", path.display());
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(refused());
    };
    let runs = record::runs_dir(root)
        .canonicalize()
        .map_err(|_| refused())?;
    let dir = dir.canonicalize().map_err(|_| refused())?;
    if !dir.starts_with(runs) {
        return Err(refused());
    }
    match epik_core::stream::read(&dir.join(name)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Stream::default()),
        read => read.map_err(failed),
    }
}

/// What the agent whose log is at `path` has said and done, for the run
/// page's stream.
#[tauri::command]
pub fn agent_stream(path: String, window: State<'_, Window>) -> Result<Stream, String> {
    stream(&window.root, &path)
}

/// A window opening or closing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Opened,
    Closed,
}

/// Whether Epik belongs in the Dock and Cmd-Tab after the window `label`
/// opens or closes, or `None` if that window leaves it as it was. Epik is
/// a regular app while its main window is open, and otherwise lives only
/// in the menu bar.
pub fn in_dock_after(label: &str, change: Change) -> Option<bool> {
    (label == LABEL).then_some(change == Change::Opened)
}

/// Puts Epik in the Dock and Cmd-Tab, or takes it out, as the window
/// `label` opening or closing calls for. Only macOS has a Dock to leave.
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
pub fn follow(handle: &AppHandle, label: &str, change: Change) {
    let in_dock = in_dock_after(label, change);
    #[cfg(target_os = "macos")]
    if let Some(in_dock) = in_dock {
        let policy = if in_dock {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        };
        if let Err(error) = handle.set_activation_policy(policy) {
            eprintln!("epik-app: couldn't change Epik's place in the Dock: {error}");
        }
    }
}

/// Opens the main window at `route` (`""` for Home, `run/<owner/name#n>`
/// for a run's page, `settings` for Settings), or brings it forward there,
/// and puts Epik in the Dock while it is open.
pub fn open(handle: &AppHandle, route: &str) -> tauri::Result<()> {
    // A run's `#` would end the URL's fragment.
    let hash = format!("#/{}", route.replace('#', "%23"));
    // Into the Dock only once there is a window to show there.
    if let Some(window) = handle.get_webview_window(LABEL) {
        let script = format!(
            "window.location.hash = {};",
            serde_json::to_string(&hash).expect("a string serializes")
        );
        window.eval(&script)?;
        window.unminimize()?;
        window.show()?;
        follow(handle, LABEL, Change::Opened);
        return window.set_focus();
    }
    WebviewWindowBuilder::new(
        handle,
        LABEL,
        WebviewUrl::App(format!("index.html{hash}").into()),
    )
    .title("Epik")
    .inner_size(1120.0, 780.0)
    .min_inner_size(720.0, 480.0)
    .on_navigation(navigation::stay_in_app)
    .build()?;
    follow(handle, LABEL, Change::Opened);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const LINE: &str = r#"{"at":"2026-10-30T18:11:41Z","from_agent":{"method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello."}}}}}"#;

    #[test]
    fn the_stream_is_read_from_a_log_in_the_records() {
        let root = tempfile::tempdir().unwrap();
        let build = root.path().join("runs/acme/widgets/13/b1");
        fs::create_dir_all(&build).unwrap();
        let log = build.join("2.jsonl");
        fs::write(&log, format!("{LINE}\n")).unwrap();
        let stream = stream(root.path(), log.to_str().unwrap()).unwrap();
        assert_eq!(stream, epik_core::stream::parse(LINE));
        assert_eq!(stream.entries.len(), 1);
    }

    #[test]
    fn a_log_not_written_yet_has_nothing_in_it() {
        let root = tempfile::tempdir().unwrap();
        let build = root.path().join("runs/acme/widgets/13/b1");
        fs::create_dir_all(&build).unwrap();
        let log = build.join("2.jsonl");
        assert_eq!(
            stream(root.path(), log.to_str().unwrap()),
            Ok(Stream::default())
        );
    }

    #[test]
    fn a_path_outside_the_records_is_refused() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("runs")).unwrap();
        let elsewhere = root.path().join("secret.jsonl");
        fs::write(&elsewhere, format!("{LINE}\n")).unwrap();
        let sneaky = root.path().join("runs/../secret.jsonl");
        for path in [elsewhere, sneaky] {
            let refused = stream(root.path(), path.to_str().unwrap());
            assert!(refused.unwrap_err().ends_with("isn't an agent's log"));
        }
        assert!(stream(root.path(), "").is_err());
    }

    #[test]
    fn epik_is_in_the_dock_while_the_main_window_is_open() {
        assert_eq!(in_dock_after(LABEL, Change::Opened), Some(true));
        assert_eq!(in_dock_after(LABEL, Change::Closed), Some(false));
    }

    #[test]
    fn the_other_windows_leave_the_dock_alone() {
        for label in [crate::popover::LABEL, crate::setup_window::LABEL] {
            assert_eq!(in_dock_after(label, Change::Opened), None);
            assert_eq!(in_dock_after(label, Change::Closed), None);
        }
    }
}
