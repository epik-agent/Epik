//! One hand-rolled client for the OpenAI chat-completions wire format.
//!
//! That single format covers OpenAI, Ollama (local and free), Groq, Gemini,
//! OpenRouter, and Anthropic's compatibility endpoint, which is where
//! model-agnosticism actually comes from here — the protocol, not a
//! framework crate. The surface needed is small, the trait is Epik's own
//! seam, and provider quirks are ours to absorb rather than a dependency's to
//! reinterpret.
//!
//! Decoding is unknown-tolerant on purpose. Unmodelled fields are ignored,
//! `choices` may be empty (Ollama sends a final usage-only chunk that way),
//! and empty deltas are dropped rather than emitted.

use std::io::BufReader;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::chat::{
    ChatModel, Keyed, Message, Reply, StopToken, ToolCall, ToolDeclaration, ToolKind, sse,
};
use crate::config::Provider;
use crate::event::{ChatEvent, Usage};
use crate::logging::Log;

/// How long to wait for the endpoint to answer at all. The stream itself is
/// deliberately untimed: a slow reply is not a stalled one, and the stop
/// token is what ends a turn early.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// A model reached over the OpenAI chat-completions protocol.
#[derive(Debug)]
pub struct OpenAiCompatible {
    endpoint: String,
    model: String,
    key: Option<String>,
    tools: Vec<ToolDeclaration>,
    agent: ureq::Agent,
}

impl OpenAiCompatible {
    /// A client for `provider`, authenticating with `key` when there is one.
    ///
    /// A missing key is not an error here: local servers want none, and for
    /// the ones that do, the refusal belongs to the provider's answer rather
    /// than to this constructor.
    #[must_use]
    pub fn new(provider: &Provider, key: Option<String>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            // Epik reports the provider's own words, so it needs the body of
            // a rejection rather than just its status code.
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            endpoint: format!(
                "{}/chat/completions",
                provider.base_url.trim_end_matches('/')
            ),
            model: provider.model.clone(),
            key,
            tools: Vec::new(),
            agent,
        }
    }

    /// The endpoint this client posts to — useful in an error message, and in
    /// a test that wants to know where a request would have gone.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Offers `tools` to the model on every request from here on. The default
    /// is none, which keeps `tools` off the wire entirely.
    pub fn use_tools(&mut self, tools: Vec<ToolDeclaration>) {
        self.tools = tools;
    }
}

impl Keyed for OpenAiCompatible {
    /// The next request authenticates with `key`. The connection is per-turn,
    /// so nothing has to be torn down for this to take.
    fn use_key(&mut self, key: Option<String>) {
        self.key = key;
    }
}

/// What goes out. The transcript is resent every turn: the protocol is
/// stateless, so the caller's history *is* the model's memory.
#[derive(Debug, Serialize)]
struct Request<'a> {
    model: &'a str,
    messages: &'a [Message],
    /// Omitted entirely when no tools are on offer, so a provider that has
    /// never heard of tools sees byte-for-byte the request it always did.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool<'a>>,
    stream: bool,
    /// Providers withhold token counts from a stream unless asked. Ollama
    /// honours this too, so cost telemetry is not an OpenAI-only luxury.
    stream_options: StreamOptions,
}

/// One entry of the request's `tools` array: a declaration in the wire's
/// clothing.
#[derive(Debug, Serialize)]
struct Tool<'a> {
    #[serde(rename = "type")]
    kind: ToolKind,
    function: &'a ToolDeclaration,
}

