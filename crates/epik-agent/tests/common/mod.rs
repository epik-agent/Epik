//! Fixtures shared by this package's integration tests. Each test
//! binary compiles its own copy and uses its own subset, hence the
//! file-wide dead-code allowance.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use epik::agent::{Agent, Task};
use epik::forge::{Credentials, Forge};

/// The real runner binary, located by cargo — which is why the tests
/// that launch Agents live in this package.
pub fn runner() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_epik-agent"))
}

/// A scratch directory that cleans up after itself.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "epik-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn join(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A forge for tests: a bare directory, no credentials.
#[derive(Debug)]
pub struct Local(pub String);

impl Forge for Local {
    fn remote(&self) -> String {
        self.0.clone()
    }

    fn credentials(&self) -> Option<Credentials> {
        None
    }
}

/// Runs git and insists it worked; the output is nobody's business.
pub fn git(args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A working repository with one commit on main, and a bare remote for
/// the forge.
pub fn seeded(scratch: &Scratch) -> (String, Local) {
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
    (work, Local(remote))
}

/// The scripted Agent: `sh -c` of an inline script — never a script
/// file, the ETXTBSY race — in the given working directory.
pub struct Shell {
    pub script: String,
    pub cwd: String,
}

impl Agent for Shell {
    fn task(&self) -> Task {
        Task {
            argv: vec!["sh".to_owned(), "-c".to_owned(), self.script.clone()],
            env: Vec::new(),
            cwd: self.cwd.clone(),
            stdin: None,
        }
    }
}

/// Removes a worktree a build kept, so a scratch drop is enough.
pub fn tidy(repository: &str, workspace: &Path) {
    let _ = std::process::Command::new("git")
        .args([
            "-C",
            repository,
            "worktree",
            "remove",
            "--force",
            "--",
            workspace.to_str().unwrap(),
        ])
        .status();
}
