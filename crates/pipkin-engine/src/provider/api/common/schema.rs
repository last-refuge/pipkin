//! Strict JSON-schema tools (reference `api/constrained-sampling.ts`), used by the adapters
//! that support provider constrained sampling, plus Responses grammar tool declarations.

use serde_json::{Value, json};

use super::opt;
use crate::provider::{ProviderRequest, ToolSpec};

/// Returns true when a provider's strict mode rejects this keyword with this value.
pub type UnsupportedKeyword = dyn Fn(&str, &Value) -> bool;

const UNSUPPORTED_STRICT_SCHEMA_KEYS: &[&str] = &[
    "$ref",
    "$defs",
    "definitions",
    "allOf",
    "oneOf",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
    "unevaluatedProperties",
    "propertyNames",
    "contains",
    "prefixItems",
    "not",
    "if",
    "then",
    "else",
];

fn is_structured_schema(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    let types: Vec<&str> = match obj.get("type") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    types.contains(&"object")
        || types.contains(&"array")
        || obj.contains_key("properties")
        || obj.contains_key("items")
}

fn schema_allows_null(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    match obj.get("type") {
        Some(Value::String(s)) if s == "null" => return true,
        Some(Value::Array(a)) if a.iter().any(|t| t == "null") => return true,
        _ => {}
    }
    if obj.get("const") == Some(&Value::Null) {
        return true;
    }
    if let Some(Value::Array(e)) = obj.get("enum")
        && e.contains(&Value::Null)
    {
        return true;
    }
    matches!(obj.get("anyOf"), Some(Value::Array(v)) if v.iter().any(schema_allows_null))
}

