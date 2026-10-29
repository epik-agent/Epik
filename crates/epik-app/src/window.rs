//! A run's window: the run's issues as a dependency graph with their live
//! states.
//!
//! There is one window per run, opened from the tray; choosing the run
//! again focuses it. The page in `ui/` asks [`run_view`] for the run's
//! [`View`] every second and draws it. The window notifies and never asks:
//! nothing in it changes anything.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use epik_core::builds::Builds;
use epik_core::record;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::view::{self, View};

/// What the run windows' command reads.
pub struct Runs {
    pub root: PathBuf,
    pub builds: Arc<Builds>,
}

/// The run's view, as the page asks for it.
#[tauri::command]
pub fn run_view(run_id: String, runs: State<'_, Runs>) -> Result<View, String> {
    let active = runs.builds.active().contains(&run_id);
    load(&runs.root, &run_id, active).map_err(|error| error.to_string())
}

/// The view of the run `run_id` under `root` now, while it is `active` in
/// this app or not.
fn load(root: &Path, run_id: &str, active: bool) -> io::Result<View> {
    let events = record::read_events(root, run_id)?;
    let run = record::fold(&events).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("run {run_id} doesn't start with run_started"),
        )
    })?;
    Ok(view::view(&run, active, Utc::now()))
}

/// Opens the run's window, or focuses it if it is open.
pub fn open(handle: &AppHandle, run_id: &str) -> tauri::Result<()> {
    let label = label(run_id);
    if let Some(window) = handle.get_webview_window(&label) {
        window.unminimize()?;
        window.show()?;
        return window.set_focus();
    }
    let id = serde_json::to_string(run_id).expect("a string serializes");
    WebviewWindowBuilder::new(handle, label, WebviewUrl::App("index.html".into()))
        .title(run_id)
        .inner_size(960.0, 600.0)
        .initialization_script(format!("window.EPIK_RUN_ID = {id};"))
        .build()?;
    Ok(())
}

/// The run's window's label. Labels may hold only ASCII letters, digits,
/// `-`, `/`, `:` and `_`, so every other character, and `_` itself, is
/// written `_` and its code in hex, keeping each run's label its own.
fn label(run_id: &str) -> String {
    let mut label = String::from("run-");
    for c in run_id.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            label.push(c);
        } else {
            label.push_str(&format!("_{:x}_", u32::from(c)));
        }
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone};
    use epik_core::record::{Event, Kind, Member, RunStatus, RunWriter};

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap()
    }

    fn label_is_valid(label: &str) -> bool {
        label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | ':' | '_'))
    }

    #[test]
    fn a_label_keeps_a_plain_run_id() {
        assert_eq!(
            label("epik-agent-Epik-356-20261028T212859Z-k3f9"),
            "run-epik-agent-Epik-356-20261028T212859Z-k3f9"
        );
    }

    #[test]
    fn a_label_escapes_what_labels_cant_hold_and_keeps_ids_apart() {
        let labels = [label("a.b-1"), label("a_b-1"), label("a_2e_b-1")];

        assert!(
            labels.iter().all(|label| label_is_valid(label)),
            "{labels:?}"
        );
        assert_ne!(labels[0], labels[1]);
        assert_ne!(labels[0], labels[2]);
        assert_ne!(labels[1], labels[2]);
    }

    #[test]
    fn it_loads_a_recorded_run() {
        let root = tempfile::tempdir().unwrap();
        let mut writer = RunWriter::create(root.path(), "o-r-1").unwrap();
        writer
            .append(&Event::RunStarted {
                at: at(),
                run_id: "o-r-1".to_owned(),
                repo: "o/r".to_owned(),
                number: 1,
                title: "The epic".to_owned(),
                kind: Kind::Epic,
                branch: "epic".to_owned(),
                members: vec![
                    Member {
                        number: 2,
                        title: "First".to_owned(),
                        blocked_by: vec![],
                    },
                    Member {
                        number: 3,
                        title: "Second".to_owned(),
                        blocked_by: vec![2],
                    },
                ],
            })
            .unwrap();

        let view = load(root.path(), "o-r-1", false).unwrap();

        assert_eq!(view.title, "The epic");
        assert_eq!(view.status, RunStatus::Interrupted);
        assert_eq!(view.nodes.len(), 2);
        assert_eq!(view.columns, 2);
    }

    #[test]
    fn a_run_that_doesnt_exist_is_an_error() {
        let root = tempfile::tempdir().unwrap();

        assert!(load(root.path(), "o-r-1", false).is_err());
        assert!(load(root.path(), "../x", false).is_err());
    }
}
