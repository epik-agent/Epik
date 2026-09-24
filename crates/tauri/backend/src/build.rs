//! Building from the assistant, host side: the chat verb through which
//! the persona starts a feature build, and the host state it draws on.
//!
//! A build is [`epik_core::plan::feature::Build`] — the feature's
//! branch, its issues in the tracker's order, its pull request — run
//! as a [`Plan`] on this module's runtime. Everything it says goes
//! into one [`Sink`], and the sink writes each event into the host's
//! one [`Log`] as a [`Change::Happened`], beside the run-level changes
//! the launch itself speaks: Reserved before the work is read, Started
//! with the shape and the titles, Finished when every node is
//! terminal, and Abandoned when the launch failed before any of it
//! ran.
//!
//! [`FeatureState`] is the host half: the log, the one [`AgentSlots`]
//! every build in the app draws on, the [`Worktrees`] root its agents
//! work in, the runtime the plans run on, and the [`Running`] register
//! that names which features are building — the one place a second
//! build of a feature already building is refused.
//!
//! The log is the only record, and the monitor window is who reads it:
//! it folds the log into an [`epik_core::monitor::Progress`] of its own
//! and draws every build at once. The host speaks into the log and
//! relays it; nothing here folds it back.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use epik_core::agent::claude::ClaudeCode;
use epik_core::dag::Dag;
use epik_core::git::url::Url;
use epik_core::github::{GitHub, Repo};
use epik_core::keystore::Secret;
use epik_core::model_tools::{Tool, arg};
use epik_core::monitor::{Change, EVENT, Entry, Log, RunId};
use epik_core::plan::AgentSlots;
use epik_core::plan::feature::{Build, Errand, Name, issue_branch};
use epik_core::plan::github::{CheckWait, Feature, feature_branch};
use epik_core::plan::policy::{Policy, Reading};
use epik_core::plan::stream::{Event, Sink};
use epik_core::prompt::{fresh_fix_brief, issue_brief, resumed_fix_brief};
use epik_core::tracker::IssueTracker;
use epik_core::tracker::github::GitHubIssues;
use epik_core::worktrees::Worktrees;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

/// How many agents the whole app runs at once.
const SLOTS: usize = 4;

/// How a build waits on the checks its policy requires: polled every
/// fifteen seconds for up to an hour. Which checks those are is the
/// repository's to say, read at launch.
const CHECKS: CheckWait = CheckWait {
    every: Duration::from_secs(15),
    bound: Duration::from_secs(60 * 60),
};

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

/// Which features are building right now, as `repository#feature`. A
/// feature enters when its build is launched and leaves when the plan
/// returns, so a second build of one feature is refused while the
/// first runs and the same feature builds again once it is done.
#[derive(Debug, Default)]
pub struct Running(Mutex<BTreeSet<String>>);

/// One feature's place in the register, held for the life of its
/// build and given back when dropped.
#[derive(Debug)]
struct Claim {
    running: Arc<Running>,
    key: String,
}

impl Running {
    fn held(&self) -> MutexGuard<'_, BTreeSet<String>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The claim on feature `number` of `repo`, or the refusal in
    /// words when a build of it is already running.
    fn claim(self: &Arc<Self>, repo: &Repo, number: u64) -> Result<Claim, String> {
        let key = format!("{repo}#{number}");
        if !self.held().insert(key.clone()) {
            return Err(format!(
                "feature {number} of {repo} is already building; one build of a feature \
                 runs at a time — the monitor window shows how the one under way is going"
            ));
        }
        Ok(Claim {
            running: Arc::clone(self),
            key,
        })
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.running.held().remove(&self.key);
    }
}

/// The feature side of the managed state: the log every change is
/// spoken to, the slots every agent in the app runs in, the root the
/// builds' worktrees stand under, the runtime their plans run on, the
/// register of which features are building, and the counter run ids
/// come from. Made in `setup`, after the log — there is no default,
/// because nothing here can exist without somewhere to speak.
pub struct FeatureState {
    log: Arc<Log>,
    slots: AgentSlots,
    worktrees: PathBuf,
    runtime: tokio::runtime::Runtime,
    running: Arc<Running>,
    /// The next run id. A counter, so an abandoned launch never
    /// recycles its id onto a later build.
    next: AtomicU64,
}

impl FeatureState {
    /// The state around `log`, with the builds' worktrees under
    /// `worktrees`.
    ///
    /// # Errors
    ///
    /// A runtime the host will not start.
    pub fn new(log: Arc<Log>, worktrees: PathBuf) -> std::io::Result<Self> {
        Ok(Self {
            log,
            slots: AgentSlots::new(NonZeroUsize::new(SLOTS).expect("four slots")),
            worktrees,
            runtime: tokio::runtime::Runtime::new()?,
            running: Arc::new(Running::default()),
            next: AtomicU64::new(1),
        })
    }

