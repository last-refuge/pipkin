//! Previewed import of the common `mcpServers` configuration shape (Claude Desktop, Cursor,
//! Pi and others): each entry is converted to Pipkin's settings entry, validated, and shown
//! with its warnings before anything is written. Nothing is launched by an import.

use serde_json::{Map, Value, json};

use super::config::{REDACTED, parse_server, valid_name};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportEntry {
    pub name: String,
    /// `stdio` or `http`.
    pub kind: String,
    /// The converted settings entry, literal values included (never shown; written on apply).
    pub spec: Value,
    /// The converted entry with literals redacted, for display.
    pub redacted: Value,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPreview {
    pub entries: Vec<ImportEntry>,
    pub warnings: Vec<String>,
}

/// `${VAR}` and `${env:VAR}` become Pipkin's `$VAR` reference; anything else stays literal.
fn convert_value(raw: &str) -> String {
    let inner = raw
        .strip_prefix("${")
        .and_then(|s| s.strip_suffix('}'))
        .map(|s| s.strip_prefix("env:").unwrap_or(s));
    match inner {
        Some(name)
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
        {
            format!("${name}")
        }
        _ => raw.to_owned(),
    }
}

fn convert_map(value: Option<&Value>, key: &str, warnings: &mut Vec<String>) -> Option<Value> {
    let map = value?.as_object()?;
    let mut out = Map::new();
    for (k, v) in map {
        match v.as_str() {
            Some(s) => {
                out.insert(k.clone(), Value::String(convert_value(s)));
            }
            None => warnings.push(format!("{key}.{k} is not a string; left out")),
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

fn redact(spec: &Value) -> Value {
    let mut out = spec.clone();
    for key in ["env", "headers"] {
        if let Some(Value::Object(map)) = out.get_mut(key) {
            for v in map.values_mut() {
                if let Some(s) = v.as_str()
                    && !s.starts_with('$')
                    && !s.starts_with("secret:")
                {
                    *v = Value::String(REDACTED.into());
                }
            }
        }
    }
    out
}

fn convert_entry(name: &str, body: &Value) -> ImportEntry {
    let mut warnings = Vec::new();
    let mut spec = Map::new();
    let kind;
    let Some(obj) = body.as_object() else {
        return ImportEntry {
            name: name.to_owned(),
            kind: "unknown".into(),
            spec: Value::Null,
            redacted: Value::Null,
            warnings: vec!["entry is not an object".into()],
        };
    };
    let text = |k: &str| {
        obj.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let declared = text("type")
        .or(text("transport"))
        .map(str::to_ascii_lowercase);
    let url = text("url").or(text("serverUrl")).or(text("httpUrl"));
    if let Some(command) = text("command") {
        kind = "stdio";
        spec.insert("command".into(), json!(command));
        if let Some(args) = obj.get("args") {
            match args.as_array() {
                Some(items) if items.iter().all(Value::is_string) => {
                    spec.insert("args".into(), args.clone());
                }
                _ => warnings.push("args must be a list of strings; left out".into()),
            }
        }
        if let Some(cwd) = text("cwd") {
            spec.insert("cwd".into(), json!(cwd));
        }
        if let Some(env) = convert_map(obj.get("env"), "env", &mut warnings) {
            spec.insert("env".into(), env);
        }
        if obj.get("url").is_some() {
            warnings.push("both command and url were given; the command was kept".into());
        }
    } else if let Some(url) = url {
        kind = "http";
        spec.insert("url".into(), json!(url));
        if let Some(headers) = convert_map(obj.get("headers"), "headers", &mut warnings) {
            spec.insert("headers".into(), headers);
        }
        if declared.as_deref() == Some("sse") {
            warnings.push(
                "declared as legacy SSE; Pipkin connects with Streamable HTTP, which this server may not support"
                    .into(),
            );
        }
    } else {
        return ImportEntry {
            name: name.to_owned(),
            kind: declared.unwrap_or_else(|| "unknown".into()),
            spec: Value::Null,
            redacted: Value::Null,
            warnings: vec!["neither command nor url was given".into()],
        };
    }
    if obj.get("disabled").and_then(Value::as_bool) == Some(true)
        || obj.get("enabled").and_then(Value::as_bool) == Some(false)
    {
        spec.insert("enabled".into(), json!(false));
    }
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "command"
                | "args"
                | "cwd"
                | "env"
                | "url"
                | "serverUrl"
                | "httpUrl"
                | "headers"
                | "type"
                | "transport"
                | "disabled"
                | "enabled"
                | "name"
                | "description"
        ) {
            warnings.push(format!("{key} is not understood; left out"));
        }
    }
    let spec = Value::Object(spec);
    if let Err(problem) = parse_server(name, &spec, "user") {
        warnings.push(problem);
    }
    ImportEntry {
        name: name.to_owned(),
        kind: kind.into(),
        redacted: redact(&spec),
        spec,
        warnings,
    }
}

/// Convert pasted configuration text. Accepts `{"mcpServers": {...}}`, `{"servers": {...}}`
/// or a bare `{name: entry}` object.
pub fn preview(text: &str) -> ImportPreview {
    let mut preview = ImportPreview::default();
    let text = text.trim().trim_start_matches('\u{feff}');
    if text.is_empty() {
        preview.warnings.push("nothing to import".into());
        return preview;
    }
    let value: Value = match serde_json::from_str(&crate::config::strip_json_comments(text)) {
        Ok(v) => v,
        Err(e) => {
            preview.warnings.push(format!("not valid JSON: {e}"));
            return preview;
        }
    };
    let Some(root) = value.as_object() else {
        preview
            .warnings
            .push("the top level must be an object".into());
        return preview;
    };
    let servers = root
        .get("mcpServers")
        .or_else(|| root.get("servers"))
        .or_else(|| root.get("mcp").and_then(|m| m.get("servers")))
        .and_then(Value::as_object)
        .unwrap_or(root);
    for (name, body) in servers {
        if !valid_name(name) {
            preview.warnings.push(format!(
                "{name:?} is not a valid server name (letters, digits, - _ .); skipped"
            ));
            continue;
        }
        preview.entries.push(convert_entry(name, body));
    }
    if preview.entries.is_empty() {
        preview.warnings.push("no server entries were found".into());
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_the_common_shapes_with_warnings_and_without_launching() {
        let p = preview(
            r#"{"mcpServers": {
            "fs": {"command": "npx", "args": ["-y", "srv"], "env": {"TOKEN": "${TOKEN}", "PLAIN": "x"}, "extra": 1},
            "remote": {"type": "sse", "url": "https://r.example/mcp", "headers": {"Authorization": "Bearer ${env:T}"}},
            "off": {"command": "x", "disabled": true},
            "nothing": {"type": "http"},
            "bad name": {"command": "x"}
        }}"#,
        );
        assert_eq!(p.entries.len(), 4);
        let fs = &p.entries[0];
        assert_eq!(fs.kind, "stdio");
        assert_eq!(
            fs.spec,
            json!({"command": "npx", "args": ["-y", "srv"], "env": {"TOKEN": "$TOKEN", "PLAIN": "x"}})
        );
        assert_eq!(fs.redacted["env"], json!({"TOKEN": "$TOKEN", "PLAIN": "…"}));
        assert_eq!(fs.warnings, ["extra is not understood; left out"]);
        let remote = &p.entries[1];
        assert_eq!(remote.kind, "http");
        assert_eq!(remote.spec["headers"]["Authorization"], "Bearer ${env:T}");
        assert!(remote.warnings.iter().any(|w| w.contains("legacy SSE")));
        assert_eq!(p.entries[2].spec["enabled"], false);
        assert_eq!(p.entries[3].warnings, ["neither command nor url was given"]);
        assert!(
            p.warnings
                .iter()
                .any(|w| w.contains("not a valid server name"))
        );
        // Bare maps and invalid text.
        assert_eq!(
            preview(r#"{"a": {"url": "http://h/mcp"}}"#).entries[0].name,
            "a"
        );
        assert!(preview("nope").warnings[0].contains("not valid JSON"));
        assert!(preview("[]").warnings[0].contains("top level"));
        assert!(preview("{}").warnings[0].contains("no server entries"));
        assert!(preview("").warnings[0].contains("nothing to import"));
    }
}