fn make_node_strict(schema: &mut Value, check: Option<&UnsupportedKeyword>) -> Result<(), String> {
    let Some(obj) = schema.as_object_mut() else {
        return Err("boolean schemas are unsupported".into());
    };
    for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
        if obj.contains_key(*key) {
            return Err(format!("{key} schemas are unsupported"));
        }
    }
    if let Some(check) = check {
        for (k, v) in obj.iter() {
            if check(k, v) {
                return Err(format!("{k}: {v} is unsupported"));
            }
        }
    }
    if let Some(any_of) = obj.get_mut("anyOf") {
        let Some(variants) = any_of.as_array_mut().filter(|a| !a.is_empty()) else {
            return Err("anyOf must contain at least one schema".into());
        };
        for variant in variants {
            if is_structured_schema(variant) {
                return Err("object and array unions are unsupported".into());
            }
            make_node_strict(variant, check)?;
        }
    }
    if let Some(items) = obj.get_mut("items") {
        if items.is_array() {
            return Err("tuple schemas are unsupported".into());
        }
        make_node_strict(items, check)?;
    }
    let is_object = obj.get("type").and_then(Value::as_str) == Some("object");
    if obj.contains_key("properties") && !is_object {
        return Err("properties require type object".into());
    }
    if !is_object {
        return Ok(());
    }
    if let Some(ap) = obj.get("additionalProperties")
        && *ap != Value::Bool(false)
    {
        return Err("schema-valued or true additionalProperties is unsupported".into());
    }
    if let Some(p) = obj.get("properties")
        && !p.is_object()
    {
        return Err("object properties must be a schema map".into());
    }
    let required: Vec<String> = match obj.get("required") {
        None => vec![],
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Some(_) => return Err("object required must be a string array".into()),
    };
    let mut properties = obj
        .remove("properties")
        .and_then(|p| match p {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();
    if required.iter().any(|k| !properties.contains_key(k)) {
        return Err("required contains an unknown property".into());
    }
    let names: Vec<String> = properties.keys().cloned().collect();
    for (key, property) in properties.iter_mut() {
        make_node_strict(property, check)?;
        if !required.contains(key) && !schema_allows_null(property) {
            *property = json!({"anyOf": [property.take(), {"type": "null"}]});
        }
    }
    obj.insert("properties".into(), Value::Object(properties));
    obj.insert(
        "required".into(),
        Value::Array(names.into_iter().map(Value::String).collect()),
    );
    obj.insert("additionalProperties".into(), Value::Bool(false));
    Ok(())
}

/// Convert a tool schema to the strict subset expected by provider constrained sampling.
pub fn make_strict_json_schema(
    schema: &Value,
    check: Option<&UnsupportedKeyword>,
) -> Result<Value, String> {
    let mut cloned = schema.clone();
    if !cloned.is_object() {
        return Err("root schema must have type object".into());
    }
    make_node_strict(&mut cloned, check)?;
    if cloned.get("type").and_then(Value::as_str) != Some("object") {
        return Err("root schema must have type object".into());
    }
    Ok(cloned)
}

/// Prefer the durable tool declaration; retain the legacy request option for callers.
pub fn constrained_sampling<'a>(req: &'a ProviderRequest, tool: &'a ToolSpec) -> Option<&'a Value> {
    tool.constrained_sampling
        .as_ref()
        .or_else(|| opt(req, "toolConstrainedSampling")?.get(&tool.name))
}

#[derive(Clone, Debug)]
pub struct Grammar {
    pub format: &'static str,
    pub definition: String,
    pub input_property: String,
}

/// Reference `resolveGrammarConstrainedSampling`. Unsupported endpoints use JSON tools.
pub fn resolve_grammar(req: &ProviderRequest, tool: &ToolSpec) -> Result<Option<Grammar>, String> {
    let Some(config) = constrained_sampling(req, tool) else {
        return Ok(None);
    };
    if config.get("type").and_then(Value::as_str) != Some("grammar")
        || req.model.compat_bool("supportsOpenAIGrammarTools") != Some(true)
    {
        return Ok(None);
    }
    let fail = |reason: &str| {
        format!(
            "Tool \"{}\" cannot use grammar constrained sampling: {reason}.",
            tool.name
        )
    };
    let variant = |key: &str| {
        config
            .get("variants")
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
    };
    let (format, definition) = if let Some(v) = variant("openai_lark") {
        ("lark", v)
    } else if let Some(v) = variant("openai_regex") {
        ("regex", v)
    } else {
        return Err(fail("no supported grammar variant was provided"));
    };
    if tool.parameters.get("type").and_then(Value::as_str) != Some("object") {
        return Err(fail(
            "grammar constrained sampling requires an object parameter schema",
        ));
    }
    let required = tool
        .parameters
        .get("required")
        .and_then(Value::as_array)
        .filter(|a| a.len() == 1)
        .and_then(|a| a[0].as_str())
        .ok_or_else(|| {
            fail("grammar constrained sampling requires exactly one required string property")
        })?;
    let property = tool
        .parameters
        .get("properties")
        .and_then(|p| p.get(required))
        .ok_or_else(|| {
            fail(&format!(
                "grammar constrained sampling requires a properties entry for {required}"
            ))
        })?;
    if property.get("type").and_then(Value::as_str) != Some("string") {
        return Err(fail(&format!(
            "grammar constrained sampling property {required} must have type string"
        )));
    }
    Ok(Some(Grammar {
        format,
        definition: definition.into(),
        input_property: required.into(),
    }))
}

/// Reference `resolveJsonSchemaStrictSampling`: `Ok(Some(true))` to send the tool strict,
/// `Ok(None)` for non-strict, `Err` when a tool requires strict mode that cannot be used.
pub fn resolve_strict(
    req: &ProviderRequest,
    tool: &ToolSpec,
    supports_strict: bool,
    check: Option<&UnsupportedKeyword>,
) -> Result<Option<bool>, String> {
    let Some(config) = constrained_sampling(req, tool) else {
        return Ok(None);
    };
    if config.get("type").and_then(Value::as_str) != Some("json_schema") {
        return Ok(None);
    }
    let require = config.get("strict").and_then(Value::as_str) == Some("require");
    if supports_strict {
        return match make_strict_json_schema(&tool.parameters, check) {
            Ok(_) => Ok(Some(true)),
            Err(_) if !require => Ok(None),
            Err(e) => Err(format!(
                "Tool \"{}\" requires JSON-schema constrained sampling, but {e}.",
                tool.name
            )),
        };
    }
    if require {
        return Err(format!(
            "Tool \"{}\" requires JSON-schema constrained sampling, but strict tools are unsupported.",
            tool.name
        ));
    }
    Ok(None)
}

/// The parameters to send: the strict form when `strict`, else the schema as given.
pub fn tool_parameters(tool: &ToolSpec, strict: Option<bool>) -> Result<Value, String> {
    if strict == Some(true) {
        make_strict_json_schema(&tool.parameters, None)
    } else {
        Ok(tool.parameters.clone())
    }
}
