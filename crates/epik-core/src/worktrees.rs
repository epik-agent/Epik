//! Where a coding agent's source lives: a linked worktree of a local
//! clone, handed out by [`Worktrees`] and given back when dropped.
//!
//! Under one root, each URL gets a directory named by [`slug`], its
//! clone at `main` inside it and a linked worktree beside the clone
//! for every [`add`](Worktrees::add) outstanding, in a directory
//! named by the [`slug`] of its branch. The first add of a URL clones
//! it and every later one fetches, so a branch is cut from what the
//! URL holds as the add is made. The clone is Epik's own, and its
//! config carries the Epik [`IDENTITY`], so a commit made in any
//! worktree of it is Epik's, whoever makes it. Dropping a [`LinkedWorktree`] frees
//! its branch's name, and dropping the last of a URL deletes the
//! clone with the directory, so the root holds only what is in use.

use std::fmt::{Display, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::git::commands::{
    AddWorktree, Branch, Clone, DeleteBranch, Fetch, ListWorktrees, Remote, RemoveWorktree,
    RevParse, SetConfig,
};
use crate::git::url::Url;
use crate::git::{
    Repository, add_worktree, branch, clone, delete_branch, fetch, list_worktrees, remove_worktree,
    rev_parse, set_config,
};

/// Who Epik's commits are by, unsigned, as the config keys that say
/// so: written to each clone as it is made, where they outrank the
/// machine's own.
pub const IDENTITY: [(&str, &str); 3] = [
    ("user.name", "Epik"),
    ("user.email", "epik@epik-agent.dev"),
    ("commit.gpgsign", "false"),
];

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

    /// A fresh linked worktree of `url`, on the new branch `name` cut
    /// at `at`: a commit-ish read in the clone once it holds what
    /// `url` holds now, where `origin/feature` is the tip of the
    /// URL's `feature`. The clone is made on the first add, under the
    /// Epik [`IDENTITY`], and shared and fetched into by the rest.
    ///
    /// # Errors
    ///
    /// A relative root, a relative local `url`, and git's own words:
    /// a URL that cannot be cloned or fetched, an `at` naming no
    /// commit, or a `name` some branch of the clone already has,
    /// among them.
    pub fn add(&self, url: &Url, name: &str, at: &str) -> Result<LinkedWorktree> {
        let group = self.root.join(slug(url));
        let clone_into = group.join("main");
        let main = if clone_into.is_dir() {
            let main = Repository::open(clone_into)?;
            fetch(Fetch::new(main.clone(), Remote::origin()))?;
            main
        } else {
            let main = clone(Clone::new(url.clone(), clone_into, None)?)?;
            for (key, value) in IDENTITY {
                set_config(SetConfig::new(
                    main.clone(),
                    key.to_owned(),
                    value.to_owned(),
                )?)?;
            }
            main
        };
        let at = rev_parse(RevParse::new(main.clone(), at.to_owned())?)?;
        branch(Branch::new(main.clone(), name.to_owned(), Some(at))?)?;
        let linked = group.join(slug(&name));
        let worktree = add_worktree(AddWorktree::new(main, linked, name.to_owned())?)?;
        Ok(LinkedWorktree {
            worktree,
            branch: name.to_owned(),
        })
    }
}

