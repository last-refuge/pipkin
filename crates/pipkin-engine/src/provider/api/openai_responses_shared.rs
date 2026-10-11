//! Shared pieces of the OpenAI Responses adapters (reference `api/openai-responses-shared.ts`):
//! context -> `input` items, tool definitions, and the typed event stream -> content blocks.
//!
//! Replay contract: a reasoning item is stored whole (JSON) as the thinking block's signature and
//! sent back verbatim; an assistant text block carries `textSignature` = `{"v":1,"id","phase"}`
//! so the message item keeps its id and phase; tool call ids are `"<call_id>|<item_id>"`.

use std::collections::HashMap;
use std::ops::ControlFlow;

use pipkin_engine_protocol::{Block, Message, StopReason};
use reqwest::header::HeaderMap;
use serde_json::{Map, Value, json};

use super::openai_completions::{ToolArgs, data_url, finalize_tool_args};
use super::transform::{
    Mapped, idle_timeout, mapped_level, post_json, sanitize_id_chars, short_hash, take_chars,
    transform_messages,
};
use crate::provider::model::ThinkingLevel;
use crate::provider::{Emitter, ProviderRequest, StreamCtx, ToolSpec, http};

// ---------------------------------------------------------------------------------------------
// Text signatures
// ---------------------------------------------------------------------------------------------

/// `{"v":1,"id":...,"phase":...}` in the reference's key order.
pub fn encode_text_signature(id: &str, phase: Option<&str>) -> String {
    let id = Value::String(id.to_owned()).to_string();
    match phase {
        Some(p) => format!(
            "{{\"v\":1,\"id\":{id},\"phase\":{}}}",
            Value::String(p.to_owned())
        ),
        None => format!("{{\"v\":1,\"id\":{id}}}"),
    }
}

/// A V1 signature, or a legacy plain item id.
pub fn parse_text_signature(signature: Option<&str>) -> Option<(String, Option<String>)> {
    let signature = signature.filter(|s| !s.is_empty())?;
    if signature.starts_with('{')
        && let Ok(v) = serde_json::from_str::<Value>(signature)
        && v.get("v").and_then(Value::as_f64) == Some(1.0)
        && let Some(id) = v.get("id").and_then(Value::as_str)
    {
        let phase = v
            .get("phase")
            .and_then(Value::as_str)
            .filter(|p| matches!(*p, "commentary" | "final_answer"))
            .map(str::to_owned);
        return Some((id.to_owned(), phase));
    }
    Some((signature.to_owned(), None))
}

// ---------------------------------------------------------------------------------------------
// Input conversion
// ---------------------------------------------------------------------------------------------

fn normalize_id_part(part: &str) -> String {
    let sanitized = take_chars(&sanitize_id_chars(part), 64);
    sanitized.trim_end_matches('_').to_owned()
}

fn foreign_item_id(item_id: &str) -> String {
    take_chars(&format!("fc_{}", short_hash(item_id)), 64)
}

fn tool_result_output(req: &ProviderRequest, content: &[Block]) -> Value {
    let text: Vec<&str> = content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let text = text.join("\n");
    let images: Vec<(&str, &str)> = content
        .iter()
        .filter_map(|b| match b {
            Block::Image {
                data, mime_type, ..
            } => Some((mime_type.as_str(), data.as_str())),
            _ => None,
        })
        .collect();
    if images.is_empty() || !req.model.supports_images() {
        let body = if !text.is_empty() {
            text
        } else if !images.is_empty() {
            "(see attached image)".into()
        } else {
            "(no tool output)".into()
        };
        return json!(body);
    }
    let mut out = Vec::new();
    if !text.is_empty() {
        out.push(json!({"type": "input_text", "text": text}));
    }
    for (mime, data) in images {
        out.push(
            json!({"type": "input_image", "detail": "auto", "image_url": data_url(mime, data)}),
        );
    }
    Value::Array(out)
}

