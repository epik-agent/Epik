//! Where a coding agent's source lives: a linked worktree of a local
//! clone, handed out by [`Worktrees`] and given back when dropped.
//!
//! Under one root, each URL gets a directory named by [`slug`], its
//! clone at `main` inside it and a linked worktree beside the clone
//! for every [`add`](Worktrees::add) outstanding. The first add of
//! a URL clones it; dropping the last [`LinkedWorktree`] of it deletes
//! the clone with the directory, so the root holds only what is in
//! use.

use std::fmt::Write;
use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::git::commands::{AddWorktree, Branch, Clone, ListWorktrees, RemoveWorktree};
use crate::git::url::Url;
use crate::git::{Repository, add_worktree, branch, clone, list_worktrees, remove_worktree};
use crate::temp;

/// A cloned URL and its linked worktrees.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Worktrees {
    root: PathBuf,
}

impl Worktrees {
    /// The worktrees under `root`, an absolute path that need not
    /// exist yet.
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// A fresh linked worktree of `url`, on a new branch at the
    /// clone's `HEAD`. The clone is made on the first add and shared
    /// by the rest.
    ///
    /// # Errors
    ///
    /// A relative root, a relative local `url`, and git's own words:
    /// a URL that cannot be cloned, or a clone with no commit to
    /// branch from, among them.
    pub fn add(&self, url: &Url) -> Result<LinkedWorktree> {
        let group = self.root.join(slug(url));
        let clone_into = group.join("main");
        let main = if clone_into.is_dir() {
            Repository::open(clone_into)?
        } else {
            clone(Clone::new(url.clone(), clone_into, None)?)?
        };
        let name = temp::name("worktree");
        branch(Branch::new(main.clone(), name.clone(), None)?)?;
        let linked = add_worktree(AddWorktree::new(main, group.join(&name), name)?)?;
        Ok(LinkedWorktree(linked))
    }
}

/// `url` as one directory name, a different one for every URL: its
/// spelling, with each byte outside letters, digits, `.`, `-`, `_`,
/// and `@` written as `%` and two hex digits.
fn slug(url: &Url) -> String {
    let spelling = url.to_string();
    let mut slug = String::with_capacity(spelling.len());
    for byte in spelling.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'@') {
            slug.push(byte as char);
        } else {
            // Writing to a String cannot fail.
            let _ = write!(slug, "%{byte:02X}");
        }
    }
    slug
}

/// A linked worktree [`Worktrees`] handed out, on a branch of its
/// own, until it is dropped.
#[derive(Debug, Eq, PartialEq)]
pub struct LinkedWorktree(Repository);

impl LinkedWorktree {
    /// The worktree, for the git that runs in it.
    #[must_use]
    pub const fn worktree(&self) -> &Repository {
        &self.0
    }

    /// Removes the worktree, whatever it holds, and deletes the clone
    /// when this was its last linked worktree.
    fn remove(&self) -> Result<()> {
        let listed = list_worktrees(ListWorktrees::new(self.0.clone()))?;
        let Some((main, others)) = listed.split_first() else {
            bail!("git listed no worktree for {}", self.0.path().display());
        };
        let main = Repository::open(main.path.clone())?;
        remove_worktree(RemoveWorktree::new(main.clone(), self.0.clone(), true))?;
        if others.len() == 1 {
            let group = main.path().parent().unwrap_or(main.path());
            std::fs::remove_dir_all(group)?;
        }
        Ok(())
    }
}

impl Drop for LinkedWorktree {
    /// A worktree that cannot be removed is left where it is.
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::*;
    use crate::git::commands::{Hash, WorktreeInfo};
    use crate::git::testing::{head, seeded};
    use crate::temp::Scratch;

    fn listed(repository: &Repository) -> Vec<WorktreeInfo> {
        list_worktrees(ListWorktrees::new(repository.clone())).unwrap()
    }

    /// The worktree at `path` on the branch named `branch`, at `head`.
    fn info(path: PathBuf, branch: Option<String>, head: &Hash) -> WorktreeInfo {
        WorktreeInfo {
            path,
            head: Some(head.clone()),
            branch,
            bare: false,
        }
    }

    /// `linked` as git lists it: on the branch its directory is named
    /// after.
    fn linked(linked: &LinkedWorktree, head: &Hash) -> WorktreeInfo {
        let path = linked.worktree().path();
        let branch = path.file_name().and_then(OsStr::to_str).map(str::to_owned);
        info(path.to_owned(), branch, head)
    }

