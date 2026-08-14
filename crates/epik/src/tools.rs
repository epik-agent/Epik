//! Tools the model can call, and the loop that runs a turn through them.
//!
//! Deliberately dumb: a [`Tool`] is a name, a description, a parameters
//! schema, and a handler; a [`Registry`] is a Vec of them. Everything a
//! tool can get wrong — an unknown name, arguments that don't parse, a
//! handler that fails — becomes an Err *result*, words fed back to the
//! model as the tool's output. Never a crash, never a turn failure.
//!
//! [`run`] is the turn: call the model, dispatch whatever tools it asks
//! for, tell it what happened, and repeat until it answers in text. A
//! later slice registers the git and GitHub verbs here; today the only
//! resident is [`current_time`], the wire's canary.

#[cfg(feature = "native")]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::chat::ToolSpec;
#[cfg(feature = "native")]
use crate::chat::{ChatError, ChatMessage, Client, Reply, ToolCall, TranscriptItem};

/// What a tool does with its parsed arguments: a JSON answer, or its
/// failure in words — which the model reads, so say what went wrong.
pub type Handler = Box<dyn Fn(&serde_json::Value) -> Result<serde_json::Value, String>>;

/// One tool: what to call it, what to tell the model about it, the JSON
/// Schema of its arguments, and what it does.
pub struct Tool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    handler: Handler,
}

impl Tool {
    #[must_use]
    pub fn new(
        name: &str,
        description: &str,
        parameters: serde_json::Value,
        handler: Handler,
    ) -> Self {
        Self {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters,
            handler,
        }
    }
}

/// The tools on offer this turn.
#[derive(Default)]
pub struct Registry(Vec<Tool>);

impl Registry {
    /// The registry every real host starts from: the built-ins, today
    /// exactly [`current_time`].
    #[cfg(feature = "native")]
    #[must_use]
    pub fn standard() -> Self {
        let mut registry = Self::default();
        registry.register(current_time());
        registry
    }

    pub fn register(&mut self, tool: Tool) {
        self.0.push(tool);
    }

    /// The request's tools array.
    #[must_use]
    pub fn to_wire(&self) -> Vec<ToolSpec> {
        self.0
            .iter()
            .map(|tool| {
                ToolSpec::function(
                    tool.name.clone(),
                    tool.description.clone(),
                    tool.parameters.clone(),
                )
            })
            .collect()
    }

    /// Runs `name` on `arguments` — the raw string the model wrote,
    /// parsed here so handlers see JSON. A model that sends nothing at
    /// all for a no-argument tool means `{}`. The Err is the tool's
    /// output too: it goes back to the model, never up as a failure.
    pub fn dispatch(&self, name: &str, arguments: &str) -> Result<serde_json::Value, String> {
        let tool = self
            .0
            .iter()
            .find(|tool| tool.name == name)
            .ok_or_else(|| format!("there is no tool named {name}"))?;
        let arguments = if arguments.trim().is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(arguments)
                .map_err(|error| format!("the arguments are not valid JSON: {error}"))?
        };
        (tool.handler)(&arguments)
    }
}

/// The most rounds of tool calls one turn may take before it is a turn
/// failure. A model that hasn't answered in this many is not going to.
pub const MAX_TOOL_ITERATIONS: usize = 16;

/// One whole turn, run until the model answers in text.
///
/// Calls [`Client::reply`] with the registry's tools on offer; when the
/// reply is tool calls, dispatches each in order, reports every call and
/// its result to `observer`, appends what happened to the working
/// messages, and asks again. Deltas stream to `on_delta` as ever. `stop`
/// is checked between deltas (inside `reply`) and between iterations —
/// once set, the turn stands down quietly with what text it has.
///
/// # Errors
///
/// Everything [`Client::reply`] can die of, plus [`ChatError::ToolLoop`]
/// when [`MAX_TOOL_ITERATIONS`] rounds pass without an answer.
#[cfg(feature = "native")]
pub fn run(
    client: &Client,
    system: &str,
    transcript: &[TranscriptItem],
    registry: &Registry,
    mut on_delta: impl FnMut(&str),
    mut observer: impl FnMut(&ToolCall, &Result<serde_json::Value, String>),
    stop: &AtomicBool,
) -> Result<String, ChatError> {
    let tools = registry.to_wire();
    let mut messages = ChatMessage::from_transcript(transcript);
    for _ in 0..MAX_TOOL_ITERATIONS {
        if stop.load(Ordering::Relaxed) {
            return Ok(String::new());
        }
        match client.reply(system, &messages, &tools, &mut on_delta, stop)? {
            Reply::Text(text) => return Ok(text),
            Reply::ToolCalls(calls) => {
                messages.push(ChatMessage::ToolCalls(calls.clone()));
                for call in calls {
                    let result = registry.dispatch(&call.name, &call.arguments);
                    observer(&call, &result);
                    let content = match &result {
                        Ok(value) => value.to_string(),
                        Err(reason) => reason.clone(),
                    };
                    messages.push(ChatMessage::ToolResult {
                        id: call.id,
                        content,
                    });
                }
            }
        }
    }
    Err(ChatError::ToolLoop {
        limit: MAX_TOOL_ITERATIONS,
    })
}

