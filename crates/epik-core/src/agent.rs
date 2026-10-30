//! Coding agents, reached over the Agent Client Protocol (ACP).
//!
//! An [`Agent`] runs one prompt, unattended, in one directory, keeps its raw
//! output, and reports how it ended and what it cost as an [`AgentOutcome`].
//! [`ClaudeAcp`] is the only agent: Claude Code through `claude-agent-acp`.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PermissionOptionKind, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, SetSessionModeRequest,
    TextContent,
};
use agent_client_protocol::{AcpAgent, Client, ConnectionTo, LineDirection};
use chrono::{SecondsFormat, Utc};
use futures::future::{Either, select};
use serde_json::{Value, json};

/// One prompt for an agent to run.
#[derive(Debug, Clone)]
pub struct AgentRequest {
    /// What to ask, such as `/epik:issue 7 epic/1-thing`.
    pub prompt: String,
    /// The directory the agent works in.
    pub cwd: PathBuf,
    /// The Claude Code plugin to load, Epik's, so the prompt can use it.
    pub plugin_dir: PathBuf,
    /// Where every message in both directions goes, one JSON object per
    /// line: `{"to_agent": ...}`, `{"from_agent": ...}`, or `{"note": ...}`
    /// for something Epik did on its own, such as refusing a permission,
    /// each with `"at"`, when it arrived, in RFC 3339 and UTC.
    pub jsonl_path: PathBuf,
    /// Where the agent's stderr goes.
    pub err_path: PathBuf,
}

/// How an agent's run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentOutcome {
    /// The prompt returned and the agent went quiet, with no error.
    pub exited_cleanly: bool,
    /// Why the run stopped: the agent's ACP stop reason (`end_turn`,
    /// `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`), or one of
    /// Epik's: `cancelled` when the cancel signal stopped it,
    /// `no_auto_mode` when the session couldn't run in auto mode, and
    /// `agent_failed` when the agent couldn't start, exited, or broke the
    /// protocol.
    pub stop_reason: String,
    pub duration_ms: u64,
    /// The session's cumulative cost, from the last `usage_update` in US
    /// dollars, if the agent sent one.
    pub cost_usd: Option<f64>,
    /// What went wrong, if anything did.
    pub error: Option<String>,
}

/// A coding agent that runs one prompt unattended.
pub trait Agent {
    /// Runs `request` to its end, or until `cancel` is set, which stops the
    /// agent's process.
    fn run(&self, request: &AgentRequest, cancel: &AtomicBool) -> AgentOutcome;
}

/// Claude Code through the `claude-agent-acp` adapter.
#[derive(Debug, Clone)]
pub struct ClaudeAcp {
    command: Vec<String>,
    quiet_period: Duration,
}

/// The mode a run must be in. Nobody is there to answer permission prompts.
const AUTO: &str = "auto";

/// How often the cancel signal is checked.
const CANCEL_POLL: Duration = Duration::from_millis(250);

impl Default for ClaudeAcp {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeAcp {
    /// Runs `claude-agent-acp` 0.99.0 through `npx`, with a quiet period of
    /// 300 seconds.
    pub fn new() -> Self {
        // Accepted debt: the adapter is a Node package, so the Epik app
        // depends on Node, as Epik's plugin already does. It is pinned
        // because its behavior was checked at this version.
        let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
        Self {
            command: vec![
                npx.into(),
                "-y".into(),
                "@agentclientprotocol/claude-agent-acp@0.99.0".into(),
            ],
            quiet_period: Duration::from_secs(300),
        }
    }

    /// Runs `command`, a program and its arguments, as the ACP agent instead.
    pub fn command(mut self, command: Vec<String>) -> Self {
        self.command = command;
        self
    }

    /// How long the agent must send nothing, after its prompt returns,
    /// before the session is closed. The end of a prompt is not the end of
    /// the work: background subagents can still be running, and closing the
    /// session loses what they do.
    pub fn quiet_period(mut self, quiet_period: Duration) -> Self {
        self.quiet_period = quiet_period;
        self
    }

