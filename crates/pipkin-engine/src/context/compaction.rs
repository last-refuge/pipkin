//! Compaction's pure parts (reference `durable/src/harness/compaction.ts`, token estimates
//! from `ai/src/utils/estimate.ts`, thresholds from `durable/src/harness/generation.ts:303-322`).
//!
//! The session runtime owns the task: it decides when to compact ([`threshold_compaction`],
//! [`select_cut`]), sends the summarization request ([`SUMMARIZATION_SYSTEM_PROMPT`],
//! [`summary_request`], [`summary_max_tokens`]), checks the reply ([`validate_summary`]) and
//! records a compaction entry whose model text is [`summary_message_text`].

use pipkin_engine_protocol::{Block, Entry, EntryId, Message, StopReason, Usage};
use serde_json::Value;

use super::derive::{self, Slot};
use super::js;
use crate::config::CompactionSettings;

pub use super::derive::{SUMMARY_PREFIX, SUMMARY_SUFFIX, summary_message_text};

const CHARS_PER_TOKEN: usize = 4;
const ESTIMATED_IMAGE_CHARS: usize = 4800;
/// Longest tool result text the serialized summary source keeps.
pub const TOOL_RESULT_MAX_CHARS: usize = 2000;

pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

pub const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work. If the conversation starts with an earlier summary, preserve its information and fold the newer messages into it.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or \"(none)\" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

fn ceil_tokens(chars: usize) -> u64 {
    chars.div_ceil(CHARS_PER_TOKEN) as u64
}

/// `calculateContextTokens`: `totalTokens`, else the sum of the components.
pub fn calculate_context_tokens(usage: &Usage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input + usage.output + usage.cache_read + usage.cache_write
    }
}

/// `estimateTextTokens`: ceil(UTF-16 length / 4).
pub fn estimate_text_tokens(text: &str) -> u64 {
    ceil_tokens(js::len(text))
}

fn json_len(value: &Value) -> usize {
    js::len(&serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".to_string()))
}

/// `estimateMessageTokens`: user and tool-result text (non-text blocks count 4800 chars as
/// images); assistant text, thinking, and tool calls as name plus JSON arguments.
pub fn estimate_message_tokens(message: &Message) -> u64 {
    let mut chars = 0usize;
    match message {
        Message::System(system) => {
            let additions = if system.tools_added.is_empty() {
                0
            } else {
                estimate_text_tokens(
                    &serde_json::to_string(&system.tools_added).unwrap_or_default(),
                )
            };
            let removals = if system.tools_removed.is_empty() {
                0
            } else {
                estimate_text_tokens(
                    &serde_json::to_string(&system.tools_removed).unwrap_or_default(),
                )
            };
            return estimate_text_tokens(&super::system::text(system)) + additions + removals;
        }
        Message::User { content, .. } | Message::ToolResult { content, .. } => {
            for block in content {
                chars += match block {
                    Block::Text { text, .. } => js::len(text),
                    _ => ESTIMATED_IMAGE_CHARS,
                };
            }
        }
        Message::Assistant { content, .. } => {
            for block in content {
                chars += match block {
                    Block::Text { text, .. } => js::len(text),
                    Block::Thinking { thinking, .. } => js::len(thinking),
                    Block::ToolCall {
                        name, arguments, ..
                    } => js::len(name) + json_len(arguments),
                    Block::Image { .. } => ESTIMATED_IMAGE_CHARS,
                };
            }
        }
    }
    ceil_tokens(chars)
}

/// Sum of [`estimate_message_tokens`].
pub fn estimate_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message_tokens).sum()
}

/// [`estimate_context`] with nothing planned.
pub fn context_tokens(entries: &[Entry]) -> u64 {
    estimate_context(entries, &[])
}

