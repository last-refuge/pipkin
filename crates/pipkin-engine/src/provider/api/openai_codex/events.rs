//! Codex event mapping and the Responses stream processor.
//!
//! Ports reference `mapCodexEvents` (`openai-codex-responses.ts:743-793`) and the shared
//! `processResponsesStream` / `mapStopReason` (`openai-responses-shared.ts:433-809`) into one
//! state machine fed one parsed JSON event at a time. Content goes through the engine
//! [`Emitter`]; scratch state (partial JSON, slots) lives here and never reaches the message.
//!
//! PRIVATE DUPLICATE of the shared Responses processor (see `convert.rs`).

use std::collections::HashMap;

use pipkin_engine_protocol::{Block, Message, StopReason};
use serde_json::{Map, Value};

use super::convert::encode_text_signature;
use super::{Failure, FailureKind};
use crate::provider::Emitter;

/// Reference `CODEX_RESPONSE_STATUSES`.
const CODEX_RESPONSE_STATUSES: [&str; 6] = [
    "completed",
    "incomplete",
    "failed",
    "cancelled",
    "queued",
    "in_progress",
];

#[derive(Debug)]
enum SlotKind {
    Thinking,
    Text,
    /// `partial_json` is `None` once the call finished (reference deletes `partialJson`).
    Function {
        partial_json: Option<String>,
    },
    /// Grammar/custom tool call: `(property, input)` until finished.
    Custom {
        input: Option<(String, String)>,
    },
}

#[derive(Debug)]
struct Slot {
    kind: SlotKind,
    index: usize,
}

/// One tool call block, for the "unfinished tool call" check and continuation replay.
#[derive(Debug)]
struct ToolBlock {
    index: usize,
    id: String,
    name: String,
    finished: bool,
    /// Final arguments as parsed here (the emitter parses its own copy at finish).
    arguments: Value,
}

/// Whether the caller should keep reading.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Continue,
    /// A terminal response event was processed; stop reading.
    Done,
}

/// Processor state for one exchange.
#[derive(Debug, Default)]
pub(crate) struct ResponsesState {
    slots: HashMap<i64, Slot>,
    reasoning_by_id: HashMap<String, usize>,
    tools: Vec<ToolBlock>,
    /// Events handed to the Responses processor (reference "message stream started").
    pub(crate) events_processed: u64,
    pub(crate) saw_terminal: bool,
    /// `None` = pending.
    pub(crate) stop: Option<StopReason>,
    pub(crate) error_message: Option<String>,
    pub(crate) raw_stop_reason: Option<String>,
    pub(crate) end_turn: Option<bool>,
    pub(crate) response_id: Option<String>,
    /// The tier that prices this response (`None` = no terminal event seen).
    pub(crate) priced_tier: Option<Option<String>>,
    requested_tier: Option<String>,
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn u(v: &Value, key: &str) -> u64 {
    v.get(key)
        .and_then(|n| n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)))
        .unwrap_or(0)
}

/// JS template-literal rendering of an optional string field.
fn js_str(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) => "null".into(),
        None => "undefined".into(),
        Some(other) => other.to_string(),
    }
}

/// Reference `resolveCodexServiceTier`.
pub(crate) fn resolve_codex_service_tier(
    response: Option<&str>,
    request: Option<&str>,
) -> Option<String> {
    if response == Some("default") && matches!(request, Some("flex" | "priority")) {
        return request.map(str::to_owned);
    }
    response.or(request).map(str::to_owned)
}

/// Reference `getServiceTierCostMultiplier`.
pub(crate) fn service_tier_multiplier(model_id: &str, tier: Option<&str>) -> f64 {
    match tier {
        Some("flex") => 0.5,
        Some("priority") => {
            if model_id == "gpt-5.5" {
                2.5
            } else {
                2.0
            }
        }
        _ => 1.0,
    }
}

