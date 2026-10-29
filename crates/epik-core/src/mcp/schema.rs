//! Checking a tool's arguments against its input schema.
//!
//! Only what `tools.json`'s schemas use is checked: `required`, and each
//! property's `type` (`string`, `integer`, `boolean`, `array` of strings),
//! `minLength`, `pattern`, `minimum`, `maximum` and `exclusiveMinimum`.

use serde_json::{Map, Value};

/// Whether `args` meet `schema`, or why not.
pub fn check(schema: &Map<String, Value>, args: &Map<String, Value>) -> Result<(), String> {
    let empty = Map::new();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for name in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !args.contains_key(name) {
            return Err(format!("{name} is required"));
        }
    }
    for (name, value) in args {
        if let Some(property) = properties.get(name).and_then(Value::as_object) {
            check_value(name, property, value)?;
        }
    }
    Ok(())
}

fn check_value(name: &str, schema: &Map<String, Value>, value: &Value) -> Result<(), String> {
    let number = |key: &str| schema.get(key).and_then(Value::as_f64);
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => {
            let text = value
                .as_str()
                .ok_or_else(|| format!("{name} must be a string"))?;
            if let Some(least) = number("minLength")
                && (text.chars().count() as f64) < least
            {
                return Err(format!("{name} must have at least {least} characters"));
            }
            if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
                let regex = regex_lite::Regex::new(pattern)
                    .map_err(|error| format!("{name}'s pattern doesn't compile: {error}"))?;
                if !regex.is_match(text) {
                    return Err(format!("{name} must match {pattern}"));
                }
            }
        }
        Some("integer") => {
            let n = value
                .as_i64()
                .ok_or_else(|| format!("{name} must be an integer"))? as f64;
            if number("minimum").is_some_and(|least| n < least)
                || number("maximum").is_some_and(|most| n > most)
                || number("exclusiveMinimum").is_some_and(|above| n <= above)
            {
                return Err(format!("{name} is out of range"));
            }
        }
        Some("boolean") if !value.is_boolean() => {
            return Err(format!("{name} must be true or false"));
        }
        Some("array") => {
            let items = value
                .as_array()
                .ok_or_else(|| format!("{name} must be a list"))?;
            if items.iter().any(|item| !item.is_string()) {
                return Err(format!("{name} must be a list of strings"));
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Map<String, Value> {
        json!({
            "type": "object",
            "properties": {
                "repo": {"type": "string", "pattern": "^[^/]+\\/[^/]+$"},
                "name": {"type": "string", "minLength": 1},
                "number": {"type": "integer", "exclusiveMinimum": 0},
                "force": {"type": "boolean"},
                "args": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["repo"]
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn arguments_that_meet_the_schema_pass() {
        let ok =
            args(json!({"repo": "o/r", "name": "x", "number": 1, "force": true, "args": ["a"]}));
        assert_eq!(check(&schema(), &ok), Ok(()));
    }

    #[test]
    fn arguments_that_dont_are_refused_with_the_reason() {
        for (bad, why) in [
            (json!({}), "repo is required"),
            (json!({"repo": 1}), "repo must be a string"),
            (json!({"repo": "o"}), "repo must match"),
            (
                json!({"repo": "o/r", "name": ""}),
                "name must have at least 1 characters",
            ),
            (
                json!({"repo": "o/r", "number": 0}),
                "number is out of range",
            ),
            (
                json!({"repo": "o/r", "number": 1.5}),
                "number must be an integer",
            ),
            (
                json!({"repo": "o/r", "force": "yes"}),
                "force must be true or false",
            ),
            (
                json!({"repo": "o/r", "args": [1]}),
                "args must be a list of strings",
            ),
        ] {
            let error = check(&schema(), &args(bad.clone())).unwrap_err();
            assert!(error.starts_with(why), "{bad}: {error}");
        }
    }
}
