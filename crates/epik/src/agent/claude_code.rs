//! Claude Code, the first real engine: an [`Agent`] whose child is the
//! `claude` CLI in headless print mode, and a typed reading of what it
//! says.
//!
//! [`ClaudeCode`] is data — a binary path, a working directory, a
//! prompt, and optionally a model and an API key. Its Task pins Epik's
//! settled isolation posture: the agent runs with Epik's configuration
//! (`--settings '{}'`, `--strict-mcp-config`), never the machine-wide
//! Claude Code setup, and with `--dangerously-skip-permissions` —
//! deliberate autonomy, because events only flow *out* of an agent;
//! there is no channel to answer a permission prompt, so the agent must
//! never ask one. Its containment is the working directory and the
//! environment it is handed. The prompt rides the Task's stdin payload,
//! not argv: stdin is the CLI's file-like channel, the right fit for
//! multi-line prompts of arbitrary length.
//!
//! The CLI's stream-json arrives as the generic `Stdout { line }`
//! events; the generic channel stays generic, and [`interpret`] layers
//! the engine's meaning on top as a pure function over lines — modelling
//! only the fields Epik reads, absorbing everything else. A line that is
//! not JSON, or JSON of an unmodelled shape, interprets to nothing;
//! nothing a newer CLI says can break decoding.

use serde::{Deserialize, Serialize};

#[cfg(feature = "native")]
use super::{Agent, Secret, Task};

/// One thing Epik understands from a Claude Code stream, in the order
/// said. The typed view is deliberately small: enough to narrate
/// progress and settle the outcome, no more.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum Update {
    /// The session opened: its id, and the model actually speaking.
    Session { id: String, model: String },
    /// The engine said something — progress narration.
    Text { text: String },
    /// The engine reached for a tool: enough to say "it's editing
    /// files" or "it's running tests", no more.
    ToolUse { name: String },
    /// The terminal line: how the run ended, in the engine's own words,
    /// with the bill when one was presented.
    Result {
        ok: bool,
        text: String,
        cost_usd: Option<f64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
}

/// Reads one stream-json line into the [`Update`]s it carries — usually
/// one, but an assistant message holds a list of content blocks, and
/// every modelled block is an update. Unmodelled shapes — hooks,
/// thinking, tool results, whatever a newer CLI invents — interpret to
/// an empty Vec, never an error.
#[must_use]
pub fn interpret(line: &str) -> Vec<Update> {
    let Ok(line) = serde_json::from_str::<Line>(line) else {
        return Vec::new();
    };
    match line.kind.as_str() {
        "system" if line.subtype == "init" => match (line.session_id, line.model) {
            (Some(id), Some(model)) => vec![Update::Session { id, model }],
            _ => Vec::new(),
        },
        "assistant" => line
            .message
            .map(|message| message.content)
            .unwrap_or_default()
            .into_iter()
            .filter_map(block_update)
            .collect(),
        "result" => {
            let usage = line.usage.unwrap_or_default();
            vec![Update::Result {
                ok: !line.is_error.unwrap_or(false),
                // A success reports under `result`; an error's words
                // arrive as an `errors` list instead.
                text: line.result.unwrap_or_else(|| line.errors.join("; ")),
                cost_usd: line.total_cost_usd,
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
            }]
        }
        _ => Vec::new(),
    }
}

fn block_update(block: Block) -> Option<Update> {
    match block.kind.as_str() {
        "text" => block.text.map(|text| Update::Text { text }),
        "tool_use" => block.name.map(|name| Update::ToolUse { name }),
        _ => None,
    }
}

// The wire view: only the fields Epik reads, everything else absorbed.

#[derive(Default, Deserialize)]
struct Line {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    subtype: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    message: Option<Message>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    errors: Vec<String>,
    #[serde(default)]
    total_cost_usd: Option<f64>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Default, Deserialize)]
struct Message {
    #[serde(default)]
    content: Vec<Block>,
}

