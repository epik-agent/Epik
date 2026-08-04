//! Reading the host's event stream.
//!
//! The host emits library events as JSON; the frontend decodes them into the
//! library's own types. Parsing here is deliberately unknown-tolerant at the
//! edges: a frontend that hard-fails on a vocabulary it does not recognize
//! turns every library addition into a breaking change.

use epik::event::Event;

/// Decodes one implementation-run event. `None` for anything this build does
/// not understand — a newer host is not an error, just quieter.
#[must_use]
pub fn run_event(payload: &serde_json::Value) -> Option<Event> {
    serde_json::from_value(payload.clone()).ok()
}

#[cfg(test)]
mod tests {
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
}
