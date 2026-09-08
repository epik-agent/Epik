//! Test infrastructure that crosses a boundary: what this crate's tests
//! share across modules and, under the `testing` feature, with its
//! dependents' tests. Two fakes with a hard rule behind them — no test
//! in this project ever depends on a real LLM: the scripted [`model`],
//! a loopback provider, and the scripted [`agent`], a deterministic
//! child. And the fixtures every git-adjacent test module used to carry
//! a copy of: the scratch directory, the seeded repository, and the
//! forge that is only a path.
//!
//! Only shared things live here. A fixture that serves one subtree
//! stays in that subtree, beside the tests it serves — the plan
//! builders in `feature::fixtures` are the example — and moves here on
//! the day a second home needs it.

pub mod agent;
pub mod model;

use std::path::{Path, PathBuf};

use crate::git::plumbing;

/// A scratch directory that cleans up after itself.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let path = crate::temp::unique(&format!("epik-test-{name}"));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }

    pub fn join(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }

    pub fn write(&self, name: &str, content: &str) -> &Self {
        std::fs::write(self.0.join(name), content).unwrap();
        self
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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
