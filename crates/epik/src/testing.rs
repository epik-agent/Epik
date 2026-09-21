//! Test infrastructure that crosses a boundary: what this crate's tests
//! share across modules and, under the `testing` feature, with its
//! dependents' tests. One fake with a hard rule behind it — no test in
//! this project ever depends on a real LLM: the scripted [`model`], a
//! loopback provider. And the fixtures every git-adjacent test module
//! used to carry a copy of: the [`Scratch`] directory, from epik-core,
//! the seeded repository, and the forge that is only a path.
//!
//! Only shared things live here. A fixture that serves one subtree
//! stays in that subtree, beside the tests it serves — the plan
//! builders in `feature::fixtures` are the example — and moves here on
//! the day a second home needs it.

use std::path::Path;

pub use epik_core::assistant::scripted as model;
pub use epik_core::temp::Scratch;

use crate::git::plumbing;

/// A working repository with one commit on main, and a bare remote
/// beside it, under `scratch`: `(work, remote)`, both paths.
pub fn seeded(scratch: &Scratch) -> (String, String) {
    let git = |args: &[&str]| plumbing(args).unwrap();
    let work = scratch.join("work");
    let remote = scratch.join("remote.git");
    git(&["init", "--initial-branch=main", &work]);
    git(&["-C", &work, "config", "user.name", "Test"]);
    git(&["-C", &work, "config", "user.email", "test@example.com"]);
    git(&["-C", &work, "config", "commit.gpgsign", "false"]);
    std::fs::write(Path::new(&work).join("hello.txt"), "hello\n").unwrap();
    git(&["-C", &work, "add", "hello.txt"]);
    git(&["-C", &work, "commit", "-m", "the first commit"]);
    git(&["init", "--bare", "--initial-branch=main", &remote]);
    (work, remote)
}

/// Removes every linked worktree a build left behind in `repository`,
/// so a scratch drop is enough.
pub fn tidy(repository: &str) {
    let listed = plumbing(&["-C", repository, "worktree", "list", "--porcelain"]).unwrap();
    for line in listed.lines() {
        if let Some(path) = line.strip_prefix("worktree ")
            && path != repository
        {
            let _ = plumbing(&[
                "-C", repository, "worktree", "remove", "--force", "--", path,
            ]);
        }
    }
}

/// A forge for tests: a bare directory, no credentials — pushing to it
/// is pushing to a path.
#[cfg(unix)]
#[derive(Debug)]
pub struct Local(pub String);

#[cfg(unix)]
impl crate::forge::Forge for Local {
    fn remote(&self) -> String {
        self.0.clone()
    }

    fn credentials(&self) -> Option<crate::forge::Credentials> {
        None
    }
}
