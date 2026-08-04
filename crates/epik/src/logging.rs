//! Where events go. One sink implementation serves every vocabulary in
//! [`crate::event`]: `Log` is generic over the event type, so a new
//! vocabulary costs no new plumbing.

use std::io::Write;
use std::sync::mpsc::Sender;

use serde::Serialize;

/// Somewhere events of type `E` can be sent. Emitting is infallible by
/// design: a broken log must never abort the work it is describing.
pub trait Log<E> {
    fn emit(&mut self, event: E);
}

/// The simplest log ignores everything, whatever it is.
#[derive(Debug)]
pub struct Silent;

impl<E> Log<E> for Silent {
    fn emit(&mut self, _: E) {}
}

/// Sending into a channel is logging; the receiver — on another thread, or
/// pumping a process boundary — decides what the events mean.
impl<E> Log<E> for Sender<E> {
    fn emit(&mut self, event: E) {
        // A gone receiver is not this sender's problem.
        let _ = self.send(event);
    }
}

/// Collecting is logging too: the sink for anything that wants the whole
/// stream at the end rather than each event as it happens, which is mostly
/// tests and mostly assertions about order.
impl<E> Log<E> for Vec<E> {
    fn emit(&mut self, event: E) {
        self.push(event);
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

impl<E: Serialize, W: Write> Log<E> for JsonLines<W> {
    fn emit(&mut self, event: E) {
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
    use crate::event::Event;

    /// A second vocabulary, standing in for the ones to come: the same
    /// `Sender` and `JsonLines` have to serve it with no new code.
    #[derive(Debug, Serialize)]
    enum Noise {
        Hum,
    }

    #[test]
    fn one_json_lines_serves_every_vocabulary() {
        let mut buffer = Vec::new();
        {
            let mut log = JsonLines::new(&mut buffer);
            Log::emit(&mut log, Event::IssueStarted { id: 1 });
            Log::emit(&mut log, Noise::Hum);
        }
        assert_eq!(
            String::from_utf8(buffer).unwrap(),
            "{\"IssueStarted\":{\"id\":1}}\n\"Hum\"\n"
        );
    }

    #[test]
    fn silent_swallows_every_vocabulary() {
        Log::emit(&mut Silent, Event::IssueStarted { id: 1 });
        Log::emit(&mut Silent, Noise::Hum);
    }
}
