//! A window's view of the conversation: a fold over [`TranscriptItem`]s
//! into entries with stable identity.
//!
//! History folds first, live events on top, through the one
//! [`Transcript::fold`]. Each fold reports itself as a [`Change`] — which
//! entry arrived, which grew, which was replaced and by what — so a
//! window applies what it is told and decides nothing. An entry keeps its
//! id for as long as it is the same thing; whatever replaces it carries a
//! fresh one.
//!
//! A [`Card`] is how the transcript shows an observed act — a tool call,
//! a tool's answer, a turn that failed, a question settled — as distinct
//! from something a speaker said.

use super::{Answer, Ask, Role, TranscriptItem};

/// The entries a window shows, oldest first, and the counter their ids
/// come from.
#[derive(Debug, Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    next_id: u64,
}

/// One thing in the transcript. `id` is opaque, and unique within its
/// [`Transcript`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub id: String,
    pub body: Body,
}

/// What an [`Entry`] shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Body {
    /// Something said. `live` while the assistant's reply is still arriving.
    Speech {
        role: Role,
        text: String,
        live: bool,
    },
    /// An observed act: a tool call, a tool's answer, a failed turn, a settled question.
    Card(Card),
    /// A question the turn is waiting on.
    Question { id: String, ask: Ask },
}

/// What one fold did to the entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Change {
    Append(Entry),
    /// The live entry `id` grew by `delta`.
    Grow {
        id: String,
        delta: String,
    },
    /// The entry `id` is replaced, in place, by `with`, which has a fresh id.
    Replace {
        id: String,
        with: Entry,
    },
}

impl Transcript {
    /// Folds one arriving item into the entries and says what changed.
    /// New item kinds become new arms in [`Body::from`], never a second
    /// channel.
    ///
    /// Deltas grow one live tail; whatever arrives next replaces that
    /// tail — the completed message with its authoritative text, an act or
    /// a failure because the conversation never held the fragments. A
    /// resolved question replaces the pending question with its id,
    /// wherever that is, and the tail stays; with no such question shown
    /// (a window that mounted mid-question and caught only the resolution)
    /// it lands like any other act.
    pub fn fold(&mut self, item: TranscriptItem) -> Change {
        let tail = self
            .entries
            .len()
            .checked_sub(1)
            .filter(|&tail| matches!(self.entries[tail].body, Body::Speech { live: true, .. }));
        let replaced = match &item {
            TranscriptItem::AssistantDelta { text } => {
                if let Some(Entry {
                    id,
                    body:
                        Body::Speech {
                            text: streaming, ..
                        },
                }) = tail.map(|tail| &mut self.entries[tail])
                {
                    streaming.push_str(text);
                    return Change::Grow {
                        id: id.clone(),
                        delta: text.clone(),
                    };
                }
                None
            }
            TranscriptItem::QuestionResolved { id, .. } => self
                .entries
                .iter()
                .position(
                    |entry| matches!(&entry.body, Body::Question { id: asked, .. } if asked == id),
                )
                .or(tail),
            _ => tail,
        };
        let entry = Entry {
            id: self.fresh_id(),
            body: item.into(),
        };
        if let Some(at) = replaced {
            let old = std::mem::replace(&mut self.entries[at], entry.clone());
            Change::Replace {
                id: old.id,
                with: entry,
            }
        } else {
            self.entries.push(entry.clone());
            Change::Append(entry)
        }
    }

    /// The entries, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The next id off the counter.
    fn fresh_id(&mut self) -> String {
        let id = self.next_id.to_string();
        self.next_id += 1;
        id
    }
}

