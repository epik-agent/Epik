//! What a worktree holds, committed as a task: whether an agent did
//! any work is read from git, and the commit that carries the work is
//! Epik's to make.

use std::fmt::{self, Display, Formatter};

use anyhow::{Result, bail};

use super::briefly;
use super::task::Task;
use crate::git::commands::{self, Add, Hash, ListWorktrees, RevParse, Status};
use crate::git::{self, Repository};

/// Everything `worktree` holds, staged and committed on `branch` under
/// `message` when the task runs, by the identity the worktree was
/// provisioned with. Each git is a process of its own, short enough to
/// go straight to the blocking pool: a [slot](super::AgentSlots) is an
/// agent's.
///
/// The commit is `branch`'s: a worktree standing anywhere else is
/// refused rather than committed, because a commit made on a wandering
/// `HEAD` is one the push, the pull request and the checks would each
/// go looking for on `branch` and not find.
#[derive(Clone, Debug)]
pub struct Commit {
    pub worktree: Repository,
    pub branch: String,
    pub message: String,
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
    /// A worktree standing off `branch`, [`NothingToCommit`] for a
    /// clean worktree, a message git would refuse, and git's own words.
    pub async fn commit(&self) -> Result<Hash> {
        let worktree = &self.worktree;
        let status = Status::new(worktree.clone());
        let add = Add::new(worktree.clone(), vec![".".into()])?;
        let commit = commands::Commit::new(worktree.clone(), self.message.clone())?;

        self.on_branch().await?;
        if briefly(|| git::status(status)).await?.is_empty() {
            return Err(NothingToCommit.into());
        }
        briefly(|| git::add(add)).await?;
        briefly(|| git::commit(commit)).await?;
        self.tip().await
    }

    /// The commit `branch` stands on, whatever `HEAD` is.
    ///
    /// # Errors
    ///
    /// No such branch, and git's own words.
    pub async fn tip(&self) -> Result<Hash> {
        let tip = RevParse::new(self.worktree.clone(), format!("refs/heads/{}", self.branch))?;
        briefly(|| git::rev_parse(tip)).await
    }

    /// That the worktree has `branch` checked out, which is what makes
    /// a commit made here `branch`'s.
    ///
    /// # Errors
    ///
    /// A worktree on another branch or on none, naming where it stands.
    async fn on_branch(&self) -> Result<()> {
        let worktree = self.worktree.clone();
        let listed = ListWorktrees::new(worktree.clone());
        let listed = briefly(move || git::list_worktrees(listed)).await?;
        // Git lists paths as it resolved them, symlinks followed, so the
        // comparison is between resolved paths.
        let here = worktree.path().canonicalize()?;
        let standing = listed
            .into_iter()
            .find(|worktree| worktree.path.canonicalize().is_ok_and(|at| at == here))
            .and_then(|worktree| worktree.branch);
        match standing {
            Some(branch) if branch == self.branch => Ok(()),
            Some(branch) => bail!("the worktree is on branch {branch}, not on {}", self.branch),
            None => bail!("the worktree is on no branch, not on {}", self.branch),
        }
    }
}

impl Task for Commit {
    async fn run(&self) -> Result<()> {
        self.commit().await.map(drop)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::git::commands::{Branch, Show, Switch};
    use crate::git::testing::{committed, credentials, head, seeded};
    use crate::git::url::Url;
    use crate::temp::Scratch;
    use crate::worktrees::{LinkedWorktree, Worktrees};

    /// A worktree on `issue-1` of a seeded origin, provisioned as an
    /// issue's is.
    fn provisioned(scratch: &Scratch) -> LinkedWorktree {
        let (origin, _) = seeded(scratch);
        let url = Url::Local(origin.path().to_owned());
        Worktrees::new(scratch.join("root").into())
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap()
    }

    /// `repository` moved onto a new branch `name`, as an agent that
    /// wandered off the one it was given leaves it.
    fn branched(repository: &Repository, name: &str) {
        let branch = Branch::new(repository.clone(), name.to_owned(), None).unwrap();
        git::branch(branch).unwrap();
        git::switch(Switch::new(repository.clone(), name.to_owned()).unwrap()).unwrap();
    }

    fn commit(linked: &LinkedWorktree) -> Commit {
        Commit {
            worktree: linked.worktree().clone(),
            branch: linked.branch().to_owned(),
            message: "feat: the work (#1)".to_owned(),
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
    async fn a_worktree_left_on_another_branch_is_refused_rather_than_committed() {
        let scratch = Scratch::new("commit-wandered");
        let linked = provisioned(&scratch);
        let worktree = linked.worktree();
        let before = head(worktree);
        branched(worktree, "fix");
        std::fs::write(worktree.path().join("new.txt"), "new\n").unwrap();

        let error = commit(&linked).run().await.unwrap_err().to_string();

        assert_eq!(error, "the worktree is on branch fix, not on issue-1");
        assert_eq!(
            git::rev_parse(
                RevParse::new(worktree.clone(), "refs/heads/issue-1".to_owned()).unwrap()
            )
            .unwrap(),
            before,
            "the issue's branch never moved"
        );
    }

    #[tokio::test]
    async fn the_tip_read_back_is_the_branchs_own_not_wherever_head_points() {
        let scratch = Scratch::new("commit-tip");
        let linked = provisioned(&scratch);
        let worktree = linked.worktree();
        let on_branch = committed(worktree, "work.txt", "work\n", "the work");
        // The agent wanders off and commits elsewhere; the branch stays.
        branched(worktree, "fix");
        let elsewhere = committed(worktree, "other.txt", "other\n", "elsewhere");

        assert_ne!(elsewhere, on_branch);
        assert_eq!(commit(&linked).tip().await.unwrap(), on_branch);
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
