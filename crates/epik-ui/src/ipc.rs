//! Reading the host's event stream.
//!
//! The host emits library events as JSON; the frontend decodes them into the
//! library's own types. Parsing here is deliberately unknown-tolerant at the
//! edges: a frontend that hard-fails on a vocabulary it does not recognize
//! turns every library addition into a breaking change.
//!
//! The names on the wire are not this crate's to choose — they come from
//! [`epik::ipc`], which both ends read — so there is nothing here but decoding.

use epik::event::{ChatEvent, Envelope, Event};

/// Decodes one implementation-run event. `None` for anything this build does
/// not understand — a newer host is not an error, just quieter.
#[must_use]
pub fn run_event(payload: &serde_json::Value) -> Option<Event> {
    serde_json::from_value(payload.clone()).ok()
}

/// Decodes one enveloped chat event.
///
/// `None` for a vocabulary or a variant this build does not know. The envelope
/// itself is required, though: an event with no conversation on it is not
/// something to guess at, since that field exists precisely so nobody has to.
#[must_use]
pub fn chat_event(payload: &serde_json::Value) -> Option<Envelope<ChatEvent>> {
    serde_json::from_value(payload.clone()).ok()
}

#[cfg(test)]
mod tests {
    use epik::event::ConversationId;

    use super::*;

    #[test]
    fn library_events_decode_into_library_types() {
        let payload = serde_json::json!({ "IssueStarted": { "id": 7 } });
        assert_eq!(run_event(&payload), Some(Event::IssueStarted { id: 7 }));
    }

    #[test]
    fn an_unrecognized_event_is_ignored_rather_than_fatal() {
        let payload = serde_json::json!({ "SomethingNewer": { "id": 7 } });
        assert_eq!(run_event(&payload), None);
    }

    #[test]
    fn an_enveloped_delta_decodes_with_its_conversation() {
        let payload = serde_json::json!({
            "conversation": 0,
            "event": { "Delta": { "text": "Hello" } },
        });

        assert_eq!(
            chat_event(&payload),
            Some(Envelope {
                conversation: ConversationId(0),
                event: ChatEvent::Delta {
                    text: "Hello".to_owned()
                },
            })
        );
    }

    #[test]
    fn a_finished_turn_decodes_with_the_usage_the_provider_reported() {
        let payload = serde_json::json!({
            "conversation": 0,
            "event": {
                "TurnFinished": { "usage": { "prompt_tokens": 18, "completion_tokens": 6 } }
            },
        });

        assert!(matches!(
            chat_event(&payload).map(|envelope| envelope.event),
            Some(ChatEvent::TurnFinished { usage: Some(_) })
        ));
    }

    #[test]
    fn a_chat_event_this_build_never_heard_of_is_ignored_rather_than_fatal() {
        let payload = serde_json::json!({
            "conversation": 0,
            "event": { "ToolCalled": { "name": "next-year" } },
        });
        assert_eq!(chat_event(&payload), None);
    }

    #[test]
    fn an_event_with_no_envelope_is_not_guessed_at() {
        let payload = serde_json::json!({ "Delta": { "text": "Hello" } });
        assert_eq!(
            chat_event(&payload),
            None,
            "the conversation field exists so that nobody has to assume one"
        );
    }
}
