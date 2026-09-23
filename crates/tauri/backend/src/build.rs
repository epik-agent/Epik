//! Building from the assistant, host side: the shared Agent budget and
//! the tools through which the persona starts feature builds and asks
//! after them.
//!
//! [`FeatureState`] is the whole of it: the record of feature builds,
//! the [`Log`] every change to it is spoken to — the window hears each
//! entry as an event and replays the rest through `monitor_log` — and
//! the one [`Budget`] of four Agent slots every build in the app draws
//! on. The feature tools themselves live in `epik::feature::tools`;
//! this module only wires them to GitHub, Claude Code, and the window's
//! question rail.
//!
//! The assembly — locating the `claude` binary and starting the
//! [`ClaudeCode`] Agent — is a closure handed to the tools, so tests
//! substitute one that provisions without ever spawning a CLI.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use epik::feature::tools::{Builds, feature_status, start_feature};
use epik::feature::{Budget, Feature, Issue, Plan};
use epik::github::{GitHub, Repo};
use epik::job::Workspace;
use epik::job::agent::Agent;
use epik::job::agent::claude::ClaudeCode;
use epik::monitor::{EVENT, Entry, Log};
use epik::process::Process;
use epik::tracker::Tracker;
use epik::tracker::github::GitHubTracker;
use epik_core::assistant::{Answer, Ask};
use epik_core::keystore::Secret;
use epik_core::model_tools::Tool;
use tauri::{AppHandle, Emitter, Manager, State};

/// The `claude` CLI. A GUI app on macOS gets a minimal PATH, so the
/// usual homes are tried first, then PATH; the refusal names every place
/// looked.
fn claude() -> Result<PathBuf, String> {
    let mut tried = vec![
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        tried.push(Path::new(&home).join(".local/bin/claude"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        tried.extend(std::env::split_paths(&path).map(|dir| dir.join("claude")));
    }
    tried
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "the claude CLI was not found; tried {}",
                tried
                    .iter()
                    .map(|candidate| candidate.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// Claude Code, started in `workspace` with `prompt`; a failure to start
/// is words for the model.
fn start(
    claude: &Path,
    workspace: &Workspace,
    prompt: String,
    model: Option<String>,
    api_key: Option<Secret>,
) -> Result<Process, String> {
    ClaudeCode {
        binary: claude.to_path_buf(),
        cwd: workspace.directory.clone(),
        prompt,
        model,
        api_key,
    }
    .start()
    .map_err(|error| format!("could not start the Agent: {error:#}"))
}

/// The feature side of the managed state: the record of feature builds,
/// the log every change to it is spoken to, and the one budget of Agent
/// slots every build in the app draws on. Made in `setup`, after the
/// log — there is no default, because the record cannot exist without
/// somewhere to speak.
pub struct FeatureState {
    pub builds: Arc<Builds>,
    pub log: Arc<Log>,
    pub budget: Arc<Budget>,
}

impl FeatureState {
    pub fn new(log: Arc<Log>) -> Self {
        Self {
            builds: Arc::new(Builds::new(Arc::clone(&log))),
            log,
            budget: Budget::new(),
        }
    }
}

/// Follows `log` from a thread of its own and emits every entry on
/// [`monitor::EVENT`](EVENT). One reader with one
/// cursor, so the window hears entries in `seq` order however many
/// builds are recording at once — the fold on the far side drops by
/// `seq`, and would drop N for good if N+1 came first. What is emitted
/// before the window listens is in [`monitor_log`]'s replay.
pub fn relay(app: AppHandle, log: Arc<Log>) {
    std::thread::spawn(move || {
        let mut cursor = 0;
        loop {
            for entry in log.wait_after(cursor, Duration::from_secs(60)) {
                cursor = entry.seq + 1;
                let _ = app.emit(EVENT, &entry);
            }
        }
    });
}

/// The whole log so far, for a window that has just opened its eyes:
/// the replay it folds before — or after — it listens on
/// [`monitor::EVENT`](epik::monitor::EVENT). No command answers with a
/// picture; the window folds this one.
#[tauri::command]
pub fn monitor_log(state: State<'_, FeatureState>) -> Vec<Entry> {
    state.log.since(0)
}

/// The feature tools as the app registers them each turn: the library's
/// `start_feature` and `feature_status` wired to GitHub as the tracker
/// and the forge, Claude Code as the Agent, and the window's question
/// rail as the asker. `github_token` reads the plan and pushes the
/// feature branch; the writing side refuses in words without it. `model`
/// is the configured agent model, and `owner` the configured default a
/// bare repository name settles against.
pub fn feature_tools(
    app: &AppHandle,
    api_key: Option<Secret>,
    model: Option<String>,
    github_token: Option<Secret>,
    owner: Option<String>,
    asker: impl Fn(Ask) -> Answer + 'static,
) -> Vec<Tool> {
    let state = app.state::<FeatureState>();
    let builds = Arc::clone(&state.builds);
    let budget = Arc::clone(&state.budget);
    let plan = {
        let token = github_token.clone();
        let owner = owner.clone();
        move |spec: &str, feature: &Feature| -> Result<Plan, String> {
            let github = GitHub::new(token.clone());
            GitHubTracker::new(&github, Repo::settle(spec, owner.as_deref())?).plan(feature)
        }
    };
    let forge = move |spec: &str| -> Result<epik::forge::github::GitHub, String> {
        let token = github_token.clone().ok_or(
            "no GitHub token: pushing the feature branch needs one — \
             set the GitHub token in Settings (Cmd+,)",
        )?;
        Ok(epik::forge::github::GitHub {
            repo: Repo::settle(spec, owner.as_deref())?,
            token,
        })
    };
    let agents = move || {
        let claude = claude()?;
        let api_key = api_key.clone();
        let model = model.clone();
        Ok(move |_issue: &Issue, workspace: &Workspace, brief: &str| {
            start(
                &claude,
                workspace,
                brief.to_owned(),
                model.clone(),
                api_key.clone(),
            )
        })
    };
    vec![
        start_feature(Arc::clone(&builds), budget, plan, forge, agents, asker).into(),
        feature_status(builds).into(),
    ]
}
