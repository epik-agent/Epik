//! Coding agents, reached over the Agent Client Protocol (ACP).
//!
//! An [`Agent`] runs one prompt, unattended, in one directory, keeps its raw
//! output, and reports how it ended and what it cost as an [`AgentOutcome`].
//! [`AcpClient`] runs every agent; what differs between agents is data, in
//! its [`AgentProfile`]: [`AgentProfile::claude`], Claude Code through
//! `claude-agent-acp`, or [`AgentProfile::codex`], Codex through
//! `codex-acp`.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, NewSessionResponse, PermissionOptionKind,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionConfigId, SessionConfigKind, SessionConfigOptionCategory,
    SessionConfigSelectOptions, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionModeRequest, TextContent,
};
use agent_client_protocol::{AcpAgent, Client, ConnectionTo, LineDirection};
use chrono::{SecondsFormat, Utc};
use futures::future::{Either, select};
use serde_json::{Value, json};

/// One prompt for an agent to run.
#[derive(Debug, Clone)]
pub struct AgentRequest {
    /// What to ask: what the work is, Epik's rules, and the procedure, from
    /// [`crate::prompt::prompt`].
    pub prompt: String,
    /// The issue the work is for.
    pub issue: u64,
    /// The branch the work's pull request goes into.
    pub branch: String,
    /// The directory the agent works in.
    pub cwd: PathBuf,
    /// Where every message in both directions goes, one JSON object per
    /// line: `{"to_agent": ...}`, `{"from_agent": ...}`, or `{"note": ...}`
    /// for something Epik did on its own, such as refusing a permission,
    /// each with `"at"`, when it arrived, in RFC 3339 and UTC.
    pub jsonl_path: PathBuf,
    /// Where the agent's stderr goes.
    pub err_path: PathBuf,
    /// The model to run, one the agent offers, or `None` for the agent's
    /// default.
    pub model: Option<String>,
}

/// How an agent's run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentOutcome {
    /// The prompt returned and the agent went quiet, with no error.
    pub exited_cleanly: bool,
    /// Why the run stopped: the agent's ACP stop reason (`end_turn`,
    /// `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`), or one of
    /// Epik's: `cancelled` when the cancel signal stopped it,
    /// `no_auto_mode` when the session couldn't run in the agent's
    /// unattended mode, `no_model` when it doesn't offer the model asked
    /// for, and `agent_failed` when the agent couldn't start, exited, or
    /// broke the protocol. A prompt Codex failed is stopped by its
    /// `codexErrorInfo`, such as `usageLimitExceeded`.
    pub stop_reason: String,
    pub duration_ms: u64,
    /// The session's cumulative cost, from the last `usage_update` in US
    /// dollars, if the agent reports cost and sent one.
    pub cost_usd: Option<f64>,
    /// What went wrong, if anything did.
    pub error: Option<String>,
}

/// A coding agent that runs one prompt unattended.
pub trait Agent {
    /// Runs `request` to its end, or until `cancel` is set, which stops the
    /// agent's process.
    fn run(&self, request: &AgentRequest, cancel: &AtomicBool) -> AgentOutcome;

    /// Whether a session in `cwd` could run unattended, with `model` if one
    /// is given, and if not, why not. It starts the agent, opens a session,
    /// and stops it without prompting, so it takes a few seconds.
    fn check(&self, cwd: &Path, model: Option<&str>) -> Result<(), String>;
}

/// A coding agent reached over ACP: how to start its adapter, and what
/// Epik needs to know to run it unattended.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentProfile {
    /// What settings call the agent, such as `claude`.
    pub id: String,
    /// What people call the agent, such as `Claude Code`.
    pub name: String,
    /// The ACP adapter: a program and its arguments.
    pub command: Vec<String>,
    /// The id of the session mode in which the agent works without asking
    /// permission. Nobody is there to answer permission prompts.
    pub unattended_mode: String,
    /// Environment variables the adapter runs with, besides Epik's own.
    pub env: Vec<(String, String)>,
    /// Whether the agent reports what a session cost in its
    /// `usage_update`s. If it doesn't, any cost it sends is ignored.
    pub reports_cost: bool,
    /// How long the agent must send nothing, after its prompt returns,
    /// before the session is closed. The end of a prompt is not always the
    /// end of the work: Claude Code's background subagents can still be
    /// running, and closing the session loses what they do.
    pub quiet_period: Duration,
}

