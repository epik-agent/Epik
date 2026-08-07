//! Event vocabularies: what the library says about what it is doing.
//!
//! Events cross process and language boundaries — the remote daemon's
//! clients, the Tauri host, the wasm frontend — so they are serde types and
//! nothing else. This module holds no I/O and no threads, which is what
//! keeps it compiling for `wasm32-unknown-unknown`, where the frontend
//! deserializes these very types. The sinks that events flow into live in
//! [`crate::logging`].

use serde::{Deserialize, Serialize};

/// One observable moment in an implementation run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Event {
    IssueStarted { id: u32 },
    IssueImplemented { id: u32 },
}

/// One observable moment in an assistant turn.
///
/// A turn is exactly one `TurnFinished` or one `Failed`, preceded by zero or
/// more `Delta`s — and, when the model works through tools, by a
/// `ToolCallStarted` answered by a `ToolCallFinished` or a `ToolCallRefused`
/// for each call, so a window renders progress rather than silence. `Failed`
/// is how a turn that broke mid-stream is reported; it is not how a refused
/// request is reported, because a refusal never started a turn. Keeping
/// those two apart is what lets a host put command errors and streamed
/// failures on different channels.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ChatEvent {
    /// The next piece of the assistant's reply, as it generates.
    Delta { text: String },
    /// The reply is complete and has been written into the transcript.
    /// `usage` is present only when the provider reported it.
    TurnFinished { usage: Option<Usage> },
    /// The turn did not complete. Whatever deltas already arrived stand.
    Failed { error: String },
    /// The assistant asked for a tool, and the registry is running the call.
    ToolCallStarted {
        id: String,
        name: String,
        arguments: String,
    },
    /// The call answered. `content` is exactly what goes back to the model.
    ToolCallFinished { id: String, content: String },
    /// The call refused — an unknown tool, arguments that do not fit, or the
    /// tool's own failure — rendered from the typed error. The model reads
    /// the same words as a tool result, which is how it gets to try again.
    /// `remedy` names what the user could do about it, when the loop can
    /// tell — the typed case beside the rendered words, so a window acts on
    /// a value rather than scraping the prose.
    ToolCallRefused {
        id: String,
        error: String,
        remedy: Option<Remedy>,
    },
}

/// What the user could do about a refusal, when the library knows.
///
/// The model already gets the refusal's words as a tool result; this is the
/// same fact in the shape a window can act on. One case so far: a GitHub
/// verb refused for want of a token, which the window answers with the
/// paste-your-PAT card.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Remedy {
    /// Supply a GitHub token: paste a PAT, or set `EPIK_GITHUB_TOKEN`.
    GithubToken,
}

/// What some work cost, as the provider counted it.
///
/// One vocabulary serves chat turns and agent runs alike: tokens are the base
/// unit, always present, with cache reads distinguished when the provider
/// distinguishes them. Money appears only when it was *reported* — a price
/// table maintained in Epik would be a lie waiting to go stale, so a provider
/// that names no price yields `None`, honestly. In an agent run the readings
/// are cumulative and monotone nondecreasing — a conformance assertion — with
/// the last reading before `Finished` authoritative.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// Tokens served from cache, when the provider tells them apart. Absent
    /// from the wire when there is nothing to say, so the shape providers
    /// already speak is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_tokens: Option<u32>,
    /// What the provider says the work cost. Reported, never synthesized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Money>,
}

impl Usage {
    /// Token counts alone — the fields every provider reports.
    #[must_use]
    pub const fn tokens(prompt: u32, completion: u32) -> Self {
        Self {
            prompt_tokens: prompt,
            completion_tokens: completion,
            cache_tokens: None,
            cost: None,
        }
    }

    /// Every token in one number: what a ceiling denominated in tokens is
    /// compared against.
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        u64::from(self.prompt_tokens)
            + u64::from(self.completion_tokens)
            + u64::from(self.cache_tokens.unwrap_or(0))
    }

    /// The componentwise sum: how a turn that made several requests bills for
    /// all of them. A count reported by either side is kept; only two absent
    /// counts stay absent, so summing never invents a zero reading.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            prompt_tokens: self.prompt_tokens + other.prompt_tokens,
            completion_tokens: self.completion_tokens + other.completion_tokens,
            cache_tokens: join(self.cache_tokens, other.cache_tokens, |a, b| a + b),
            cost: join(self.cost, other.cost, |a, b| Money {
                micro_usd: a.micro_usd + b.micro_usd,
            }),
        }
    }

    /// The componentwise maximum: how a wrapper keeps its cumulative totals
    /// monotone when the process under it claims a total lower than one it
    /// already claimed. Totals never run backward, whatever the feed says.
    #[must_use]
    pub fn max(self, other: Self) -> Self {
        Self {
            prompt_tokens: self.prompt_tokens.max(other.prompt_tokens),
            completion_tokens: self.completion_tokens.max(other.completion_tokens),
            cache_tokens: join(self.cache_tokens, other.cache_tokens, u32::max),
            cost: join(self.cost, other.cost, Money::max),
        }
    }
}

