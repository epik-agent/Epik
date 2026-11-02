//! `AcpClient`, with the Claude Code and Codex profiles, run against the fake ACP agent in `src/bin/fake-acp-agent.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use epik_core::agent::{AcpClient, Agent, AgentOutcome, AgentProfile, AgentRequest};
use serde_json::Value;

/// A fresh, empty directory for one test, under the target directory.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("acp_client")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The Claude Code profile, running the fake agent with `flags`.
fn fake_profile(flags: &[&str]) -> AgentProfile {
    let mut command = vec![env!("CARGO_BIN_EXE_fake-acp-agent").to_string()];
    command.extend(flags.iter().map(|flag| flag.to_string()));
    AgentProfile {
        command,
        quiet_period: Duration::from_millis(500),
        ..AgentProfile::claude()
    }
}

/// The Codex profile, running the fake agent with `flags`.
fn fake_codex(flags: &[&str]) -> AcpClient {
    AcpClient::new(AgentProfile {
        command: fake_profile(flags).command,
        quiet_period: Duration::from_millis(500),
        ..AgentProfile::codex()
    })
}

fn fake(flags: &[&str]) -> AcpClient {
    AcpClient::new(fake_profile(flags))
}

fn request(dir: &Path) -> AgentRequest {
    AgentRequest {
        prompt: "Implement issue #7.".into(),
        issue: 7,
        branch: "epic/1-thing".into(),
        cwd: dir.to_path_buf(),
        jsonl_path: dir.join("7.jsonl"),
        err_path: dir.join("7.err"),
        model: None,
    }
}

fn run(agent: &AcpClient, request: &AgentRequest) -> AgentOutcome {
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
    assert!(error.contains("Claude Code"), "{error}");
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
    let agent = AcpClient::new(AgentProfile {
        command: vec![dir.join("no-such-agent").display().to_string()],
        ..AgentProfile::claude()
    });

    let outcome = run(&agent, &request);

    assert!(!outcome.exited_cleanly);
    assert_eq!(outcome.stop_reason, "agent_failed");
    assert!(outcome.error.is_some());
}

#[test]
fn the_claude_profile_is_claude_code_through_its_pinned_adapter() {
    let profile = AgentProfile::claude();

    assert_eq!(profile.id, "claude");
    assert_eq!(profile.name, "Claude Code");
    assert_eq!(
        profile.command.last().unwrap(),
        "@agentclientprotocol/claude-agent-acp@0.99.0"
    );
    assert_eq!(profile.unattended_mode, "auto");
    assert!(profile.reports_cost);
    assert_eq!(profile.quiet_period, Duration::from_secs(300));
}

#[test]
fn an_offered_model_is_set_before_prompting() {
    let dir = scratch("set_model");
    let request = AgentRequest {
        model: Some("opus".into()),
        ..request(&dir)
    };

    let outcome = run(&fake(&["--models", "sonnet,opus"]), &request);

    assert!(outcome.exited_cleanly, "{outcome:?}");
    let log = log(&request);
    let set = &sent(&log, "session/set_config_option")[0];
    assert_eq!(set["params"]["configId"], "model");
    assert_eq!(set["params"]["value"], "opus");
    let err = std::fs::read_to_string(&request.err_path).unwrap();
    assert!(err.contains("set \"model\" to opus"), "{err}");
    let position = |method: &str| {
        log.iter()
            .position(|line| line["to_agent"]["method"] == method)
            .unwrap()
    };
    assert!(position("session/set_config_option") < position("session/prompt"));
}

#[test]
fn no_model_sets_nothing() {
    let dir = scratch("no_model");
    let request = request(&dir);

    let outcome = run(&fake(&["--models", "sonnet,opus"]), &request);

    assert!(outcome.exited_cleanly, "{outcome:?}");
    assert!(sent(&log(&request), "session/set_config_option").is_empty());
}

#[test]
fn a_model_not_offered_stops_before_prompting() {
    let dir = scratch("model_not_offered");
    let request = AgentRequest {
        model: Some("gpt-5".into()),
        ..request(&dir)
    };

    let outcome = run(&fake(&["--models", "sonnet,opus"]), &request);

    assert!(!outcome.exited_cleanly);
    assert_eq!(outcome.stop_reason, "no_model");
    let error = outcome.error.unwrap();
    assert!(error.contains("gpt-5"), "{error}");
    assert!(error.contains("sonnet, opus"), "{error}");
    assert!(sent(&log(&request), "session/prompt").is_empty());
}

#[test]
fn a_profile_that_does_not_report_cost_reports_none() {
    let dir = scratch("no_cost");
    let request = request(&dir);
    let agent = AcpClient::new(AgentProfile {
        reports_cost: false,
        ..fake_profile(&["--cost", "1.25"])
    });

    let outcome = run(&agent, &request);

    assert!(outcome.exited_cleanly, "{outcome:?}");
    assert_eq!(outcome.cost_usd, None);
}

#[test]
fn the_profile_s_environment_reaches_the_agent() {
    let dir = scratch("env");
    let request = request(&dir);
    let agent = AcpClient::new(AgentProfile {
        env: vec![("EPIK_FAKE".into(), "from the profile".into())],
        ..fake_profile(&["--echo-env", "EPIK_FAKE"])
    });

    let outcome = run(&agent, &request);

    assert!(outcome.exited_cleanly, "{outcome:?}");
    let err = std::fs::read_to_string(&request.err_path).unwrap();
    assert!(err.contains("EPIK_FAKE=from the profile"), "{err}");
}