impl AgentProfile {
    /// Claude Code: `claude-agent-acp` 0.99.0 through `npx`, in `auto` mode,
    /// reporting cost, with a quiet period of 300 seconds.
    pub fn claude() -> Self {
        // Accepted debt: the adapter is a Node package, so the Epik app
        // depends on Node, as Epik's plugin already does. It is pinned
        // because its behavior was checked at this version.
        let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
        Self {
            id: "claude".into(),
            name: "Claude Code".into(),
            command: vec![
                npx.into(),
                "-y".into(),
                "@agentclientprotocol/claude-agent-acp@0.99.0".into(),
            ],
            unattended_mode: "auto".into(),
            env: Vec::new(),
            reports_cost: true,
            quiet_period: Duration::from_secs(300),
        }
    }

    /// Codex: `codex-acp` 2.5.0 through `npx`, running the Codex it depends
    /// on, in `agent` mode, not reporting cost, with a quiet period of 5
    /// seconds.
    ///
    /// In `agent` mode Codex works in a sandbox with no network, and its own
    /// automatic reviewer answers its approvals, such as for `git push` and
    /// `gh`. Its prompt returns only when its work is done: it has no
    /// background subagents to wait for.
    pub fn codex() -> Self {
        let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
        Self {
            id: "codex".into(),
            name: "Codex".into(),
            command: vec![npx.into(), "-y".into(), CODEX_ACP.into()],
            unattended_mode: "agent".into(),
            env: Vec::new(),
            reports_cost: false,
            quiet_period: Duration::from_secs(5),
        }
    }
}

/// The Codex adapter's npm package, pinned to the version whose behavior
/// was checked. It brings its own Codex.
pub const CODEX_ACP: &str = "@agentclientprotocol/codex-acp@2.5.0";

/// Runs the agent an [`AgentProfile`] describes, over ACP.
#[derive(Debug, Clone)]
pub struct AcpClient {
    profile: AgentProfile,
}

/// How often the cancel signal is checked.
const CANCEL_POLL: Duration = Duration::from_millis(250);

impl AcpClient {
    pub fn new(profile: AgentProfile) -> Self {
        Self { profile }
    }

