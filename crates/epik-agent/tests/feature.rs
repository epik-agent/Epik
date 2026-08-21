//! The feature build end to end: a plan over a scratch repository,
//! scripted Agents through the real runner — located by cargo, which is
//! why these tests live in this package — and the shared record's
//! verdict. All deterministic: every Agent is an inline `sh -c` script
//! (never a script file — the ETXTBSY race), concurrency is pinned with
//! marker files rather than timing, and every wait is a bounded poll on
//! a condition, not a sleep-and-hope.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Scratch, Shell, git, runner, seeded, tidy};
use epik::build::Workspace;
use epik::feature::merge::Branch;
use epik::feature::{self, Budget, Build, Issue, IssueId, Node, Plan, State};

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
/// build fails a test instead of hanging it, and bounded well past
/// [`eventually`]'s budget, so a slow CI runner times out there in
/// words rather than fabricating an Agent failure here.
fn until(path: &str) -> String {
    format!(
        "i=0; until [ -e '{path}' ]; do i=$((i+1)); \
         if [ \"$i\" -gt 2400 ]; then exit 1; fi; sleep 0.1; done"
    )
}

/// Polls a condition on the record until it holds, bounded in words.
/// The budget stays comfortably under [`until`]'s 240 seconds.
fn eventually(what: &str, holds: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !holds() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn snapshot(record: &Arc<Mutex<Build>>) -> Build {
    record.lock().unwrap().clone()
}

fn state(record: &Arc<Mutex<Build>>, number: u64) -> State {
    snapshot(record).states[&id(number)].clone()
}

fn counted(record: &Arc<Mutex<Build>>, wanted: &State) -> usize {
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
        Arc::new(branch),
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
        Budget::new(),
    );

    eventually("the diamond finishes", || snapshot(&record).finished());
    let build = snapshot(&record);
    assert!(build.problems.is_empty());
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
        Arc::new(branch),
        runner(),
        scripted(BTreeMap::from([
            (2, lands(2)),
            (3, link(3)),
            (4, link(4)),
            (5, link(5)),
            (6, link(6)),
        ])),
        Budget::new(),
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
        Arc::new(branch),
        runner(),
        scripted((2..=7).map(|number| (number, held(number))).collect()),
        Budget::new(),
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
/// Skipped with the blocker it is stuck on named, and its independent
/// sibling still lands.
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
        Arc::new(branch),
        runner(),
        scripted(BTreeMap::from([
            (2, "echo 'it all went wrong' >&2; exit 3".to_owned()),
            (3, "exit 9".to_owned()), // must never be dispatched
            (4, lands(4)),
            (5, "exit 9".to_owned()), // must never be dispatched
        ])),
        Budget::new(),
    );

    eventually("the build finishes around the failure", || {
        snapshot(&record).finished()
    });
    let build = snapshot(&record);
    let State::Failed { report } = &build.states[&id(2)] else {
        panic!("2 should have failed: {:?}", build.states);
    };
    assert!(report.contains("exited with code 3"), "{report}");
    let State::Skipped { reason } = &build.states[&id(3)] else {
        panic!("3 waited on 2: {:?}", build.states);
    };
    assert!(reason.contains("waits on 2"), "{reason}");
    let State::Skipped { reason } = &build.states[&id(5)] else {
        panic!("5 waited on 3: {:?}", build.states);
    };
    assert!(
        reason.contains("waits on 3"),
        "doom is transitive: {reason}"
    );
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

/// The Agent factory is caller code and may panic; the worker catches
/// it, the issue fails with the panic's words, the slot comes back, and
/// the rest of the plan still builds to the end. The worktree
/// provisioned for the Agent that never came is removed, and its branch
/// with it.
#[test]
fn a_panicking_agent_factory_fails_its_issue_and_the_build_still_ends() {
    let scratch = Scratch::new("panic");
    let (work, forge) = seeded(&scratch);
    let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();
    let feature_workspace = branch.workspace().directory.clone();
    let provisioned = Arc::new(Mutex::new(None));

    let record = feature::build(
        plan(
            1,
            vec![
                node(1, false, &[2, 3], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[]),
            ],
        ),
        Arc::new(branch),
        runner(),
        {
            let provisioned = Arc::clone(&provisioned);
            move |issue: &Issue, workspace: &Workspace, _brief: &str| {
                if issue.id.0 == "3" {
                    *provisioned.lock().unwrap() = Some(workspace.directory.clone());
                    panic!("the factory had no Agent for issue 3");
                }
                Shell {
                    script: lands(2),
                    cwd: workspace.directory.to_string_lossy().into_owned(),
                }
            }
        },
        Budget::new(),
    );

    eventually("the build survives the panic", || {
        snapshot(&record).finished()
    });
    let build = snapshot(&record);
    let State::Failed { report } = &build.states[&id(3)] else {
        panic!("the panic becomes a failure: {:?}", build.states);
    };
    assert!(report.contains("the worker panicked"), "{report}");
    assert!(report.contains("no Agent for issue 3"), "{report}");
    assert!(
        matches!(build.states[&id(2)], State::Merged { .. }),
        "{:?}",
        build.states
    );
    let orphan = provisioned
        .lock()
        .unwrap()
        .clone()
        .expect("issue 3 was provisioned");
    assert!(
        !orphan.exists(),
        "the worktree for the Agent that never came is removed"
    );
    assert!(
        git2::Repository::open(&work)
            .unwrap()
            .find_branch("issue/3", git2::BranchType::Local)
            .is_err(),
        "and its branch with it"
    );

    tidy(&work, &feature_workspace);
}

