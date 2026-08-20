//! The chat vocabulary and the client that speaks it.
//!
//! [`TranscriptItem`] is the unit that crosses the IPC barrier and gets
//! folded into a window's view of the conversation. Its serde form is
//! tagged, and that tag is the forward-compatibility contract: new kinds
//! of item become new variants on the same event channel and new arms in
//! the same fold, never a schema break.
//!
//! [`Client`] holds a conversation with an OpenAI-compatible
//! chat-completions endpoint — transcript in, streamed deltas out. It is
//! provider-agnostic data: a base URL, a model, a key. Provider
//! specificity lives only in constructors, of which there is exactly one
//! today, [`Client::anthropic`]. The library is synchronous; running a
//! turn somewhere it won't block anything is the host's problem.

#[cfg(feature = "scripted")]
pub mod scripted;

#[cfg(feature = "native")]
use std::sync::atomic::AtomicBool;

use crate::keystore::Secret;

/// A speaker, by its wire-protocol name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "lowercase")
)]
pub enum Role {
    User,
    Assistant,
}

/// One entry on the transcript channel.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "kind", rename_all = "snake_case")
)]
pub enum TranscriptItem {
    /// A complete utterance, said by `role`. The only variant a
    /// [`Conversation`] holds.
    Message { role: Role, text: String },
    /// A streaming fragment of the assistant's reply in progress.
    /// Ephemeral: it exists on the event channel and nowhere else.
    AssistantDelta { text: String },
    /// A turn that came to grief instead of a reply. Not conversation
    /// content — the failure is news, not something anybody said.
    TurnFailed { reason: String },
    /// The model reached for a tool: an observed act, not an utterance.
    /// `arguments` is pretty-printed JSON, for reading.
    ToolCall { name: String, arguments: String },
    /// What the tool said back. `content` is capped for the transcript —
    /// the model gets the whole thing on the wire; the transcript gets
    /// enough to see what happened.
    ToolResult {
        name: String,
        ok: bool,
        content: String,
    },
    /// The persona is asking the user something and the turn is waiting
    /// on the answer. Ephemeral like a delta: it exists on the event
    /// channel and is never pushed into a [`Conversation`] — the settled
    /// record is [`QuestionResolved`](Self::QuestionResolved).
    Question { id: String, ask: Ask },
    /// A question and what the user answered — the settled record,
    /// appended and emitted exactly like a tool call and its result.
    QuestionResolved {
        id: String,
        ask: Ask,
        answer: Answer,
    },
}

/// What the persona needs from the user, in modality-free terms: the
/// backend says *what* it wants, never how to collect it — every input
/// modality is the frontend's. Tagged like [`TranscriptItem`], so a new
/// kind of question is a new variant, never a schema break.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "kind", rename_all = "snake_case")
)]
pub enum Ask {
    /// Where a repository should live. `prompt` is the persona's own
    /// wording of the question.
    Repository { prompt: String },
    /// The check command a feature build will be judged by. `prompt` is
    /// the build machinery's wording — the card is raised as the build's
    /// first act, never by the persona — and `proposal` is what
    /// detection prefilled, absent when no marker matched.
    Check {
        prompt: String,
        proposal: Option<String>,
    },
}

/// What the user answered. A decline is a first-class answer — the
/// persona reads it and reacts — not an error.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "kind", rename_all = "snake_case")
)]
pub enum Answer {
    /// A repository location: a git URL, of which a path is one.
    Repository { url: String },
    /// The check command in force: the user confirmed or edited it.
    Check { command: String },
    /// The user would rather not say. For a check, the decline is the
    /// skip: the build runs on observation alone.
    Declined,
}

/// The event channel transcript items arrive on, backend to window.
pub const TRANSCRIPT_EVENT: &str = "transcript";

#[cfg(feature = "native")]
static PROMPT: include_crypt::EncryptedFile = include_crypt::include_crypt!("prompt.md");

/// Epik's voice: the system prompt that leads every turn, whoever the
/// host is — the desktop app today, a headless daemon tomorrow. Written
/// in prompt.md at this crate's root, compiled in encrypted so the binary
/// doesn't hand it to strings(1) — obfuscation, not secrecy — and never
/// part of any transcript. It rides with `native` because every host
/// that speaks is one; the obfuscation machinery doesn't build for wasm.
#[cfg(feature = "native")]
pub static SYSTEM_PROMPT: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| PROMPT.decrypt_str().expect("the prompt decrypts to utf-8"));

/// The canonical transcript: what was actually said, in order. One holder
/// per conversation — in the app, the backend. Completed [`Message`]s
/// only; deltas and failures never land here.
///
/// [`Message`]: TranscriptItem::Message
#[derive(Debug, Default)]
pub struct Conversation(Vec<TranscriptItem>);

impl Conversation {
    /// Appends `item`. The transcript only ever grows.
    pub fn push(&mut self, item: TranscriptItem) {
        self.0.push(item);
    }

    /// The transcript, oldest first.
    pub fn items(&self) -> std::slice::Iter<'_, TranscriptItem> {
        self.0.iter()
    }
}

/// One completed tool call, as the model asked for it: the id the result
/// must answer to, the tool's name, and the arguments exactly as the
/// model wrote them — a raw JSON string, parsed only at dispatch.
#[cfg(feature = "serde")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// What a reply turned out to be: the model either spoke or reached for
/// tools, never both.
#[cfg(feature = "serde")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reply {
    /// The final text, assembled from the deltas.
    Text(String),
    /// The calls the model wants made, in index order.
    ToolCalls(Vec<ToolCall>),
}

/// One tool as the wire advertises it: the OpenAI function-tool shape.
#[cfg(feature = "serde")]
#[derive(Clone, Debug, serde::Serialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    kind: &'static str,
    function: ToolFunction,
}

#[cfg(feature = "serde")]
#[derive(Clone, Debug, serde::Serialize)]
struct ToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[cfg(feature = "serde")]
impl ToolSpec {
    /// A function tool: `parameters` is its arguments' JSON Schema.
    #[must_use]
    pub fn function(name: String, description: String, parameters: serde_json::Value) -> Self {
        Self {
            kind: "function",
            function: ToolFunction {
                name,
                description,
                parameters,
            },
        }
    }
}

