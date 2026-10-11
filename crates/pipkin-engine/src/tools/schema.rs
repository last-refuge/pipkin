//! Tool argument validation: the subset of JSON Schema the tools use (`object` with
//! `properties`/`required`, `array` with `items`, `string`, `number`, `integer`, `boolean`,
//! `null`), with the coercions the reference applies before checking (pi-ai
//! `validateToolArguments`: optional `null` properties removed, then TypeBox `Value.Convert`),
//! and the reference error text.

use serde_json::{Map, Value};

use super::text::{js_json_number, js_parse_number};

/// Validate (and coerce) `args` against `schema`. The error is the reference message:
/// `Validation failed for tool "<name>":\n  - <path>: <message>...\n\nReceived arguments:\n<json>`.
pub fn validate(tool: &str, schema: &Value, args: &Value) -> Result<Value, String> {
    let mut value = args.clone();
    normalize_optional_nulls(&mut value, schema);
    convert(&mut value, schema);
    let mut errors = Vec::new();
    check(&value, schema, &mut Vec::new(), &mut errors);
    if errors.is_empty() {
        return Ok(value);
    }
    let lines: Vec<String> = errors.iter().map(|e| format!("  - {e}")).collect();
    let received = serde_json::to_string_pretty(args).unwrap_or_else(|_| "null".into());
    Err(format!(
        "Validation failed for tool \"{tool}\":\n{}\n\nReceived arguments:\n{received}",
        lines.join("\n")
    ))
}

fn types(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    }
}

fn properties(schema: &Value) -> Option<&Map<String, Value>> {
    schema.get("properties").and_then(Value::as_object)
}

fn required(schema: &Value) -> Vec<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn accepts_null(schema: &Value) -> bool {
    let ts = types(schema);
    ts.is_empty() || ts.contains(&"null")
}

/// Remove optional properties whose value is `null` when their schema rejects `null`.
fn normalize_optional_nulls(value: &mut Value, schema: &Value) {
    match value {
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items").filter(|s| s.is_object()) {
                for item in items {
                    normalize_optional_nulls(item, item_schema);
                }
            }
        }
        Value::Object(object) => {
            let Some(props) = properties(schema) else {
                return;
            };
            let req = required(schema);
            for (key, prop_schema) in props {
                let Some(current) = object.get_mut(key) else {
                    continue;
                };
                if current.is_null()
                    && !req.contains(&key.as_str())
                    && prop_schema.get("$ref").is_none()
                    && !accepts_null(prop_schema)
                {
                    object.remove(key);
                } else {
                    normalize_optional_nulls(current, prop_schema);
                }
            }
        }
        _ => {}
    }
}

fn truthy_word(text: &str) -> Option<bool> {
    if text.eq_ignore_ascii_case("true") {
        Some(true)
    } else if text.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

fn number_value(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

/// TypeBox `Value.Convert` for the supported keywords.
fn convert(value: &mut Value, schema: &Value) {
    let ts = types(schema);
    let Some(first) = ts.first().copied() else {
        return;
    };
    if ts.len() > 1 && ts.iter().any(|t| matches_type(value, t)) {
        return;
    }
    match first {
        "string" => {
            let converted = match value {
                Value::Null => Some("null".to_owned()),
                Value::Bool(b) => Some(b.to_string()),
                Value::Number(n) => Some(js_json_number(n)),
                _ => None,
            };
            if let Some(s) = converted {
                *value = Value::String(s);
            }
        }
        "number" | "integer" => {
            let integer = first == "integer";
            let parsed = match &*value {
                Value::Null => Some(0.0),
                Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
                Value::String(s) => truthy_word(s)
                    .map(|b| if b { 1.0 } else { 0.0 })
                    .or_else(|| js_parse_number(s).filter(|n| n.is_finite())),
                Value::Number(n) if integer => n.as_f64(),
                _ => None,
            };
            if let Some(mut n) = parsed {
                if integer {
                    n = n.trunc();
                }
                *value = number_value(n);
            }
        }
        "boolean" => {
            let converted = match &*value {
                Value::Null => Some(false),
                Value::Number(n) if n.as_f64() == Some(1.0) => Some(true),
                Value::Number(n) if n.as_f64() == Some(0.0) => Some(false),
                Value::String(s) if s == "1" => Some(true),
                Value::String(s) if s == "0" => Some(false),
                Value::String(s) => truthy_word(s),
                _ => None,
            };
            if let Some(b) = converted {
                *value = Value::Bool(b);
            }
        }
        "array" => {
            if !value.is_array() {
                *value = Value::Array(vec![value.take()]);
            }
            if let (Value::Array(items), Some(item_schema)) = (value, schema.get("items")) {
                for item in items {
                    convert(item, item_schema);
                }
            }
        }
        "object" => {
            if let (Value::Object(object), Some(props)) = (value, properties(schema)) {
                for (key, prop_schema) in props {
                    if let Some(current) = object.get_mut(key) {
                        convert(current, prop_schema);
                    }
                }
            }
        }
        _ => {}
    }
}

fn matches_type(value: &Value, ty: &str) -> bool {
    match ty {
        "number" => value.is_number(),
        "integer" => value.as_f64().is_some_and(|n| n.fract() == 0.0),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn format_path(path: &[String]) -> String {
    if path.is_empty() {
        "root".to_owned()
    } else {
        path.join(".")
    }
}

/// TypeBox's error list, in its order: type, then `required` (one error naming every missing
/// property), then each present property in schema order, then array items.
fn check(value: &Value, schema: &Value, path: &mut Vec<String>, errors: &mut Vec<String>) {
    let ts = types(schema);
    if !ts.is_empty() && !ts.iter().any(|t| matches_type(value, t)) {
        let message = if ts.len() == 1 {
            format!("must be {}", ts[0])
        } else {
            format!("must be either {}", ts.join(" or "))
        };
        errors.push(format!("{}: {message}", format_path(path)));
        return;
    }
    if let Value::Object(object) = value {
        let missing: Vec<&str> = required(schema)
            .into_iter()
            .filter(|key| !object.contains_key(*key))
            .collect();
        if let Some(first) = missing.first() {
            let base = path.join(".");
            let at = if base.is_empty() {
                (*first).to_owned()
            } else {
                format!("{base}.{first}")
            };
            errors.push(format!(
                "{at}: must have required properties {}",
                missing.join(", ")
            ));
        }
        if let Some(props) = properties(schema) {
            for (key, prop_schema) in props {
                if let Some(current) = object.get(key) {
                    path.push(key.clone());
                    check(current, prop_schema, path, errors);
                    path.pop();
                }
            }
        }
    }
    if let (Value::Array(items), Some(item_schema)) = (value, schema.get("items")) {
        for (index, item) in items.iter().enumerate() {
            path.push(index.to_string());
            check(item, item_schema, path, errors);
            path.pop();
        }
    }
}