/// The built-in canary: no arguments, the local date-time and UTC offset
/// as JSON. It proves the round trip, and afterwards it tells the time.
#[cfg(feature = "native")]
#[must_use]
pub fn current_time() -> Tool {
    Tool::new(
        "current_time",
        "The current local date and time, with the UTC offset.",
        serde_json::json!({ "type": "object", "properties": {} }),
        Box::new(|_| {
            let now = chrono::Local::now();
            Ok(serde_json::json!({
                "local": now.format("%Y-%m-%dT%H:%M:%S").to_string(),
                "utc_offset": now.offset().to_string(),
            }))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo() -> Tool {
        Tool::new(
            "echo",
            "Says the argument back.",
            serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
            }),
            Box::new(|arguments| Ok(serde_json::json!({ "echo": arguments["text"] }))),
        )
    }

    #[test]
    fn dispatch_hands_parsed_arguments_to_the_handler() {
        let mut registry = Registry::default();
        registry.register(echo());
        assert_eq!(
            registry.dispatch("echo", r#"{"text":"hi"}"#),
            Ok(serde_json::json!({ "echo": "hi" }))
        );
    }

    #[test]
    fn an_unknown_tool_is_an_err_result_not_a_panic() {
        let error = Registry::default().dispatch("nonesuch", "{}").unwrap_err();
        assert!(error.contains("nonesuch"), "{error}");
    }

    #[test]
    fn malformed_arguments_are_an_err_result_not_a_panic() {
        let mut registry = Registry::default();
        registry.register(echo());
        let error = registry.dispatch("echo", "{\"text\":").unwrap_err();
        assert!(error.contains("not valid JSON"), "{error}");
    }

    #[test]
    fn empty_arguments_mean_an_empty_object() {
        let mut registry = Registry::default();
        registry.register(Tool::new(
            "nullary",
            "Takes nothing.",
            serde_json::json!({ "type": "object", "properties": {} }),
            Box::new(|arguments| {
                assert_eq!(arguments, &serde_json::json!({}));
                Ok(serde_json::json!("ran"))
            }),
        ));
        assert_eq!(
            registry.dispatch("nullary", ""),
            Ok(serde_json::json!("ran"))
        );
    }

    #[test]
    fn the_registry_advertises_its_tools_in_the_wire_shape() {
        let mut registry = Registry::default();
        registry.register(echo());
        let wire = serde_json::to_value(registry.to_wire()).unwrap();
        assert_eq!(wire[0]["type"], "function");
        assert_eq!(wire[0]["function"]["name"], "echo");
        assert!(wire[0]["function"]["parameters"].is_object());
    }

    /// The loop end to end, against the scripted model: a real client, a
    /// loopback socket, and a script the test controls exactly.
    #[cfg(feature = "scripted")]
    mod looped {
        use super::*;
        use crate::chat::scripted::{Fragment, Scripted, Turn};
        use crate::chat::{Role, TranscriptItem};

        fn transcript() -> Vec<TranscriptItem> {
            vec![TranscriptItem::Message {
                role: Role::User,
                text: "go".to_owned(),
            }]
        }

        fn run_against(
            model: &Scripted,
            registry: &Registry,
            observer: impl FnMut(&ToolCall, &Result<serde_json::Value, String>),
        ) -> Result<String, ChatError> {
            let client = Client::new(model.base_url(), "scripted".to_owned(), None);
            run(
                &client,
                "system",
                &transcript(),
                registry,
                |_| {},
                observer,
                &AtomicBool::new(false),
            )
        }

        #[test]
        fn a_tool_call_turn_dispatches_and_reports_back_to_the_model() {
            let model = Scripted::spawn(vec![
                Turn::ToolCalls(vec![
                    vec![Fragment::open(0, "call_1", "echo", "{\"text\":")],
                    vec![Fragment::more(0, "\"hi\"}")],
                ]),
                Turn::text(&["done"]),
            ]);
            let mut registry = Registry::default();
            registry.register(echo());
            let mut observed = Vec::new();

            let text = run_against(&model, &registry, |call, result| {
                observed.push((call.clone(), result.clone()));
            })
            .unwrap();

            assert_eq!(text, "done");
            assert_eq!(
                observed,
                [(
                    ToolCall {
                        id: "call_1".to_owned(),
                        name: "echo".to_owned(),
                        arguments: "{\"text\":\"hi\"}".to_owned(),
                    },
                    Ok(serde_json::json!({ "echo": "hi" })),
                )]
            );

            let requests = model.requests();
            assert_eq!(requests.len(), 2);
            assert_eq!(
                requests[0]["tools"][0]["function"]["name"], "echo",
                "the tools ride the first request"
            );
            let messages = requests[1]["messages"].as_array().unwrap();
            assert_eq!(
                &messages[messages.len() - 2..],
                &[
                    serde_json::json!({
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "echo", "arguments": "{\"text\":\"hi\"}" },
                        }],
                    }),
                    serde_json::json!({
                        "role": "tool",
                        "content": "{\"echo\":\"hi\"}",
                        "tool_call_id": "call_1",
                    }),
                ],
                "the second request carries the call and its answer, in order"
            );
        }

        #[test]
        fn two_tools_in_one_turn_dispatch_in_index_order() {
            let model = Scripted::spawn(vec![
                Turn::ToolCalls(vec![vec![
                    Fragment::open(0, "call_a", "echo", "{\"text\":\"one\"}"),
                    Fragment::open(1, "call_b", "nonesuch", "{}"),
                ]]),
                Turn::text(&["done"]),
            ]);
            let mut registry = Registry::default();
            registry.register(echo());
            let mut observed = Vec::new();

            let text = run_against(&model, &registry, |call, result| {
                observed.push((call.name.clone(), result.is_ok()));
            })
            .unwrap();

            assert_eq!(text, "done");
            assert_eq!(
                observed,
                [("echo".to_owned(), true), ("nonesuch".to_owned(), false)],
                "the observer fires in dispatch order, failures included"
            );

            let messages = model.requests()[1]["messages"].as_array().unwrap().clone();
            let tool_answers: Vec<_> = messages
                .iter()
                .filter(|message| message["role"] == "tool")
                .map(|message| message["tool_call_id"].as_str().unwrap().to_owned())
                .collect();
            assert_eq!(tool_answers, ["call_a", "call_b"]);
        }

        /// The unknown tool's Err went back to the model as content, not
        /// up as a failure — the turn still ended in text.
        #[test]
        fn a_failed_dispatch_feeds_the_model_the_words() {
            let model = Scripted::spawn(vec![
                Turn::ToolCalls(vec![vec![Fragment::open(0, "call_1", "nonesuch", "{}")]]),
                Turn::text(&["ok"]),
            ]);

            let text = run_against(&model, &Registry::default(), |_, _| {}).unwrap();

            assert_eq!(text, "ok");
            let messages = model.requests()[1]["messages"].as_array().unwrap().clone();
            let answer = messages.last().unwrap();
            assert_eq!(answer["role"], "tool");
            assert!(
                answer["content"].as_str().unwrap().contains("nonesuch"),
                "{answer}"
            );
        }

        #[test]
        fn a_turn_that_never_stops_calling_tools_is_a_turn_failure() {
            let script = std::iter::repeat_n(
                Turn::ToolCalls(vec![vec![Fragment::open(
                    0,
                    "call_1",
                    "echo",
                    "{\"text\":\"x\"}",
                )]]),
                MAX_TOOL_ITERATIONS,
            )
            .collect();
            let model = Scripted::spawn(script);
            let mut registry = Registry::default();
            registry.register(echo());

            let error = run_against(&model, &registry, |_, _| {}).unwrap_err();

            assert_eq!(
                error,
                ChatError::ToolLoop {
                    limit: MAX_TOOL_ITERATIONS
                }
            );
            assert!(error.to_string().contains("16"), "{error}");
            assert_eq!(model.requests().len(), MAX_TOOL_ITERATIONS);
        }

        #[test]
        fn a_plain_text_turn_calls_no_tools_and_asks_once() {
            let model = Scripted::spawn(vec![Turn::text(&["Hel", "lo"])]);

            let text = run_against(&model, &Registry::standard(), |_, _| {
                panic!("no tool was called");
            })
            .unwrap();

            assert_eq!(text, "Hello");
            assert_eq!(model.requests().len(), 1);
        }
    }

    #[cfg(feature = "native")]
    #[test]
    fn current_time_answers_in_parseable_fields() {
        let registry = Registry::standard();
        let answer = registry.dispatch("current_time", "{}").unwrap();
        let local = answer["local"].as_str().unwrap();
        assert!(local.contains('T'), "{local}");
        let offset = answer["utc_offset"].as_str().unwrap();
        assert!(
            offset.starts_with('+') || offset.starts_with('-'),
            "{offset}"
        );
    }
}
