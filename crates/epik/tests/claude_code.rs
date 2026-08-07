//! `ClaudeCode` through the #107 gauntlet, with a claude that is entirely
//! script — no key, no network, no Anthropic.
//!
//! The staging: each conformance [`Script`] is translated into the
//! stream-json feed a claude playing that script would print — assistant
//! lines for narrative and usage, real sleeps for silence, a result line for
//! the finish — and written beside a copy of the stub binary, which
//! [`ClaudeCode`] then spawns exactly as it would spawn the real thing. The
//! events the gauntlet judges all come out of the real wrapper: the real
//! spawn, the real reader thread, the real `recv_timeout`, the real kill.

// ClaudeCode is unix-only by decision; so, therefore, is its gauntlet.
#![cfg(unix)]
// Tests are entitled to panic. The allow-unwrap-in-tests clippy setting only
// covers #[test] functions, not the helpers here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use epik::agent::{
    AgentError, AgentEvent, Budget, ClaudeCode, CodingAgent, Play, Script, Stop, Task, TaskKind,
    conformance,
};
use epik::chat::StopToken;
use epik::event::{Money, Usage};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The stub performs a line spelled exactly so as raw bytes that are not
/// UTF-8. Its twin lives in `tests/bin/stub_claude.rs`.
const MANGLED: &str = "<mangled-utf8>";

/// The staging directories, alive for the whole test process: the agents
/// staged in them run long after the builder returns, and a `TempDir`
/// dropped early would pull the feed out from under a live stub.
static STAGES: Mutex<Vec<TempDir>> = Mutex::new(Vec::new());

/// Stages a raw feed as a fake claude: the beats beside a hard link to the
/// stub binary in a fresh directory, and a `ClaudeCode` pointed at the
/// link. The link is what lets the stub find its feed — beside its own
/// executable — with no environment variable smuggled past the task; it is
/// a link rather than a copy because the directories live for the whole
/// process, and they hang off `CARGO_TARGET_TMPDIR` so the link stays on
/// the stub's own filesystem and `cargo clean` sweeps the lot.
fn staged_feed(beats: &[(u64, String)]) -> ClaudeCode {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    fs::write(
        dir.path().join("feed.json"),
        serde_json::to_string(beats).unwrap(),
    )
    .unwrap();
    let binary = dir.path().join("claude");
    fs::hard_link(env!("CARGO_BIN_EXE_stub-claude"), &binary).unwrap();
    let agent = ClaudeCode::at(binary);
    STAGES.lock().unwrap().push(dir);
    agent
}

/// [`staged_feed`], from a conformance script.
#[allow(clippy::needless_pass_by_value)] // the conformance suite's builder signature
fn staged(script: Script) -> ClaudeCode {
    staged_feed(&feed(&script))
}

/// What a claude playing `script` would print: each beat a pause and a
/// line. The contract's claims are cumulative but the stream's per-message
/// bills are increments, so token claims are delta-encoded against the
/// high-water mark — a backsliding claim is a delta of nothing, which is
/// exactly how it must land — while a claimed cost rides as
/// `total_cost_usd`, cumulative by name. Consecutive silences accumulate
/// into the pause before the next line, which is how a quiet pipe works.
fn feed(script: &Script) -> Vec<(u64, String)> {
    let mut beats = Vec::new();
    let mut pause = Duration::ZERO;
    let mut high_water = Usage::default();
    for play in script {
        let line = match play {
            Play::Silence(gap) => {
                pause += *gap;
                continue;
            }
            Play::Progress(text) => json!({"type": "assistant", "message": {
                "content": [{"type": "text", "text": text}],
            }}),
            Play::Detail(value) => {
                json!({"type": "system", "subtype": "detail", "detail": value})
            }
            Play::Usage(claim) => {
                let line = billed(claim, &high_water);
                high_water = high_water.max(*claim);
                line
            }
            Play::Finish(stop) => resulted(stop, high_water),
        };
        let millis = u64::try_from(std::mem::take(&mut pause).as_millis()).unwrap();
        beats.push((millis, line.to_string()));
    }
    beats
}

