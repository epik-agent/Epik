//! Where a coding agent's source lives: a linked worktree of a local
//! clone, handed out by [`Worktrees`] and given back when dropped.
//!
//! Under one root, each URL gets a directory named by [`slug`], its
//! clone at `main` inside it and a linked worktree beside the clone
//! for every [`add`](Worktrees::add) outstanding, in a directory
//! named by the [`slug`] of its branch. The first add of a URL clones
//! it and every later one fetches, so a branch is cut from what the
//! URL holds as the add is made. Both run [as](run_as) the holder of
//! the credentials the add is given, so a remote that asks who is
//! there hears one answer, the same one its pushes give. The clone is
//! Epik's own, and its config carries the Epik [`IDENTITY`], so a
//! commit made in any worktree of it is Epik's, whoever makes it.
//! Dropping a [`LinkedWorktree`] frees its branch's name, and dropping
//! the last of a URL deletes the clone with the directory, so the
//! root holds only what is in use.
//! Adds and drops come from many threads at once and take turns: each
//! sees the clone as the one before it left it. A name one of this
//! Epik's own worktrees holds is that worktree's for as long as it is
//! out; what a dead Epik left in the clone is this one's to reclaim.

use std::collections::HashSet;
use std::fmt::{Display, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use anyhow::{Result, bail};

use crate::git::askpass::Credentials;
use crate::git::commands::{
    AddWorktree, Branch, Clone, Command, DeleteBranch, Fetch, Hash, ListWorktrees, Remote,
    RemoveWorktree, RevParse, SetConfig,
};
use crate::git::url::Url;
use crate::git::{
    Repository, add_worktree, branch, delete_branch, list_worktrees, remove_worktree, rev_parse,
    run_as, set_config,
};

/// Who Epik's commits are by, unsigned, as the config keys that say
/// so: written to a clone by every add of it, where they outrank the
/// machine's own.
pub const IDENTITY: [(&str, &str); 3] = [
    ("user.name", "Epik"),
    ("user.email", "epik@epik-agent.dev"),
    ("commit.gpgsign", "false"),
];

/// Whose turn it is to change what stands under a root, and which
/// worktrees this Epik has out: held for the length of an add or a
/// removal, one per process, so the threads of one Epik take turns
/// over every root at once. Two Epiks on one root each take their own
/// turns, and what is under the root is then theirs to share.
static TURN: Mutex<Held> = Mutex::new(Held::new());

/// A branch of one clone, as the clone's directory and the branch's
/// name: what an add asks for, and what tells one clone's `issue-1`
/// from another's.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Name(PathBuf, String);

/// The names this Epik's outstanding worktrees are on: a name in here
/// is a living worktree's and stands; every other name in a clone is
/// what a dead Epik left, and an add takes it back.
struct Held(Option<HashSet<Name>>);

impl Held {
    const fn new() -> Self {
        Self(None)
    }

    fn names(&mut self) -> &mut HashSet<Name> {
        self.0.get_or_insert_with(HashSet::new)
    }
}

/// The turn, taken and held: a thread that panicked holding it left
/// the clone as git did, which the taker reads afresh.
fn turn() -> MutexGuard<'static, Held> {
    TURN.lock().unwrap_or_else(PoisonError::into_inner)
}

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
    /// Epik [`IDENTITY`], and shared and fetched into by the rest,
    /// each as the holder of `credentials`. A local path asks for
    /// none, and is cloned whatever they are.
    ///
    /// # Errors
    ///
    /// A relative root, a relative local `url`, and git's own words:
    /// a URL that cannot be cloned or fetched, a remote that refuses
    /// `credentials`, an `at` naming no commit, or a `name` some
    /// branch of the clone already has, among them.
    pub fn add(
        &self,
        url: &Url,
        name: &str,
        at: &str,
        credentials: &Credentials,
    ) -> Result<LinkedWorktree> {
        let mut turn = turn();
        let group = self.root.join(slug(url));
        let clone_into = group.join("main");
        if clone_into.is_dir() {
            let main = Repository::open(clone_into.clone())?;
            let fetch = Fetch::new(main, Remote::origin());
            run_as(&Command::Fetch(fetch), credentials)?;
        } else {
            let clone = Clone::new(url.clone(), clone_into.clone(), None)?;
            run_as(&Command::Clone(clone), credentials)?;
        }
        let main = Repository::open(clone_into.clone())?;
        // Written each time, so a clone whose provisioning was cut
        // short is Epik's from the next add on.
        for (key, value) in IDENTITY {
            set_config(SetConfig::new(
                main.clone(),
                key.to_owned(),
                value.to_owned(),
            )?)?;
        }
        // A name a living worktree of this Epik is on is that
        // worktree's, and git says so; any other is a dead Epik's
        // leavings, taken back before the branch is cut.
        let held = Name(clone_into, name.to_owned());
        let mine = turn.names().insert(held.clone());
        let linked = group.join(slug(&name));
        let cut = cut(&main, mine, &linked, name, at);
        if cut.is_err() && mine {
            // The name was this add's, and is no one's now: the next
            // add of it starts where this one did.
            turn.names().remove(&held);
            let _ = DeleteBranch::new(main, name.to_owned()).map(delete_branch);
        }
        let (worktree, at) = cut?;
        Ok(LinkedWorktree {
            worktree,
            held,
            cut: at,
        })
    }
}

