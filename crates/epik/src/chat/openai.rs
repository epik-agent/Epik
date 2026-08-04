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

use crate::chat::{ChatModel, Message, Reply, StopToken, sse};
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
            agent,
        }
    }

    /// The endpoint this client posts to — useful in an error message, and in
    /// a test that wants to know where a request would have gone.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

/// What goes out. The transcript is resent every turn: the protocol is
/// stateless, so the caller's history *is* the model's memory.
#[derive(Debug, Serialize)]
struct Request<'a> {
    model: &'a str,
    messages: &'a [Message],
    stream: bool,
    /// Providers withhold token counts from a stream unless asked. Ollama
    /// honours this too, so cost telemetry is not an OpenAI-only luxury.
    stream_options: StreamOptions,
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
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Delta {
    content: Option<String>,
}

/// The sentinel that ends a chat-completions stream.
const DONE: &str = "[DONE]";

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

    /// The text this chunk adds to the reply, skipping the empty deltas that
    /// accompany a finish reason.
    fn text(&self) -> String {
        self.choices
            .iter()
            .filter_map(|choice| choice.delta.content.as_deref())
            .collect()
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

        let mut reply = Reply::default();
        for event in sse::Events::new(BufReader::new(response.body_mut().as_reader())) {
            // Between events is where a stop is honoured: whatever arrived so
            // far stands, and the connection is dropped on the way out.
            if stop.is_stopped() {
                reply.interrupted = true;
                break;
            }
            let event = event.with_context(|| format!("reading {}'s reply", self.endpoint))?;
            let Some(chunk) = Chunk::decode(&event.data)? else {
                break;
            };
            if let Some(usage) = chunk.usage {
                reply.usage = Some(usage);
            }
            let text = chunk.text();
            if !text.is_empty() {
                reply.text.push_str(&text);
                log.emit(ChatEvent::Delta { text });
            }
        }
        Ok(reply)
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

    /// Replays a captured stream through the real decoding path.
    fn replay(stream: &str) -> Result<Reply> {
        let mut reply = Reply::default();
        let mut log = Vec::new();
        for event in sse::Events::new(stream.as_bytes()) {
            let Some(chunk) = Chunk::decode(&event?.data)? else {
                break;
            };
            if let Some(usage) = chunk.usage {
                reply.usage = Some(usage);
            }
            let text = chunk.text();
            if !text.is_empty() {
                reply.text.push_str(&text);
                Log::emit(&mut log, ChatEvent::Delta { text });
            }
        }
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
        assert_eq!(
            reply.usage,
            Some(Usage {
                prompt_tokens: 20,
                completion_tokens: 5,
            })
        );
    }

    #[test]
    fn an_openai_stream_decodes() {
        let reply = replay(OPENAI).unwrap();
        assert_eq!(reply.text, "Hello, I'm Epik.");
        assert_eq!(
            reply.usage,
            Some(Usage {
                prompt_tokens: 18,
                completion_tokens: 6,
            })
        );
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
