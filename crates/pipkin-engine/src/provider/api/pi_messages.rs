//! `pi-messages`: the reference engine's own message protocol (the Radius gateway speaks it).
//!
//! Ported from the reference `api/pi-messages.ts`:
//! - `POST {baseUrl}/messages[?debug=1]` with `authorization: Bearer <key>`, `accept:
//!   text/event-stream`, `content-type: application/json`, then the request headers (model
//!   headers are not applied, as in the reference). No request-level retry.
//! - Body `{model, context: {messages}, options}`: the context is the transcript with a leading
//!   system message (`content` = system prompt, `toolsAdded` = tools, `timestamp: 0`), followed
//!   by the stored messages in their reference JSON shape; options carry `temperature`,
//!   `maxTokens`, `reasoning`, `cacheRetention`, `sessionId`, `toolChoice` (absent ones are
//!   omitted).
//! - Response: SSE `data:` frames, each a serialized assistant-message event (`text_*`,
//!   `thinking_*`, `toolcall_*`) addressed by `contentIndex`, ending in `done` or `error`.
//!   The terminal event's usage (including cost) is the server's and is kept as reported.
//! - Errors: non-2xx -> `<status> <statusText>: <error.message | body>[ (<code>)]`.

use std::collections::HashMap;
use std::sync::Arc;

use pipkin_engine_protocol::{Block, Message, StopReason, Usage};
use serde_json::{Map, Value, json};

use super::common::{Headers, idle_timeout};
use crate::provider::http::{self, ReadError};
use crate::provider::sse::SseEvent;
use crate::provider::{
    ApiAdapter, BoxFuture, CacheRetention, Emitter, ProviderRequest, StreamCtx, ThinkingLevel,
};

pub const API: &str = "pi-messages";

pub fn adapter() -> Arc<dyn ApiAdapter> {
    Arc::new(PiMessages)
}

pub struct PiMessages;

impl ApiAdapter for PiMessages {
    fn api(&self) -> &'static str {
        API
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(run(ctx))
    }
}

/// The transcript as the reference `normalizeContext` builds it.
fn context(req: &ProviderRequest) -> Value {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if !req.system.is_empty()
        || (!req.tools.is_empty() && !req.messages.iter().any(|m| matches!(m, Message::System(_))))
    {
        let mut system = json!({"role": "system", "content": req.system, "timestamp": 0});
        if !req.tools.is_empty() {
            system["toolsAdded"] = Value::Array(
                req.tools
                    .iter()
                    .map(|t| {
                        serde_json::to_value(pipkin_engine_protocol::ToolDeclaration::from(t))
                            .expect("tool declaration serializes")
                    })
                    .collect(),
            );
        }
        messages.push(system);
    }
    for m in &req.messages {
        messages.push(serde_json::to_value(m).unwrap_or(Value::Null));
    }
    json!({ "messages": messages })
}

pub(crate) fn build_payload(req: &ProviderRequest) -> Value {
    let mut options = Map::new();
    if let Some(t) = req.options.get("temperature").filter(|v| v.is_number()) {
        options.insert("temperature".into(), t.clone());
    }
    if let Some(m) = req.max_tokens {
        options.insert("maxTokens".into(), json!(m));
    }
    if req.thinking != ThinkingLevel::Off {
        options.insert("reasoning".into(), json!(req.thinking.as_str()));
    }
    let retention = match req.cache_retention {
        CacheRetention::None => "none",
        CacheRetention::Short => "short",
        CacheRetention::Long => "long",
    };
    options.insert("cacheRetention".into(), json!(retention));
    if !req.session_id.is_empty() {
        options.insert("sessionId".into(), json!(req.session_id));
    }
    if let Some(choice) = req.options.get("toolChoice").filter(|v| !v.is_null()) {
        options.insert("toolChoice".into(), choice.clone());
    }
    json!({"model": req.model.id, "context": context(req), "options": options})
}