    /// Starts the agent and runs `main` on the connection to it, keeping
    /// what passes in `record`. The agent stops when `main` returns.
    async fn connect<R>(
        &self,
        record: Arc<Record>,
        main: impl AsyncFnOnce(
            ConnectionTo<agent_client_protocol::Agent>,
        ) -> Result<R, agent_client_protocol::Error>,
    ) -> Result<R, agent_client_protocol::Error> {
        let config = AcpAgent::from_args(&self.profile.command)?
            .into_config()
            .envs(self.profile.env.iter().cloned());
        let agent = AcpAgent::new(config).with_debug({
            let record = record.clone();
            move |line, direction| record.line(line, direction)
        });
        let on_update = record.clone();
        let on_permission = record;
        let reports_cost = self.profile.reports_cost;
        Client
            .builder()
            .name("epik")
            .on_receive_notification(
                async move |notification: SessionNotification, _connection| {
                    if let SessionUpdate::UsageUpdate(usage) = notification.update
                        && let Some(cost) = usage.cost
                        && cost.currency == "USD"
                        && reports_cost
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
            .connect_with(agent, main)
            .await
    }

    async fn converse(
        &self,
        connection: ConnectionTo<agent_client_protocol::Agent>,
        request: &AgentRequest,
        record: &Record,
    ) -> Result<Ended, agent_client_protocol::Error> {
        let session = open(&connection, &request.cwd).await?;
        let setup = match setup(&self.profile, &session, request.model.as_deref()) {
            Ok(setup) => setup,
            Err(refusal) => return Ok(Ended::Refused(refusal)),
        };
        if setup.switch_mode {
            connection
                .send_request(SetSessionModeRequest::new(
                    session.session_id.clone(),
                    self.profile.unattended_mode.clone(),
                ))
                .block_task()
                .await?;
        }
        if let Some((option, model)) = setup.model {
            connection
                .send_request(SetSessionConfigOptionRequest::new(
                    session.session_id.clone(),
                    option,
                    model.as_str(),
                ))
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

        let quiet_period = self.profile.quiet_period;
        loop {
            let quiet_for = record.last_heard.lock().unwrap().elapsed();
            if quiet_for >= quiet_period {
                break;
            }
            async_io::Timer::after(quiet_period - quiet_for).await;
        }

        let stop_reason = serde_json::to_value(response.stop_reason)
            .ok()
            .and_then(|value| value.as_str().map(String::from))
            .unwrap_or_default();
        Ok(Ended::Stopped(stop_reason, None))
    }
}

impl Agent for AcpClient {
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
                let session = self.connect(record.clone(), async |connection| {
                    self.converse(connection, request, &record).await
                });
                // Dropping an unfinished session stops the agent's process
                // group.
                let ended = futures::executor::block_on(async {
                    match select(pin!(session), pin!(cancelled)).await {
                        Either::Left((Ok(ended), _)) => ended,
                        Either::Left((Err(error), _)) => match codex_error(&error) {
                            Some((info, message)) => Ended::Stopped(
                                info,
                                Some(format!("{} stopped: {message}", self.profile.name)),
                            ),
                            None => Ended::Failed(error.to_string()),
                        },
                        Either::Right(_) => Ended::Cancelled,
                    }
                });
                (ended, *record.cost_usd.lock().unwrap())
            }
        };
        let (stop_reason, error) = match ended {
            Ended::Stopped(reason, error) => (reason, error),
            Ended::Refused(refusal @ Refusal::NoUnattendedMode(_)) => {
                ("no_auto_mode".into(), Some(refusal.to_string()))
            }
            Ended::Refused(refusal @ Refusal::NoModel(_)) => {
                ("no_model".into(), Some(refusal.to_string()))
            }
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

    fn check(&self, cwd: &Path, model: Option<&str>) -> Result<(), String> {
        // Nothing is kept: the session does no work.
        let record = Arc::new(Record::discarding());
        let checked = futures::executor::block_on(self.connect(record, async |connection| {
            let session = open(&connection, cwd).await?;
            Ok(setup(&self.profile, &session, model).map(|_| ()))
        }));
        match checked {
            Ok(result) => result.map_err(|refusal| refusal.to_string()),
            Err(error) => Err(format!(
                "{} couldn't open a session: {error}",
                self.profile.name
            )),
        }
    }
}

/// Initializes the connection and opens a session in `cwd`.
async fn open(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    cwd: &Path,
) -> Result<NewSessionResponse, agent_client_protocol::Error> {
    // No file-system or terminal capabilities: the agent uses its own.
    connection
        .send_request(InitializeRequest::new(ProtocolVersion::V1))
        .block_task()
        .await?;
    connection
        .send_request(NewSessionRequest::new(cwd))
        .block_task()
        .await
}

/// What a new session needs before it can work.
#[derive(Debug, PartialEq)]
struct Setup {
    /// It isn't in the unattended mode yet.
    switch_mode: bool,
    /// The config option that chooses the model, and the model to choose.
    model: Option<(SessionConfigId, String)>,
}

/// Why a session can't do the work.
#[derive(Debug, PartialEq)]
enum Refusal {
    NoUnattendedMode(String),
    NoModel(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoUnattendedMode(why) | Self::NoModel(why) => f.write_str(why),
        }
    }
}

/// What `session` needs to run unattended with `model`, or why it can't:
/// `profile`'s unattended mode must be among its modes, and `model`, if
/// given, among the values of its model option. That option is the one
/// whose category is `model`, or else whose id is.
fn setup(
    profile: &AgentProfile,
    session: &NewSessionResponse,
    model: Option<&str>,
) -> Result<Setup, Refusal> {
    let unattended = profile.unattended_mode.as_str();
    let modes = session.modes.as_ref();
    let switch_mode = modes.is_none_or(|modes| &*modes.current_mode_id.0 != unattended);
    let available: Vec<&str> = modes
        .map(|modes| modes.available_modes.iter().map(|m| &*m.id.0).collect())
        .unwrap_or_default();
    if switch_mode && !available.contains(&unattended) {
        return Err(Refusal::NoUnattendedMode(format!(
            "{} can't run unattended: its session has no `{unattended}` mode; its modes \
             are: {}",
            profile.name,
            available.join(", ")
        )));
    }

    let Some(model) = model else {
        return Ok(Setup {
            switch_mode,
            model: None,
        });
    };
    let options = session.config_options.as_deref().unwrap_or_default();
    let option = options
        .iter()
        .find(|option| option.category == Some(SessionConfigOptionCategory::Model))
        .or_else(|| options.iter().find(|option| &*option.id.0 == "model"));
    let Some(option) = option else {
        return Err(Refusal::NoModel(format!(
            "{} can't run the model `{model}`: it offers no choice of model",
            profile.name
        )));
    };
    let offered: Vec<&str> = match &option.kind {
        SessionConfigKind::Select(select) => match &select.options {
            SessionConfigSelectOptions::Ungrouped(values) => {
                values.iter().map(|value| &*value.value.0).collect()
            }
            SessionConfigSelectOptions::Grouped(groups) => groups
                .iter()
                .flat_map(|group| &group.options)
                .map(|value| &*value.value.0)
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    if !offered.contains(&model) {
        return Err(Refusal::NoModel(format!(
            "{} doesn't offer the model `{model}`; its models are: {}",
            profile.name,
            offered.join(", ")
        )));
    }
    Ok(Setup {
        switch_mode,
        model: Some((option.id.clone(), model.to_owned())),
    })
}

/// How a run ended. Only `Stopped` with no error is clean.
enum Ended {
    /// The prompt returned with this ACP stop reason and the agent went
    /// quiet, or failed for this reason of the agent's own, and why.
    Stopped(String, Option<String>),
    Refused(Refusal),
    Failed(String),
    Cancelled,
}

/// The reason Codex gives for failing a prompt, its `codexErrorInfo`, and
/// its message, if `error` is such a failure. Codex sends it as an
/// "Internal error" whose data has both. The reason is a name, such as
/// `usageLimitExceeded`, or an object keyed by one.
fn codex_error(error: &agent_client_protocol::Error) -> Option<(String, String)> {
    let data = error.data.as_ref()?;
    let info = match data.get("codexErrorInfo")? {
        Value::String(name) => name.clone(),
        Value::Object(object) => object.keys().next()?.clone(),
        _ => return None,
    };
    let message = data
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(&error.message)
        .to_owned();
    Some((info, message))
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
    jsonl: Mutex<Box<dyn Write + Send>>,
    err: Mutex<Box<dyn Write + Send>>,
    last_heard: Mutex<Instant>,
    cost_usd: Mutex<Option<f64>>,
}

impl Record {
    fn create(request: &AgentRequest) -> std::io::Result<Self> {
        Ok(Self {
            jsonl: Mutex::new(Box::new(File::create(&request.jsonl_path)?)),
            err: Mutex::new(Box::new(File::create(&request.err_path)?)),
            last_heard: Mutex::new(Instant::now()),
            cost_usd: Mutex::new(None),
        })
    }

    /// A record that keeps nothing.
    fn discarding() -> Self {
        Self {
            jsonl: Mutex::new(Box::new(std::io::sink())),
            err: Mutex::new(Box::new(std::io::sink())),
            last_heard: Mutex::new(Instant::now()),
            cost_usd: Mutex::new(None),
        }
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

    /// A session in auto mode with `options`.
    fn session(options: Value) -> NewSessionResponse {
        serde_json::from_value(json!({
            "sessionId": "s",
            "modes": {"currentModeId": "auto", "availableModes": [{"id": "auto", "name": "Auto"}]},
            "configOptions": options,
        }))
        .unwrap()
    }

    fn select(id: &str, category: Option<&str>, values: &[&str]) -> Value {
        let values: Vec<Value> = values
            .iter()
            .map(|value| json!({"value": value, "name": value}))
            .collect();
        json!({
            "id": id,
            "name": id,
            "category": category,
            "type": "select",
            "currentValue": values[0]["value"],
            "options": values,
        })
    }

    fn chosen(session: &NewSessionResponse, model: &str) -> Result<(String, String), Refusal> {
        setup(&AgentProfile::claude(), session, Some(model)).map(|setup| {
            setup
                .model
                .map(|(id, model)| (id.0.to_string(), model))
                .unwrap()
        })
    }

    #[test]
    fn the_model_option_is_the_one_in_the_model_category() {
        let session = session(json!([
            select("model", None, &["haiku"]),
            select("llm", Some("model"), &["sonnet", "opus"]),
        ]));

        assert_eq!(chosen(&session, "opus"), Ok(("llm".into(), "opus".into())));
        assert!(chosen(&session, "haiku").is_err());
    }

    #[test]
    fn without_a_model_category_the_model_option_is_the_one_named_model() {
        let session = session(json!([
            select("effort", Some("thought_level"), &["high"]),
            select("model", None, &["sonnet"]),
        ]));

        assert_eq!(
            chosen(&session, "sonnet"),
            Ok(("model".into(), "sonnet".into()))
        );
    }

    #[test]
    fn grouped_models_are_all_offered() {
        let mut option = select("model", Some("model"), &["sonnet"]);
        option["options"] = json!([
            {"group": "a", "name": "A", "options": [{"value": "sonnet", "name": "Sonnet"}]},
            {"group": "b", "name": "B", "options": [{"value": "opus", "name": "Opus"}]},
        ]);
        let session = session(json!([option]));

        assert_eq!(
            chosen(&session, "opus"),
            Ok(("model".into(), "opus".into()))
        );
        let Err(Refusal::NoModel(why)) = chosen(&session, "haiku") else {
            panic!("haiku isn't offered");
        };
        assert!(why.ends_with("sonnet, opus"), "{why}");
    }

    #[test]
    fn a_session_already_in_the_unattended_mode_is_not_switched() {
        let setup = setup(&AgentProfile::claude(), &session(json!([])), None).unwrap();

        assert_eq!(
            setup,
            Setup {
                switch_mode: false,
                model: None
            }
        );
    }

    #[test]
    fn every_line_written_says_when_it_arrived() {
        let dir = tempfile::tempdir().unwrap();
        let request = AgentRequest {
            prompt: String::new(),
            issue: 1,
            branch: "main".into(),
            cwd: dir.path().to_owned(),
            jsonl_path: dir.path().join("1.jsonl"),
            err_path: dir.path().join("1.err"),
            model: None,
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
