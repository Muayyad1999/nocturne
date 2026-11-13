//! A small JSON Schema validator for tool arguments.
//!
//! Tool inputs are streamed eagerly, which means the provider does not
//! validate them; weaker local models also get types wrong ("30" for 30, a
//! bare string for a one-element list). [`coerce`] repairs the harmless
//! mistakes and [`validate`] rejects the rest with a message the model can act
//! on. Supported keywords: `type`, `properties`, `required`, `enum`, `items`,
//! `minimum`, `maximum`, `minItems`, `maxItems`, `minLength`, `maxLength`,
//! `additionalProperties: false`.

use serde_json::Value;

/// Repair common, unambiguous type slips in place.
pub fn coerce(schema: &Value, value: &mut Value) {
    let Some(kind) = schema.get("type").and_then(Value::as_str) else {
        return;
    };
    match kind {
        "integer" | "number" => {
            if let Value::String(text) = value {
                let text = text.trim();
                if let Ok(int) = text.parse::<i64>() {
                    *value = Value::from(int);
                } else if kind == "number"
                    && let Ok(float) = text.parse::<f64>()
                    && let Some(number) = serde_json::Number::from_f64(float)
                {
                    *value = Value::Number(number);
                }
            } else if kind == "integer"
                && let Some(float) = value.as_f64()
                && value.as_i64().is_none()
                && float.fract() == 0.0
                && float.abs() < 9.0e15
            {
                *value = Value::from(float as i64);
            }
        }
        "boolean" => {
            if let Value::String(text) = value {
                match text.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" => *value = Value::Bool(true),
                    "false" | "no" => *value = Value::Bool(false),
                    _ => {}
                }
            }
        }
        "string" => match value {
            Value::Number(number) => *value = Value::String(number.to_string()),
            Value::Bool(flag) => *value = Value::String(flag.to_string()),
            _ => {}
        },
        "array" => {
            if let Value::String(text) = value {
                // A JSON array that arrived double-encoded, or a single item.
                match serde_json::from_str::<Value>(text) {
                    Ok(parsed @ Value::Array(_)) => *value = parsed,
                    _ => *value = Value::Array(vec![Value::String(std::mem::take(text))]),
                }
            }
            if let (Some(items), Value::Array(list)) = (schema.get("items"), &mut *value) {
                for item in list {
                    coerce(items, item);
                }
            }
        }
        "object" => {
            if let Value::String(text) = value
                && let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(text)
            {
                *value = parsed;
            }
            if let (Some(Value::Object(props)), Value::Object(map)) =
                (schema.get("properties"), &mut *value)
            {
                // Explicit nulls for optional fields are the same as leaving them out.
                let required: Vec<&str> = schema
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                map.retain(|key, item| !(item.is_null() && !required.contains(&key.as_str())));
                for (key, item) in map.iter_mut() {
                    if let Some(sub) = props.get(key) {
                        coerce(sub, item);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Check `value` against `schema`; the error names the offending path.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    validate_at(schema, value, "")
}

fn validate_at(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let here = if path.is_empty() { "arguments" } else { path };

    if let Some(kind) = schema.get("type") {
        let allowed: Vec<&str> = match kind {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !allowed.is_empty() && !allowed.iter().any(|kind| type_matches(kind, value)) {
            return Err(format!(
                "{here} must be {} (got {})",
                allowed.join(" or "),
                type_name(value)
            ));
        }
    }

    if let Some(options) = schema.get("enum").and_then(Value::as_array)
        && !options.contains(value)
    {
        let listed: Vec<String> = options.iter().map(Value::to_string).collect();
        return Err(format!("{here} must be one of {}", listed.join(", ")));
    }

    match value {
        Value::Object(map) => {
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for key in required.iter().filter_map(Value::as_str) {
                    if !map.contains_key(key) {
                        return Err(format!("{here} is missing required field \"{key}\""));
                    }
                }
            }
            let props = schema.get("properties").and_then(Value::as_object);
            for (key, item) in map {
                let child = join(path, key);
                match props.and_then(|props| props.get(key)) {
                    Some(sub) => validate_at(sub, item, &child)?,
                    None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                        let known: Vec<&str> = props
                            .map(|props| props.keys().map(String::as_str).collect())
                            .unwrap_or_default();
                        return Err(format!(
                            "{child} is not a known field (expected: {})",
                            known.join(", ")
                        ));
                    }
                    None => {}
                }
            }
        }
        Value::Array(list) => {
            if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
                && (list.len() as u64) < min
            {
                return Err(format!("{here} needs at least {min} item(s)"));
            }
            if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
                && (list.len() as u64) > max
            {
                return Err(format!("{here} allows at most {max} item(s)"));
            }
            if let Some(items) = schema.get("items") {
                for (index, item) in list.iter().enumerate() {
                    validate_at(items, item, &format!("{here}[{index}]"))?;
                }
            }
        }
        Value::Number(number) => {
            let n = number.as_f64().unwrap_or(0.0);
            if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
                && n < min
            {
                return Err(format!("{here} must be at least {min}"));
            }
            if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
                && n > max
            {
                return Err(format!("{here} must be at most {max}"));
            }
        }
        Value::String(text) => {
            let length = text.chars().count() as u64;
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
                && length < min
            {
                return Err(format!(
                    "{here} must not be shorter than {min} character(s)"
                ));
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
                && length > max
            {
                return Err(format!("{here} must not be longer than {max} characters"));
            }
        }
        _ => {}
    }
    Ok(())
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn type_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        _ => true,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "device": {"type": "string"},
                "vlan": {"type": "integer", "minimum": 1, "maximum": 4094},
                "commands": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                "save": {"type": "boolean"},
                "mode": {"type": "string", "enum": ["access", "trunk"]}
            },
            "required": ["device", "commands"],
            "additionalProperties": false
        })
    }

    #[test]
    fn accepts_valid_input() {
        let value = json!({"device": "sw1", "vlan": 30, "commands": ["show vlan"], "save": true, "mode": "trunk"});
        assert_eq!(validate(&schema(), &value), Ok(()));
    }

    #[test]
    fn reports_missing_and_unknown_fields() {
        let missing = validate(&schema(), &json!({"device": "sw1"})).unwrap_err();
        assert!(
            missing.contains("missing required field \"commands\""),
            "{missing}"
        );
        let unknown = validate(
            &schema(),
            &json!({"device": "sw1", "commands": ["x"], "cmds": []}),
        )
        .unwrap_err();
        assert!(unknown.contains("cmds is not a known field"), "{unknown}");
    }

    #[test]
    fn reports_type_range_and_enum_errors_with_paths() {
        let bad_type =
            validate(&schema(), &json!({"device": "sw1", "commands": ["ok", 5]})).unwrap_err();
        assert!(
            bad_type.contains("commands[1] must be string"),
            "{bad_type}"
        );
        let bad_range = validate(
            &schema(),
            &json!({"device": "sw1", "commands": ["x"], "vlan": 5000}),
        )
        .unwrap_err();
        assert!(
            bad_range.contains("vlan must be at most 4094"),
            "{bad_range}"
        );
        let bad_enum = validate(
            &schema(),
            &json!({"device": "sw1", "commands": ["x"], "mode": "routed"}),
        )
        .unwrap_err();
        assert!(bad_enum.contains("mode must be one of"), "{bad_enum}");
    }

    #[test]
    fn coerces_harmless_slips() {
        let mut value = json!({"device": 7, "vlan": "30", "commands": "show vlan", "save": "true", "mode": null});
        coerce(&schema(), &mut value);
        assert_eq!(
            value,
            json!({"device": "7", "vlan": 30, "commands": ["show vlan"], "save": true})
        );
        assert_eq!(validate(&schema(), &value), Ok(()));
    }

    #[test]
    fn coerces_double_encoded_array_and_whole_floats() {
        let mut value = json!({"device": "a", "commands": "[\"one\", \"two\"]", "vlan": 20.0});
        coerce(&schema(), &mut value);
        assert_eq!(value["commands"], json!(["one", "two"]));
        assert_eq!(value["vlan"], json!(20));
    }
}
