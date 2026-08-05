//! Talking to a language model.
//!
//! This is library infrastructure, not the window's conversation surface.
//! The same client will draft commit messages and summarize runs, and it is
//! what tests talk to; whatever the window's main pane eventually hosts is a
//! separate question.
//!
//! The shape mirrors the implementation side: a trait for the thing that
//! does the work ([`ChatModel`]), a vocabulary for what it is doing
//! ([`ChatEvent`](crate::event::ChatEvent)), and an owner of the truth
//! ([`Conversation`]).
//!
//! Everything here is blocking, on purpose. Async is the host's problem —
//! `spawn_blocking` under Tauri, plain threads in the daemon and in tests —
//! and cancellation is a [`StopToken`] checked between deltas, which is all
//! a Stop button will ever need.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::event::{ChatEvent, Usage};
use crate::logging::Log;

mod scripted;
pub use scripted::Scripted;

// The HTTP client and its SSE framing are native-only: wasm32 builds of this
// crate exist to supply types to the frontend, not to make requests.
#[cfg(feature = "native")]
pub mod openai;
#[cfg(feature = "native")]
mod sse;

#[cfg(feature = "native")]
pub use openai::OpenAiCompatible;

/// Who said a thing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// One turn in a transcript.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// Cancellation: a flag checked between deltas. Cloning shares the flag, so
/// a host can hand one copy to the thread that is streaming and keep another
/// for the button that stops it.
#[derive(Clone, Debug, Default)]
pub struct StopToken(Arc<AtomicBool>);

impl StopToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the turn in flight to wind up. Never unsets: a stopped turn
    /// stays stopped, and the next turn gets a fresh token.
    pub fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// One assistant turn, assembled from its deltas.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Reply {
    /// Every delta, concatenated.
    pub text: String,
    /// Present only when the provider reported it.
    pub usage: Option<Usage>,
    /// The stop token fired before the model was finished.
    pub interrupted: bool,
}

/// A language model, as Epik needs one: a transcript in, one streamed reply
/// out.
///
/// The caller owns the history. That is the right seam for the stateless
/// chat-completions wire protocol, and for a session-owning model — a Claude
/// Code adapter, say — the answer is a narrower trait rather than a
/// contortion of this one.
pub trait ChatModel {
    /// Streams one assistant reply to `transcript`.
    ///
    /// Deltas go to `log` as they arrive; the assembled reply comes back.
    /// The call blocks. `stop` is checked between deltas, and when it is set
    /// the reply returns with what it has and `interrupted` set — an
    /// interruption is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error when the reply cannot be completed at all.
    fn reply(
        &mut self,
        transcript: &[Message],
        log: &mut dyn Log<ChatEvent>,
        stop: &StopToken,
    ) -> Result<Reply>;
}

/// A model that authenticates with a key, and can be handed one later.
///
/// This is the seam that makes the paste-your-key card work: a key arriving
/// after the first turn was refused has to take effect on the conversation
/// already in progress, not on the next process. A model with nobody to
/// authenticate to implements this and does nothing with it.
pub trait Keyed {
    fn use_key(&mut self, key: Option<String>);
}

/// The canonical transcript, and the model that extends it.
///
/// A frontend's transcript is a *view*, folded from the event stream. This is
/// the truth. The duplication is deliberate: core owns what a conversation
/// is, so the daemon and every future client get it for free rather than
/// each reimplementing it.
#[derive(Debug)]
pub struct Conversation<M> {
    system: String,
    messages: Vec<Message>,
    model: M,
}

impl<M: ChatModel> Conversation<M> {
    pub fn new(system: impl Into<String>, model: M) -> Self {
        Self {
            system: system.into(),
            messages: Vec::new(),
            model,
        }
    }

    /// The system prompt followed by every completed turn: exactly what goes
    /// on the wire. The protocol is stateless, so this is resent every turn.
    #[must_use]
    pub fn transcript(&self) -> Vec<Message> {
        let mut transcript = Vec::with_capacity(self.messages.len() + 1);
        transcript.push(Message::system(self.system.clone()));
        transcript.extend(self.messages.iter().cloned());
        transcript
    }

