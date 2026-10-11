//! Thinking levels, budgets, sampling parameters and output-token clamping (reference
//! `api/simple-options.ts`, `utils/estimate.ts`).

use pipkin_engine_protocol::{Block, Message, StopReason, Usage};
use serde_json::{Map, Value, json};

use super::opt;
use crate::provider::ProviderRequest;
use crate::provider::model::{Model, ThinkingLevel};

// ---------------------------------------------------------------------------------------------
// Levels
// ---------------------------------------------------------------------------------------------

/// A model's `thinkingLevelMap` entry: missing, explicitly `null`, or a native value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapped<'a> {
    Missing,
    Null,
    Value(&'a str),
}

pub fn mapped_level<'a>(model: &'a Model, level: &str) -> Mapped<'a> {
    match model.thinking_level_map.get(level) {
        None => Mapped::Missing,
        Some(None) => Mapped::Null,
        Some(Some(v)) => Mapped::Value(v),
    }
}

impl<'a> Mapped<'a> {
    /// `map[level] ?? fallback`.
    pub fn or<'b>(self, fallback: &'b str) -> &'b str
    where
        'a: 'b,
    {
        match self {
            Mapped::Value(v) => v,
            _ => fallback,
        }
    }
}

/// The level a request asks for: the requested level clamped to what the model supports,
/// `None` when that is `off`. The agent sends no reasoning option at all for `off`
/// (reference `agent.ts:471`), so a requested `off` is never clamped up to a model's minimum
/// level here.
pub fn reasoning_effort(req: &ProviderRequest) -> Option<ThinkingLevel> {
    if req.thinking == ThinkingLevel::Off {
        return None;
    }
    match req.model.clamp_thinking(req.thinking) {
        ThinkingLevel::Off => None,
        level => Some(level),
    }
}

/// `xhigh`/`max` behave as `high` for token budgets (reference `clampReasoning`).
pub fn clamp_reasoning(level: ThinkingLevel) -> ThinkingLevel {
    match level {
        ThinkingLevel::Xhigh | ThinkingLevel::Max => ThinkingLevel::High,
        other => other,
    }
}

// ---------------------------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------------------------

/// Tokens always left for the answer when a thinking budget shares the response ceiling.
pub const MIN_ANSWER_TOKENS: u64 = 1024;

/// A custom budget for exactly this level from the `thinkingBudgets` option.
pub fn custom_budget(req: &ProviderRequest, level: ThinkingLevel) -> Option<u64> {
    opt(req, "thinkingBudgets")?.get(level.as_str())?.as_u64()
}

/// Reference `thinkingBudgetForLevel`: the default budgets (`xhigh`/`max` use `high`),
/// overridable through the `thinkingBudgets` request option.
pub fn thinking_budget_for_level(req: &ProviderRequest, level: ThinkingLevel) -> u64 {
    let level = clamp_reasoning(level);
    custom_budget(req, level).unwrap_or(match level {
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 2048,
        ThinkingLevel::Medium => 8192,
        _ => 16384,
    })
}

/// Reference `clampThinkingBudgetToAnswerRoom`.
pub fn clamp_thinking_budget_to_answer_room(budget: u64, ceiling: u64) -> u64 {
    budget.min(ceiling.saturating_sub(MIN_ANSWER_TOKENS))
}

/// Reference `adjustMaxTokensForThinking`: `(maxTokens, thinkingBudget)`. `None` means no
/// explicit caller cap (the model cap is used and thinking fits inside it).
pub fn adjust_max_tokens_for_thinking(
    req: &ProviderRequest,
    base_max_tokens: Option<u64>,
    level: ThinkingLevel,
) -> (u64, u64) {
    let mut budget = thinking_budget_for_level(req, level);
    let model_max = req.model.max_tokens;
    let max_tokens = match base_max_tokens {
        None => model_max,
        Some(base) => base.saturating_add(budget).min(model_max),
    };
    if max_tokens <= budget {
        budget = clamp_thinking_budget_to_answer_room(budget, max_tokens);
    }
    (max_tokens, budget)
}

