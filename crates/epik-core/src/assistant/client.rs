//! The client that speaks to the model.
//!
//! [`Client`] holds a conversation with an OpenAI-compatible
//! chat-completions endpoint — transcript in, streamed deltas out. It is
//! provider-agnostic data: a base URL, a model, a key. Provider
//! specificity lives only in constructors, of which there is exactly one
//! today, [`Client::anthropic`]. [`run`] is a whole turn: it calls the
//! model with a [`Registry`] of tools on offer, dispatches whatever the
//! model asks for, tells it what happened, and repeats until it answers
//! in text. The library is synchronous; running a turn somewhere it
//! won't block anything is the host's problem.

#[cfg(feature = "native")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "native")]
use super::{
    TranscriptItem,
    wire::{self, Events, Message, Step},
};
#[cfg(feature = "native")]
use crate::keystore::Secret;
#[cfg(feature = "native")]
use crate::model_tools::{Registry, ToolCall, ToolSpec};

/// The pinned Anthropic model [`Client::anthropic`] speaks to.
pub const ANTHROPIC_MODEL: &str = "claude-sonnet-5";

/// One model a provider will answer for: the id the configuration
/// states and the name a dropdown shows.
///
/// Unknown fields are absorbed, in the manner of the other wire types:
/// the Models endpoint says more about a model than a dropdown needs.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
}

/// What a reply turned out to be: the model either spoke or reached for
/// tools, never both.
#[cfg(feature = "native")]
#[derive(Clone, Debug, Eq, PartialEq)]
enum Reply {
    /// The final text, assembled from the deltas.
    Text(String),
    /// The calls the model wants made, in index order.
    ToolCalls(Vec<ToolCall>),
}

/// What a turn can die of. The key never appears in any of it.
#[cfg(feature = "native")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
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

