//! The runner and the launcher, end to end: real processes, the real
//! binary — located by cargo itself — and the scripted Agent. All
//! deterministic; no network, no LLM.

use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use epik::agent::{Agent, Event, Exit, Handle, Scripted, Secret, Task, run};

fn runner() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_epik-agent"))
}

fn launch(agent: &impl Agent) -> (Receiver<Event>, Handle) {
    let (events_in, events) = channel();
    let handle = run(agent, runner(), events_in).expect("the runner spawns");
    (events, handle)
}

/// Every event of a run, in order — the channel closes when the run is
/// over.
fn drain(events: Receiver<Event>) -> Vec<Event> {
    events.into_iter().collect()
}

fn stdout_lines(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Stdout { line } => Some(line.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_run_is_started_then_its_lines_in_order_then_exited_zero() {
    let (events, handle) = launch(&Scripted::counting(3));
    let seen = drain(events);

    assert!(
        matches!(seen.first(), Some(Event::Started { .. })),
        "{seen:?}"
    );
    assert_eq!(stdout_lines(&seen), ["line 1", "line 2", "line 3"]);
    assert_eq!(
        seen.last(),
        Some(&Event::Exited(Exit {
            code: Some(0),
            signal: None,
        }))
    );
    assert_eq!(
        handle.wait(),
        Ok(Exit {
            code: Some(0),
            signal: None,
        })
    );
}

#[test]
fn stderr_is_captured_distinctly_and_a_nonzero_exit_reported() {
    let (events, handle) = launch(&Scripted::failing());
    let seen = drain(events);

    assert!(
        seen.iter().any(|event| matches!(
            event,
            Event::Stderr { line } if line == "it all went wrong"
        )),
        "{seen:?}"
    );
    assert!(stdout_lines(&seen).is_empty(), "stderr is not stdout");
    assert_eq!(
        handle.wait(),
        Ok(Exit {
            code: Some(3),
            signal: None,
        })
    );
}

#[test]
fn the_stdin_payload_reaches_the_child() {
    let (events, handle) = launch(&Scripted::stdin_echo("said into stdin"));
    assert_eq!(stdout_lines(&drain(events)), ["said into stdin"]);
    handle.wait().unwrap();
}

/// The secret reaches the child through the environment, and its bytes
/// appear in no Debug along the way — only in the event stream, because
/// the fixture deliberately echoed it.
#[test]
fn an_env_secret_reaches_the_child_and_no_debug_output() {
    let agent = Scripted::env_echo("EPIK_TEST_SECRET", Secret::from("hush-hush-bytes"));
    assert!(
        !format!("{:?}", agent.task()).contains("hush-hush-bytes"),
        "the Task redacts"
    );

    let (events, handle) = launch(&agent);
    assert!(
        !format!("{handle:?}").contains("hush-hush-bytes"),
        "the Handle has nothing to leak"
    );
    assert_eq!(stdout_lines(&drain(events)), ["hush-hush-bytes"]);
    handle.wait().unwrap();
}

#[test]
fn kill_ends_the_whole_tree_and_the_stream_still_settles() {
    let (events, handle) = launch(&Scripted::hanging());
    let Event::Started { pid } = events.recv().expect("the child starts") else {
        panic!("the first event is Started");
    };

    handle.kill();
    handle.kill(); // idempotent

    let seen = drain(events);
    assert_eq!(
        seen.last(),
        Some(&Event::Exited(Exit {
            code: None,
            signal: Some(libc::SIGKILL),
        })),
        "the stream still ends with Exited: {seen:?}"
    );
    assert_eq!(
        handle.wait(),
        Ok(Exit {
            code: None,
            signal: Some(libc::SIGKILL),
        })
    );

    // The child from Started is gone...
    #[allow(clippy::cast_possible_wrap)]
    let pid = pid as i32;
    settles(
        || unsafe { libc::kill(pid, 0) } == -1,
        "the child should be gone",
    );
    // ...and no orphaned sleep survives anywhere — the marker duration
    // appears in no process listing.
    settles(no_orphaned_sleep, "an orphaned sleep survives the kill");
}

/// Polls `check` briefly: process teardown is real-world asynchronous.
fn settles(check: impl Fn() -> bool, complaint: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "{complaint}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn no_orphaned_sleep() -> bool {
    let listing = std::process::Command::new("ps")
        .args(["-axo", "command"])
        .output()
        .expect("ps runs");
    // Whole-command matches only: a shell or editor merely *mentioning*
    // the marker (a grep, this file open in a tool) is not an orphan.
    !String::from_utf8_lossy(&listing.stdout)
        .lines()
        .any(|line| matches!(line.trim(), "sleep 6371" | "sh -c sleep 6371"))
}

/// An Agent is anything that yields a Task — the tests' own bare one
/// drives the fault and framing cases.
struct Bare(Task);

impl Agent for Bare {
    fn task(&self) -> Task {
        self.0.clone()
    }
}

fn shell(script: &str) -> Bare {
    Bare(Task {
        argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
        env: Vec::new(),
        cwd: "/".to_owned(),
        stdin: None,
    })
}

#[test]
fn a_nonexistent_program_is_a_legible_fault_not_a_hang() {
    let agent = Bare(Task {
        argv: vec!["/nonexistent/epik-no-such-program".to_owned()],
        env: Vec::new(),
        cwd: "/".to_owned(),
        stdin: None,
    });
    let (events, handle) = launch(&agent);

    let fault = handle.wait().unwrap_err();
    assert!(
        fault.contains("/nonexistent/epik-no-such-program"),
        "{fault}"
    );
    assert!(drain(events).is_empty(), "nothing ran, so nothing happened");
}

#[test]
fn an_absurdly_long_line_is_capped_with_the_truncation_noted() {
    let (events, handle) = launch(&shell("head -c 100000 /dev/zero | tr '\\0' x; echo"));
    let seen = drain(events);
    let lines = stdout_lines(&seen);
    assert_eq!(lines.len(), 1, "{seen:?}");
    assert!(lines[0].starts_with("xxx"), "the kept prefix is the line");
    assert!(
        lines[0].contains("more bytes truncated)"),
        "the cut is noted in the line"
    );
    assert!(
        lines[0].len() < 70_000,
        "the cap held: {} bytes",
        lines[0].len()
    );
    handle.wait().unwrap();
}

#[test]
fn non_utf8_output_is_decoded_lossily_rather_than_wedging() {
    let (events, handle) = launch(&shell("printf '\\377\\376ok\\n'"));
    let lines_owned: Vec<String> = stdout_lines(&drain(events))
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines_owned.len(), 1);
    assert!(lines_owned[0].contains("ok"), "{lines_owned:?}");
    assert!(
        lines_owned[0].contains('\u{fffd}'),
        "the bad bytes became replacement characters: {lines_owned:?}"
    );
    handle.wait().unwrap();
}
