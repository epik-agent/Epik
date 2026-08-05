//! What the chat pane shows, and the fold that decides it.
//!
//! [`Pane`] is a pure fold over everything that happens to a conversation from
//! this side of the boundary: the user's own messages, the host's event stream,
//! and the answers to the commands it sent. There is no Leptos here and no IPC,
//! so every rule about what the user sees is tested on the host target with a
//! plain `cargo test` — which is the spike's deepest lesson, carried over
//! unchanged.
//!
//! This is a *view*. The canonical transcript lives in the library, and the
//! two deliberately differ in one place: a turn that failed is rolled back
//! there, so a retry is a plain resend, while here the question and the error
//! card both stay on screen. Core owns truth; the pane owns a rendering of it.

use epik::event::{ChatEvent, Usage};
use epik::session::Status;

/// One entry in the transcript.
///
/// `Hash` because a turn's position and content together are what identifies it
/// on screen: a rendered turn is reused only while both are unchanged, so a
/// question that was withdrawn and asked again is a new turn rather than a stale
/// one at the same index.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Turn {
    /// What the user said.
    User { text: String },
    /// What the model said, complete. Rendered as markdown.
    Assistant { text: String },
    /// A turn that broke mid-stream: an error card, sitting where the rest of
    /// the answer would have been.
    Failed { error: String },
}

/// The chat pane's whole state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Pane {
    turns: Vec<Turn>,
    /// The assistant turn in flight, accumulating deltas. `Some` from the
    /// moment a message is sent — waiting for the first delta is still
    /// waiting — and that is also what disables the input.
    streaming: Option<String>,
    /// What the host said about the session. `None` until it answers.
    status: Option<Status>,
    /// The last command error: a banner, never a transcript card. Streamed
    /// failures are the other channel and land in `turns`, and keeping the two
    /// apart on screen is the point of keeping them apart in the protocol.
    refusal: Option<String>,
    /// What the last turn cost, when the provider counted it.
    usage: Option<Usage>,
}

impl Pane {
    /// The host has answered for the session: the status bar can name the
    /// model, and the paste-your-key card knows whether to appear.
    pub fn opened(&mut self, status: Status) {
        self.status = Some(status);
        // An answer having arrived, whatever the last one complained about is
        // no longer the current state of things.
        self.refusal = None;
    }

    /// The user has sent `text` — unless a turn is already in flight, in which
    /// case nothing happens and the caller has its answer in the return value.
    ///
    /// Three things enforce one turn at a time: the disabled input, this, and
    /// the host's mutex. That is not too many for a failure whose shape would
    /// be two replies interleaved in one buffer.
    #[must_use]
    pub fn asked(&mut self, text: impl Into<String>) -> bool {
        if self.is_busy() {
            return false;
        }
        self.turns.push(Turn::User { text: text.into() });
        self.streaming = Some(String::new());
        self.refusal = None;
        true
    }

    /// One event from the host. This is the fold.
    pub fn saw(&mut self, event: ChatEvent) {
        match event {
            // A delta with no turn in flight means a host further along than
            // this build; keeping the text is friendlier than dropping it.
            ChatEvent::Delta { text } => self
                .streaming
                .get_or_insert_with(String::new)
                .push_str(&text),
            ChatEvent::TurnFinished { usage } => {
                self.finish();
                if usage.is_some() {
                    self.usage = usage;
                }
            }
            ChatEvent::Failed { error } => {
                // Whatever deltas arrived stand: half an answer is still worth
                // reading, and the card beneath it says why it stopped.
                self.finish();
                self.turns.push(Turn::Failed { error });
            }
        }
    }

    /// A command was refused. A banner, not a transcript card: a request that
    /// was never accepted produced no turn to show.
    pub fn refused(&mut self, error: impl Into<String>) {
        self.refusal = Some(error.into());
    }

    /// The turn never started, and the host said why.
    ///
    /// The question comes back off the transcript — the library's own does the
    /// same, and for the same reason — and is handed back so the composer can
    /// return it rather than make the user type it again.
    pub fn unsent(&mut self, error: impl Into<String>) -> Option<String> {
        self.streaming = None;
        self.refusal = Some(error.into());
        match self.turns.last() {
            Some(Turn::User { .. }) => match self.turns.pop() {
                Some(Turn::User { text }) => Some(text),
                _ => None,
            },
            _ => None,
        }
    }

    /// Ends the turn in flight, keeping what arrived. A model that said
    /// nothing gets no turn — which is the transcript the library keeps too.
    fn finish(&mut self) {
        if let Some(text) = self.streaming.take()
            && !text.is_empty()
        {
            self.turns.push(Turn::Assistant { text });
        }
    }

    /// The completed transcript, oldest first.
    #[must_use]
    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    /// The reply as far as it has arrived, while it is arriving.
    #[must_use]
    pub fn streaming(&self) -> Option<&str> {
        self.streaming.as_deref()
    }

    /// A turn is in flight, so the input is disabled and a second question
    /// will not be taken.
    #[must_use]
    pub const fn is_busy(&self) -> bool {
        self.streaming.is_some()
    }