#[cfg(feature = "native")]
impl std::fmt::Display for Error {
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

#[cfg(feature = "native")]
impl std::error::Error for Error {}

/// A client for one OpenAI-compatible chat-completions endpoint: where to
/// ask, what to ask for, and what to present at the door.
#[cfg(feature = "native")]
#[derive(Debug)]
pub struct Client {
    base_url: String,
    model: String,
    key: Option<Secret>,
}

#[cfg(feature = "native")]
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
    /// [`Error`], distinguishing transport failures, the API's own
    /// errors (verbatim), and a stream that stopped making sense.
    fn reply(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        mut on_delta: impl FnMut(&str),
        stop: &AtomicBool,
    ) -> Result<Reply, Error> {
        use std::io::{BufRead, BufReader};

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
            .map_err(|error| Error::Transport(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.body_mut().read_to_string().unwrap_or_default();
            return Err(Error::Api(wire::api_message(&status.to_string(), &body)));
        }

        let mut events = Events::default();
        let mut reply = String::new();
        let mut calls = wire::Calls::default();
        let mut tools_finished = false;
        let mut step =
            |payload: &str, reply: &mut String, calls: &mut wire::Calls| -> Result<bool, Error> {
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
            let line = line.map_err(|error| Error::Transport(error.to_string()))?;
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
/// [`Error`], distinguishing transport failures, the API's own
/// errors (verbatim), and an answer that was not a model list.
#[cfg(feature = "native")]
pub fn models(key: &Secret) -> Result<Vec<ModelInfo>, Error> {
    models_at(ANTHROPIC_API, key)
}

/// [`models`], against a stated base — the prefix ending in `/v1` —
/// which is how a test aims it at a loopback port.
#[cfg(feature = "native")]
fn models_at(base_url: &str, key: &Secret) -> Result<Vec<ModelInfo>, Error> {
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
        .map_err(|error| Error::Transport(error.to_string()))?;
    let status = response.status();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| Error::Transport(error.to_string()))?;
    if !status.is_success() {
        return Err(Error::Api(wire::api_message(&status.to_string(), &body)));
    }
    let page: Page = serde_json::from_str(&body)
        .map_err(|error| Error::MalformedStream(format!("not a model list: {error}")))?;
    Ok(page.data)
}

/// The most rounds of tool calls one turn may take before it is a turn
/// failure. A model that hasn't answered in this many is not going to.
#[cfg(feature = "native")]
const MAX_TOOL_ITERATIONS: usize = 16;

/// One whole turn, run until the model answers in text.
///
/// Calls [`Client::reply`] with the registry's tools on offer; when the
/// reply is tool calls, dispatches each in order, reports every call and
/// its result to `observer`, appends what happened to the working
/// messages, and asks again. Deltas stream to `on_delta` as ever. `stop`
/// is checked between deltas (inside `reply`) and between iterations —
/// once set, the turn stands down quietly with what text it has.
///
/// # Errors
///
/// Everything [`Client::reply`] can die of, plus [`Error::ToolLoop`]
/// when [`MAX_TOOL_ITERATIONS`] rounds pass without an answer.
#[cfg(feature = "native")]
pub fn run(
    client: &Client,
    system: &str,
    transcript: &[TranscriptItem],
    registry: &Registry,
    mut on_delta: impl FnMut(&str),
    mut observer: impl FnMut(&ToolCall, &Result<serde_json::Value, String>),
    stop: &AtomicBool,
) -> Result<String, Error> {
    let tools = registry.to_wire();
    let mut messages = Message::from_transcript(transcript);
    for _ in 0..MAX_TOOL_ITERATIONS {
        if stop.load(Ordering::Relaxed) {
            return Ok(String::new());
        }
        match client.reply(system, &messages, &tools, &mut on_delta, stop)? {
            Reply::Text(text) => return Ok(text),
            Reply::ToolCalls(calls) => {
                messages.push(Message::ToolCalls(calls.clone()));
                for call in calls {
                    let result = registry.dispatch(&call.name, &call.arguments);
                    observer(&call, &result);
                    let content = match &result {
                        Ok(value) => value.to_string(),
                        Err(reason) => reason.clone(),
                    };
                    messages.push(Message::ToolResult {
                        id: call.id,
                        content,
                    });
                }
            }
        }
    }
    Err(Error::ToolLoop {
        limit: MAX_TOOL_ITERATIONS,
    })
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::assistant::Role;
    use crate::assistant::scripted::{Fragment, Scripted, Turn, read_request};

    /// One reply from the model at `base` to an empty conversation: the
    /// least a client can ask, for tests about the stream rather than
    /// the messages.
    fn reply_to_nothing(base: String) -> Result<Reply, Error> {
        Client::new(base, "scripted".to_owned(), None).reply(
            "system",
            &[],
            &[],
            |_| {},
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn anthropic_resolves_an_absent_model_to_the_pinned_default() {
        assert_eq!(Client::anthropic("k".into(), None).model, ANTHROPIC_MODEL);
        assert_eq!(
            Client::anthropic("k".into(), Some("other".to_owned())).model,
            "other"
        );
    }

    /// A scripted HTTP server on a std listener: enough protocol to prove
    /// the client end to end, with no network and no runtime.
    mod served {
        use super::*;
        use std::io::Write;
        use std::net::TcpListener;

        /// Serves `response` to one connection, after draining the
        /// request. Returns the base URL to aim the client at.
        fn serve(response: impl Into<String>) -> String {
            let response = response.into();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/v1", listener.local_addr().unwrap());
            std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                read_request(&mut socket).expect("a whole request");
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
                    &[Message::Text {
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
                "HTTP/1.1 402 Payment Required\r\ncontent-type: application/json\r\ncontent-length: 55\r\nconnection: close\r\n\r\n{\"error\":{\"message\":\"Your credit balance is too low.\"}}"
                    .to_owned(),
            );
            assert_eq!(body.len(), 55, "the scripted content-length is honest");

            let error = reply_to_nothing(base).unwrap_err();

            assert_eq!(
                error,
                Error::Api("Your credit balance is too low.".to_owned())
            );
        }

        #[test]
        fn gibberish_mid_stream_is_a_malformed_stream_error() {
            let base = serve(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
                 data: what protocol\n\n",
            );

            let error = reply_to_nothing(base).unwrap_err();

            assert!(matches!(error, Error::MalformedStream(_)), "{error}");
        }

        #[test]
        fn the_model_list_comes_back_in_the_apis_order_with_extra_fields_absorbed() {
            let body = r#"{"data":[{"id":"claude-new","display_name":"Claude New","type":"model","created_at":"2026-01-01T00:00:00Z"},{"id":"claude-old","display_name":"Claude Old"}],"has_more":false,"first_id":"claude-new","last_id":"claude-old"}"#;
            let base = serve(format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
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
            let base = serve(format!(
                "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            ));

            let error = models_at(&base, &Secret::from("sk-wrong")).unwrap_err();

            assert_eq!(error, Error::Api("invalid x-api-key".to_owned()));
        }

        #[test]
        fn a_model_list_nobody_answers_is_a_transport_error() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/v1", listener.local_addr().unwrap());
            drop(listener);

            let error = models_at(&base, &Secret::from("sk-test")).unwrap_err();

            assert!(matches!(error, Error::Transport(_)), "{error}");
        }
    }

    /// The scripted-model fixture driving the real client: the stream's
    /// tool-call grammar, fragment by fragment, over an actual socket.
    mod fixture {
        use super::*;

        #[test]
        fn interleaved_fragmented_tool_calls_come_back_whole_and_ordered() {
            let model = Scripted::spawn(vec![Turn::ToolCalls(vec![
                vec![Fragment::open(0, "call_a", "alpha", "{\"a\"")],
                vec![Fragment::open(1, "call_b", "beta", "")],
                vec![Fragment::more(0, ":1}"), Fragment::more(1, "{}")],
            ])]);

            let reply = reply_to_nothing(model.base_url()).unwrap();

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

            let reply = reply_to_nothing(model.base_url()).unwrap();

            assert_eq!(reply, Reply::Text("Hello".to_owned()));
        }

        #[test]
        fn garbage_json_is_a_malformed_stream_error() {
            let model = Scripted::spawn(vec![Turn::Malformed("data: {not json\n\n".to_owned())]);

            let error = reply_to_nothing(model.base_url()).unwrap_err();

            assert!(matches!(error, Error::MalformedStream(_)), "{error}");
        }

        #[test]
        fn a_request_beyond_the_script_fails_with_a_useful_message() {
            let model = Scripted::spawn(vec![]);

            let error = reply_to_nothing(model.base_url()).unwrap_err();

            assert_eq!(
                error,
                Error::Api(
                    "the scripted model's script has ended; request 1 is beyond it".to_owned()
                )
            );
        }
    }

    /// The live tier: a real turn against whatever EPIK_LIVE_BASE_URL and
    /// EPIK_LIVE_MODEL point at — which also proves the client is not
    /// Anthropic-specific. Skipped silently when the environment says
    /// nothing.
    ///
    /// Two ways this surprises. It skips only when the variables are unset,
    /// so a `.cargo/config.toml` that always sets them makes the test fail
    /// with `Transport("io: Connection refused")` at the `reply` call
    /// whenever the local server behind them is down — start the server,
    /// not the debugger. And it lives behind the `native` feature, which is
    /// off by default: `cargo test --package epik-core` compiles it out,
    /// while `cargo test --workspace` gets it through feature unification
    /// with the crates that enable `native`.
    mod live {
        use super::*;

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
                    &[Message::Text {
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

    /// The loop end to end, against the scripted model: a real client, a
    /// loopback socket, and a script the test controls exactly.
    mod looped {
        use super::*;
        use crate::model_tools::echoing;

        fn transcript() -> Vec<TranscriptItem> {
            vec![TranscriptItem::Message {
                role: Role::User,
                text: "go".to_owned(),
            }]
        }

        fn run_against(
            model: &Scripted,
            registry: &Registry,
            observer: impl FnMut(&ToolCall, &Result<serde_json::Value, String>),
        ) -> Result<String, Error> {
            let client = Client::new(model.base_url(), "scripted".to_owned(), None);
            run(
                &client,
                "system",
                &transcript(),
                registry,
                |_| {},
                observer,
                &AtomicBool::new(false),
            )
        }

        /// One turn over the echo registry against a model scripted to
        /// make the tool calls `calls` and then say "done": what `note`
        /// makes of each call and its result, in dispatch order, and
        /// the requests the model saw.
        fn observed<T>(
            calls: Vec<Vec<Fragment>>,
            mut note: impl FnMut(&ToolCall, &Result<serde_json::Value, String>) -> T,
        ) -> (Vec<T>, Vec<serde_json::Value>) {
            let model = Scripted::spawn(vec![Turn::ToolCalls(calls), Turn::text(&["done"])]);
            let mut observed = Vec::new();
            let text = run_against(&model, &echoing(), |call, result| {
                observed.push(note(call, result));
            })
            .unwrap();
            assert_eq!(text, "done");
            (observed, model.requests())
        }

        #[test]
        fn a_tool_call_turn_dispatches_and_reports_back_to_the_model() {
            let (observed, requests) = observed(
                vec![
                    vec![Fragment::open(0, "call_1", "echo", "{\"text\":")],
                    vec![Fragment::more(0, "\"hi\"}")],
                ],
                |call, result| (call.clone(), result.clone()),
            );

            assert_eq!(
                observed,
                [(
                    ToolCall {
                        id: "call_1".to_owned(),
                        name: "echo".to_owned(),
                        arguments: "{\"text\":\"hi\"}".to_owned(),
                    },
                    Ok(serde_json::json!({ "echo": "hi" })),
                )]
            );

            assert_eq!(requests.len(), 2);
            assert_eq!(
                requests[0]["tools"][0]["function"]["name"], "echo",
                "the tools ride the first request"
            );
            let messages = requests[1]["messages"].as_array().unwrap();
            assert_eq!(
                &messages[messages.len() - 2..],
                &[
                    serde_json::json!({
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "echo", "arguments": "{\"text\":\"hi\"}" },
                        }],
                    }),
                    serde_json::json!({
                        "role": "tool",
                        "content": "{\"echo\":\"hi\"}",
                        "tool_call_id": "call_1",
                    }),
                ],
                "the second request carries the call and its answer, in order"
            );
        }

        #[test]
        fn two_tools_in_one_turn_dispatch_in_index_order() {
            let (observed, requests) = observed(
                vec![vec![
                    Fragment::open(0, "call_a", "echo", "{\"text\":\"one\"}"),
                    Fragment::open(1, "call_b", "nonesuch", "{}"),
                ]],
                |call, result| (call.name.clone(), result.is_ok()),
            );

            assert_eq!(
                observed,
                [("echo".to_owned(), true), ("nonesuch".to_owned(), false)],
                "the observer fires in dispatch order, failures included"
            );

            let messages = requests[1]["messages"].as_array().unwrap().clone();
            let tool_answers: Vec<_> = messages
                .iter()
                .filter(|message| message["role"] == "tool")
                .map(|message| message["tool_call_id"].as_str().unwrap().to_owned())
                .collect();
            assert_eq!(tool_answers, ["call_a", "call_b"]);
        }

        /// The unknown tool's Err went back to the model as content, not
        /// up as a failure — the turn still ended in text.
        #[test]
        fn a_failed_dispatch_feeds_the_model_the_words() {
            let model = Scripted::spawn(vec![
                Turn::ToolCalls(vec![vec![Fragment::open(0, "call_1", "nonesuch", "{}")]]),
                Turn::text(&["ok"]),
            ]);

            let text = run_against(&model, &Registry::default(), |_, _| {}).unwrap();

            assert_eq!(text, "ok");
            let messages = model.requests()[1]["messages"].as_array().unwrap().clone();
            let answer = messages.last().unwrap();
            assert_eq!(answer["role"], "tool");
            assert!(
                answer["content"].as_str().unwrap().contains("nonesuch"),
                "{answer}"
            );
        }

        #[test]
        fn a_turn_that_never_stops_calling_tools_is_a_turn_failure() {
            let script = std::iter::repeat_n(
                Turn::ToolCalls(vec![vec![Fragment::open(
                    0,
                    "call_1",
                    "echo",
                    "{\"text\":\"x\"}",
                )]]),
                MAX_TOOL_ITERATIONS,
            )
            .collect();
            let model = Scripted::spawn(script);
            let registry = echoing();

            let error = run_against(&model, &registry, |_, _| {}).unwrap_err();

            assert_eq!(
                error,
                Error::ToolLoop {
                    limit: MAX_TOOL_ITERATIONS
                }
            );
            assert!(error.to_string().contains("16"), "{error}");
            assert_eq!(model.requests().len(), MAX_TOOL_ITERATIONS);
        }

        #[test]
        fn a_plain_text_turn_calls_no_tools_and_asks_once() {
            let model = Scripted::spawn(vec![Turn::text(&["Hel", "lo"])]);

            let text = run_against(&model, &Registry::standard(), |_, _| {
                panic!("no tool was called");
            })
            .unwrap();

            assert_eq!(text, "Hello");
            assert_eq!(model.requests().len(), 1);
        }
    }
}
