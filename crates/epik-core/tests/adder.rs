//! The whole loop, end to end, with nothing real in it: a tracker read
//! from a file says what the adder library is and which issue waits on
//! which; a plan of scripted agents implements it in a worktree of a
//! bare origin; the work is committed and pushed; the worktree goes;
//! and a fresh clone holds the library.

#![cfg(all(feature = "testing", unix))]

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use epik_core::agent::Update;
use epik_core::agent::scripted::{self, Scripted, assert_written};
use epik_core::git::testing::{branch_of, cloned, committed_all, identified, pushed, seeded};
use epik_core::git::url::Url;
use epik_core::plan::implement::Narration;
use epik_core::plan::{Implement, Plan, ProcessSlots};
use epik_core::temp::Scratch;
use epik_core::tracker::IssueTracker;
use epik_core::tracker::json::JsonTracker;
use epik_core::worktrees::Worktrees;

const TRACKER: &str = include_str!("fixtures/adder/issues.json");

#[tokio::test]
async fn a_plan_of_scripted_agents_implements_the_tracked_issues_into_a_pushed_branch() {
    let scratch = Scratch::new("adder");
    let (origin, _) = seeded(&scratch);
    let url = Url::Local(origin.path().to_owned());
    let worktrees = Worktrees::new(scratch.0.join("root"));
    let tracker = JsonTracker::read(TRACKER).unwrap();
    let source = scripted::adder();

    let branch = {
        let linked = worktrees.add(&url).unwrap();
        let repository = linked.worktree().clone();

        // One task per issue, each a scripted agent writing the files
        // the issue names, in the order the tracker gives.
        let plan: Plan<Implement<Scripted>> = tracker.dependents("1").unwrap().map(|id| {
            let files = tracker
                .content(&id)
                .unwrap()
                .lines()
                .map(|path| (PathBuf::from(path), source[Path::new(path)].clone()))
                .collect();
            Implement(Scripted {
                cwd: repository.path().to_owned(),
                files,
            })
        });
        let Narration(updates) = plan
            .execute(ProcessSlots::new(NonZeroUsize::new(2).unwrap()))
            .await
            .unwrap();

        assert_written(repository.path(), &source);
        let count = |wanted: fn(&Update) -> bool| updates.iter().filter(|u| wanted(u)).count();
        assert_eq!(
            count(|u| matches!(u, Update::Session { .. })),
            3,
            "{updates:?}"
        );
        assert_eq!(
            count(|u| matches!(u, Update::ToolUse { .. })),
            4,
            "{updates:?}"
        );
        assert_eq!(
            count(|u| matches!(u, Update::Result { ok: true, .. })),
            3,
            "{updates:?}"
        );
        assert!(
            matches!(updates[0], Update::Session { .. })
                && matches!(updates[3], Update::Result { .. }),
            "the scaffolding issue's run comes first, whole: {updates:?}"
        );

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
