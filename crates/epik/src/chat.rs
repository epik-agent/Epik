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
    /// endpoint, speaking to [`ANTHROPIC_MODEL`].
    #[must_use]
    pub fn anthropic(key: Secret) -> Self {
        Self::new(
            "https://api.anthropic.com/v1".to_owned(),
            ANTHROPIC_MODEL.to_owned(),
            Some(key),
        )
    }
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
    use super::{ChatError, Role, Step, TranscriptItem};

    #[derive(serde::Serialize)]
    pub(super) struct Request<'a> {
        model: &'a str,
        messages: Vec<Message<'a>>,
        stream: bool,
    }

    #[derive(serde::Serialize)]
    struct Message<'a> {
        role: &'a str,
        content: &'a str,
    }

    const fn role_name(role: Role) -> &'static str {
        match role {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }

    /// The request body: the system message first, then the transcript's
    /// completed messages in order. Deltas and failures are not
    /// conversation and do not go to the wire.
    pub(super) fn request<'a>(
        model: &'a str,
        system: &'a str,
        transcript: &'a [TranscriptItem],
    ) -> Request<'a> {
        let mut messages = vec![Message {
            role: "system",
            content: system,
        }];
        messages.extend(transcript.iter().filter_map(|item| match item {
            TranscriptItem::Message { role, text } => Some(Message {
                role: role_name(*role),
                content: text,
            }),
            TranscriptItem::AssistantDelta { .. } | TranscriptItem::TurnFailed { .. } => None,
        }));
        Request {
            model,
            messages,
            stream: true,
        }
    }

    #[derive(serde::Deserialize)]
    struct Chunk {
        choices: Vec<Choice>,
    }

    #[derive(serde::Deserialize)]
    struct Choice {
        delta: Delta,
    }

    #[derive(serde::Deserialize)]
    struct Delta {
        #[serde(default)]
        content: Option<String>,
    }

    /// Reads one data payload of the stream.
    pub(super) fn step(payload: &str) -> Result<Step, ChatError> {
        if payload.trim() == "[DONE]" {
            return Ok(Step::Done);
        }
        let chunk: Chunk = serde_json::from_str(payload)
            .map_err(|_| ChatError::MalformedStream(payload.to_owned()))?;
        Ok(chunk
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.delta.content)
            .map_or(Step::Skip, Step::Delta))
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
    /// The model's reply to `transcript`, with `system` said first and
    /// kept out of it.
    ///
    /// Deltas reach `on_delta` as the model produces them; the return is
    /// the whole reply. The caller owns history — nothing here remembers
    /// anything. `stop` is checked between deltas; once it is set, the
    /// reply so far comes back and the rest of the stream is left where
    /// it is.
    ///
    /// # Errors
    ///
    /// [`ChatError`], distinguishing transport failures, the API's own
    /// errors (verbatim), and a stream that stopped making sense.
    pub fn reply(
        &self,
        system: &str,
        transcript: &[TranscriptItem],
        mut on_delta: impl FnMut(&str),
        stop: &AtomicBool,
    ) -> Result<String, ChatError> {
        use std::io::{BufRead, BufReader};
        use std::sync::atomic::Ordering;

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = serde_json::to_string(&wire::request(&self.model, system, transcript))
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
        let reader = BufReader::new(response.body_mut().as_reader());
        for line in reader.lines() {
            if stop.load(Ordering::Relaxed) {
                return Ok(reply);
            }
            let line = line.map_err(|error| ChatError::Transport(error.to_string()))?;
            let Some(payload) = events.line(&line) else {
                continue;
            };
            match wire::step(&payload)? {
                Step::Delta(delta) => {
                    on_delta(&delta);
                    reply.push_str(&delta);
                }
                Step::Done => return Ok(reply),
                Step::Skip => {}
            }
        }
        if let Some(payload) = events.flush()
            && let Step::Delta(delta) = wire::step(&payload)?
        {
            on_delta(&delta);
            reply.push_str(&delta);
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ] {
                let wire = serde_json::to_string(&item).unwrap();
                assert!(wire.contains(tag), "{wire}");
                let received: TranscriptItem = serde_json::from_str(&wire).unwrap();
                assert_eq!(received, item);
            }
        }

        #[test]
        fn both_roles_speak_their_wire_protocol_names() {
            assert_eq!(
                serde_json::to_string(&Role::Assistant).unwrap(),
                r#""assistant""#
            );
            assert_eq!(serde_json::to_string(&Role::User).unwrap(), r#""user""#);
        }

        #[test]
        fn the_request_leads_with_the_system_message_and_streams() {
            let transcript = [
                message(Role::User, "hi"),
                message(Role::Assistant, "hi yourself"),
            ];
            let body = serde_json::to_value(wire::request("m", "be brief", &transcript)).unwrap();
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

        #[test]
        fn deltas_and_failures_never_go_to_the_wire() {
            let transcript = [
                TranscriptItem::AssistantDelta {
                    text: "he".to_owned(),
                },
                TranscriptItem::TurnFailed {
                    reason: "x".to_owned(),
                },
                message(Role::User, "hi"),
            ];
            let body = serde_json::to_value(wire::request("m", "s", &transcript)).unwrap();
            assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        }

        #[test]
        fn a_content_delta_steps_forward() {
            let step = wire::step(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#).unwrap();
            assert_eq!(step, Step::Delta("Hel".to_owned()));
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
                    &[super::message(Role::User, "hi")],
                    |delta| deltas.push(delta.to_owned()),
                    &AtomicBool::new(false),
                )
                .unwrap();

            assert_eq!(deltas, ["Hel", "lo"]);
            assert_eq!(reply, "Hello");
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
                .reply("system", &[], |_| {}, &AtomicBool::new(false))
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
                .reply("system", &[], |_| {}, &AtomicBool::new(false))
                .unwrap_err();

            assert!(matches!(error, ChatError::MalformedStream(_)), "{error}");
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
                    &[super::message(Role::User, "Say hello.")],
                    |delta| streamed.push_str(delta),
                    &AtomicBool::new(false),
                )
                .unwrap();

            assert!(!reply.is_empty());
            assert_eq!(streamed, reply, "the deltas are the reply");
        }
    }
}