/// An item as a freshly arrived entry shows it. A delta is the opening
/// of a live assistant reply.
impl From<TranscriptItem> for Body {
    fn from(item: TranscriptItem) -> Self {
        match item {
            TranscriptItem::Message { role, text } => Self::Speech {
                role,
                text,
                live: false,
            },
            TranscriptItem::AssistantDelta { text } => Self::Speech {
                role: Role::Assistant,
                text,
                live: true,
            },
            TranscriptItem::Question { id, ask } => Self::Question { id, ask },
            TranscriptItem::QuestionResolved { ask, answer, .. } => Self::Card(Card {
                title: Some(ask_title(&ask).to_owned()),
                // A settled check is a command; the others are prose.
                mono: matches!(answer, Answer::Check { .. }),
                body: match answer {
                    Answer::Repository { url } => url,
                    Answer::Check { command } => command,
                    Answer::Declined => "declined".to_owned(),
                },
                tone: Tone::Neutral,
            }),
            TranscriptItem::ToolCall { name, arguments } => Self::Card(Card {
                title: Some(name),
                body: arguments,
                tone: Tone::Neutral,
                mono: true,
            }),
            TranscriptItem::ToolResult { name, ok, content } => Self::Card(Card {
                title: Some(name),
                body: content,
                tone: if ok { Tone::Neutral } else { Tone::Error },
                mono: true,
            }),
            TranscriptItem::TurnFailed { reason } => Self::Card(Card {
                title: None,
                body: reason,
                tone: Tone::Error,
                mono: false,
            }),
        }
    }
}

impl TranscriptItem {
    /// Whether this item is the last of its turn: the assistant's
    /// completed reply, or the failure that came instead. The rule a
    /// window uses to clear its busy state.
    #[must_use]
    pub const fn ends_turn(&self) -> bool {
        matches!(
            self,
            Self::Message {
                role: Role::Assistant,
                ..
            } | Self::TurnFailed { .. }
        )
    }
}

/// How a card carries itself: matter-of-fact, or bad news.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tone {
    Neutral,
    Error,
}

/// Everything a card is, decided before any rendering: the pure mapping
/// the tests pin down.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Card {
    pub title: Option<String>,
    pub body: String,
    pub tone: Tone,
    /// Machine text — arguments, results — is monospace; prose is not.
    pub mono: bool,
}

impl Card {
    /// The card for `item`, if `item` is card-shaped: tool calls, tool
    /// results, failures, and settled questions are; speech and a
    /// pending question have bodies of their own.
    #[must_use]
    pub fn of(item: &TranscriptItem) -> Option<Self> {
        match Body::from(item.clone()) {
            Body::Card(card) => Some(card),
            Body::Speech { .. } | Body::Question { .. } => None,
        }
    }
}

/// A card title for `ask`: what kind of thing was asked for.
#[must_use]
pub const fn ask_title(ask: &Ask) -> &'static str {
    match ask {
        Ask::Repository { .. } => "repository",
        Ask::Check { .. } => "check",
    }
}

/// How many characters of body a folded card shows.
pub const PREVIEW_CAP: usize = 200;

