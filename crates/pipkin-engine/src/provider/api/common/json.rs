//! JSON repair for streamed tool arguments (reference `utils/json-parse.ts` `repairJson` /
//! `parseJsonWithRepair`) and the shared way adapters settle a tool call's arguments.

use serde_json::{Value, json};

use crate::provider::Emitter;

/// Repair malformed JSON string literals the way the reference `repairJson` does: escape raw
/// control characters inside strings and double backslashes before invalid escapes.
pub fn repair_json(json: &str) -> String {
    let chars: Vec<char> = json.chars().collect();
    let mut out = String::with_capacity(json.len() + 8);
    let mut in_string = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !in_string {
            out.push(c);
            if c == '"' {
                in_string = true;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            out.push(c);
            in_string = false;
            i += 1;
            continue;
        }
        if c == '\\' {
            match chars.get(i + 1) {
                None => {
                    out.push_str("\\\\");
                    i += 1;
                }
                Some('u')
                    if chars.len() >= i + 6
                        && chars[i + 2..i + 6].iter().all(char::is_ascii_hexdigit) =>
                {
                    out.push_str("\\u");
                    out.extend(&chars[i + 2..i + 6]);
                    i += 6;
                }
                Some(n @ ('"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u')) => {
                    out.push('\\');
                    out.push(*n);
                    i += 2;
                }
                Some(_) => {
                    out.push_str("\\\\");
                    i += 1;
                }
            }
            continue;
        }
        match c {
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) <= 0x1f => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// Reference `parseJsonWithRepair`: parse, retrying once with [`repair_json`] when that
/// changes anything; otherwise the original parse error.
pub fn parse_json_with_repair(json: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(json).or_else(|e| {
        let repaired = repair_json(json);
        if repaired != json {
            serde_json::from_str(&repaired)
        } else {
            Err(e)
        }
    })
}

/// A tool call's final arguments as streamed.
pub enum ToolArgs {
    /// A JSON argument string.
    Json(String),
    /// A grammar (custom) tool's raw input, sent as `{"input": ...}`.
    Custom(String),
}

/// Settle tool arguments on the emitter. JSON is parsed with repair; arguments that still do
/// not parse to an object are handed to the emitter raw, so the message never carries an
/// executable call with guessed arguments (the emitter turns them into an error at finish).
pub fn finalize_tool_args(e: &mut Emitter, args: impl IntoIterator<Item = (usize, ToolArgs)>) {
    for (index, args) in args {
        match args {
            ToolArgs::Custom(input) => e.tool_args_value(index, json!({"input": input})),
            ToolArgs::Json(raw) => {
                if raw.trim().is_empty() {
                    e.tool_args_value(index, json!({}));
                } else if let Some(v) = parse_json_with_repair(&raw).ok().filter(Value::is_object) {
                    e.tool_args_value(index, v);
                } else {
                    e.tool_args(index, &raw);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_escapes_control_characters_and_bad_escapes() {
        let raw = "{\"a\":\"line\nbreak\",\"b\":\"C:\\path\",\"c\":\"\\u00e9\"}";
        let v = parse_json_with_repair(raw).unwrap();
        assert_eq!(v["a"], "line\nbreak");
        assert_eq!(v["b"], "C:\\path");
        assert_eq!(v["c"], "\u{e9}");
        assert!(parse_json_with_repair("{\"a\":\"cut").is_err());
        assert_eq!(repair_json("{\"a\":\"x\ty\"}"), "{\"a\":\"x\\ty\"}");
        assert_eq!(repair_json("{\"a\":\"c:\\d\"}"), "{\"a\":\"c:\\\\d\"}");
        assert_eq!(repair_json("{\"a\":\"\\u00e9\"}"), "{\"a\":\"\\u00e9\"}");
        assert!(parse_json_with_repair("{\"a\":\"x\u{1}\"}").is_ok());
    }
}
