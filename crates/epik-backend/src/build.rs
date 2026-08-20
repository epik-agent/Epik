//! Building from chat, host side: the one run slot, the shared Agent
//! budget, and the tools through which the persona starts builds and
//! asks after them.
//!
//! [`BuildState`] holds one plain run at a time — the same claim
//! discipline as a turn: a second `start_build` while one is in flight
//! is refused in words — and keeps the last run's record after it
//! finishes, so status outlives completion. Starting returns at once:
//! an inline wait would hold the turn for the whole build. The record
//! is the run's only sink; nothing an Agent says is ever emitted on the
//! transcript channel. `build_status` and the git verbs are the
//! persona's view of it.
//!
//! [`FeatureState`] is the feature side: the record of feature builds
//! and the one [`Budget`] of four Agent slots that plain builds and
//! feature builds draw on together — a plain build claims a slot for
//! its Agent's life, or refuses in words when all four are out. The
//! feature tools themselves live in `epik::feature::tools`; this module
//! only wires them to GitHub, Claude Code, and the window's question
//! rail.
//!
//! The assembly — locating the runner and the `claude` binary, building
//! the [`ClaudeCode`] Agent — is a closure handed to the tools, so tests
//! substitute one that provisions without ever spawning a CLI.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use epik::agent::Handle;
use epik::agent::claude_code::{ClaudeCode, Update};
use epik::build::{self, Order, Phase, Record, Run, Workspace};
use epik::chat::{Answer, Ask};
use epik::feature::tools::{Builds, feature_status, start_feature};
use epik::feature::{Budget, Issue, IssueId, Plan};
use epik::github::{GitHub, GitHubTracker, Repo};
use epik::keystore::Secret;
use epik::tools::Tool;
use epik::tracker::Tracker;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

/// One run: its record, and the handle that is its life. Dropping the
/// handle kills the Agent, so the slot keeps it for as long as the record
/// is the current one.
struct Build {
    record: Run,
    _handle: Option<Handle>,
}

/// The run slot: at most one build in flight, and the last one's record
/// afterwards.
#[derive(Default)]
pub struct BuildState {
    current: Mutex<Option<Build>>,
}

/// What `start` hands back: the record to keep, and the handle when a
/// process was actually launched.
pub type Started = (Run, Option<Handle>);

impl BuildState {
    /// Claims the slot and starts a build through `start` — which
    /// provisions and launches, and whose Err is words for the model —
    /// returning the workspace at once. Refused in words while a run is
    /// in flight; the claim is held through the start, so two turns
    /// cannot both provision.
    pub fn start(
        &self,
        order: Order,
        start: impl FnOnce(Order) -> Result<Started, String>,
    ) -> Result<Workspace, String> {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(build) = current.as_ref() {
            let record = build.record.lock().unwrap_or_else(PoisonError::into_inner);
            if record.phase == Phase::Running {
                return Err(format!(
                    "a build is already running on branch {} of {}; wait for it to finish \
                     (build_status says how it is going) before starting another",
                    record.workspace.branch, record.workspace.repository
                ));
            }
        }
        let (record, handle) = start(order)?;
        let workspace = record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .workspace
            .clone();
        *current = Some(Build {
            record,
            _handle: handle,
        });
        Ok(workspace)
    }

    /// A snapshot of the current or last run's record, if there has been
    /// one.
    #[must_use]
    pub fn record(&self) -> Option<Record> {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|build| {
                build
                    .record
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            })
    }
}

/// The agent runner: `epik-agent` beside the running executable — true
/// in a dev `target/debug`, and in a bundle once the runner ships as a
/// sidecar.
fn runner() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("could not locate the running executable: {error}"))?;
    let runner = exe
        .parent()
        .ok_or("the running executable has no parent directory")?
        .join("epik-agent");
    if runner.is_file() {
        Ok(runner)
    } else {
        Err(format!(
            "the agent runner is not beside the app: expected {}",
            runner.display()
        ))
    }
}

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