/// Context -> Responses `input` (reference `convertResponsesMessages`). Tool call ids are kept
/// as `call|item` only for providers in `allowed_tool_call_providers`.
pub fn convert_messages(req: &ProviderRequest, allowed_tool_call_providers: &[&str]) -> Vec<Value> {
    let model = &req.model;
    let mut out: Vec<Value> = Vec::new();
    let allowed = allowed_tool_call_providers.contains(&model.provider.as_str());
    let normalize = |id: &str, source: &Message| -> String {
        if !allowed || !id.contains('|') {
            return normalize_id_part(id);
        }
        let mut parts = id.split('|');
        let call_id = normalize_id_part(parts.next().unwrap_or_default());
        let item_id = parts.next().unwrap_or_default();
        let foreign = match source {
            Message::Assistant { provider, api, .. } => {
                *provider != model.provider || *api != model.api
            }
            _ => true,
        };
        let mut item = if foreign {
            foreign_item_id(item_id)
        } else {
            normalize_id_part(item_id)
        };
        if !item.starts_with("fc_") {
            item = normalize_id_part(&format!("fc_{item}"));
        }
        format!("{call_id}|{item}")
    };
    let messages = transform_messages(&req.messages, model, Some(&normalize));
    let developer = model.reasoning && model.compat_bool("supportsDeveloperRole") != Some(false);
    if !req.system.is_empty() {
        out.push(
            json!({"role": if developer { "developer" } else { "system" }, "content": req.system}),
        );
    }

    let mut msg_index = 0usize;
    for msg in &messages {
        match msg {
            Message::User { content, .. } => {
                let parts: Vec<Value> = content
                    .iter()
                    .filter_map(|b| match b {
                        Block::Text { text, .. } => {
                            Some(json!({"type": "input_text", "text": text}))
                        }
                        Block::Image {
                            data, mime_type, ..
                        } => Some(json!({
                            "type": "input_image",
                            "detail": "auto",
                            "image_url": data_url(mime_type, data)
                        })),
                        _ => None,
                    })
                    .collect();
                if parts.is_empty() {
                    continue;
                }
                out.push(json!({"role": "user", "content": parts}));
            }
            Message::Assistant {
                content,
                provider,
                api,
                model: source_model,
                ..
            } => {
                let same_provider_api = *provider == model.provider && *api == model.api;
                let same_model = same_provider_api && *source_model == model.id;
                let different_model = same_provider_api && *source_model != model.id;
                let mut items: Vec<Value> = Vec::new();
                let mut text_index = 0usize;
                for block in content {
                    match block {
                        Block::Thinking { signature, .. } => {
                            if let Some(sig) = signature.as_deref().filter(|s| !s.is_empty()) {
                                // An unparseable signature is not a Responses reasoning item;
                                // it cannot be replayed here and is skipped.
                                if let Ok(item) = serde_json::from_str::<Value>(sig)
                                    && item.is_object()
                                {
                                    items.push(item);
                                }
                            }
                        }
                        Block::Text { text, extra } => {
                            let parsed = parse_text_signature(
                                extra.get("textSignature").and_then(Value::as_str),
                            );
                            let fallback = if text_index == 0 {
                                format!("msg_pi_{msg_index}")
                            } else {
                                format!("msg_pi_{msg_index}_{text_index}")
                            };
                            text_index += 1;
                            let (mut id, phase) = match parsed {
                                Some((id, phase)) if !id.is_empty() => (id, phase),
                                Some((_, phase)) => (fallback, phase),
                                None => (fallback, None),
                            };
                            if id.encode_utf16().count() > 64 {
                                id = format!("msg_{}", short_hash(&id));
                            }
                            let mut item = json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{"type": "output_text", "text": text, "annotations": []}],
                                "status": "completed",
                                "id": id,
                            });
                            if let Some(p) = phase {
                                item["phase"] = json!(p);
                            }
                            items.push(item);
                        }
                        Block::ToolCall {
                            id,
                            name,
                            arguments,
                            extra,
                        } => {
                            let mut parts = id.split('|');
                            let call_id = parts.next().unwrap_or_default();
                            let item_id = parts
                                .next()
                                .filter(|i| !different_model && i.starts_with("fc_"));
                            let mut item = json!({
                                "type": "function_call",
                                "call_id": call_id,
                                "name": name,
                                "arguments": arguments.to_string(),
                            });
                            if let Some(i) = item_id {
                                item["id"] = json!(i);
                            }
                            if same_model && let Some(ns) = extra.get("namespace") {
                                item["namespace"] = ns.clone();
                            }
                            items.push(item);
                        }
                        Block::Image { .. } => {}
                    }
                }
                if items.is_empty() {
                    continue;
                }
                out.extend(items);
            }
            Message::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                let call_id = tool_call_id.split('|').next().unwrap_or_default();
                out.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": tool_result_output(req, content),
                }));
            }
        }
        msg_index += 1;
    }
    out
}

