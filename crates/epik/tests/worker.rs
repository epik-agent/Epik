//! The `epik-worker` story, held to at the process boundary.
//!
//! Every test spawns the real binary hermetically — a temp directory as the
//! whole `PATH`, another as the Epik home, and nothing else in the
//! environment. The refusal tests assert the acceptance verbatim: a
//! structured refusal naming the capability and the locations searched, the
//! correct exit code, no backtrace, and no worktree created. Fake tools are
//! shell scripts, so the fleet of absences (no git, an old git, a bogus
//! agent path) is staged without touching the machine's real toolchain.
//!
//! The conducted-run tests go further: a local origin stands in for the
//! clone, a little GitHub on a loopback port answers the handful of REST
//! reads one issue run makes, and a scripted claude plays the agent — so
//! the success path (exit 0, well-formed JSON events on stdout, the
//! worktree cleaned) and the cancellation path (a signal winds the run up
//! and the agent's whole process group dies with it) both run end to end
//! with no network and no Anthropic.

// The worker is unix-only, like the agent it governs; so are its tests.
#![cfg(unix)]
// Tests are entitled to panic. The allow-unwrap-in-tests clippy setting only
// covers #[test] functions, not the helpers here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use epik::run::{Phase, RunEvent, Verdict};
use serde_json::json;
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

    /// The worker, hermetically staged: args set, environment cleared down
    /// to the rig's own `PATH` and home. Tests customize before spawning.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_epik-worker"));
        command
            .args(args)
            .env_clear()
            .env("PATH", self.bin.path())
            .env("EPIK_HOME", self.home.path());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// Stages a whole conductable run: the machine's real git appended to
    /// the `PATH` (fetch and worktree need the genuine article beside the
    /// fakes), a local origin standing in for the clone, a little GitHub
    /// on a loopback port, and a fake `gh`. The caller scripts the claude.
    fn conductable(&self, origin: &Path, api_port: u16) {
        self.tool("gh", "exit 0");
        self.config(&format!(
            "[worker]\nurl = {:?}\napi = \"http://127.0.0.1:{api_port}\"\n",
            origin.display().to_string(),
        ));
    }

    /// The rig's `PATH` with the machine's real git appended.
    fn path_with_real_git(&self) -> std::ffi::OsString {
        let told = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let git = PathBuf::from(String::from_utf8_lossy(&told.stdout).trim().to_owned());
        env::join_paths([self.bin.path(), git.parent().unwrap()]).unwrap()
    }
}

/// Runs git in `dir`, insisting it succeed: origin-side scaffolding,
/// hermetic and with a fixed identity, on the library's own testing
/// pattern.
fn sh(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "epik")
        .env("GIT_AUTHOR_EMAIL", "epik@example.invalid")
        .env("GIT_COMMITTER_NAME", "epik")
        .env("GIT_COMMITTER_EMAIL", "epik@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// A little origin: one commit on `main`, standing in for the clone URL.
fn origin(root: &TempDir) -> (PathBuf, String) {
    let dir = root.path().join("github");
    fs::create_dir_all(&dir).unwrap();
    sh(&dir, &["init", "-b", "main"]);
    fs::write(dir.join("README.md"), "hello").unwrap();
    sh(&dir, &["add", "."]);
    sh(&dir, &["commit", "-m", "hello"]);
    let sha = String::from_utf8_lossy(&sh(&dir, &["rev-parse", "main"]).stdout)
        .trim()
        .to_owned();
    (dir, sha)
}

/// A little GitHub on a loopback port: exactly the REST reads one issue
/// run makes, canned around `sha` — the commit the run's worktree will
/// stand on. When `closes`, the issue reads open first and closed after,
/// which is the arc the wide prompt promises: judgment's re-read sees the
/// closure. When not, the issue stays open and judgment disbelieves the
/// agent — the staged failure.
fn little_github(sha: &str, closes: bool) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let sha = sha.to_owned();
    thread::spawn(move || {
        let mut issue_reads = 0;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Some(path) = request_path(&mut stream) else {
                continue;
            };
            let body = if path.starts_with("/repos/epik-agent/Epik/issues/7") {
                issue_reads += 1;
                let state = if closes && issue_reads > 1 {
                    "closed"
                } else {
                    "open"
                };
                json!({"number": 7, "title": "make it work", "body": "details", "state": state})
            } else if path.starts_with("/repos/epik-agent/Epik/pulls/41") {
                json!({
                    "number": 41, "title": "the work", "state": "closed", "merged": true,
                    "head": {"ref": "issue-7", "sha": sha},
                    "base": {"ref": "main", "sha": sha},
                })
            } else if path.starts_with("/repos/epik-agent/Epik/pulls?") {
                json!([{"number": 41}])
            } else if path.contains("/check-runs") {
                json!({"check_runs": [{"name": "CI", "conclusion": "success"}]})
            } else {
                json!({"message": format!("little github never learned {path}")})
            };
            respond(&mut stream, &body.to_string());
        }
    });
    port
}

