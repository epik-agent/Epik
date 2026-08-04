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
