//! The **Set up Epik** window: the setup checklist.
//!
//! It shows each item from `epik_core::setup` with its state and detail,
//! and for each item that isn't done the one button that does it. Nothing
//! runs until the user presses a button. The page in `ui/setup.html` asks
//! [`setup_checklist`] for the items, presses buttons through
//! [`setup_run`], and polls [`setup_output`] for what the step prints.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use epik_core::builds::Builds;
use epik_core::setup::{self, Check, Item, State as ItemState};
use serde::Serialize;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

/// The window's label.
const LABEL: &str = "setup";

/// One item as the checklist shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub id: &'static str,
    pub title: String,
    pub state: ItemState,
    pub detail: String,
    /// The button's label, for an item that isn't done.
    pub button: Option<String>,
}

/// The checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Checklist {
    pub rows: Vec<Row>,
    /// Whether every item is done.
    pub complete: bool,
}

/// The checklist for `checks`: each item's state, and the one button of
/// each that isn't done.
pub fn checklist(checks: &[Check]) -> Checklist {
    Checklist {
        rows: checks
            .iter()
            .map(|check| Row {
                id: check.item.id(),
                title: check.title.clone(),
                state: check.state,
                detail: check.detail.clone(),
                button: setup::step(check).map(|step| step.label),
            })
            .collect(),
        complete: !checks.is_empty() && checks.iter().all(|c| c.state == ItemState::Done),
    }
}

/// What a pressed button has printed, and how it ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Output {
    pub lines: Vec<String>,
    pub running: bool,
    pub error: Option<String>,
}

/// What the window's commands reach.
pub struct Setup {
    pub root: PathBuf,
    pub builds: Arc<Builds>,
    /// Each item's latest step, by id.
    pub outputs: Arc<Mutex<BTreeMap<String, Output>>>,
}

/// Checks every item, and once all are done writes `~/.epik/setup-done`.
#[tauri::command]
pub async fn setup_checklist(setup: State<'_, Setup>) -> Result<Checklist, String> {
    let checks = setup.builds.setup();
    let list = checklist(&checks);
    if list.complete {
        mark_done(&setup.root);
    }
    Ok(list)
}

fn mark_done(root: &Path) {
    if let Err(error) = std::fs::write(setup::done_file(root), "") {
        eprintln!("epik-app: couldn't record that setup is done: {error}");
    }
}

/// Starts the step of the item `id`, as its button said, unless one is
/// already running.
#[tauri::command]
pub fn setup_run(id: String, setup: State<'_, Setup>) -> Result<(), String> {
    let item = Item::ALL
        .into_iter()
        .find(|item| item.id() == id)
        .ok_or_else(|| format!("no setup item {id:?}"))?;
    let check = setup
        .builds
        .setup()
        .into_iter()
        .find(|check| check.item == item)
        .ok_or_else(|| format!("no setup item {id:?}"))?;
    let step = setup::step(&check).ok_or_else(|| format!("{} is already done", check.title))?;
    {
        let mut outputs = setup.outputs.lock().unwrap();
        if outputs.get(&id).is_some_and(|output| output.running) {
            return Err(format!("{} is already being set up", check.title));
        }
        outputs.insert(
            id.clone(),
            Output {
                running: true,
                ..Output::default()
            },
        );
    }
    let (root, outputs) = (setup.root.clone(), setup.outputs.clone());
    thread::spawn(move || {
        let print = |line: String| {
            if let Some(output) = outputs.lock().unwrap().get_mut(&id) {
                output.lines.push(line);
            }
        };
        let result = setup::run_step(&root, &step, &print);
        if let Some(output) = outputs.lock().unwrap().get_mut(&id) {
            output.running = false;
            output.error = result.err();
        }
    });
    Ok(())
}

/// What the step of item `id` has printed so far.
#[tauri::command]
pub fn setup_output(id: String, setup: State<'_, Setup>) -> Output {
    setup
        .outputs
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .unwrap_or_default()
}

/// Opens the checklist, or focuses it if it is open.
pub fn open(handle: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = handle.get_webview_window(LABEL) {
        window.unminimize()?;
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(handle, LABEL, WebviewUrl::App("setup.html".into()))
        .title("Set up Epik")
        .inner_size(640.0, 720.0)
        .build()?;
    Ok(())
}

/// Whether the checklist opens by itself at startup: on the first launch,
/// before setup was ever completed, or whenever something it needs isn't
/// done.
pub fn opens_at_start(root: &Path, checks: &[Check]) -> bool {
    !setup::done_file(root).exists() || setup::refusal(checks).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(item: Item, state: ItemState, detail: &str) -> Check {
        Check {
            item,
            title: item.title().to_owned(),
            state,
            detail: detail.to_owned(),
            usable: state == ItemState::Done,
        }
    }

    fn all_done() -> Vec<Check> {
        Item::ALL
            .into_iter()
            .map(|item| check(item, ItemState::Done, "fine"))
            .collect()
    }

    #[test]
    fn each_item_shows_its_state_and_detail_and_only_undone_ones_a_button() {
        let mut checks = all_done();
        checks[1] = check(Item::GhAuth, ItemState::Missing, "not signed in");
        checks[4] = check(
            Item::Node,
            ItemState::Outdated,
            "Node 22.1.0; Epik needs 26 or later",
        );

        let list = checklist(&checks);

        assert!(!list.complete);
        let buttons: Vec<(&str, Option<&str>)> = list
            .rows
            .iter()
            .map(|row| (row.id, row.button.as_deref()))
            .collect();
        assert_eq!(
            buttons,
            [
                ("gh", None),
                ("gh-auth", Some("Sign in to GitHub (opens the browser)")),
                ("claude", None),
                ("claude-auth", None),
                ("node", Some("Update the private copy")),
                ("plugin", None),
            ]
        );
        assert_eq!(list.rows[4].detail, "Node 22.1.0; Epik needs 26 or later");
        assert_eq!(list.rows[4].state, ItemState::Outdated);
    }

    #[test]
    fn with_every_item_done_the_checklist_is_complete() {
        let list = checklist(&all_done());

        assert!(list.complete);
        assert!(list.rows.iter().all(|row| row.button.is_none()));
        assert!(!checklist(&[]).complete);
    }

    #[test]
    fn it_opens_at_start_until_setup_is_done_and_when_a_build_would_be_refused() {
        let root = tempfile::tempdir().unwrap();
        assert!(opens_at_start(root.path(), &all_done()), "first launch");

        mark_done(root.path());
        assert!(!opens_at_start(root.path(), &all_done()));

        let mut no_plugin = all_done();
        no_plugin[5] = check(Item::Plugin, ItemState::Missing, "not installed");
        assert!(
            !opens_at_start(root.path(), &no_plugin),
            "builds don't need it"
        );

        let mut signed_out = all_done();
        signed_out[3] = check(Item::ClaudeAuth, ItemState::Missing, "not signed in");
        assert!(opens_at_start(root.path(), &signed_out));
    }
}