/// One message as the request will carry it. The transcript's completed
/// messages map to [`Text`]; the other two forms exist only inside a tool
/// turn, where the loop appends what it did so the model can go on.
///
/// [`Text`]: ChatMessage::Text
#[cfg(feature = "serde")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChatMessage {
    /// An utterance, from the transcript.
    Text { role: Role, text: String },
    /// The assistant's turn that was tool calls instead of words.
    ToolCalls(Vec<ToolCall>),
    /// A tool's answer, addressed to the call by its id. `content` is the
    /// whole answer — the wire never truncates.
    ToolResult { id: String, content: String },
}

#[cfg(feature = "serde")]
impl ChatMessage {
    /// The transcript's completed messages, in order, as wire messages.
    /// Deltas and failures are not conversation; tool items are the
    /// transcript's record of past turns, already summarized by the
    /// assistant text that followed them, and carry no call ids — so
    /// none of them go to the wire.
    #[must_use]
    pub fn from_transcript(transcript: &[TranscriptItem]) -> Vec<Self> {
        transcript
            .iter()
            .filter_map(|item| match item {
                TranscriptItem::Message { role, text } => Some(Self::Text {
                    role: *role,
                    text: text.clone(),
                }),
                _ => None,
            })
            .collect()
    }
}

/// How much of a tool's answer the transcript keeps. The model always
/// gets all of it; this cap is only for what a window shows and stores.
pub const TOOL_RESULT_CAP: usize = 2000;

/// The first `cap` characters of `text`, with a note about the rest —
/// or `text` itself when it already fits.
#[cfg(feature = "serde")]
fn elide(text: &str, cap: usize) -> String {
    let total = text.chars().count();
    if total <= cap {
        return text.to_owned();
    }
    let mut kept: String = text.chars().take(cap).collect();
    kept.push_str(&format!("\n… ({} more characters elided)", total - cap));
    kept
}

#[cfg(feature = "serde")]
impl TranscriptItem {
    /// The transcript's record of `call`: its arguments pretty-printed
    /// when they parse, verbatim when they don't.
    #[must_use]
    pub fn tool_call(call: &ToolCall) -> Self {
        let arguments = serde_json::from_str::<serde_json::Value>(&call.arguments)
            .and_then(|value| serde_json::to_string_pretty(&value))
            .unwrap_or_else(|_| call.arguments.clone());
        Self::ToolCall {
            name: call.name.clone(),
            arguments,
        }
    }

    /// The transcript's record of what `name` answered, capped at
    /// [`TOOL_RESULT_CAP`] characters.
    #[must_use]
    pub fn tool_result(name: &str, result: &Result<serde_json::Value, String>) -> Self {
        let (ok, content) = match result {
            Ok(value) => (
                true,
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
            ),
            Err(reason) => (false, reason.clone()),
        };
        Self::ToolResult {
            name: name.to_owned(),
            ok,
            content: elide(&content, TOOL_RESULT_CAP),
        }
    }
}

/// What a turn can die of. The key never appears in any of it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChatError {
    /// There is no key to present.
    NoKey,
    /// The wire failed beneath the protocol: no route, no socket, no
    /// answer.
    Transport(String),
    /// The API answered with an error of its own, quoted verbatim —
    /// its wording reaches the user intact.
    Api(String),
    /// The answer stopped speaking the protocol.
    MalformedStream(String),
    /// The turn kept asking for tools past `limit` rounds without ever
    /// arriving at an answer.
    ToolLoop { limit: usize },
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoKey => write!(f, "no API key is set"),
            Self::Transport(reason) => write!(f, "could not reach the model: {reason}"),
            Self::Api(message) => write!(f, "{message}"),
            Self::MalformedStream(detail) => {
                write!(f, "the reply stopped speaking the protocol: {detail}")
            }
            Self::ToolLoop { limit } => write!(
                f,
                "the turn hit the cap of {limit} rounds of tool calls without an answer"
            ),
        }
    }
}

impl std::error::Error for ChatError {}

/// The pinned Anthropic model [`Client::anthropic`] speaks to.
pub const ANTHROPIC_MODEL: &str = "claude-sonnet-4-5";

/// A client for one OpenAI-compatible chat-completions endpoint: where to
/// ask, what to ask for, and what to present at the door.
#[derive(Debug)]
// The fields are read by `reply`, which needs `native`; the data itself
// is welcome everywhere.
#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub struct Client {
    base_url: String,
    model: String,
    key: Option<Secret>,
}

impl Client {
    /// A client for `model` behind `base_url` — the prefix ending in
    /// `/v1` — presenting `key` when there is one. A local server that
    /// wants no key gets no header.
    #[must_use]
    pub fn new(base_url: String, model: String, key: Option<Secret>) -> Self {
        Self {
            base_url,
            model,
            key,
        }
    }

    /// The one provider constructor so far: Anthropic's OpenAI-compatible
    /// endpoint, speaking to `model` — or to [`ANTHROPIC_MODEL`], the
    /// pinned default, when none is stated.
    #[must_use]
    pub fn anthropic(key: Secret, model: Option<String>) -> Self {
        Self::new(
            "https://api.anthropic.com/v1".to_owned(),
            model.unwrap_or_else(|| ANTHROPIC_MODEL.to_owned()),
            Some(key),
        )
    }
}

/// One model a provider will answer for: the id the configuration
/// states and the name a dropdown shows.
///
/// Unknown fields are absorbed, in the manner of the other wire types:
/// the Models endpoint says more about a model than a dropdown needs.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
}

/// Anthropic's native API, where the Models endpoint lives. Distinct from
/// the OpenAI-compatible `/v1` the chat client speaks to: each is used
/// where it is documented.
#[cfg(feature = "native")]
const ANTHROPIC_API: &str = "https://api.anthropic.com/v1";

/// The native API's version header, sent on every request to it.
#[cfg(feature = "native")]
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The models Anthropic will answer for under `key`, newest first — the
/// order the API returns and the order a dropdown shows.
///
/// A discovery affordance and never load-bearing: a caller that cannot
/// get the list keeps whatever model it already had in mind.
///
/// # Errors
///
/// [`ChatError`], distinguishing transport failures, the API's own
/// errors (verbatim), and an answer that was not a model list.
#[cfg(feature = "native")]
pub fn models(key: &Secret) -> Result<Vec<ModelInfo>, ChatError> {
    models_at(ANTHROPIC_API, key)
}

