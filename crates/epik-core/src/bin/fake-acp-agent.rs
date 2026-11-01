//! A fake ACP agent for `epik-core`'s tests. It is not part of Epik.
//!
//! It speaks just enough of the Agent Client Protocol, newline-delimited
//! JSON-RPC on stdin and stdout, to stand in for `claude-agent-acp`: it
//! answers `initialize`, `session/new`, `session/set_mode`,
//! `session/set_config_option` and `session/prompt`, and sends `session/update` notifications including a
//! `usage_update` with a cost. Flags choose how it misbehaves:
//!
//! - `--mode <id>`: the session's current mode (default `auto`).
//! - `--modes <id,id,...>`: the modes the session offers (default `auto`).
//! - `--models <id,id,...>`: offer these models in a `model` config option,
//!   the first current (default: no such option). Setting it writes the model
//!   to stderr; setting one not offered is an error.
//! - `--echo-env <name>`: write the environment variable `name` to stderr
//!   when started.
//! - `--cost <amount>`: the cost the prompt reports (default `0.25`).
//! - `--after-prompt <n>`: after the prompt returns, send `n` more updates,
//!   100 ms apart; the last reports a cost one dollar higher.
//! - `--ask-permission`: during the prompt, ask the client for permission and
//!   wait for its answer.
//! - `--exit-early`: after the session opens, write to stderr and exit with
//!   status 3 instead of answering the prompt.
//! - `--hang`: never answer the prompt.
//!
//! It writes JSON by hand, not with `agent-client-protocol`, so the tests
//! check the client's wire format independently of the crate the client uses.

use std::io::{BufRead, Write};
use std::thread::sleep;
use std::time::Duration;

use serde_json::{Value, json};

struct Options {
    mode: String,
    modes: Vec<String>,
    models: Vec<String>,
    echo_env: Option<String>,
    cost: f64,
    after_prompt: u32,
    ask_permission: bool,
    exit_early: bool,
    hang: bool,
}

fn options() -> Options {
    let mut options = Options {
        mode: "auto".into(),
        modes: vec!["auto".into()],
        models: Vec::new(),
        echo_env: None,
        cost: 0.25,
        after_prompt: 0,
        ask_permission: false,
        exit_early: false,
        hang: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().expect("flag needs a value");
        match arg.as_str() {
            "--mode" => options.mode = value(),
            "--modes" => options.modes = value().split(',').map(String::from).collect(),
            "--models" => options.models = value().split(',').map(String::from).collect(),
            "--echo-env" => options.echo_env = Some(value()),
            "--cost" => options.cost = value().parse().expect("cost is a number"),
            "--after-prompt" => options.after_prompt = value().parse().expect("a count"),
            "--ask-permission" => options.ask_permission = true,
            "--exit-early" => options.exit_early = true,
            "--hang" => options.hang = true,
            other => panic!("unknown flag {other}"),
        }
    }
    options
}

fn send(message: Value) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{message}").expect("write to stdout");
    stdout.flush().expect("flush stdout");
}

fn update(session_id: &str, update: Value) {
    send(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": session_id, "update": update},
    }));
}

fn text(session_id: &str, text: &str) {
    update(
        session_id,
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}),
    );
}

fn usage(session_id: &str, cost: f64) {
    update(
        session_id,
        json!({
            "sessionUpdate": "usage_update",
            "used": 1000,
            "size": 200000,
            "cost": {"amount": cost, "currency": "USD"},
        }),
    );
}

/// The session's config options: a `model` option, if there are models.
fn config_options(models: &[String], current: &str) -> Value {
    if models.is_empty() {
        return json!([]);
    }
    let values: Vec<Value> = models
        .iter()
        .map(|model| json!({"value": model, "name": model}))
        .collect();
    json!([{
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": current,
        "options": values,
    }])
}

fn main() {
    let options = options();
    if let Some(name) = &options.echo_env {
        eprintln!("{name}={}", std::env::var(name).unwrap_or_default());
    }
    let mut model = options.models.first().cloned().unwrap_or_default();
    let session_id = "fake-session";
    let mut lines = std::io::stdin().lock().lines();
    while let Some(Ok(line)) = lines.next() {
        let message: Value = serde_json::from_str(&line).expect("a JSON-RPC message");
        let id = message["id"].clone();
        let reply = |result: Value| send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
        match message["method"].as_str() {
            Some("initialize") => reply(json!({
                "protocolVersion": 1,
                "agentCapabilities": {},
                "agentInfo": {"name": "fake-acp-agent", "version": "0.0.0"},
            })),
            Some("session/new") => {
                let modes: Vec<Value> = options
                    .modes
                    .iter()
                    .map(|mode| json!({"id": mode, "name": mode}))
                    .collect();
                reply(json!({
                    "sessionId": session_id,
                    "modes": {"currentModeId": options.mode, "availableModes": modes},
                    "configOptions": config_options(&options.models, &model),
                }));
                if options.exit_early {
                    eprintln!("fake-acp-agent: exiting early");
                    std::process::exit(3);
                }
            }
            Some("session/set_mode") => reply(json!({})),
            Some("session/set_config_option") => {
                let params = &message["params"];
                let value = params["value"].as_str().unwrap_or_default();
                eprintln!("fake-acp-agent: set {} to {value}", params["configId"]);
                if params["configId"] == "model" && options.models.iter().any(|m| m == value) {
                    model = value.to_owned();
                    reply(json!({"configOptions": config_options(&options.models, &model)}));
                } else {
                    send(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32602, "message": "no such option or value"},
                    }));
                }
            }
            Some("session/prompt") => {
                if options.hang {
                    continue;
                }
                text(session_id, "Working on it.");
                if options.ask_permission {
                    send(json!({
                        "jsonrpc": "2.0",
                        "id": "permission-1",
                        "method": "session/request_permission",
                        "params": {
                            "sessionId": session_id,
                            "toolCall": {"toolCallId": "tool-1", "title": "gh pr merge"},
                            "options": [
                                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                                {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
                            ],
                        },
                    }));
                    let answer = lines.next().expect("an answer").expect("a line");
                    let answer: Value = serde_json::from_str(&answer).expect("JSON");
                    text(
                        session_id,
                        &format!("Permission answer: {}", answer["result"]),
                    );
                }
                usage(session_id, options.cost);
                reply(json!({"stopReason": "end_turn"}));
                for n in 1..=options.after_prompt {
                    sleep(Duration::from_millis(100));
                    if n == options.after_prompt {
                        usage(session_id, options.cost + 1.0);
                    } else {
                        text(session_id, "A background subagent is still working.");
                    }
                }
            }
            _ => {
                if !id.is_null() {
                    send(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32601, "message": "method not found"},
                    }));
                }
            }
        }
    }
}