/// The request line's path, with the headers drained so the client is not
/// wedged mid-send.
fn request_path(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    let head = String::from_utf8_lossy(&head);
    head.lines()
        .next()?
        .split_whitespace()
        .nth(1)
        .map(str::to_owned)
}

fn respond(stream: &mut TcpStream, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len(),
    );
}

/// The run logs a rig accumulated, in file order: each file's name beside
/// its lines parsed back into the issue-run vocabulary.
fn run_logs(rig: &Rig) -> Vec<(String, Vec<RunEvent>)> {
    let dir = rig.home.path().join("logs/epik-agent/Epik");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    files
        .iter()
        .map(|path| {
            let events = fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).expect("every log line is one JSON event"))
                .collect();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, events)
        })
        .collect()
}

/// Polls `check` to true within `deadline`, or fails naming `what`.
fn eventually(what: &str, deadline: Duration, mut check: impl FnMut() -> bool) {
    let end = Instant::now() + deadline;
    while !check() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Whether a pid still names a live process.
fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
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
    assert!(
        !rig.home.path().join("logs").exists(),
        "and no log file claiming a run happened"
    );
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
fn no_path_at_all_refuses_with_nowhere_to_search() {
    let rig = Rig::new();
    // The launchd case: no PATH in the environment whatsoever.
    let output = Command::new(env!("CARGO_BIN_EXE_epik-worker"))
        .args(["--issue", "1", "--target", "main"])
        .env_clear()
        .env("EPIK_HOME", rig.home.path())
        .output()
        .unwrap();

    assert_refused(&rig, &output);
    let said = stderr(&output);
    assert!(
        said.contains("git is absent, with nowhere to search"),
        "an absent PATH is nowhere, never the cwd: {said}"
    );
}

#[test]
fn an_explicit_relative_agent_spelling_is_vouched_for_absolutely() {
    let rig = Rig::new();
    rig.git("2.42.0");
    rig.tool("gh", "exit 0");
    rig.config("[worker]\nagent = \"bin/claude\"\n");
    let elsewhere = TempDir::new().unwrap();

    let output = rig
        .command(&["--issue", "1", "--target", "main"])
        .current_dir(elsewhere.path())
        .output()
        .unwrap();

    assert_refused(&rig, &output);
    let said = stderr(&output);
    let settled = elsewhere.path().canonicalize().unwrap().join("bin/claude");
    assert!(
        said.contains(&settled.display().to_string()),
        "the spelling settles against the launch cwd — what would spawn is what is vouched for: {said}"
    );
}

#[test]
fn a_verified_issue_run_exits_zero_streaming_json_events() {
    let rig = Rig::new();
    let root = TempDir::new().unwrap();
    let (origin_dir, sha) = origin(&root);
    let port = little_github(&sha, true);
    rig.conductable(&origin_dir, port);
    // A claude that is entirely script: it claims completion — and leaves a
    // note of the identity and credential the harness injected.
    rig.tool(
        "claude",
        "printf '%s\\n' \"$GIT_AUTHOR_NAME $GH_TOKEN\" > \"$(dirname \"$0\")/seen\"\n\
         echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"done\"}'",
    );

    let output = rig
        .command(&["--issue", "7", "--target", "main"])
        .env("PATH", rig.path_with_real_git())
        .env("EPIK_GITHUB_TOKEN", "ghp-fake")
        .output()
        .unwrap();

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stderr(&output).trim(),
        "",
        "a verified run has nothing to complain about"
    );
    let events: Vec<RunEvent> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("every stdout line is one JSON event"))
        .collect();
    assert_eq!(
        events.first(),
        Some(&RunEvent::Entered(Phase::Worktree)),
        "{events:?}"
    );
    assert_eq!(
        events.last(),
        Some(&RunEvent::Finished(Verdict::Done)),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RunEvent::Agent(_))),
        "the agent's stream rides within: {events:?}"
    );
    assert_eq!(
        fs::read_to_string(rig.bin.path().join("seen"))
            .unwrap()
            .trim(),
        "Epik ghp-fake",
        "identity and credential are injected, never discovered"
    );
    assert!(
        !rig.home
            .path()
            .join("work/epik-agent/Epik/issue-7")
            .exists(),
        "a verified success leaves no worktree"
    );
    let logs = run_logs(&rig);
    let [(name, logged)] = logs.as_slice() else {
        panic!("one run, one log file: {logs:?}");
    };
    assert!(
        name.starts_with("issue-7-") && name.ends_with("Z.jsonl"),
        "the file is named for the run and its start: {name}"
    );
    assert_eq!(
        logged, &events,
        "the log is the stream exactly as stdout heard it"
    );
}