/// Tool definitions (reference `convertResponsesTools`). `strict` is only sent when the
/// endpoint supports it; tools never request strict JSON-schema sampling here.
pub fn convert_tools(tools: &[ToolSpec], supports_strict_mode: bool) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let mut tool = json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            });
            if supports_strict_mode {
                tool["strict"] = json!(false);
            }
            tool
        })
        .collect()
}

/// `reasoning` / `include` / sampling for a Responses request. `skip_off` suppresses the
/// explicit "off" effort (GitHub Copilot); `xai_include` always asks for encrypted reasoning.
pub fn apply_reasoning(
    p: &mut Map<String, Value>,
    req: &ProviderRequest,
    effort: Option<ThinkingLevel>,
    skip_off: bool,
    xai_include: bool,
) {
    let model = &req.model;
    let summary = req
        .options
        .get("reasoningSummary")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let requested = effort.or(summary.map(|_| ThinkingLevel::Medium));
    if model.reasoning {
        if let Some(level) = requested {
            let value = match effort {
                Some(l) => mapped_level(model, l.as_str()).or(l.as_str()).to_owned(),
                None => level.as_str().to_owned(),
            };
            p.insert(
                "reasoning".into(),
                json!({"effort": value, "summary": summary.unwrap_or("auto")}),
            );
            p.insert("include".into(), json!(["reasoning.encrypted_content"]));
        } else if !skip_off {
            let off = mapped_level(model, "off");
            if off != Mapped::Null {
                p.insert("reasoning".into(), json!({"effort": off.or("none")}));
            }
        }
        if xai_include {
            p.insert("include".into(), json!(["reasoning.encrypted_content"]));
        }
    }
    if let Some(sampling) = super::transform::resolve_sampling_params(req, requested) {
        for (k, v) in sampling {
            p.insert(k, v);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Slot {
    Thinking(usize),
    Text(usize),
    Tool(usize),
}

enum PendingArgs {
    Json(String),
    Custom(String),
}

/// Event-by-event state for one Responses stream (reference `processResponsesStream`).
#[derive(Default)]
pub struct ResponsesStream {
    slots: HashMap<i64, Slot>,
    /// Tool calls whose `output_item.done` has not arrived, with their streamed arguments.
    pending: HashMap<usize, PendingArgs>,
    reasoning_by_id: HashMap<String, usize>,
    saw_terminal: bool,
    /// `None` until something sets it (reference "pending").
    pub stop: Option<StopReason>,
    pub error_message: Option<String>,
    /// The service tier the response reports.
    pub service_tier: Option<String>,
}

fn block_text(e: &Emitter, index: usize) -> String {
    match e.content().get(index) {
        Some(Block::Text { text, .. }) => text.clone(),
        Some(Block::Thinking { thinking, .. }) => thinking.clone(),
        _ => String::new(),
    }
}

/// Set a block to the provider's final text: stream the missing suffix when the final text
/// extends what was streamed, else replace it.
fn settle_text(e: &mut Emitter, index: usize, thinking: bool, text: &str) {
    let current = block_text(e, index);
    if let Some(suffix) = text.strip_prefix(current.as_str()) {
        if thinking {
            e.thinking(index, suffix);
        } else {
            e.text(index, suffix);
        }
    } else {
        e.set_block_text(index, text);
    }
}

fn js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

impl ResponsesStream {
    fn create_slot(&mut self, e: &mut Emitter, output_index: i64, item: &Value) -> Option<Slot> {
        let slot = match item.get("type").and_then(Value::as_str)? {
            "reasoning" => Slot::Thinking(e.start_thinking()),
            "message" => {
                if item.get("phase").and_then(Value::as_str) == Some("final_answer") {
                    self.stop = Some(StopReason::Stop);
                }
                Slot::Text(e.start_text())
            }
            kind @ ("function_call" | "custom_tool_call") => {
                let id = format!(
                    "{}|{}",
                    js_string(item.get("call_id")),
                    js_string(item.get("id"))
                );
                let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
                let index = e.start_tool_call(&id, name);
                if let Some(ns) = item.get("namespace").filter(|v| !v.is_null()) {
                    e.block_extra(index, "namespace", ns.clone());
                }
                let pending = if kind == "function_call" {
                    PendingArgs::Json(
                        item.get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    )
                } else {
                    PendingArgs::Custom(
                        item.get("input")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    )
                };
                self.pending.insert(index, pending);
                Slot::Tool(index)
            }
            _ => return None,
        };
        self.slots.insert(output_index, slot);
        Some(slot)
    }

    fn backfill_reasoning(&self, e: &mut Emitter, output: &[Value]) {
        for item in output {
            if item.get("type").and_then(Value::as_str) != Some("reasoning") {
                continue;
            }
            let Some(encrypted) = item
                .get("encrypted_content")
                .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            else {
                continue;
            };
            let Some(index) = item
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| self.reasoning_by_id.get(id))
                .copied()
            else {
                continue;
            };
            let Some(Block::Thinking {
                signature: Some(sig),
                ..
            }) = e.content().get(index)
            else {
                continue;
            };
            let Ok(Value::Object(mut stored)) = serde_json::from_str::<Value>(sig) else {
                continue;
            };
            if stored
                .get("encrypted_content")
                .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            {
                continue;
            }
            stored.insert("encrypted_content".into(), encrypted.clone());
            e.thinking_signature(index, &Value::Object(stored).to_string(), false);
        }
    }

    fn finalize_response(&mut self, e: &mut Emitter, response: &Value) -> Result<(), String> {
        self.saw_terminal = true;
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            self.backfill_reasoning(e, output);
        }
        if let Some(id) = response
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            e.set_response_id(id);
        }
        if let Some(usage) = response.get("usage").filter(|u| u.is_object()) {
            let n = |p: &str| usage.pointer(p).and_then(Value::as_u64).unwrap_or(0);
            let cached = n("/input_tokens_details/cached_tokens");
            let cache_write = n("/input_tokens_details/cache_write_tokens");
            let u = e.usage_mut();
            u.input = n("/input_tokens")
                .saturating_sub(cached)
                .saturating_sub(cache_write);
            u.output = n("/output_tokens");
            u.cache_read = cached;
            u.cache_write = cache_write;
            u.total_tokens = n("/total_tokens");
        }
        self.service_tier = response
            .get("service_tier")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let status = response.get("status").and_then(Value::as_str);
        let reason = response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str);
        let raw = match (status, reason) {
            (Some(s), Some(r)) => Some(format!("{s}.{r}")),
            (Some(s), None) => Some(s.to_owned()),
            (None, Some(r)) => Some(format!("undefined.{r}")),
            (None, None) => None,
        };
        if let Some(raw) = raw {
            e.message_extra("rawStopReason", json!(raw));
        }
        let (stop, message) = map_status(status, reason)?;
        self.stop = Some(stop);
        self.error_message = message;
        if stop == StopReason::Stop && e.has_tool_calls() {
            self.stop = Some(StopReason::ToolUse);
        }
        Ok(())
    }

    /// Apply one decoded event. An `Err` ends the stream with that error text.
    pub fn handle(&mut self, e: &mut Emitter, event: &Value) -> Result<(), String> {
        let ty = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let output_index = event
            .get("output_index")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        let delta = event
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let slot = self.slots.get(&output_index).copied();
        match ty {
            "response.created" => {
                if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                    e.set_response_id(id);
                }
            }
            "response.output_item.added" => {
                if let Some(item) = event.get("item") {
                    self.create_slot(e, output_index, item);
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(Slot::Thinking(i)) = slot {
                    e.thinking(i, delta);
                }
            }
            "response.reasoning_summary_part.done" => {
                if let Some(Slot::Thinking(i)) = slot {
                    e.thinking(i, "\n\n");
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(Slot::Text(i)) = slot {
                    e.text(i, delta);
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(Slot::Tool(i)) = slot
                    && let Some(PendingArgs::Json(buf)) = self.pending.get_mut(&i)
                {
                    buf.push_str(delta);
                }
            }
            "response.function_call_arguments.done" => {
                if let Some(Slot::Tool(i)) = slot
                    && let Some(PendingArgs::Json(buf)) = self.pending.get_mut(&i)
                {
                    *buf = event
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                }
            }
            "response.custom_tool_call_input.delta" => {
                if let Some(Slot::Tool(i)) = slot
                    && let Some(PendingArgs::Custom(buf)) = self.pending.get_mut(&i)
                {
                    buf.push_str(delta);
                }
            }
            "response.custom_tool_call_input.done" => {
                if let Some(Slot::Tool(i)) = slot
                    && let Some(PendingArgs::Custom(buf)) = self.pending.get_mut(&i)
                {
                    *buf = event
                        .get("input")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                }
            }
            "response.output_item.done" => {
                let Some(item) = event.get("item") else {
                    return Ok(());
                };
                let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
                if kind == "message"
                    && item.get("phase").and_then(Value::as_str) == Some("final_answer")
                {
                    self.stop = Some(StopReason::Stop);
                }
                let slot = match slot {
                    Some(s) => Some(s),
                    None => self.create_slot(e, output_index, item),
                };
                match (kind, slot) {
                    ("reasoning", Some(Slot::Thinking(i))) => {
                        let join = |key: &str, field: &str| -> String {
                            item.get(key)
                                .and_then(Value::as_array)
                                .map(|parts| {
                                    parts
                                        .iter()
                                        .map(|p| {
                                            p.get(field).and_then(Value::as_str).unwrap_or_default()
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n\n")
                                })
                                .unwrap_or_default()
                        };
                        let summary = join("summary", "text");
                        let content = join("content", "text");
                        let text = if !summary.is_empty() {
                            summary
                        } else if !content.is_empty() {
                            content
                        } else {
                            block_text(e, i)
                        };
                        settle_text(e, i, true, &text);
                        e.thinking_signature(i, &item.to_string(), false);
                        if let Some(id) = item.get("id").and_then(Value::as_str) {
                            self.reasoning_by_id.insert(id.to_owned(), i);
                        }
                        self.slots.remove(&output_index);
                    }
                    ("message", Some(Slot::Text(i))) => {
                        let text: String = item
                            .get("content")
                            .and_then(Value::as_array)
                            .map(|parts| {
                                parts
                                    .iter()
                                    .map(|c| {
                                        let field = if c.get("type").and_then(Value::as_str)
                                            == Some("output_text")
                                        {
                                            "text"
                                        } else {
                                            "refusal"
                                        };
                                        c.get(field).and_then(Value::as_str).unwrap_or_default()
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        settle_text(e, i, false, &text);
                        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                        let phase = item.get("phase").and_then(Value::as_str);
                        e.block_extra(i, "textSignature", json!(encode_text_signature(id, phase)));
                        self.slots.remove(&output_index);
                    }
                    ("function_call", Some(Slot::Tool(i))) => {
                        if let Some(PendingArgs::Json(buf)) = self.pending.remove(&i) {
                            let raw = item
                                .get("arguments")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                                .unwrap_or(if buf.is_empty() { "{}".into() } else { buf });
                            finalize_tool_args(e, [(i, ToolArgs::Json(raw))]);
                            if let Some(ns) = item.get("namespace").filter(|v| !v.is_null()) {
                                e.block_extra(i, "namespace", ns.clone());
                            }
                            self.slots.remove(&output_index);
                        }
                    }
                    ("custom_tool_call", Some(Slot::Tool(i))) => {
                        if let Some(PendingArgs::Custom(buf)) = self.pending.remove(&i) {
                            let input = item
                                .get("input")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .unwrap_or(buf);
                            finalize_tool_args(e, [(i, ToolArgs::Custom(input))]);
                            if let Some(ns) = item.get("namespace").filter(|v| !v.is_null()) {
                                e.block_extra(i, "namespace", ns.clone());
                            }
                            self.slots.remove(&output_index);
                        }
                    }
                    _ => {}
                }
            }
            "response.completed" | "response.incomplete" => {
                let response = event.get("response").cloned().unwrap_or(Value::Null);
                self.finalize_response(e, &response)?;
            }
            "error" => {
                return Err(format!(
                    "Error Code {}: {}",
                    js_string(event.get("code")),
                    js_string(event.get("message"))
                ));
            }
            "response.failed" => {
                self.saw_terminal = true;
                if let Some(status) = event.pointer("/response/status").and_then(Value::as_str) {
                    e.message_extra("rawStopReason", json!(status));
                }
                let error = event.pointer("/response/error").filter(|v| truthy_obj(v));
                let reason = event
                    .pointer("/response/incomplete_details/reason")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty());
                let message = if let Some(error) = error {
                    let pick = |k: &str, d: &str| -> String {
                        match error.get(k) {
                            Some(Value::String(s)) if !s.is_empty() => s.clone(),
                            Some(v) if !v.is_null() && !v.is_string() => v.to_string(),
                            _ => d.to_owned(),
                        }
                    };
                    format!(
                        "{}: {}",
                        pick("code", "unknown"),
                        pick("message", "no message")
                    )
                } else if let Some(reason) = reason {
                    format!("incomplete: {reason}")
                } else {
                    "Unknown error (no error details in response)".into()
                };
                return Err(message);
            }
            _ => {}
        }
        Ok(())
    }

    /// Settle every unfinished tool call's arguments (failure and success paths alike).
    pub fn settle(&mut self, e: &mut Emitter) -> Vec<usize> {
        let unfinished: Vec<usize> = self.pending.keys().copied().collect();
        finalize_tool_args(
            e,
            self.pending.drain().map(|(i, p)| {
                (
                    i,
                    match p {
                        PendingArgs::Json(raw) => ToolArgs::Json(raw),
                        PendingArgs::Custom(input) => ToolArgs::Custom(input),
                    },
                )
            }),
        );
        unfinished
    }

    /// End-of-stream checks: a terminal event must have arrived, and a tool-use stop may not
    /// hand over calls whose item never finished.
    pub fn end(&mut self, e: &mut Emitter) -> Result<(), String> {
        let unfinished = self.settle(e);
        if !self.saw_terminal {
            return Err("OpenAI Responses stream ended before a terminal response event".into());
        }
        if self.stop == Some(StopReason::ToolUse)
            && let Some(&index) = unfinished.iter().min()
            && let Some(Block::ToolCall { id, name, .. }) = e.content().get(index)
        {
            return Err(format!(
                "OpenAI Responses stream completed with an unfinished tool call: {name} ({id})"
            ));
        }
        Ok(())
    }
}

fn truthy_obj(v: &Value) -> bool {
    !v.is_null() && v.as_bool() != Some(false)
}

fn map_status(
    status: Option<&str>,
    reason: Option<&str>,
) -> Result<(StopReason, Option<String>), String> {
    let Some(status) = status.filter(|s| !s.is_empty()) else {
        return Ok((StopReason::Stop, None));
    };
    Ok(match status {
        "completed" | "in_progress" | "queued" => (StopReason::Stop, None),
        "incomplete" => match reason {
            Some("max_output_tokens") => (StopReason::Length, None),
            Some(r) => (StopReason::Error, Some(format!("Response incomplete: {r}"))),
            None => (
                StopReason::Error,
                Some("Response incomplete without a provider reason".into()),
            ),
        },
        "failed" | "cancelled" => (StopReason::Error, None),
        other => return Err(format!("Unhandled stop reason: {other}")),
    })
}

/// One Responses exchange: send, stream, and settle the message. `http_error` formats an HTTP
/// failure; `label` names the API in the "no stop reason" error. Returns the message and the
/// service tier the response reported.
pub async fn exchange(
    ctx: &StreamCtx<'_>,
    url: &str,
    headers: HeaderMap,
    body: &Value,
    http_error: &(dyn Fn(&str) -> String + Sync),
    label: &str,
) -> (Message, Option<String>) {
    let req = ctx.req;
    let mut e = ctx.emitter();
    let resp = match post_json(ctx, url, headers, body).await {
        Ok(r) => r,
        Err(m) => return (ctx.failed(e, http_error(&m)), None),
    };
    let mut state = ResponsesStream::default();
    let mut failure: Option<String> = None;
    let read = http::read_sse(resp, idle_timeout(req), &ctx.cancel, |ev| {
        let data = ev.data.trim();
        if data.is_empty() || data.starts_with("[DONE]") {
            return ControlFlow::Continue(());
        }
        let event: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(err) => {
                failure = Some(format!("Could not parse stream event: {err}"));
                return ControlFlow::Break(());
            }
        };
        // The SDK raises an `error` member on any event as an API error.
        if let Some(err) = event.get("error").filter(|v| v.is_object())
            && event.get("type").and_then(Value::as_str) != Some("response.failed")
        {
            failure = Some(match err.get("message") {
                Some(Value::String(m)) => m.clone(),
                _ => err.to_string(),
            });
            return ControlFlow::Break(());
        }
        match state.handle(&mut e, &event) {
            Ok(()) => ControlFlow::Continue(()),
            Err(m) => {
                failure = Some(m);
                ControlFlow::Break(())
            }
        }
    })
    .await;
    let tier = state.service_tier.clone();
    if let Err(err) = read {
        state.settle(&mut e);
        let message = match err {
            http::ReadError::Aborted => "Request was aborted".to_owned(),
            http::ReadError::Failed(m) => m,
        };
        return (ctx.failed(e, message), tier);
    }
    if let Some(m) = failure {
        state.settle(&mut e);
        return (ctx.failed(e, m), tier);
    }
    if let Err(m) = state.end(&mut e) {
        return (ctx.failed(e, m), tier);
    }
    if ctx.cancel.is_cancelled() {
        return (ctx.failed(e, "Request was aborted"), tier);
    }
    match state.stop {
        None => (
            ctx.failed(e, format!("{label} stream ended without a stop reason")),
            tier,
        ),
        Some(StopReason::Error | StopReason::Aborted) => {
            let m = state
                .error_message
                .take()
                .unwrap_or_else(|| "An unknown error occurred".into());
            (ctx.failed(e, m), tier)
        }
        Some(stop) => (e.finish(req, stop, None), tier),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_signatures_round_trip_and_accept_legacy_ids() {
        let sig = encode_text_signature("msg_1", Some("final_answer"));
        assert_eq!(sig, r#"{"v":1,"id":"msg_1","phase":"final_answer"}"#);
        assert_eq!(
            parse_text_signature(Some(&sig)),
            Some(("msg_1".into(), Some("final_answer".into())))
        );
        assert_eq!(
            parse_text_signature(Some(r#"{"v":1,"id":"m","phase":"weird"}"#)),
            Some(("m".into(), None))
        );
        assert_eq!(
            parse_text_signature(Some("msg_legacy")),
            Some(("msg_legacy".into(), None))
        );
        assert_eq!(parse_text_signature(Some("")), None);
    }

    #[test]
    fn id_parts_are_sanitized_like_the_reference() {
        assert_eq!(normalize_id_part("call+a/b=="), "call_a_b");
        assert_eq!(normalize_id_part(&"x".repeat(80)).len(), 64);
        assert_eq!(foreign_item_id("abc"), "fc_y0biex7f9bbh");
    }

    #[test]
    fn status_mapping() {
        assert_eq!(
            map_status(Some("completed"), None).unwrap().0,
            StopReason::Stop
        );
        assert_eq!(
            map_status(Some("incomplete"), Some("max_output_tokens"))
                .unwrap()
                .0,
            StopReason::Length
        );
        assert_eq!(
            map_status(Some("incomplete"), Some("content_filter")).unwrap(),
            (
                StopReason::Error,
                Some("Response incomplete: content_filter".into())
            )
        );
        assert_eq!(
            map_status(Some("cancelled"), None).unwrap(),
            (StopReason::Error, None)
        );
        assert!(map_status(Some("bogus"), None).is_err());
    }
}
