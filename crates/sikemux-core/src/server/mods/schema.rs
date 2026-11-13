//! The JSON Schema a mod's `mod.json` may give each collection, of which the core knows a plain
//! subset. A schema that uses more is refused when the manifest is read, so every write is
//! checked against exactly what its schema says.

use serde_json::{Map, Value};

const TYPES: [&str; 7] = [
    "object", "array", "string", "number", "integer", "boolean", "null",
];

/// Why `schema` uses something the core does not check, naming where.
pub(super) fn unsupported(schema: &Value, at: &str) -> Option<String> {
    let Value::Object(schema) = schema else {
        return Some(format!("{at} is not an object"));
    };
    for (key, value) in schema {
        let wrong = match key.as_str() {
            "description" | "title" => !value.is_string(),
            "type" => match value {
                Value::String(name) => !TYPES.contains(&name.as_str()),
                Value::Array(names) => !names
                    .iter()
                    .all(|name| name.as_str().is_some_and(|name| TYPES.contains(&name))),
                _ => true,
            },
            "enum" => !value.is_array(),
            "required" => !value
                .as_array()
                .is_some_and(|names| names.iter().all(Value::is_string)),
            "minimum" | "maximum" => !value.is_number(),
            "maxLength" => !value.is_u64(),
            "properties" => {
                let Value::Object(properties) = value else {
                    return Some(format!("{at}.properties is not an object"));
                };
                if let Some(problem) = properties
                    .iter()
                    .find_map(|(name, inner)| unsupported(inner, &format!("{at}.{name}")))
                {
                    return Some(problem);
                }
                false
            }
            "items" => return unsupported(value, &format!("{at}.items")),
            "additionalProperties" => match value {
                Value::Bool(_) => false,
                inner => return unsupported(inner, &format!("{at}.additionalProperties")),
            },
            other => {
                return Some(format!(
                    "{at} uses \"{other}\", which Sikemux does not check; a schema may use type, properties, required, enum, items, additionalProperties, minimum, maximum and maxLength"
                ))
            }
        };
        if wrong {
            return Some(format!("{at} has a {key} that does not read"));
        }
    }
    None
}

fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn is_type(value: &Value, wanted: &str) -> bool {
    let found = type_of(value);
    found == wanted || (wanted == "number" && found == "integer")
}

fn joined(path: &str, name: &str) -> String {
    match path.is_empty() {
        true => name.to_owned(),
        false => format!("{path}.{name}"),
    }
}

fn field(path: &str) -> String {
    match path.is_empty() {
        true => "the document".to_owned(),
        false => format!("field {path}"),
    }
}

/// Checks `value` against a schema `unsupported` let through, saying which field failed and why.
pub(super) fn check(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let Value::Object(schema) = schema else {
        return Ok(());
    };
    if let Some(wanted) = schema.get("type") {
        let names: Vec<&str> = match wanted {
            Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
            other => other.as_str().into_iter().collect(),
        };
        if !names.iter().any(|name| is_type(value, name)) {
            return Err(format!(
                "{} must be {}, not {}",
                field(path),
                names.join(" or "),
                type_of(value)
            ));
        }
    }
    if let Some(Value::Array(allowed)) = schema.get("enum") {
        if !allowed.contains(value) {
            let listed: Vec<String> = allowed.iter().map(Value::to_string).collect();
            return Err(format!(
                "{} must be one of {}, not {value}",
                field(path),
                listed.join(", ")
            ));
        }
    }
    match value {
        Value::Number(number) => {
            let number = number.as_f64().unwrap_or_default();
            if let Some(least) = schema.get("minimum").and_then(Value::as_f64) {
                if number < least {
                    return Err(format!("{} must be at least {least}", field(path)));
                }
            }
            if let Some(most) = schema.get("maximum").and_then(Value::as_f64) {
                if number > most {
                    return Err(format!("{} must be at most {most}", field(path)));
                }
            }
        }
        Value::String(text) => {
            if let Some(most) = schema.get("maxLength").and_then(Value::as_u64) {
                let length = text.chars().count() as u64;
                if length > most {
                    return Err(format!(
                        "{} holds {length} characters, more than its {most}",
                        field(path)
                    ));
                }
            }
        }
        Value::Array(items) => {
            if let Some(each) = schema.get("items") {
                for (at, item) in items.iter().enumerate() {
                    check(each, item, &format!("{path}[{at}]"))?;
                }
            }
        }
        Value::Object(fields) => object(schema, fields, path)?,
        _ => {}
    }
    Ok(())
}

fn object(
    schema: &Map<String, Value>,
    fields: &Map<String, Value>,
    path: &str,
) -> Result<(), String> {
    if let Some(Value::Array(required)) = schema.get("required") {
        if let Some(missing) = required
            .iter()
            .filter_map(Value::as_str)
            .find(|name| !fields.contains_key(*name))
        {
            return Err(format!("{} is required", field(&joined(path, missing))));
        }
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    for (name, value) in fields {
        let inner = joined(path, name);
        match (
            properties.and_then(|known| known.get(name)),
            schema.get("additionalProperties"),
        ) {
            (Some(known), _) => check(known, value, &inner)?,
            (None, Some(Value::Bool(false))) => {
                return Err(format!("{} is not one the schema allows", field(&inner)))
            }
            (None, Some(other)) => check(other, value, &inner)?,
            (None, None) => {}
        }
    }
    Ok(())
}
