//! Tools the model can call, in the OpenAI function-tool protocol.
//!
//! Deliberately dumb: a [`Tool`] is a name, a description, a parameters
//! schema, and a handler; a [`Registry`] is a Vec of them. Everything a
//! tool can get wrong — an unknown name, arguments that don't parse, a
//! handler that fails — becomes an Err *result*, words fed back to the
//! model as the tool's output. Never a crash, never a turn failure.
//!
//! The wire shapes are the protocol's own: a [`ToolSpec`] is a tool as
//! a request advertises it, a [`ToolCall`] is a call as the model makes
//! it. The loop that runs a turn through a registry lives with the
//! client that speaks to the model. The providers' verbs live beside
//! this port — [`git`] and [`github`], one tool per verb — and
//! [`current_time`], the wire's canary, is the one built-in.

#[cfg(feature = "native")]
pub mod git;
#[cfg(feature = "native")]
pub mod github;

/// The typed reads of a tool's JSON arguments, shared by every tool
/// module. Each refusal is words for the model: which argument, and
/// what it had to be.
pub mod arg {
    use serde_json::Value;

    /// A required string.
    ///
    /// # Errors
    ///
    /// The argument is absent or not a string.
    pub(super) fn string<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, String> {
        arguments[name]
            .as_str()
            .ok_or_else(|| format!("the {name} argument must be a string"))
    }

    /// A required string with something in it.
    ///
    /// # Errors
    ///
    /// The argument is absent, not a string, or blank.
    pub fn non_empty<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, String> {
        arguments[name]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("the {name} argument must be a non-empty string"))
    }

    /// A required whole number.
    ///
    /// # Errors
    ///
    /// The argument is absent or not a non-negative integer.
    pub(crate) fn number(arguments: &Value, name: &str) -> Result<u64, String> {
        arguments[name]
            .as_u64()
            .ok_or_else(|| format!("the {name} argument must be a whole number"))
    }

    /// A string that may be absent, held to the non-empty rule when it
    /// is there: absent is fine, but a blank value is a refusal.
    ///
    /// # Errors
    ///
    /// The argument is present and not a non-blank string.
    pub(crate) fn optional<'a>(
        arguments: &'a Value,
        name: &str,
    ) -> Result<Option<&'a str>, String> {
        match &arguments[name] {
            Value::Null => Ok(None),
            Value::String(value) if !value.trim().is_empty() => Ok(Some(value)),
            _ => Err(format!(
                "the {name} argument must be a non-empty string when given"
            )),
        }
    }
}

/// One tool as a request advertises it: the protocol's function tool,
/// `{"type": "function", "function": {name, description, parameters}}`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    kind: &'static str,
    function: ToolFunction,
}

#[derive(Clone, Debug, serde::Serialize)]
struct ToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl ToolSpec {
    /// A function tool: `parameters` is its arguments' JSON Schema.
    #[must_use]
    pub fn function(name: String, description: String, parameters: serde_json::Value) -> Self {
        Self {
            kind: "function",
            function: ToolFunction {
                name,
                description,
                parameters,
            },
        }
    }
}

/// One completed tool call, as the model asked for it: the id the result
/// must answer to, the tool's name, and the arguments exactly as the
/// model wrote them — a raw JSON string, parsed only at dispatch.
///
/// Serializes as the protocol's tool call, `{"id", "type": "function",
/// "function": {name, arguments}}`, so an assistant message echoes it
/// back to the model exactly as it was asked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub(crate) arguments: String,
}

impl serde::Serialize for ToolCall {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(serde::Serialize)]
        struct Function<'a> {
            name: &'a str,
            arguments: &'a str,
        }
        #[derive(serde::Serialize)]
        struct Wire<'a> {
            id: &'a str,
            #[serde(rename = "type")]
            kind: &'static str,
            function: Function<'a>,
        }
        Wire {
            id: &self.id,
            kind: "function",
            function: Function {
                name: &self.name,
                arguments: &self.arguments,
            },
        }
        .serialize(serializer)
    }
}

/// What a tool does with its parsed arguments: a JSON answer, or its
/// failure in words — which the model reads, so say what went wrong.
type Handler = Box<dyn Fn(&serde_json::Value) -> Result<serde_json::Value, String>>;

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