/// Model sampling defaults < per-thinking-level defaults < request `samplingParams`
/// (reference `resolveSamplingParams`). `None` when none is set.
pub fn resolve_sampling_params(
    req: &ProviderRequest,
    effort: Option<ThinkingLevel>,
) -> Option<Map<String, Value>> {
    let model = &req.model;
    let level = model.clamp_thinking(effort.unwrap_or(ThinkingLevel::Off));
    let base = model.extra.get("samplingParams").and_then(Value::as_object);
    let by_level = model
        .extra
        .get("samplingParamsByThinkingLevel")
        .and_then(|m| m.get(level.as_str()))
        .and_then(Value::as_object);
    let request = req.options.get("samplingParams").and_then(Value::as_object);
    if base.is_none() && by_level.is_none() && request.is_none() {
        return None;
    }
    let mut out = Map::new();
    for source in [base, by_level, request].into_iter().flatten() {
        for (k, v) in source {
            out.insert(k.clone(), v.clone());
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// Context estimation and output-token clamping
// ---------------------------------------------------------------------------------------------

const CHARS_PER_TOKEN: u64 = 4;
const ESTIMATED_IMAGE_CHARS: u64 = 4800;
const CONTEXT_SAFETY_TOKENS: i64 = 4096;

fn js_len(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

fn ceil_tokens(chars: u64) -> u64 {
    chars.div_ceil(CHARS_PER_TOKEN)
}

fn content_chars(content: &[Block]) -> u64 {
    content
        .iter()
        .map(|b| match b {
            Block::Text { text, .. } => js_len(text),
            Block::Image { .. } => ESTIMATED_IMAGE_CHARS,
            Block::Thinking { thinking, .. } => js_len(thinking),
            Block::ToolCall {
                name, arguments, ..
            } => js_len(name) + js_len(&arguments.to_string()),
        })
        .sum()
}

fn message_tokens(message: &Message) -> u64 {
    ceil_tokens(content_chars(message.content()))
}

fn usage_tokens(u: &Usage) -> u64 {
    if u.total_tokens > 0 {
        u.total_tokens
    } else {
        u.input + u.output + u.cache_read + u.cache_write
    }
}

/// Reference `estimateContextTokens(context).tokens`: the last applicable assistant usage
/// plus estimates of what came after it, or a character estimate of everything. The engine's
/// flat system prompt and tool list stand in for the reference's leading system message (its
/// text and `toolsAdded`).
pub fn estimate_context_tokens(req: &ProviderRequest) -> u64 {
    let messages = &req.messages;
    let mut latest_prefix = 0i64; // the leading system message has timestamp 0
    let mut last: Option<(usize, u64)> = None;
    for (i, m) in messages.iter().enumerate() {
        if let Message::Assistant {
            usage,
            stop_reason,
            timestamp,
            ..
        } = m
        {
            let tokens = usage_tokens(usage);
            if *timestamp >= latest_prefix
                && !matches!(stop_reason, StopReason::Aborted | StopReason::Error)
                && tokens > 0
            {
                last = Some((i, tokens));
            }
        }
        latest_prefix = latest_prefix.max(m.timestamp());
    }
    if let Some((index, tokens)) = last {
        return tokens
            + messages[index + 1..]
                .iter()
                .map(message_tokens)
                .sum::<u64>();
    }
    let mut tokens = ceil_tokens(js_len(&req.system));
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "parameters": t.parameters}))
            .collect();
        tokens += ceil_tokens(js_len(&Value::Array(tools).to_string()));
    }
    tokens + messages.iter().map(message_tokens).sum::<u64>()
}

/// Reference `clampMaxTokensToContext` (`simple-options.ts:18-22`): cap an output-token
/// request to what fits in the context window (at least 1).
pub fn clamp_max_tokens_to_context(req: &ProviderRequest, max_tokens: u64) -> u64 {
    let window = req.model.context_window;
    if window == 0 {
        return max_tokens.max(1);
    }
    let available = window as i64 - estimate_context_tokens(req) as i64 - CONTEXT_SAFETY_TOKENS;
    max_tokens.min(available.max(1) as u64)
}

/// Reference `buildBaseOptions(...).maxTokens` (`simple-options.ts:46`): the caller's
/// `max_tokens` or the model's, clamped to the context.
pub fn base_max_tokens(req: &ProviderRequest) -> u64 {
    clamp_max_tokens_to_context(req, req.max_tokens.unwrap_or(req.model.max_tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_params_merge_in_order() {
        let mut req = crate::provider::api::openai_completions::tests::request(json!({
            "id": "m", "provider": "x", "api": "openai-completions", "reasoning": true,
            "samplingParams": {"temperature": 0.1, "top_p": 0.9},
            "samplingParamsByThinkingLevel": {"high": {"temperature": 0.5}}
        }));
        req.options
            .insert("samplingParams".into(), json!({"top_p": 0.5}));
        let merged = resolve_sampling_params(&req, Some(ThinkingLevel::High)).unwrap();
        assert_eq!(
            Value::Object(merged),
            json!({"temperature": 0.5, "top_p": 0.5})
        );
        req.options
            .insert("thinkingBudgets".into(), json!({"high": 99}));
        assert_eq!(thinking_budget_for_level(&req, ThinkingLevel::Max), 99);
        assert_eq!(clamp_thinking_budget_to_answer_room(16384, 2000), 976);
    }

    #[test]
    fn off_is_never_clamped_up_to_a_minimum_level() {
        let mut req = crate::provider::api::openai_completions::tests::request(json!({
            "id": "m", "provider": "x", "api": "openai-completions", "reasoning": true,
            "thinkingLevelMap": {"off": null}
        }));
        assert_eq!(reasoning_effort(&req), None);
        req.thinking = ThinkingLevel::Xhigh;
        assert_eq!(reasoning_effort(&req), Some(ThinkingLevel::High));
    }
}
