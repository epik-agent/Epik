//! An issue's branch, pushed as a task: to the URL its worktree was
//! cloned from, as the holder of the credentials it was cloned with.

use anyhow::Result;

use super::ProcessSlots;
use super::task::Task;
use crate::git::askpass::Credentials;
use crate::git::commands::{self, Command, Remote};
use crate::git::{self, Repository};

/// `branch` of `worktree` pushed to its origin when the task runs,
/// [as](git::run_as) the holder of `credentials`, in one of the
/// `slots` for as long as git runs. A push that fails is the task's
/// failure, in git's own words.
#[derive(Clone, Debug)]
pub struct Push {
    pub worktree: Repository,
    pub branch: String,
    pub credentials: Credentials,
    pub slots: ProcessSlots,
}

impl Task for Push {
    async fn run(&self) -> Result<()> {
        let push =
            commands::Push::new(self.worktree.clone(), Remote::origin(), self.branch.clone())?;
        let (push, credentials) = (Command::Push(push), self.credentials.clone());
        self.slots
            .run(move || git::run_as(&push, &credentials).map(drop))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use super::*;
    use crate::git::commands::RevParse;
    use crate::git::testing::{OFFERED, PATIENCE, aimed, committed, credentials, guarded, seeded};
    use crate::git::url::Url;
    use crate::temp::Scratch;
    use crate::worktrees::{LinkedWorktree, Worktrees};

    /// A seeded origin, and a worktree of it on `issue-1` with a
    /// commit of its own.
    fn worked(scratch: &Scratch) -> (Repository, LinkedWorktree) {
        let (origin, _) = seeded(scratch);
        let url = Url::Local(origin.path().to_owned());
        let linked = Worktrees::new(scratch.join("root").into())
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap();
        committed(linked.worktree(), "work.txt", "work\n", "the work");
        (origin, linked)
    }

    fn push(linked: &LinkedWorktree) -> Push {
        Push {
            worktree: linked.worktree().clone(),
            branch: linked.branch().to_owned(),
            credentials: credentials(),
            slots: ProcessSlots::new(NonZeroUsize::MIN),
        }
    }

    #[tokio::test]
    async fn the_branch_reaches_the_origin_at_the_worktrees_head() {
        let scratch = Scratch::new("push-branch");
        let (origin, linked) = worked(&scratch);

        push(&linked).run().await.unwrap();

        let tip = |repository: &Repository, revision: &str| {
            git::rev_parse(RevParse::new(repository.clone(), revision.to_owned()).unwrap()).unwrap()
        };
        assert_eq!(tip(&origin, "issue-1"), tip(linked.worktree(), "HEAD"));
    }

    #[tokio::test]
    async fn a_push_that_fails_is_the_tasks_failure_in_gits_own_words() {
        let scratch = Scratch::new("push-nowhere");
        let (origin, linked) = worked(&scratch);
        std::fs::remove_dir_all(origin.path()).unwrap();

        let error = push(&linked).run().await.unwrap_err().to_string();

        assert!(error.contains("origin.git"), "{error}");
        assert!(
            error.contains("not appear to be a git repository"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn the_credentials_reach_git_through_askpass() {
        let scratch = Scratch::new("push-token");
        let (_, linked) = worked(&scratch);
        let (url, seen) = guarded();
        aimed(linked.worktree(), url);

        let error = push(&linked).run().await.unwrap_err().to_string();

        assert!(!error.contains("sesame"), "{error}");
        assert_eq!(seen.recv_timeout(PATIENCE).as_deref(), Ok(OFFERED));
    }
}