fn endpoint(req: &ProviderRequest) -> String {
    let mut url = format!("{}/messages", req.base_url().trim_end_matches('/'));
    if req.options.get("debug").and_then(Value::as_bool) == Some(true) {
        url.push_str("?debug=1");
    }
    url
}

fn format_response_error(status: reqwest::StatusCode, body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let error = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .filter(|e| e.is_object());
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(body);
    let code = error
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str)
        .map(|c| format!(" ({c})"))
        .unwrap_or_default();
    format!(
        "{} {}: {message}{code}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    )
}

#[derive(Default)]
struct State {
    /// Server `contentIndex` -> emitter index.
    blocks: HashMap<u64, usize>,
    /// Post-finish fixups by emitter index: final text/thinking that differ from the deltas.
    replace_text: HashMap<usize, String>,
    redacted: Vec<usize>,
    terminal: Option<Terminal>,
}

struct Terminal {
    stop: StopReason,
    error: Option<String>,
    usage: Option<Usage>,
}

fn stop_reason(s: &str) -> StopReason {
    match s {
        "length" => StopReason::Length,
        "toolUse" => StopReason::ToolUse,
        "error" => StopReason::Error,
        "aborted" => StopReason::Aborted,
        _ => StopReason::Stop,
    }
}

/// Extend a block to its final content when the deltas are a prefix of it; otherwise record
/// a replacement applied after finishing.
fn settle(
    e: &mut Emitter,
    st: &mut State,
    index: usize,
    current: &str,
    final_text: &str,
    thinking: bool,
) {
    if final_text == current {
        return;
    }
    if let Some(rest) = final_text.strip_prefix(current) {
        if thinking {
            e.thinking(index, rest);
        } else {
            e.text(index, rest);
        }
    } else {
        st.replace_text.insert(index, final_text.to_owned());
    }
}