    /// Who is answering, once the host has said.
    #[must_use]
    pub const fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    /// The last command error, for the banner.
    #[must_use]
    pub fn refusal(&self) -> Option<&str> {
        self.refusal.as_deref()
    }

    /// What the last turn cost, when the provider counted it.
    #[must_use]
    pub const fn usage(&self) -> Option<Usage> {
        self.usage
    }

    /// Whether to show the paste-your-key card: the host has answered, and the
    /// provider it named has no key to reach it with.
    #[must_use]
    pub fn needs_key(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.key.wanted())
    }

    /// Why this computer's keyring could not be consulted, when it could not
    /// be. Worth saying on the card, since a key pasted onto a machine in that
    /// state will not be kept either.
    #[must_use]
    pub fn key_trouble(&self) -> Option<&str> {
        self.status.as_ref().and_then(|status| status.key.trouble())
    }
}

#[cfg(test)]
mod tests {
    use epik::session::Key;

    use super::*;

    fn status(key: Key) -> Status {
        Status {
            provider: "ollama".to_owned(),
            model: "smollm2:135m".to_owned(),
            key,
        }
    }

    fn delta(text: &str) -> ChatEvent {
        ChatEvent::Delta {
            text: text.to_owned(),
        }
    }

    /// A pane mid-turn: asked, and streaming.
    fn asked(text: &str) -> Pane {
        let mut pane = Pane::default();
        assert!(pane.asked(text), "an idle pane takes a question");
        pane
    }

    #[test]
    fn a_fresh_pane_shows_nothing_and_asks_for_nothing() {
        let pane = Pane::default();
        assert!(pane.turns().is_empty());
        assert!(!pane.is_busy(), "an empty pane takes a question");
        assert!(
            !pane.needs_key(),
            "no card until the host has said whether a key is wanted"
        );
        assert_eq!(pane.status(), None);
    }

    #[test]
    fn deltas_accumulate_into_a_streaming_buffer_before_the_turn_lands() {
        let mut pane = asked("Hi");
        assert_eq!(pane.streaming(), Some(""), "waiting is still streaming");

        pane.saw(delta("Hello, "));
        pane.saw(delta("I'm Epik."));

        assert_eq!(pane.streaming(), Some("Hello, I'm Epik."));
        assert_eq!(
            pane.turns(),
            [Turn::User {
                text: "Hi".to_owned()
            }],
            "the answer is not a turn until it is finished"
        );
    }

    #[test]
    fn a_finished_turn_becomes_a_completed_turn() {
        let mut pane = asked("Hi");
        pane.saw(delta("Hello, I'm Epik."));

        pane.saw(ChatEvent::TurnFinished { usage: None });

        assert_eq!(
            pane.turns(),
            [
                Turn::User {
                    text: "Hi".to_owned()
                },
                Turn::Assistant {
                    text: "Hello, I'm Epik.".to_owned()
                },
            ]
        );
        assert_eq!(pane.streaming(), None);
        assert!(!pane.is_busy(), "the input comes back");
    }

    #[test]
    fn a_turn_that_produced_nothing_leaves_no_turn_behind() {
        let mut pane = asked("Hi");

        pane.saw(ChatEvent::TurnFinished { usage: None });

        assert_eq!(
            pane.turns(),
            [Turn::User {
                text: "Hi".to_owned()
            }],
            "an empty answer is not an empty bubble"
        );
        assert!(!pane.is_busy());
    }

    #[test]
    fn a_streamed_failure_becomes_an_error_card_after_what_arrived() {
        let mut pane = asked("Hi");
        pane.saw(delta("Hello, I'm "));

        pane.saw(ChatEvent::Failed {
            error: "the network went away".to_owned(),
        });

        assert_eq!(
            pane.turns(),
            [
                Turn::User {
                    text: "Hi".to_owned()
                },
                Turn::Assistant {
                    text: "Hello, I'm ".to_owned()
                },
                Turn::Failed {
                    error: "the network went away".to_owned()
                },
            ],
            "half an answer is worth reading, and the card says why it stopped"
        );
        assert!(!pane.is_busy(), "a failed turn does not leave a hang");
        assert_eq!(
            pane.refusal(),
            None,
            "a streamed failure is not a command error"
        );
    }

    #[test]
    fn a_failure_before_the_first_delta_is_a_card_on_its_own() {
        let mut pane = asked("Hi");

        pane.saw(ChatEvent::Failed {
            error: "nothing is listening".to_owned(),
        });

        assert_eq!(
            pane.turns(),
            [
                Turn::User {
                    text: "Hi".to_owned()
                },
                Turn::Failed {
                    error: "nothing is listening".to_owned()
                },
            ]
        );
    }

    #[test]
    fn the_pane_takes_a_second_question_once_the_first_is_answered() {
        let mut pane = asked("first");
        assert!(
            !pane.asked("second"),
            "a question arriving mid-turn is refused, not interleaved"
        );
        assert_eq!(pane.turns().len(), 1);

        pane.saw(ChatEvent::TurnFinished { usage: None });

        assert!(pane.asked("second"), "and taken once the turn is done");
    }