/// [`models`], against a stated base — the prefix ending in `/v1` —
/// which is how a test aims it at a loopback port.
#[cfg(feature = "native")]
fn models_at(base_url: &str, key: &Secret) -> Result<Vec<ModelInfo>, ChatError> {
    #[derive(serde::Deserialize)]
    struct Page {
        data: Vec<ModelInfo>,
    }
    let url = format!("{}/models?limit=1000", base_url.trim_end_matches('/'));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut response = agent
        .get(&url)
        .header("x-api-key", key.reveal())
        .header("anthropic-version", ANTHROPIC_VERSION)
        .call()
        .map_err(|error| ChatError::Transport(error.to_string()))?;
    let status = response.status();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| ChatError::Transport(error.to_string()))?;
    if !status.is_success() {
        return Err(ChatError::Api(wire::api_message(
            &status.to_string(),
            &body,
        )));
    }
    let page: Page = serde_json::from_str(&body)
        .map_err(|error| ChatError::MalformedStream(format!("not a model list: {error}")))?;
    Ok(page.data)
}

/// Assembles server-sent-event lines into event data payloads.
///
/// Fed one line at a time: `data:` lines accumulate, a blank line
/// completes the event and hands back its payload (multiple data lines
/// joined with newlines, per the SSE contract), and every other field —
/// event names, ids, comments — is ignored, which is all the protocol
/// this stream speaks.
// Driven by `reply` under `native`; compiled — and tested — everywhere.
#[cfg_attr(not(feature = "native"), allow(dead_code))]
#[derive(Debug, Default)]
struct Events {
    data: Vec<String>,
}

#[cfg_attr(not(feature = "native"), allow(dead_code))]
impl Events {
    fn line(&mut self, line: &str) -> Option<String> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.flush();
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.data
                .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
        }
        None
    }

    /// The event in progress, for a stream that ended without its blank
    /// line.
    fn flush(&mut self) -> Option<String> {
        if self.data.is_empty() {
            None
        } else {
            let payload = self.data.join("\n");
            self.data.clear();
            Some(payload)
        }
    }
}

/// What one SSE data payload means to the stream.
#[cfg(feature = "serde")]
// Driven by `reply` under `native`; compiled — and tested — everywhere
// serde is.
#[cfg_attr(not(feature = "native"), allow(dead_code))]
#[derive(Debug, Eq, PartialEq)]
enum Step {
    /// A fragment of the reply.
    Delta(String),
    /// Fragments of tool calls, to be accumulated by index.
    ToolCalls(Vec<wire::ToolCallFragment>),
    /// The model finished by asking for tools instead of speaking.
    ToolsFinished,
    /// The end of the stream.
    Done,
    /// Protocol housekeeping with nothing to say — a role announcement,
    /// an empty choice.
    Skip,
}

/// The wire shapes of the OpenAI-compatible chat-completions protocol,
/// and the pure mappings in and out of them.
#[cfg(feature = "serde")]
// Driven by `reply` under `native`; compiled — and tested — everywhere
// serde is.
#[cfg_attr(not(feature = "native"), allow(dead_code))]
mod wire {
    use super::{ChatError, ChatMessage, Role, Step, ToolCall, ToolSpec};