/// Combines two optional readings: both present combine, one present stands,
/// none stays none.
fn join<T>(a: Option<T>, b: Option<T>, both: impl FnOnce(T, T) -> T) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(both(a, b)),
        (a, b) => a.or(b),
    }
}

/// Money as a provider reported it: millionths of a US dollar, fixed-point.
///
/// Fixed-point because billing arithmetic in `f64` accumulates dust nobody
/// can audit; US dollars because that is the only currency any wrapped
/// provider reports in. A `Money` exists only where a cost was actually
/// stated — see [`Usage::cost`].
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Money {
    /// Millionths of a dollar: $3.50 is `3_500_000`.
    pub micro_usd: u64,
}

/// Which conversation something happened in.
///
/// A host's registry assigns these; a `Conversation` never learns its own. The
/// v0 window ignores the field, because only one conversation exists — and
/// that is exactly the point. One field is the entire architectural cost of
/// leaving multi-conversation UX open, whereas retrofitting an envelope onto a
/// protocol already in use is the expensive version of the same decision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ConversationId(pub u64);

/// An event, and where it came from.
///
/// Generic over the vocabulary, so the run cockpit's events will travel in
/// this same envelope down this same channel rather than needing one of their
/// own. The sink that does the wrapping is
/// [`Enveloping`](crate::logging::Enveloping).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Envelope<E> {
    pub conversation: ConversationId,
    pub event: E,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_through_json() {
        let event = Event::IssueStarted { id: 7 };
        let json = serde_json::to_string(&event).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn a_refusal_carries_its_remedy_through_json() {
        let event = ChatEvent::ToolCallRefused {
            id: "call-1".to_owned(),
            error: "no GitHub token".to_owned(),
            remedy: Some(Remedy::GithubToken),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: ChatEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back, "the typed case survives the boundary");
    }

    #[test]
    fn usage_on_the_wire_is_unchanged_until_there_is_something_new_to_say() {
        // What every provider already sends still deserializes ...
        let old: Usage = serde_json::from_str(r#"{"prompt_tokens":18,"completion_tokens":6}"#)
            .expect("yesterday's wire shape still reads");
        assert_eq!(old, Usage::tokens(18, 6));
        // ... and a count with nothing new to say serializes without the new
        // keys, so yesterday's readers keep reading.
        assert_eq!(
            serde_json::to_string(&old).unwrap(),
            r#"{"prompt_tokens":18,"completion_tokens":6}"#
        );
    }

    #[test]
    fn a_reported_cost_rides_along_as_fixed_point() {
        let usage = Usage {
            cache_tokens: Some(4),
            cost: Some(Money {
                micro_usd: 3_500_000,
            }),
            ..Usage::tokens(18, 6)
        };
        let json = serde_json::to_string(&usage).unwrap();
        assert!(json.contains(r#""micro_usd":3500000"#), "{json}");
        let back: Usage = serde_json::from_str(&json).unwrap();
        assert_eq!(usage, back);
    }

    #[test]
    fn an_envelope_round_trips_with_its_conversation_intact() {
        let envelope = Envelope {
            conversation: ConversationId(3),
            event: ChatEvent::Delta {
                text: "Hello".to_owned(),
            },
        };
        let json = serde_json::to_string(&envelope).unwrap();
        let back: Envelope<ChatEvent> = serde_json::from_str(&json).unwrap();
        assert_eq!(envelope, back);
    }

    #[test]
    fn one_envelope_carries_either_vocabulary() {
        let run = Envelope {
            conversation: ConversationId(0),
            event: Event::IssueStarted { id: 7 },
        };
        let chat = Envelope {
            conversation: ConversationId(0),
            event: ChatEvent::TurnFinished { usage: None },
        };
        // Same wrapper, same field name: a second vocabulary on this channel
        // costs a client nothing but a second match arm.
        assert!(
            serde_json::to_string(&run)
                .unwrap()
                .contains("conversation")
        );
        assert!(
            serde_json::to_string(&chat)
                .unwrap()
                .contains("conversation")
        );
    }
}