impl<'a> Tool<'a> {
    const fn function(function: &'a ToolDeclaration) -> Self {
        Self {
            kind: ToolKind::Function,
            function,
        }
    }
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// What comes back, chunk by chunk — modelling only the fields Epik reads.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Chunk {
    /// Empty on the usage-only chunk that ends an Ollama stream.
    choices: Vec<Choice>,
    usage: Option<Usage>,
    /// Some providers report a mid-stream failure in-band rather than by
    /// breaking the connection.
    error: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Choice {
    delta: Delta,
    /// `"stop"` for a turn that ended in text, `"tool_calls"` for one that
    /// ended asking for tools.
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Delta {
    content: Option<String>,
    /// Tool calls arrive in fragments — an opening one carrying the id and
    /// the function name, then pieces of the argument string — all keyed to
    /// their call by `index`.
    tool_calls: Vec<CallFragment>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CallFragment {
    index: usize,
    id: Option<String>,
    function: FunctionFragment,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FunctionFragment {
    name: Option<String>,
    arguments: Option<String>,
}

/// The sentinel that ends a chat-completions stream.
const DONE: &str = "[DONE]";

/// What `finish_reason` says when the turn ended asking for tools.
const ASKED_FOR_TOOLS: &str = "tool_calls";

impl Chunk {
    /// Decodes one `data:` payload. `None` for the end-of-stream sentinel.
    ///
    /// # Errors
    ///
    /// Returns an error for a payload that is not a chunk at all, and for a
    /// chunk in which the provider reported a failure.
    fn decode(payload: &str) -> Result<Option<Self>> {
        if payload.trim() == DONE {
            return Ok(None);
        }
        let chunk: Self = serde_json::from_str(payload)
            .with_context(|| format!("decoding a chunk of the reply: {payload}"))?;
        if let Some(error) = &chunk.error {
            bail!("the provider reported an error mid-stream: {error}");
        }
        Ok(Some(chunk))
    }
}

/// Folds a stream of chunks into one [`Reply`].
///
/// Text is emitted as it arrives; tool calls are accumulated as fragments
/// and become calls only once a finish reason has said the turn ended asking
/// for them — so an interrupted or torn stream can never hand a tool half an
/// argument string.
#[derive(Debug, Default)]
struct Assembly {
    reply: Reply,
    calls: Vec<PendingCall>,
    asked_for_tools: bool,
}

/// One call under assembly, its fragments concatenated as they arrive.
#[derive(Debug, Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

impl PendingCall {
    fn absorb(&mut self, fragment: &CallFragment) {
        self.id.push_str(fragment.id.as_deref().unwrap_or_default());
        self.name
            .push_str(fragment.function.name.as_deref().unwrap_or_default());
        self.arguments
            .push_str(fragment.function.arguments.as_deref().unwrap_or_default());
    }

    fn into_call(self) -> ToolCall {
        ToolCall::new(self.id, self.name, self.arguments)
    }
}

impl Assembly {
    /// Folds one chunk in, emitting whatever text it adds as a delta and
    /// skipping the empty deltas that accompany a finish reason.
    fn absorb(&mut self, chunk: Chunk, log: &mut dyn Log<ChatEvent>) {
        if let Some(usage) = chunk.usage {
            self.reply.usage = Some(usage);
        }
        let mut text = String::new();
        for choice in chunk.choices {
            self.asked_for_tools |= choice.finish_reason.as_deref() == Some(ASKED_FOR_TOOLS);
            text.push_str(choice.delta.content.as_deref().unwrap_or_default());
            for fragment in &choice.delta.tool_calls {
                while self.calls.len() <= fragment.index {
                    self.calls.push(PendingCall::default());
                }
                self.calls[fragment.index].absorb(fragment);
            }
        }
        if !text.is_empty() {
            self.reply.text.push_str(&text);
            log.emit(ChatEvent::Delta { text });
        }
    }

    /// The assembled reply, with the calls finalized when the turn asked for
    /// them.
    fn finish(mut self) -> Reply {
        if self.asked_for_tools {
            self.reply.tool_calls = self.calls.into_iter().map(PendingCall::into_call).collect();
        }
        self.reply
    }
}

impl ChatModel for OpenAiCompatible {
    fn reply(
        &mut self,
        transcript: &[Message],
        log: &mut dyn Log<ChatEvent>,
        stop: &StopToken,
    ) -> Result<Reply> {
        let request = Request {
            model: &self.model,
            messages: transcript,
            tools: self.tools.iter().map(Tool::function).collect(),
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
        };

        let mut builder = self
            .agent
            .post(&self.endpoint)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream");
        if let Some(key) = &self.key {
            builder = builder.header("authorization", format!("Bearer {key}"));
        }
        let mut response = builder
            .send_json(&request)
            .with_context(|| format!("asking {} for a reply", self.endpoint))?;

        let status = response.status();
        if !status.is_success() {
            let complaint = response
                .body_mut()
                .read_to_string()
                .unwrap_or_else(|error| format!("(its body was unreadable: {error})"));
            bail!(
                "{} refused with {status}: {}",
                self.endpoint,
                complaint.trim()
            );
        }

        let mut assembly = Assembly::default();
        for event in sse::Events::new(BufReader::new(response.body_mut().as_reader())) {
            // Between events is where a stop is honoured: whatever arrived so
            // far stands, and the connection is dropped on the way out.
            if stop.is_stopped() {
                assembly.reply.interrupted = true;
                break;
            }
            let event = event.with_context(|| format!("reading {}'s reply", self.endpoint))?;
            let Some(chunk) = Chunk::decode(&event.data)? else {
                break;
            };
            assembly.absorb(chunk, log);
        }
        Ok(assembly.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::Silent;

    /// Two providers speaking the same protocol differently. See
    /// `fixtures/README.md` for where each came from and what each one is
    /// here to catch — briefly: Ollama tags every delta with a role, OpenAI
    /// sends a final delta with no `content` key at all, and both end with a
    /// usage-only chunk whose `choices` is empty.
    const OLLAMA: &str = include_str!("fixtures/ollama.sse");
    const OPENAI: &str = include_str!("fixtures/openai.sse");

    /// The same two providers asking for tools. OpenAI streams each call's
    /// argument string in fragments; Ollama sends every call whole in one
    /// delta.
    const OLLAMA_TOOLS: &str = include_str!("fixtures/ollama-tools.sse");
    const OPENAI_TOOLS: &str = include_str!("fixtures/openai-tools.sse");

    /// Replays a captured stream through the real decoding path.
    fn replay(stream: &str) -> Result<Reply> {
        let mut assembly = Assembly::default();
        let mut log = Vec::new();
        for event in sse::Events::new(stream.as_bytes()) {
            let Some(chunk) = Chunk::decode(&event?.data)? else {
                break;
            };
            assembly.absorb(chunk, &mut log);
        }
        let reply = assembly.finish();
        // Every delta emitted must add up to the assembled reply, whichever
        // provider produced it.
        let emitted: String = log
            .iter()
            .map(|event| match event {
                ChatEvent::Delta { text } => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(emitted, reply.text);
        Ok(reply)
    }

    #[test]
    fn an_ollama_stream_decodes() {
        let reply = replay(OLLAMA).unwrap();
        assert_eq!(reply.text, "Hello! How can I");
        assert_eq!(reply.usage, Some(Usage::tokens(20, 5)));
        assert!(
            reply.tool_calls.is_empty(),
            "a stream with no tool traffic decodes exactly as before"
        );
    }

    #[test]
    fn an_openai_stream_decodes() {
        let reply = replay(OPENAI).unwrap();
        assert_eq!(reply.text, "Hello, I'm Epik.");
        assert_eq!(reply.usage, Some(Usage::tokens(18, 6)));
        assert!(
            reply.tool_calls.is_empty(),
            "a stream with no tool traffic decodes exactly as before"
        );
    }

    #[test]
    fn an_openai_tool_call_stream_assembles_its_fragments_into_calls() {
        let reply = replay(OPENAI_TOOLS).unwrap();
        assert!(reply.text.is_empty(), "this turn ended asking, not saying");
        assert_eq!(
            reply.tool_calls,
            [
                ToolCall::new("call_ep1k0001", "get_weather", r#"{"city":"Paris"}"#),
                ToolCall::new("call_ep1k0002", "get_time", r#"{"zone":"CET"}"#),
            ],
            "argument fragments concatenate back into the JSON the model wrote"
        );
        for call in &reply.tool_calls {
            serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                .expect("the reassembled arguments are intact JSON");
        }
        assert_eq!(reply.usage, Some(Usage::tokens(61, 42)));
    }

    #[test]
    fn an_ollama_tool_call_stream_decodes() {
        let reply = replay(OLLAMA_TOOLS).unwrap();
        assert!(reply.text.is_empty());
        assert_eq!(
            reply.tool_calls,
            [ToolCall::new(
                "call_z7ymrlq0",
                "get_weather",
                r#"{"city":"Paris"}"#
            )],
            "a call that arrives whole in one delta assembles just the same"
        );
    }

    #[test]
    fn a_mixed_stream_keeps_both_the_text_and_the_calls() {
        let stream = "data: {\"choices\":[{\"index\":0,\"delta\":\
                      {\"content\":\"Checking.\"},\"finish_reason\":null}]}\n\n\
                      data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":\
                      [{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\
                      \"function\":{\"name\":\"look\",\"arguments\":\"{}\"}}]},\
                      \"finish_reason\":null}]}\n\n\
                      data: {\"choices\":[{\"index\":0,\"delta\":{},\
                      \"finish_reason\":\"tool_calls\"}]}\n\n\
                      data: [DONE]\n\n";
        let reply = replay(stream).unwrap();
        assert_eq!(reply.text, "Checking.");
        assert_eq!(reply.tool_calls, [ToolCall::new("call_1", "look", "{}")]);
    }

    #[test]
    fn a_stream_cut_off_mid_call_surrenders_no_torn_arguments() {
        // The finish reason never arrives, so the fragments never become
        // calls: half a JSON string is not something to hand a tool.
        let stream = "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":\
                      [{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\
                      \"function\":{\"name\":\"look\",\"arguments\":\"{\\\"ci\"}}]},\
                      \"finish_reason\":null}]}\n\n";
        let reply = replay(stream).unwrap();
        assert!(reply.tool_calls.is_empty());
    }

    #[test]
    fn a_provider_that_reports_a_mid_stream_error_fails_the_turn() {
        let stream = "data: {\"error\":{\"message\":\"context length exceeded\"}}\n\n";
        let error = replay(stream).expect_err("an in-band error ends the turn");
        assert!(error.to_string().contains("context length exceeded"));
    }

    #[test]
    fn a_chunk_carrying_fields_this_client_never_heard_of_still_decodes() {
        let stream = "data: {\"choices\":[{\"index\":0,\"logprobs\":null,\
                      \"delta\":{\"content\":\"ok\",\"reasoning\":\"thinking\"}}],\
                      \"a_field_from_next_year\":true}\n\n";
        assert_eq!(replay(stream).unwrap().text, "ok");
    }

    #[test]
    fn a_request_offering_no_tools_is_byte_for_byte_what_it_always_was() {
        let request = Request {
            model: "smollm2:135m",
            messages: &[Message::user("Hi")],
            tools: Vec::new(),
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"model":"smollm2:135m","messages":[{"role":"user","content":"Hi"}],"stream":true,"stream_options":{"include_usage":true}}"#,
            "with no tools registered, today's providers see today's request"
        );
    }

    #[test]
    fn a_request_offering_tools_declares_each_as_a_function() {
        let weather = ToolDeclaration {
            name: "get_weather".to_owned(),
            description: "Today's weather in a city.".to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"],
            }),
        };
        let request = Request {
            model: "gpt-4o-mini",
            messages: &[],
            tools: vec![Tool::function(&weather)],
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            // The schema's keys arrive alphabetized: an opaque `Value` keeps
            // its entries in map order, and that is fine — JSON objects are
            // unordered, and only `Request`'s own field order is promised.
            r#"{"model":"gpt-4o-mini","messages":[],"tools":[{"type":"function","function":{"name":"get_weather","description":"Today's weather in a city.","parameters":{"properties":{"city":{"type":"string"}},"required":["city"],"type":"object"}}}],"stream":true,"stream_options":{"include_usage":true}}"#
        );
    }

    #[test]
    fn a_transcript_with_tool_traffic_serializes_as_a_provider_expects() {
        let transcript = [
            Message::tool_calls(
                "",
                vec![ToolCall::new(
                    "call_1",
                    "get_weather",
                    r#"{"city":"Paris"}"#,
                )],
            ),
            Message::tool_result("call_1", "12°C, clearing"),
        ];
        assert_eq!(
            serde_json::to_string(&transcript).unwrap(),
            r#"[{"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Paris\"}"}}]},{"role":"tool","tool_call_id":"call_1","content":"12°C, clearing"}]"#,
            "the ask and the answer round through the transcript, keyed by call id"
        );
    }

    #[test]
    fn the_base_url_keeps_exactly_one_slash_before_the_path() {
        for base_url in ["http://localhost:11434/v1", "http://localhost:11434/v1/"] {
            let provider = Provider {
                base_url: base_url.to_owned(),
                model: "smollm2:135m".to_owned(),
            };
            assert_eq!(
                OpenAiCompatible::new(&provider, None).endpoint(),
                "http://localhost:11434/v1/chat/completions"
            );
        }
    }

    /// The one test here that touches the network, and only to be refused by
    /// the loopback interface — which is what "the network went away" looks
    /// like from inside the client.
    #[test]
    fn an_unreachable_endpoint_says_where_it_tried() {
        let provider = Provider {
            // Port 1 is reserved and nothing may listen there.
            base_url: "http://127.0.0.1:1/v1".to_owned(),
            model: "nothing-is-listening".to_owned(),
        };
        let mut model = OpenAiCompatible::new(&provider, None);

        let error = model
            .reply(&[Message::user("Hi")], &mut Silent, &StopToken::new())
            .expect_err("there is no server to answer");

        assert!(
            format!("{error:#}").contains("http://127.0.0.1:1/v1/chat/completions"),
            "a failure should name the endpoint it was reaching for: {error:#}"
        );
    }
}