    /// The completed turns, without the system prompt.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub const fn model(&self) -> &M {
        &self.model
    }

    /// The model, to be reconfigured mid-conversation — a key that has just
    /// arrived, say. The transcript is untouched, which is the whole reason
    /// this exists rather than building a second conversation: what was said
    /// stands.
    pub const fn model_mut(&mut self) -> &mut M {
        &mut self.model
    }

    /// Appends the user's turn, streams the assistant's, and finalizes it
    /// into the transcript.
    ///
    /// A turn that fails leaves no trace: the user's message is rolled back,
    /// so the transcript never holds a turn that produced nothing and a retry
    /// is a plain resend. An interrupted turn keeps whatever the model said,
    /// and says nothing when it said nothing.
    ///
    /// The failure is reported twice, deliberately: as a `Failed` event for
    /// whatever is watching the stream, and as an `Err` for the caller that
    /// has to decide what to do. A host uses the event and swallows the
    /// error, which is what keeps streamed failures and command errors on
    /// separate channels.
    ///
    /// # Errors
    ///
    /// Returns the model's error when the turn could not be completed.
    pub fn send(
        &mut self,
        text: impl Into<String>,
        log: &mut dyn Log<ChatEvent>,
        stop: &StopToken,
    ) -> Result<Reply> {
        self.messages.push(Message::user(text));
        let transcript = self.transcript();
        match self.model.reply(&transcript, log, stop) {
            Ok(reply) => {
                if !reply.text.is_empty() {
                    self.messages.push(Message::assistant(reply.text.clone()));
                }
                log.emit(ChatEvent::TurnFinished { usage: reply.usage });
                Ok(reply)
            }
            Err(error) => {
                self.messages.pop();
                log.emit(ChatEvent::Failed {
                    error: format!("{error:#}"),
                });
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::Silent;

    const PERSONA: &str = "You are Epik.";

    /// Collects the event stream, and can pull the stop token mid-turn.
    #[derive(Debug, Default)]
    struct Watcher {
        events: Vec<ChatEvent>,
        stop_after_first_delta: Option<StopToken>,
    }

    impl Log<ChatEvent> for Watcher {
        fn emit(&mut self, event: ChatEvent) {
            if matches!(event, ChatEvent::Delta { .. })
                && let Some(stop) = self.stop_after_first_delta.take()
            {
                stop.stop();
            }
            self.events.push(event);
        }
    }

    fn deltas(events: &[ChatEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                ChatEvent::Delta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_turn_streams_then_lands_in_the_transcript() {
        let mut chat = Conversation::new(PERSONA, Scripted::saying(["Hello, ", "I'm Epik."]));
        let mut watcher = Watcher::default();

        let reply = chat
            .send("Hi", &mut watcher, &StopToken::new())
            .expect("the scripted turn succeeds");

        assert_eq!(reply.text, "Hello, I'm Epik.");
        assert_eq!(deltas(&watcher.events), "Hello, I'm Epik.");
        assert_eq!(
            watcher.events.last(),
            Some(&ChatEvent::TurnFinished { usage: None })
        );
        assert_eq!(
            chat.messages(),
            [Message::user("Hi"), Message::assistant("Hello, I'm Epik.")]
        );
    }

    #[test]
    fn the_system_prompt_heads_every_transcript_on_the_wire() {
        let mut chat = Conversation::new(PERSONA, Scripted::saying(["one"]).then_saying(["two"]));

        chat.send("first", &mut Silent, &StopToken::new()).unwrap();
        chat.send("second", &mut Silent, &StopToken::new()).unwrap();

        let seen = chat.model().seen();
        assert_eq!(
            seen[0],
            [Message::system(PERSONA), Message::user("first")],
            "the first turn sends the persona and the question"
        );
        assert_eq!(
            seen[1],
            [
                Message::system(PERSONA),
                Message::user("first"),
                Message::assistant("one"),
                Message::user("second"),
            ],
            "the second turn resends the whole conversation"
        );
    }

    #[test]
    fn usage_reaches_the_finished_event_when_the_provider_reports_it() {
        let usage = Usage {
            prompt_tokens: 11,
            completion_tokens: 3,
        };
        let mut chat = Conversation::new(PERSONA, Scripted::saying(["hi"]).reporting(usage));
        let mut watcher = Watcher::default();

        chat.send("Hi", &mut watcher, &StopToken::new()).unwrap();

        assert_eq!(
            watcher.events.last(),
            Some(&ChatEvent::TurnFinished { usage: Some(usage) })
        );
    }

    #[test]
    fn a_failed_turn_leaves_the_transcript_as_it_was() {
        let mut chat = Conversation::new(PERSONA, Scripted::failing("the network went away"));
        let mut watcher = Watcher::default();

        let error = chat
            .send("Hi", &mut watcher, &StopToken::new())
            .expect_err("the scripted turn fails");

        assert!(error.to_string().contains("the network went away"));
        assert_eq!(
            watcher.events,
            [ChatEvent::Failed {
                error: "the network went away".to_owned()
            }]
        );
        assert!(
            chat.messages().is_empty(),
            "the user's rolled-back turn makes a retry a plain resend"
        );
    }

    #[test]
    fn a_later_turn_succeeds_after_a_failed_one() {
        let mut chat = Conversation::new(
            PERSONA,
            Scripted::failing("timed out").then_saying(["second time lucky"]),
        );

        chat.send("Hi", &mut Silent, &StopToken::new())
            .expect_err("the first turn fails");
        chat.send("Hi", &mut Silent, &StopToken::new())
            .expect("the second turn succeeds");

        assert_eq!(
            chat.messages(),
            [Message::user("Hi"), Message::assistant("second time lucky"),],
            "the retry resent the same question and got an answer"
        );
    }

    #[test]
    fn stopping_mid_stream_finalizes_the_turn_with_what_arrived() {
        let stop = StopToken::new();
        let mut chat = Conversation::new(PERSONA, Scripted::saying(["kept", "dropped", "dropped"]));
        let mut watcher = Watcher {
            stop_after_first_delta: Some(stop.clone()),
            ..Watcher::default()
        };

        let reply = chat
            .send("Hi", &mut watcher, &stop)
            .expect("an interruption is not an error");

        assert!(reply.interrupted);
        assert_eq!(reply.text, "kept");
        assert_eq!(
            watcher.events,
            [
                ChatEvent::Delta {
                    text: "kept".to_owned()
                },
                ChatEvent::TurnFinished { usage: None },
            ]
        );
        assert_eq!(
            chat.messages(),
            [Message::user("Hi"), Message::assistant("kept")],
            "a stopped turn is still a turn"
        );
    }

    #[test]
    fn stopping_before_the_first_delta_records_no_assistant_turn() {
        let stop = StopToken::new();
        stop.stop();
        let mut chat = Conversation::new(PERSONA, Scripted::saying(["never sent"]));
        let mut watcher = Watcher::default();

        let reply = chat.send("Hi", &mut watcher, &stop).unwrap();

        assert!(reply.interrupted);
        assert!(reply.text.is_empty());
        assert_eq!(watcher.events, [ChatEvent::TurnFinished { usage: None }]);
        assert_eq!(
            chat.messages(),
            [Message::user("Hi")],
            "a model that said nothing gets no turn in the transcript"
        );
    }
}
