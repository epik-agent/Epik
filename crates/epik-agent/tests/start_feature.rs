//! The chat verbs of a feature build, end to end: `start_feature` and
//! `feature_status` dispatched through a real registry, scripted Shell
//! Agents through the real runner, scratch repositories, and an asker
//! the test plays. No network, no GitHub, no model — except the
//! scripted one, over a loopback socket.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Local, Scratch, Shell, runner, seeded};
use epik::build::Workspace;
use epik::chat::{Answer, Ask};
use epik::feature::tools::{Builds, feature_status, start_feature};
use epik::feature::{Budget, Issue, IssueId, Node, Plan};
use epik::tools::Registry;
use serde_json::{Value, json};

fn id(number: u64) -> IssueId {
    IssueId::from(number)
}

fn node(number: u64, children: &[u64], blockers: &[u64]) -> Node {
    Node {
        issue: Issue {
            id: id(number),
            title: format!("issue {number}"),
            closed: false,
        },
        children: children.iter().copied().map(id).collect(),
        blockers: blockers
            .iter()
            .map(|&number| Issue {
                id: id(number),
                title: format!("issue {number}"),
                closed: false,
            })
            .collect(),
    }
}

fn plan(feature: u64, nodes: Vec<Node>) -> Plan {
    Plan::descend(&id(feature), |asked| {
        nodes
            .iter()
            .find(|node| &node.issue.id == asked)
            .cloned()
            .ok_or_else(|| format!("no fixture for {asked}"))
    })
    .unwrap()
}

/// One script per issue number becomes the Agent factory a build takes.
fn scripted(
    scripts: BTreeMap<u64, String>,
) -> impl Fn(&Issue, &Workspace, &str) -> Shell + Send + Sync + 'static {
    move |issue, workspace, _brief| Shell {
        script: scripts[&issue.id.0.parse::<u64>().unwrap()].clone(),
        cwd: workspace.directory.to_string_lossy().into_owned(),
    }
}

/// A script step that lands work: write the issue's file and commit it.
fn lands(number: u64) -> String {
    format!(
        "printf 'issue {number}\\n' > issue-{number}.txt && \
         git add -A && git commit -q -m 'implement issue {number}'"
    )
}

/// A script step that waits until `path` exists — bounded, so a broken
/// build fails a test instead of hanging it.
fn until(path: &str) -> String {
    format!(
        "i=0; until [ -e '{path}' ]; do i=$((i+1)); \
         if [ \"$i\" -gt 2400 ]; then exit 1; fi; sleep 0.1; done"
    )
}