    #[test]
    fn a_url_is_cloned_once_and_each_add_is_its_own_worktree_on_its_own_branch_until_dropped() {
        let scratch = Scratch::new("worktrees");
        let (origin, work) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let head = head(&work);
        // Git lists paths as it resolved them, symlinks followed.
        let root = scratch.0.canonicalize().unwrap().join("root");
        let worktrees = Worktrees::new(root.clone());
        let group = root.join(slug(&url));
        let main = group.join("main");
        let main_info = info(main.clone(), Some("main".to_owned()), &head);

        let first = worktrees.add(&url).unwrap();
        assert!(main.is_dir(), "the first add cloned");
        assert_eq!(first.worktree().path().parent(), Some(group.as_path()));
        assert!(first.worktree().path().join("hello.txt").is_file());
        assert_eq!(
            listed(first.worktree()),
            [main_info.clone(), linked(&first, &head)]
        );

        let cloned_at = std::fs::metadata(&main).unwrap().modified().unwrap();
        let second = worktrees.add(&url).unwrap();
        assert_eq!(
            std::fs::metadata(&main).unwrap().modified().unwrap(),
            cloned_at,
            "the second add reused the clone"
        );
        assert_eq!(
            listed(first.worktree()),
            [
                main_info.clone(),
                linked(&first, &head),
                linked(&second, &head)
            ],
            "each add on its own branch"
        );

        std::fs::write(first.worktree().path().join("loose.txt"), "loose\n").unwrap();
        let first_path = first.worktree().path().to_owned();
        drop(first);
        assert!(!first_path.exists());
        assert!(main.is_dir(), "the clone stays while a worktree is out");
        assert_eq!(
            listed(second.worktree()),
            [main_info, linked(&second, &head)]
        );

        drop(second);
        assert!(!group.exists(), "the last drop deleted the clone");
        assert!(root.is_dir(), "and only the clone");
    }

    /// The whole loop an agent's source goes through: a bare origin, a
    /// worktree of it, an engine writing there, a commit and a push,
    /// the worktree gone with its drop, and the work back out of the
    /// origin by a fresh clone made outside `Worktrees`.
    #[cfg(feature = "testing")]
    #[test]
    fn what_an_agent_commits_in_a_worktree_is_pushed_and_clones_back() {
        use crate::agent::Agent;
        use crate::agent::scripted::{self, Scripted};
        use crate::git::commands::{Add, Commit, Push};
        use crate::git::testing::{cloned, identified, origin};
        use crate::git::{add, commit, push};
        use crate::process::Exit;

        let scratch = Scratch::new("worktrees-adder");
        let (bare, _) = seeded(&scratch);
        let url = Url::Local(bare.path().to_owned());
        let worktrees = Worktrees::new(scratch.0.join("root"));
        let linked = worktrees.add(&url).unwrap();
        let repository = linked.worktree().clone();
        let branch = repository
            .path()
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap()
            .to_owned();

        let files = scripted::adder();
        let mut engine = Scripted {
            cwd: repository.path().to_owned(),
            files: files.clone(),
        }
        .start()
        .unwrap();
        for _ in engine.stdout() {}
        assert_eq!(engine.wait().unwrap(), Exit::Code(0));

        identified(&repository);
        add(Add::new(repository.clone(), files.keys().cloned().collect()).unwrap()).unwrap();
        commit(Commit::new(repository.clone(), "the adder library".to_owned()).unwrap()).unwrap();
        push(Push::new(repository.clone(), origin(), branch.clone()).unwrap()).unwrap();
        drop(linked);
        assert!(!repository.path().exists(), "the worktree removed itself");

        let check = cloned(&scratch, &bare, "check", Some(&branch));
        for (path, content) in &files {
            assert_eq!(
                std::fs::read_to_string(check.path().join(path)).unwrap(),
                *content,
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn add_needs_a_url_git_can_clone() {
        let scratch = Scratch::new("worktrees-unclonable");
        let worktrees = Worktrees::new(scratch.join("root").into());
        let error = worktrees
            .add(&"/nonexistent/repo.git".parse().unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("nonexistent"), "{error}");
        assert!(!Path::new(&scratch.join("root")).exists());
    }

    #[test]
    fn a_slug_is_one_directory_name_and_differs_where_the_url_does() {
        let cases = [
            (
                "https://github.com/owner/repo.git",
                "https%3A%2F%2Fgithub.com%2Fowner%2Frepo.git",
            ),
            (
                "git@github.com:owner/repo.git",
                "git@github.com%3Aowner%2Frepo.git",
            ),
            ("/srv/git/repo.git", "%2Fsrv%2Fgit%2Frepo.git"),
            ("host:répo", "host%3Ar%C3%A9po"),
        ];
        for (text, expected) in cases {
            let slug = slug(&text.parse().unwrap());
            assert_eq!(slug, expected);
            assert_eq!(Path::new(&slug).components().count(), 1, "{slug}");
        }
    }
}