#[test]
fn a_failed_runs_log_survives_beside_the_retained_worktree() {
    let rig = Rig::new();
    let root = TempDir::new().unwrap();
    let (origin_dir, sha) = origin(&root);
    // A github that never closes the issue: the agent's claimed completion
    // is disbelieved, so the run fails and its worktree is retained.
    let port = little_github(&sha, false);
    rig.conductable(&origin_dir, port);
    rig.tool(
        "claude",
        "echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"done\"}'",
    );

    let output = rig
        .command(&["--issue", "7", "--target", "main"])
        .env("PATH", rig.path_with_real_git())
        .env("EPIK_GITHUB_TOKEN", "ghp-fake")
        .output()
        .unwrap();

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        rig.home
            .path()
            .join("work/epik-agent/Epik/issue-7")
            .is_dir(),
        "the failed run's worktree is retained"
    );
    let logs = run_logs(&rig);
    let [(name, logged)] = logs.as_slice() else {
        panic!("the log survives beside the forensics: {logs:?}");
    };
    assert!(name.starts_with("issue-7-"), "{name}");
    assert!(
        matches!(
            logged.last(),
            Some(RunEvent::Finished(Verdict::Failed { .. }))
        ),
        "the log's last word is the failure: {logged:?}"
    );
}

#[test]
fn two_runs_of_the_same_issue_leave_two_log_files() {
    let rig = Rig::new();
    let root = TempDir::new().unwrap();
    let (origin_dir, sha) = origin(&root);
    rig.tool(
        "claude",
        "echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"done\"}'",
    );
    // Each run gets its own little GitHub, so both walk the same
    // open-then-closed arc; the home — and its logs — persists across both.
    for _ in 0..2 {
        let port = little_github(&sha, true);
        rig.conductable(&origin_dir, port);
        let output = rig
            .command(&["--issue", "7", "--target", "main"])
            .env("PATH", rig.path_with_real_git())
            .env("EPIK_GITHUB_TOKEN", "ghp-fake")
            .output()
            .unwrap();
        assert_eq!(code(&output), 0, "{}", stderr(&output));
    }

    let logs = run_logs(&rig);
    assert_eq!(
        logs.len(),
        2,
        "one file per run, named by start time: {logs:?}"
    );
    for (name, logged) in &logs {
        assert!(name.starts_with("issue-7-"), "{name}");
        assert_eq!(
            logged.last(),
            Some(&RunEvent::Finished(Verdict::Done)),
            "each file carries its own whole run"
        );
    }
}

#[test]
fn a_signal_winds_the_run_up_and_the_agents_whole_group_dies_with_it() {
    let rig = Rig::new();
    let root = TempDir::new().unwrap();
    let (origin_dir, sha) = origin(&root);
    let port = little_github(&sha, true);
    rig.conductable(&origin_dir, port);
    // A claude that digs in: it notes its pid, spawns a child of its own —
    // the orphan candidate — and sleeps far past the test. `/bin/sleep` by
    // its absolute path: the rig's PATH carries only the fakes and git.
    rig.tool(
        "claude",
        "echo $$ > \"$(dirname \"$0\")/claude.pid\"\n\
         /bin/sleep 300 &\n\
         echo $! > \"$(dirname \"$0\")/child.pid\"\n\
         wait",
    );

    let mut worker = rig
        .command(&["--issue", "7", "--target", "main"])
        .env("PATH", rig.path_with_real_git())
        .env("EPIK_GITHUB_TOKEN", "ghp-fake")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // A pid file exists before its number lands, so the poll waits for the
    // contents, not the file.
    let noted = |name: &str| {
        fs::read_to_string(rig.bin.path().join(name))
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    eventually(
        "the scripted claude to start",
        Duration::from_mins(1),
        || noted("claude.pid").is_some() && noted("child.pid").is_some(),
    );
    let claude_pid = noted("claude.pid").unwrap();
    let child_pid = noted("child.pid").unwrap();
    assert!(
        alive(&claude_pid) && alive(&child_pid),
        "the run is in flight"
    );

    assert!(
        Command::new("kill")
            .args(["-INT", &worker.id().to_string()])
            .status()
            .unwrap()
            .success()
    );

    eventually("the worker to exit", Duration::from_secs(30), || {
        worker.try_wait().unwrap().is_some()
    });
    let output = worker.wait_with_output().unwrap();
    assert_eq!(code(&output), 1, "a canceled run is a failed run");
    assert!(stderr(&output).contains("canceled"), "{}", stderr(&output));
    eventually(
        "the agent's whole process group to die",
        Duration::from_secs(10),
        || !alive(&claude_pid) && !alive(&child_pid),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Canceled"),
        "the stop is narrated in the event stream"
    );
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