fn on_event(e: &mut Emitter, st: &mut State, event: &SseEvent) -> Result<(), String> {
    let Some(data) = event
        .data
        .lines()
        .next()
        .map(str::trim)
        .filter(|d| !d.is_empty() && *d != "[DONE]")
    else {
        return Ok(());
    };
    let ev: Value = serde_json::from_str(data).map_err(|err| err.to_string())?;
    let kind = ev.get("type").and_then(Value::as_str).unwrap_or("");
    let ci = ev.get("contentIndex").and_then(Value::as_u64).unwrap_or(0);
    let s = |k: &str| ev.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    match kind {
        "text_start" => {
            let i = e.start_text();
            st.blocks.insert(ci, i);
        }
        "thinking_start" => {
            let i = e.start_thinking();
            st.blocks.insert(ci, i);
        }
        "text_delta" => {
            if let Some(&i) = st.blocks.get(&ci) {
                e.text(i, &s("delta"));
            }
        }
        "thinking_delta" => {
            if let Some(&i) = st.blocks.get(&ci) {
                e.thinking(i, &s("delta"));
            }
        }
        "text_end" => {
            if let Some(&i) = st.blocks.get(&ci) {
                let current = match e.content().get(i) {
                    Some(Block::Text { text, .. }) => text.clone(),
                    _ => String::new(),
                };
                if let Some(content) = ev.get("content").and_then(Value::as_str) {
                    settle(e, st, i, &current, content, false);
                }
                if let Some(sig) = ev.get("contentSignature").and_then(Value::as_str) {
                    e.block_extra(i, "textSignature", json!(sig));
                }
            }
        }
        "thinking_end" => {
            if let Some(&i) = st.blocks.get(&ci) {
                let current = match e.content().get(i) {
                    Some(Block::Thinking { thinking, .. }) => thinking.clone(),
                    _ => String::new(),
                };
                if let Some(content) = ev.get("content").and_then(Value::as_str) {
                    settle(e, st, i, &current, content, true);
                }
                if let Some(sig) = ev.get("contentSignature").and_then(Value::as_str) {
                    e.thinking_signature(i, sig, false);
                }
                if ev.get("redacted").and_then(Value::as_bool) == Some(true) {
                    st.redacted.push(i);
                }
            }
        }
        "toolcall_start" => {
            let i = e.start_tool_call(&s("id"), &s("toolName"));
            st.blocks.insert(ci, i);
        }
        "toolcall_delta" => {
            if let Some(&i) = st.blocks.get(&ci) {
                e.tool_args(i, &s("delta"));
            }
        }
        "toolcall_end" => {
            if let Some(&i) = st.blocks.get(&ci)
                && let Some(call) = ev.get("toolCall").and_then(Value::as_object)
            {
                let id = call.get("id").and_then(Value::as_str);
                let name = call.get("name").and_then(Value::as_str);
                e.set_tool_identity(i, id, name);
                if let Some(args) = call.get("arguments").filter(|a| a.is_object()) {
                    e.tool_args_value(i, args.clone());
                }
                for (k, v) in call {
                    if !matches!(k.as_str(), "type" | "id" | "name" | "arguments") {
                        e.block_extra(i, k, v.clone());
                    }
                }
            }
        }
        "done" | "error" => {
            let usage = ev
                .get("usage")
                .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok());
            if let Some(id) = ev.get("responseId").and_then(Value::as_str) {
                e.set_response_id(id);
            }
            if let Some(level) = ev.get("providerThinkingLevel").and_then(Value::as_str) {
                e.message_extra("providerThinkingLevel", json!(level));
            }
            if let Some(rewrite) = ev.get("rewrite").filter(|r| r.is_object()) {
                e.message_extra(
                    "diagnostics",
                    json!([{"type": "pi_messages_rewrite", "timestamp": crate::util::now_ms(),
                        "details": rewrite}]),
                );
            }
            let reason = ev.get("reason").and_then(Value::as_str).unwrap_or(kind);
            let stop = if kind == "error" {
                match reason {
                    "aborted" => StopReason::Aborted,
                    _ => StopReason::Error,
                }
            } else {
                stop_reason(reason)
            };
            st.terminal = Some(Terminal {
                stop,
                error: ev
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                usage,
            });
        }
        _ => {}
    }
    Ok(())
}

/// Apply the server's final usage and block fixups to the finished message.
fn apply_fixups(mut msg: Message, st: &State, pre_finish: &[Block]) -> Message {
    if let Message::Assistant { content, usage, .. } = &mut msg {
        // `finish` drops empty text blocks; map pre-finish indexes to post-finish ones.
        let mut map = HashMap::new();
        let mut j = 0;
        for (i, b) in pre_finish.iter().enumerate() {
            if matches!(b, Block::Text { text, .. } if text.is_empty()) {
                continue;
            }
            map.insert(i, j);
            j += 1;
        }
        for (i, text) in &st.replace_text {
            match map.get(i).and_then(|j| content.get_mut(*j)) {
                Some(Block::Text { text: t, .. }) => *t = text.clone(),
                Some(Block::Thinking { thinking, .. }) => *thinking = text.clone(),
                _ => {}
            }
        }
        for i in &st.redacted {
            if let Some(Block::Thinking { redacted, .. }) =
                map.get(i).and_then(|j| content.get_mut(*j))
            {
                *redacted = true;
            }
        }
        if let Some(u) = st.terminal.as_ref().and_then(|t| t.usage.clone()) {
            *usage = u;
        }
    }
    msg
}

