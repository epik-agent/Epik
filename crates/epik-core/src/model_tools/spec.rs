//! A tool as a row of data: what the model calls it, what it is for,
//! what it takes, and what runs it.
//!
//! A tool module is a `const` slice of [`Spec`]s, read at a glance in
//! the OpenAI vocabulary — name, description, parameters — with the
//! handler named beside them, so the compiler checks that every row
//! has one. [`Spec::tool`] writes the parameters' JSON Schema and
//! wraps the handler, whose error, with every context it gathered,
//! becomes the words the model reads.

use anyhow::Result;
use serde_json::{Value, json};

use super::Tool;

/// What a handler does with a tool's parsed arguments: a JSON answer,
/// or why not, for the model.
pub type Handler = fn(&Value) -> Result<Value>;

/// One tool, as data.
pub struct Spec {
    /// What the model calls it.
    pub name: &'static str,
    /// What it is for, in a sentence for the model.
    pub description: &'static str,
    /// What it takes, in the order the schema lists them.
    pub parameters: &'static [Param],
    /// What runs it.
    pub handler: Handler,
}

impl Spec {
    /// The JSON Schema of the parameters: an object of the properties,
    /// naming the required ones.
    #[must_use]
    pub fn schema(&self) -> Value {
        let properties: serde_json::Map<String, Value> = self
            .parameters
            .iter()
            .map(|param| (param.name.to_owned(), param.kind.schema(param.description)))
            .collect();
        let required: Vec<&str> = self
            .parameters
            .iter()
            .filter(|param| param.required)
            .map(|param| param.name)
            .collect();
        json!({ "type": "object", "properties": properties, "required": required })
    }

    /// The tool this row describes.
    #[must_use]
    pub fn tool(&self) -> Tool {
        let handler = self.handler;
        Tool::new(
            self.name,
            self.description,
            self.schema(),
            Box::new(move |arguments| handler(arguments).map_err(|error| format!("{error:#}"))),
        )
    }
}

/// One parameter: its name, its kind, whether the model must give it,
/// and what it must say.
pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
    pub description: &'static str,
}

/// A parameter the model must give.
#[must_use]
pub const fn required(name: &'static str, kind: Kind, description: &'static str) -> Param {
    Param {
        name,
        kind,
        required: true,
        description,
    }
}

/// A parameter the model may leave out.
#[must_use]
pub const fn optional(name: &'static str, kind: Kind, description: &'static str) -> Param {
    Param {
        name,
        kind,
        required: false,
        description,
    }
}

/// The JSON type of a parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    String,
    Integer,
    Boolean,
    /// An array of strings.
    Strings,
}

impl Kind {
    /// The JSON Schema of a value of this kind, with its sentence.
    fn schema(self, description: &str) -> Value {
        match self {
            Self::String => json!({ "type": "string", "description": description }),
            Self::Integer => json!({ "type": "integer", "description": description }),
            Self::Boolean => json!({ "type": "boolean", "description": description }),
            Self::Strings => json!({
                "type": "array",
                "items": { "type": "string" },
                "description": description,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    fn greet(arguments: &Value) -> Result<Value> {
        let name = arguments["name"]
            .as_str()
            .context("the name argument must be a string")?;
        Ok(json!({ "greeting": format!("hello, {name}") }))
    }

    const GREET: Spec = Spec {
        name: "greet",
        description: "Says hello.",
        parameters: &[
            required("name", Kind::String, "Who to greet."),
            optional("times", Kind::Integer, "How many times."),
            optional("loudly", Kind::Boolean, "Whether to shout."),
            optional("also", Kind::Strings, "Who else."),
        ],
        handler: greet,
    };

    #[test]
    fn the_schema_lists_every_parameter_and_names_the_required_ones() {
        assert_eq!(
            GREET.schema(),
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Who to greet." },
                    "times": { "type": "integer", "description": "How many times." },
                    "loudly": { "type": "boolean", "description": "Whether to shout." },
                    "also": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Who else.",
                    },
                },
                "required": ["name"],
            })
        );
    }

    #[test]
    fn the_tool_runs_the_handler_and_turns_its_error_into_words() {
        let mut registry = crate::model_tools::Registry::default();
        registry.register(GREET.tool());
        assert_eq!(
            registry.dispatch("greet", r#"{"name":"Ada"}"#),
            Ok(json!({ "greeting": "hello, Ada" }))
        );
        assert_eq!(
            registry.dispatch("greet", "{}"),
            Err("the name argument must be a string".to_owned())
        );
    }
}