/// The production start: claim an Agent slot from the shared budget,
/// locate the binaries, provision, assemble Claude Code in the workspace
/// with the brief as its prompt, launch. `api_key` rides into the
/// Agent's environment when there is one; without it the CLI's own
/// logged-in auth applies. The slot is held for the Agent's life: a
/// waiter joins the drainer — whose last act is the commit observation —
/// and releases it, so a feature build alongside gets the slot back.
fn start_claude(
    order: Order,
    api_key: Option<Secret>,
    budget: &Arc<Budget>,
) -> Result<Started, String> {
    let slot = budget.claim().ok_or(
        "all four Agent slots are busy; wait for one to finish \
         (build_status and feature_status say how they are going) before starting another",
    )?;
    let runner = runner()?;
    let claude = claude()?;
    let workspace = build::provision(&order)?;
    let agent = ClaudeCode {
        binary: claude.to_string_lossy().into_owned(),
        cwd: workspace.directory.to_string_lossy().into_owned(),
        prompt: workspace.brief(&order.prompt),
        model: None,
        api_key,
    };
    let record = Record::new(order, workspace);
    let (handle, drainer) = build::launch(&agent, &runner, record.clone())
        .map_err(|error| format!("could not launch the agent runner: {error}"))?;
    // The chat surface reads the record as it fills and never waits for
    // the observation; the waiter exists only to hold the slot exactly
    // as long as the run.
    std::thread::spawn(move || {
        let _held = slot;
        let _ = drainer.join();
    });
    Ok((record, Some(handle)))
}

fn string<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, String> {
    arguments[name]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("the {name} argument must be a non-empty string"))
}

/// The `start_build` tool over any starter — the slot's claim and the
/// starter's words are what the model reads.
fn start_build(start: impl Fn(Order) -> Result<Workspace, String> + 'static) -> Tool {
    Tool::new(
        "start_build",
        "Launches a coding agent to implement prompt in repository — a local git repository \
         path (make one with git_init if the user's chosen location has none yet) — on a new \
         branch named branch, started from base or the repository's default branch. Returns at \
         once with the branch and the private worktree directory the agent works in; the build \
         then runs on its own for minutes. As soon as it has started, tell the user the branch \
         and repository and end your turn — do not call build_status in the same turn; the user \
         will ask how it is going. One build runs at a time. The agent commits as it goes; the \
         branch is where the work lands.",
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The instructions for the agent: what to build, in full.",
                },
                "repository": {
                    "type": "string",
                    "description": "The absolute path of the local git repository to build in.",
                },
                "branch": {
                    "type": "string",
                    "description": "The name of the new branch the work lands on. Must not exist yet.",
                },
                "base": {
                    "type": "string",
                    "description": "The branch or commit to start from; the repository's default branch when omitted.",
                },
            },
            "required": ["prompt", "repository", "branch"],
        }),
        Box::new(move |arguments| {
            let order = Order {
                prompt: string(arguments, "prompt")?.to_owned(),
                repository: string(arguments, "repository")?.to_owned(),
                branch: string(arguments, "branch")?.to_owned(),
                base: arguments["base"].as_str().map(str::to_owned),
            };
            let workspace = start(order)?;
            Ok(json!({
                "started": true,
                "repository": workspace.repository,
                "branch": workspace.branch,
                "base_commit": workspace.base_commit,
                "directory": workspace.directory,
            }))
        }),
    )
}

/// How many of the most recent narration updates `build_status` reports.
const NARRATION_SHOWN: usize = 30;

/// The record in words and numbers, for the model.
fn status(record: &Record) -> Value {
    let (phase, exit) = match &record.phase {
        Phase::Running => ("running", Value::Null),
        Phase::Finished(exit) => (
            "finished",
            json!({ "code": exit.code, "signal": exit.signal }),
        ),
    };
    let result = record.result().map(|update| match update {
        Update::Result {
            ok,
            text,
            cost_usd,
            input_tokens,
            output_tokens,
        } => json!({
            "ok": ok,
            "text": text,
            "cost_usd": cost_usd,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        }),
        _ => Value::Null,
    });
    let shown = record.narration.len().saturating_sub(NARRATION_SHOWN);
    json!({
        "phase": phase,
        "exit": exit,
        "repository": record.workspace.repository,
        "branch": record.workspace.branch,
        "base_commit": record.workspace.base_commit,
        "directory": record.workspace.directory,
        "prompt": record.order.prompt,
        "narration": record.narration[shown..],
        "narration_total": record.narration.len(),
        "stderr": record.stderr,
        "result": result,
        "commits": record.commits.as_ref().map(|commits| json!({
            "branch_advanced": commits.advanced,
            "head": commits.head,
            "worktree_clean": commits.clean,
            "worktree_kept_at": commits.kept,
        })),
    })
}