async fn run(ctx: StreamCtx<'_>) -> Message {
    let req = ctx.req;
    let mut e = ctx.emitter();
    let Some(key) = req.credential.secret().map(str::to_owned) else {
        return ctx.failed(
            e,
            format!(
                "No API key provided for provider \"{}\"",
                req.model.provider
            ),
        );
    };
    let mut headers = Headers::default();
    headers.set("authorization", format!("Bearer {key}"));
    headers.set("accept", "text/event-stream");
    headers.set("content-type", "application/json");
    headers.merge_optional(&req.headers);
    let headers = match headers.to_header_map() {
        Ok(h) => h,
        Err(m) => return ctx.failed(e, m),
    };
    let body = build_payload(req);
    let url = endpoint(req);
    let send = ctx.http.post(&url).headers(headers).json(&body).send();
    let resp = tokio::select! {
        _ = ctx.cancel.cancelled() => return ctx.failed(e, "Request was aborted"),
        r = send => match r {
            Ok(r) => r,
            Err(err) => return ctx.failed(e, http::transport_error(&err)),
        },
    };
    let status = resp.status();
    if !status.is_success() {
        let text = tokio::select! {
            _ = ctx.cancel.cancelled() => return ctx.failed(e, "Request was aborted"),
            t = resp.text() => t.unwrap_or_default(),
        };
        return ctx.failed(e, format_response_error(status, &text));
    }
    let mut st = State::default();
    let mut failure = None;
    let read = http::read_sse(
        resp,
        idle_timeout(req, http::DEFAULT_IDLE_TIMEOUT),
        &ctx.cancel,
        |event| {
            if let Err(err) = on_event(&mut e, &mut st, &event) {
                failure = Some(err);
                return std::ops::ControlFlow::Break(());
            }
            if st.terminal.is_some() {
                std::ops::ControlFlow::Break(())
            } else {
                std::ops::ControlFlow::Continue(())
            }
        },
    )
    .await;
    match read {
        Err(ReadError::Aborted) => return ctx.failed(e, "Request was aborted"),
        Err(ReadError::Failed(err)) => return ctx.failed(e, err),
        Ok(()) => {}
    }
    if let Some(err) = failure {
        return ctx.failed(e, err);
    }
    let pre_finish = e.content().to_vec();
    let Some(terminal) = st.terminal.take() else {
        return ctx.failed(
            e,
            format!(
                "{} stream ended without a terminal event",
                req.model.provider
            ),
        );
    };
    let msg = e.finish(req, terminal.stop, terminal.error.clone());
    st.terminal = Some(terminal);
    apply_fixups(msg, &st, &pre_finish)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock::{MockResponse, MockServer};
    use crate::provider::{Credential, Model, StreamEvent, ToolSpec};
    use std::collections::BTreeMap;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn model() -> Model {
        serde_json::from_value(json!({
            "id": "radius-large",
            "api": API,
            "provider": "radius",
            "baseUrl": "https://radius.pi.dev/v1/",
            "reasoning": true,
            "cost": {"input": 1.0, "output": 2.0}
        }))
        .unwrap()
    }

    fn request() -> ProviderRequest {
        ProviderRequest {
            model: model(),
            system: "Sys".into(),
            messages: vec![Message::user_text("hi", 5)],
            tools: vec![ToolSpec {
                constrained_sampling: None,
                name: "read".into(),
                description: "Read".into(),
                parameters: json!({"type": "object"}),
            }],
            thinking: ThinkingLevel::High,
            session_id: "sess".into(),
            max_tokens: Some(1000),
            cache_retention: CacheRetention::Long,
            credential: Credential::OAuth {
                access: "tok".into(),
                metadata: Map::new(),
            },
            headers: BTreeMap::from([("x-extra".into(), Some("1".into()))]),
            base_url: None,
            timeout: Duration::from_secs(5),
            env: HashMap::new(),
            options: Map::from_iter([
                ("toolChoice".to_owned(), json!("auto")),
                ("debug".to_owned(), json!(true)),
            ]),
        }
    }

    async fn exchange(
        req: &ProviderRequest,
        cancel: CancellationToken,
    ) -> (Message, Vec<StreamEvent>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let http = http::client();
        let msg = PiMessages
            .stream(StreamCtx {
                net: None,
                req,
                http: &http,
                events: Some(tx.into()),
                cancel,
            })
            .await;
        let mut events = vec![];
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        (msg, events)
    }

    fn sse(events: &[Value]) -> String {
        events
            .iter()
            .map(|e| format!("data: {e}\r\n\r\n"))
            .collect()
    }

    #[test]
    fn golden_payload() {
        assert_eq!(
            build_payload(&request()),
            json!({
                "model": "radius-large",
                "context": {"messages": [
                    {"role": "system", "content": "Sys", "timestamp": 0,
                        "toolsAdded": [{"name": "read", "description": "Read", "parameters": {"type": "object"}}]},
                    {"role": "user", "content": [{"type": "text", "text": "hi"}], "timestamp": 5}
                ]},
                "options": {"maxTokens": 1000, "reasoning": "high", "cacheRetention": "long",
                    "sessionId": "sess", "toolChoice": "auto"}
            })
        );
    }

    #[tokio::test]
    async fn golden_exchange() {
        let body = sse(&[
            json!({"type": "start"}),
            json!({"type": "thinking_start", "contentIndex": 0}),
            json!({"type": "thinking_delta", "contentIndex": 0, "delta": "hm"}),
            json!({"type": "thinking_end", "contentIndex": 0, "content": "hmm", "contentSignature": "sig"}),
            json!({"type": "text_start", "contentIndex": 1}),
            json!({"type": "text_delta", "contentIndex": 1, "delta": "Hé 😀"}),
            json!({"type": "text_end", "contentIndex": 1, "content": "Hé 😀", "contentSignature": "{\"v\":1}"}),
            json!({"type": "toolcall_start", "contentIndex": 2, "id": "c1", "toolName": "read"}),
            json!({"type": "toolcall_delta", "contentIndex": 2, "delta": "{\"pa"}),
            json!({"type": "toolcall_end", "contentIndex": 2, "toolCall": {"type": "toolCall", "id": "c1", "name": "read", "arguments": {"path": "x"}, "thoughtSignature": "ts"}}),
            json!({"type": "done", "reason": "toolUse", "responseId": "r1", "providerThinkingLevel": "high",
                "usage": {"input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 0, "totalTokens": 17,
                    "cost": {"input": 0.5, "output": 0.25, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.75}},
                "rewrite": {"policyId": "p", "policyVersion": 1, "changed": true, "tokenCountChange": -3,
                    "messageCountChange": 0, "systemPromptChanged": false}}),
            json!({"type": "text_start", "contentIndex": 3}),
        ]);
        let pieces: Vec<String> = body
            .as_bytes()
            .chunks(9)
            .map(|c| unsafe { String::from_utf8_unchecked(c.to_vec()) })
            .collect();
        let server = MockServer::start(move |_, _| MockResponse::sse(pieces.clone())).await;
        let mut req = request();
        req.base_url = Some(format!("{}/v1/", server.url));
        let (msg, events) = exchange(&req, CancellationToken::new()).await;
        let r = &server.requests()[0];
        assert_eq!(r.path, "/v1/messages?debug=1");
        assert_eq!(r.header("authorization"), Some("Bearer tok"));
        assert_eq!(r.header("accept"), Some("text/event-stream"));
        assert_eq!(r.header("x-extra"), Some("1"));
        assert_eq!(r.json(), build_payload(&req));

        let Message::Assistant {
            content,
            usage,
            stop_reason,
            extra,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::ToolUse);
        assert!(
            matches!(&content[0], Block::Thinking { thinking, signature: Some(s), .. } if thinking == "hmm" && s == "sig")
        );
        assert!(
            matches!(&content[1], Block::Text { text, extra } if text == "Hé 😀" && extra["textSignature"] == "{\"v\":1}")
        );
        assert!(
            matches!(&content[2], Block::ToolCall { id, arguments, extra, .. }
            if id == "c1" && *arguments == json!({"path": "x"}) && extra["thoughtSignature"] == "ts")
        );
        assert_eq!(
            content.len(),
            3,
            "events after the terminal one are ignored"
        );
        assert_eq!(usage.total_tokens, 17);
        assert_eq!(usage.cost.total, 0.75, "server-reported cost is kept");
        assert_eq!(extra["responseId"], "r1");
        assert_eq!(extra["providerThinkingLevel"], "high");
        assert_eq!(extra["diagnostics"][0]["type"], "pi_messages_rewrite");
        assert!(!events.is_empty());
    }

    #[tokio::test]
    async fn redacted_thinking_and_replaced_content() {
        let body = sse(&[
            json!({"type": "thinking_start", "contentIndex": 0}),
            json!({"type": "thinking_end", "contentIndex": 0, "content": "[Reasoning redacted]", "contentSignature": "opaque", "redacted": true}),
            json!({"type": "text_start", "contentIndex": 1}),
            json!({"type": "text_delta", "contentIndex": 1, "delta": "draft"}),
            json!({"type": "text_end", "contentIndex": 1, "content": "final"}),
            json!({"type": "done", "reason": "stop", "usage": {"input": 1, "output": 1}}),
        ]);
        let server = MockServer::start(move |_, _| MockResponse::sse(vec![body.clone()])).await;
        let mut req = request();
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        let Message::Assistant { content, .. } = &msg else {
            panic!()
        };
        assert!(
            matches!(&content[0], Block::Thinking { redacted: true, signature: Some(s), .. } if s == "opaque")
        );
        assert_eq!(content[1], Block::text("final"));
    }

    #[tokio::test]
    async fn errors_events_and_missing_terminal() {
        let server = MockServer::start(|_, _| {
            MockResponse::json(
                402,
                &json!({"error": {"message": "Out of credits", "code": "insufficient_quota"}}),
            )
        })
        .await;
        let mut req = request();
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e == "402 Payment Required: Out of credits (insufficient_quota)"),
            "{msg:?}"
        );
        assert!(!crate::provider::errors::is_retryable(&msg));
        assert_eq!(
            format_response_error(reqwest::StatusCode::BAD_GATEWAY, "<html>"),
            "502 Bad Gateway: <html>"
        );

        let body = sse(&[
            json!({"type": "text_start", "contentIndex": 0}),
            json!({"type": "text_delta", "contentIndex": 0, "delta": "par"}),
            json!({"type": "error", "reason": "error", "errorMessage": "upstream 503 overloaded", "usage": {"input": 3}}),
        ]);
        let server = MockServer::start(move |_, _| MockResponse::sse(vec![body.clone()])).await;
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        let Message::Assistant {
            stop_reason,
            error_message,
            content,
            usage,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::Error);
        assert_eq!(error_message.as_deref(), Some("upstream 503 overloaded"));
        assert_eq!(content, &vec![Block::text("par")]);
        assert_eq!(usage.input, 3);

        let server = MockServer::start(|_, _| {
            MockResponse::sse(vec![
                "data: {\"type\":\"text_start\",\"contentIndex\":0}\n\n".into(),
            ])
        })
        .await;
        req.base_url = Some(server.url.clone());
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e == "radius stream ended without a terminal event")
        );

        req.credential = Credential::None;
        let (msg, _) = exchange(&req, CancellationToken::new()).await;
        assert!(
            matches!(&msg, Message::Assistant { error_message: Some(e), .. } if e == "No API key provided for provider \"radius\"")
        );
    }

    #[tokio::test]
    async fn cancellation_mid_stream() {
        let server = MockServer::start(|_, _| {
            let mut r = MockResponse::sse(vec![sse(&[
                json!({"type": "text_start", "contentIndex": 0}),
                json!({"type": "text_delta", "contentIndex": 0, "delta": "partial"}),
            ])]);
            r.hang = true;
            r
        })
        .await;
        let mut req = request();
        req.base_url = Some(server.url.clone());
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            c2.cancel();
        });
        let (msg, _) = exchange(&req, cancel).await;
        let Message::Assistant {
            stop_reason,
            content,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::Aborted);
        assert_eq!(content, &vec![Block::text("partial")]);
    }
}