    #[test]
    fn a_refused_turn_hands_the_question_back_and_ends_the_wait() {
        let mut pane = asked("Hi");

        let returned = pane.unsent("one turn at a time");

        assert_eq!(
            returned.as_deref(),
            Some("Hi"),
            "the composer gets it back rather than the user retyping it"
        );
        assert!(
            pane.turns().is_empty(),
            "a turn that never went leaves none"
        );
        assert!(!pane.is_busy(), "and does not leave the input disabled");
        assert_eq!(pane.refusal(), Some("one turn at a time"));
    }

    #[test]
    fn a_command_error_is_a_banner_and_a_streamed_failure_is_a_card() {
        let mut pane = Pane::default();

        pane.refused("the keyring would not take it");

        assert_eq!(pane.refusal(), Some("the keyring would not take it"));
        assert!(
            pane.turns().is_empty(),
            "the two channels are observably different: this one shows no card"
        );
    }

    #[test]
    fn asking_again_clears_the_last_refusal() {
        let mut pane = Pane::default();
        pane.refused("the session could not be opened");

        assert!(pane.asked("Hi"));

        assert_eq!(
            pane.refusal(),
            None,
            "a stale complaint should not sit over a live turn"
        );
    }

    #[test]
    fn the_status_bar_names_the_model_the_host_reports() {
        let mut pane = Pane::default();

        pane.opened(status(Key::Present));

        assert_eq!(
            pane.status().map(|status| status.model.as_str()),
            Some("smollm2:135m")
        );
        assert!(!pane.needs_key());
    }

    #[test]
    fn a_provider_with_no_key_asks_for_one() {
        let mut pane = Pane::default();

        pane.opened(status(Key::Absent));

        assert!(
            pane.needs_key(),
            "the paste-your-key card belongs on screen"
        );
    }

    #[test]
    fn a_keyring_that_will_not_answer_asks_for_a_key_and_says_why_it_may_not_stick() {
        let mut pane = Pane::default();

        pane.opened(status(Key::Unreachable {
            reason: "no default store has been set".to_owned(),
        }));

        assert!(
            pane.needs_key(),
            "somewhere to paste one is still the right offer"
        );
        assert_eq!(pane.key_trouble(), Some("no default store has been set"));
    }

    #[test]
    fn a_key_that_is_there_has_no_trouble_to_report() {
        let mut pane = Pane::default();
        pane.opened(status(Key::Present));
        assert_eq!(pane.key_trouble(), None);

        pane.opened(status(Key::Absent));
        assert_eq!(
            pane.key_trouble(),
            None,
            "an empty keyring is a working keyring"
        );
    }

    #[test]
    fn a_key_that_has_been_stored_puts_the_card_away() {
        let mut pane = Pane::default();
        pane.opened(status(Key::Absent));

        pane.opened(status(Key::Present));

        assert!(!pane.needs_key());
    }

    #[test]
    fn usage_is_kept_when_the_provider_reports_it_and_not_invented_when_it_does_not() {
        let mut pane = asked("Hi");
        pane.saw(delta("hello"));
        let counted = Usage {
            prompt_tokens: 18,
            completion_tokens: 6,
        };

        pane.saw(ChatEvent::TurnFinished {
            usage: Some(counted),
        });
        assert_eq!(pane.usage(), Some(counted));

        // A second turn from a provider that counts nothing must not erase what
        // the first one reported, nor pretend to a count of its own.
        assert!(pane.asked("Again"));
        pane.saw(delta("hello"));
        pane.saw(ChatEvent::TurnFinished { usage: None });
        assert_eq!(pane.usage(), Some(counted));
    }

    #[test]
    fn a_delta_for_a_turn_this_build_never_saw_start_is_kept_rather_than_dropped() {
        let mut pane = Pane::default();

        pane.saw(delta("out of nowhere"));

        assert_eq!(pane.streaming(), Some("out of nowhere"));
    }

    #[test]
    fn a_whole_conversation_folds_in_order() {
        let mut pane = Pane::default();
        pane.opened(status(Key::Present));

        assert!(pane.asked("Hi"));
        pane.saw(delta("Hello, "));
        pane.saw(delta("I'm Epik."));
        pane.saw(ChatEvent::TurnFinished { usage: None });

        assert!(pane.asked("And then?"));
        pane.saw(delta("Then "));
        pane.saw(ChatEvent::Failed {
            error: "the network went away".to_owned(),
        });

        assert_eq!(
            pane.turns(),
            [
                Turn::User {
                    text: "Hi".to_owned()
                },
                Turn::Assistant {
                    text: "Hello, I'm Epik.".to_owned()
                },
                Turn::User {
                    text: "And then?".to_owned()
                },
                Turn::Assistant {
                    text: "Then ".to_owned()
                },
                Turn::Failed {
                    error: "the network went away".to_owned()
                },
            ]
        );
        assert!(!pane.is_busy(), "the app stays usable after a failure");
        assert!(pane.asked("Again?"), "and takes the next question");
    }
}
