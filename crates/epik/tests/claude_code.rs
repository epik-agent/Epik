//! The Claude Code path end to end: the scripted engine — canned
//! stream-json from a real process — and, opt-in only, the real CLI.
//!
//! The scripted engine is the deterministic stand-in for the claude
//! binary: `sh -c` printf of captured fixture lines — inline only, never
//! a written script file (the ETXTBSY lesson) — and it exercises the
//! whole path: Agent → Event stream → interpret → typed Updates. Only
//! the live tier, behind EPIK_CLAUDE_LIVE=1, touches a real model.

#![cfg(all(feature = "testing", unix))]

use std::process::Command;
use std::time::{Duration, Instant};

use epik::agent::claude_code::{ClaudeCode, Update, interpret};
use epik::agent::{Agent, Event, Exit};
use epik::testing::Scratch;
use epik::testing::agent::shell;

const SESSION: &str = include_str!("../src/agent/claude_code/fixtures/session.jsonl");
const RESULT_ERROR: &str = include_str!("../src/agent/claude_code/fixtures/result_error.json");

/// The scripted engine: a child that says `lines` — one printf argument
/// each, single-quoted for the shell — and then runs `coda`.
fn canned(lines: &[&str], coda: &str) -> Agent {
    let quoted: Vec<String> = lines
        .iter()
        .map(|line| format!("'{}'", line.replace('\'', r"'\''")))
        .collect();
    shell(&format!("printf '%s\\n' {}; {coda}", quoted.join(" ")))
}

/// Reads the whole run: every interpreted Update in order, and the
/// final exit.
fn updates_of(mut agent: Agent) -> (Vec<Update>, Result<Exit, String>) {
    let updates = agent
        .events()
        .filter_map(|event| match event {
            Event::Stdout { line } => Some(interpret(&line)),
            Event::Stderr { .. } => None,
        })
        .flatten()
        .collect();
    (updates, agent.wait().map_err(|error| format!("{error:#}")))
}

#[test]
fn a_canned_session_yields_its_updates_through_a_real_process() {
    let lines: Vec<&str> = SESSION.lines().collect();
    let (updates, exit) = updates_of(canned(&lines, "true"));

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
fn a_canned_error_session_surfaces_the_error_result() {
    let (updates, exit) = updates_of(canned(&[RESULT_ERROR.trim()], "exit 1"));

    let [Update::Result { ok, text, .. }] = updates.as_slice() else {
        panic!("one result: {updates:?}");
    };
    assert!(!ok);
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
    let mut events = engine.events();
    assert!(events.any(|event| matches!(
        event,
        Event::Stdout { line } if interpret(&line)
            .iter()
            .any(|update| matches!(update, Update::Session { .. }))
    )));
    drop(events);

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
    if std::env::var("EPIK_CLAUDE_LIVE").as_deref() != Ok("1") {
        eprintln!("EPIK_CLAUDE_LIVE is unset; skipping");
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
            .success()
    );

    let agent = ClaudeCode {
        binary,
        cwd: scratch.path().to_owned(),
        prompt: "create a file named hello.txt containing 'hello' and nothing else".to_owned(),
        model: None,
        api_key: None,
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
