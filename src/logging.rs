use std::io::Write;
use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};

/// One observable moment in an implementation run.
///
/// Events cross process boundaries — remote daemon clients, the Tauri shell —
/// so they are serde-serializable from the start.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Event {
    IssueStarted { id: u32 },
    IssueImplemented { id: u32 },
}

/// Where implementation progress goes. Emitting is infallible by design:
/// a broken log must never abort an implementation run.
pub trait Log {
    fn emit(&mut self, event: Event);
}

/// The simplest log ignores everything.
#[derive(Debug)]
pub struct Silent;

impl Log for Silent {
    fn emit(&mut self, _: Event) {}
}

/// Sending into a channel is logging; the receiver — on another thread, or
/// pumping a process boundary — decides what the events mean.
impl Log for Sender<Event> {
    fn emit(&mut self, event: Event) {
        // A gone receiver is not this sender's problem.
        let _ = self.send(event);
    }
}

/// Writes each event as one JSON line: the wire format for logs that cross
/// a process boundary.
#[derive(Debug)]
pub struct JsonLines<W: Write>(W);

impl<W: Write> JsonLines<W> {
    pub const fn new(writer: W) -> Self {
        Self(writer)
    }
}

impl<W: Write> Log for JsonLines<W> {
    fn emit(&mut self, event: Event) {
        // Emitting is infallible: an event that can't be written is dropped.
        if let Ok(json) = serde_json::to_string(&event) {
            let _ = writeln!(self.0, "{json}");
            let _ = self.0.flush();
        }
    }
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
