//! The assistant's vocabulary: what was said, what the assistant is
//! asking, what the user answered.
//!
//! [`TranscriptItem`] is the unit that crosses the IPC barrier and gets
//! folded into a window's view of the conversation. Its serde form is
//! tagged, and that tag is the forward-compatibility contract: new kinds
//! of item become new variants on the same event channel and new arms in
//! the same fold, never a schema break.
//!
//! The [`Client`] speaks to the model and [`run`] is a whole turn of it
//! over a [`Registry`](crate::model_tools::Registry), under `native`.

mod client;
pub mod json;
#[cfg(feature = "native")]
mod prompt;
#[cfg(all(feature = "native", any(test, feature = "testing")))]
pub mod scripted;
mod segments;
mod transcript;
#[cfg(feature = "native")]
mod wire;

use serde::{Deserialize, Serialize};

pub use client::{ANTHROPIC_MODEL, ModelInfo};
#[cfg(feature = "native")]
pub use client::{Client, Error, models, run};
#[cfg(feature = "native")]
pub use prompt::SYSTEM_PROMPT;
pub use segments::{Segment, segments};
pub use transcript::{Body, Card, Change, Entry, PREVIEW_CAP, Tone, Transcript, ask_title, shown};

/// A speaker, by its wire-protocol name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// One entry on the transcript channel.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptItem {
    /// A complete utterance, said by `role`. The only variant a
    /// [`Conversation`] holds.
    Message { role: Role, text: String },
    /// A streaming fragment of the assistant's reply in progress.
    /// Ephemeral: it exists on the event channel and nowhere else.
    AssistantDelta { text: String },
    /// A turn that came to grief instead of a reply. Not conversation
    /// content — the failure is news, not something anybody said.
    TurnFailed { reason: String },
    /// The model reached for a tool: an observed act, not an utterance.
    /// `arguments` is pretty-printed JSON, for reading.
    ToolCall { name: String, arguments: String },
    /// What the tool said back. `content` is capped for the transcript —
    /// the model gets the whole thing on the wire; the transcript gets
    /// enough to see what happened.
    ToolResult {
        name: String,
        ok: bool,
        content: String,
    },
    /// The persona is asking the user something and the turn is waiting
    /// on the answer. Ephemeral like a delta: it exists on the event
    /// channel and is never pushed into a [`Conversation`] — the settled
    /// record is [`QuestionResolved`](Self::QuestionResolved).
    Question { id: String, ask: Ask },
    /// A question and what the user answered — the settled record,
    /// appended and emitted exactly like a tool call and its result.
    QuestionResolved {
        id: String,
        ask: Ask,
        answer: Answer,
    },
}

/// What the persona needs from the user, in modality-free terms: the
/// backend says *what* it wants, never how to collect it — every input
/// modality is the frontend's. Tagged like [`TranscriptItem`], so a new
/// kind of question is a new variant, never a schema break.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ask {
    /// Where a repository should live. `prompt` is the persona's own
    /// wording of the question.
    Repository { prompt: String },
    /// The check command a feature build will be judged by. `prompt` is
    /// the build machinery's wording — the card is raised as the build's
    /// first act, never by the persona — and `proposal` is what
    /// detection prefilled, absent when no marker matched.
    Check {
        prompt: String,
        proposal: Option<String>,
    },
}

/// What the user answered. A decline is a first-class answer — the
/// persona reads it and reacts — not an error.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Answer {
    /// A repository location: a git URL, of which a path is one.
    Repository { url: String },
    /// The check command in force: the user confirmed or edited it.
    Check { command: String },
    /// The user would rather not say. For a check, the decline is the
    /// skip: the build runs on observation alone.
    Declined,
}

/// The event channel transcript items arrive on, backend to window.
pub const TRANSCRIPT_EVENT: &str = "transcript";

/// The canonical transcript: what was actually said, in order. One holder
/// per conversation — in the app, the backend. Completed [`Message`]s
/// only; deltas and failures never land here.
///
/// [`Message`]: TranscriptItem::Message
#[derive(Debug, Default)]
pub struct Conversation(Vec<TranscriptItem>);

impl Conversation {
    /// Appends `item`. The transcript only ever grows.
    pub fn push(&mut self, item: TranscriptItem) {
        self.0.push(item);
    }

    /// The transcript, oldest first.
    pub fn items(&self) -> std::slice::Iter<'_, TranscriptItem> {
        self.0.iter()
    }
}