/// The worktree at `linked`, on `name` cut at `at` in `main`, when the
/// name is `mine` to cut: what a dead Epik left under the name goes
/// first, so the cut is of a clone as an unbroken run leaves it.
fn cut(
    main: &Repository,
    mine: bool,
    linked: &Path,
    name: &str,
    at: &str,
) -> Result<(Repository, Hash)> {
    if mine {
        reclaim(main, linked, name)?;
    }
    let at = rev_parse(RevParse::new(main.clone(), at.to_owned())?)?;
    branch(Branch::new(
        main.clone(),
        name.to_owned(),
        Some(at.clone()),
    )?)?;
    let worktree = add_worktree(AddWorktree::new(
        main.clone(),
        linked.to_owned(),
        name.to_owned(),
    )?)?;
    Ok((worktree, at))
}

/// `name` freed in `main`, with the worktree on it and the directory
/// at `linked`. A branch no worktree of the clone is on and a
/// directory that is not there are each already free.
fn reclaim(main: &Repository, linked: &Path, name: &str) -> Result<()> {
    let listed = list_worktrees(ListWorktrees::new(main.clone()))?;
    for stale in listed
        .iter()
        .filter(|worktree| worktree.branch.as_deref() == Some(name))
    {
        let stale = Repository::open(stale.path.clone())?;
        remove_worktree(RemoveWorktree::new(main.clone(), stale, true))?;
    }
    let _ = DeleteBranch::new(main.clone(), name.to_owned()).map(delete_branch);
    if linked.exists() {
        std::fs::remove_dir_all(linked)?;
    }
    Ok(())
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
    /// The branch it is on, and the clone that branch is of: given
    /// back as the removal frees the name.
    held: Name,
    cut: Hash,
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
        &self.held.1
    }

    /// The commit the branch was cut at.
    #[must_use]
    pub const fn cut(&self) -> &Hash {
        &self.cut
    }

    /// Removes the worktree, whatever it holds, then its branch from
    /// the clone — or the clone itself, when this was its last linked
    /// worktree.
    fn remove(&self) -> Result<()> {
        let mut turn = turn();
        let worktree = &self.worktree;
        let listed = list_worktrees(ListWorktrees::new(worktree.clone()))?;
        let Some((main, others)) = listed.split_first() else {
            bail!("git listed no worktree for {}", worktree.path().display());
        };
        let main = Repository::open(main.path.clone())?;
        turn.names().remove(&self.held);
        remove_worktree(RemoveWorktree::new(main.clone(), worktree.clone(), true))?;
        if others.len() == 1 {
            let group = main.path().parent().unwrap_or(main.path());
            std::fs::remove_dir_all(group)?;
        } else {
            delete_branch(DeleteBranch::new(main, self.branch().to_owned())?)?;
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
    use crate::git::testing::{
        INTRUDED, INTRUDER, OFFERED, PATIENCE, aimed, branch_of, committed, credentials, guarded,
        head, identified, pushed, seeded,
    };
    use crate::git::{fetch, show, switch};
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

        let first = worktrees
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap();
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
        let second = worktrees
            .add(&url, "issue-2", "origin/main", &credentials())
            .unwrap();
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

        let early = worktrees
            .add(&url, "issue-1", "origin/feature", &credentials())
            .unwrap();
        let landed = committed(&work, "early.txt", "early\n", "what landed early");
        pushed(&work, "feature");
        let late = worktrees
            .add(&url, "issue-2", "origin/feature", &credentials())
            .unwrap();

        assert_eq!(head(late.worktree()), landed);
        assert!(late.worktree().path().join("early.txt").is_file());
        assert!(!early.worktree().path().join("early.txt").exists());
    }

    #[test]
    fn adds_and_drops_of_one_url_from_two_threads_at_once_each_succeed() {
        let scratch = Scratch::new("worktrees-at-once");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let worktrees = Worktrees::new(scratch.join("root").into());
        // Each round starts with the URL never cloned, or with its
        // clone going as the other thread's last worktree drops.
        let rounds = |name: &'static str| {
            let (worktrees, url) = (&worktrees, &url);
            move || {
                for _ in 0..5 {
                    let linked = worktrees
                        .add(url, name, "origin/main", &credentials())
                        .unwrap();
                    assert!(linked.worktree().path().join("hello.txt").is_file());
                }
            }
        };
        std::thread::scope(|scope| {
            scope.spawn(rounds("issue-1"));
            scope.spawn(rounds("issue-2"));
        });
    }

    #[test]
    fn a_commit_in_any_worktree_is_epiks_by_the_clones_own_config() {
        let scratch = Scratch::new("worktrees-identity");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let worktrees = Worktrees::new(scratch.join("root").into());
        let first = worktrees
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap();
        let second = worktrees
            .add(&url, "issue-2", "origin/main", &credentials())
            .unwrap();

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
        let _other = worktrees
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap();

        let held = worktrees
            .add(&url, "issue-2", "origin/main", &credentials())
            .unwrap();
        let taken = worktrees
            .add(&url, "issue-2", "origin/main", &credentials())
            .unwrap_err()
            .to_string();
        assert!(taken.contains("'issue-2' already exists"), "{taken}");
        assert!(held.worktree().path().is_dir(), "the holder stands");

        drop(held);
        worktrees
            .add(&url, "issue-2", "origin/main", &credentials())
            .unwrap();
    }

    #[test]
    fn an_add_that_cannot_make_its_worktree_leaves_the_name_free() {
        let scratch = Scratch::new("worktrees-interrupted");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let root = PathBuf::from(scratch.join("root"));
        let worktrees = Worktrees::new(root.clone());
        let _held = worktrees
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap();
        // A file where the worktree's directory goes: git refuses.
        std::fs::write(root.join(slug(&url)).join("issue-2"), "in the way\n").unwrap();

        worktrees
            .add(&url, "issue-2", "origin/main", &credentials())
            .unwrap_err();

        std::fs::remove_file(root.join(slug(&url)).join("issue-2")).unwrap();
        let second = worktrees
            .add(&url, "issue-2", "origin/main", &credentials())
            .unwrap();
        assert_eq!(branch_of(second.worktree()), "issue-2");
    }

    #[test]
    fn an_add_reclaims_a_worktree_a_dead_epik_left_behind() {
        let scratch = Scratch::new("worktrees-leftover");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let root = PathBuf::from(scratch.join("root"));
        let worktrees = Worktrees::new(root.clone());
        let held = worktrees
            .add(&url, "issue-2", "origin/main", &credentials())
            .unwrap();
        // An Epik killed mid-build: its worktree and branch stand in
        // the clone, and Drop never ran to say they were its.
        let group = root.join(slug(&url));
        let main = Repository::open(group.join("main")).unwrap();
        branch(Branch::new(main.clone(), "issue-1".to_owned(), None).unwrap()).unwrap();
        let abandoned = add_worktree(
            AddWorktree::new(main, group.join("issue-1"), "issue-1".to_owned()).unwrap(),
        )
        .unwrap();
        std::fs::write(abandoned.path().join("half.txt"), "half\n").unwrap();

        let reclaimed = worktrees
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap();

        assert_eq!(branch_of(reclaimed.worktree()), "issue-1");
        assert!(!reclaimed.worktree().path().join("half.txt").exists());
        assert!(held.worktree().path().is_dir(), "the holder stands");
    }

    #[test]
    fn a_commit_is_epiks_though_the_clone_lost_the_identity() {
        let scratch = Scratch::new("worktrees-reidentified");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let root = PathBuf::from(scratch.join("root"));
        let worktrees = Worktrees::new(root.clone());
        let first = worktrees
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap();
        // The clone landed and the identity did not: what the machine
        // would have given stands in the clone's config instead.
        let main = Repository::open(root.join(slug(&url)).join("main")).unwrap();
        identified(&main);

        let second = worktrees
            .add(&url, "issue-2", "origin/main", &credentials())
            .unwrap();

        committed(second.worktree(), "work.txt", "work\n", "the work");
        let shown = show(Show::new(second.worktree().clone(), "HEAD".to_owned()).unwrap()).unwrap();
        assert!(
            shown.contains("Author: Epik <epik@epik-agent.dev>"),
            "{shown}"
        );
        drop(first);
    }

    #[test]
    fn add_needs_a_commit_to_cut_the_branch_at() {
        let scratch = Scratch::new("worktrees-nowhere");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let worktrees = Worktrees::new(scratch.join("root").into());
        let error = worktrees
            .add(&url, "issue-1", "origin/feature", &credentials())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("\"origin/feature\" names no commit"),
            "{error}"
        );

        let named = worktrees
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap();
        assert_eq!(branch_of(named.worktree()), "issue-1");
    }

    #[test]
    fn a_branch_with_a_slash_in_it_is_one_directory_beside_the_clone() {
        let scratch = Scratch::new("worktrees-slash");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let root = scratch.0.canonicalize().unwrap().join("root");
        let linked = Worktrees::new(root.clone())
            .add(&url, "issue/1", "origin/main", &credentials())
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
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap_err()
            .to_string();
        assert!(error.contains("nonexistent"), "{error}");
        assert!(!Path::new(&scratch.join("root")).exists());
    }

    #[test]
    fn a_clone_answers_a_remote_that_asks_with_the_credentials_and_fails_in_gits_words() {
        let scratch = Scratch::new("worktrees-guarded-clone");
        let worktrees = Worktrees::new(scratch.join("root").into());
        let (url, seen) = guarded();

        let error = worktrees
            .add(
                &url.parse().unwrap(),
                "issue-1",
                "origin/main",
                &credentials(),
            )
            .unwrap_err()
            .to_string();

        assert_eq!(seen.recv_timeout(PATIENCE).as_deref(), Ok(OFFERED));
        assert!(error.contains("Authentication failed"), "{error}");
        assert!(!error.contains("sesame"), "{error}");
    }

    #[test]
    fn a_fetch_answers_with_the_credentials_ahead_of_a_helper_that_would() {
        let scratch = Scratch::new("worktrees-guarded-fetch");
        let (origin, _) = seeded(&scratch);
        let url = Url::Local(origin.path().to_owned());
        let root = PathBuf::from(scratch.join("root"));
        let worktrees = Worktrees::new(root.clone());
        let _first = worktrees
            .add(&url, "issue-1", "origin/main", &credentials())
            .unwrap();
        let main = Repository::open(root.join(slug(&url)).join("main")).unwrap();
        let helper = SetConfig::new(
            main.clone(),
            "credential.helper".to_owned(),
            INTRUDER.to_owned(),
        );
        set_config(helper.unwrap()).unwrap();

        // The helper answers a fetch made with nothing in hand.
        let (guard, seen) = guarded();
        aimed(&main, guard);
        fetch(Fetch::new(main.clone(), Remote::origin())).unwrap_err();
        assert_eq!(seen.recv_timeout(PATIENCE).as_deref(), Ok(INTRUDED));

        let (guard, seen) = guarded();
        aimed(&main, guard);
        let error = worktrees
            .add(&url, "issue-2", "origin/main", &credentials())
            .unwrap_err()
            .to_string();
        assert_eq!(seen.recv_timeout(PATIENCE).as_deref(), Ok(OFFERED));
        assert!(error.contains("Authentication failed"), "{error}");
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
