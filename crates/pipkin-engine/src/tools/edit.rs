//! `edit`: exact (then fuzzy) text replacement (`tools/edit.ts`, `tools/edit-diff.ts`).

use serde_json::{Map, Value, json};
use unicode_normalization::UnicodeNormalization;

use super::diff::{display_diff, unified_patch};
use super::path::{file_error_code, resolve_tool_path};
use super::text::js_trim_end;
use super::{Tool, ToolCtx, ToolError, ToolOutput, files, spec_from_json};
use crate::provider::{BoxFuture, ToolSpec};
use pipkin_engine_protocol::Block;

const SCHEMA: &str = r#"{"type":"object","required":["path","edits"],"properties":{"path":{"type":"string","description":"Path to the file to edit (relative or absolute)"},"edits":{"type":"array","items":{"type":"object","required":["oldText","newText"],"properties":{"oldText":{"type":"string","description":"Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."},"newText":{"type":"string","description":"Replacement text for this targeted edit."}}},"description":"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead."}}}"#;

const DESCRIPTION: &str = "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.";

pub struct EditTool;

fn is_single_edit(value: &Value) -> bool {
    value.get("oldText").is_some_and(Value::is_string)
        && value.get("newText").is_some_and(Value::is_string)
}

/// `prepareEditArguments`: `edits` as a JSON string or as a single edit object, and a
/// top-level `oldText`/`newText` pair, are repaired. Works on a copy.
pub fn prepare_edit_arguments(input: Value) -> Value {
    let Value::Object(mut args) = input else {
        return input;
    };
    match args.get("edits") {
        Some(Value::String(text)) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                if parsed.is_array() {
                    args.insert("edits".into(), parsed);
                } else if parsed.is_object() && is_single_edit(&parsed) {
                    args.insert("edits".into(), Value::Array(vec![parsed]));
                }
            }
        }
        Some(edit) if edit.is_object() && is_single_edit(edit) => {
            let edit = edit.clone();
            args.insert("edits".into(), Value::Array(vec![edit]));
        }
        _ => {}
    }
    let (Some(Value::String(old_text)), Some(Value::String(new_text))) =
        (args.get("oldText"), args.get("newText"))
    else {
        return Value::Object(args);
    };
    let pair = json!({ "oldText": old_text, "newText": new_text });
    let mut edits = match args.get("edits") {
        Some(Value::Array(edits)) => edits.clone(),
        _ => vec![],
    };
    edits.push(pair);
    let mut rest: Map<String, Value> = Map::new();
    for (key, value) in args {
        if key != "oldText" && key != "newText" {
            rest.insert(key, value);
        }
    }
    rest.insert("edits".into(), Value::Array(edits));
    Value::Object(rest)
}

#[derive(Clone, Debug)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

pub fn detect_line_ending(content: &str) -> &'static str {
    let Some(lf) = content.find('\n') else {
        return "\n";
    };
    match content.find("\r\n") {
        Some(crlf) if crlf < lf => "\r\n",
        _ => "\n",
    }
}

pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_owned()
    }
}

/// `normalizeForFuzzyMatch`: NFKC, trailing whitespace per line, smart quotes, dashes and
/// special spaces.
pub fn normalize_for_fuzzy_match(text: &str) -> String {
    let nfkc: String = text.nfkc().collect();
    let trimmed: Vec<&str> = nfkc.split('\n').map(js_trim_end).collect();
    trimmed
        .join("\n")
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
            c => c,
        })
        .collect()
}

/// Lines with their endings (`/[^\n]*\n|[^\n]+/g`).
fn split_lines_with_endings(content: &str) -> Vec<&str> {
    content.split_inclusive('\n').collect()
}

#[derive(Clone, Copy)]
struct Replacement<'a> {
    edit_index: usize,
    index: usize,
    length: usize,
    new_text: &'a str,
}

fn line_spans(content: &str) -> Vec<(usize, usize)> {
    let mut offset = 0;
    split_lines_with_endings(content)
        .into_iter()
        .map(|line| {
            let span = (offset, offset + line.len());
            offset = span.1;
            span
        })
        .collect()
}

