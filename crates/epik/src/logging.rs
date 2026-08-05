//! Where events go. One sink implementation serves every vocabulary in
//! [`crate::event`]: `Log` is generic over the event type, so a new
//! vocabulary costs no new plumbing.

use std::io::Write;
use std::sync::mpsc::Sender;

use serde::Serialize;

use crate::event::{ConversationId, Envelope};

/// Somewhere events of type `E` can be sent. Emitting is infallible by
/// design: a broken log must never abort the work it is describing.
pub trait Log<E> {
    fn emit(&mut self, event: E);
}

/// A borrowed log is a log. That is what lets a sink be lent to something that
/// wants one of its own — [`Enveloping`] wrapping a log it does not own, say.
impl<E, L: Log<E> + ?Sized> Log<E> for &mut L {
    fn emit(&mut self, event: E) {
        (**self).emit(event);
    }
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

/// Puts every event into an [`Envelope`] on its way to `inner`: the sink an
/// IPC layer logs through.
///
/// The work being observed emits plain events and does not know which
/// conversation it is; the host, which assigned the id, is what says so. Being
/// generic over the vocabulary, this serves the run cockpit's events as
/// readily as chat's.
#[derive(Debug)]
pub struct Enveloping<L> {
    conversation: ConversationId,
    inner: L,
}

impl<L> Enveloping<L> {
    pub const fn new(conversation: ConversationId, inner: L) -> Self {
        Self {
            conversation,
            inner,
        }
    }
}

impl<E, L: Log<Envelope<E>>> Log<E> for Enveloping<L> {
    fn emit(&mut self, event: E) {
        self.inner.emit(Envelope {
            conversation: self.conversation,
            event,
        });
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

    #[test]
    fn enveloping_stamps_the_conversation_onto_every_event() {
        let mut collected: Vec<Envelope<Event>> = Vec::new();
        {
            let mut log = Enveloping::new(ConversationId(7), &mut collected);
            log.emit(Event::IssueStarted { id: 1 });
            log.emit(Event::IssueImplemented { id: 1 });
        }
        assert_eq!(
            collected,
            [
                Envelope {
                    conversation: ConversationId(7),
                    event: Event::IssueStarted { id: 1 }
                },
                Envelope {
                    conversation: ConversationId(7),
                    event: Event::IssueImplemented { id: 1 }
                },
            ]
        );
    }

    #[test]
    fn what_emits_plain_events_needs_no_knowledge_of_the_envelope() {
        // A `Log<Event>` is all the work being observed ever sees, so it can
        // be handed an enveloping sink without noticing.
        fn observed(log: &mut dyn Log<Event>) {
            log.emit(Event::IssueStarted { id: 4 });
        }

        let mut collected: Vec<Envelope<Event>> = Vec::new();
        observed(&mut Enveloping::new(ConversationId(0), &mut collected));

        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].conversation, ConversationId(0));
    }
}
