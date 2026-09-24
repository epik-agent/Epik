//! The Claude Code path end to end: the scripted engine — canned
//! stream-json from a real process — and, opt-in only, the real CLI.
//!
//! The scripted engine is the deterministic stand-in for the claude
//! binary: `sh -c` printf of captured fixture lines — inline only, never
//! a written script file (the ETXTBSY lesson) — and it exercises the
//! whole path: Process → stdout lines → interpret → typed Updates. Only
//! the live tier, behind EPIK_LIVE_CLAUDE=1, touches a real model.

#![cfg(all(feature = "testing", unix))]

use std::process::Command;
use std::time::{Duration, Instant};

use epik_core::agent::claude::ClaudeCode;
use epik_core::agent::claude::fixtures::{RESULT_ERROR, SESSION};
use epik_core::agent::{Agent, Update};
use epik_core::process::{Exit, Process};
use epik_core::temp::Scratch;

/// The scripted engine: `sh -c`, from `/`, saying `lines` — one printf
/// argument each, single-quoted for the shell — and then running
/// `coda`. Inline, never a written script file: the ETXTBSY lesson.
fn canned(lines: &[&str], coda: &str) -> Process {
    let quoted: Vec<String> = lines
        .iter()
        .map(|line| format!("'{}'", line.replace('\'', r"'\''")))
        .collect();
    let script = format!("printf '%s\\n' {}; {coda}", quoted.join(" "));
    Process::new("sh", vec!["-c".to_owned(), script], "/", [], None).expect("sh starts")
}

/// Reads the whole run: every interpreted Update in order, and the
/// final exit.
fn updates_of(mut process: Process) -> (Vec<Update>, Result<Exit, String>) {
    let updates = process
        .stdout()
        .flat_map(|line| ClaudeCode::interpret(&line))
        .collect();
    (
        updates,
        process.wait().map_err(|error| format!("{error:#}")),
    )
}

#[test]
fn a_canned_session_yields_its_updates_through_a_real_process() {
    let lines: Vec<&str> = SESSION.lines().collect();
    let (updates, exit) = updates_of(canned(&lines, "true"));

    assert_eq!(updates.len(), 4, "{updates:?}");
    assert!(
        matches!(&updates[0], Update::Session { model, .. } if model == "claude-fable-5"),
        "{:?}",
        updates[0]
    );
    assert!(
        matches!(&updates[1], Update::ToolUse { name } if name == "Write"),
        "{:?}",
        updates[1]
    );
    assert!(
        matches!(&updates[2], Update::Text { .. }),
        "{:?}",
        updates[2]
    );
    assert!(
        matches!(
            &updates[3],
            Update::Result {
                ok: true,
                cost_usd: Some(_),
                ..
            }
        ),
        "{:?}",
        updates[3]
    );
    assert_eq!(exit, Ok(Exit::Code(0)));
}

#[test]
fn a_canned_error_session_surfaces_the_error_result() {
    let (updates, exit) = updates_of(canned(&[RESULT_ERROR.trim()], "exit 1"));

    let [Update::Result { ok, text, .. }] = updates.as_slice() else {
        panic!("one result: {updates:?}");
    };
    assert!(!ok, "an error result is not ok");
    assert_eq!(text, "Reached maximum number of turns (1)");
    assert_eq!(exit, Ok(Exit::Code(1)));
}

/// Polls the process listing until no command line is exactly
/// `command`, giving up after a few seconds: `Ok` when gone, else the
/// survivors as listed. Exactly, so a shell whose own arguments mention
/// the marker — a developer's, or a harness's — is not mistaken for it.
fn gone(command: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let listed = Command::new("pgrep")
            .args(["-x", "-f", "-l", command])
            .output()
            .expect("pgrep runs");
        if !listed.status.success() {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(String::from_utf8_lossy(&listed.stdout).into_owned());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn dropping_an_engine_mid_stream_takes_it_with_it() {
    let lines: Vec<&str> = SESSION.lines().take(3).collect();
    let engine = canned(&lines, "sleep 6379");

    // The stream is flowing — the session start has been interpreted —
    // and the engine now hangs.
    let mut lines = engine.stdout();
    assert!(
        lines.any(|line| {
            ClaudeCode::interpret(&line)
                .iter()
                .any(|update| matches!(update, Update::Session { .. }))
        }),
        "the session start reached the reader"
    );
    drop(lines);

    drop(engine);
    if let Err(survivors) = gone("sleep 6379") {
        panic!("the hanging engine should be killed with its process group; left: {survivors}");
    }
}

/// The live tier: the real CLI, its logged-in auth, a real model — only
/// ever on request. This is the one test in the project that touches an
/// LLM.
#[test]
fn a_live_claude_writes_the_file_it_was_asked_for() {
    if std::env::var("EPIK_LIVE_CLAUDE").as_deref() != Ok("1") {
        eprintln!("EPIK_LIVE_CLAUDE is unset; skipping");
        return;
    }
    let binary = String::from_utf8(
        Command::new("which")
            .arg("claude")
            .output()
            .expect("which runs")
            .stdout,
    )
    .expect("a path is text")
    .trim()
    .to_owned();
    assert!(!binary.is_empty(), "no claude CLI on PATH");

    let scratch = Scratch::new("claude-live");
    assert!(
        Command::new("git")
            .args(["-C", scratch.path(), "init", "--quiet"])
            .status()
            .expect("git runs")
            .success(),
        "the scratch repository was made"
    );

    let agent = ClaudeCode {
        binary: binary.into(),
        cwd: scratch.0.clone(),
        prompt: "create a file named hello.txt containing 'hello' and nothing else".to_owned(),
        model: None,
        api_key: None,
        resume: None,
    }
    .start()
    .expect("claude starts");
    let (updates, exit) = updates_of(agent);

    assert!(
        updates
            .iter()
            .any(|update| matches!(update, Update::Session { .. })),
        "{updates:?}"
    );
    assert!(
        matches!(updates.last(), Some(Update::Result { ok: true, .. })),
        "{updates:?}"
    );
    assert_eq!(exit, Ok(Exit::Code(0)));
    let written = std::fs::read_to_string(scratch.join("hello.txt")).expect("the file exists");
    assert_eq!(written.trim(), "hello");
}
