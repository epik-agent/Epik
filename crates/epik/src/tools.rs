//! The tool registry: what a model can do, in one place.
//!
//! A tool is a name, a description, a JSON schema for its arguments, and a
//! handler. The registry is populated once at construction and surfaced to
//! any door that asks: the chat loop renders [`Tool`]s into a provider
//! request and calls handlers with the argument JSON the model produced; a
//! future `epik-mcp` binary lists the same registry to another client. The
//! interface is transport-agnostic by shape — nothing in it says whether a
//! handler is a local function or a future MCP client call, which is what
//! lets Linear and its like arrive later with zero shim-writing.
//!
//! Schemas are data, serialized into the provider request verbatim. The
//! registry does not validate a model's arguments beyond what
//! deserialization into the handler's input demands.
//!
//! Refusals are vocabulary: [`Error`] is a value a door renders, an unknown
//! tool name is a typed refusal, and a tool's own failure travels through
//! still typed rather than flattened into prose. Handlers are synchronous,
//! like everything else in the library, and the registry itself needs no
//! token and no network — those belong to the tools that use them.

use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::chat::ToolDeclaration;

/// A tool's face: what a door shows a model. The handler stays behind it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Tool {
    /// What a model calls it by.
    pub name: String,
    /// What a model reads to decide whether to call it.
    pub description: String,
    /// A JSON schema for the arguments, as data. It goes into the provider
    /// request verbatim; the registry never interprets it.
    pub schema: Value,
}

impl Tool {
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>, schema: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            schema,
        }
    }
}

/// A tool's face rendered for a provider request: same name, same words,
/// the schema carried over as the parameters.
impl From<&Tool> for ToolDeclaration {
    fn from(tool: &Tool) -> Self {
        Self {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.schema.clone(),
        }
    }
}

/// Why a call did not answer, as a value. Doors render these; nothing here
/// is meant to be scraped out of a log.
#[derive(Debug)]
pub enum Error {
    /// The refusal: no tool by this name is registered.
    Unknown { name: String },
    /// The arguments would not deserialize into what the handler takes —
    /// which is the only validation the registry does. Models produce
    /// malformed JSON and misshapen arguments alike; both land here, worded
    /// for the model to try again.
    Arguments { name: String, error: String },
    /// The tool ran and could not do it. The error is the tool's own, still
    /// a value: a door renders it, and a caller that knows the tool can
    /// downcast to its vocabulary.
    Failed {
        name: String,
        error: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { name } => write!(f, "no tool named {name} is registered"),
            Self::Arguments { name, error } => {
                write!(f, "{name}: the arguments do not fit: {error}")
            }
            Self::Failed { name, error } => write!(f, "{name}: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Failed { error, .. } => {
                let error: &(dyn std::error::Error + 'static) = error.as_ref();
                Some(error)
            }
            Self::Unknown { .. } | Self::Arguments { .. } => None,
        }
    }
}

/// Erased and private: only [`Registry::register`] builds one, so `Unknown`
/// stays the registry's word alone — a handler has no way to say it.
type Handler = Box<dyn FnMut(Value) -> Result<Value, Error> + Send>;

struct Entry {
    tool: Tool,
    handler: Handler,
}

