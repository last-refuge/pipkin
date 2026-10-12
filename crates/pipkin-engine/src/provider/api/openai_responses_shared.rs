//! Shared pieces of the OpenAI Responses adapters (`openai-responses`, `azure-openai-responses`,
//! `openai-codex-responses`; reference `api/openai-responses-shared.ts`): context -> `input`
//! items, tool definitions, the typed event stream -> content blocks, and service-tier pricing.
//!
//! Replay contract: a reasoning item is stored whole (JSON) as the thinking block's signature and
//! sent back verbatim; an assistant text block carries `textSignature` = `{"v":1,"id","phase"}`
//! so the message item keeps its id and phase; tool call ids are `"<call_id>|<item_id>"`.
//!
//! Engine differences from the reference context model, all by construction:
//! - The system prompt is a flat string on the request, so there are no mid-conversation system
//!   messages and no `toolsAdded` anchors (`additional_tools` / client `tool_search_*` items are
//!   never produced).
//! - `ToolSpec` has no `constrainedSampling`, so grammar (`custom`) tools and strict JSON-schema
//!   sampling never apply; every tool is a `function` tool.
//! - Rust strings are always valid UTF-8, so `sanitizeSurrogates` is a no-op.

use std::collections::HashMap;
use std::ops::ControlFlow;

use pipkin_engine_protocol::{Block, Message, StopReason, Usage};
use reqwest::header::HeaderMap;
use serde_json::{Map, Value, json};

use super::common::{
    Mapped, NO_TIMEOUT, ToolArgs, data_url, finalize_tool_args, idle_timeout, js_truthy,
    mapped_level, post_json, resolve_sampling_params, sanitize_id_chars, short_hash, take_chars,
    transform_messages,
};
use crate::provider::model::{Model, ThinkingLevel};
use crate::provider::{Emitter, ProviderRequest, StreamCtx, ToolSpec, http};

// ---------------------------------------------------------------------------------------------
// Text signatures
// ---------------------------------------------------------------------------------------------

/// Reference `encodeTextSignatureV1`: `{"v":1,"id":...,"phase":...}` in the reference's key
/// order; an empty phase is omitted.
pub fn encode_text_signature(id: &str, phase: Option<&str>) -> String {
    text_signature(Some(&Value::String(id.to_owned())), phase)
}

/// `JSON.stringify({v: 1, id, phase})` for a message item: an absent id is omitted (as
/// `JSON.stringify` drops `undefined`), any other id value is kept as is.
fn text_signature(id: Option<&Value>, phase: Option<&str>) -> String {
    let mut out = Map::new();
    out.insert("v".into(), json!(1));
    if let Some(id) = id {
        out.insert("id".into(), id.clone());
    }
    if let Some(phase) = phase.filter(|p| !p.is_empty()) {
        out.insert("phase".into(), json!(phase));
    }
    Value::Object(out).to_string()
}

/// Reference `parseTextSignature`: a V1 signature, or a legacy plain item id.
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

/// `part.replace(/[^a-zA-Z0-9_-]/g, "_")`, cut to 64 UTF-16 units, trailing `_` removed.
fn normalize_id_part(part: &str) -> String {
    let sanitized = take_chars(&sanitize_id_chars(part), 64);
    sanitized.trim_end_matches('_').to_owned()
}

fn foreign_item_id(item_id: &str) -> String {
    take_chars(&format!("fc_{}", short_hash(item_id)), 64)
}

