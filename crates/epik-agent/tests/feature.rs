//! The feature build end to end: a plan over a scratch repository,
//! scripted Agents through the real runner — located by cargo, which is
//! why these tests live in this package — and the shared record's
//! verdict. All deterministic: every Agent is an inline `sh -c` script
//! (never a script file — the ETXTBSY race), concurrency is pinned with
//! marker files rather than timing, and every wait is a bounded poll on
//! a condition, not a sleep-and-hope.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use epik::agent::{Agent, Task};
use epik::build::Workspace;
use epik::feature::merge::Branch;
use epik::feature::{self, Build, Issue, IssueId, Node, Plan, State};
use epik::forge::{Credentials, Forge};

fn runner() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_epik-agent"))
}

/// A forge for tests: a bare directory, no credentials.
#[derive(Debug)]
struct Local(String);

impl Forge for Local {
    fn remote(&self) -> String {
        self.0.clone()
    }

    fn credentials(&self) -> Option<Credentials> {
        None
    }
}

/// A scratch directory that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "epik-feature-{name}-{}-{}",
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

fn git(args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A working repository with one commit on main, and a bare remote for
/// the forge.
fn seeded(scratch: &Scratch) -> (String, Local) {
    let work = scratch.join("work");
    let remote = scratch.join("remote.git");
    git(&["init", "--initial-branch=main", &work]);
    git(&["-C", &work, "config", "user.name", "Test"]);
    git(&["-C", &work, "config", "user.email", "test@example.com"]);
    git(&["-C", &work, "config", "commit.gpgsign", "false"]);
    std::fs::write(Path::new(&work).join("hello.txt"), "hello\n").unwrap();
    git(&["-C", &work, "add", "hello.txt"]);
    git(&["-C", &work, "commit", "-m", "the first commit"]);
    git(&["init", "--bare", "--initial-branch=main", &remote]);
    (work, Local(remote))
}

fn id(number: u64) -> IssueId {
    IssueId::from(number)
}

fn issue(number: u64, closed: bool) -> Issue {
    Issue {
        id: id(number),
        title: format!("issue {number}"),
        closed,
    }
}

fn node(number: u64, closed: bool, children: &[u64], blockers: &[(u64, bool)]) -> Node {
    Node {
        issue: issue(number, closed),
        children: children.iter().copied().map(id).collect(),
        blockers: blockers
            .iter()
            .map(|&(number, closed)| issue(number, closed))
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

/// The scripted Agent: `sh -c` of an inline script, in the workspace.
struct Shell {
    script: String,
    cwd: String,
}

impl Agent for Shell {
    fn task(&self) -> Task {
        Task {
            argv: vec!["sh".to_owned(), "-c".to_owned(), self.script.clone()],
            env: Vec::new(),
            cwd: self.cwd.clone(),
            stdin: None,
        }
    }
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

/// A script step that waits — bounded, so a broken build fails a test
/// instead of hanging it — until `path` exists.
fn until(path: &str) -> String {
    format!(
        "i=0; until [ -e '{path}' ]; do i=$((i+1)); \
         if [ \"$i\" -gt 600 ]; then exit 1; fi; sleep 0.1; done"
    )
}

/// Polls a condition on the record until it holds, bounded in words.
fn eventually(what: &str, holds: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !holds() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn snapshot(record: &std::sync::Arc<std::sync::Mutex<Build>>) -> Build {
    record.lock().unwrap().clone()
}

fn state(record: &std::sync::Arc<std::sync::Mutex<Build>>, number: u64) -> State {
    snapshot(record).states[&id(number)].clone()
}

fn counted(record: &std::sync::Arc<std::sync::Mutex<Build>>, wanted: &State) -> usize {
    snapshot(record)
        .states
        .values()
        .filter(|state| std::mem::discriminant(*state) == std::mem::discriminant(wanted))
        .count()
}

/// The feature branch's tip holds (or lacks) a file, by git2 — the
/// independent implementation.
fn on_branch(repository: &str, branch: &str, file: &str) -> bool {
    let verified = git2::Repository::open(repository).unwrap();
    verified
        .find_branch(branch, git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap()
        .get_name(file)
        .is_some()
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

/// Removes the feature workspace a build kept for itself.
fn tidy(repository: &str, workspace: &Path) {
    let _ = std::process::Command::new("git")
        .args([
            "-C",
            repository,
            "worktree",
            "remove",
            "--force",
            "--",
            workspace.to_str().unwrap(),
        ])
        .status();
}

/// The diamond: 2 at the top, 3 and 4 in the middle, 5 at the tail. The
/// middles rendezvous on marker files — each proceeds only once both
/// have started, so the test deadlocks into a bounded failure unless
/// they truly run at once — and each asserts the top's work is already
/// in its worktree, pinning the cut-at-dispatch. The tail asserts both
/// middles' work, pinning that it ran only after both landed.
#[test]
fn a_diamond_runs_its_middles_at_once_and_its_tail_only_after_both_land() {
    let scratch = Scratch::new("diamond");
    let (work, forge) = seeded(&scratch);
    let remote = forge.0.clone();
    let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();
    let feature_workspace = branch.workspace().directory.clone();
    let sync = scratch.join("sync");
    std::fs::create_dir_all(&sync).unwrap();

    let middle = |mine: u64, sibling: u64| {
        format!(
            "touch '{sync}/started-{mine}' && {} && test -f issue-2.txt && {}",
            until(&format!("{sync}/started-{sibling}")),
            lands(mine)
        )
    };
    let record = feature::build(
        plan(
            1,
            vec![
                node(1, false, &[2, 3, 4, 5], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
                node(4, false, &[], &[(2, false)]),
                node(5, false, &[], &[(3, false), (4, false)]),
            ],
        ),
        branch,
        runner(),
        scripted(BTreeMap::from([
            (2, lands(2)),
            (3, middle(3, 4)),
            (4, middle(4, 3)),
            (
                5,
                format!("test -f issue-3.txt && test -f issue-4.txt && {}", lands(5)),
            ),
        ])),
    );

    eventually("the diamond finishes", || snapshot(&record).finished());
    let build = snapshot(&record);
    for number in [2, 3, 4, 5] {
        assert!(
            matches!(
                build.states[&id(number)],
                State::Merged { checked: false, .. }
            ),
            "issue {number} should have landed unchecked: {:?}",
            build.states
        );
        assert!(on_branch(
            &work,
            "feature/wumpus",
            &format!("issue-{number}.txt")
        ));
        assert!(
            build.runs.contains_key(&id(number)),
            "the record is the sink"
        );
    }
    assert_eq!(
        tip(&work, "feature/wumpus"),
        tip(&remote, "feature/wumpus"),
        "the remote kept pace"
    );

    tidy(&work, &feature_workspace);
}

/// The chain: each issue blocked by its predecessor. Every Agent
/// asserts its predecessor's file is already in the worktree it was
/// given, so the run pins both one-at-a-time dispatch and the branch
/// cut from the tip the predecessor just advanced.
#[test]
fn a_chain_of_five_runs_one_at_a_time_each_from_its_predecessors_tip() {
    let scratch = Scratch::new("chain");
    let (work, forge) = seeded(&scratch);
    let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();
    let feature_workspace = branch.workspace().directory.clone();

    let link = |mine: u64| format!("test -f issue-{}.txt && {}", mine - 1, lands(mine));
    let record = feature::build(
        plan(
            1,
            vec![
                node(1, false, &[2, 3, 4, 5, 6], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
                node(4, false, &[], &[(3, false)]),
                node(5, false, &[], &[(4, false)]),
                node(6, false, &[], &[(5, false)]),
            ],
        ),
        branch,
        runner(),
        scripted(BTreeMap::from([
            (2, lands(2)),
            (3, link(3)),
            (4, link(4)),
            (5, link(5)),
            (6, link(6)),
        ])),
    );

    eventually("the chain finishes", || snapshot(&record).finished());
    let build = snapshot(&record);
    for number in [2, 3, 4, 5, 6] {
        assert!(
            matches!(build.states[&id(number)], State::Merged { .. }),
            "issue {number}: {:?}",
            build.states
        );
        assert!(on_branch(
            &work,
            "feature/wumpus",
            &format!("issue-{number}.txt")
        ));
    }

    tidy(&work, &feature_workspace);
}

/// Six ready issues, four slots. Every Agent announces itself with a
/// marker file and holds until released, so the test observes the slot
/// budget at rest: four Running, two Waiting, four marker files.
/// Releasing one issue frees exactly one slot, and a fifth Agent starts
/// while three siblings are still held — the refill waits on no round
/// barrier.
#[test]
fn a_plan_with_more_ready_issues_than_slots_never_runs_more_than_four_agents() {
    let scratch = Scratch::new("slots");
    let (work, forge) = seeded(&scratch);
    let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();
    let feature_workspace = branch.workspace().directory.clone();
    let sync = scratch.join("sync");
    std::fs::create_dir_all(&sync).unwrap();

    let started = || -> Vec<u64> {
        let mut started: Vec<u64> = std::fs::read_dir(&sync)
            .unwrap()
            .filter_map(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix("started-"))
                    .and_then(|number| number.parse().ok())
            })
            .collect();
        started.sort_unstable();
        started
    };
    let release = |number: u64| std::fs::write(format!("{sync}/go-{number}"), "").unwrap();

    let held = |mine: u64| {
        format!(
            "touch '{sync}/started-{mine}' && {} && {}",
            until(&format!("{sync}/go-{mine}")),
            lands(mine)
        )
    };
    let record = feature::build(
        plan(
            1,
            vec![
                node(1, false, &[2, 3, 4, 5, 6, 7], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[]),
                node(4, false, &[], &[]),
                node(5, false, &[], &[]),
                node(6, false, &[], &[]),
                node(7, false, &[], &[]),
            ],
        ),
        branch,
        runner(),
        scripted((2..=7).map(|number| (number, held(number))).collect()),
    );

    eventually("four Agents start", || started().len() == 4);
    assert_eq!(counted(&record, &State::Running), 4, "every slot is taken");
    assert_eq!(counted(&record, &State::Waiting), 2, "two wait for a slot");
    assert_eq!(
        started().len(),
        4,
        "no fifth Agent while the slots are full"
    );

    // Release one: its slot refills at once, while its three siblings
    // still hold theirs — no round barrier.
    let first = started()[0];
    release(first);
    eventually("the freed slot refills", || started().len() == 5);
    eventually("the released issue lands", || {
        matches!(state(&record, first), State::Merged { .. })
    });
    assert_eq!(counted(&record, &State::Running), 4);
    assert_eq!(
        counted(
            &record,
            &State::Merged {
                commit: String::new(),
                checked: false
            }
        ),
        1
    );
    assert_eq!(counted(&record, &State::Waiting), 1);
    assert_eq!(started().len(), 5, "one slot freed, one Agent started");

    for number in 2..=7 {
        release(number);
    }
    eventually("the whole plan lands", || snapshot(&record).finished());
    let build = snapshot(&record);
    assert!(
        build
            .states
            .values()
            .all(|state| matches!(state, State::Merged { .. })),
        "{:?}",
        build.states
    );

    tidy(&work, &feature_workspace);
}

/// One issue fails; what waited on it — directly and transitively — is
/// Skipped, and its independent sibling still lands.
#[test]
fn a_failed_issue_leaves_its_dependents_skipped_and_its_siblings_merged() {
    let scratch = Scratch::new("failure");
    let (work, forge) = seeded(&scratch);
    let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();
    let feature_workspace = branch.workspace().directory.clone();

    let record = feature::build(
        plan(
            1,
            vec![
                node(1, false, &[2, 3, 4, 5], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[(2, false)]),
                node(4, false, &[], &[]),
                node(5, false, &[], &[(3, false)]),
            ],
        ),
        branch,
        runner(),
        scripted(BTreeMap::from([
            (2, "echo 'it all went wrong' >&2; exit 3".to_owned()),
            (3, "exit 9".to_owned()), // must never be dispatched
            (4, lands(4)),
            (5, "exit 9".to_owned()), // must never be dispatched
        ])),
    );

    eventually("the build finishes around the failure", || {
        snapshot(&record).finished()
    });
    let build = snapshot(&record);
    let State::Failed { report } = &build.states[&id(2)] else {
        panic!("2 should have failed: {:?}", build.states);
    };
    assert!(report.contains("exited with code 3"), "{report}");
    assert_eq!(build.states[&id(3)], State::Skipped);
    assert_eq!(build.states[&id(5)], State::Skipped, "doom is transitive");
    assert!(
        matches!(build.states[&id(4)], State::Merged { .. }),
        "the independent sibling still lands: {:?}",
        build.states
    );
    assert!(on_branch(&work, "feature/wumpus", "issue-4.txt"));
    assert!(
        !build.runs.contains_key(&id(3)) && !build.runs.contains_key(&id(5)),
        "a skipped issue never got an Agent"
    );

    tidy(&work, &feature_workspace);
}