fn replacement_line_range(
    lines: &[(usize, usize)],
    r: &Replacement<'_>,
) -> Result<(usize, usize), String> {
    let start = r.index;
    let end = r.index + r.length;
    let outside = || "Replacement range is outside the base content.".to_owned();
    let start_line = lines
        .iter()
        .position(|(s, e)| start >= *s && start < *e)
        .ok_or_else(outside)?;
    let mut end_line = start_line;
    while end_line < lines.len() && lines[end_line].1 < end {
        end_line += 1;
    }
    if end_line >= lines.len() {
        return Err(outside());
    }
    Ok((start_line, end_line + 1))
}

fn apply_replacements(content: &str, replacements: &[Replacement<'_>], offset: usize) -> String {
    let mut result = content.to_owned();
    for r in replacements.iter().rev() {
        let at = r.index - offset;
        result.replace_range(at..at + r.length, r.new_text);
    }
    result
}

/// `applyReplacementsPreservingUnchangedLines`.
fn apply_preserving_unchanged_lines(
    original: &str,
    base: &str,
    replacements: &[Replacement<'_>],
) -> Result<String, String> {
    let original_lines = split_lines_with_endings(original);
    let base_lines = line_spans(base);
    if original_lines.len() != base_lines.len() {
        return Err(
            "Cannot preserve unchanged lines because the base content has a different line count."
                .into(),
        );
    }
    let mut sorted = replacements.to_vec();
    sorted.sort_by_key(|r| r.index);
    let mut groups: Vec<(usize, usize, Vec<Replacement<'_>>)> = Vec::new();
    for r in sorted {
        let (start, end) = replacement_line_range(&base_lines, &r)?;
        if let Some(current) = groups.last_mut()
            && start < current.1
        {
            current.1 = current.1.max(end);
            current.2.push(r);
            continue;
        }
        groups.push((start, end, vec![r]));
    }
    let mut original_index = 0;
    let mut result = String::new();
    for (start, end, group) in &groups {
        result.push_str(&original_lines[original_index..*start].concat());
        let from = base_lines[*start].0;
        let to = base_lines[end - 1].1;
        result.push_str(&apply_replacements(&base[from..to], group, from));
        original_index = *end;
    }
    result.push_str(&original_lines[original_index..].concat());
    Ok(result)
}

struct Found {
    index: usize,
    length: usize,
    fuzzy: bool,
}

/// `fuzzyFindText`: exact first, then in fuzzy-normalized space.
fn fuzzy_find(content: &str, old_text: &str) -> Option<Found> {
    if let Some(index) = content.find(old_text) {
        return Some(Found {
            index,
            length: old_text.len(),
            fuzzy: false,
        });
    }
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old_text);
    fuzzy_content.find(&fuzzy_old).map(|index| Found {
        index,
        length: fuzzy_old.len(),
        fuzzy: true,
    })
}

fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old_text);
    if fuzzy_old.is_empty() {
        // `"abc".split("")` has one piece per character.
        return fuzzy_content.encode_utf16().count().saturating_sub(1);
    }
    fuzzy_content.matches(fuzzy_old.as_str()).count()
}

/// `applyEditsToNormalizedContent`: returns `(base, new)` content, both LF-normalized.
pub fn apply_edits(
    normalized: &str,
    edits: &[Edit],
    path: &str,
) -> Result<(String, String), String> {
    let edits: Vec<Edit> = edits
        .iter()
        .map(|e| Edit {
            old_text: normalize_to_lf(&e.old_text),
            new_text: normalize_to_lf(&e.new_text),
        })
        .collect();
    let total = edits.len();
    for (i, edit) in edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(if total == 1 {
                format!("oldText must not be empty in {path}.")
            } else {
                format!("edits[{i}].oldText must not be empty in {path}.")
            });
        }
    }
    let used_fuzzy = edits
        .iter()
        .any(|e| fuzzy_find(normalized, &e.old_text).is_some_and(|f| f.fuzzy));
    let base_for_replacement = if used_fuzzy {
        normalize_for_fuzzy_match(normalized)
    } else {
        normalized.to_owned()
    };
    let mut matched: Vec<Replacement<'_>> = Vec::new();
    for (i, edit) in edits.iter().enumerate() {
        let Some(found) = fuzzy_find(&base_for_replacement, &edit.old_text) else {
            return Err(if total == 1 {
                format!(
                    "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
                )
            } else {
                format!(
                    "Could not find edits[{i}] in {path}. The oldText must match exactly including all whitespace and newlines."
                )
            });
        };
        let occurrences = count_occurrences(&base_for_replacement, &edit.old_text);
        if occurrences > 1 {
            return Err(if total == 1 {
                format!(
                    "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
                )
            } else {
                format!(
                    "Found {occurrences} occurrences of edits[{i}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
                )
            });
        }
        matched.push(Replacement {
            edit_index: i,
            index: found.index,
            length: found.length,
            new_text: &edit.new_text,
        });
    }
    matched.sort_by_key(|r| r.index);
    for pair in matched.windows(2) {
        let (previous, current) = (&pair[0], &pair[1]);
        if previous.index + previous.length > current.index {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                previous.edit_index, current.edit_index
            ));
        }
    }
    let new_content = if used_fuzzy {
        apply_preserving_unchanged_lines(normalized, &base_for_replacement, &matched)?
    } else {
        apply_replacements(&base_for_replacement, &matched, 0)
    };
    if new_content == normalized {
        return Err(if total == 1 {
            format!(
                "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
            )
        } else {
            format!("No changes made to {path}. The replacements produced identical content.")
        });
    }
    Ok((normalized.to_owned(), new_content))
}

