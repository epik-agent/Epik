//! `ClaudeAcp` run against the fake ACP agent in `src/bin/fake-acp-agent.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use epik_core::agent::{Agent, AgentOutcome, AgentRequest, ClaudeAcp};
use serde_json::Value;

/// A fresh, empty directory for one test, under the target directory.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("claude_acp")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fake(flags: &[&str]) -> ClaudeAcp {
    let mut command = vec![env!("CARGO_BIN_EXE_fake-acp-agent").to_string()];
    command.extend(flags.iter().map(|flag| flag.to_string()));
    ClaudeAcp::new()
        .command(command)
        .quiet_period(Duration::from_millis(500))
}

fn request(dir: &Path) -> AgentRequest {
    AgentRequest {
        prompt: "Implement issue #7.".into(),
        issue: 7,
        branch: "epic/1-thing".into(),
        cwd: dir.to_path_buf(),
        jsonl_path: dir.join("7.jsonl"),
        err_path: dir.join("7.err"),
    }
}

fn run(agent: &ClaudeAcp, request: &AgentRequest) -> AgentOutcome {
    agent.run(request, &AtomicBool::new(false))
}

/// The log's lines, each parsed as JSON.
fn log(request: &AgentRequest) -> Vec<Value> {
    std::fs::read_to_string(&request.jsonl_path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The messages Epik sent to the agent whose method is `method`.
fn sent(log: &[Value], method: &str) -> Vec<Value> {
    log.iter()
        .filter_map(|line| line.get("to_agent"))
        .filter(|message| message["method"] == method)
        .cloned()
        .collect()
}

#[test]
fn a_clean_run_reports_its_stop_reason_and_cost() {
    let dir = scratch("clean");
    let request = request(&dir);

    let outcome = run(&fake(&["--cost", "1.25"]), &request);

    assert!(outcome.exited_cleanly, "{outcome:?}");
    assert_eq!(outcome.stop_reason, "end_turn");
    assert_eq!(outcome.cost_usd, Some(1.25));
    assert_eq!(outcome.error, None);
    assert!(outcome.duration_ms >= 500, "waited the quiet period");
}

#[test]
fn a_clean_run_logs_every_message_in_both_directions() {
    let dir = scratch("log");
    let request = request(&dir);

    run(&fake(&[]), &request);

    let log = log(&request);
    let initialize = &sent(&log, "initialize")[0];
    assert_eq!(initialize["params"]["protocolVersion"], 1);
    assert_eq!(
        initialize["params"]["clientCapabilities"]["terminal"],
        Value::Bool(false)
    );
    let new_session = &sent(&log, "session/new")[0];
    assert_eq!(new_session["params"]["cwd"], dir.to_str().unwrap());
    assert_eq!(
        new_session["params"]["_meta"]["claudeCode"]["options"]["plugins"],
        Value::Null,
        "the prompt carries the procedure, so no plugin is loaded"
    );
    let prompt = &sent(&log, "session/prompt")[0];
    assert_eq!(prompt["params"]["prompt"][0]["text"], "Implement issue #7.");
    let received: Vec<&Value> = log.iter().filter_map(|l| l.get("from_agent")).collect();
    assert!(
        received
            .iter()
            .any(|m| m["result"]["stopReason"] == "end_turn")
    );
    assert!(
        received
            .iter()
            .any(|m| m["params"]["update"]["sessionUpdate"] == "usage_update")
    );
}

#[test]
fn updates_after_the_prompt_returns_are_waited_for() {
    let dir = scratch("after_prompt");
    let request = request(&dir);

    let outcome = run(&fake(&["--cost", "0.5", "--after-prompt", "4"]), &request);

    assert!(outcome.exited_cleanly, "{outcome:?}");
    assert_eq!(outcome.cost_usd, Some(1.5), "the last usage_update's cost");
    assert!(outcome.duration_ms >= 400 + 500);
}

#[test]
fn a_permission_request_is_rejected_and_noted() {
    let dir = scratch("permission");
    let request = request(&dir);

    let outcome = run(&fake(&["--ask-permission"]), &request);

    assert!(outcome.exited_cleanly, "{outcome:?}");
    let log = log(&request);
    let answer = log
        .iter()
        .filter_map(|line| line.get("to_agent"))
        .find(|message| message["id"] == "permission-1")
        .expect("an answer to the permission request");
    assert_eq!(answer["result"]["outcome"]["outcome"], "selected");
    assert_eq!(answer["result"]["outcome"]["optionId"], "reject");
    assert!(
        log.iter()
            .filter_map(|line| line.get("note"))
            .any(|note| note.as_str().unwrap().contains("gh pr merge")),
        "{log:?}"
    );
}

#[test]
fn a_session_not_in_auto_mode_is_switched_to_it() {
    let dir = scratch("set_mode");
    let request = request(&dir);

    let outcome = run(
        &fake(&["--mode", "default", "--modes", "default,auto,plan"]),
        &request,
    );

    assert!(outcome.exited_cleanly, "{outcome:?}");
    let set_mode = &sent(&log(&request), "session/set_mode")[0];
    assert_eq!(set_mode["params"]["modeId"], "auto");
}

#[test]
fn a_session_without_auto_mode_stops_before_prompting() {
    let dir = scratch("no_auto");
    let request = request(&dir);

    let outcome = run(
        &fake(&["--mode", "default", "--modes", "default,plan"]),
        &request,
    );

    assert!(!outcome.exited_cleanly);
    assert_eq!(outcome.stop_reason, "no_auto_mode");
    let error = outcome.error.unwrap();
    assert!(error.contains("auto"), "{error}");
    assert!(error.contains("default, plan"), "{error}");
    assert!(sent(&log(&request), "session/prompt").is_empty());
}

#[test]
fn cancelling_stops_the_agent() {
    let dir = scratch("cancel");
    let request = request(&dir);
    let cancel = Arc::new(AtomicBool::new(false));
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            cancel.store(true, Ordering::SeqCst);
        })
    };

    let started = Instant::now();
    let outcome = fake(&["--hang"]).run(&request, &cancel);
    canceller.join().unwrap();

    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!outcome.exited_cleanly);
    assert_eq!(outcome.stop_reason, "cancelled");
    assert!(outcome.error.is_some());
}

#[test]
fn an_agent_that_exits_early_is_reported_with_its_stderr_kept() {
    let dir = scratch("exit_early");
    let request = request(&dir);

    let outcome = run(&fake(&["--exit-early"]), &request);

    assert!(!outcome.exited_cleanly);
    assert_eq!(outcome.stop_reason, "agent_failed");
    assert!(outcome.error.is_some());
    let err = std::fs::read_to_string(&request.err_path).unwrap();
    assert!(err.contains("fake-acp-agent: exiting early"), "{err}");
}

#[test]
fn an_agent_command_that_cannot_start_is_reported() {
    let dir = scratch("no_command");
    let request = request(&dir);
    let agent = ClaudeAcp::new().command(vec![dir.join("no-such-agent").display().to_string()]);

    let outcome = run(&agent, &request);

    assert!(!outcome.exited_cleanly);
    assert_eq!(outcome.stop_reason, "agent_failed");
    assert!(outcome.error.is_some());
}
