//! An issue's branch, pushed as a task: to the URL its worktree was
//! cloned from, as the holder of the credentials it was cloned with.

use anyhow::Result;

use super::briefly;
use super::github::CLASSIC;
use super::task::Task;
use crate::git::askpass::Credentials;
use crate::git::commands::{self, Command};
use crate::git::{self, Repository};

/// What GitHub says of a push its rules turn away, whether the rule is
/// a ruleset's or classic branch protection's. Said in either case
/// where GitHub happens to capitalize it.
const PROTECTED: &str = "protected branch";

/// `refused`, git's own words on a push that failed, with [`CLASSIC`]
/// over it when GitHub turned the push away for a protected branch.
fn whence(refused: anyhow::Error) -> anyhow::Error {
    match refused.to_string().to_lowercase().contains(PROTECTED) {
        true => refused.context(CLASSIC),
        false => refused,
    }
}

/// `branch` of `worktree` pushed to its origin when the task runs,
/// [as](git::run_as) the holder of `credentials`. The push is one git,
/// short enough to go straight to the blocking pool: a
/// [slot](super::AgentSlots) is an agent's. A push that fails is the
/// task's failure, in git's own words.
///
/// The branch is cut afresh each run, so a run that failed after its
/// push leaves one standing that the next cut does not descend from.
/// The push is [over](commands::Push::over) that branch, under a lease
/// on the origin's `branch` as the clone's own fetch last saw it: a
/// rerun replaces what it left behind, and a branch someone else moved
/// since is refused as `stale info` rather than overwritten.
///
/// A push GitHub turns away for a protected branch carries [`CLASSIC`]:
/// the launch reads rulesets, and a classic rule it never saw is
/// enforced here.
#[derive(Clone, Debug)]
pub struct Push {
    pub worktree: Repository,
    pub branch: String,
    pub credentials: Credentials,
}

impl Task for Push {
    async fn run(&self) -> Result<()> {
        let push = commands::Push::over(self.worktree.clone(), self.branch.clone())?;
        let (push, credentials) = (Command::Push(push), self.credentials.clone());
        briefly(move || git::run_as(&push, &credentials).map(drop))
            .await
            .map_err(whence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::commands::{Branch, Hash, RevParse, Switch};
    use crate::git::testing::{
        OFFERED, PATIENCE, aimed, committed, credentials, guarded, published, pushed,
    };
    use crate::git::url::Url;
    use crate::git::{branch, switch};
    use crate::temp::Scratch;
    use crate::worktrees::{LinkedWorktree, Worktrees};

    /// The commit `revision` names in `repository`.
    fn rev(repository: &Repository, revision: &str) -> Result<Hash> {
        git::rev_parse(RevParse::new(repository.clone(), revision.to_owned())?)
    }

    /// A published origin, and a worktree of it on `issue-1` with a
    /// commit of its own.
    fn worked(scratch: &Scratch) -> (Repository, LinkedWorktree) {
        let (origin, _) = published(scratch);
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
        }
    }

    #[tokio::test]
    async fn the_branch_reaches_the_origin_at_the_worktrees_head() {
        let scratch = Scratch::new("push-branch");
        let (origin, linked) = worked(&scratch);

        push(&linked).run().await.unwrap();

        assert_eq!(
            rev(&origin, "issue-1").unwrap(),
            rev(linked.worktree(), "HEAD").unwrap()
        );
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

    /// A commit on `issue-1` in `work`, pushed, `work` left back on
    /// `main`: `issue-1` standing in the origin on a history of its
    /// own, as a run that failed after its push leaves it. The first
    /// call cuts the branch.
    fn left_pushed(work: &Repository, name: &str) -> Hash {
        if rev(work, "issue-1").is_err() {
            branch(Branch::new(work.clone(), "issue-1".to_owned(), None).unwrap()).unwrap();
        }
        switch(Switch::new(work.clone(), "issue-1".to_owned()).unwrap()).unwrap();
        let tip = committed(work, name, name, name);
        pushed(work, "issue-1");
        switch(Switch::new(work.clone(), "main".to_owned()).unwrap()).unwrap();
        tip
    }

    /// A run of `issue-1` under `scratch`'s one root: a worktree cut
    /// from `main` as the origin stands now, and a commit of its own
    /// named `work`.
    fn rerun(scratch: &Scratch, origin: &Repository, work: &str) -> LinkedWorktree {
        let url = Url::Local(origin.path().to_owned());
        let linked = Worktrees::new(scratch.join("root").into())
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap();
        committed(linked.worktree(), work, work, work);
        linked
    }

    #[tokio::test]
    async fn a_rerun_pushes_over_the_branch_an_earlier_run_left_standing() {
        let scratch = Scratch::new("push-rerun");
        let (origin, work) = published(&scratch);
        let earlier = left_pushed(&work, "earlier.txt");
        let linked = rerun(&scratch, &origin, "work.txt");

        push(&linked).run().await.unwrap();

        assert_eq!(
            rev(&origin, "issue-1").unwrap(),
            rev(linked.worktree(), "HEAD").unwrap()
        );
        assert_ne!(rev(&origin, "issue-1").unwrap(), earlier);
    }

    /// A rerun in the same root, where the clone is reused and only
    /// fetched: the lease is worth what that fetch read.
    #[tokio::test]
    async fn a_run_after_a_run_that_pushed_and_failed_pushes_over_it() {
        let scratch = Scratch::new("push-again");
        let (origin, _) = published(&scratch);
        let first = rerun(&scratch, &origin, "first.txt");
        push(&first).run().await.unwrap();
        // The run failed past its push; its worktree goes back.
        drop(first);

        let second = rerun(&scratch, &origin, "second.txt");
        push(&second).run().await.unwrap();

        assert_eq!(
            rev(&origin, "issue-1").unwrap(),
            rev(second.worktree(), "HEAD").unwrap()
        );
    }

    #[tokio::test]
    async fn a_branch_moved_by_someone_else_refuses_in_gits_own_words() {
        let scratch = Scratch::new("push-lease");
        let (origin, work) = published(&scratch);
        left_pushed(&work, "earlier.txt");
        let linked = rerun(&scratch, &origin, "work.txt");
        // Someone other than Epik moves the branch, after the fetch the
        // rerun's worktree was cut behind.
        let theirs = left_pushed(&work, "theirs.txt");

        let error = push(&linked).run().await.unwrap_err().to_string();

        assert!(error.contains("stale info"), "{error}");
        assert_eq!(
            rev(&origin, "issue-1").unwrap(),
            theirs,
            "the branch stands where they left it"
        );
    }

    #[test]
    fn a_push_refused_for_a_protected_branch_says_a_classic_rule_may_be_the_cause() {
        let refused = anyhow::anyhow!(
            "remote: error: GH006: Protected branch update failed for refs/heads/issue-1.\n\
             ! [remote rejected] issue-1 -> issue-1 (protected branch hook declined)"
        );
        let words = format!("{:#}", whence(refused));
        assert!(words.contains("classic branch protection"), "{words}");
        assert!(words.contains("GH006"), "and git's own words: {words}");
    }

    #[test]
    fn a_push_refused_for_anything_else_is_gits_own_words_alone() {
        let refused = anyhow::anyhow!("! [rejected] issue-1 -> issue-1 (stale info)");
        let words = format!("{:#}", whence(refused));
        assert_eq!(words, "! [rejected] issue-1 -> issue-1 (stale info)");
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