    /// The next run id, held by no one: ids count up from 1 and are
    /// never reused.
    fn reserve(&self) -> RunId {
        RunId(self.next.fetch_add(1, Ordering::Relaxed))
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
/// [`monitor::EVENT`](epik_core::monitor::EVENT). No command answers
/// with a picture; the window folds this one.
#[tauri::command]
pub fn monitor_log(state: State<'_, FeatureState>) -> Vec<Entry> {
    state.log.since(0)
}

/// The feature tools as the app registers them each turn: the one chat
/// verb wired to GitHub as the tracker and the forge and to Claude
/// Code as the agent. `github_token` reads the plan, pushes the
/// branches and merges the pull requests; the build refuses in words
/// without it. `model` is the configured agent model, `api_key` the
/// key its environment carries, and `owner` the configured default a
/// bare repository name settles against.
pub fn feature_tools(
    app: &AppHandle,
    api_key: Option<Secret>,
    model: Option<String>,
    github_token: Option<Secret>,
    owner: Option<String>,
) -> Vec<Tool> {
    vec![start_feature(
        app.clone(),
        api_key,
        model,
        github_token,
        owner,
    )]
}

/// What `start_feature` answers with once the build is running, and
/// what its handler assembles on the way.
struct Launched {
    branch: String,
    issues: usize,
}

/// The `start_feature` tool: reads the feature's work out of the
/// tracker, records the plan, and puts it on the runtime. It answers
/// when the build is running, not when it finishes — the turn that
/// called it ends while the build proceeds, and the monitor window is
/// where the build is watched from there on.
fn start_feature(
    app: AppHandle,
    api_key: Option<Secret>,
    model: Option<String>,
    github_token: Option<Secret>,
    owner: Option<String>,
) -> Tool {
    Tool::new(
        "start_feature",
        "Builds a whole feature — an issue and the sub-issues beneath it — on GitHub: up to \
         four coding agents work the ready issues at once, each in a worktree of its own, \
         and each finished issue is pushed, opened as a pull request into one feature \
         branch, merged once GitHub's checks pass, and closed. The feature's own pull \
         request is opened at the end and left for you. Returns as soon as the build is \
         running; several features may build at once, and one feature builds once at a \
         time. The build then proceeds on its own for a long time, so tell the user the \
         branch and end your turn. Reporting on a running build is the monitor window's \
         work, not yours: it shows every build under way, each feature its own tab. Send \
         the user there rather than promising to check, and do not read the build's \
         progress off GitHub — the pushes, pull requests and merges you would find there \
         are the build's own steps, mid-flight.",
        json!({
            "type": "object",
            "properties": {
                "repo": {
                    "type": "string",
                    "description": "The repository as GitHub names it: one owner/name string.",
                },
                "feature": {
                    "type": "integer",
                    "description": "The feature issue's number.",
                },
            },
            "required": ["repo", "feature"],
        }),
        Box::new(move |arguments| {
            let spec = arg::non_empty(arguments, "repo")?;
            let number = arg::number(arguments, "feature")?;
            let repo = Repo::settle(spec, owner.as_deref())?;
            let state = app.state::<FeatureState>();
            let Launched { branch, issues } = launch(
                &state,
                repo.clone(),
                number,
                api_key.clone(),
                model.clone(),
                github_token.clone(),
            )?;
            Ok(json!({
                "started": true,
                "feature": number,
                "repository": repo.to_string(),
                "branch": branch,
                "issues": issues,
            }))
        }),
    )
}

/// The launch itself: claim the feature, reserve a run id, read the
/// work, and hand the plan to the runtime. Every failure from the
/// reservation on is Abandoned in the log with its own words before it
/// is handed back to the model, so a build that was asked for never
/// vanishes from the record unexplained.
fn launch(
    state: &FeatureState,
    repo: Repo,
    number: u64,
    api_key: Option<Secret>,
    model: Option<String>,
    github_token: Option<Secret>,
) -> Result<Launched, String> {
    // Before anything is recorded: two builds of one feature would
    // race on the same branch and the same pull request merges, and
    // the refusal is this register's alone to make.
    let claim = state.running.claim(&repo, number)?;
    let run = state.reserve();
    let branch = feature_branch(number);
    let log = Arc::clone(&state.log);
    log.record(Change::Reserved {
        run,
        feature: number,
        repository: repo.to_string(),
        branch: branch.clone(),
        base: None,
    });
    let abandon = |reason: String| {
        log.record(Change::Abandoned {
            run,
            reason: reason.clone(),
        });
        reason
    };
    let github = GitHub::new(github_token);
    let issues = GitHubIssues {
        github: github.clone(),
        repo: repo.clone(),
    };
    let read = read_work(&issues, number).map_err(abandon)?;
    // Before the build creates anything: what the repository's rulesets
    // let it do, and the refusal when they forbid what it would do.
    let Reading { settled, said } = Policy::read(&github, &repo, number, &read.work)
        .map_err(|error| abandon(format!("the repository's rules could not be read: {error}")))?;
    let policy = settled.map_err(abandon)?;
    let url = Url::Scheme(
        format!("https://github.com/{repo}.git")
            .parse()
            .map_err(|error| abandon(format!("{repo} is no clone URL: {error}")))?,
    );
    let claude = claude().map_err(abandon)?;
    let feature = Feature {
        github,
        repo,
        number,
    };
    let sink = {
        let log = Arc::clone(&log);
        Sink::new(move |event: Event<Name>| log.record(Change::Happened { run, event }))
    };
    let Work {
        work,
        title,
        titles,
        briefs,
    } = read;
    let build = Build {
        feature,
        url,
        worktrees: Worktrees::new(state.worktrees.clone()),
        required: policy.checks.clone(),
        checks: CHECKS,
        merge: policy.merge,
        slots: state.slots.clone(),
        sink,
        agent: Box::new(move |issue, cwd, errand| {
            let branch = issue_branch(issue);
            let text = briefs.get(&issue).map_or("", String::as_str);
            let (prompt, resume) = match errand {
                Errand::Implement => (issue_brief(&branch, text), None),
                Errand::Resume { session, checks } => {
                    (resumed_fix_brief(checks), Some(session.clone()))
                }
                Errand::Fresh { diff, checks } => {
                    (fresh_fix_brief(&branch, text, diff, checks), None)
                }
            };
            ClaudeCode {
                binary: claude.clone(),
                cwd: cwd.to_owned(),
                prompt,
                model: model.clone(),
                api_key: api_key.clone(),
                resume,
            }
        }),
    };
    let count = work.nodes().len();
    // A feature with no issues under it runs its branch and its pull
    // request and nothing between: the one thing the old plan's
    // `doomed` named that the shape of a `Dag` cannot say for itself.
    let mut problems = match count {
        0 => vec![format!("feature {number} has no issues under it")],
        _ => Vec::new(),
    };
    problems.extend(said);
    log.record(Change::Started {
        run,
        work: work.clone(),
        title,
        titles,
        problems,
        checked: !policy.checks.is_empty(),
    });
    let plan = build.plan(work);
    state.runtime.spawn(async move {
        // The plan's own failure is already on the stream, node by
        // node; Finished says only that nothing is left to run.
        let _outcome = plan.execute().await;
        log.record(Change::Finished { run });
        drop(claim);
    });
    Ok(Launched {
        branch,
        issues: count,
    })
}

/// The feature's work as the tracker gives it, with the words each
/// node's agent and each node's label are made from.
struct Work {
    work: Dag<u64>,
    title: String,
    titles: Vec<(u64, String)>,
    briefs: BTreeMap<u64, String>,
}

/// Reads feature `number` out of `issues`: the shape of its work, its
/// own title, every issue's title, and every issue's content as the
/// tracker holds it — which is what its agent is briefed with.
fn read_work(issues: &GitHubIssues, number: u64) -> Result<Work, String> {
    let words = |error: anyhow::Error| format!("{error:#}");
    let work = issues.work(&number).map_err(words)?;
    let title = issues
        .github
        .issue(&issues.repo, number)
        .map_err(|error| error.to_string())?
        .title;
    let mut titles = Vec::new();
    let mut briefs = BTreeMap::new();
    for node in work.nodes() {
        let id = *node.value();
        let issue = issues
            .github
            .issue(&issues.repo, id)
            .map_err(|error| error.to_string())?;
        titles.push((id, issue.title));
        briefs.insert(id, issues.content(&id).map_err(words)?);
    }
    Ok(Work {
        work,
        title,
        titles,
        briefs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> Repo {
        Repo::new("o", "r")
    }

    #[test]
    fn a_second_build_of_one_feature_is_refused_in_words() {
        let running = Arc::new(Running::default());
        // The claim lives to the end of the test: dropping it here
        // would free the feature and the second claim would succeed.
        let _held = running.claim(&repo(), 1).unwrap();
        let refused = running.claim(&repo(), 1).unwrap_err();
        assert!(
            refused.contains("feature 1 of o/r is already building"),
            "{refused}"
        );
        assert!(refused.contains("the monitor window shows"), "{refused}");
    }

    #[test]
    fn another_feature_and_another_repository_build_alongside() {
        let running = Arc::new(Running::default());
        let _one = running.claim(&repo(), 1).unwrap();
        running.claim(&repo(), 2).expect("another feature");
        running
            .claim(&Repo::new("o", "other"), 1)
            .expect("the same number elsewhere");
    }

    /// The claim is the build's life: it is given back when the build
    /// ends, and the same feature builds again.
    #[test]
    fn a_finished_build_lets_the_feature_build_again() {
        let running = Arc::new(Running::default());
        let held = running.claim(&repo(), 1).unwrap();
        drop(held);
        running.claim(&repo(), 1).expect("the first build is done");
    }
}
