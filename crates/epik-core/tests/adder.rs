//! The adder library, built end to end with nothing real in it: a
//! scripted agent, or a plan of them under a tracker read from a file,
//! implements it in a worktree of a bare origin; the work is committed
//! and pushed; the worktree goes; and a fresh clone holds the library.

#![cfg(all(feature = "testing", unix))]

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use epik_core::agent::{Agent, Update};
use epik_core::git::testing::{
    branch_of, cloned, committed_all, credentials, identified, published, pushed,
};
use epik_core::git::url::Url;
use epik_core::plan::stream::received;
use epik_core::plan::{AgentSlots, Happened, Implement, Plan, Reported, Sink, Task};
use epik_core::process::Exit;
use epik_core::temp::Scratch;
use epik_core::tracker::IssueTracker;
use epik_core::worktrees::Worktrees;

mod support;
use support::json_tracker::JsonTracker;
use support::scripted::{self, Scripted, assert_written};

const TRACKER: &str = include_str!("fixtures/adder/issues.json");

fn slots(count: usize) -> AgentSlots {
    AgentSlots::new(NonZeroUsize::new(count).unwrap())
}

#[tokio::test]
async fn a_scripted_agent_runs_as_a_task() {
    let scratch = Scratch::new("implement");
    let task = Implement {
        agent: Scripted {
            cwd: scratch.0.clone(),
            files: [("note.txt".into(), "noted\n".to_owned())].into(),
        },
        slots: slots(1),
        sink: Sink::new(drop),
    };
    task.run().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("note.txt")).unwrap(),
        "noted\n"
    );
}

/// The loop with one agent and no plan: a bare origin, a worktree of
/// it, the engine writing there, a commit and a push, the worktree
/// gone with its drop, and the work back out by a fresh clone.
#[test]
fn what_an_agent_commits_in_a_worktree_is_pushed_and_clones_back() {
    let scratch = Scratch::new("worktrees-adder");
    let (bare, _) = published(&scratch);
    let url = Url::Local(bare.path().to_owned());
    let worktrees = Worktrees::new(scratch.0.join("root"));
    let linked = worktrees
        .add(&url, "adder", "origin/main", &credentials())
        .unwrap();
    let repository = linked.worktree().clone();
    let branch = branch_of(&repository);

    let files = scripted::adder();
    let engine = Scripted {
        cwd: repository.path().to_owned(),
        files: files.clone(),
    }
    .start()
    .unwrap();
    assert_eq!(engine.finish().unwrap().exit, Exit::Code(0));

    identified(&repository);
    committed_all(
        &repository,
        files.keys().cloned().collect(),
        "the adder library",
    );
    pushed(&repository, &branch);
    drop(linked);
    assert!(!repository.path().exists(), "the worktree removed itself");

    let check = cloned(&scratch, &bare, "check", Some(&branch));
    assert_written(check.path(), &files);
}

#[tokio::test]
async fn an_agent_that_exits_with_a_failure_is_the_tasks_error() {
    let scratch = Scratch::new("implement-fails");
    let task = Implement {
        agent: Scripted {
            cwd: scratch.0.join("missing"),
            files: [].into(),
        },
        slots: slots(1),
        sink: Sink::new(drop),
    };
    task.run().await.unwrap_err();
}

#[tokio::test]
async fn a_plan_of_scripted_agents_implements_the_tracked_issues_into_a_pushed_branch() {
    let scratch = Scratch::new("adder");
    let (origin, _) = published(&scratch);
    let url = Url::Local(origin.path().to_owned());
    let worktrees = Worktrees::new(scratch.0.join("root"));
    let tracker = JsonTracker::read(TRACKER).unwrap();
    let source = scripted::adder();

    let branch = {
        let linked = worktrees
            .add(&url, "adder", "origin/main", &credentials())
            .unwrap();
        let repository = linked.worktree().clone();

        // One task per issue, each a scripted agent writing the files
        // the issue names, in the order the tracker gives, and all of
        // them reporting to one sink under the issue's id.
        let root = "0".to_owned();
        let slots = slots(2);
        let (sink, mut receiver) = Sink::channel();
        let issues = tracker.work(&root).unwrap();
        let ids: Vec<String> = issues.nodes().map(|node| node.value().clone()).collect();
        let plan: Plan<Reported<Implement<Scripted>>> = issues.map(|id| {
            let files = tracker
                .content(&id)
                .unwrap()
                .lines()
                .map(|path| (PathBuf::from(path), source[Path::new(path)].clone()))
                .collect();
            let agent = Scripted {
                cwd: repository.path().to_owned(),
                files,
            };
            Reported::with(sink.node(id), |sink| Implement {
                agent,
                slots: slots.clone(),
                sink,
            })
        });
        plan.execute().await.unwrap();

        // Each issue's account, picked out of the one stream: started,
        // the agent's session to its result, finished.
        let events = received(&mut receiver);
        for id in ids {
            let happened: Vec<_> = events
                .iter()
                .filter(|event| event.node == id)
                .map(|event| &event.happened)
                .collect();
            assert!(
                matches!(
                    happened.as_slice(),
                    [
                        Happened::Started,
                        Happened::Said(Update::Session { .. }),
                        ..,
                        Happened::Said(Update::Result { ok: true, .. }),
                        Happened::Finished,
                    ]
                ),
                "issue {id}: {happened:?}"
            );
        }
        assert_written(repository.path(), &source);

        identified(&repository);
        committed_all(
            &repository,
            source.keys().cloned().collect(),
            "the adder library",
        );
        let branch = branch_of(&repository);
        pushed(&repository, &branch);
        branch
    };

    let check = cloned(&scratch, &origin, "check", Some(&branch));
    assert_written(check.path(), &source);
}
