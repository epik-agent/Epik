//! The `epik-worker` refusal story, held to at the process boundary.
//!
//! Every test spawns the real binary hermetically — a temp directory as the
//! whole `PATH`, another as the Epik home, and nothing else in the
//! environment — and asserts the acceptance verbatim: a structured refusal
//! naming the capability and the locations searched, the correct exit code,
//! no backtrace, and no worktree created. Fake tools are shell scripts, so
//! the fleet of absences (no git, an old git, a bogus agent path) is staged
//! without touching the machine's real toolchain.

// The worker is unix-only, like the agent it governs; so are its tests.
#![cfg(unix)]
// Tests are entitled to panic. The allow-unwrap-in-tests clippy setting only
// covers #[test] functions, not the helpers here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use tempfile::TempDir;

/// The worker's exit vocabulary, mirrored: distinct codes are part of the
/// refusal story.
const BROKEN: i32 = 4;
const USAGE: i32 = 2;
const REFUSED: i32 = 3;

/// A hermetic stage for one worker invocation: `bin` is the whole `PATH`,
/// `home` is the whole Epik home.
struct Rig {
    home: TempDir,
    bin: TempDir,
}

impl Rig {
    fn new() -> Self {
        Self {
            home: TempDir::new().unwrap(),
            bin: TempDir::new().unwrap(),
        }
    }

    /// Installs a fake tool on the rig's `PATH`.
    fn tool(&self, name: &str, script: &str) {
        let path = self.bin.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A git that answers `--version` and nothing else.
    fn git(&self, version: &str) {
        self.tool("git", &format!("echo \"git version {version}\""));
    }

    fn config(&self, text: &str) {
        fs::write(self.home.path().join("config.toml"), text).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_epik-worker"))
            .args(args)
            .env_clear()
            .env("PATH", self.bin.path())
            .env("EPIK_HOME", self.home.path())
            .output()
            .unwrap()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("the worker exits; it never dies of a signal")
}

/// The refusal story's invariants, all of them: the refusal exit code, no
/// backtrace, nothing on stdout, and no worktree — or clone — created.
fn assert_refused(rig: &Rig, output: &Output) {
    let said = stderr(output);
    assert_eq!(code(output), REFUSED, "{said}");
    assert!(said.starts_with("refused: "), "{said}");
    assert!(!said.to_lowercase().contains("backtrace"), "{said}");
    assert!(!said.contains("panicked"), "{said}");
    assert!(
        output.stdout.is_empty(),
        "no run happened, so no events: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !rig.home.path().join("work").exists(),
        "no worktree before the preflight passes"
    );
    assert!(!rig.home.path().join("repos").exists(), "and no clone");
}

#[test]
fn a_missing_git_refuses_naming_the_locations_searched() {
    let rig = Rig::new(); // the PATH directory exists and holds nothing

    let output = rig.run(&["--issue", "1", "--target", "main"]);

    assert_refused(&rig, &output);
    let said = stderr(&output);
    assert!(said.contains("git is absent"), "{said}");
    assert!(
        said.contains(&rig.bin.path().join("git").display().to_string()),
        "the searched location is named: {said}"
    );
}

#[test]
fn a_git_beneath_the_version_floor_is_unfit_not_absent() {
    let rig = Rig::new();
    rig.git("2.16.0");

    let output = rig.run(&["--issue", "1", "--target", "main"]);

    assert_refused(&rig, &output);
    let said = stderr(&output);
    assert!(said.contains("beneath the floor"), "{said}");
    assert!(
        said.contains(&rig.bin.path().join("git").display().to_string()),
        "the unfit git is named where it was found: {said}"
    );
}

#[test]
fn a_bogus_agent_path_refuses_naming_it_and_never_reaches_the_keystore() {
    let rig = Rig::new();
    rig.git("2.42.0");
    rig.tool("gh", "exit 0");
    rig.config("[worker]\nagent = \"/nowhere/claude\"\n");

    let output = rig.run(&["--feature", "104", "--target", "main"]);

    assert_refused(&rig, &output);
    let said = stderr(&output);
    assert!(said.contains("/nowhere/claude"), "{said}");
    assert!(said.contains("absent"), "{said}");
    assert!(
        !said.contains("GitHub token"),
        "the token layer goes unconsulted behind a refused agent layer: {said}"
    );
}

#[test]
fn a_bare_agent_name_off_path_refuses_with_the_hunt() {
    let rig = Rig::new();
    rig.git("2.42.0");
    rig.tool("gh", "exit 0"); // the default `claude` is never installed

    let output = rig.run(&["--issue", "7", "--target", "main"]);

    assert_refused(&rig, &output);
    let said = stderr(&output);
    assert!(
        said.contains(&rig.bin.path().join("claude").display().to_string()),
        "every PATH stop the hunt made is named: {said}"
    );
}

/// A keyring consult can be a permission dialog on macOS — the exact thing
/// tests must never pop — so the end-to-end token refusal runs where the
/// keystore is deterministically headless: Linux under `env_clear`, whose
/// secret service has no bus to answer on. The rails themselves are covered
/// on every platform by the library's own preflight tests.
#[cfg(target_os = "linux")]
#[test]
fn no_token_anywhere_refuses_naming_the_token() {
    let rig = Rig::new();
    rig.git("2.42.0");
    rig.tool("claude", "exit 0");
    rig.tool("gh", "exit 0");

    let output = rig.run(&["--issue", "1", "--target", "main"]);

    assert_refused(&rig, &output);
    let said = stderr(&output);
    assert!(said.contains("GitHub token"), "{said}");
}

#[test]
fn a_malformed_command_line_is_a_usage_error() {
    let rig = Rig::new();
    for args in [
        &[][..],
        &["--issue", "1"][..],
        &["--target", "main"][..],
        &["--issue", "seven", "--target", "main"][..],
        &["--issue", "1", "--feature", "2", "--target", "main"][..],
        &["--frobnicate"][..],
    ] {
        let output = rig.run(args);
        assert_eq!(code(&output), USAGE, "{args:?}: {}", stderr(&output));
        assert!(
            stderr(&output).contains("usage:"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn unparseable_config_is_fatal_at_startup_not_a_refusal() {
    let rig = Rig::new();
    rig.config("this is not toml = = =");

    let output = rig.run(&["--issue", "1", "--target", "main"]);

    assert_eq!(code(&output), BROKEN, "{}", stderr(&output));
    assert!(stderr(&output).contains("parsing"), "{}", stderr(&output));
}

#[test]
fn a_repo_that_is_not_owner_name_is_fatal_at_startup() {
    let rig = Rig::new();
    rig.config("[worker]\nrepo = \"nonsense\"\n");

    let output = rig.run(&["--issue", "1", "--target", "main"]);

    assert_eq!(code(&output), BROKEN, "{}", stderr(&output));
    assert!(stderr(&output).contains("nonsense"), "{}", stderr(&output));
}

#[test]
fn an_unwritable_home_is_fatal_at_startup() {
    let rig = Rig::new();
    // EPIK_HOME pointing at a file: a home that cannot be made a directory.
    let occupied = rig.home.path().join("occupied");
    fs::write(&occupied, "").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_epik-worker"))
        .args(["--issue", "1", "--target", "main"])
        .env_clear()
        .env("PATH", rig.bin.path())
        .env("EPIK_HOME", &occupied)
        .output()
        .unwrap();

    assert_eq!(code(&output), BROKEN, "{}", stderr(&output));
    assert!(stderr(&output).contains("creating"), "{}", stderr(&output));
}
