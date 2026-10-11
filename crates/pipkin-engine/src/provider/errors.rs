//! Error classification for whole-turn retry and context-overflow recovery.
//!
//! The patterns are ported verbatim from the reference engine (`ai/src/utils/retry.ts`,
//! `ai/src/utils/overflow.ts` at the pinned revision) because they classify provider error
//! *text*; adapters therefore keep the reference error formats (`"<status>: <body>"`).

use std::sync::OnceLock;

use pipkin_engine_protocol::{Message, StopReason};
use regex::{Regex, RegexBuilder};

fn alternation(patterns: &[&str]) -> Regex {
    RegexBuilder::new(&patterns.join("|"))
        .case_insensitive(true)
        .build()
        .expect("static pattern")
}

fn non_retryable() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        alternation(&[
            "GoUsageLimitError",
            "FreeUsageLimitError",
            "Monthly usage limit reached",
            "available balance",
            "insufficient_quota",
            "out of budget",
            "quota exceeded",
            "billing",
            "subscription_sharing_usage_limit_exceeded",
        ])
    })
}

fn retryable() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        alternation(&[
            "overloaded",
            "currently experiencing high demand",
            "model is at capacity",
            "rate.?limit",
            "too many requests",
            "429",
            "500",
            "502",
            "503",
            "504",
            "520",
            "524",
            "service.?unavailable",
            "server.?error",
            "internal.?error",
            "provider.?returned.?error",
            "exceeded request buffer limit while retrying upstream",
            "network.?error",
            "connection.?error",
            "connection.?refused",
            "connection.?lost",
            "other side closed",
            "fetch failed",
            "getaddrinfo",
            "ENOTFOUND",
            "EAI_AGAIN",
            "upstream.?connect",
            "reset before headers",
            "socket hang up",
            "socket connection was closed",
            "timed? out",
            "timeout",
            "terminated",
            "websocket.?closed",
            "websocket.?error",
            "ended without",
            "stream ended before message_stop",
            "stream ended before a terminal response event",
            "http2 request did not get a response",
            "retry delay",
            "you can retry your request",
            "try your request again",
            "please retry your request",
            "ResourceExhausted",
            "subscription_sharing_usage_unavailable",
            "subscription_sharing_user_unavailable",
        ])
    })
}

fn overflow() -> &'static [Regex] {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    R.get_or_init(|| {
        [
            r"prompt (?:is )?too long",
            r"prompt exceeds max length",
            r"request_too_large",
            r"input is too long for requested model",
            r"exceeds the context window",
            r"exceeds (?:the )?(?:model'?s )?maximum context length(?: of [\d,]+ tokens?|\s*\([\d,]+\))",
            r"input token count.*exceeds the maximum",
            r"maximum prompt length is \d+",
            r"reduce the length of the messages",
            r"maximum context length is \d+ tokens",
            r"exceeds (?:the )?maximum allowed input length of [\d,]+ tokens?",
            r"input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)",
            r"exceeds the limit of \d+",
            r"exceeds the available context size",
            r"greater than the context length",
            r"context window exceeds limit",
            r"exceeded model token limit",
            r"too large for model with \d+ maximum context length",
            r"prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?",
            r"model_context_window_exceeded",
            r"prompt too long; exceeded (?:max )?context length",
            r"range of input length should be",
            r"context[_ ]length[_ ]exceeded",
            r"too many tokens",
            r"token limit exceeded",
        ]
        .iter()
        .map(|p| RegexBuilder::new(p).case_insensitive(true).build().expect("static"))
        .collect()
    })
}

fn non_overflow() -> &'static [Regex] {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    R.get_or_init(|| {
        [r"^(Throttling error|Service unavailable):", r"rate limit", r"too many requests"]
            .iter()
            .map(|p| RegexBuilder::new(p).case_insensitive(true).build().expect("static"))
            .collect()
    })
}

fn cerebras_bodyless() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        RegexBuilder::new(r"^4(?:00|13)\s*(?:status code)?\s*\(no body\)")
            .case_insensitive(true)
            .build()
            .expect("static")
    })
}

/// An errored assistant turn that is worth retrying (never an abort).
pub fn is_retryable(message: &Message) -> bool {
    let Message::Assistant {
        stop_reason: StopReason::Error,
        error_message: Some(text),
        ..
    } = message
    else {
        return false;
    };
    !non_retryable().is_match(text) && retryable().is_match(text)
}

/// The turn failed (or silently truncated) because the context is too large.
pub fn is_context_overflow(message: &Message, context_window: u64) -> bool {
    let Message::Assistant {
        stop_reason,
        error_message,
        usage,
        provider,
        ..
    } = message
    else {
        return false;
    };
    if *stop_reason == StopReason::Error
        && let Some(text) = error_message
        && !non_overflow().iter().any(|r| r.is_match(text))
        && (overflow().iter().any(|r| r.is_match(text))
            || (provider == "cerebras" && cerebras_bodyless().is_match(text)))
    {
        return true;
    }
    let input = usage.input + usage.cache_read;
    if context_window > 0 && *stop_reason == StopReason::Stop && input > context_window {
        return true;
    }
    context_window > 0
        && *stop_reason == StopReason::Length
        && usage.output == 0
        && input as f64 >= context_window as f64 * 0.99
}

/// Delay before whole-turn retry `attempt` (1-based): `base * 2^(attempt-1)`, capped.
pub fn retry_delay_ms(attempt: u32, base_ms: u64, cap_ms: u64) -> u64 {
    let factor = 1u64.checked_shl(attempt.saturating_sub(1)).unwrap_or(u64::MAX);
    base_ms.saturating_mul(factor).min(cap_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipkin_engine_protocol::Usage;

    fn err(text: &str) -> Message {
        Message::Assistant {
            content: vec![],
            api: String::new(),
            provider: "p".into(),
            model: String::new(),
            usage: Usage::default(),
            stop_reason: StopReason::Error,
            error_message: Some(text.into()),
            timestamp: 0,
            extra: Default::default(),
        }
    }

    #[test]
    fn classification_matches_the_reference_patterns() {
        assert!(is_retryable(&err("529 overloaded_error")));
        assert!(is_retryable(&err("Anthropic stream ended before message_stop")));
        assert!(!is_retryable(&err("429: insufficient_quota")));
        assert!(!is_retryable(&err("401: invalid api key")));
        assert!(is_context_overflow(&err("prompt is too long: 1 tokens > 0 maximum"), 0));
        assert!(!is_context_overflow(&err("ThrottlingException: Too many tokens, rate limit"), 0));
        assert_eq!(retry_delay_ms(1, 2000, 60000), 2000);
        assert_eq!(retry_delay_ms(3, 2000, 60000), 8000);
        assert_eq!(retry_delay_ms(40, 2000, 60000), 60000);
    }
}
