//! The `claude` CLI's stream-json protocol: one JSON event per line of
//! stdout, and the reading of each into the [`Update`]s it carries.
//!
//! The protocol has two layers. The outer event is the Claude Agent
//! SDK's `SDKMessage`, tagged by `type`: a `system` event whose
//! `init` subtype opens the session, `assistant` and `user` turns, and
//! a `result` event that closes the run with its outcome and bill. An
//! `assistant` event's `message` is a Messages API message: content
//! blocks tagged by `type`, `text` and `tool_use` among them.
//!
//! The model here is only the fields Epik reads, and every tagged
//! enum has an `Other` arm. An event or block the CLI invents later
//! decodes to that arm rather than failing, so whatever a newer CLI
//! says that this reading does not know, it says to nobody; the
//! fields it adds to shapes this reading does know are ignored, as
//! serde ignores unknown fields. A line that is not JSON, or JSON with
//! no `type`, is nothing at all.

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::agent::Update;

/// Reads one stream-json line into the [`Update`]s it carries —
/// usually one, but an assistant message holds a list of content
/// blocks, and every modelled block is an update.
pub(super) fn interpret(line: &str) -> Vec<Update> {
    let Ok(event) = serde_json::from_str::<Event>(line) else {
        return Vec::new();
    };
    match event {
        Event::System {
            subtype,
            session_id: Some(id),
            model: Some(model),
        } if subtype == "init" => vec![Update::Session { id, model }],
        Event::Assistant { message } => message
            .content
            .into_iter()
            .filter_map(|block| match block {
                Block::Text { text } => Some(Update::Text { text }),
                Block::ToolUse { name } => Some(Update::ToolUse { name }),
                Block::Other => None,
            })
            .collect(),
        Event::Result {
            is_error,
            result,
            errors,
            total_cost_usd,
            usage,
        } => {
            let usage = usage.unwrap_or_default();
            vec![Update::Result {
                ok: !is_error.unwrap_or(false),
                // A success reports under `result`; an error's words
                // arrive as an `errors` list instead.
                text: result.unwrap_or_else(|| errors.join("; ")),
                cost_usd: total_cost_usd,
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
            }]
        }
        Event::System { .. } | Event::Other => Vec::new(),
    }
}

/// One event, as the CLI writes one per line.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    System {
        #[serde(default)]
        subtype: String,
        session_id: Option<String>,
        model: Option<String>,
    },
    Assistant {
        message: Message,
    },
    Result {
        is_error: Option<bool>,
        result: Option<String>,
        #[serde(default)]
        errors: Vec<String>,
        total_cost_usd: Option<Decimal>,
        usage: Option<Usage>,
    },
    /// Every other event: user turns, hooks, stream events, and
    /// whatever comes next.
    #[serde(other)]
    Other,
}

/// A Messages API message: its content blocks.
#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    content: Vec<Block>,
}

/// One content block of a message.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
    Text {
        text: String,
    },
    ToolUse {
        name: String,
    },
    /// Every other block: thinking, images, tool results, and
    /// whatever comes next.
    #[serde(other)]
    Other,
}

/// The tokens a result event bills for.
#[derive(Default, Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A complete captured session; see fixtures/README.md for
    /// provenance.
    const SESSION: &str = include_str!("fixtures/session.jsonl");
    const RESULT_ERROR: &str = include_str!("fixtures/result_error.json");

    fn updates(stream: &str) -> Vec<Update> {
        stream.lines().flat_map(interpret).collect()
    }

    #[test]
    fn a_captured_session_interprets_to_its_four_updates() {
        let updates = updates(SESSION);
        assert_eq!(
            updates,
            [
                Update::Session {
                    id: "ed2b50bb-bb39-4b32-9326-2102013dbdd1".to_owned(),
                    model: "claude-fable-5".to_owned(),
                },
                Update::ToolUse {
                    name: "Write".to_owned(),
                },
                Update::Text {
                    text: "Created `hello.txt` in the working directory containing exactly \
                           `hello`."
                        .to_owned(),
                },
                Update::Result {
                    ok: true,
                    text: "Created `hello.txt` in the working directory containing exactly \
                           `hello`."
                        .to_owned(),
                    cost_usd: Some(Decimal::new(316_251, 6)),
                    input_tokens: Some(4),
                    output_tokens: Some(181),
                },
            ],
            "hooks, thinking, and tool results interpret to nothing"
        );
    }

    #[test]
    fn an_error_result_decodes_with_its_words_intact() {
        let updates = interpret(RESULT_ERROR);
        let [
            Update::Result {
                ok, text, cost_usd, ..
            },
        ] = updates.as_slice()
        else {
            panic!("one result: {updates:?}");
        };
        assert!(!ok);
        assert_eq!(text, "Reached maximum number of turns (1)");
        assert!(cost_usd.is_some());
    }

    #[test]
    fn a_result_without_a_bill_still_settles_the_outcome() {
        assert_eq!(
            interpret(r#"{"type":"result","is_error":false,"result":"done"}"#),
            [Update::Result {
                ok: true,
                text: "done".to_owned(),
                cost_usd: None,
                input_tokens: None,
                output_tokens: None,
            }]
        );
    }

    #[test]
    fn lines_that_are_not_stream_json_interpret_to_nothing() {
        for line in [
            "",
            "not json at all",
            r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#,
            r#"{"type":"system","subtype":"hook_started","hook_name":"SessionStart"}"#,
            r#"{"type":"celebration","confetti":true}"#,
            r#"{"no_type_at_all":1}"#,
        ] {
            assert_eq!(interpret(line), [], "{line:?}");
        }
    }

    #[test]
    fn unmodelled_fields_and_blocks_are_absorbed_not_fatal() {
        let line = r#"{"type":"assistant","message":{"content":[
            {"type":"thinking","thinking":"hmm"},
            {"type":"hologram","frames":3},
            {"type":"text","text":"onwards","novelty":42}],
            "future_field":{"deep":[1,2,3]}},"another":"one"}"#;
        assert_eq!(
            interpret(line),
            [Update::Text {
                text: "onwards".to_owned()
            }]
        );
    }

    #[test]
    fn an_update_round_trips_through_serde() {
        let update = Update::Result {
            ok: false,
            text: "ran aground".to_owned(),
            cost_usd: Some(Decimal::new(25, 2)),
            input_tokens: None,
            output_tokens: Some(7),
        };
        let wire = serde_json::to_string(&update).unwrap();
        assert_eq!(serde_json::from_str::<Update>(&wire).unwrap(), update);
    }
}