    async fn run_session(
        &self,
        request: &AgentRequest,
        record: Arc<Record>,
    ) -> Result<Ended, agent_client_protocol::Error> {
        let agent = AcpAgent::from_args(&self.command)?.with_debug({
            let record = record.clone();
            move |line, direction| record.line(line, direction)
        });
        let on_update = record.clone();
        let on_permission = record.clone();
        Client
            .builder()
            .name("epik")
            .on_receive_notification(
                async move |notification: SessionNotification, _connection| {
                    if let SessionUpdate::UsageUpdate(usage) = notification.update
                        && let Some(cost) = usage.cost
                        && cost.currency == "USD"
                    {
                        *on_update.cost_usd.lock().unwrap() = Some(cost.amount);
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |permission: RequestPermissionRequest, responder, _connection| {
                    on_permission.note(&format!(
                        "rejected a permission request: {}",
                        permission
                            .tool_call
                            .fields
                            .title
                            .as_deref()
                            .unwrap_or("(untitled)")
                    ));
                    responder.respond(RequestPermissionResponse::new(reject(&permission)))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(agent, async |connection| {
                self.converse(connection, request, &record).await
            })
            .await
    }

    async fn converse(
        &self,
        connection: ConnectionTo<agent_client_protocol::Agent>,
        request: &AgentRequest,
        record: &Record,
    ) -> Result<Ended, agent_client_protocol::Error> {
        // No file-system or terminal capabilities: the agent uses its own.
        connection
            .send_request(InitializeRequest::new(ProtocolVersion::V1))
            .block_task()
            .await?;

        let plugins = json!({"options": {"plugins": [
            {"type": "local", "path": request.plugin_dir},
        ]}});
        let session = connection
            .send_request(
                NewSessionRequest::new(&request.cwd)
                    .meta(serde_json::Map::from_iter([("claudeCode".into(), plugins)])),
            )
            .block_task()
            .await?;

        let modes = session.modes.as_ref();
        if modes.is_none_or(|modes| &*modes.current_mode_id.0 != AUTO) {
            let available: Vec<&str> = modes
                .map(|modes| modes.available_modes.iter().map(|m| &*m.id.0).collect())
                .unwrap_or_default();
            if !available.contains(&AUTO) {
                return Ok(Ended::NoAutoMode(format!(
                    "the session can't run in auto mode, which needs a recent model and \
                     can be turned off by an administrator; its modes are: {}",
                    available.join(", ")
                )));
            }
            connection
                .send_request(SetSessionModeRequest::new(session.session_id.clone(), AUTO))
                .block_task()
                .await?;
        }

        let response = connection
            .send_request(PromptRequest::new(
                session.session_id,
                vec![ContentBlock::Text(TextContent::new(request.prompt.clone()))],
            ))
            .block_task()
            .await?;

        loop {
            let quiet_for = record.last_heard.lock().unwrap().elapsed();
            if quiet_for >= self.quiet_period {
                break;
            }
            async_io::Timer::after(self.quiet_period - quiet_for).await;
        }

        let stop_reason = serde_json::to_value(response.stop_reason)
            .ok()
            .and_then(|value| value.as_str().map(String::from))
            .unwrap_or_default();
        Ok(Ended::Stopped(stop_reason))
    }
}

impl Agent for ClaudeAcp {
    fn run(&self, request: &AgentRequest, cancel: &AtomicBool) -> AgentOutcome {
        let started = Instant::now();
        let (ended, cost_usd) = match Record::create(request) {
            Err(error) => (
                Ended::Failed(format!("couldn't create the agent's logs: {error}")),
                None,
            ),
            Ok(record) => {
                let record = Arc::new(record);
                let cancelled = async {
                    while !cancel.load(Ordering::SeqCst) {
                        async_io::Timer::after(CANCEL_POLL).await;
                    }
                };
                // Dropping an unfinished session stops the agent's process
                // group.
                let ended = futures::executor::block_on(async {
                    match select(
                        pin!(self.run_session(request, record.clone())),
                        pin!(cancelled),
                    )
                    .await
                    {
                        Either::Left((Ok(ended), _)) => ended,
                        Either::Left((Err(error), _)) => Ended::Failed(error.to_string()),
                        Either::Right(_) => Ended::Cancelled,
                    }
                });
                (ended, *record.cost_usd.lock().unwrap())
            }
        };
        let (stop_reason, error) = match ended {
            Ended::Stopped(reason) => (reason, None),
            Ended::NoAutoMode(why) => ("no_auto_mode".into(), Some(why)),
            Ended::Failed(why) => ("agent_failed".into(), Some(why)),
            Ended::Cancelled => ("cancelled".into(), Some("cancelled by Epik".into())),
        };
        AgentOutcome {
            exited_cleanly: error.is_none(),
            stop_reason,
            duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            cost_usd,
            error,
        }
    }
}

/// How a run ended. Only `Stopped` is clean.
enum Ended {
    /// The prompt returned with this ACP stop reason and the agent went
    /// quiet.
    Stopped(String),
    NoAutoMode(String),
    Failed(String),
    Cancelled,
}

/// Rejects a permission request, once if the agent offers that.
fn reject(permission: &RequestPermissionRequest) -> RequestPermissionOutcome {
    [
        PermissionOptionKind::RejectOnce,
        PermissionOptionKind::RejectAlways,
    ]
    .iter()
    .find_map(|kind| {
        permission
            .options
            .iter()
            .find(|option| option.kind == *kind)
    })
    .map_or(RequestPermissionOutcome::Cancelled, |option| {
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option.option_id.clone()))
    })
}

/// What a run keeps as it goes: its logs, when the agent last sent
/// anything, and the session's cost so far.
struct Record {
    jsonl: Mutex<File>,
    err: Mutex<File>,
    last_heard: Mutex<Instant>,
    cost_usd: Mutex<Option<f64>>,
}

impl Record {
    fn create(request: &AgentRequest) -> std::io::Result<Self> {
        Ok(Self {
            jsonl: Mutex::new(File::create(&request.jsonl_path)?),
            err: Mutex::new(File::create(&request.err_path)?),
            last_heard: Mutex::new(Instant::now()),
            cost_usd: Mutex::new(None),
        })
    }

    fn line(&self, line: &str, direction: LineDirection) {
        // A line that isn't JSON is kept as a string.
        let message = || serde_json::from_str(line).unwrap_or_else(|_| Value::from(line));
        match direction {
            LineDirection::Stdin => self.json(json!({"to_agent": message()})),
            LineDirection::Stdout => {
                *self.last_heard.lock().unwrap() = Instant::now();
                self.json(json!({"from_agent": message()}));
            }
            LineDirection::Stderr => {
                let _ = self
                    .err
                    .lock()
                    .unwrap()
                    .write_all(format!("{line}\n").as_bytes());
            }
        }
    }

    fn note(&self, note: &str) {
        self.json(json!({"note": note}));
    }

    /// Writes `value` as a line, with when it arrived as `"at"`.
    fn json(&self, mut value: Value) {
        if let Value::Object(object) = &mut value {
            let at = Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true);
            object.insert("at".into(), Value::from(at));
        }
        // A log that can't be written mustn't stop the run.
        // One write per line, rather than one per JSON token.
        let _ = self
            .jsonl
            .lock()
            .unwrap()
            .write_all(format!("{value}\n").as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_written_says_when_it_arrived() {
        let dir = tempfile::tempdir().unwrap();
        let request = AgentRequest {
            prompt: String::new(),
            cwd: dir.path().to_owned(),
            plugin_dir: dir.path().to_owned(),
            jsonl_path: dir.path().join("1.jsonl"),
            err_path: dir.path().join("1.err"),
        };
        let record = Record::create(&request).unwrap();
        let before = chrono::Utc::now();

        record.line(r#"{"id":1}"#, LineDirection::Stdin);
        record.line(r#"{"id":1,"result":{}}"#, LineDirection::Stdout);
        record.line("not json", LineDirection::Stdout);
        record.note("rejected a permission request: rm");

        let text = std::fs::read_to_string(&request.jsonl_path).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 4);
        for line in &lines {
            let at = line["at"].as_str().unwrap_or_else(|| panic!("{line}"));
            let at = chrono::DateTime::parse_from_rfc3339(at).unwrap();
            assert!(at >= before && at <= chrono::Utc::now(), "{line}");
            assert!(at.offset().local_minus_utc() == 0, "{line}");
        }
        assert_eq!(lines[0]["to_agent"], json!({"id": 1}));
        assert_eq!(lines[2]["from_agent"], "not json");
        assert_eq!(lines[3]["note"], "rejected a permission request: rm");
    }
}