/// `estimateContext` (compaction.ts:307-322): the usage of the newest assistant appended after
/// the head marker with non-zero context tokens (its request included everything before it),
/// plus estimates of the derived messages after it and of `extra` (messages the next request
/// adds, such as a system prompt not yet measured); without such an assistant, estimates of
/// every derived message plus `extra`.
///
/// Assistants of entries kept from before a compaction (ids below the marker's) do not count:
/// their usage measured the history the summary replaced.
pub fn estimate_context(entries: &[Entry], extra: &[Message]) -> u64 {
    let has_head = entries.first().is_some_and(derive::is_head_marker);
    let contributions: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| derive::borrowed_contribution(entry, has_head && index == 0))
        .collect();
    let after = entries
        .first()
        .filter(|entry| derive::is_head_marker(entry))
        .map(|entry| entry.id);

    let mut measured: Option<(usize, usize, u64)> = None;
    for (index, entry) in entries.iter().enumerate().rev() {
        if after.is_some_and(|after| entry.id <= after) {
            continue;
        }
        let found =
            contributions[index]
                .iter()
                .enumerate()
                .rev()
                .find_map(|(position, message)| match message.as_ref() {
                    Message::Assistant { usage, .. } if calculate_context_tokens(usage) > 0 => {
                        Some((index, position, calculate_context_tokens(usage)))
                    }
                    _ => None,
                });
        if found.is_some() {
            measured = found;
            break;
        }
    }

    // Flatten with provenance, order like the request, and find the measured assistant.
    let mut flat = Vec::new();
    let mut origin = Vec::new();
    for (index, messages) in contributions.iter().enumerate() {
        for (position, message) in messages.iter().enumerate() {
            flat.push(message.as_ref());
            origin.push((index, position));
        }
    }
    let slots = derive::order_slots(&flat);
    let (from, mut tokens) = match measured {
        Some((index, position, tokens)) => {
            let at = slots
                .iter()
                .rposition(|slot| matches!(slot, Slot::Input(i) if origin[*i] == (index, position)))
                .map_or(0, |at| at + 1);
            (at, tokens)
        }
        None => (0, 0),
    };
    for slot in &slots[from..] {
        tokens += match slot {
            Slot::Input(index) => estimate_message_tokens(flat[*index]),
            Slot::Missing {
                call_id,
                tool_name,
                timestamp,
            } => estimate_message_tokens(&derive::missing_result(call_id, tool_name, *timestamp)),
        };
    }
    tokens += estimate_tokens(extra);
    tokens
}

/// Which compaction a request's preparation starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Threshold {
    /// Over `contextWindow - reserveTokens`: compact before sending.
    Blocking,
    /// Over the background threshold: compact alongside the request.
    Background,
}

/// `(blocking, background)`: `contextWindow - reserveTokens` and that minus
/// `backgroundTokens`. Either can be negative for a small window.
pub fn thresholds(context_window: u64, settings: &CompactionSettings) -> (i64, i64) {
    let blocking = context_window as i64 - settings.reserve_tokens as i64;
    (blocking, blocking - settings.background_tokens as i64)
}

/// `thresholdCompaction` without the cut check: `None` when compaction is disabled, the window
/// is unknown (0) or `tokens` is under both thresholds. The caller still requires
/// [`select_cut`] to find a cut, and starts a background compaction only when none is running.
pub fn threshold_compaction(
    tokens: u64,
    context_window: u64,
    settings: &CompactionSettings,
) -> Option<Threshold> {
    if !settings.enabled || context_window == 0 {
        return None;
    }
    let (blocking, background) = thresholds(context_window, settings);
    let tokens = tokens as i64;
    if tokens > blocking {
        Some(Threshold::Blocking)
    } else if settings.background_tokens > 0 && tokens > background {
        Some(Threshold::Background)
    } else {
        None
    }
}

/// The summarization request's `maxTokens`: `floor(0.8 * reserveTokens)`, capped by the
/// model's `maxTokens` when that is positive.
pub fn summary_max_tokens(reserve_tokens: u64, model_max_tokens: u64) -> u64 {
    let budget = reserve_tokens * 4 / 5;
    if model_max_tokens > 0 {
        budget.min(model_max_tokens)
    } else {
        budget
    }
}