/// Reference `mapStopReason`.
fn map_stop_reason(status: Option<&str>, incomplete: Option<&str>) -> (StopReason, Option<String>) {
    match status {
        None => (StopReason::Stop, None),
        Some("completed") | Some("in_progress") | Some("queued") => (StopReason::Stop, None),
        Some("incomplete") => {
            if incomplete == Some("max_output_tokens") {
                (StopReason::Length, None)
            } else {
                (
                    StopReason::Error,
                    Some(match incomplete {
                        Some(r) => format!("Response incomplete: {r}"),
                        None => "Response incomplete without a provider reason".into(),
                    }),
                )
            }
        }
        // failed | cancelled (statuses are normalized to the known set first).
        Some(_) => (StopReason::Error, None),
    }
}

impl ResponsesState {
    pub(crate) fn new(requested_tier: Option<String>) -> ResponsesState {
        ResponsesState {
            requested_tier,
            ..ResponsesState::default()
        }
    }

    pub(crate) fn started(&self) -> bool {
        self.events_processed > 0
    }

    /// Reference `mapCodexEvents` + one iteration of `processResponsesStream`.
    pub(crate) fn handle(&mut self, emitter: &mut Emitter, event: Value) -> Result<Step, Failure> {
        let Some(kind) = s(&event, "type").map(str::to_owned) else {
            return Ok(Step::Continue);
        };
        match kind.as_str() {
            "error" => {
                let nested = event.get("error").filter(|e| e.is_object());
                let code = s(&event, "code")
                    .or_else(|| nested.and_then(|n| s(n, "code")))
                    .map(str::to_owned);
                let message = s(&event, "message")
                    .or_else(|| nested.and_then(|n| s(n, "message")))
                    .map(str::to_owned);
                let text = message
                    .filter(|m| !m.is_empty())
                    .or_else(|| code.clone().filter(|c| !c.is_empty()))
                    .unwrap_or_else(|| event.to_string());
                return Err(Failure {
                    kind: FailureKind::Api,
                    message: format!("Codex error: {text}"),
                    code,
                });
            }
            "response.failed" => {
                let err = event.get("response").and_then(|r| r.get("error"));
                let code = err.and_then(|e| s(e, "code")).map(str::to_owned);
                let message = err
                    .and_then(|e| s(e, "message"))
                    .filter(|m| !m.is_empty())
                    .unwrap_or("Codex response failed")
                    .to_owned();
                return Err(Failure {
                    kind: FailureKind::Api,
                    message,
                    code,
                });
            }
            "response.done" | "response.completed" | "response.incomplete" => {
                let response = event.get("response").cloned().unwrap_or(Value::Null);
                if let Some(end_turn) = response.get("end_turn").and_then(Value::as_bool) {
                    self.end_turn = Some(end_turn);
                }
                self.events_processed += 1;
                self.finalize(emitter, &response);
                return Ok(Step::Done);
            }
            _ => {}
        }
        self.events_processed += 1;
        self.process(emitter, &kind, &event);
        Ok(Step::Continue)
    }

    fn output_index(event: &Value) -> i64 {
        event
            .get("output_index")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(-1)
    }

    fn slot(&self, event: &Value, want: fn(&SlotKind) -> bool) -> Option<usize> {
        self.slots
            .get(&Self::output_index(event))
            .filter(|slot| want(&slot.kind))
            .map(|slot| slot.index)
    }

    fn create_slot(&mut self, emitter: &mut Emitter, output_index: i64, item: &Value) -> bool {
        let (kind, index) = match s(item, "type") {
            Some("reasoning") => (SlotKind::Thinking, emitter.start_thinking()),
            Some("message") => (SlotKind::Text, emitter.start_text()),
            Some("function_call") => {
                let id = format!("{}|{}", js_str(item, "call_id"), js_str(item, "id"));
                let name = s(item, "name").unwrap_or_default().to_owned();
                let index = emitter.start_tool_call(&id, &name);
                if let Some(ns) = item.get("namespace") {
                    emitter.block_extra(index, "namespace", ns.clone());
                }
                self.tools.push(ToolBlock {
                    index,
                    id,
                    name,
                    finished: false,
                    arguments: Value::Object(Map::new()),
                });
                let partial = s(item, "arguments").unwrap_or_default().to_owned();
                (
                    SlotKind::Function {
                        partial_json: Some(partial),
                    },
                    index,
                )
            }
            Some("custom_tool_call") => {
                let id = format!("{}|{}", js_str(item, "call_id"), js_str(item, "id"));
                let name = s(item, "name").unwrap_or_default().to_owned();
                let index = emitter.start_tool_call(&id, &name);
                if let Some(ns) = item.get("namespace") {
                    emitter.block_extra(index, "namespace", ns.clone());
                }
                // No grammar tools are declared by this engine, so the property is "input".
                let input = s(item, "input").unwrap_or_default().to_owned();
                let mut args = Map::new();
                args.insert("input".into(), Value::String(input.clone()));
                self.tools.push(ToolBlock {
                    index,
                    id,
                    name,
                    finished: false,
                    arguments: Value::Object(args),
                });
                (
                    SlotKind::Custom {
                        input: Some(("input".into(), input)),
                    },
                    index,
                )
            }
            _ => return false,
        };
        self.slots.insert(output_index, Slot { kind, index });
        true
    }