/// How much of a tool's answer the transcript keeps. The model always
/// gets all of it; this cap is only for what a window shows and stores.
const TOOL_RESULT_CAP: usize = 2000;

/// The first `cap` characters of `text`, with a note about the rest —
/// or `text` itself when it already fits.
fn elide(text: &str, cap: usize) -> String {
    let total = text.chars().count();
    if total <= cap {
        return text.to_owned();
    }
    let mut kept: String = text.chars().take(cap).collect();
    kept.push_str(&format!("\n… ({} more characters elided)", total - cap));
    kept
}

impl TranscriptItem {
    /// The transcript's record of a call to `name`: its arguments
    /// pretty-printed when they parse, verbatim when they don't.
    #[must_use]
    pub fn tool_call(name: &str, arguments: &str) -> Self {
        let arguments = serde_json::from_str::<serde_json::Value>(arguments)
            .and_then(|value| serde_json::to_string_pretty(&value))
            .unwrap_or_else(|_| arguments.to_owned());
        Self::ToolCall {
            name: name.to_owned(),
            arguments,
        }
    }

    /// The transcript's record of what `name` answered, capped at
    /// [`TOOL_RESULT_CAP`] characters.
    #[must_use]
    pub fn tool_result(name: &str, result: &Result<serde_json::Value, String>) -> Self {
        let (ok, content) = match result {
            Ok(value) => (
                true,
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
            ),
            Err(reason) => (false, reason.clone()),
        };
        Self::ToolResult {
            name: name.to_owned(),
            ok,
            content: elide(&content, TOOL_RESULT_CAP),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: Role, text: &str) -> TranscriptItem {
        TranscriptItem::Message {
            role,
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_conversation_keeps_items_in_the_order_they_were_said() {
        let mut conversation = Conversation::default();
        conversation.push(message(Role::User, "first"));
        conversation.push(message(Role::Assistant, "second"));
        conversation.push(message(Role::User, "third"));

        let texts: Vec<_> = conversation
            .items()
            .map(|item| match item {
                TranscriptItem::Message { text, .. } => text.as_str(),
                _ => panic!("only messages were pushed"),
            })
            .collect();
        assert_eq!(texts, ["first", "second", "third"]);
    }

    #[test]
    fn a_fresh_conversation_has_nothing_to_say() {
        assert_eq!(Conversation::default().items().count(), 0);
    }

    /// The tag in the JSON is the forward-compatibility contract: a
    /// future item kind is a new tag value, and an old reader can see
    /// what it is not, rather than misread what it is.
    #[test]
    fn every_transcript_item_crosses_the_wire_tagged() {
        for (item, tag) in [
            (message(Role::User, "hello"), r#""kind":"message""#),
            (
                TranscriptItem::AssistantDelta {
                    text: "he".to_owned(),
                },
                r#""kind":"assistant_delta""#,
            ),
            (
                TranscriptItem::TurnFailed {
                    reason: "no route".to_owned(),
                },
                r#""kind":"turn_failed""#,
            ),
            (
                TranscriptItem::ToolCall {
                    name: "current_time".to_owned(),
                    arguments: "{}".to_owned(),
                },
                r#""kind":"tool_call""#,
            ),
            (
                TranscriptItem::ToolResult {
                    name: "current_time".to_owned(),
                    ok: true,
                    content: "{\"local\":\"…\"}".to_owned(),
                },
                r#""kind":"tool_result""#,
            ),
            (
                TranscriptItem::Question {
                    id: "1".to_owned(),
                    ask: Ask::Repository {
                        prompt: "Where should this live?".to_owned(),
                    },
                },
                r#""kind":"question""#,
            ),
            (
                TranscriptItem::QuestionResolved {
                    id: "1".to_owned(),
                    ask: Ask::Repository {
                        prompt: "Where should this live?".to_owned(),
                    },
                    answer: Answer::Repository {
                        url: "/tmp/wumpus.git".to_owned(),
                    },
                },
                r#""kind":"question_resolved""#,
            ),
            (
                TranscriptItem::QuestionResolved {
                    id: "2".to_owned(),
                    ask: Ask::Repository {
                        prompt: "Where?".to_owned(),
                    },
                    answer: Answer::Declined,
                },
                r#""kind":"question_resolved""#,
            ),
            (
                TranscriptItem::QuestionResolved {
                    id: "3".to_owned(),
                    ask: Ask::Check {
                        prompt: "What says green?".to_owned(),
                        proposal: Some("cargo test".to_owned()),
                    },
                    answer: Answer::Check {
                        command: "cargo test --workspace".to_owned(),
                    },
                },
                r#""kind":"question_resolved""#,
            ),
        ] {
            let wire = serde_json::to_string(&item).unwrap();
            assert!(wire.contains(tag), "{wire}");
            let received: TranscriptItem = serde_json::from_str(&wire).unwrap();
            assert_eq!(received, item);
        }
    }

    /// Asks and answers are tagged inside the item, one tag each.
    #[test]
    fn asks_and_answers_carry_their_own_tags() {
        let ask = serde_json::to_string(&Ask::Repository {
            prompt: "Where?".to_owned(),
        })
        .unwrap();
        assert_eq!(ask, r#"{"kind":"repository","prompt":"Where?"}"#);
        let check = serde_json::to_string(&Ask::Check {
            prompt: "What says green?".to_owned(),
            proposal: None,
        })
        .unwrap();
        assert_eq!(
            check,
            r#"{"kind":"check","prompt":"What says green?","proposal":null}"#
        );
        assert_eq!(
            serde_json::to_string(&Answer::Check {
                command: "make test".to_owned(),
            })
            .unwrap(),
            r#"{"kind":"check","command":"make test"}"#
        );
        assert_eq!(
            serde_json::to_string(&Answer::Declined).unwrap(),
            r#"{"kind":"declined"}"#
        );
    }

    /// A newer build's question kind does not decode here: tagged
    /// enums without an `#[serde(other)]` arm fail on an unknown
    /// tag. Acceptable — both ends of the barrier ship together —
    /// and documented rather than papered over with an `Unknown`.
    #[test]
    fn an_ask_kind_from_a_newer_build_fails_to_decode() {
        assert!(serde_json::from_str::<Ask>(r#"{"kind":"color","prompt":"?"}"#).is_err());
        assert!(serde_json::from_str::<Answer>(r#"{"kind":"color","value":"red"}"#).is_err());
    }

    #[test]
    fn both_roles_speak_their_wire_protocol_names() {
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            r#""assistant""#
        );
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), r#""user""#);
    }

    #[test]
    fn a_tool_call_item_pretty_prints_arguments_that_parse() {
        let item = TranscriptItem::tool_call("f", r#"{"a":1}"#);
        assert_eq!(
            item,
            TranscriptItem::ToolCall {
                name: "f".to_owned(),
                arguments: "{\n  \"a\": 1\n}".to_owned(),
            }
        );
    }

    #[test]
    fn unparseable_arguments_reach_the_transcript_verbatim() {
        let item = TranscriptItem::tool_call("f", "{\"a\":");
        assert!(matches!(
            item,
            TranscriptItem::ToolCall { arguments, .. } if arguments == "{\"a\":"
        ));
    }

    #[test]
    fn a_result_within_the_cap_is_kept_whole() {
        let item = TranscriptItem::tool_result("f", &Ok(serde_json::json!({ "a": 1 })));
        assert_eq!(
            item,
            TranscriptItem::ToolResult {
                name: "f".to_owned(),
                ok: true,
                content: "{\n  \"a\": 1\n}".to_owned(),
            }
        );
    }

    #[test]
    fn a_failed_result_carries_the_tools_words_and_is_not_ok() {
        let item = TranscriptItem::tool_result("f", &Err("no such day".to_owned()));
        assert_eq!(
            item,
            TranscriptItem::ToolResult {
                name: "f".to_owned(),
                ok: false,
                content: "no such day".to_owned(),
            }
        );
    }

    /// The boundary: exactly at the cap nothing is cut; one past it,
    /// the transcript keeps the cap's worth plus an elision note.
    #[test]
    fn the_transcript_cap_cuts_only_past_the_boundary() {
        let at_cap = "x".repeat(TOOL_RESULT_CAP);
        let TranscriptItem::ToolResult { content, .. } =
            TranscriptItem::tool_result("f", &Err(at_cap.clone()))
        else {
            panic!("a result item");
        };
        assert_eq!(content, at_cap);

        let over = "x".repeat(TOOL_RESULT_CAP + 1);
        let TranscriptItem::ToolResult { content, .. } =
            TranscriptItem::tool_result("f", &Err(over))
        else {
            panic!("a result item");
        };
        assert_eq!(
            content,
            format!(
                "{}\n… (1 more characters elided)",
                "x".repeat(TOOL_RESULT_CAP)
            )
        );
    }
}