/// The state logic of the expand toggle, as a pure function: what the
/// card displays for `body` at `expanded`, and whether there is a toggle
/// at all. A body within [`PREVIEW_CAP`] has nothing to expand.
#[must_use]
pub fn shown(body: &str, expanded: bool) -> (String, bool) {
    if body.chars().count() <= PREVIEW_CAP {
        return (body.to_owned(), false);
    }
    if expanded {
        return (body.to_owned(), true);
    }
    let mut preview: String = body.chars().take(PREVIEW_CAP).collect();
    preview.push('…');
    (preview, true)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn message(role: Role, text: &str) -> TranscriptItem {
        TranscriptItem::Message {
            role,
            text: text.to_owned(),
        }
    }

    fn delta(text: &str) -> TranscriptItem {
        TranscriptItem::AssistantDelta {
            text: text.to_owned(),
        }
    }

    fn failed(reason: &str) -> TranscriptItem {
        TranscriptItem::TurnFailed {
            reason: reason.to_owned(),
        }
    }

    fn tool_call() -> TranscriptItem {
        TranscriptItem::ToolCall {
            name: "current_time".to_owned(),
            arguments: "{}".to_owned(),
        }
    }

    fn tool_result(ok: bool) -> TranscriptItem {
        TranscriptItem::ToolResult {
            name: "current_time".to_owned(),
            ok,
            content: "{\"local\":\"noon\"}".to_owned(),
        }
    }

    fn where_to() -> Ask {
        Ask::Repository {
            prompt: "Where should this live?".to_owned(),
        }
    }

    fn question(id: &str) -> TranscriptItem {
        TranscriptItem::Question {
            id: id.to_owned(),
            ask: where_to(),
        }
    }

    fn resolved(id: &str) -> TranscriptItem {
        TranscriptItem::QuestionResolved {
            id: id.to_owned(),
            ask: where_to(),
            answer: Answer::Repository {
                url: "/tmp/wumpus.git".to_owned(),
            },
        }
    }

    fn folded(items: impl IntoIterator<Item = TranscriptItem>) -> Transcript {
        let mut transcript = Transcript::default();
        for item in items {
            transcript.fold(item);
        }
        transcript
    }

    fn bodies(transcript: &Transcript) -> Vec<Body> {
        transcript
            .entries()
            .iter()
            .map(|entry| entry.body.clone())
            .collect()
    }

    /// `text` as the assistant's reply still arriving.
    fn streaming(text: &str) -> Body {
        Body::Speech {
            role: Role::Assistant,
            text: text.to_owned(),
            live: true,
        }
    }

    #[test]
    fn items_append_in_order() {
        let mut transcript = Transcript::default();
        let first = transcript.fold(message(Role::User, "first"));
        let second = transcript.fold(message(Role::Assistant, "second"));
        assert_eq!(
            bodies(&transcript),
            [
                message(Role::User, "first").into(),
                message(Role::Assistant, "second").into()
            ]
        );
        assert_eq!(first, Change::Append(transcript.entries()[0].clone()));
        assert_eq!(second, Change::Append(transcript.entries()[1].clone()));
    }

    #[test]
    fn tool_items_land_in_order_like_messages() {
        let items = [
            message(Role::User, "what time is it?"),
            tool_call(),
            tool_result(true),
            message(Role::Assistant, "It's noon."),
        ];
        let expected: Vec<Body> = items.iter().cloned().map(Body::from).collect();
        assert_eq!(bodies(&folded(items)), expected);
    }

    #[test]
    fn deltas_grow_one_live_entry() {
        let mut transcript = folded([message(Role::User, "hi")]);
        let opened = transcript.fold(delta("Hel"));
        let tail = transcript.entries()[1].clone();
        assert_eq!(tail.body, streaming("Hel"));
        assert_eq!(opened, Change::Append(tail.clone()));

        let grew = transcript.fold(delta("lo"));
        assert_eq!(
            grew,
            Change::Grow {
                id: tail.id.clone(),
                delta: "lo".to_owned(),
            }
        );
        assert_eq!(
            transcript.entries()[1..],
            [Entry {
                id: tail.id,
                body: streaming("Hello"),
            }]
        );
    }

    #[test]
    fn the_completed_message_replaces_the_live_entry_and_is_not_live() {
        let mut transcript = folded([message(Role::User, "hi"), delta("Hel")]);
        let tail = transcript.entries()[1].id.clone();
        let change = transcript.fold(message(Role::Assistant, "Hello"));
        assert_eq!(
            bodies(&transcript),
            [
                message(Role::User, "hi").into(),
                Body::Speech {
                    role: Role::Assistant,
                    text: "Hello".to_owned(),
                    live: false,
                }
            ],
            "the final text is authoritative"
        );
        assert_eq!(
            change,
            Change::Replace {
                id: tail,
                with: transcript.entries()[1].clone(),
            }
        );
    }

    #[test]
    fn a_failure_replaces_the_live_entry() {
        let mut transcript = folded([message(Role::User, "hi"), delta("Hel")]);
        let tail = transcript.entries()[1].id.clone();
        let change = transcript.fold(failed("the wire went quiet"));
        assert_eq!(
            bodies(&transcript),
            [
                message(Role::User, "hi").into(),
                failed("the wire went quiet").into()
            ],
            "the conversation never held the fragments, so the view doesn't either"
        );
        assert!(
            matches!(&change, Change::Replace { id, .. } if *id == tail),
            "{change:?}"
        );
    }

    #[test]
    fn a_failure_with_no_stream_just_lands() {
        let mut transcript = folded([message(Role::User, "hi")]);
        let change = transcript.fold(failed("no key"));
        assert_eq!(change, Change::Append(transcript.entries()[1].clone()));
    }

    /// Text the model streamed before deciding on a tool never became
    /// conversation, so a tool call replaces the fragments like a
    /// completed message would.
    #[test]
    fn a_tool_call_replaces_a_streaming_tail() {
        let mut transcript = folded([message(Role::User, "hi"), delta("Let me")]);
        let change = transcript.fold(tool_call());
        assert_eq!(
            bodies(&transcript),
            [message(Role::User, "hi").into(), tool_call().into()]
        );
        assert!(matches!(change, Change::Replace { .. }), "{change:?}");
    }

    #[test]
    fn a_question_replaces_a_streaming_tail_like_any_act() {
        let transcript = folded([message(Role::User, "hi"), delta("Let me"), question("1")]);
        assert_eq!(
            bodies(&transcript),
            [message(Role::User, "hi").into(), question("1").into()]
        );
    }

    #[test]
    fn a_question_appends_and_its_resolution_replaces_it_in_place() {
        let mut transcript = folded([message(Role::User, "write me wumpus")]);
        let asked = transcript.fold(question("1"));
        assert_eq!(asked, Change::Append(transcript.entries()[1].clone()));
        assert_eq!(
            transcript.entries()[1].body,
            Body::Question {
                id: "1".to_owned(),
                ask: where_to(),
            }
        );
        transcript.fold(tool_call());
        transcript.fold(delta("Let me"));
        let pending = transcript.entries()[1].id.clone();

        let change = transcript.fold(resolved("1"));
        assert_eq!(
            bodies(&transcript),
            [
                message(Role::User, "write me wumpus").into(),
                resolved("1").into(),
                tool_call().into(),
                streaming("Let me"),
            ],
            "the resolution takes the pending question's place, and the tail stays"
        );
        assert_eq!(
            change,
            Change::Replace {
                id: pending,
                with: transcript.entries()[1].clone(),
            }
        );
    }

    #[test]
    fn a_resolution_with_no_pending_question_appends() {
        let mut transcript = folded([message(Role::User, "hi")]);
        let change = transcript.fold(resolved("7"));
        assert_eq!(
            bodies(&transcript),
            [message(Role::User, "hi").into(), resolved("7").into()]
        );
        assert_eq!(change, Change::Append(transcript.entries()[1].clone()));
    }

    #[test]
    fn a_resolution_with_no_pending_question_replaces_a_streaming_tail() {
        let transcript = folded([delta("Let me"), resolved("7")]);
        assert_eq!(bodies(&transcript), [resolved("7").into()]);
    }

    #[test]
    fn a_resolution_replaces_only_the_question_with_its_id() {
        let transcript = folded([question("1"), question("2"), resolved("2")]);
        assert_eq!(
            bodies(&transcript),
            [question("1").into(), resolved("2").into()]
        );
    }

    /// The fresh id is what makes a keyed view rebuild a pending question
    /// as its resolution.
    #[test]
    fn a_replacements_id_differs_from_the_replaced_entrys() {
        for (shown, arriving) in [
            (delta("Hel"), message(Role::Assistant, "Hello")),
            (question("1"), resolved("1")),
        ] {
            let mut transcript = folded([shown]);
            let Change::Replace { id, with } = transcript.fold(arriving) else {
                panic!("a replacement");
            };
            assert_ne!(id, with.id);
        }
    }

    #[test]
    fn ids_are_unique_across_a_transcript() {
        let mut transcript = Transcript::default();
        let mut ids = HashSet::new();
        for item in [
            message(Role::User, "hi"),
            delta("Hel"),
            delta("lo"),
            message(Role::Assistant, "Hello"),
            question("1"),
            tool_call(),
            tool_result(true),
            resolved("1"),
            delta("Do"),
            failed("the wire went quiet"),
        ] {
            match transcript.fold(item) {
                Change::Append(entry) | Change::Replace { with: entry, .. } => {
                    assert!(ids.insert(entry.id), "every new entry's id is new");
                }
                Change::Grow { id, .. } => {
                    assert!(ids.contains(&id), "a growth names an entry already seen");
                }
            }
        }
    }

    #[test]
    fn folding_history_then_live_items_is_folding_them_all_live() {
        let history = [
            message(Role::User, "what time is it?"),
            tool_call(),
            tool_result(true),
            message(Role::Assistant, "It's noon."),
        ];
        let live = [
            message(Role::User, "write me wumpus"),
            question("1"),
            delta("On"),
            delta(" it"),
            resolved("1"),
        ];
        let mut mounted = folded(history.clone());
        for item in live.clone() {
            mounted.fold(item);
        }
        let watched = folded(history.into_iter().chain(live));
        assert_eq!(mounted.entries(), watched.entries());
    }

    #[test]
    fn only_an_assistant_message_and_a_failure_end_a_turn() {
        for (item, ends) in [
            (message(Role::Assistant, "Hello"), true),
            (failed("no key"), true),
            (message(Role::User, "hi"), false),
            (delta("Hel"), false),
            (tool_call(), false),
            (tool_result(true), false),
            (tool_result(false), false),
            (question("1"), false),
            (resolved("1"), false),
        ] {
            assert_eq!(item.ends_turn(), ends, "{item:?}");
        }
    }

    #[test]
    fn a_settled_question_is_a_neutral_prose_card_titled_by_its_kind() {
        let ask = Ask::Repository {
            prompt: "Where should this live?".to_owned(),
        };
        let answered = TranscriptItem::QuestionResolved {
            id: "1".to_owned(),
            ask: ask.clone(),
            answer: Answer::Repository {
                url: "/tmp/wumpus.git".to_owned(),
            },
        };
        assert_eq!(
            Card::of(&answered),
            Some(Card {
                title: Some("repository".to_owned()),
                body: "/tmp/wumpus.git".to_owned(),
                tone: Tone::Neutral,
                mono: false,
            })
        );

        let declined = TranscriptItem::QuestionResolved {
            id: "2".to_owned(),
            ask,
            answer: Answer::Declined,
        };
        assert_eq!(
            Card::of(&declined),
            Some(Card {
                title: Some("repository".to_owned()),
                body: "declined".to_owned(),
                tone: Tone::Neutral,
                mono: false,
            })
        );
    }

    #[test]
    fn a_settled_check_is_a_mono_card_carrying_the_command_in_force() {
        let ask = Ask::Check {
            prompt: "What says green?".to_owned(),
            proposal: Some("cargo test".to_owned()),
        };
        let confirmed = TranscriptItem::QuestionResolved {
            id: "1".to_owned(),
            ask: ask.clone(),
            answer: Answer::Check {
                command: "cargo test --workspace".to_owned(),
            },
        };
        assert_eq!(
            Card::of(&confirmed),
            Some(Card {
                title: Some("check".to_owned()),
                body: "cargo test --workspace".to_owned(),
                tone: Tone::Neutral,
                mono: true,
            })
        );

        let skipped = TranscriptItem::QuestionResolved {
            id: "2".to_owned(),
            ask,
            answer: Answer::Declined,
        };
        assert_eq!(
            Card::of(&skipped),
            Some(Card {
                title: Some("check".to_owned()),
                body: "declined".to_owned(),
                tone: Tone::Neutral,
                mono: false,
            })
        );
    }

    #[test]
    fn a_pending_question_is_not_a_card() {
        assert_eq!(Card::of(&question("1")), None);
    }

    #[test]
    fn a_tool_call_is_a_neutral_mono_card_titled_by_its_tool() {
        assert_eq!(
            Card::of(&tool_call()),
            Some(Card {
                title: Some("current_time".to_owned()),
                body: "{}".to_owned(),
                tone: Tone::Neutral,
                mono: true,
            })
        );
    }

    #[test]
    fn a_tool_result_wears_its_outcome_as_tone() {
        assert_eq!(Card::of(&tool_result(true)).unwrap().tone, Tone::Neutral);
        assert_eq!(Card::of(&tool_result(false)).unwrap().tone, Tone::Error);
    }

    #[test]
    fn a_failed_turn_is_an_untitled_error_card_in_prose() {
        assert_eq!(
            Card::of(&failed("the wire went quiet")),
            Some(Card {
                title: None,
                body: "the wire went quiet".to_owned(),
                tone: Tone::Error,
                mono: false,
            })
        );
    }

    #[test]
    fn speech_is_never_a_card() {
        assert_eq!(Card::of(&message(Role::User, "hi")), None);
        assert_eq!(Card::of(&delta("he")), None);
    }

    #[test]
    fn a_body_at_the_cap_has_nothing_to_expand() {
        let body = "x".repeat(PREVIEW_CAP);
        assert_eq!(shown(&body, false), (body.clone(), false));
        assert_eq!(shown(&body, true), (body, false));
    }

    #[test]
    fn one_past_the_cap_folds_with_an_ellipsis_and_a_toggle() {
        let body = "x".repeat(PREVIEW_CAP + 1);
        assert_eq!(
            shown(&body, false),
            (format!("{}…", "x".repeat(PREVIEW_CAP)), true)
        );
        assert_eq!(shown(&body, true), (body, true), "expanded shows it all");
    }
}
