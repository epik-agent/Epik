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

// Tests are entitled to panic. The allow-unwrap-in-tests clippy setting only
// covers #[test] functions, not the helpers here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use epik::agent::{
    AgentError, AgentEvent, Budget, ClaudeCode, CodingAgent, Play, Script, Stop, Task, TaskKind,
    conformance,
};
use epik::chat::StopToken;
use epik::event::Usage;
use serde_json::{Value, json};

/// Stages `script` as a fake claude: a feed file and a copy of the stub
/// binary in a fresh directory, and a `ClaudeCode` pointed at the copy. The
/// copy is what lets the stub find its feed — beside its own executable —
/// with no environment variable smuggled past the task. The directory
/// outlives the builder (`keep`) because the agent runs later.
#[allow(clippy::needless_pass_by_value)] // the conformance suite's builder signature
fn staged(script: Script) -> ClaudeCode {
    let dir = tempfile::tempdir().unwrap().keep();
    fs::write(
        dir.join("feed.json"),
        serde_json::to_string(&feed(&script)).unwrap(),
    )
    .unwrap();
    let binary = dir.join("claude");
    fs::copy(env!("CARGO_BIN_EXE_stub-claude"), &binary).unwrap();
    ClaudeCode::at(binary)
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
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::Finished(_))),
        "a run that errored never finished: {events:?}"
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
    let worktree = tempfile::tempdir().unwrap().keep();
    let agent = staged(vec![Play::Finish(Stop::Completed)]).allowing(["Bash", "Edit"]);
    let provisioned = Task {
        worktree: worktree.clone(),
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
        worktree.canonicalize().unwrap(),
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
