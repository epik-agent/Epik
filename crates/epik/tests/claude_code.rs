//! The Claude Code path end to end: the scripted engine — canned
//! stream-json through the real runner — and, opt-in only, the real CLI.
//!
//! The scripted engine is the deterministic stand-in for the claude
//! binary: its child is `sh -c` printf of captured fixture lines —
//! inline only, never a written script file (the ETXTBSY lesson) — and
//! it exercises the whole path: runner → Event stream → interpret →
//! typed Updates. Only the live tier, behind EPIK_CLAUDE_LIVE=1,
//! touches a real model.

#![cfg(all(feature = "testing", unix))]

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;

use epik::agent::claude_code::{ClaudeCode, Update, interpret};
use epik::agent::{Agent, Event, Exit, run};
use epik::testing::Scratch;
use epik::testing::agent::Scripted;

const SESSION: &str = include_str!("../src/agent/claude_code/fixtures/session.jsonl");
const RESULT_ERROR: &str = include_str!("../src/agent/claude_code/fixtures/result_error.json");

/// The runner binary, from the standard cargo-built location — built on
/// demand for a bare `cargo test -p epik`, where cargo would not have.
fn runner() -> PathBuf {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let target = std::env::var("CARGO_TARGET_DIR").map_or_else(
        |_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    );
    let path = target.join(profile).join("epik-agent");
    if !path.exists() {
        let mut build = std::process::Command::new(env!("CARGO"));
        build.args(["build", "--quiet", "-p", "epik-agent"]);
        if profile == "release" {
            build.arg("--release");
        }
        assert!(
            build.status().expect("cargo runs").success(),
            "the runner builds"
        );
    }
    path
}

/// The scripted engine: a child that says `lines` — one printf argument
/// each, single-quoted for the shell — and then runs `coda`.
fn canned(lines: &[&str], coda: &str) -> Scripted {
    let quoted: Vec<String> = lines
        .iter()
        .map(|line| format!("'{}'", line.replace('\'', r"'\''")))
        .collect();
    Scripted::shell(format!("printf '%s\\n' {}; {coda}", quoted.join(" ")))
}

/// Launches `agent` and reads the whole run: every interpreted Update in
/// order, and the final exit.
fn updates_of(agent: &impl Agent) -> (Vec<Update>, Result<Exit, String>) {
    let (events_in, events) = channel();
    let handle = run(agent, &runner(), events_in).expect("the runner spawns");
    let updates = events
        .into_iter()
        .filter_map(|event| match event {
            Event::Stdout { line } => Some(interpret(&line)),
            _ => None,
        })
        .flatten()
        .collect();
    (updates, handle.wait())
}

#[test]
#[ignore = "needs the epik-agent runner, whose crate is gone; the agent subsystem is being replaced"]
fn a_canned_session_yields_its_updates_through_the_real_runner() {
    let lines: Vec<&str> = SESSION.lines().collect();
    let (updates, exit) = updates_of(&canned(&lines, "true"));

    assert_eq!(updates.len(), 4, "{updates:?}");
    assert!(matches!(&updates[0], Update::Session { model, .. } if model == "claude-fable-5"));
    assert!(matches!(&updates[1], Update::ToolUse { name } if name == "Write"));
    assert!(matches!(&updates[2], Update::Text { .. }));
    assert!(matches!(
        &updates[3],
        Update::Result {
            ok: true,
            cost_usd: Some(_),
            ..
        }
    ));
    assert_eq!(exit, Ok(Exit::Code(0)));
}

#[test]
#[ignore = "needs the epik-agent runner, whose crate is gone; the agent subsystem is being replaced"]
fn a_canned_error_session_surfaces_the_error_result() {
    let (updates, exit) = updates_of(&canned(&[RESULT_ERROR.trim()], "exit 1"));

    let [Update::Result { ok, text, .. }] = updates.as_slice() else {
        panic!("one result: {updates:?}");
    };
    assert!(!ok);
    assert_eq!(text, "Reached maximum number of turns (1)");
    assert_eq!(exit, Ok(Exit::Code(1)));
}

#[test]
#[ignore = "needs the epik-agent runner, whose crate is gone; the agent subsystem is being replaced"]
fn kill_still_works_on_an_engine_mid_stream() {
    let lines: Vec<&str> = SESSION.lines().take(3).collect();
    let engine = canned(&lines, "sleep 6379");
    let (events_in, events) = channel();
    let handle = run(&engine, &runner(), events_in).expect("the runner spawns");

    // The stream is flowing — the session start has been interpreted —
    // and the engine now hangs.
    let mut seen_session = false;
    while !seen_session {
        let event = events.recv().expect("the stream flows");
        if let Event::Stdout { line } = event {
            seen_session = interpret(&line)
                .iter()
                .any(|update| matches!(update, Update::Session { .. }));
        }
    }

    handle.kill();
    assert_eq!(handle.wait(), Ok(Exit::Signal(9)));
}

/// The live tier: the real CLI, its logged-in auth, a real model — only
/// ever on request. This is the one test in the project that touches an
/// LLM.
#[test]
#[ignore = "needs the epik-agent runner, whose crate is gone; the agent subsystem is being replaced"]
fn a_live_claude_writes_the_file_it_was_asked_for() {
    if std::env::var("EPIK_CLAUDE_LIVE").as_deref() != Ok("1") {
        eprintln!("EPIK_CLAUDE_LIVE is unset; skipping");
        return;
    }
    let binary = String::from_utf8(
        std::process::Command::new("which")
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
        std::process::Command::new("git")
            .args(["-C", scratch.path(), "init", "--quiet"])
            .status()
            .expect("git runs")
            .success()
    );

    let agent = ClaudeCode {
        binary,
        cwd: scratch.path().to_owned(),
        prompt: "create a file named hello.txt containing 'hello' and nothing else".to_owned(),
        model: None,
        api_key: None,
    };
    let (updates, exit) = updates_of(&agent);

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
