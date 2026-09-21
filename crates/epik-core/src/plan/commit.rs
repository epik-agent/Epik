//! What a worktree holds, committed as a task: whether an agent did
//! any work is read from git, and the commit that carries the work is
//! Epik's to make.

use std::fmt::{self, Display, Formatter};

use anyhow::Result;

use super::ProcessSlots;
use super::task::Task;
use crate::git::commands::{self, Add, Hash, RevParse, Status};
use crate::git::{self, Repository};

/// Everything `worktree` holds, staged and committed under `message`
/// when the task runs, by the identity the worktree was provisioned
/// with. Each git is a process of its own, in one of the `slots` for
/// as long as it runs.
#[derive(Clone, Debug)]
pub struct Commit {
    pub worktree: Repository,
    pub message: String,
    pub slots: ProcessSlots,
}

/// A worktree that is as its branch's tip left it: the fact, for
/// whoever asked for a commit to tell from a failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NothingToCommit;

impl Display for NothingToCommit {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("the worktree holds nothing to commit")
    }
}

impl std::error::Error for NothingToCommit {}

impl Commit {
    /// The commit, made: its hash, as git reads the branch's new tip
    /// back.
    ///
    /// # Errors
    ///
    /// [`NothingToCommit`] for a clean worktree, a message git would
    /// refuse, and git's own words.
    pub async fn commit(&self) -> Result<Hash> {
        let worktree = &self.worktree;
        let status = Status::new(worktree.clone());
        let add = Add::new(worktree.clone(), vec![".".into()])?;
        let commit = commands::Commit::new(worktree.clone(), self.message.clone())?;
        let head = RevParse::new(worktree.clone(), "HEAD".to_owned())?;

        if self.slots.run(|| git::status(status)).await?.is_empty() {
            return Err(NothingToCommit.into());
        }
        self.slots.run(|| git::add(add)).await?;
        self.slots.run(|| git::commit(commit)).await?;
        self.slots.run(|| git::rev_parse(head)).await
    }
}

impl Task for Commit {
    async fn run(&self) -> Result<()> {
        self.commit().await.map(drop)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::num::NonZeroUsize;

    use super::*;
    use crate::git::commands::Show;
    use crate::git::testing::{head, seeded};
    use crate::git::url::Url;
    use crate::temp::Scratch;
    use crate::worktrees::{LinkedWorktree, Worktrees};

    /// A worktree on `issue-1` of a seeded origin, provisioned as an
    /// issue's is.
    fn provisioned(scratch: &Scratch) -> LinkedWorktree {
        let (origin, _) = seeded(scratch);
        let url = Url::Local(origin.path().to_owned());
        Worktrees::new(scratch.join("root").into())
            .add(&url, "issue-1", "origin/main")
            .unwrap()
    }

    fn commit(linked: &LinkedWorktree) -> Commit {
        Commit {
            worktree: linked.worktree().clone(),
            message: "feat: the work (#1)".to_owned(),
            slots: ProcessSlots::new(NonZeroUsize::MIN),
        }
    }

    #[tokio::test]
    async fn edits_an_agent_left_uncommitted_become_a_commit_by_epik() {
        let scratch = Scratch::new("commit-work");
        let linked = provisioned(&scratch);
        let root = linked.worktree().path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/new.txt"), "new\n").unwrap();
        std::fs::remove_file(root.join("hello.txt")).unwrap();

        let made = commit(&linked).commit().await.unwrap();

        assert_eq!(made, head(linked.worktree()));
        let status = git::status(Status::new(linked.worktree().clone())).unwrap();
        assert_eq!(status, [], "the commit carries everything");
        let shown =
            git::show(Show::new(linked.worktree().clone(), "HEAD".to_owned()).unwrap()).unwrap();
        for said in [
            "Author: Epik <epik@epik-agent.dev>",
            "feat: the work (#1)",
            "+++ b/src/new.txt",
            "--- a/hello.txt",
        ] {
            assert!(shown.contains(said), "{said:?} in {shown}");
        }
    }

    #[tokio::test]
    async fn a_worktree_left_clean_reports_nothing_to_commit() {
        let scratch = Scratch::new("commit-clean");
        let linked = provisioned(&scratch);
        let before = head(linked.worktree());

        let error = commit(&linked).run().await.unwrap_err();

        assert!(error.is::<NothingToCommit>(), "{error}");
        assert_eq!(error.to_string(), "the worktree holds nothing to commit");
        assert_eq!(head(linked.worktree()), before);
    }

    #[tokio::test]
    async fn a_commit_needs_a_message_before_any_git_runs() {
        let scratch = Scratch::new("commit-wordless");
        let linked = provisioned(&scratch);
        std::fs::write(linked.worktree().path().join("new.txt"), "new\n").unwrap();
        let wordless = Commit {
            message: " ".to_owned(),
            ..commit(&linked)
        };

        let error = wordless.run().await.unwrap_err().to_string();

        assert_eq!(error, "a commit needs a message");
        let status = git::status(Status::new(linked.worktree().clone())).unwrap();
        assert_eq!(status[0].staged, '?', "nothing was staged");
    }
}