fn access_error(path: &str, error: &std::io::Error) -> ToolError {
    ToolError::new(format!(
        "Could not edit file: {path}. Error code: {}.",
        file_error_code(error)
    ))
}

impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        spec_from_json("edit", DESCRIPTION, SCHEMA)
    }

    fn prepare_arguments(&self, args: Value) -> Value {
        prepare_edit_arguments(args)
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let path = args["path"].as_str().unwrap_or_default().to_owned();
            let edits: Vec<Edit> = args["edits"]
                .as_array()
                .map(|edits| {
                    edits
                        .iter()
                        .map(|e| Edit {
                            old_text: e["oldText"].as_str().unwrap_or_default().to_owned(),
                            new_text: e["newText"].as_str().unwrap_or_default().to_owned(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if edits.is_empty() {
                return Err(ToolError::new(
                    "Edit tool input is invalid. edits must contain at least one replacement.",
                ));
            }
            let absolute = resolve_tool_path(&ctx.cwd, &path);
            let count = edits.len();
            let cancel = ctx.cancel.clone();
            files::with_mutation_lock(ctx, absolute.clone(), move || {
                let aborted = || ToolError::new("Operation aborted");
                if cancel.is_cancelled() {
                    return Err(aborted());
                }
                let info =
                    std::fs::symlink_metadata(&absolute).map_err(|e| access_error(&path, &e))?;
                let kind = info.file_type();
                if !kind.is_file() && !kind.is_symlink() {
                    if kind.is_dir() {
                        return Err(ToolError::new(format!(
                            "Could not edit file: {path}. Path is not a file."
                        )));
                    }
                    // Sockets, FIFOs and devices are an unsupported file type (`invalid`).
                    return Err(ToolError::new(format!(
                        "Could not edit file: {path}. Error code: invalid."
                    )));
                }
                let bytes = std::fs::read(&absolute).map_err(|e| access_error(&path, &e))?;
                let text = String::from_utf8_lossy(&bytes).into_owned();
                if cancel.is_cancelled() {
                    return Err(aborted());
                }
                let (bom, content) = match text.strip_prefix('\u{feff}') {
                    Some(rest) => ("\u{feff}", rest),
                    None => ("", text.as_str()),
                };
                let ending = detect_line_ending(content);
                let normalized = normalize_to_lf(content);
                let (base, new_content) =
                    apply_edits(&normalized, &edits, &path).map_err(ToolError::new)?;
                if cancel.is_cancelled() {
                    return Err(aborted());
                }
                let final_content = format!("{bom}{}", restore_line_endings(&new_content, ending));
                files::write_text(&absolute, &final_content)
                    .map_err(|(_, e)| access_error(&path, &e))?;
                if cancel.is_cancelled() {
                    return Err(aborted());
                }
                let (diff, first_changed) = display_diff(&base, &new_content, 4);
                let mut details = json!({
                    "diff": diff,
                    "patch": unified_patch(&path, &base, &new_content, 4),
                });
                if let Some(line) = first_changed {
                    details["firstChangedLine"] = json!(line);
                }
                Ok(ToolOutput {
                    content: Some(vec![Block::text(format!(
                        "Successfully replaced {count} block(s) in {path}."
                    ))]),
                    details: Some(details),
                    ..ToolOutput::default()
                })
            })
            .await
        })
    }
}