/// Reference `convertToolResultOutput`.
fn tool_result_output(model: &Model, content: &[Block]) -> Value {
    let text = content
        .iter()
        .filter_map(Block::as_text)
        .collect::<Vec<_>>()
        .join("\n");
    let images: Vec<(&str, &str)> = content
        .iter()
        .filter_map(|b| match b {
            Block::Image {
                data, mime_type, ..
            } => Some((mime_type.as_str(), data.as_str())),
            _ => None,
        })
        .collect();
    if images.is_empty() || !model.supports_images() {
        let body = if !text.is_empty() {
            text
        } else if !images.is_empty() {
            "(see attached image)".into()
        } else {
            "(no tool output)".into()
        };
        return Value::String(body);
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

/// Context -> Responses `input` (reference `convertResponsesMessages`,
/// `openai-responses-shared.ts:145-354`). `system` is the leading instruction message
/// (`includeSystemPrompt`; Codex sends it as `instructions` instead and passes `None`). Tool
/// call ids are kept as `call|item` only for providers in `allowed_tool_call_providers`.
///
/// Fails when a same-model reasoning signature is not valid JSON (the reference `JSON.parse`
/// throws there).
pub fn convert_messages(
    model: &Model,
    system: Option<&str>,
    messages: &[Message],
    allowed_tool_call_providers: &[&str],
) -> Result<Vec<Value>, String> {
    let allowed = allowed_tool_call_providers.contains(&model.provider.as_str());
    let mut normalize = |id: &str, source: &Message| -> String {
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
    let messages = transform_messages(messages, model, Some(&mut normalize));
    let mut out: Vec<Value> = Vec::new();
    if let Some(system) = system.filter(|s| !s.is_empty()) {
        let developer =
            model.reasoning && model.compat_bool("supportsDeveloperRole") != Some(false);
        out.push(
            json!({"role": if developer { "developer" } else { "system" }, "content": system}),
        );
    }

    // `msgIndex` only advances for messages that produced output (the reference `continue`s
    // past the increment for empty user/assistant messages).
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
                                let item: Value = serde_json::from_str(sig).map_err(|e| {
                                    format!("Invalid reasoning signature JSON: {e}")
                                })?;
                                items.push(item);
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
                            let mut item = Map::new();
                            item.insert("type".into(), "function_call".into());
                            if let Some(i) = item_id {
                                item.insert("id".into(), json!(i));
                            }
                            item.insert("call_id".into(), json!(call_id));
                            item.insert("name".into(), json!(name));
                            item.insert("arguments".into(), json!(arguments.to_string()));
                            if same_model && let Some(ns) = extra.get("namespace") {
                                item.insert("namespace".into(), ns.clone());
                            }
                            items.push(Value::Object(item));
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
                    "output": tool_result_output(model, content),
                }));
            }
        }
        msg_index += 1;
    }
    Ok(out)
}

/// Tool definitions (reference `convertResponsesTools`, `openai-responses-shared.ts:360-397`).
/// `strict` is the `strict` option (`None` = JSON `null`, as Codex sends it; the reference
/// default is `false`) and is only sent when the endpoint supports strict mode.
pub fn convert_tools(
    tools: &[ToolSpec],
    supports_strict_mode: bool,
    strict: Option<bool>,
) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let mut tool = Map::new();
            tool.insert("type".into(), json!("function"));
            tool.insert("name".into(), json!(t.name));
            tool.insert("description".into(), json!(t.description));
            tool.insert("parameters".into(), t.parameters.clone());
            if supports_strict_mode {
                tool.insert("strict".into(), strict.map_or(Value::Null, Value::Bool));
            }
            Value::Object(tool)
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
    if let Some(sampling) = resolve_sampling_params(req, requested) {
        for (k, v) in sampling {
            p.insert(k, v);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Service tiers
// ---------------------------------------------------------------------------------------------

/// Reference `getServiceTierCostMultiplier`. `fast_is_priority`: `openai-responses` prices the
/// `fast` tier like `priority` (`openai-responses.ts:399`); Codex does not
/// (`openai-codex-responses.ts:606-613`).
pub fn service_tier_multiplier(model_id: &str, tier: Option<&str>, fast_is_priority: bool) -> f64 {
    match tier {
        Some("flex") => 0.5,
        Some("priority") => {
            if model_id == "gpt-5.5" {
                2.5
            } else {
                2.0
            }
        }
        Some("fast") if fast_is_priority => {
            service_tier_multiplier(model_id, Some("priority"), true)
        }
        _ => 1.0,
    }
}

/// Reference `applyServiceTierPricing`.
pub fn apply_service_tier_pricing(usage: &mut Usage, multiplier: f64) {
    if multiplier == 1.0 {
        return;
    }
    usage.cost.input *= multiplier;
    usage.cost.output *= multiplier;
    usage.cost.cache_read *= multiplier;
    usage.cost.cache_write *= multiplier;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
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

#[derive(Debug)]
enum PendingArgs {
    Json(String),
    Custom(String),
}

/// Event-by-event state for one Responses stream (reference `processResponsesStream`,
/// `openai-responses-shared.ts:433-777`).
#[derive(Debug, Default)]
pub struct ResponsesStream {
    slots: HashMap<i64, Slot>,
    /// Tool calls whose `output_item.done` has not arrived, with their streamed arguments
    /// (reference `partialJson` / `customInput` still on the block).
    pending: HashMap<usize, PendingArgs>,
    reasoning_by_id: HashMap<String, usize>,
    /// A terminal response event (`response.completed`/`incomplete`/`failed`) arrived.
    pub saw_terminal: bool,
    /// `None` until something sets it (reference "pending").
    pub stop: Option<StopReason>,
    pub error_message: Option<String>,
    /// The service tier the response reports.
    pub service_tier: Option<String>,
    /// The response id (`response.created`, then the terminal response).
    pub response_id: Option<String>,
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
        if suffix.is_empty() {
            return;
        }
        if thinking {
            e.thinking(index, suffix);
        } else {
            e.text(index, suffix);
        }
    } else {
        e.set_block_text(index, text);
    }
}

/// JS template-literal rendering of a field (`undefined` / `null` spelled out).
fn js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// A token count (`x || 0`; fractional counts are floored).
fn count(v: &Value, pointer: &str) -> u64 {
    v.pointer(pointer)
        .and_then(|n| n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)))
        .unwrap_or(0)
}