/// The `build_status` tool over any record source.
fn build_status(record: impl Fn() -> Option<Record> + 'static) -> Tool {
    Tool::new(
        "build_status",
        "How the build launched by start_build is going — or went: its phase, the agent's \
         latest narration, the terminal result when there is one (ok, text, cost), and at the \
         end whether the branch advanced and the worktree was left clean. Call it once when the \
         user asks and report what it says; never poll it in a loop, since the build runs on \
         its own and a running phase means exactly that. This covers the launched build only; \
         questions about a repository's history are for git_log.",
        json!({ "type": "object", "properties": {} }),
        Box::new(move |_| {
            Ok(record().map_or_else(
                || json!({ "phase": "none", "note": "no build has been started" }),
                |record| status(&record),
            ))
        }),
    )
}

/// The feature side of the managed state: the record of feature builds,
/// and the one budget of Agent slots every build in the app draws on.
pub struct FeatureState {
    pub builds: Arc<Builds>,
    pub budget: Arc<Budget>,
}

impl Default for FeatureState {
    fn default() -> Self {
        Self {
            builds: Arc::new(Builds::default()),
            budget: Budget::new(),
        }
    }
}

/// The build tools as the app registers them each turn: the slot lives
/// in managed state, the starter is Claude Code with `api_key`, and the
/// Agent slot comes from the shared budget.
pub fn tools(app: AppHandle, api_key: Option<Secret>) -> Vec<Tool> {
    let starter = {
        let app = app.clone();
        move |order| {
            let budget = Arc::clone(&app.state::<FeatureState>().budget);
            app.state::<BuildState>()
                .start(order, |order| start_claude(order, api_key.clone(), &budget))
        }
    };
    vec![
        start_build(starter),
        build_status(move || app.state::<BuildState>().record()),
    ]
}

/// An `owner/name` spelling settled, or refused in words the model reads.
fn parsed(spec: &str) -> Result<Repo, String> {
    Repo::parse(spec).ok_or_else(|| format!("{spec:?} is not an owner/name repository spelling"))
}

