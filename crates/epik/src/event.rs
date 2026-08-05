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
/// more `Delta`s. `Failed` is how a turn that broke mid-stream is reported;
/// it is not how a refused request is reported, because a refusal never
/// started a turn. Keeping those two apart is what lets a host put command
/// errors and streamed failures on different channels.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ChatEvent {
    /// The next piece of the assistant's reply, as it generates.
    Delta { text: String },
    /// The reply is complete and has been written into the transcript.
    /// `usage` is present only when the provider reported it.
    TurnFinished { usage: Option<Usage> },
    /// The turn did not complete. Whatever deltas already arrived stand.
    Failed { error: String },
}

/// What a turn cost, as the provider counted it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
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