    /// Replace a block's text with the provider's final copy, streaming any pure extension.
    fn finalize_text(emitter: &mut Emitter, index: usize, current: &str, final_text: &str) {
        if let Some(rest) = final_text.strip_prefix(current) {
            if !rest.is_empty() {
                match emitter.content().get(index) {
                    Some(Block::Thinking { .. }) => emitter.thinking(index, rest),
                    _ => emitter.text(index, rest),
                }
            }
        } else {
            emitter.set_block_text(index, final_text);
        }
    }

    fn block_text(emitter: &Emitter, index: usize) -> String {
        match emitter.content().get(index) {
            Some(Block::Text { text, .. }) => text.clone(),
            Some(Block::Thinking { thinking, .. }) => thinking.clone(),
            _ => String::new(),
        }
    }

    fn process(&mut self, emitter: &mut Emitter, kind: &str, event: &Value) {
        match kind {
            "response.created" => {
                if let Some(id) = event.get("response").and_then(|r| s(r, "id")) {
                    self.response_id = Some(id.to_owned());
                    emitter.set_response_id(id);
                }
            }
            "response.output_item.added" => {
                if let Some(item) = event.get("item") {
                    self.create_slot(emitter, Self::output_index(event), item);
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(index) = self.slot(event, |k| matches!(k, SlotKind::Thinking)) {
                    emitter.thinking(index, s(event, "delta").unwrap_or_default());
                }
            }
            "response.reasoning_summary_part.done" => {
                if let Some(index) = self.slot(event, |k| matches!(k, SlotKind::Thinking)) {
                    emitter.thinking(index, "\n\n");
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(index) = self.slot(event, |k| matches!(k, SlotKind::Text)) {
                    emitter.text(index, s(event, "delta").unwrap_or_default());
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(slot) = self.slots.get_mut(&Self::output_index(event))
                    && let SlotKind::Function {
                        partial_json: Some(partial),
                    } = &mut slot.kind
                {
                    partial.push_str(s(event, "delta").unwrap_or_default());
                }
            }
            "response.function_call_arguments.done" => {
                if let Some(slot) = self.slots.get_mut(&Self::output_index(event))
                    && let SlotKind::Function {
                        partial_json: Some(partial),
                    } = &mut slot.kind
                {
                    *partial = s(event, "arguments").unwrap_or_default().to_owned();
                }
            }
            "response.custom_tool_call_input.delta" => {
                if let Some(slot) = self.slots.get_mut(&Self::output_index(event))
                    && let SlotKind::Custom {
                        input: Some((_, input)),
                    } = &mut slot.kind
                {
                    input.push_str(s(event, "delta").unwrap_or_default());
                }
            }
            "response.custom_tool_call_input.done" => {
                if let Some(slot) = self.slots.get_mut(&Self::output_index(event))
                    && let SlotKind::Custom {
                        input: Some((_, input)),
                    } = &mut slot.kind
                {
                    *input = s(event, "input").unwrap_or_default().to_owned();
                }
            }
            "response.output_item.done" => {
                let Some(item) = event.get("item") else {
                    return;
                };
                let output_index = Self::output_index(event);
                if !self.slots.contains_key(&output_index) {
                    self.create_slot(emitter, output_index, item);
                }
                self.item_done(emitter, output_index, item);
            }
            _ => {}
        }
    }

    fn item_done(&mut self, emitter: &mut Emitter, output_index: i64, item: &Value) {
        let Some(slot) = self.slots.remove(&output_index) else {
            return;
        };
        let index = slot.index;
        let handled = match (s(item, "type"), &slot.kind) {
            (Some("reasoning"), SlotKind::Thinking) => {
                let join = |key: &str| -> String {
                    item.get(key)
                        .and_then(Value::as_array)
                        .map(|parts| {
                            parts
                                .iter()
                                .map(|p| s(p, "text").unwrap_or_default())
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        })
                        .unwrap_or_default()
                };
                let summary = join("summary");
                let content = join("content");
                let current = Self::block_text(emitter, index);
                let final_text = if !summary.is_empty() {
                    summary
                } else if !content.is_empty() {
                    content
                } else {
                    current.clone()
                };
                Self::finalize_text(emitter, index, &current, &final_text);
                emitter.thinking_signature(index, &item.to_string(), false);
                if let Some(id) = s(item, "id") {
                    self.reasoning_by_id.insert(id.to_owned(), index);
                }
                true
            }
            (Some("message"), SlotKind::Text) => {
                let text: String = item
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .map(|c| {
                                if s(c, "type") == Some("output_text") {
                                    s(c, "text").unwrap_or_default()
                                } else {
                                    s(c, "refusal").unwrap_or_default()
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let current = Self::block_text(emitter, index);
                Self::finalize_text(emitter, index, &current, &text);
                let signature = encode_text_signature(
                    &js_str(item, "id"),
                    s(item, "phase").filter(|p| !p.is_empty()),
                );
                emitter.block_extra(index, "textSignature", Value::String(signature));
                true
            }
            (
                Some("function_call"),
                SlotKind::Function {
                    partial_json: Some(partial),
                },
            ) => {
                let raw = s(item, "arguments")
                    .filter(|a| !a.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        if partial.is_empty() {
                            "{}".into()
                        } else {
                            partial.clone()
                        }
                    });
                // The emitter parses the final string at finish; an invalid string becomes an
                // error there instead of an executable call (engine policy).
                emitter.tool_args(index, &raw);
                if let Some(ns) = item.get("namespace") {
                    emitter.block_extra(index, "namespace", ns.clone());
                }
                if let Some(tool) = self.tools.iter_mut().find(|t| t.index == index) {
                    tool.finished = true;
                    tool.arguments = serde_json::from_str::<Value>(&raw)
                        .ok()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| Value::Object(Map::new()));
                }
                true
            }
            (
                Some("custom_tool_call"),
                SlotKind::Custom {
                    input: Some((property, input)),
                },
            ) => {
                let final_input = s(item, "input").map(str::to_owned).unwrap_or(input.clone());
                let mut args = Map::new();
                args.insert(property.clone(), Value::String(final_input));
                let args = Value::Object(args);
                emitter.tool_args_value(index, args.clone());
                if let Some(ns) = item.get("namespace") {
                    emitter.block_extra(index, "namespace", ns.clone());
                }
                if let Some(tool) = self.tools.iter_mut().find(|t| t.index == index) {
                    tool.finished = true;
                    tool.arguments = args;
                }
                true
            }
            _ => false,
        };
        if !handled {
            self.slots.insert(output_index, slot);
        }
    }

    /// Reference `finalizeResponse` for a (normalized) `response.completed`.
    fn finalize(&mut self, emitter: &mut Emitter, response: &Value) {
        self.saw_terminal = true;
        self.backfill_reasoning(emitter, response);
        if let Some(id) = s(response, "id") {
            self.response_id = Some(id.to_owned());
            emitter.set_response_id(id);
        }
        if let Some(usage) = response.get("usage").filter(|u| u.is_object()) {
            let details = usage
                .get("input_tokens_details")
                .cloned()
                .unwrap_or(Value::Null);
            let cached = u(&details, "cached_tokens");
            let cache_write = u(&details, "cache_write_tokens");
            let out = emitter.usage_mut();
            out.input = u(usage, "input_tokens").saturating_sub(cached + cache_write);
            out.output = u(usage, "output_tokens");
            out.cache_read = cached;
            out.cache_write = cache_write;
            out.total_tokens = u(usage, "total_tokens");
        }
        self.priced_tier = Some(resolve_codex_service_tier(
            s(response, "service_tier"),
            self.requested_tier.as_deref(),
        ));
        let status = s(response, "status").filter(|st| CODEX_RESPONSE_STATUSES.contains(st));
        let incomplete = response
            .get("incomplete_details")
            .and_then(|d| s(d, "reason"));
        self.raw_stop_reason = match (status, incomplete) {
            (st, Some(reason)) => Some(format!("{}.{reason}", st.unwrap_or("undefined"))),
            (Some(st), None) => Some(st.to_owned()),
            (None, None) => None,
        };
        let (stop, error) = map_stop_reason(status, incomplete);
        self.stop = Some(stop);
        self.error_message = error;
        if stop == StopReason::Stop && emitter.has_tool_calls() {
            self.stop = Some(StopReason::ToolUse);
        }
    }

    /// Azure-style backfill of `encrypted_content` missing from `output_item.done`.
    fn backfill_reasoning(&self, emitter: &mut Emitter, response: &Value) {
        let Some(items) = response.get("output").and_then(Value::as_array) else {
            return;
        };
        for item in items {
            if s(item, "type") != Some("reasoning") {
                continue;
            }
            let Some(encrypted) = item.get("encrypted_content").filter(|e| match e {
                Value::String(s) => !s.is_empty(),
                Value::Null | Value::Bool(false) => false,
                _ => true,
            }) else {
                continue;
            };
            let Some(&index) = s(item, "id").and_then(|id| self.reasoning_by_id.get(id)) else {
                continue;
            };
            let Some(Block::Thinking {
                signature: Some(sig),
                ..
            }) = emitter.content().get(index)
            else {
                continue;
            };
            let Ok(Value::Object(mut stored)) = serde_json::from_str::<Value>(sig) else {
                continue;
            };
            if stored
                .get("encrypted_content")
                .is_some_and(|e| !e.is_null() && e != "")
            {
                continue;
            }
            stored.insert("encrypted_content".into(), encrypted.clone());
            emitter.thinking_signature(index, &Value::Object(stored).to_string(), false);
        }
    }

    /// Post-stream checks (reference `processResponsesStream` tail + `assertSuccessfulOutput`).
    /// Returns the successful stop reason or the error text.
    pub(crate) fn outcome(&self) -> Result<StopReason, String> {
        if !self.saw_terminal {
            return Err("OpenAI Responses stream ended before a terminal response event".into());
        }
        if self.stop == Some(StopReason::ToolUse)
            && let Some(tool) = self.tools.iter().find(|t| !t.finished)
        {
            return Err(format!(
                "OpenAI Responses stream completed with an unfinished tool call: {} ({})",
                tool.name, tool.id
            ));
        }
        match self.stop {
            None => Err("Codex stream ended without a stop reason".into()),
            Some(StopReason::Error) | Some(StopReason::Aborted) => Err(self
                .error_message
                .clone()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "An unknown error occurred".into())),
            Some(stop) => Ok(stop),
        }
    }

    /// The assistant message as it will be persisted, for WebSocket continuation bookkeeping
    /// (tool arguments as parsed here, empty text blocks dropped like `Emitter::finish`).
    pub(crate) fn snapshot(
        &self,
        emitter: &Emitter,
        model: &crate::provider::Model,
        stop: StopReason,
    ) -> Message {
        let mut content: Vec<Block> = emitter.content().to_vec();
        for tool in &self.tools {
            if let Some(Block::ToolCall { arguments, .. }) = content.get_mut(tool.index) {
                *arguments = tool.arguments.clone();
            }
        }
        content.retain(|b| !matches!(b, Block::Text { text, .. } if text.is_empty()));
        Message::Assistant {
            content,
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            usage: Default::default(),
            stop_reason: stop,
            error_message: None,
            timestamp: 0,
            extra: Map::new(),
        }
    }
}