/// An assistant line billing the increment from `high_water` to `claim`.
fn billed(claim: &Usage, high_water: &Usage) -> Value {
    let mut usage = json!({
        "input_tokens": claim.prompt_tokens.saturating_sub(high_water.prompt_tokens),
        "output_tokens": claim.completion_tokens.saturating_sub(high_water.completion_tokens),
    });
    if let Some(cache) = claim.cache_tokens {
        let read = cache.saturating_sub(high_water.cache_tokens.unwrap_or(0));
        usage["cache_read_input_tokens"] = json!(read);
    }
    let mut line = json!({"type": "assistant", "message": {"content": [], "usage": usage}});
    if let Some(cost) = claim.cost {
        line["total_cost_usd"] = json!(usd(cost.micro_usd));
    }
    line
}

/// The result line for `stop`, carrying the run's cumulative totals — the
/// authoritative reading, subagents and all. Only `Completed` and `Died`
/// have a spelling a real claude could print; the gauntlet never reaches a
/// result line with any other stop.
fn resulted(stop: &Stop, total: Usage) -> Value {
    let (subtype, is_error, text) = match stop {
        Stop::Died { error } => ("error_during_execution", true, error.clone()),
        _ => ("success", false, String::new()),
    };
    let mut usage = json!({
        "input_tokens": total.prompt_tokens,
        "output_tokens": total.completion_tokens,
    });
    if let Some(cache) = total.cache_tokens {
        usage["cache_read_input_tokens"] = json!(cache);
    }
    let mut line = json!({
        "type": "result",
        "subtype": subtype,
        "is_error": is_error,
        "result": text,
        "usage": usage,
    });
    if let Some(cost) = total.cost {
        line["total_cost_usd"] = json!(usd(cost.micro_usd));
    }
    line
}

/// Micro-dollars back onto the wire as the float claude speaks.
#[allow(clippy::cast_precision_loss)] // test money is far below 2^52
fn usd(micro_usd: u64) -> f64 {
    micro_usd as f64 / 1_000_000.0
}

/// The acceptance gate: the wrapper around a real process answers the same
/// gauntlet `Scripted` does — smuggled work, real silences, posthumous
/// chatter, backsliding totals, a pre-set stop token. The stall scripts
/// wait out genuine quiet (the suite's windows are tens of seconds), so
/// this test costs a couple of real minutes: the price of testing a real
/// `recv_timeout` instead of a simulated one.
#[test]
fn claude_code_conforms() {
    conformance::conforms(staged);
}

fn task() -> Task {
    Task {
        kind: TaskKind::Implement,
        prompt: "implement issue #0".to_owned(),
        worktree: PathBuf::from("."),
        env: Vec::new(),
        budget: Budget {
            max_tokens: None,
            max_cost: None,
            stall: Duration::from_secs(30),
        },
    }
}

fn run(agent: &ClaudeCode, task: &Task) -> (Vec<AgentEvent>, Result<Stop, AgentError>) {
    let mut events = Vec::new();
    let mut sink = |event: AgentEvent| events.push(event);
    let result = agent.run(task, &mut sink, &StopToken::new());
    (events, result)
}

#[test]
fn a_missing_binary_is_a_typed_refusal() {
    let agent = ClaudeCode::at("/nowhere/claude");

    let (events, result) = run(&agent, &task());

    match result {
        Err(AgentError::Unstartable { binary, .. }) => {
            assert_eq!(binary, PathBuf::from("/nowhere/claude"));
        }
        other => panic!("a binary that is not there is a typed refusal, got {other:?}"),
    }
    assert!(
        events.is_empty(),
        "a run that never started streams nothing at all: {events:?}"
    );
}