/// The feature tools as the app registers them each turn: the library's
/// `start_feature` and `feature_status` wired to GitHub as the tracker
/// and the forge, Claude Code as the Agent, and the window's question
/// rail as the asker. `github_token` reads the plan and pushes the
/// feature branch; the writing side refuses in words without it.
pub fn feature_tools(
    app: &AppHandle,
    api_key: Option<Secret>,
    github_token: Option<Secret>,
    asker: impl Fn(Ask) -> Answer + 'static,
) -> Vec<Tool> {
    let state = app.state::<FeatureState>();
    let builds = Arc::clone(&state.builds);
    let budget = Arc::clone(&state.budget);
    let plan = {
        let token = github_token.clone();
        move |spec: &str, feature: &IssueId| -> Result<Plan, String> {
            let github = GitHub::new(token.clone());
            GitHubTracker::new(&github, parsed(spec)?).plan(feature)
        }
    };
    let forge = move |spec: &str| -> Result<epik::forge::GitHub, String> {
        let token = github_token.clone().ok_or(
            "no GitHub token: pushing the feature branch needs one — \
             set the GitHub token in Settings (Cmd+,)",
        )?;
        Ok(epik::forge::GitHub {
            repo: parsed(spec)?,
            token,
        })
    };
    let agents = move || {
        let claude = claude()?.to_string_lossy().into_owned();
        let api_key = api_key.clone();
        Ok(
            move |_issue: &Issue, workspace: &Workspace, brief: &str| ClaudeCode {
                binary: claude.clone(),
                cwd: workspace.directory.to_string_lossy().into_owned(),
                prompt: brief.to_owned(),
                model: None,
                api_key: api_key.clone(),
            },
        )
    };
    vec![
        start_feature(
            Arc::clone(&builds),
            budget,
            runner,
            plan,
            forge,
            agents,
            asker,
        ),
        feature_status(builds),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use epik::tools::Registry;
    use std::sync::Arc;

    /// A scratch directory that cleans up after itself.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "epik-backend-build-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn join(&self, name: &str) -> String {
            self.0.join(name).to_str().unwrap().to_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The test starter: provisions for real, launches nothing. The
    /// record it hands back is a build that will never finish — enough
    /// for the slot to consider it in flight.
    fn provision_only(order: Order) -> Result<Started, String> {
        let workspace = build::provision(&order)?;
        Ok((Record::new(order, workspace), None))
    }

    fn tidy(workspace: &Workspace) {
        let directory = workspace.directory.to_string_lossy().into_owned();
        let _ = std::process::Command::new("git")
            .args([
                "-C",
                &workspace.repository,
                "worktree",
                "remove",
                "--force",
                "--",
                &directory,
            ])
            .status();
    }

    /// The tools over `state`, with git beside them, as a turn would
    /// have them.
    fn registry(state: &Arc<BuildState>) -> Registry {
        let mut registry = Registry::default();
        registry.extend(epik::git::all());
        registry.register(start_build({
            let state = Arc::clone(state);
            move |order| state.start(order, provision_only)
        }));
        registry.register(build_status({
            let state = Arc::clone(state);
            move || state.record()
        }));
        registry
    }

    fn init(registry: &Registry, directory: &str) {
        let made = registry
            .dispatch("git_init", &json!({ "directory": directory }).to_string())
            .unwrap();
        assert_eq!(made["ok"], true, "{made}");
    }

    #[test]
    fn the_slot_refuses_a_second_build_while_one_is_in_flight() {
        let scratch = Scratch::new("slot");
        let state = Arc::new(BuildState::default());
        let registry = registry(&state);
        let repository = scratch.join("repo.git");
        init(&registry, &repository);

        let first = registry
            .dispatch(
                "start_build",
                &json!({ "prompt": "p", "repository": repository, "branch": "one" }).to_string(),
            )
            .unwrap();
        assert_eq!(first["started"], true, "{first}");
        assert_eq!(first["branch"], "one");

        let refused = registry
            .dispatch(
                "start_build",
                &json!({ "prompt": "p", "repository": repository, "branch": "two" }).to_string(),
            )
            .unwrap_err();
        assert!(refused.contains("already running"), "{refused}");
        assert!(refused.contains("one"), "{refused}");

        let status = registry.dispatch("build_status", "{}").unwrap();
        assert_eq!(status["phase"], "running");
        assert_eq!(status["branch"], "one");

        tidy(&build::Workspace {
            repository: repository.clone(),
            branch: "one".to_owned(),
            base_commit: String::new(),
            directory: PathBuf::from(first["directory"].as_str().unwrap()),
        });
    }

    #[test]
    fn a_missing_repository_surfaces_provisions_words_as_the_result() {
        let state = Arc::new(BuildState::default());
        let registry = registry(&state);
        let error = registry
            .dispatch(
                "start_build",
                &json!({
                    "prompt": "p",
                    "repository": "/nonexistent/epik-build",
                    "branch": "b",
                })
                .to_string(),
            )
            .unwrap_err();
        assert!(error.contains("not a git repository"), "{error}");
        assert!(state.record().is_none(), "nothing was started");
        assert_eq!(
            registry.dispatch("build_status", "{}").unwrap()["phase"],
            "none"
        );
    }

    #[test]
    fn missing_arguments_are_refused_in_words() {
        let state = Arc::new(BuildState::default());
        let registry = registry(&state);
        let error = registry
            .dispatch("start_build", r#"{"prompt":"p","repository":"/r"}"#)
            .unwrap_err();
        assert!(error.contains("branch"), "{error}");
    }

    /// The finished record reads back as words and numbers, result and
    /// observation included.
    #[test]
    fn status_reports_a_finished_run_with_its_result_and_observation() {
        let run = Record::new(
            Order {
                prompt: "p".to_owned(),
                repository: "/r".to_owned(),
                branch: "b".to_owned(),
                base: None,
            },
            Workspace {
                repository: "/r".to_owned(),
                branch: "b".to_owned(),
                base_commit: "base".to_owned(),
                directory: PathBuf::from("/w"),
            },
        );
        {
            let mut record = run.lock().unwrap();
            record.narration.push(Update::Text {
                text: "working".to_owned(),
            });
            record.narration.push(Update::Result {
                ok: true,
                text: "done".to_owned(),
                cost_usd: Some(0.25),
                input_tokens: None,
                output_tokens: Some(7),
            });
            record.phase = Phase::Finished(epik::agent::Exit {
                code: Some(0),
                signal: None,
            });
            record.commits = Some(build::Commits {
                head: "tip".to_owned(),
                advanced: true,
                clean: true,
                kept: None,
            });
        }
        let status = status(&run.lock().unwrap());
        assert_eq!(status["phase"], "finished");
        assert_eq!(status["exit"]["code"], 0);
        assert_eq!(status["result"]["ok"], true);
        assert_eq!(status["result"]["text"], "done");
        assert_eq!(status["result"]["cost_usd"], 0.25);
        assert_eq!(status["commits"]["branch_advanced"], true);
        assert_eq!(status["commits"]["worktree_clean"], true);
        assert!(status["commits"]["worktree_kept_at"].is_null());
        assert_eq!(status["narration_total"], 2);
    }

    /// The whole turn against the scripted model: git_init, then
    /// start_build with the injected starter, and the reply reaches text
    /// while the "build" is nominally running.
    #[test]
    fn a_scripted_turn_inits_a_repository_starts_a_build_and_answers() {
        use epik::chat::scripted::{Fragment, Scripted, Turn as Script};
        use epik::chat::{Client, Role, TranscriptItem};

        let scratch = Scratch::new("turn");
        let repository = scratch.join("wumpus.git");
        let model = Scripted::spawn(vec![
            Script::ToolCalls(vec![vec![Fragment::open(
                0,
                "call_1",
                "git_init",
                &json!({ "directory": repository }).to_string(),
            )]]),
            Script::ToolCalls(vec![vec![Fragment::open(
                0,
                "call_2",
                "start_build",
                &json!({
                    "prompt": "Write text-mode hunt the wumpus.",
                    "repository": repository,
                    "branch": "wumpus",
                })
                .to_string(),
            )]]),
            Script::text(&["Building on branch wumpus."]),
        ]);
        let state = Arc::new(BuildState::default());
        let registry = registry(&state);
        let client = Client::new(model.base_url(), "scripted".to_owned(), None);
        let mut observed = Vec::new();

        let text = epik::tools::run(
            &client,
            "system",
            &[TranscriptItem::Message {
                role: Role::User,
                text: "Write me text-mode hunt-the-wumpus.".to_owned(),
            }],
            &registry,
            |_| {},
            |call, result| observed.push((call.name.clone(), result.clone())),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();

        assert_eq!(text, "Building on branch wumpus.");
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0].0, "git_init");
        assert_eq!(observed[1].0, "start_build");
        let started = observed[1].1.as_ref().unwrap();
        assert_eq!(started["branch"], "wumpus");
        assert_eq!(started["repository"], repository);

        let record = state.record().unwrap();
        assert_eq!(record.phase, Phase::Running);
        assert_eq!(record.order.prompt, "Write text-mode hunt the wumpus.");
        assert!(record.workspace.directory.is_dir());

        // The third request carried the start_build result to the model.
        let third = &model.requests()[2];
        let answer = third["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(answer["tool_call_id"], "call_2");
        assert!(
            answer["content"].as_str().unwrap().contains("wumpus"),
            "{answer}"
        );

        tidy(&record.workspace);
    }
}