#[test]
fn check_passes_for_an_offered_model_or_none() {
    let dir = scratch("check_ok");
    let agent = fake(&["--models", "sonnet,opus"]);

    assert_eq!(agent.check(&dir, Some("opus")), Ok(()));
    assert_eq!(agent.check(&dir, None), Ok(()));
}

#[test]
fn check_with_a_model_not_offered_fails_listing_the_models() {
    let dir = scratch("check_model");

    let error = fake(&["--models", "sonnet,opus"])
        .check(&dir, Some("gpt-5"))
        .unwrap_err();

    assert!(error.contains("Claude Code"), "{error}");
    assert!(error.contains("gpt-5"), "{error}");
    assert!(error.contains("sonnet, opus"), "{error}");
}

#[test]
fn check_with_a_model_and_no_model_option_fails() {
    let dir = scratch("check_no_option");

    let error = fake(&[]).check(&dir, Some("opus")).unwrap_err();

    assert!(error.contains("Claude Code"), "{error}");
    assert!(error.contains("opus"), "{error}");
}

#[test]
fn check_without_the_unattended_mode_fails_naming_it() {
    let dir = scratch("check_no_auto");

    let error = fake(&["--mode", "default", "--modes", "default,plan"])
        .check(&dir, None)
        .unwrap_err();

    assert!(error.contains("Claude Code"), "{error}");
    assert!(error.contains("auto"), "{error}");
    assert!(error.contains("default, plan"), "{error}");
}

#[test]
fn check_with_an_agent_that_cannot_start_fails() {
    let dir = scratch("check_no_command");
    let agent = AcpClient::new(AgentProfile {
        command: vec![dir.join("no-such-agent").display().to_string()],
        ..AgentProfile::claude()
    });

    let error = agent.check(&dir, None).unwrap_err();

    assert!(error.contains("Claude Code"), "{error}");
}

#[test]
fn the_codex_profile_is_codex_through_its_pinned_adapter() {
    let profile = AgentProfile::codex();

    assert_eq!(profile.id, "codex");
    assert_eq!(profile.name, "Codex");
    assert_eq!(
        profile.command.last().unwrap(),
        "@agentclientprotocol/codex-acp@2.5.0"
    );
    assert_eq!(profile.unattended_mode, "agent");
    assert!(profile.env.is_empty(), "the adapter runs its own Codex");
    assert!(!profile.reports_cost);
    assert_eq!(profile.quiet_period, Duration::from_secs(5));
}

/// Codex's session modes, with `agent` current, as codex-acp 2.5.0 offers
/// them.
const CODEX_MODES: [&str; 4] = [
    "--mode",
    "agent",
    "--modes",
    "read-only,workspace-write,agent,agent-full-access",
];

#[test]
fn a_codex_run_in_its_agent_mode_succeeds_with_no_cost() {
    let dir = scratch("codex");
    let request = request(&dir);
    let mut flags = CODEX_MODES.to_vec();
    flags.push("--no-cost");

    let outcome = run(&fake_codex(&flags), &request);

    assert!(outcome.exited_cleanly, "{outcome:?}");
    assert_eq!(outcome.stop_reason, "end_turn");
    assert_eq!(outcome.cost_usd, None);
    assert!(sent(&log(&request), "session/set_mode").is_empty());
}

#[test]
fn a_codex_session_not_in_agent_mode_is_switched_to_it() {
    let dir = scratch("codex_set_mode");
    let request = request(&dir);

    let outcome = run(
        &fake_codex(&[
            "--mode",
            "read-only",
            "--modes",
            "read-only,agent",
            "--no-cost",
        ]),
        &request,
    );

    assert!(outcome.exited_cleanly, "{outcome:?}");
    let set_mode = &sent(&log(&request), "session/set_mode")[0];
    assert_eq!(set_mode["params"]["modeId"], "agent");
}

#[test]
fn check_of_codex_without_its_agent_mode_fails_naming_it() {
    let dir = scratch("codex_check_no_agent");

    let error = fake_codex(&[
        "--mode",
        "read-only",
        "--modes",
        "read-only,workspace-write",
    ])
    .check(&dir, None)
    .unwrap_err();

    assert!(error.contains("Codex"), "{error}");
    assert!(error.contains("`agent`"), "{error}");
    assert!(error.contains("read-only, workspace-write"), "{error}");
}

#[test]
fn a_prompt_that_fails_with_codex_error_info_records_that_reason() {
    let dir = scratch("codex_error");
    let request = request(&dir);
    let mut flags = CODEX_MODES.to_vec();
    flags.extend(["--codex-error", "usageLimitExceeded"]);

    let outcome = run(&fake_codex(&flags), &request);

    assert!(!outcome.exited_cleanly);
    assert_eq!(outcome.stop_reason, "usageLimitExceeded");
    let error = outcome.error.unwrap();
    assert_eq!(
        error,
        "Codex stopped: Quota exceeded. Check your plan and billing details."
    );
}
