//! The Epik app's main window: its sidebar, Home, a run's page, and
//! Settings.
//!
//! The window, drawn by `epik-ui` at `index.html#/`, asks these commands
//! for what to show every second, and draws it. The window notifies; its only controls are Rerun
//! and Delete on a run that needs attention, and Settings, which also adds,
//! checks and removes hosts, and can uninstall Epik. Every command
//! goes through `epik-core`; nothing here touches `~/.epik` itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Local, Utc};
use epik_core::builds::Builds;
use epik_core::hosted::{HostedKey, HostedRun};
use epik_core::hosts;
use epik_core::record::{self, IssueStatus};
use epik_core::repositories;
use epik_core::run;
use epik_core::{settings, setup, uninstall};
use epik_view::{
    AgentSettings, Home, Popover, Removal, RunPage, Settings, Sidebar, Stream, Uninstall,
    UninstallStep,
};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::popover::Quit;
use crate::runs::Runs;
use crate::{navigation, pages};

/// What the window's commands reach.
pub struct Window {
    pub root: PathBuf,
    pub builds: Arc<Builds>,
    /// The runs of the monitored repositories, on every host, which the
    /// tray reads too.
    pub runs: Arc<Runs>,
    /// When this Epik started, for the sidebar's status line and the tray
    /// menu.
    pub started: DateTime<Utc>,
}

/// The main window's label.
pub const LABEL: &str = "main";

fn key(run: &str) -> Result<HostedKey, String> {
    run.parse()
}