fn is_candidate(contributions: &[Vec<Message>], index: usize) -> bool {
    match contributions[index].first() {
        Some(Message::Assistant { .. }) => return true,
        Some(Message::User { .. }) => {}
        _ => return false,
    }
    // The calls of the nearest preceding assistant.
    let mut calls: Vec<&str> = Vec::new();
    for before in (0..index).rev() {
        let Some(assistant) = contributions[before]
            .iter()
            .rev()
            .find(|message| matches!(message, Message::Assistant { .. }))
        else {
            continue;
        };
        calls = assistant
            .tool_calls()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        break;
    }
    if calls.is_empty() {
        return true;
    }
    // A result of those calls that follows this entry, before the next assistant, belongs
    // before it: cutting here would split the call from its result.
    for (after, messages) in contributions.iter().enumerate().skip(index) {
        for (position, message) in messages.iter().enumerate() {
            match message {
                Message::Assistant { .. } if after > index || position > 0 => return true,
                Message::ToolResult { tool_call_id, .. }
                    if calls.contains(&tool_call_id.as_str()) =>
                {
                    return false;
                }
                _ => {}
            }
        }
    }
    true
}

/// `selectCut` over per-entry contributions: the index of the first kept entry.
pub(crate) fn select_cut_index(
    contributions: &[Vec<Message>],
    has_head: bool,
    keep_recent_tokens: u64,
) -> Option<usize> {
    let start = usize::from(has_head);
    let candidates: Vec<usize> = (start..contributions.len())
        .filter(|&index| is_candidate(contributions, index))
        .collect();
    let mut kept = 0u64;
    let mut cut = None;
    for index in (start..contributions.len()).rev() {
        kept += estimate_tokens(&contributions[index]);
        if kept < keep_recent_tokens {
            continue;
        }
        cut = candidates
            .iter()
            .copied()
            .find(|&candidate| candidate >= index)
            .or_else(|| candidates.last().copied());
        break;
    }
    let cut = cut?;
    (start..cut)
        .any(|index| !contributions[index].is_empty())
        .then_some(cut)
}

/// The first entry a summary keeps, or `None` when there is nothing to compact. Walks back
/// from the newest entry until `keep_recent_tokens` are kept, then cuts at the first
/// candidate at or after that entry (else the last candidate). A candidate is an entry whose
/// contribution starts with an assistant message, or with a user message that no result of
/// the preceding assistant's calls still follows. The head marker is never a cut, and a cut
/// with nothing but the marker before it is no cut.
pub fn select_cut(entries: &[Entry], keep_recent_tokens: u64) -> Option<EntryId> {
    let contributions = derive::contributions(entries);
    let has_head = entries.first().is_some_and(derive::is_head_marker);
    select_cut_index(&contributions, has_head, keep_recent_tokens).map(|index| entries[index].id)
}

/// The messages a summary replaces: the derived messages of the entries before `first_kept`
/// (the head marker first), ordered like a request. Empty when `first_kept` is not among the
/// entries.
pub fn summary_source(entries: &[Entry], first_kept: EntryId) -> Vec<Message> {
    let Some(cut) = entries.iter().position(|entry| entry.id == first_kept) else {
        return vec![];
    };
    let contributions = derive::contributions(entries);
    derive::order_tool_results(contributions[..cut].iter().flatten().cloned().collect())
}