/// Polls a condition until it holds, bounded in words.
fn eventually(what: &str, holds: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !holds() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The registry a turn would carry: the two feature tools over injected
/// seams — a fixture plan, a bare-directory forge, the real runner, and
/// scripted Shell Agents.
fn registry(
    builds: &Arc<Builds>,
    budget: Arc<Budget>,
    the_plan: Plan,
    remote: String,
    scripts: BTreeMap<u64, String>,
    asker: impl Fn(Ask) -> Answer + 'static,
) -> Registry {
    let mut registry = Registry::default();
    registry.register(start_feature(
        Arc::clone(builds),
        budget,
        || Ok(PathBuf::from(runner())),
        move |_, _| Ok(the_plan.clone()),
        move |_| Ok(Local(remote.clone())),
        move || Ok(scripted(scripts.clone())),
        asker,
    ));
    registry.register(feature_status(Arc::clone(builds)));
    registry
}

fn status(registry: &Registry) -> Value {
    registry.dispatch("feature_status", "{}").unwrap()
}

fn finished(registry: &Registry) -> bool {
    status(registry)["finished"] == json!(true)
}

/// Removes every linked worktree a build kept, so a scratch drop is
/// enough.
fn tidy(repository: &str) {
    let listed = epik::spawn(
        std::process::Command::new("git")
            .args(["-C", repository, "worktree", "list", "--porcelain"])
            .stdout(std::process::Stdio::piped()),
    )
    .unwrap()
    .wait_with_output()
    .unwrap();
    for line in String::from_utf8_lossy(&listed.stdout).lines() {
        if let Some(path) = line.strip_prefix("worktree ")
            && path != repository
        {
            let _ = epik::spawn(std::process::Command::new("git").args([
                "-C", repository, "worktree", "remove", "--force", "--", path,
            ]))
            .and_then(|mut git| git.wait());
        }
    }
}

fn tip(repository: &str, branch: &str) -> String {
    let verified = git2::Repository::open(repository).unwrap();
    verified
        .find_branch(branch, git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string()
}

/// The whole slice in one run: the tool answers while the Agents still
/// work, a second launch is refused naming the first, the answered
/// check judges every merge, and `feature_status` reads issues moving
/// to Merged while the feature branch reaches the remote.
#[test]
fn a_feature_builds_from_one_tool_call_and_a_second_is_refused_meanwhile() {
    let scratch = Scratch::new("feature-tool");
    let (work, forge) = seeded(&scratch);
    let remote = forge.0.clone();
    let sync = scratch.join("sync");
    std::fs::create_dir_all(&sync).unwrap();
    let go = format!("{sync}/go");

    let builds = Arc::new(Builds::default());
    let asked = Arc::new(Mutex::new(Vec::<Ask>::new()));
    let registry = registry(
        &builds,
        Budget::new(),
        plan(
            1,
            vec![node(1, &[2, 3], &[]), node(2, &[], &[]), node(3, &[], &[2])],
        ),
        remote.clone(),
        BTreeMap::from([
            (2, format!("{} && {}", until(&go), lands(2))),
            (3, lands(3)),
        ]),
        {
            let asked = Arc::clone(&asked);
            move |question| {
                asked.lock().unwrap().push(question);
                Answer::Check {
                    command: "test -f hello.txt".to_owned(),
                }
            }
        },
    );

    let started = registry
        .dispatch(
            "start_feature",
            &json!({ "repo": "o/r", "repository": work, "feature": 1 }).to_string(),
        )
        .unwrap();
    assert_eq!(started["started"], json!(true));
    assert_eq!(started["run"], json!(1));
    assert_eq!(started["branch"], "feature-1", "named for the issue");
    assert_eq!(started["base"], "main", "the repository's default branch");
    assert_eq!(started["check"], json!("test -f hello.txt"));
    assert_eq!(started["issues"], json!(2));
    assert_eq!(asked.lock().unwrap().len(), 1, "one card, machinery-raised");

    // The tool has answered while issue 2's Agent still holds: the turn
    // ends, the build proceeds.
    let running = status(&registry);
    assert_eq!(running["finished"], json!(false));

    // A second launch during the first is refused with the build named.
    let refused = registry
        .dispatch(
            "start_feature",
            &json!({ "repo": "o/r", "repository": work, "feature": 9 }).to_string(),
        )
        .unwrap_err();
    assert!(refused.contains("run 1"), "{refused}");
    assert!(refused.contains("feature 1"), "{refused}");
    assert!(refused.contains("feature-1"), "{refused}");
    assert_eq!(
        asked.lock().unwrap().len(),
        1,
        "a refused start raises no card"
    );

    std::fs::write(&go, "").unwrap();
    eventually("the feature finishes", || finished(&registry));

    let done = status(&registry);
    assert_eq!(done["run"], json!(1));
    assert_eq!(done["check"], json!("test -f hello.txt"));
    let issues = done["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2);
    for issue in issues {
        assert_eq!(issue["state"], json!("merged"), "{issue}");
        assert_eq!(issue["checked"], json!(true), "every merge was judged");
        assert!(issue["title"].as_str().unwrap().starts_with("issue "));
    }
    let landed = tip(&work, "feature-1");
    assert_eq!(done["tip"], json!(landed.clone()));
    assert_eq!(
        tip(&remote, "feature-1"),
        landed,
        "the feature branch reached the remote as issues landed"
    );

    tidy(&work);
}

/// The decline path, end to end: the build runs on observation alone,
/// merges land unchecked, and the record says so.
#[test]
fn a_declined_check_builds_unchecked_and_the_record_says_so() {
    let scratch = Scratch::new("feature-unchecked");
    let (work, forge) = seeded(&scratch);
    let builds = Arc::new(Builds::default());
    let registry = registry(
        &builds,
        Budget::new(),
        plan(1, vec![node(1, &[2], &[]), node(2, &[], &[])]),
        forge.0.clone(),
        BTreeMap::from([(2, lands(2))]),
        |_| Answer::Declined,
    );

    let started = registry
        .dispatch(
            "start_feature",
            &json!({ "repo": "o/r", "repository": work, "feature": 1 }).to_string(),
        )
        .unwrap();
    assert_eq!(started["check"], Value::Null);

    eventually("the unchecked feature finishes", || finished(&registry));
    let done = status(&registry);
    assert_eq!(done["check"], Value::Null, "the record says unchecked");
    let issues = done["issues"].as_array().unwrap();
    assert_eq!(issues[0]["state"], json!("merged"));
    assert_eq!(issues[0]["checked"], json!(false));

    tidy(&work);
}

/// The chat surface over the whole thing: a scripted model calls
/// start_feature, the turn ends in text while the build is still
/// running, and nothing an Agent said ever crossed the turn — the model
/// saw exactly one tool call and its result.
#[test]
fn a_scripted_turn_starts_a_feature_and_answers_while_it_runs() {
    use epik::chat::scripted::{Fragment, Scripted, Turn};
    use epik::chat::{Client, Role, TranscriptItem};

    let scratch = Scratch::new("feature-turn");
    let (work, forge) = seeded(&scratch);
    let sync = scratch.join("sync");
    std::fs::create_dir_all(&sync).unwrap();
    let go = format!("{sync}/go");

    let model = Scripted::spawn(vec![
        Turn::ToolCalls(vec![vec![Fragment::open(
            0,
            "call_1",
            "start_feature",
            &json!({ "repo": "o/r", "repository": work, "feature": 1 }).to_string(),
        )]]),
        Turn::text(&["Building feature 1 on feature-1."]),
    ]);
    let builds = Arc::new(Builds::default());
    let registry = registry(
        &builds,
        Budget::new(),
        plan(1, vec![node(1, &[2], &[]), node(2, &[], &[])]),
        forge.0.clone(),
        BTreeMap::from([(2, format!("{} && {}", until(&go), lands(2)))]),
        |_| Answer::Check {
            command: "true".to_owned(),
        },
    );
    let client = Client::new(model.base_url(), "scripted".to_owned(), None);
    let mut observed = Vec::new();

    let text = epik::tools::run(
        &client,
        "system",
        &[TranscriptItem::Message {
            role: Role::User,
            text: "Build feature 1.".to_owned(),
        }],
        &registry,
        |_| {},
        |call, result| observed.push((call.name.clone(), result.clone())),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();

    assert_eq!(text, "Building feature 1 on feature-1.");
    assert!(
        !finished(&registry),
        "the turn ended while the build proceeds"
    );
    assert_eq!(
        observed.len(),
        1,
        "one tool call crossed the turn; no Agent event did"
    );
    let (name, result) = &observed[0];
    assert_eq!(name, "start_feature");
    assert_eq!(result.as_ref().unwrap()["branch"], "feature-1");

    std::fs::write(&go, "").unwrap();
    eventually("the feature finishes", || finished(&registry));
    tidy(&work);
}
