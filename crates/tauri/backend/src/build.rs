//! Building from the assistant, host side: the two chat verbs through
//! which the persona starts a feature build and asks after it, and the
//! host state they draw on.
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
//! The log is the only record. `feature_status` folds it into an
//! [`epik_core::monitor::Progress`] and reads the run's [`Watch`],
//! which is the same fold and the same vocabulary the window draws
//! from: where the two could disagree, there is only one.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use epik_core::agent::claude::ClaudeCode;
use epik_core::dag::Dag;
use epik_core::git::url::Url;
use epik_core::github::{GitHub, Merge, Repo};
use epik_core::keystore::Secret;
use epik_core::model_tools::{Tool, arg};
use epik_core::monitor::{Change, EVENT, Entry, Log, Progress, RunId, Stage, Watch};
use epik_core::plan::AgentSlots;
use epik_core::plan::feature::{Build, Name, issue_branch};
use epik_core::plan::github::{CheckWait, Feature, feature_branch};
use epik_core::plan::stream::{Event, Sink};
use epik_core::tracker::IssueTracker;
use epik_core::tracker::github::GitHubIssues;
use epik_core::worktrees::Worktrees;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State};

/// How many agents the whole app runs at once.
const SLOTS: usize = 4;

/// How a build waits on GitHub's checks: polled every fifteen seconds
/// for up to an hour, and a green list read the same way twice before
/// it is the verdict.
const CHECKS: CheckWait = CheckWait {
    expected: true,
    every: Duration::from_secs(15),
    bound: Duration::from_secs(60 * 60),
    settles: NonZeroUsize::new(2).unwrap(),
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

/// The standing preface every issue's agent opens with: where it is,
/// what it must not do, and what implementing an issue means here. The
/// branch is already checked out in the worktree it runs in, and the
/// commit and the push are the build's own steps, after the agent has
/// exited.
const PREAMBLE: &str = "You are working in a git worktree that has been prepared for you, \
on the branch named below, which is already checked out. Implement the issue that follows. \
Commit your work as you go, with clear commit messages, so that the branch tells the story \
of what you built. Write tests covering the work, and make them pass. Do not push, do not \
create or switch branches, and do not touch git configuration — the identity and branch are \
set for you.";

/// What one issue's agent is told: the preamble, the branch it is on,
/// then the issue as the tracker holds it.
fn brief(branch: &str, issue: &str) -> String {
    format!("{PREAMBLE}\n\nBranch: {branch}\n\n{issue}")
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
                 runs at a time — ask feature_status how the one under way is going"
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
    pub log: Arc<Log>,
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

/// The feature tools as the app registers them each turn: the two chat
/// verbs wired to GitHub as the tracker and the forge and to Claude
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
    let state = app.state::<FeatureState>();
    let log = Arc::clone(&state.log);
    vec![
        start_feature(app.clone(), api_key, model, github_token, owner),
        feature_status(log),
    ]
}

/// What `start_feature` answers with once the build is running, and
/// what its handler assembles on the way.
struct Launched {
    run: RunId,
    branch: String,
    issues: usize,
}

/// The `start_feature` tool: reads the feature's work out of the
/// tracker, records the plan, and puts it on the runtime. It answers
/// when the build is running, not when it finishes — the turn that
/// called it ends while the build proceeds, and `feature_status` is
/// the door back in.
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
         running, with the run id that names this build; several features may build at \
         once, each with its own run id, and one feature builds once at a time. The build \
         then proceeds on its own for a long time, so tell the user the branch and run id \
         and end your turn — the user will ask how it is going, and feature_status answers \
         for the run.",
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
            let Launched {
                run,
                branch,
                issues,
            } = launch(
                &state,
                repo.clone(),
                number,
                api_key.clone(),
                model.clone(),
                github_token.clone(),
            )?;
            Ok(json!({
                "started": true,
                "run": run.0,
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
        checks: CHECKS,
        merge: Merge::Squash,
        slots: state.slots.clone(),
        sink,
        agent: Box::new(move |issue, cwd| ClaudeCode {
            binary: claude.clone(),
            cwd: cwd.to_owned(),
            prompt: brief(
                &issue_branch(issue),
                briefs.get(&issue).map_or("", String::as_str),
            ),
            model: model.clone(),
            api_key: api_key.clone(),
        }),
    };
    let count = work.nodes().len();
    // A feature with no issues under it runs its branch and its pull
    // request and nothing between: the one thing the old plan's
    // `doomed` named that the shape of a `Dag` cannot say for itself.
    let problems = match count {
        0 => vec![format!("feature {number} has no issues under it")],
        _ => Vec::new(),
    };
    log.record(Change::Started {
        run,
        work: work.clone(),
        title,
        titles,
        problems,
        checked: CHECKS.expected,
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
        run,
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

/// The `feature_status` tool over the log: the fold, and the run's
/// watch read out of it. The log is the record — there is no second
/// one — so what a model reads here and what the window draws are the
/// same picture.
fn feature_status(log: Arc<Log>) -> Tool {
    Tool::new(
        "feature_status",
        "How a feature build started by start_feature is going — or went: each issue's \
         title and state (waiting, running, merging, merged with the commit, failed with \
         its report, skipped naming the blocker whose failure stopped it), how many stand \
         where, and whether the build has finished. Several feature builds may run at \
         once: pass run — the run id start_feature answered with — to read a particular \
         build; with run omitted, the most recently started build answers, whichever \
         feature it is. Call it once when the user asks and report what it says; never \
         poll it in a loop — the build runs on its own.",
        json!({
            "type": "object",
            "properties": {
                "run": {
                    "type": "integer",
                    "description": "The run id start_feature answered with, selecting that build; the most recently started build when omitted.",
                },
            },
        }),
        Box::new(move |arguments| {
            let mut progress = Progress::default();
            for entry in log.since(0) {
                progress.absorb(&entry);
            }
            let watch = match arguments["run"].as_u64() {
                Some(asked) => progress
                    .watch(RunId(asked))
                    .ok_or_else(|| format!("no feature build has run id {asked}"))?,
                None => match progress.watches().last() {
                    Some(latest) => latest,
                    // A host that has started nothing has nothing to
                    // report, which is an answer rather than a refusal.
                    None => {
                        return Ok(json!({
                            "run": Value::Null,
                            "note": "no feature build has been started",
                        }));
                    }
                },
            };
            Ok(status(watch))
        }),
    )
}

/// One build as the model reads it, folded from the log and from
/// nothing else.
fn status(watch: &Watch) -> Value {
    let layout = watch.layout();
    let titles: BTreeMap<u64, &str> = layout
        .iter()
        .flat_map(|layout| &layout.nodes)
        .map(|node| (node.id, node.title.as_str()))
        .collect();
    let issues: Vec<Value> = watch
        .states()
        .iter()
        .map(|(id, state)| {
            let mut entry = serde_json::to_value(state).expect("a state serializes to an object");
            if let Value::Object(entry) = &mut entry {
                entry.insert("issue".to_owned(), json!(id));
                entry.insert("title".to_owned(), json!(titles.get(id)));
            }
            entry
        })
        .collect();
    let counts = watch.counts();
    json!({
        "run": watch.run().0,
        "feature": watch.feature(),
        "title": watch.title(),
        "repository": watch.repository(),
        "branch": watch.branch(),
        "stage": match watch.stage() {
            Stage::Starting => "starting".to_owned(),
            Stage::Building => "building".to_owned(),
            Stage::Finished => "finished".to_owned(),
            Stage::Abandoned { reason } => format!("abandoned: {reason}"),
        },
        "finished": matches!(watch.stage(), Stage::Finished | Stage::Abandoned { .. }),
        "checked": watch.checked(),
        "problems": watch.problems(),
        "counts": {
            "merged": counts.merged,
            "running": counts.running,
            "merging": counts.merging,
            "failed": counts.failed,
            "skipped": counts.skipped,
            "ready": counts.ready,
            "blocked": counts.blocked,
            "total": counts.total,
        },
        "issues": issues,
    })
}

#[cfg(test)]
mod tests {
    use epik_core::happening::{Happened, Name, Step};
    use epik_core::monitor::Event;

    use super::*;

    fn repo() -> Repo {
        Repo::new("o", "r")
    }

    /// The run of `tool` on `arguments`, or its refusal in words.
    fn ask(tool: &Tool, arguments: Value) -> Result<Value, String> {
        tool.call(&arguments)
    }

    /// Feature 1 holding 2 and 3, with 3 waiting on 2 — the shape a
    /// tracker gives and Started carries.
    fn two_then_three() -> Dag<u64> {
        use epik_core::dag::Node;
        Dag::new(vec![Node::new(2, []), Node::new(3, [0])]).unwrap()
    }

    /// A log holding one reserved, started build of feature 1.
    fn building() -> Arc<Log> {
        let log = Arc::new(Log::new());
        log.record(Change::Reserved {
            run: RunId(1),
            feature: 1,
            repository: repo().to_string(),
            branch: feature_branch(1),
            base: None,
        });
        log.record(Change::Started {
            run: RunId(1),
            work: two_then_three(),
            title: "the feature".to_owned(),
            titles: vec![(2, "first".to_owned()), (3, "second".to_owned())],
            problems: Vec::new(),
            checked: true,
        });
        log
    }

    /// One event of run 1's stream, recorded.
    fn happened(log: &Log, node: Name, happened: Happened) {
        log.record(Change::Happened {
            run: RunId(1),
            event: Event { node, happened },
        });
    }

    /// Issue `issue`'s state in a status answer.
    fn state(status: &Value, issue: u64) -> &Value {
        status["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["issue"] == json!(issue))
            .unwrap_or_else(|| panic!("no issue {issue} in {status}"))
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
        assert!(refused.contains("feature_status"), "{refused}");
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

    #[test]
    fn status_with_no_build_says_so() {
        let answer = ask(&feature_status(Arc::new(Log::new())), json!({})).unwrap();
        assert_eq!(answer["run"], Value::Null);
        assert_eq!(answer["note"], "no feature build has been started");
    }

    #[test]
    fn status_for_an_unknown_run_is_refused_by_its_number() {
        let refused = ask(&feature_status(building()), json!({ "run": 9 })).unwrap_err();
        assert_eq!(refused, "no feature build has run id 9");
    }

    /// Every word of the answer comes from the log's own entries: the
    /// states are derived from the stream, and nothing else is kept.
    #[test]
    fn status_reads_each_issues_state_out_of_the_stream() {
        let log = building();
        let status = feature_status(Arc::clone(&log));

        let answer = ask(&status, json!({})).unwrap();
        assert_eq!(answer["run"], json!(1));
        assert_eq!(answer["feature"], json!(1));
        assert_eq!(answer["title"], "the feature");
        assert_eq!(answer["repository"], "o/r");
        assert_eq!(answer["branch"], "feature-1");
        assert_eq!(answer["stage"], "building");
        assert_eq!(answer["finished"], json!(false));
        assert_eq!(answer["checked"], json!(true));
        assert_eq!(state(&answer, 2)["state"], "waiting");
        assert_eq!(state(&answer, 2)["title"], "first");
        assert_eq!(answer["counts"]["ready"], json!(1));
        assert_eq!(answer["counts"]["blocked"], json!(1));

        happened(&log, Name::Step(2, Step::Agent), Happened::Started);
        let answer = ask(&status, json!({ "run": 1 })).unwrap();
        assert_eq!(state(&answer, 2)["state"], "running");
        assert_eq!(answer["counts"]["running"], json!(1));

        happened(
            &log,
            Name::Step(2, Step::Push),
            Happened::Pushed("abc1234def".to_owned()),
        );
        happened(&log, Name::Step(2, Step::Close), Happened::Finished);
        let answer = ask(&status, json!({})).unwrap();
        assert_eq!(state(&answer, 2)["state"], "merged");
        assert_eq!(state(&answer, 2)["commit"], "abc1234def");
        assert_eq!(state(&answer, 3)["state"], "waiting", "ready now");
    }

    /// A failure is the issue's, in the words of the step that failed,
    /// and what waited on it is skipped naming the blocker — both the
    /// fold's reading of the stream, neither spoken into it.
    #[test]
    fn a_failure_upstream_skips_what_waited_on_it() {
        let log = building();
        happened(
            &log,
            Name::Step(2, Step::Checks),
            Happened::Failed("checks failed on abc1234: test".to_owned()),
        );
        let answer = ask(&feature_status(log), json!({})).unwrap();
        assert_eq!(state(&answer, 2)["state"], "failed");
        assert_eq!(
            state(&answer, 2)["report"],
            "checks failed on abc1234: test"
        );
        assert_eq!(state(&answer, 3)["state"], "skipped");
        assert_eq!(state(&answer, 3)["blocker"], json!(2));
        assert_eq!(answer["counts"]["failed"], json!(1));
        assert_eq!(answer["counts"]["skipped"], json!(1));
    }

    #[test]
    fn a_finished_build_says_it_is_finished_and_an_abandoned_one_says_why() {
        let log = building();
        log.record(Change::Finished { run: RunId(1) });
        let answer = ask(&feature_status(Arc::clone(&log)), json!({})).unwrap();
        assert_eq!(answer["stage"], "finished");
        assert_eq!(answer["finished"], json!(true));

        let log = Arc::new(Log::new());
        log.record(Change::Reserved {
            run: RunId(1),
            feature: 1,
            repository: repo().to_string(),
            branch: feature_branch(1),
            base: None,
        });
        log.record(Change::Abandoned {
            run: RunId(1),
            reason: "the claude CLI was not found".to_owned(),
        });
        let answer = ask(&feature_status(log), json!({})).unwrap();
        assert_eq!(answer["stage"], "abandoned: the claude CLI was not found");
        assert_eq!(answer["finished"], json!(true));
    }

    /// With no run named, the most recently started build answers.
    #[test]
    fn status_without_a_run_reads_the_latest_build() {
        let log = building();
        log.record(Change::Reserved {
            run: RunId(2),
            feature: 4,
            repository: repo().to_string(),
            branch: feature_branch(4),
            base: None,
        });
        let answer = ask(&feature_status(log), json!({})).unwrap();
        assert_eq!(answer["run"], json!(2));
        assert_eq!(answer["feature"], json!(4));
        assert_eq!(answer["stage"], "starting");
    }

    #[test]
    fn the_brief_leads_with_the_preamble_and_names_the_branch_and_the_issue() {
        let brief = brief(&issue_branch(7), "# Lay the stone\n\nAnd square it.");
        assert!(brief.starts_with(PREAMBLE), "{brief}");
        assert!(brief.contains("Branch: issue-7"), "{brief}");
        assert!(brief.contains("# Lay the stone"), "{brief}");
    }
}
