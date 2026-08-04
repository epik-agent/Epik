//! The live tier: a real model, over the real wire, for free.
//!
//! On a laptop this skips when nothing is listening — needing Ollama running
//! to run the test suite would be a tax on every other test. In CI that same
//! rule would produce a tier that looks green forever without ever having
//! executed, so `EPIK_REQUIRE_LIVE=1` turns a missing server into a failure.
//!
//! What is asserted is protocol, never prose: that a stream started, that
//! deltas arrived, that the turn finalized, that the transcript grew. A
//! sub-1B model is entirely adequate for that, and answer quality is not this
//! suite's business.

// Tests are entitled to panic. The allow-unwrap-in-tests clippy setting only
// covers #[test] functions, not the helpers here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::env;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use epik::chat::{ChatModel, Conversation, Message, OpenAiCompatible, Role, StopToken};
use epik::config::Provider;
use epik::event::ChatEvent;

/// Where the live model is, and which one it is. Defaults describe a local
/// Ollama; CI overrides neither, because CI installs exactly this.
const BASE_URL_ENV: &str = "EPIK_LIVE_BASE_URL";
const MODEL_ENV: &str = "EPIK_LIVE_MODEL";
const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";
const DEFAULT_MODEL: &str = "smollm2:135m";

/// Set this and a missing server stops being a reason to skip.
const REQUIRE_ENV: &str = "EPIK_REQUIRE_LIVE";

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The provider to talk to, or `None` when this tier is skipping.
///
/// # Panics
///
/// Panics when nothing is listening and `EPIK_REQUIRE_LIVE` says something
/// should be.
fn live_provider() -> Option<Provider> {
    let provider = Provider {
        base_url: env::var(BASE_URL_ENV).unwrap_or_else(|_| DEFAULT_BASE_URL.to_owned()),
        model: env::var(MODEL_ENV).unwrap_or_else(|_| DEFAULT_MODEL.to_owned()),
    };

    if listening(&provider.base_url) {
        return Some(provider);
    }
    assert!(
        env::var(REQUIRE_ENV).unwrap_or_default() != "1",
        "{REQUIRE_ENV}=1, but nothing is listening at {}. Start a model \
         server, or unset {REQUIRE_ENV} to let this tier skip.",
        provider.base_url
    );
    eprintln!(
        "skipping the live tier: nothing is listening at {}. \
         Start one with `ollama serve`, or set {REQUIRE_ENV}=1 to make this a failure.",
        provider.base_url
    );
    None
}

/// Whether anything holds the port `base_url` points at. "Is a server
/// listening" is the actual question, so ask TCP rather than HTTP.
fn listening(base_url: &str) -> bool {
    let (scheme, rest) = base_url.split_once("://").unwrap_or(("http", base_url));
    let authority = rest.split('/').next().unwrap_or(rest);
    let address = if authority.contains(':') {
        authority.to_owned()
    } else if scheme == "https" {
        format!("{authority}:443")
    } else {
        format!("{authority}:80")
    };

    address
        .to_socket_addrs()
        .into_iter()
        .flatten()
        .any(|address| TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok())
}

fn deltas(events: &[ChatEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            ChatEvent::Delta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_live_model_streams_a_reply_into_the_transcript() {
    let Some(provider) = live_provider() else {
        return;
    };
    let mut chat = Conversation::new(
        "You are Epik. Answer in one short sentence.",
        OpenAiCompatible::new(&provider, None),
    );
    let mut events: Vec<ChatEvent> = Vec::new();

    let reply = chat
        .send("Hi", &mut events, &StopToken::new())
        .expect("a listening model answers");

    let deltas = deltas(&events);
    assert!(
        !deltas.is_empty(),
        "the stream produced no deltas: {events:?}"
    );
    assert_eq!(
        deltas.concat(),
        reply.text,
        "the assembled reply must be exactly what was streamed"
    );
    assert!(!reply.text.trim().is_empty(), "the model said nothing");
    assert!(!reply.interrupted, "nothing asked this turn to stop");
    assert!(
        matches!(events.last(), Some(ChatEvent::TurnFinished { .. })),
        "a turn finishes with TurnFinished: {events:?}"
    );

    assert_eq!(
        chat.messages().len(),
        2,
        "the transcript grew by the question and the answer: {:?}",
        chat.messages()
    );
    assert_eq!(chat.messages()[0], Message::user("Hi"));
    assert_eq!(chat.messages()[1].role, Role::Assistant);
    assert_eq!(chat.messages()[1].content, reply.text);
}

#[test]
fn a_live_model_reports_what_the_turn_cost() {
    let Some(provider) = live_provider() else {
        return;
    };
    let mut chat = Conversation::new("You are Epik.", OpenAiCompatible::new(&provider, None));
    let mut events: Vec<ChatEvent> = Vec::new();

    let reply = chat.send("Hi", &mut events, &StopToken::new()).unwrap();

    let usage = reply
        .usage
        .expect("the client asks for usage, and this provider reports it");
    assert!(usage.prompt_tokens > 0, "{usage:?}");
    assert!(usage.completion_tokens > 0, "{usage:?}");
    assert_eq!(
        events.last(),
        Some(&ChatEvent::TurnFinished { usage: Some(usage) }),
        "what the turn cost travels on the event that finishes it"
    );
}

#[test]
fn a_stopped_live_turn_finalizes_rather_than_hanging() {
    let Some(provider) = live_provider() else {
        return;
    };
    // Already stopped: the client reads the first event off the wire, sees the
    // token, and winds up. Deterministic, unlike stopping partway through a
    // reply whose length nobody promised.
    let stop = StopToken::new();
    stop.stop();
    let mut model = OpenAiCompatible::new(&provider, None);
    let mut events: Vec<ChatEvent> = Vec::new();

    let reply = model
        .reply(
            &[Message::user("Count to five hundred.")],
            &mut events,
            &stop,
        )
        .expect("an interruption is not an error");

    assert!(reply.interrupted);
    assert!(reply.text.is_empty(), "nothing was accepted after the stop");
    assert!(events.is_empty(), "no deltas were emitted: {events:?}");
}