fn failed(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
pub fn home(window: State<'_, Window>) -> Result<Home, String> {
    let shown = window.runs.monitored().map_err(failed)?;
    Ok(pages::home(&shown, Utc::now()))
}

#[tauri::command]
pub fn sidebar(window: State<'_, Window>) -> Result<Sidebar, String> {
    let shown = window.runs.monitored().map_err(failed)?;
    let monitored = repositories::list(&window.root).map_err(failed)?;
    Ok(pages::sidebar(
        &shown,
        &monitored,
        window.started,
        Utc::now(),
    ))
}

#[tauri::command]
pub fn popover(window: State<'_, Window>) -> Result<Popover, String> {
    let shown = window.runs.monitored().map_err(failed)?;
    Ok(pages::popover(&shown, window.started, Utc::now()))
}

#[tauri::command]
pub fn run_page(run: String, window: State<'_, Window>) -> Result<RunPage, String> {
    let named = key(&run)?;
    let shown = window.runs.monitored().map_err(failed)?;
    let host = match &named.host {
        Some(destination) => Some(
            shown
                .hosts
                .iter()
                .find(|host| host.destination == *destination)
                .cloned()
                .ok_or_else(|| format!("{destination} isn't a host"))?,
        ),
        None => None,
    };
    let root = named.root(&window.root);
    let key = named.run;
    let state = run::read(&root, &key).map_err(failed)?;
    let builds = record::list_builds(&root, &key).map_err(failed)?;
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
    let run_dir = run::run_dir(&root, &key);
    Ok(pages::run_page(
        &HostedRun { host, state },
        &shown,
        &merged_by_build,
        Some(&run_dir),
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
    let runs = window.runs.clone();
    let rerun = off_main(window, move |builds| rerun_named(builds, &run)).await;
    runs.forget();
    rerun
}

#[tauri::command]
pub async fn delete_run(run: String, window: State<'_, Window>) -> Result<(), String> {
    let runs = window.runs.clone();
    let deleted = off_main(window, move |builds| delete_named(builds, &run)).await;
    runs.forget();
    deleted
}

/// Reruns the run named `run` on the host it builds on: a remote host's
/// run is rerun there.
fn rerun_named(builds: &Builds, run: &str) -> Result<(), String> {
    let HostedKey { host, run } = key(run)?;
    builds.rerun_on(host.as_deref(), &run).map(drop)
}

/// Deletes the run named `run` on the host it builds on: a remote host's
/// run is deleted there, and goes from its mirror here.
fn delete_named(builds: &Builds, run: &str) -> Result<(), String> {
    let HostedKey { host, run } = key(run)?;
    builds.delete_on(host.as_deref(), &run)
}

#[tauri::command]
pub fn settings(window: State<'_, Window>) -> Result<Settings, String> {
    let monitored = repositories::list(&window.root).map_err(failed)?;
    let runs = run::list(&window.root).map_err(failed)?;
    let hosts = hosts::list(&window.root)
        .map_err(failed)?
        .into_iter()
        .map(|host| {
            let mirrored = run::list(&hosts::dir(&window.root, &host.destination))?;
            Ok((host, mirrored))
        })
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(failed)?;
    let default = hosts::default(&window.root).map_err(failed)?;
    Ok(pages::settings(
        &monitored,
        &runs,
        &hosts,
        default.as_deref(),
        Local::now().offset(),
    ))
}

/// Adds the host `destination`, an SSH destination, and checks it at once.
/// A host that couldn't be checked is still added: Settings says why.
#[tauri::command]
pub async fn add_host(destination: String, window: State<'_, Window>) -> Result<(), String> {
    let destination = destination.trim().to_owned();
    hosts::add(&window.root, &destination, Utc::now()).map_err(failed)?;
    off_main(window, move |builds| {
        if let Err(error) = builds.check_host(&destination) {
            eprintln!("epik-app: couldn't check {destination}: {error}");
        }
        Ok(())
    })
    .await
}

/// Checks the host `destination` again: whether it answers, and its setup.
#[tauri::command]
pub async fn check_host(destination: String, window: State<'_, Window>) -> Result<(), String> {
    off_main(window, move |builds| {
        if let Err(error) = builds.check_host(&destination) {
            eprintln!("epik-app: couldn't check {destination}: {error}");
        }
        Ok(())
    })
    .await
}

/// Removes the host `destination`, and its mirror.
#[tauri::command]
pub fn remove_host(destination: String, window: State<'_, Window>) -> Result<(), String> {
    let removed = hosts::remove(&window.root, &destination);
    window.runs.forget();
    removed.map_err(failed)
}

/// Makes the host `destination` the default for builds, or this computer
/// for none.
#[tauri::command]
pub fn set_default_host(
    destination: Option<String>,
    window: State<'_, Window>,
) -> Result<(), String> {
    hosts::set_default(&window.root, destination.as_deref()).map_err(failed)
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

/// The agent and model each role runs with.
#[tauri::command]
pub fn agent_settings(window: State<'_, Window>) -> Result<AgentSettings, String> {
    settings::read(&window.root).map_err(failed)
}

/// Keeps the agent and model each role runs with, for the builds that
/// start after.
#[tauri::command]
pub fn set_agent_settings(
    settings: AgentSettings,
    window: State<'_, Window>,
) -> Result<(), String> {
    settings::write(&window.root, &settings).map_err(failed)
}

/// The ids of the agents whose setup isn't done. Checking runs commands,
/// so not on the main thread.
#[tauri::command]
pub async fn unready_agents(window: State<'_, Window>) -> Result<Vec<String>, String> {
    let builds = window.builds.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Every item is checked whatever the roles, so any roles will do.
        let checks = builds.setup(Some(&AgentSettings::default()))?;
        Ok(setup::unready_agents(&checks)
            .into_iter()
            .map(str::to_owned)
            .collect())
    })
    .await
    .map_err(failed)?
}

/// Why Epik can't be uninstalled while a build runs.
const BUILDING: &str = "Wait for the running build to finish, or stop it first.";

/// What the confirmation to uninstall Epik says: how many runs' history
/// goes, whether a build is running, and how the user removes the app.
#[tauri::command]
pub fn uninstall_plan(window: State<'_, Window>) -> Result<Uninstall, String> {
    Ok(Uninstall {
        runs: run::list(&window.root).map_err(failed)?.len(),
        building: !window.builds.active().is_empty(),
        removal: this_removal()?,
    })
}

/// How the user removes this app, which runs from `exe` on `os`, from the
/// AppImage at `appimage` if `APPIMAGE` names one. Each platform's
/// installer owns the app, so the platform removes it, not Epik.
fn removal(os: &str, exe: &Path, appimage: Option<PathBuf>) -> Removal {
    match os {
        "macos" => {
            let bundle = exe
                .ancestors()
                .find(|path| path.extension().is_some_and(|extension| extension == "app"));
            Removal::MacApp(bundle.unwrap_or(exe).to_owned())
        }
        "windows" => Removal::WindowsApp,
        _ => match appimage {
            Some(appimage) if !appimage.as_os_str().is_empty() => Removal::AppImage(appimage),
            _ => Removal::Deb,
        },
    }
}

/// How the user removes the running app.
fn this_removal() -> Result<Removal, String> {
    let exe = std::env::current_exe().map_err(failed)?;
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    Ok(removal(std::env::consts::OS, &exe, appimage))
}

/// Takes the user to where they remove the app, revealing it or opening
/// Installed Apps, then quits through the tray's Quit. It quits even if
/// it couldn't take them there.
#[tauri::command]
pub fn quit_to_remove(quit: State<'_, Quit>) {
    let opened = this_removal().and_then(|removal| {
        match removal {
            Removal::MacApp(path) | Removal::AppImage(path) => {
                tauri_plugin_opener::reveal_item_in_dir(path)
            }
            Removal::WindowsApp => {
                tauri_plugin_opener::open_url("ms-settings:appsfeatures", None::<&str>)
            }
            Removal::Deb => Ok(()),
        }
        .map_err(failed)
    });
    if let Err(error) = opened {
        eprintln!("epik-app: couldn't show where to remove Epik: {error}");
    }
    quit.quit();
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
/// records, this computer's or a remote host's mirror's. A log not written
/// yet has nothing in it.
fn stream(root: &Path, path: &str) -> Result<Stream, String> {
    let path = Path::new(path);
    let refused = || format!("{} isn't an agent's log", path.display());
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(refused());
    };
    let dir = dir.canonicalize().map_err(|_| refused())?;
    let mirrors = hosts::list(root)
        .map_err(failed)?
        .into_iter()
        .map(|host| hosts::dir(root, &host.destination));
    let within = std::iter::once(root.to_owned())
        .chain(mirrors)
        .filter_map(|root| record::runs_dir(&root).canonicalize().ok())
        .any(|runs| dir.starts_with(runs));
    if !within {
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
/// for a run's page on this computer and `run/<host>:<owner/name#n>` for
/// one on a remote host, `settings` for Settings), or brings it forward
/// there, and puts Epik in the Dock while it is open.
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

    use epik_core::builds::Started;
    use epik_core::ipc::{Request, Response};
    use epik_core::mirrors::Mirrors;
    use epik_core::record::Kind;
    use epik_core::remote::Client;
    use epik_core::run::RunKey;

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
    fn the_stream_is_read_from_a_log_in_a_hosts_mirror() {
        let root = tempfile::tempdir().unwrap();
        hosts::add(root.path(), "devbox", Utc::now()).unwrap();
        let build = hosts::dir(root.path(), "devbox").join("runs/acme/widgets/13/b1");
        fs::create_dir_all(&build).unwrap();
        let log = build.join("2.jsonl");
        fs::write(&log, format!("{LINE}\n")).unwrap();

        let stream = stream(root.path(), log.to_str().unwrap()).unwrap();

        assert_eq!(stream.entries.len(), 1);
    }

    #[test]
    fn a_path_in_a_mirror_outside_its_records_is_refused() {
        let root = tempfile::tempdir().unwrap();
        hosts::add(root.path(), "devbox", Utc::now()).unwrap();
        let mirror = hosts::dir(root.path(), "devbox");
        fs::create_dir_all(mirror.join("runs")).unwrap();
        let stray = mirror.join("secret.jsonl");
        fs::write(&stray, format!("{LINE}\n")).unwrap();
        let gone = root.path().join("hosts/gone/runs/acme/widgets/13/b1");
        fs::create_dir_all(&gone).unwrap();
        let unlisted = gone.join("2.jsonl");
        fs::write(&unlisted, format!("{LINE}\n")).unwrap();

        for path in [stray, unlisted] {
            let refused = stream(root.path(), path.to_str().unwrap());
            assert!(refused.unwrap_err().ends_with("isn't an agent's log"));
        }
    }

    /// A host's app that keeps what it is asked, starts every rerun and
    /// deletes every run.
    #[derive(Default)]
    struct Remote {
        asked: std::sync::Mutex<Vec<Request>>,
    }

    impl Client for Remote {
        fn request(&self, request: &Request) -> std::io::Result<Response> {
            self.asked.lock().unwrap().push(request.clone());
            Ok(match request {
                Request::Rerun { run, .. } => Response::Started(Started {
                    run: run.clone(),
                    build_id: "20261101T120000Z-abcd".to_owned(),
                    kind: Kind::Issue,
                    branch: "main".to_owned(),
                    build_dir: "/home/me/.epik/runs/acme/widgets/13/b".into(),
                    host: None,
                }),
                Request::Delete { run, .. } => Response::Deleted { run: run.clone() },
                _ => Response::RunKeys { runs: Vec::new() },
            })
        }
    }

    #[test]
    fn a_hosts_run_is_rerun_and_deleted_on_its_host() {
        let root = tempfile::tempdir().unwrap();
        hosts::add(root.path(), "devbox", Utc::now()).unwrap();
        let remote = Arc::new(Remote::default());
        let mirrors = Mirrors::new(root.path(), {
            let remote = remote.clone();
            move |_| remote.clone()
        });
        let builds = Builds::real(root.path()).reaching(Arc::new(mirrors));

        rerun_named(&builds, "devbox:acme/widgets#13").unwrap();
        delete_named(&builds, "devbox:acme/widgets#13").unwrap();

        let run = RunKey::new("acme/widgets", 13);
        assert_eq!(
            *remote.asked.lock().unwrap(),
            [
                Request::Rerun {
                    run: run.clone(),
                    host: None
                },
                Request::Delete { run, host: None },
            ]
        );
        assert!(builds.active().is_empty(), "nothing builds here");
    }

    #[test]
    fn a_run_on_this_computer_is_rerun_and_deleted_here() {
        let root = tempfile::tempdir().unwrap();
        let builds = Builds::real(root.path());

        let rerun = rerun_named(&builds, "acme/widgets#13").unwrap_err();
        let deleted = delete_named(&builds, "acme/widgets#13").unwrap_err();

        assert_eq!(rerun, "no run acme/widgets#13");
        assert_eq!(deleted, "no run acme/widgets#13");
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
    fn on_macos_the_app_is_the_bundle_around_the_executable() {
        let exe = Path::new("/Applications/Epik.app/Contents/MacOS/epik-app");
        assert_eq!(
            removal("macos", exe, None),
            Removal::MacApp("/Applications/Epik.app".into())
        );
    }

    #[test]
    fn on_macos_an_executable_outside_a_bundle_is_the_app() {
        let exe = Path::new("/Users/me/Epik/target/debug/epik-app");
        assert_eq!(removal("macos", exe, None), Removal::MacApp(exe.into()));
    }

    #[test]
    fn on_windows_the_app_was_installed_with_the_msi() {
        let exe = Path::new(r"C:\Program Files\Epik\epik-app.exe");
        assert_eq!(removal("windows", exe, None), Removal::WindowsApp);
    }

    #[test]
    fn on_linux_the_app_is_the_appimage_when_run_from_one() {
        let exe = Path::new("/tmp/.mount_EpikXyz/usr/bin/epik-app");
        let appimage = PathBuf::from("/home/me/Apps/Epik.AppImage");
        assert_eq!(
            removal("linux", exe, Some(appimage.clone())),
            Removal::AppImage(appimage)
        );
    }

    #[test]
    fn on_linux_the_app_was_otherwise_installed_with_the_deb() {
        let exe = Path::new("/usr/bin/epik-app");
        assert_eq!(removal("linux", exe, None), Removal::Deb);
        assert_eq!(
            removal("linux", exe, Some(PathBuf::new())),
            Removal::Deb,
            "an empty APPIMAGE"
        );
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