/// A second build over the same repository, after a failure: the issue
/// branch the first run left standing is a corpse the rerun supersedes,
/// so the retry builds instead of insta-failing on `worktree add -b`.
#[test]
fn a_second_build_over_the_same_repository_supersedes_the_first_runs_branches() {
    let scratch = Scratch::new("rerun");
    let (work, forge) = seeded(&scratch);
    let remote = forge.0.clone();
    let nodes = || vec![node(1, false, &[2], &[]), node(2, false, &[], &[])];

    let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();
    let feature_workspace = branch.workspace().directory.clone();
    let record = feature::build(
        plan(1, nodes()),
        Arc::new(branch),
        runner(),
        scripted(BTreeMap::from([(2, "exit 3".to_owned())])),
        Budget::new(),
    );
    eventually("the first build fails", || snapshot(&record).finished());
    assert!(matches!(state(&record, 2), State::Failed { .. }));
    tidy(&work, &feature_workspace);

    // The rerun: the standing feature branch is adopted as it stands,
    // and the standing issue/2 branch is deleted at dispatch.
    git(&["-C", &work, "rev-parse", "refs/heads/issue/2"]);
    let branch =
        Branch::establish(&work, "feature/wumpus", "main", common::Local(remote), None).unwrap();
    let feature_workspace = branch.workspace().directory.clone();
    let record = feature::build(
        plan(1, nodes()),
        Arc::new(branch),
        runner(),
        scripted(BTreeMap::from([(2, lands(2))])),
        Budget::new(),
    );
    eventually("the rerun lands", || snapshot(&record).finished());
    assert!(
        matches!(state(&record, 2), State::Merged { .. }),
        "{:?}",
        snapshot(&record).states
    );
    assert!(on_branch(&work, "feature/wumpus", "issue-2.txt"));

    tidy(&work, &feature_workspace);
}

/// The budget is shared: a slot claimed outside the feature build — a
/// plain build's, in the app — leaves it three Agents, and releasing
/// the slot wakes the scheduler for the fourth. Marker files pin the
/// count at rest, exactly as the slots test does.
#[test]
fn a_slot_claimed_outside_the_build_leaves_it_three_agents_until_released() {
    let scratch = Scratch::new("budget");
    let (work, forge) = seeded(&scratch);
    let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();
    let feature_workspace = branch.workspace().directory.clone();
    let sync = scratch.join("sync");
    std::fs::create_dir_all(&sync).unwrap();

    let started = || std::fs::read_dir(&sync).unwrap().count();
    let held = |mine: u64| {
        format!(
            "touch '{sync}/started-{mine}' && {} && {}",
            until(&format!("{sync}/go-{mine}")),
            lands(mine)
        )
    };
    let budget = Budget::new();
    let claimed = budget.claim().expect("a fresh budget has slots");
    let record = feature::build(
        plan(
            1,
            vec![
                node(1, false, &[2, 3, 4, 5], &[]),
                node(2, false, &[], &[]),
                node(3, false, &[], &[]),
                node(4, false, &[], &[]),
                node(5, false, &[], &[]),
            ],
        ),
        Arc::new(branch),
        runner(),
        scripted((2..=5).map(|number| (number, held(number))).collect()),
        std::sync::Arc::clone(&budget),
    );

    eventually("three Agents start", || started() >= 3);
    assert_eq!(
        counted(&record, &State::Running),
        3,
        "one slot is spoken for"
    );
    assert_eq!(
        started(),
        3,
        "no fourth Agent while the outside claim holds"
    );

    // The outside claim ends — a plain build finished — and the freed
    // slot wakes the scheduler for the fourth issue.
    drop(claimed);
    eventually("the fourth Agent starts", || started() >= 4);

    for number in 2..=5 {
        std::fs::write(format!("{sync}/go-{number}"), "").unwrap();
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
