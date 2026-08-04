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
}