    #[derive(serde::Serialize)]
    pub(super) struct Request<'a> {
        model: &'a str,
        messages: Vec<Message<'a>>,
        stream: bool,
        // Serialized only when present, so a plain chat request's JSON
        // stays byte-identical to the shape before tools existed.
        #[serde(skip_serializing_if = "<[_]>::is_empty")]
        tools: &'a [ToolSpec],
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_choice: Option<&'a str>,
    }

    #[derive(serde::Serialize)]
    struct Message<'a> {
        role: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<MessageToolCall<'a>>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<&'a str>,
    }

    impl<'a> Message<'a> {
        fn text(role: &'a str, content: &'a str) -> Self {
            Self {
                role,
                content: Some(content),
                tool_calls: None,
                tool_call_id: None,
            }
        }
    }

    /// A completed call inside an assistant message, echoed back to the
    /// model exactly as it asked for it.
    #[derive(serde::Serialize)]
    struct MessageToolCall<'a> {
        id: &'a str,
        #[serde(rename = "type")]
        kind: &'static str,
        function: MessageFunction<'a>,
    }

    #[derive(serde::Serialize)]
    struct MessageFunction<'a> {
        name: &'a str,
        arguments: &'a str,
    }

    const fn role_name(role: Role) -> &'static str {
        match role {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }

    /// The request body: the system message first, then `messages` in
    /// order — utterances, the assistant's tool-call turns, and tools'
    /// answers addressed by call id.
    pub(super) fn request<'a>(
        model: &'a str,
        system: &'a str,
        messages: &'a [ChatMessage],
        tools: &'a [ToolSpec],
        tool_choice: Option<&'a str>,
    ) -> Request<'a> {
        let mut wire = vec![Message::text("system", system)];
        wire.extend(messages.iter().map(|message| {
            match message {
                ChatMessage::Text { role, text } => Message::text(role_name(*role), text),
                ChatMessage::ToolCalls(calls) => Message {
                    role: "assistant",
                    content: None,
                    tool_calls: Some(
                        calls
                            .iter()
                            .map(|call| MessageToolCall {
                                id: &call.id,
                                kind: "function",
                                function: MessageFunction {
                                    name: &call.name,
                                    arguments: &call.arguments,
                                },
                            })
                            .collect(),
                    ),
                    tool_call_id: None,
                },
                ChatMessage::ToolResult { id, content } => Message {
                    role: "tool",
                    content: Some(content),
                    tool_calls: None,
                    tool_call_id: Some(id),
                },
            }
        }));
        Request {
            model,
            messages: wire,
            stream: true,
            tools,
            tool_choice,
        }
    }

    #[derive(serde::Deserialize)]
    struct Chunk {
        choices: Vec<Choice>,
    }

    #[derive(serde::Deserialize)]
    struct Choice {
        delta: Delta,
        #[serde(default)]
        finish_reason: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct Delta {
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        tool_calls: Option<Vec<ToolCallFragment>>,
    }

    /// One delta's worth of one tool call: the id and name arrive once
    /// per index, the arguments in as many pieces as the stream likes.
    #[derive(Debug, Eq, PartialEq, serde::Deserialize)]
    pub(super) struct ToolCallFragment {
        index: usize,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        function: Option<FunctionFragment>,
    }

    #[derive(Debug, Eq, PartialEq, serde::Deserialize)]
    struct FunctionFragment {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        arguments: Option<String>,
    }

    /// Assembles [`ToolCallFragment`]s into completed calls, keyed by
    /// index. Yields them in index order when the stream finishes.
    #[derive(Debug, Default)]
    pub(super) struct Calls(std::collections::BTreeMap<usize, ToolCall>);

    impl Calls {
        pub(super) fn absorb(&mut self, fragments: Vec<ToolCallFragment>) {
            for fragment in fragments {
                let call = self.0.entry(fragment.index).or_insert_with(|| ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
                if let Some(id) = fragment.id {
                    call.id = id;
                }
                if let Some(function) = fragment.function {
                    if let Some(name) = function.name {
                        call.name = name;
                    }
                    if let Some(arguments) = function.arguments {
                        call.arguments.push_str(&arguments);
                    }
                }
            }
        }

        pub(super) fn is_empty(&self) -> bool {
            self.0.is_empty()
        }

        pub(super) fn finish(self) -> Vec<ToolCall> {
            self.0.into_values().collect()
        }
    }

    /// Reads one data payload of the stream.
    pub(super) fn step(payload: &str) -> Result<Step, ChatError> {
        if payload.trim() == "[DONE]" {
            return Ok(Step::Done);
        }
        let chunk: Chunk = serde_json::from_str(payload)
            .map_err(|_| ChatError::MalformedStream(payload.to_owned()))?;
        let Some(choice) = chunk.choices.into_iter().next() else {
            return Ok(Step::Skip);
        };
        if let Some(fragments) = choice.delta.tool_calls {
            return Ok(Step::ToolCalls(fragments));
        }
        if let Some(content) = choice.delta.content {
            return Ok(Step::Delta(content));
        }
        if choice.finish_reason.as_deref() == Some("tool_calls") {
            return Ok(Step::ToolsFinished);
        }
        Ok(Step::Skip)
    }

    #[derive(serde::Deserialize)]
    struct ApiError {
        error: ApiErrorBody,
    }

    #[derive(serde::Deserialize)]
    struct ApiErrorBody {
        message: String,
    }

    /// The API's own words for what went wrong, verbatim when the body
    /// carries them, with the raw body and then the bare status as
    /// fallbacks.
    pub(super) fn api_message(status: &str, body: &str) -> String {
        if let Ok(error) = serde_json::from_str::<ApiError>(body) {
            return error.error.message;
        }
        if body.trim().is_empty() {
            status.to_owned()
        } else {
            body.to_owned()
        }
    }
}

#[cfg(feature = "native")]
impl Client {
    /// The model's reply to `messages`, with `system` said first and kept
    /// out of them, and `tools` on offer when there are any.
    ///
    /// Deltas reach `on_delta` as the model produces them; the return is
    /// what the reply turned out to be — the whole text, or the tool
    /// calls the model wants made. The caller owns history — nothing here
    /// remembers anything. `stop` is checked between deltas; once it is
    /// set, the text so far comes back and the rest of the stream is left
    /// where it is.
    ///
    /// # Errors
    ///
    /// [`ChatError`], distinguishing transport failures, the API's own
    /// errors (verbatim), and a stream that stopped making sense.
    pub fn reply(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        mut on_delta: impl FnMut(&str),
        stop: &AtomicBool,
    ) -> Result<Reply, ChatError> {
        use std::io::{BufRead, BufReader};
        use std::sync::atomic::Ordering;

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body =
            serde_json::to_string(&wire::request(&self.model, system, messages, tools, None))
                .expect("the request body serializes");

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .new_agent();
        let mut request = agent.post(&url).header("content-type", "application/json");
        if let Some(key) = &self.key {
            request = request.header("authorization", &format!("Bearer {}", key.reveal()));
        }
        let mut response = request
            .send(&body)
            .map_err(|error| ChatError::Transport(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.body_mut().read_to_string().unwrap_or_default();
            return Err(ChatError::Api(wire::api_message(
                &status.to_string(),
                &body,
            )));
        }

        let mut events = Events::default();
        let mut reply = String::new();
        let mut calls = wire::Calls::default();
        let mut tools_finished = false;
        let mut step = |payload: &str,
                        reply: &mut String,
                        calls: &mut wire::Calls|
         -> Result<bool, ChatError> {
            match wire::step(payload)? {
                Step::Delta(delta) => {
                    on_delta(&delta);
                    reply.push_str(&delta);
                }
                Step::ToolCalls(fragments) => calls.absorb(fragments),
                Step::ToolsFinished => tools_finished = true,
                Step::Done => return Ok(true),
                Step::Skip => {}
            }
            Ok(false)
        };
        let reader = BufReader::new(response.body_mut().as_reader());
        let mut done = false;
        for line in reader.lines() {
            if stop.load(Ordering::Relaxed) {
                return Ok(Reply::Text(reply));
            }
            let line = line.map_err(|error| ChatError::Transport(error.to_string()))?;
            let Some(payload) = events.line(&line) else {
                continue;
            };
            if step(&payload, &mut reply, &mut calls)? {
                done = true;
                break;
            }
        }
        if !done && let Some(payload) = events.flush() {
            step(&payload, &mut reply, &mut calls)?;
        }
        if tools_finished || !calls.is_empty() {
            Ok(Reply::ToolCalls(calls.finish()))
        } else {
            Ok(Reply::Text(reply))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_resolves_an_absent_model_to_the_pinned_default() {
        assert_eq!(Client::anthropic("k".into(), None).model, ANTHROPIC_MODEL);
        assert_eq!(
            Client::anthropic("k".into(), Some("other".to_owned())).model,
            "other"
        );
    }

    fn message(role: Role, text: &str) -> TranscriptItem {
        TranscriptItem::Message {
            role,
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_conversation_keeps_items_in_the_order_they_were_said() {
        let mut conversation = Conversation::default();
        conversation.push(message(Role::User, "first"));
        conversation.push(message(Role::Assistant, "second"));
        conversation.push(message(Role::User, "third"));

        let texts: Vec<_> = conversation
            .items()
            .map(|item| match item {
                TranscriptItem::Message { text, .. } => text.as_str(),
                _ => panic!("only messages were pushed"),
            })
            .collect();
        assert_eq!(texts, ["first", "second", "third"]);
    }

    #[test]
    fn a_fresh_conversation_has_nothing_to_say() {
        assert_eq!(Conversation::default().items().count(), 0);
    }

    /// The obfuscation round-trips: what ships encrypted decrypts to
    /// exactly the file Bill writes.
    #[cfg(feature = "native")]
    #[test]
    fn the_system_prompt_decrypts_to_the_prompt_file() {
        assert_eq!(*SYSTEM_PROMPT, include_str!("../prompt.md"));
    }

    #[test]
    fn sse_events_assemble_line_by_line() {
        let mut events = Events::default();
        assert_eq!(events.line("data: one"), None);
        assert_eq!(events.line(""), Some("one".to_owned()));
        assert_eq!(events.line("data: two"), None);
        assert_eq!(events.line(""), Some("two".to_owned()));
    }

    #[test]
    fn multi_line_data_joins_with_newlines() {
        let mut events = Events::default();
        events.line("data: first");
        events.line("data: second");
        assert_eq!(events.line(""), Some("first\nsecond".to_owned()));
    }

    #[test]
    fn other_sse_fields_comments_and_crlf_are_taken_in_stride() {
        let mut events = Events::default();
        events.line("event: message\r");
        events.line(": a comment");
        events.line("id: 7");
        events.line("data: payload\r");
        assert_eq!(events.line("\r"), Some("payload".to_owned()));
    }

    #[test]
    fn a_blank_line_with_no_data_is_no_event() {
        let mut events = Events::default();
        assert_eq!(events.line(""), None);
    }

    #[test]
    fn an_unterminated_final_event_still_flushes() {
        let mut events = Events::default();
        events.line("data: tail");
        assert_eq!(events.flush(), Some("tail".to_owned()));
        assert_eq!(events.flush(), None);
    }

    #[cfg(feature = "serde")]
    mod wire_shape {
        use super::*;

        /// The tag in the JSON is the forward-compatibility contract: a
        /// future item kind is a new tag value, and an old reader can see
        /// what it is not, rather than misread what it is.
        #[test]
        fn every_transcript_item_crosses_the_wire_tagged() {
            for (item, tag) in [
                (message(Role::User, "hello"), r#""kind":"message""#),
                (
                    TranscriptItem::AssistantDelta {
                        text: "he".to_owned(),
                    },
                    r#""kind":"assistant_delta""#,
                ),
                (
                    TranscriptItem::TurnFailed {
                        reason: "no route".to_owned(),
                    },
                    r#""kind":"turn_failed""#,
                ),
                (
                    TranscriptItem::ToolCall {
                        name: "current_time".to_owned(),
                        arguments: "{}".to_owned(),
                    },
                    r#""kind":"tool_call""#,
                ),
                (
                    TranscriptItem::ToolResult {
                        name: "current_time".to_owned(),
                        ok: true,
                        content: "{\"local\":\"…\"}".to_owned(),
                    },
                    r#""kind":"tool_result""#,
                ),
                (
                    TranscriptItem::Question {
                        id: "1".to_owned(),
                        ask: Ask::Repository {
                            prompt: "Where should this live?".to_owned(),
                        },
                    },
                    r#""kind":"question""#,
                ),
                (
                    TranscriptItem::QuestionResolved {
                        id: "1".to_owned(),
                        ask: Ask::Repository {
                            prompt: "Where should this live?".to_owned(),
                        },
                        answer: Answer::Repository {
                            url: "/tmp/wumpus.git".to_owned(),
                        },
                    },
                    r#""kind":"question_resolved""#,
                ),
                (
                    TranscriptItem::QuestionResolved {
                        id: "2".to_owned(),
                        ask: Ask::Repository {
                            prompt: "Where?".to_owned(),
                        },
                        answer: Answer::Declined,
                    },
                    r#""kind":"question_resolved""#,
                ),
                (
                    TranscriptItem::QuestionResolved {
                        id: "3".to_owned(),
                        ask: Ask::Check {
                            prompt: "What says green?".to_owned(),
                            proposal: Some("cargo test".to_owned()),
                        },
                        answer: Answer::Check {
                            command: "cargo test --workspace".to_owned(),
                        },
                    },
                    r#""kind":"question_resolved""#,
                ),
            ] {
                let wire = serde_json::to_string(&item).unwrap();
                assert!(wire.contains(tag), "{wire}");
                let received: TranscriptItem = serde_json::from_str(&wire).unwrap();
                assert_eq!(received, item);
            }
        }

        /// Asks and answers are tagged inside the item, one tag each.
        #[test]
        fn asks_and_answers_carry_their_own_tags() {
            let ask = serde_json::to_string(&Ask::Repository {
                prompt: "Where?".to_owned(),
            })
            .unwrap();
            assert_eq!(ask, r#"{"kind":"repository","prompt":"Where?"}"#);
            let check = serde_json::to_string(&Ask::Check {
                prompt: "What says green?".to_owned(),
                proposal: None,
            })
            .unwrap();
            assert_eq!(
                check,
                r#"{"kind":"check","prompt":"What says green?","proposal":null}"#
            );
            assert_eq!(
                serde_json::to_string(&Answer::Check {
                    command: "make test".to_owned(),
                })
                .unwrap(),
                r#"{"kind":"check","command":"make test"}"#
            );
            assert_eq!(
                serde_json::to_string(&Answer::Declined).unwrap(),
                r#"{"kind":"declined"}"#
            );
        }

        /// A newer build's question kind does not decode here: tagged
        /// enums without an `#[serde(other)]` arm fail on an unknown
        /// tag. Acceptable — both ends of the barrier ship together —
        /// and documented rather than papered over with an `Unknown`.
        #[test]
        fn an_ask_kind_from_a_newer_build_fails_to_decode() {
            assert!(serde_json::from_str::<Ask>(r#"{"kind":"colour","prompt":"?"}"#).is_err());
            assert!(serde_json::from_str::<Answer>(r#"{"kind":"colour","value":"red"}"#).is_err());
        }

        #[test]
        fn both_roles_speak_their_wire_protocol_names() {
            assert_eq!(
                serde_json::to_string(&Role::Assistant).unwrap(),
                r#""assistant""#
            );
            assert_eq!(serde_json::to_string(&Role::User).unwrap(), r#""user""#);
        }

        fn text(role: Role, text: &str) -> ChatMessage {
            ChatMessage::Text {
                role,
                text: text.to_owned(),
            }
        }

        #[test]
        fn the_request_leads_with_the_system_message_and_streams() {
            let messages = [text(Role::User, "hi"), text(Role::Assistant, "hi yourself")];
            let body =
                serde_json::to_value(wire::request("m", "be brief", &messages, &[], None)).unwrap();
            assert_eq!(
                body,
                serde_json::json!({
                    "model": "m",
                    "messages": [
                        { "role": "system", "content": "be brief" },
                        { "role": "user", "content": "hi" },
                        { "role": "assistant", "content": "hi yourself" },
                    ],
                    "stream": true,
                })
            );
        }

        /// The byte-identity contract: a request with no tools serializes
        /// to exactly the JSON this client sent before tools existed.
        #[test]
        fn a_plain_request_is_byte_identical_to_the_pre_tools_shape() {
            let messages = [text(Role::User, "hi")];
            let body =
                serde_json::to_string(&wire::request("m", "s", &messages, &[], None)).unwrap();
            assert_eq!(
                body,
                r#"{"model":"m","messages":[{"role":"system","content":"s"},{"role":"user","content":"hi"}],"stream":true}"#
            );
        }

        #[test]
        fn tools_and_tool_choice_ride_along_when_present() {
            let tools = [ToolSpec::function(
                "current_time".to_owned(),
                "the local time".to_owned(),
                serde_json::json!({ "type": "object", "properties": {} }),
            )];
            let body =
                serde_json::to_value(wire::request("m", "s", &[], &tools, Some("auto"))).unwrap();
            assert_eq!(
                body["tools"],
                serde_json::json!([{
                    "type": "function",
                    "function": {
                        "name": "current_time",
                        "description": "the local time",
                        "parameters": { "type": "object", "properties": {} },
                    },
                }])
            );
            assert_eq!(body["tool_choice"], serde_json::json!("auto"));
        }

        #[test]
        fn a_tool_turn_maps_to_an_assistant_call_and_an_addressed_answer() {
            let messages = [
                text(Role::User, "what time is it?"),
                ChatMessage::ToolCalls(vec![ToolCall {
                    id: "call_1".to_owned(),
                    name: "current_time".to_owned(),
                    arguments: "{}".to_owned(),
                }]),
                ChatMessage::ToolResult {
                    id: "call_1".to_owned(),
                    content: "{\"local\":\"noon\"}".to_owned(),
                },
            ];
            let body = serde_json::to_value(wire::request("m", "s", &messages, &[], None)).unwrap();
            assert_eq!(
                body["messages"],
                serde_json::json!([
                    { "role": "system", "content": "s" },
                    { "role": "user", "content": "what time is it?" },
                    {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "current_time", "arguments": "{}" },
                        }],
                    },
                    {
                        "role": "tool",
                        "content": "{\"local\":\"noon\"}",
                        "tool_call_id": "call_1",
                    },
                ])
            );
        }

        #[test]
        fn only_completed_messages_map_from_the_transcript() {
            let transcript = [
                TranscriptItem::AssistantDelta {
                    text: "he".to_owned(),
                },
                TranscriptItem::TurnFailed {
                    reason: "x".to_owned(),
                },
                TranscriptItem::ToolCall {
                    name: "t".to_owned(),
                    arguments: "{}".to_owned(),
                },
                TranscriptItem::ToolResult {
                    name: "t".to_owned(),
                    ok: true,
                    content: "1".to_owned(),
                },
                message(Role::User, "hi"),
            ];
            assert_eq!(
                ChatMessage::from_transcript(&transcript),
                [text(Role::User, "hi")]
            );
        }

        #[test]
        fn a_content_delta_steps_forward() {
            let step = wire::step(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#).unwrap();
            assert_eq!(step, Step::Delta("Hel".to_owned()));
        }

        #[test]
        fn a_tool_call_delta_steps_into_fragments() {
            let step = wire::step(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"f","arguments":"{\"a\""}}]}}]}"#,
            )
            .unwrap();
            assert!(
                matches!(step, Step::ToolCalls(ref f) if f.len() == 1),
                "{step:?}"
            );
        }

        #[test]
        fn finish_reason_tool_calls_ends_the_speaking() {
            let step =
                wire::step(r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#).unwrap();
            assert_eq!(step, Step::ToolsFinished);
        }

        /// The id and name arrive once per index; the arguments string
        /// accumulates across as many fragments as the stream likes.
        #[test]
        fn fragmented_arguments_accumulate_into_one_call() {
            let mut calls = wire::Calls::default();
            for payload in [
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"f","arguments":""}}]}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"a\":"}}]}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}"#,
            ] {
                let Step::ToolCalls(fragments) = wire::step(payload).unwrap() else {
                    panic!("expected fragments");
                };
                calls.absorb(fragments);
            }
            assert_eq!(
                calls.finish(),
                [ToolCall {
                    id: "call_1".to_owned(),
                    name: "f".to_owned(),
                    arguments: "{\"a\":1}".to_owned(),
                }]
            );
        }

        /// Two calls whose fragments interleave still come out whole, in
        /// index order.
        #[test]
        fn interleaved_indexes_assemble_into_ordered_calls() {
            let mut calls = wire::Calls::default();
            for payload in [
                r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"beta","arguments":"{\"b\""}}]}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"alpha","arguments":"{}"}}]}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":":2}"}}]}}]}"#,
            ] {
                let Step::ToolCalls(fragments) = wire::step(payload).unwrap() else {
                    panic!("expected fragments");
                };
                calls.absorb(fragments);
            }
            assert_eq!(
                calls.finish(),
                [
                    ToolCall {
                        id: "call_a".to_owned(),
                        name: "alpha".to_owned(),
                        arguments: "{}".to_owned(),
                    },
                    ToolCall {
                        id: "call_b".to_owned(),
                        name: "beta".to_owned(),
                        arguments: "{\"b\":2}".to_owned(),
                    },
                ]
            );
        }

        #[test]
        fn a_role_announcement_is_housekeeping() {
            let step = wire::step(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#).unwrap();
            assert_eq!(step, Step::Skip);
        }

        #[test]
        fn done_ends_the_stream() {
            assert_eq!(wire::step("[DONE]").unwrap(), Step::Done);
        }

        #[test]
        fn gibberish_is_a_malformed_stream_not_a_panic() {
            assert!(matches!(
                wire::step("not json"),
                Err(ChatError::MalformedStream(_))
            ));
        }

        #[test]
        fn a_tool_call_item_pretty_prints_arguments_that_parse() {
            let item = TranscriptItem::tool_call(&ToolCall {
                id: "call_1".to_owned(),
                name: "f".to_owned(),
                arguments: r#"{"a":1}"#.to_owned(),
            });
            assert_eq!(
                item,
                TranscriptItem::ToolCall {
                    name: "f".to_owned(),
                    arguments: "{\n  \"a\": 1\n}".to_owned(),
                }
            );
        }

        #[test]
        fn unparseable_arguments_reach_the_transcript_verbatim() {
            let item = TranscriptItem::tool_call(&ToolCall {
                id: "call_1".to_owned(),
                name: "f".to_owned(),
                arguments: "{\"a\":".to_owned(),
            });
            assert!(matches!(
                item,
                TranscriptItem::ToolCall { arguments, .. } if arguments == "{\"a\":"
            ));
        }

        #[test]
        fn a_result_within_the_cap_is_kept_whole() {
            let item = TranscriptItem::tool_result("f", &Ok(serde_json::json!({ "a": 1 })));
            assert_eq!(
                item,
                TranscriptItem::ToolResult {
                    name: "f".to_owned(),
                    ok: true,
                    content: "{\n  \"a\": 1\n}".to_owned(),
                }
            );
        }

        #[test]
        fn a_failed_result_carries_the_tools_words_and_is_not_ok() {
            let item = TranscriptItem::tool_result("f", &Err("no such day".to_owned()));
            assert_eq!(
                item,
                TranscriptItem::ToolResult {
                    name: "f".to_owned(),
                    ok: false,
                    content: "no such day".to_owned(),
                }
            );
        }

        /// The boundary: exactly at the cap nothing is cut; one past it,
        /// the transcript keeps the cap's worth plus an elision note.
        #[test]
        fn the_transcript_cap_cuts_only_past_the_boundary() {
            let at_cap = "x".repeat(TOOL_RESULT_CAP);
            let TranscriptItem::ToolResult { content, .. } =
                TranscriptItem::tool_result("f", &Err(at_cap.clone()))
            else {
                panic!("a result item");
            };
            assert_eq!(content, at_cap);

            let over = "x".repeat(TOOL_RESULT_CAP + 1);
            let TranscriptItem::ToolResult { content, .. } =
                TranscriptItem::tool_result("f", &Err(over))
            else {
                panic!("a result item");
            };
            assert_eq!(
                content,
                format!(
                    "{}\n… (1 more characters elided)",
                    "x".repeat(TOOL_RESULT_CAP)
                )
            );
        }

        #[test]
        fn the_apis_own_error_message_survives_verbatim() {
            let body = r#"{"error":{"message":"Your credit balance is too low.","type":"invalid_request_error"}}"#;
            assert_eq!(
                wire::api_message("400 Bad Request", body),
                "Your credit balance is too low."
            );
            assert_eq!(wire::api_message("500", "backend melted"), "backend melted");
            assert_eq!(
                wire::api_message("502 Bad Gateway", "  "),
                "502 Bad Gateway"
            );
        }
    }

    /// A scripted HTTP server on a std listener: enough protocol to prove
    /// the client end to end, with no network and no runtime.
    #[cfg(feature = "native")]
    mod scripted {
        use super::*;
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::AtomicBool;

        /// Serves `response` to one connection, after draining the
        /// request. Returns the base URL to aim the client at.
        fn serve(response: &'static str) -> String {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/v1", listener.local_addr().unwrap());
            std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                // Read to the end of the body: headers, then the JSON the
                // client said it would send.
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let headers = String::from_utf8_lossy(&request).to_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|length| length.trim().parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; length];
                socket.read_exact(&mut body).unwrap();
                socket.write_all(response.as_bytes()).unwrap();
            });
            base
        }

        #[test]
        fn deltas_arrive_in_order_and_concatenate_into_the_reply() {
            let base = serve(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
                 data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
                 data: [DONE]\n\n",
            );
            let client = Client::new(base, "scripted".to_owned(), None);
            let mut deltas = Vec::new();

            let reply = client
                .reply(
                    "system",
                    &[ChatMessage::Text {
                        role: Role::User,
                        text: "hi".to_owned(),
                    }],
                    &[],
                    |delta| deltas.push(delta.to_owned()),
                    &AtomicBool::new(false),
                )
                .unwrap();

            assert_eq!(deltas, ["Hel", "lo"]);
            assert_eq!(reply, Reply::Text("Hello".to_owned()));
        }

        #[test]
        fn an_api_error_carries_the_bodys_message() {
            let body = r#"{"error":{"message":"Your credit balance is too low."}}"#;
            let base = serve(
                "HTTP/1.1 402 Payment Required\r\ncontent-type: application/json\r\ncontent-length: 55\r\nconnection: close\r\n\r\n{\"error\":{\"message\":\"Your credit balance is too low.\"}}",
            );
            assert_eq!(body.len(), 55, "the scripted content-length is honest");
            let client = Client::new(base, "scripted".to_owned(), None);

            let error = client
                .reply("system", &[], &[], |_| {}, &AtomicBool::new(false))
                .unwrap_err();

            assert_eq!(
                error,
                ChatError::Api("Your credit balance is too low.".to_owned())
            );
        }

        #[test]
        fn gibberish_mid_stream_is_a_malformed_stream_error() {
            let base = serve(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
                 data: what protocol\n\n",
            );
            let client = Client::new(base, "scripted".to_owned(), None);

            let error = client
                .reply("system", &[], &[], |_| {}, &AtomicBool::new(false))
                .unwrap_err();

            assert!(matches!(error, ChatError::MalformedStream(_)), "{error}");
        }

        #[test]
        fn the_model_list_comes_back_in_the_apis_order_with_extra_fields_absorbed() {
            let body = r#"{"data":[{"id":"claude-new","display_name":"Claude New","type":"model","created_at":"2026-01-01T00:00:00Z"},{"id":"claude-old","display_name":"Claude Old"}],"has_more":false,"first_id":"claude-new","last_id":"claude-old"}"#;
            let base = serve(Box::leak(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .into_boxed_str(),
            ));

            let models = models_at(&base, &Secret::from("sk-test")).unwrap();

            assert_eq!(
                models,
                [
                    ModelInfo {
                        id: "claude-new".to_owned(),
                        display_name: "Claude New".to_owned(),
                    },
                    ModelInfo {
                        id: "claude-old".to_owned(),
                        display_name: "Claude Old".to_owned(),
                    },
                ]
            );
        }

        #[test]
        fn a_refused_model_list_carries_the_apis_words() {
            let body = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
            let base = serve(Box::leak(
                format!(
                    "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .into_boxed_str(),
            ));

            let error = models_at(&base, &Secret::from("sk-wrong")).unwrap_err();

            assert_eq!(error, ChatError::Api("invalid x-api-key".to_owned()));
        }

        #[test]
        fn a_model_list_nobody_answers_is_a_transport_error() {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/v1", listener.local_addr().unwrap());
            drop(listener);

            let error = models_at(&base, &Secret::from("sk-test")).unwrap_err();

            assert!(matches!(error, ChatError::Transport(_)), "{error}");
        }
    }

    /// The scripted-model fixture driving the real client: the stream's
    /// tool-call grammar, fragment by fragment, over an actual socket.
    #[cfg(feature = "scripted")]
    mod fixture {
        use super::*;
        use crate::chat::scripted::{Fragment, Scripted, Turn};
        use std::sync::atomic::AtomicBool;

        #[test]
        fn interleaved_fragmented_tool_calls_come_back_whole_and_ordered() {
            let model = Scripted::spawn(vec![Turn::ToolCalls(vec![
                vec![Fragment::open(0, "call_a", "alpha", "{\"a\"")],
                vec![Fragment::open(1, "call_b", "beta", "")],
                vec![Fragment::more(0, ":1}"), Fragment::more(1, "{}")],
            ])]);
            let client = Client::new(model.base_url(), "scripted".to_owned(), None);

            let reply = client
                .reply("system", &[], &[], |_| {}, &AtomicBool::new(false))
                .unwrap();

            assert_eq!(
                reply,
                Reply::ToolCalls(vec![
                    ToolCall {
                        id: "call_a".to_owned(),
                        name: "alpha".to_owned(),
                        arguments: "{\"a\":1}".to_owned(),
                    },
                    ToolCall {
                        id: "call_b".to_owned(),
                        name: "beta".to_owned(),
                        arguments: "{}".to_owned(),
                    },
                ])
            );
        }

        #[test]
        fn a_truncated_stream_still_yields_what_it_said() {
            let model = Scripted::spawn(vec![Turn::Malformed(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n"
                    .to_owned(),
            )]);
            let client = Client::new(model.base_url(), "scripted".to_owned(), None);

            let reply = client
                .reply("system", &[], &[], |_| {}, &AtomicBool::new(false))
                .unwrap();

            assert_eq!(reply, Reply::Text("Hello".to_owned()));
        }

        #[test]
        fn garbage_json_is_a_malformed_stream_error() {
            let model = Scripted::spawn(vec![Turn::Malformed("data: {not json\n\n".to_owned())]);
            let client = Client::new(model.base_url(), "scripted".to_owned(), None);

            let error = client
                .reply("system", &[], &[], |_| {}, &AtomicBool::new(false))
                .unwrap_err();

            assert!(matches!(error, ChatError::MalformedStream(_)), "{error}");
        }

        #[test]
        fn a_request_beyond_the_script_fails_with_a_useful_message() {
            let model = Scripted::spawn(vec![]);
            let client = Client::new(model.base_url(), "scripted".to_owned(), None);

            let error = client
                .reply("system", &[], &[], |_| {}, &AtomicBool::new(false))
                .unwrap_err();

            assert_eq!(
                error,
                ChatError::Api(
                    "the scripted model's script has ended; request 1 is beyond it".to_owned()
                )
            );
        }
    }

    /// The live tier: a real turn against whatever EPIK_LIVE_BASE_URL and
    /// EPIK_LIVE_MODEL point at — which also proves the client is not
    /// Anthropic-specific. Skipped silently when the environment says
    /// nothing.
    #[cfg(feature = "native")]
    mod live {
        use super::*;
        use std::sync::atomic::AtomicBool;

        #[test]
        fn a_live_turn_streams_a_reply() {
            let (Ok(base), Ok(model)) = (
                std::env::var("EPIK_LIVE_BASE_URL"),
                std::env::var("EPIK_LIVE_MODEL"),
            ) else {
                eprintln!("live tier unset; skipping");
                return;
            };
            let client = Client::new(base, model, None);
            let mut streamed = String::new();

            let reply = client
                .reply(
                    "Answer in one short sentence.",
                    &[ChatMessage::Text {
                        role: Role::User,
                        text: "Say hello.".to_owned(),
                    }],
                    &[],
                    |delta| streamed.push_str(delta),
                    &AtomicBool::new(false),
                )
                .unwrap();

            let Reply::Text(reply) = reply else {
                panic!("no tools were offered, so the reply is text");
            };
            assert!(!reply.is_empty());
            assert_eq!(streamed, reply, "the deltas are the reply");
        }
    }
}