fn output_index(event: &Value) -> i64 {
    event
        .get("output_index")
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(-1)
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
                if let Some(ns) = item.get("namespace") {
                    e.block_extra(index, "namespace", ns.clone());
                }
                let field = if kind == "function_call" {
                    "arguments"
                } else {
                    "input"
                };
                let initial = item
                    .get(field)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let pending = if kind == "function_call" {
                    PendingArgs::Json(initial)
                } else {
                    PendingArgs::Custom(initial)
                };
                self.pending.insert(index, pending);
                Slot::Tool(index)
            }
            _ => return None,
        };
        self.slots.insert(output_index, slot);
        Some(slot)
    }

    /// Azure can omit `encrypted_content` from `output_item.done` and send it only in the
    /// terminal response; backfill the stored reasoning item from there.
    fn backfill_reasoning(&self, e: &mut Emitter, output: &[Value]) {
        for item in output {
            if item.get("type").and_then(Value::as_str) != Some("reasoning") {
                continue;
            }
            let Some(encrypted) = item.get("encrypted_content").filter(|v| js_truthy(v)) else {
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
            if stored.get("encrypted_content").is_some_and(js_truthy) {
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
            self.response_id = Some(id.to_owned());
            e.set_response_id(id);
        }
        if let Some(usage) = response.get("usage").filter(|u| js_truthy(u)) {
            let cached = count(usage, "/input_tokens_details/cached_tokens");
            let cache_write = count(usage, "/input_tokens_details/cache_write_tokens");
            let u = e.usage_mut();
            u.input = count(usage, "/input_tokens")
                .saturating_sub(cached)
                .saturating_sub(cache_write);
            u.output = count(usage, "/output_tokens");
            u.reasoning = Some(count(usage, "/output_tokens_details/reasoning_tokens"));
            u.cache_read = cached;
            u.cache_write = cache_write;
            u.total_tokens = count(usage, "/total_tokens");
        }
        self.service_tier = response
            .get("service_tier")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let status = response.get("status").and_then(Value::as_str);
        let reason = response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
            .filter(|r| !r.is_empty());
        // `incompleteReason ? `${status}.${incompleteReason}` : status`
        let raw = match (status, reason) {
            (s, Some(r)) => Some(format!("{}.{r}", s.unwrap_or("undefined"))),
            (Some(s), None) => Some(s.to_owned()),
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
        let output_index = output_index(event);
        let delta = event
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let slot = self.slots.get(&output_index).copied();
        match ty {
            "response.created" => {
                if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                    self.response_id = Some(id.to_owned());
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
                self.item_done(e, output_index, slot, item);
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
                let error = event.pointer("/response/error").filter(|v| js_truthy(v));
                let reason = event
                    .pointer("/response/incomplete_details/reason")
                    .filter(|v| js_truthy(v));
                let message = if let Some(error) = error {
                    let pick = |k: &str, d: &str| -> String {
                        match error.get(k) {
                            Some(v) if js_truthy(v) => js_string(Some(v)),
                            _ => d.to_owned(),
                        }
                    };
                    format!(
                        "{}: {}",
                        pick("code", "unknown"),
                        pick("message", "no message")
                    )
                } else if let Some(reason) = reason {
                    format!("incomplete: {}", js_string(Some(reason)))
                } else {
                    "Unknown error (no error details in response)".into()
                };
                return Err(message);
            }
            _ => {}
        }
        Ok(())
    }

    fn item_done(&mut self, e: &mut Emitter, output_index: i64, slot: Option<Slot>, item: &Value) {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if kind == "message" && item.get("phase").and_then(Value::as_str) == Some("final_answer") {
            self.stop = Some(StopReason::Stop);
        }
        let slot = match slot {
            Some(s) => Some(s),
            None => self.create_slot(e, output_index, item),
        };
        match (kind, slot) {
            ("reasoning", Some(Slot::Thinking(i))) => {
                let join = |key: &str| -> String {
                    item.get(key)
                        .and_then(Value::as_array)
                        .map(|parts| {
                            parts
                                .iter()
                                .map(|p| p.get("text").and_then(Value::as_str).unwrap_or_default())
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        })
                        .unwrap_or_default()
                };
                let summary = join("summary");
                let content = join("content");
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
                let signature =
                    text_signature(item.get("id"), item.get("phase").and_then(Value::as_str));
                e.block_extra(i, "textSignature", json!(signature));
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
                    if let Some(ns) = item.get("namespace") {
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
                    if let Some(ns) = item.get("namespace") {
                        e.block_extra(i, "namespace", ns.clone());
                    }
                    self.slots.remove(&output_index);
                }
            }
            _ => {}
        }
    }

    /// Settle every unfinished tool call's arguments (failure and success paths alike).
    /// Returns the unfinished block indexes.
    pub fn settle(&mut self, e: &mut Emitter) -> Vec<usize> {
        let mut unfinished: Vec<usize> = self.pending.keys().copied().collect();
        unfinished.sort_unstable();
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

    /// End-of-stream checks (reference `processResponsesStream` tail): a terminal event must
    /// have arrived, and a tool-use stop may not hand over calls whose item never finished.
    pub fn end(&mut self, e: &mut Emitter) -> Result<(), String> {
        let unfinished = self.settle(e);
        if !self.saw_terminal {
            return Err("OpenAI Responses stream ended before a terminal response event".into());
        }
        if self.stop == Some(StopReason::ToolUse)
            && let Some(&index) = unfinished.first()
            && let Some(Block::ToolCall { id, name, .. }) = e.content().get(index)
        {
            return Err(format!(
                "OpenAI Responses stream completed with an unfinished tool call: {name} ({id})"
            ));
        }
        Ok(())
    }

    /// The successful stop reason after [`Self::end`], or the error text. `label` names the API
    /// in the "no stop reason" error.
    pub fn outcome(&mut self, label: &str) -> Result<StopReason, String> {
        match self.stop {
            None => Err(format!("{label} stream ended without a stop reason")),
            Some(StopReason::Error | StopReason::Aborted) => Err(self
                .error_message
                .take()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "An unknown error occurred".into())),
            Some(stop) => Ok(stop),
        }
    }
}

/// Reference `mapStopReason` (`openai-responses-shared.ts:779-809`).
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

/// One SDK-style Responses exchange (`openai-responses`, `azure-openai-responses`): send,
/// stream, and settle the message. `http_error` formats an HTTP failure; `label` names the API
/// in the "no stop reason" error. Returns the message and the service tier the response
/// reported.
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
    let read = http::read_sse(resp, idle_timeout(req, NO_TIMEOUT), &ctx.cancel, |ev| {
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
    match state.outcome(label) {
        Ok(stop) => (e.finish(req, stop, None), tier),
        Err(m) => (ctx.failed(e, m), tier),
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
        // `JSON.stringify` drops an undefined id and an empty phase.
        assert_eq!(text_signature(None, Some("")), r#"{"v":1}"#);
        assert_eq!(encode_text_signature("m", Some("")), r#"{"v":1,"id":"m"}"#);
    }

    #[test]
    fn id_parts_are_sanitized_like_the_reference() {
        assert_eq!(normalize_id_part("call+a/b=="), "call_a_b");
        assert_eq!(normalize_id_part(&"x".repeat(80)).len(), 64);
        assert_eq!(foreign_item_id("abc"), "fc_y0biex7f9bbh");
        assert_eq!(normalize_id_part("call_1.2"), "call_1_2");
        assert_eq!(normalize_id_part("a\u{1f600}"), "a");
        assert_eq!(normalize_id_part("ab__"), "ab");
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

    #[test]
    fn service_tier_pricing_follows_each_reference_table() {
        assert_eq!(service_tier_multiplier("gpt-5.5", Some("fast"), true), 2.5);
        assert_eq!(service_tier_multiplier("gpt-5.5", Some("fast"), false), 1.0);
        assert_eq!(service_tier_multiplier("m", Some("priority"), false), 2.0);
        assert_eq!(service_tier_multiplier("m", Some("flex"), false), 0.5);
    }

    #[test]
    fn invalid_same_model_reasoning_signatures_fail_like_the_reference() {
        let model: Model = serde_json::from_value(json!({
            "id": "gpt", "provider": "openai", "api": "openai-responses"
        }))
        .unwrap();
        let msg = Message::Assistant {
            content: vec![Block::Thinking {
                thinking: "t".into(),
                signature: Some("not json".into()),
                redacted: false,
                extra: Map::new(),
            }],
            api: "openai-responses".into(),
            provider: "openai".into(),
            model: "gpt".into(),
            usage: Default::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 1,
            extra: Map::new(),
        };
        let err = convert_messages(&model, None, &[msg], &["openai"]).unwrap_err();
        assert!(err.starts_with("Invalid reasoning signature JSON"), "{err}");
    }
}