#[derive(Default, Deserialize)]
struct Block {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Default, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

/// The `claude` CLI as an Agent. Data only: the caller supplies the
/// binary's absolute path — no PATH divination — and resolves any API
/// key itself; this type never touches the keystore. With no key, the
/// CLI's own logged-in auth applies; both are legitimate.
#[cfg(feature = "native")]
#[derive(Clone, Debug)]
pub struct ClaudeCode {
    pub binary: String,
    pub cwd: String,
    pub prompt: String,
    pub model: Option<String>,
    pub api_key: Option<Secret>,
}

#[cfg(feature = "native")]
impl Agent for ClaudeCode {
    fn task(&self) -> Task {
        let mut argv = vec![
            self.binary.clone(),
            "-p".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            // Stream output requires it.
            "--verbose".to_owned(),
        ];
        if let Some(model) = &self.model {
            argv.push("--model".to_owned());
            argv.push(model.clone());
        }
        argv.extend(
            [
                // Epik's isolation posture: the agent runs with Epik's
                // configuration, not the user's machine-wide setup...
                "--settings",
                "{}",
                "--strict-mcp-config",
                // ...and with deliberate autonomy: no channel exists to
                // answer a prompt, so none may be asked.
                "--dangerously-skip-permissions",
            ]
            .map(str::to_owned),
        );
        Task {
            argv,
            env: self
                .api_key
                .iter()
                .map(|key| ("ANTHROPIC_API_KEY".to_owned(), key.clone()))
                .collect(),
            cwd: self.cwd.clone(),
            // The prompt is stdin, never an argument: file-like, any
            // length, any number of lines.
            stdin: Some(self.prompt.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A complete captured session; see fixtures/README.md for
    /// provenance.
    const SESSION: &str = include_str!("claude_code/fixtures/session.jsonl");
    const RESULT_ERROR: &str = include_str!("claude_code/fixtures/result_error.json");

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
                    cost_usd: Some(0.316_251),
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
    fn lines_that_are_not_stream_json_interpret_to_nothing() {
        for line in [
            "",
            "not json at all",
            r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#,
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
            cost_usd: Some(0.25),
            input_tokens: None,
            output_tokens: Some(7),
        };
        let wire = serde_json::to_string(&update).unwrap();
        assert_eq!(serde_json::from_str::<Update>(&wire).unwrap(), update);
    }

    #[cfg(feature = "native")]
    mod task {
        use super::super::*;
        use crate::chat::ANTHROPIC_MODEL;

        fn agent() -> ClaudeCode {
            ClaudeCode {
                binary: "/opt/homebrew/bin/claude".to_owned(),
                cwd: "/work/repo".to_owned(),
                prompt: "create a file\nnamed hello.txt".to_owned(),
                model: Some(ANTHROPIC_MODEL.to_owned()),
                api_key: Some(Secret::from("sk-ant-hush-hush")),
            }
        }

        /// The isolation and skip-permissions flags are load-bearing;
        /// the whole argv is pinned, and the prompt is not in it.
        #[test]
        fn the_argv_is_exactly_the_settled_posture() {
            assert_eq!(
                agent().task().argv,
                [
                    "/opt/homebrew/bin/claude",
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--model",
                    ANTHROPIC_MODEL,
                    "--settings",
                    "{}",
                    "--strict-mcp-config",
                    "--dangerously-skip-permissions",
                ]
            );

            let mut without_model = agent();
            without_model.model = None;
            assert_eq!(
                without_model.task().argv,
                [
                    "/opt/homebrew/bin/claude",
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--settings",
                    "{}",
                    "--strict-mcp-config",
                    "--dangerously-skip-permissions",
                ]
            );
        }

        #[test]
        fn the_prompt_rides_stdin_and_the_cwd_is_the_given_one() {
            let task = agent().task();
            assert_eq!(
                task.stdin.as_deref(),
                Some("create a file\nnamed hello.txt")
            );
            assert_eq!(task.cwd, "/work/repo");
        }

        #[test]
        fn the_key_is_in_the_env_exactly_when_supplied() {
            let task = agent().task();
            assert_eq!(task.env.len(), 1);
            assert_eq!(
                task.env.get("ANTHROPIC_API_KEY"),
                Some(&Secret::from("sk-ant-hush-hush"))
            );

            let mut logged_in = agent();
            logged_in.api_key = None;
            assert!(
                logged_in.task().env.is_empty(),
                "no key means the CLI's own auth"
            );
        }

        #[test]
        fn no_debug_output_carries_the_keys_bytes() {
            let agent = agent();
            for debugged in [format!("{agent:?}"), format!("{:?}", agent.task())] {
                assert!(!debugged.contains("sk-ant-hush-hush"), "{debugged}");
            }
        }
    }
}
