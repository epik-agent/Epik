//! The Epik app's main window: Home, a run's page, and Settings.
//!
//! The window, drawn by `epik-ui` at `index.html#/`, asks these commands
//! for what to show every second, and draws it. The window notifies; its only controls are Rerun
//! and Delete on a run that needs attention, and Settings. Every command
//! goes through `epik-core`; nothing here touches `~/.epik` itself.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Local, Utc};
use epik_core::builds::Builds;
use epik_core::record::{self, IssueStatus};
use epik_core::repositories;
use epik_core::run::{self, RunKey};
use epik_view::{Home, Popover, RunPage, Settings};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::pages;

/// What the window's commands reach.
pub struct Window {
    pub root: PathBuf,
    pub builds: Arc<Builds>,
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
    let runs = repositories::monitored_runs(&window.root).map_err(failed)?;
    let monitored = repositories::list(&window.root).map_err(failed)?;
    Ok(pages::home(&runs, &monitored, Utc::now()))
}

#[tauri::command]
pub fn popover(window: State<'_, Window>) -> Result<Popover, String> {
    let runs = repositories::monitored_runs(&window.root).map_err(failed)?;
    Ok(pages::popover(&runs, Utc::now()))
}

#[tauri::command]
pub fn run_page(run: String, window: State<'_, Window>) -> Result<RunPage, String> {
    let key = key(&run)?;
    let state = run::read(&window.root, &key).map_err(failed)?;
    let others = repositories::monitored_runs(&window.root).map_err(failed)?;
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
    off_main(window, move |builds| builds.rerun(&key).map(drop)).await
}

#[tauri::command]
pub async fn delete_run(run: String, window: State<'_, Window>) -> Result<(), String> {
    let key = key(&run)?;
    off_main(window, move |builds| builds.delete(&key)).await
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
    repositories::add(&window.root, repo.trim_end_matches('/'), true, Utc::now())
        .map(drop)
        .map_err(failed)
}

#[tauri::command]
pub fn stop_monitoring(repo: String, window: State<'_, Window>) -> Result<(), String> {
    repositories::remove(&window.root, &repo).map_err(failed)
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

/// Opens an agent's full log, which must be one of Epik's records.
#[tauri::command]
pub fn open_log(path: String, window: State<'_, Window>) -> Result<(), String> {
    let path = PathBuf::from(path);
    let runs = record::runs_dir(&window.root);
    let inside = path
        .canonicalize()
        .is_ok_and(|path| runs.canonicalize().is_ok_and(|runs| path.starts_with(runs)));
    if !inside {
        return Err(format!("{} isn't an agent's log", path.display()));
    }
    open::that_detached(&path).map_err(failed)
}

/// Opens the main window at `route` (`""` for Home, `run/<owner/name#n>`
/// for a run's page), or brings it forward there.
pub fn open(handle: &AppHandle, route: &str) -> tauri::Result<()> {
    // A run's `#` would end the URL's fragment.
    let hash = format!("#/{}", route.replace('#', "%23"));
    if let Some(window) = handle.get_webview_window(LABEL) {
        let script = format!(
            "window.location.hash = {};",
            serde_json::to_string(&hash).expect("a string serializes")
        );
        window.eval(&script)?;
        window.unminimize()?;
        window.show()?;
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
    .build()?;
    Ok(())
}