/// A tool of epik-core's table as one of these: the same four parts,
/// until this port follows the rest into epik-core.
impl From<epik_core::model_tools::Tool> for Tool {
    fn from(tool: epik_core::model_tools::Tool) -> Self {
        Self {
            name: tool.name().to_owned(),
            description: tool.description().to_owned(),
            parameters: tool.parameters().clone(),
            handler: Box::new(move |arguments| tool.call(arguments)),
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

    /// Registers a whole set at once — how the git and GitHub verbs
    /// arrive.
    pub fn extend(&mut self, tools: impl IntoIterator<Item = Tool>) {
        self.0.extend(tools);
    }

    /// The request's tools array.
    #[must_use]
    pub(crate) fn to_wire(&self) -> Vec<ToolSpec> {
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

/// The built-in canary: no arguments, the local date-time and UTC offset
/// as JSON. It proves the round trip, and afterwards it tells the time.
#[cfg(feature = "native")]
#[must_use]
fn current_time() -> Tool {
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

/// The tool that says its `text` argument back: the fixture the loop
/// tests drive.
#[cfg(test)]
pub(crate) fn echo() -> Tool {
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

/// A registry of exactly [`echo`].
#[cfg(test)]
pub(crate) fn echoing() -> Registry {
    let mut registry = Registry::default();
    registry.register(echo());
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_hands_parsed_arguments_to_the_handler() {
        let registry = echoing();
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
        let registry = echoing();
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
    fn a_tool_call_serializes_as_the_protocol_writes_it() {
        let call = ToolCall {
            id: "call_1".to_owned(),
            name: "echo".to_owned(),
            arguments: "{\"text\":\"hi\"}".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(&call).unwrap(),
            serde_json::json!({
                "id": "call_1",
                "type": "function",
                "function": { "name": "echo", "arguments": "{\"text\":\"hi\"}" },
            })
        );
    }

    #[test]
    fn the_registry_advertises_its_tools_in_the_wire_shape() {
        let registry = echoing();
        let wire = serde_json::to_value(registry.to_wire()).unwrap();
        assert_eq!(wire[0]["type"], "function");
        assert_eq!(wire[0]["function"]["name"], "echo");
        assert!(wire[0]["function"]["parameters"].is_object());
    }

    /// The whole registry as the backend assembles it each turn:
    /// current_time, the GitHub verbs, and the git verbs together.
    #[cfg(feature = "native")]
    mod registration {
        use super::*;

        fn full() -> Registry {
            let mut registry = Registry::standard();
            registry.extend(github::all(
                crate::github::GitHub::at("http://127.0.0.1:1", None),
                None,
            ));
            registry.extend(git::all());
            registry
        }

        #[test]
        fn every_verb_is_present_and_every_name_unique() {
            let wire = serde_json::to_value(full().to_wire()).unwrap();
            let names: Vec<String> = wire
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
                .collect();
            let expected_github = [
                "default_branch",
                "issue",
                "create_issue",
                "edit_issue",
                "comment",
                "close_issue",
                "reopen_issue",
                "open_pull",
                "pull",
                "pull_for",
                "merge_pull",
                "branch_sha",
                "compare",
                "create_branch",
                "check_conclusions",
                "issue_graph",
                "add_sub_issue",
                "remove_sub_issue",
                "add_blocked_by",
                "remove_blocked_by",
            ];
            let expected_git = [
                "status",
                "log",
                "diff",
                "show",
                "branch_list",
                "current_branch",
                "add",
                "commit",
                "checkout",
                "fetch",
                "pull",
                "push",
                "clone",
                "remote_list",
                "init",
            ];
            assert!(names.contains(&"current_time".to_owned()));
            assert!(
                names.contains(&"feature_plan".to_owned()),
                "the tracker's read of a whole feature rides with the GitHub verbs"
            );
            for verb in expected_github {
                assert!(names.contains(&format!("github_{verb}")), "{verb}");
            }
            for verb in expected_git {
                assert!(names.contains(&format!("git_{verb}")), "{verb}");
            }
            assert_eq!(
                names.len(),
                2 + expected_github.len() + expected_git.len(),
                "{names:?}"
            );
            let unique: std::collections::BTreeSet<_> = names.iter().collect();
            assert_eq!(unique.len(), names.len(), "names must be unique");
        }

        #[test]
        fn every_parameters_value_is_a_json_schema_object() {
            let wire = serde_json::to_value(full().to_wire()).unwrap();
            for tool in wire.as_array().unwrap() {
                let name = tool["function"]["name"].as_str().unwrap();
                let parameters = &tool["function"]["parameters"];
                assert_eq!(parameters["type"], "object", "{name}: {parameters}");
                assert!(parameters["properties"].is_object(), "{name}: {parameters}");
                assert!(
                    !tool["function"]["description"].as_str().unwrap().is_empty(),
                    "{name} says what it is for"
                );
            }
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