/// The tools, with their handlers: populated once, surfaced to every door.
///
/// `Send`, so a host can build it on one thread and hand it to the thread
/// that runs the chat loop.
#[derive(Default)]
pub struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Registers `tool`, backed by `handler`.
    ///
    /// The handler's input is deserialized from the argument JSON a model
    /// produced, and its output is serialized back into the JSON the model
    /// reads. [`Value`] at either end opts out of the typing: a passthrough
    /// handler — a future MCP client call, say — takes the arguments as
    /// they came and answers with whatever it got.
    ///
    /// # Panics
    ///
    /// Panics when a tool named `tool.name` is already registered. The
    /// registry is populated once at construction, so a duplicate is a
    /// programming error to fix, not a state to render — and never a tool
    /// silently shadowed.
    pub fn register<I, O, E>(
        &mut self,
        tool: Tool,
        mut handler: impl FnMut(I) -> Result<O, E> + Send + 'static,
    ) where
        I: DeserializeOwned,
        O: Serialize,
        E: std::error::Error + Send + Sync + 'static,
    {
        assert!(
            !self
                .entries
                .iter()
                .any(|entry| entry.tool.name == tool.name),
            "a tool named {:?} is already registered",
            tool.name
        );
        let name = tool.name.clone();
        let handler: Handler = Box::new(move |arguments| {
            let input = serde_json::from_value(arguments).map_err(|error| Error::Arguments {
                name: name.clone(),
                error: error.to_string(),
            })?;
            let output = handler(input).map_err(|error| Error::Failed {
                name: name.clone(),
                error: Box::new(error),
            })?;
            // An output that will not serialize is the tool's own bug, and
            // it is reported as the tool's own failure.
            serde_json::to_value(output).map_err(|error| Error::Failed {
                name: name.clone(),
                error: Box::new(error),
            })
        });
        self.entries.push(Entry { tool, handler });
    }

    /// Every registered tool, in registration order — the order a door shows
    /// them to a model.
    pub fn tools(&self) -> impl Iterator<Item = &Tool> {
        self.entries.iter().map(|entry| &entry.tool)
    }

    /// The tools as a provider request carries them, ready for a model's
    /// `use_tools`.
    #[must_use]
    pub fn declarations(&self) -> Vec<ToolDeclaration> {
        self.tools().map(Into::into).collect()
    }

    /// Calls the tool named `name` with the JSON `arguments` a model
    /// produced, and answers with the handler's output as JSON, ready to go
    /// back to the model.
    ///
    /// # Errors
    ///
    /// [`Error::Unknown`] when no tool has that name; [`Error::Arguments`]
    /// when `arguments` is not JSON, or does not deserialize into what the
    /// handler takes; [`Error::Failed`] when the tool ran and could not do
    /// it.
    pub fn call(&mut self, name: &str, arguments: &str) -> Result<Value, Error> {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.tool.name == name)
        else {
            return Err(Error::Unknown {
                name: name.to_owned(),
            });
        };
        let arguments = serde_json::from_str(arguments).map_err(|error| Error::Arguments {
            name: name.to_owned(),
            error: error.to_string(),
        })?;
        (entry.handler)(arguments)
    }
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registry")
            .field(
                "tools",
                &self.tools().map(|tool| &tool.name).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use serde_json::json;

    use super::*;

    /// What the scripted tool takes: typed, so the tests exercise the
    /// deserialization the registry promises and nothing more.
    #[derive(Deserialize)]
    struct Echo {
        text: String,
    }

    /// A tool's own failure vocabulary, as a real tool would have one.
    #[derive(Debug, Eq, PartialEq)]
    enum Cranky {
        Refuses,
    }

    impl fmt::Display for Cranky {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "the tool refuses on principle")
        }
    }

    impl std::error::Error for Cranky {}

    fn echoing() -> Registry {
        let mut registry = Registry::new();
        registry.register(
            Tool::new(
                "echo",
                "Says the text back.",
                json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } },
                    "required": ["text"],
                }),
            ),
            |args: Echo| Ok::<_, Infallible>(json!({ "echoed": args.text })),
        );
        registry
    }

    #[test]
    fn a_registered_tool_is_listed_with_its_schema_verbatim() {
        let registry = echoing();

        let tools: Vec<_> = registry.tools().collect();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].description, "Says the text back.");
        assert_eq!(tools[0].schema["required"], json!(["text"]));
    }

    #[test]
    fn declarations_render_the_tools_for_a_provider_request() {
        let registry = echoing();

        let declarations = registry.declarations();

        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].name, "echo");
        assert_eq!(declarations[0].description, "Says the text back.");
        let echo = registry.tools().next().unwrap();
        assert_eq!(
            declarations[0].parameters, echo.schema,
            "the schema goes on the wire verbatim, as the parameters"
        );
    }

    #[test]
    fn a_call_by_name_with_json_arguments_answers_in_json() {
        let mut registry = echoing();

        let result = registry.call("echo", r#"{"text": "hello"}"#).unwrap();

        assert_eq!(result, json!({ "echoed": "hello" }));
    }

    #[test]
    fn an_unknown_tool_is_a_typed_refusal() {
        let mut registry = echoing();

        let error = registry.call("linear", "{}").unwrap_err();

        let Error::Unknown { name } = error else {
            panic!("an unknown name should refuse as Unknown, got {error:?}");
        };
        assert_eq!(name, "linear");
    }

    #[test]
    fn arguments_that_do_not_fit_the_handler_are_a_typed_error() {
        let mut registry = echoing();

        let error = registry.call("echo", r#"{"text": 7}"#).unwrap_err();

        assert!(
            matches!(&error, Error::Arguments { name, .. } if name == "echo"),
            "misshapen arguments should be an Arguments error, got {error:?}"
        );
    }

    #[test]
    fn arguments_that_are_not_json_at_all_are_the_same_typed_error() {
        let mut registry = echoing();

        let error = registry.call("echo", "not json").unwrap_err();

        assert!(
            matches!(error, Error::Arguments { .. }),
            "a model's malformed JSON should be an Arguments error, got {error:?}"
        );
    }

    #[test]
    fn a_failing_tool_reports_its_own_error_still_typed() {
        let mut registry = Registry::new();
        registry.register(
            Tool::new("cranky", "Never cooperates.", json!({ "type": "object" })),
            |_: Value| Err::<Value, _>(Cranky::Refuses),
        );

        let error = registry.call("cranky", "{}").unwrap_err();

        let Error::Failed { name, error } = error else {
            panic!("a tool's failure should be Failed, got {error:?}");
        };
        assert_eq!(name, "cranky");
        assert_eq!(
            error.downcast_ref::<Cranky>(),
            Some(&Cranky::Refuses),
            "the tool's vocabulary survives the trip through the registry"
        );
    }

    #[test]
    fn a_passthrough_handler_takes_the_arguments_as_they_came() {
        // The transport-agnostic shape: a future MCP client call is a
        // handler on Value at both ends, registered like any other tool.
        let mut registry = Registry::new();
        registry.register(
            Tool::new("relay", "Forwards to somewhere else.", json!({})),
            |args: Value| Ok::<_, Infallible>(args),
        );

        let result = registry
            .call("relay", r#"{"anything": ["at", "all"]}"#)
            .unwrap();

        assert_eq!(result, json!({ "anything": ["at", "all"] }));
    }

    #[test]
    fn tools_list_in_registration_order() {
        let mut registry = Registry::new();
        for name in ["c", "a", "b"] {
            registry.register(Tool::new(name, "", json!({})), |args: Value| {
                Ok::<_, Infallible>(args)
            });
        }

        let names: Vec<_> = registry.tools().map(|tool| tool.name.as_str()).collect();

        assert_eq!(names, ["c", "a", "b"], "a door shows tools as registered");
    }

    #[test]
    #[should_panic(expected = "already registered")]
    fn a_duplicate_name_is_a_bug_rather_than_a_shadowed_tool() {
        let mut registry = echoing();
        registry.register(Tool::new("echo", "Again.", json!({})), |args: Value| {
            Ok::<_, Infallible>(args)
        });
    }

    #[test]
    fn state_in_a_handler_carries_between_calls() {
        // FnMut, deliberately: a handler holding a client — or a test
        // holding a counter — mutates through its own captures.
        let mut registry = Registry::new();
        let mut calls = 0_u32;
        registry.register(
            Tool::new("count", "Counts its own calls.", json!({})),
            move |_: Value| {
                calls += 1;
                Ok::<_, Infallible>(json!(calls))
            },
        );

        registry.call("count", "{}").unwrap();
        let result = registry.call("count", "{}").unwrap();

        assert_eq!(result, json!(2));
    }

    #[test]
    fn the_registry_crosses_to_the_thread_that_runs_the_loop() {
        fn sendable<T: Send>(_: &T) {}
        sendable(&echoing());
    }

    #[test]
    fn refusals_render_for_a_door() {
        assert_eq!(
            Error::Unknown {
                name: "linear".to_owned()
            }
            .to_string(),
            "no tool named linear is registered"
        );
    }
}