/// `name` as one directory name, a different one for every name: its
/// spelling, with each byte outside letters, digits, `.`, `-`, `_`,
/// and `@` written as `%` and two hex digits.
fn slug(name: &impl Display) -> String {
    let spelling = name.to_string();
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
pub struct LinkedWorktree {
    worktree: Repository,
    branch: String,
}

impl LinkedWorktree {
    /// The worktree, for the git that runs in it.
    #[must_use]
    pub const fn worktree(&self) -> &Repository {
        &self.worktree
    }

    /// The branch the worktree was cut on.
    #[must_use]
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Removes the worktree, whatever it holds, then its branch from
    /// the clone — or the clone itself, when this was its last linked
    /// worktree.
    fn remove(&self) -> Result<()> {
        let worktree = &self.worktree;
        let listed = list_worktrees(ListWorktrees::new(worktree.clone()))?;
        let Some((main, others)) = listed.split_first() else {
            bail!("git listed no worktree for {}", worktree.path().display());
        };
        let main = Repository::open(main.path.clone())?;
        remove_worktree(RemoveWorktree::new(main.clone(), worktree.clone(), true))?;
        if others.len() == 1 {
            let group = main.path().parent().unwrap_or(main.path());
            std::fs::remove_dir_all(group)?;
        } else {
            delete_branch(DeleteBranch::new(main, self.branch.clone())?)?;
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
    use std::path::Path;

    use super::*;
    use crate::git::commands::{Hash, Show, Switch, WorktreeInfo};
    use crate::git::testing::{branch_of, committed, head, pushed, seeded};
    use crate::git::{show, switch};
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

    /// `linked` as git lists it: on its branch.
    fn linked(linked: &LinkedWorktree, head: &Hash) -> WorktreeInfo {
        let path = linked.worktree().path().to_owned();
        info(path, Some(linked.branch().to_owned()), head)
    }

    #[test]
    fn a_url_is_cloned_once_and_each_add_is_its_own_worktree_on_the_branch_named_until_dropped() {
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

        let first = worktrees.add(&url, "issue-1", "origin/main").unwrap();
        assert!(main.is_dir(), "the first add cloned");
        assert_eq!(
            first.branch(),
            "issue-1",
            "the branch is named for its issue"
        );
        assert_eq!(first.worktree().path(), group.join("issue-1"));
        assert!(first.worktree().path().join("hello.txt").is_file());
        assert_eq!(
            listed(first.worktree()),
            [main_info.clone(), linked(&first, &head)]
        );

        let cloned_at = std::fs::metadata(&main).unwrap().modified().unwrap();
        let second = worktrees.add(&url, "issue-2", "origin/main").unwrap();
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

    #[test]
    fn a_worktree_cut_after_a_merge_landed_on_the_feature_branch_contains_it() {
        let scratch = Scratch::new("worktrees-late");
        let (origin, work) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let worktrees = Worktrees::new(scratch.join("root").into());
        branch(Branch::new(work.clone(), "feature".to_owned(), None).unwrap()).unwrap();
        switch(Switch::new(work.clone(), "feature".to_owned()).unwrap()).unwrap();
        pushed(&work, "feature");

        let early = worktrees.add(&url, "issue-1", "origin/feature").unwrap();
        let landed = committed(&work, "early.txt", "early\n", "what landed early");
        pushed(&work, "feature");
        let late = worktrees.add(&url, "issue-2", "origin/feature").unwrap();

        assert_eq!(head(late.worktree()), landed);
        assert!(late.worktree().path().join("early.txt").is_file());
        assert!(!early.worktree().path().join("early.txt").exists());
    }

    #[test]
    fn a_commit_in_any_worktree_is_epiks_by_the_clones_own_config() {
        let scratch = Scratch::new("worktrees-identity");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let worktrees = Worktrees::new(scratch.join("root").into());
        let first = worktrees.add(&url, "issue-1", "origin/main").unwrap();
        let second = worktrees.add(&url, "issue-2", "origin/main").unwrap();

        for linked in [&first, &second] {
            committed(linked.worktree(), "work.txt", "work\n", "the work");
            let shown =
                show(Show::new(linked.worktree().clone(), "HEAD".to_owned()).unwrap()).unwrap();
            assert!(
                shown.contains("Author: Epik <epik@epik-agent.dev>"),
                "{shown}"
            );
        }
    }

    #[test]
    fn a_branch_is_one_worktrees_until_the_drop_frees_its_name() {
        let scratch = Scratch::new("worktrees-taken");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let worktrees = Worktrees::new(scratch.join("root").into());
        let _other = worktrees.add(&url, "issue-1", "origin/main").unwrap();

        let held = worktrees.add(&url, "issue-2", "origin/main").unwrap();
        let taken = worktrees
            .add(&url, "issue-2", "origin/main")
            .unwrap_err()
            .to_string();
        assert!(taken.contains("'issue-2' already exists"), "{taken}");
        assert!(held.worktree().path().is_dir(), "the holder stands");

        drop(held);
        worktrees.add(&url, "issue-2", "origin/main").unwrap();
    }

    #[test]
    fn add_needs_a_commit_to_cut_the_branch_at() {
        let scratch = Scratch::new("worktrees-nowhere");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let worktrees = Worktrees::new(scratch.join("root").into());
        let error = worktrees
            .add(&url, "issue-1", "origin/feature")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("\"origin/feature\" names no commit"),
            "{error}"
        );
    }

    #[test]
    fn a_branch_with_a_slash_in_it_is_one_directory_beside_the_clone() {
        let scratch = Scratch::new("worktrees-slash");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let root = scratch.0.canonicalize().unwrap().join("root");
        let linked = Worktrees::new(root.clone())
            .add(&url, "issue/1", "origin/main")
            .unwrap();
        assert_eq!(
            linked.worktree().path(),
            root.join(slug(&url)).join("issue%2F1")
        );
        assert_eq!(branch_of(linked.worktree()), "issue/1");
    }

    #[test]
    fn add_needs_a_url_git_can_clone() {
        let scratch = Scratch::new("worktrees-unclonable");
        let worktrees = Worktrees::new(scratch.join("root").into());
        let url = "/nonexistent/repo.git".parse().unwrap();
        let error = worktrees
            .add(&url, "issue-1", "origin/main")
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
            let slug = slug(&text.parse::<Url>().unwrap());
            assert_eq!(slug, expected);
            assert_eq!(Path::new(&slug).components().count(), 1, "{slug}");
        }
    }
}
