//! The scripted engine's protocol: one JSON event per line of stdout,
//! written by the script and read back into [`Update`]s. The same
//! [`Event`] serializes into the script and deserializes out of the
//! stream, so the two cannot drift.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::Update;

/// One event, as the script says one per line.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Event {
    /// The run began, under this session id.
    Started { session: String },
    /// One file written, whole, at this path relative to the working
    /// directory.
    Wrote { path: PathBuf },
    /// The run ended, every one of `files` written.
    Stopped { files: usize },
}

/// Reads one line into the [`Update`] it carries: a started event
/// opens the session, a write is a use of the `Write` tool, and the
/// stopped event is the result. A line that is not an event is
/// nothing.
pub(super) fn interpret(line: &str) -> Vec<Update> {
    let Ok(event) = serde_json::from_str::<Event>(line) else {
        return Vec::new();
    };
    vec![match event {
        Event::Started { session } => Update::Session {
            id: session,
            model: "scripted".to_owned(),
        },
        Event::Wrote { .. } => Update::ToolUse {
            name: "Write".to_owned(),
        },
        Event::Stopped { files } => Update::Result {
            ok: true,
            text: format!("wrote {files} files"),
            cost_usd: None,
            input_tokens: None,
            output_tokens: None,
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_round_trips_through_its_line() {
        for event in [
            Event::Started {
                session: "s-1".to_owned(),
            },
            Event::Wrote {
                path: "src/adder/__init__.py".into(),
            },
            Event::Stopped { files: 4 },
        ] {
            let line = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<Event>(&line).unwrap(), event);
        }
    }

    #[test]
    fn lines_that_are_not_events_interpret_to_nothing() {
        for line in [
            "",
            "not json",
            r#"{"type":"celebration"}"#,
            r#"{"path":"x"}"#,
        ] {
            assert_eq!(interpret(line), [], "{line:?}");
        }
    }
}