fn text_of(content: &[Block]) -> String {
    content
        .iter()
        .filter_map(Block::as_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate(text: &str, max: usize) -> String {
    let length = js::len(text);
    if length <= max {
        return text.to_string();
    }
    format!(
        "{}\n\n[... {} more characters truncated]",
        js::prefix(text, max),
        length - max
    )
}

/// `key=JSON` pairs of a tool call's arguments (`Object.entries`).
fn call_arguments(arguments: &Value) -> String {
    let render = |value: &Value| serde_json::to_string(value).unwrap_or_default();
    match arguments {
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| format!("{key}={}", render(value)))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, value)| format!("{index}={}", render(value)))
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

/// `serializeConversation`: a plain transcript so the summarizer reads it instead of
/// continuing it. `[User]: `, `[Assistant thinking]: `, `[Assistant]: `,
/// `[Assistant tool calls]: name(k=JSON, ...); ...`, `[Tool result]: ` (truncated to 2000
/// UTF-16 units plus a note); empty parts are skipped; parts are joined by a blank line.
pub fn serialize_for_summary(messages: &[Message]) -> String {
    let mut parts = Vec::new();
    for message in messages {
        match message {
            Message::System(_) => {}
            Message::User { content, .. } => {
                let text = text_of(content);
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            Message::Assistant { content, .. } => {
                let mut thinking = Vec::new();
                let mut text = Vec::new();
                let mut calls = Vec::new();
                for block in content {
                    match block {
                        Block::Thinking { thinking: t, .. } => thinking.push(t.as_str()),
                        Block::Text { text: t, .. } => text.push(t.as_str()),
                        Block::ToolCall {
                            name, arguments, ..
                        } => calls.push(format!("{name}({})", call_arguments(arguments))),
                        Block::Image { .. } => {}
                    }
                }
                if !thinking.is_empty() {
                    parts.push(format!("[Assistant thinking]: {}", thinking.join("\n")));
                }
                if !text.is_empty() {
                    parts.push(format!("[Assistant]: {}", text.join("\n")));
                }
                if !calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", calls.join("; ")));
                }
            }
            Message::ToolResult { content, .. } => {
                let text = text_of(content);
                if !text.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate(&text, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
        }
    }
    parts.join("\n\n")
}

/// The fixed summarization instructions, plus `"\n\nAdditional focus: <instructions>"` for a
/// manual compaction with instructions.
pub fn summarization_prompt(instructions: Option<&str>) -> String {
    match instructions {
        Some(focus) => format!("{SUMMARIZATION_PROMPT}\n\nAdditional focus: {focus}"),
        None => SUMMARIZATION_PROMPT.to_string(),
    }
}

/// The summarizer's user message: the serialized conversation in `<conversation>` tags, then
/// [`summarization_prompt`]. The request is `[system: SUMMARIZATION_SYSTEM_PROMPT, user: this]`
/// with no tools and no prompt caching.
pub fn summary_request(messages: &[Message], instructions: Option<&str>) -> String {
    format!(
        "<conversation>\n{}\n</conversation>\n\n{}",
        serialize_for_summary(messages),
        summarization_prompt(instructions)
    )
}

/// The summary of a clean reply (`stop`, no tool call, non-empty trimmed text, text blocks
/// joined by `\n`), or the reference's failure text.
pub fn validate_summary(message: &Message) -> Result<String, String> {
    let Message::Assistant {
        content,
        stop_reason,
        error_message,
        ..
    } = message
    else {
        return Err("Summarization produced no text".to_string());
    };
    let has_call = content
        .iter()
        .any(|block| matches!(block, Block::ToolCall { .. }));
    if *stop_reason == StopReason::Stop && !has_call {
        let text = text_of(content);
        let text = js::trim(&text);
        if !text.is_empty() {
            return Ok(text.to_string());
        }
    }
    Err(match stop_reason {
        StopReason::Error | StopReason::Aborted => format!(
            "Summarization failed: {}",
            error_message.clone().unwrap_or_else(|| {
                if *stop_reason == StopReason::Error {
                    "error"
                } else {
                    "aborted"
                }
                .to_string()
            })
        ),
        StopReason::Length => {
            "Summarization hit the token limit; the summary is incomplete".to_string()
        }
        _ if has_call => "Summarization attempted to call a tool".to_string(),
        _ => "Summarization produced no text".to_string(),
    })
}

#[cfg(test)]
mod tests;