/// Real claude states `total_cost_usd` only on its result line, so this is
/// the shape a real overspend arrives in — and the budget must outrank the
/// result's own stop.
#[test]
fn a_cost_stated_only_on_the_result_line_still_breaches_the_budget() {
    let result_line = json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": "",
        "usage": {"input_tokens": 10, "output_tokens": 10},
        "total_cost_usd": 2.0,
    });
    let agent = staged_feed(&[(0, result_line.to_string())]);
    let over = Task {
        budget: Budget {
            max_tokens: None,
            max_cost: Some(Money {
                micro_usd: 1_000_000,
            }),
            stall: Duration::from_secs(30),
        },
        ..task()
    };

    let (events, result) = run(&agent, &over);

    assert_eq!(
        result.unwrap(),
        Stop::Spent,
        "$2 on the result line against a cap of $1"
    );
    assert_eq!(events.last(), Some(&AgentEvent::Finished(Stop::Spent)));
}

/// One line of bytes that are not UTF-8, mid-stream: noise to skip, never
/// a reason to write the whole process off.
#[test]
fn a_mangled_line_mid_stream_does_not_end_the_run() {
    let narrate =
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "before"}]}});
    let result_line = json!({"type": "result", "subtype": "success", "is_error": false, "result": "", "usage": {}});
    let agent = staged_feed(&[
        (0, narrate.to_string()),
        (0, MANGLED.to_owned()),
        (0, result_line.to_string()),
    ]);

    let (events, result) = run(&agent, &task());

    assert_eq!(
        result.unwrap(),
        Stop::Completed,
        "one mangled byte is not a death: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::Progress(text) if text == "before")),
        "the stream before the mangled line stands: {events:?}"
    );
}

#[test]
fn a_claude_that_exits_without_a_result_died() {
    // A feed with no result line: the stub says one thing and exits, which
    // is what a crashed claude looks like from the outside.
    let agent = staged(vec![Play::Progress("about to vanish".to_owned())]);

    let (events, result) = run(&agent, &task());

    let Ok(Stop::Died { error }) = result else {
        panic!("EOF without a result is a death, got {result:?}");
    };
    assert!(error.contains("without a result"), "{error}");
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Finished(Stop::Died { error })),
        "even a death finishes exactly once"
    );
}

/// The stub's init line reports its cwd, argv, and one probe env var, and
/// the fold passes it through as `Detail` — so what the wrapper actually
/// provisioned is read back out of the process it provisioned.
#[test]
fn the_wrapper_provisions_the_process_it_promised() {
    let worktree = tempfile::tempdir().unwrap();
    let agent = staged(vec![Play::Finish(Stop::Completed)]).allowing(["Bash", "Edit"]);
    let provisioned = Task {
        worktree: worktree.path().to_owned(),
        env: vec![("EPIK_STUB_PROBE".to_owned(), "injected".to_owned())],
        ..task()
    };
    // Note the stub runs in the worktree while its feed stays back in the
    // staging directory, beside the binary: cwd and feed discovery are
    // deliberately not the same thing.
    let (events, result) = run(&agent, &provisioned);

    assert_eq!(result.unwrap(), Stop::Completed);
    let init = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::Detail(value) if value["subtype"] == "init" => Some(value),
            _ => None,
        })
        .expect("the handshake passes through as Detail");
    let cwd = PathBuf::from(init["cwd"].as_str().unwrap());
    assert_eq!(
        cwd.canonicalize().unwrap(),
        worktree.path().canonicalize().unwrap(),
        "the process runs in the task's worktree"
    );
    assert_eq!(
        init["probe"], "injected",
        "the task's env reaches the process"
    );
    let argv: Vec<&str> = init["argv"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["-p", "implement issue #0"]),
        "the prompt rides as the -p argument: {argv:?}"
    );
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["--allowedTools", "Bash,Edit"]),
        "the tapering dial reaches the command line: {argv:?}"
    );
    assert!(
        !argv.contains(&"--dangerously-skip-permissions"),
        "a tapered run does not also get everything: {argv:?}"
    );
}
