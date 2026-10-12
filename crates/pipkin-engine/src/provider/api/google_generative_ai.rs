//! `google-generative-ai`: the Gemini API (`generativelanguage.googleapis.com`).
//!
//! Reference: `packages/ai/src/api/google-generative-ai.ts` (+ `google-shared.ts`). The
//! reference goes through `@google/genai`; this sends the same REST request the SDK builds:
//! `POST {baseUrl}/models/{model}:streamGenerateContent?alt=sse` with `x-goog-api-key`.

use std::sync::Arc;

use pipkin_engine_protocol::Message;

use super::google_shared as shared;
use crate::provider::{ApiAdapter, BoxFuture, StreamCtx};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GoogleGenerativeAi;

pub fn adapter() -> Arc<dyn ApiAdapter> {
    Arc::new(GoogleGenerativeAi)
}

/// `models/{id}` unless the id already names a model resource.
fn model_path(id: &str) -> String {
    if id.starts_with("models/") || id.starts_with("tunedModels/") {
        id.to_owned()
    } else {
        format!("models/{id}")
    }
}

pub(crate) fn request_url(base_url: &str, model_id: &str) -> String {
    let base = base_url.trim();
    let base = if base.is_empty() {
        DEFAULT_BASE_URL
    } else {
        base.trim_end_matches('/')
    };
    format!(
        "{base}/{}:streamGenerateContent?alt=sse",
        model_path(model_id)
    )
}

async fn run(ctx: StreamCtx<'_>) -> Message {
    let req = ctx.req;
    let Some(key) = req.credential.secret().filter(|k| !k.is_empty()) else {
        return ctx.failed(
            ctx.emitter(),
            format!("No API key for provider: {}", req.model.provider),
        );
    };
    let body = match shared::build_body(req, true) {
        Ok(b) => b,
        Err(e) => return ctx.failed(ctx.emitter(), e),
    };
    let mut headers = shared::base_headers(req);
    headers.set_default("x-goog-api-key", key);
    let url = request_url(req.base_url(), &req.model.id);
    shared::stream(&ctx, &url, &headers, &body).await
}

impl ApiAdapter for GoogleGenerativeAi {
    fn api(&self) -> &'static str {
        "google-generative-ai"
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(run(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::api::google_shared::tests::{assistant, model, request, tool_result};
    use crate::provider::mock::{MockResponse, MockServer};
    use crate::provider::{Credential, ProviderRequest, StreamEvent, ThinkingLevel, ToolSpec};
    use pipkin_engine_protocol::{Block, Delta, StopReason};
    use serde_json::{Value, json};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn gemini(id: &str) -> crate::provider::Model {
        model(json!({
            "id": id, "name": id, "api": "google-generative-ai", "provider": "google",
            "baseUrl": "https://generativelanguage.googleapis.com/v1beta", "reasoning": true,
            "input": ["text", "image"], "contextWindow": 1048576, "maxTokens": 65536,
            "cost": {"input": 1.0, "output": 10.0, "cacheRead": 0.1, "cacheWrite": 0.0}
        }))
    }

    async fn run_with(
        req: &ProviderRequest,
        cancel: CancellationToken,
    ) -> (Message, Vec<StreamEvent>) {
        let http = crate::provider::http::client();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ctx = StreamCtx {
            net: None,
            req,
            http: &http,
            events: Some(tx.into()),
            cancel,
        };
        let msg = adapter().stream(ctx).await;
        let mut events = vec![];
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        (msg, events)
    }

    fn sse(chunks: &[Value]) -> String {
        chunks
            .iter()
            .map(|c| format!("data: {c}\r\n\r\n"))
            .collect()
    }

    /// An SSE response split into tiny byte pieces, including inside multi-byte characters.
    pub(crate) fn fragmented(body: &str, size: usize) -> MockResponse {
        let mut r = MockResponse::sse(vec![]);
        r.chunks = body.as_bytes().chunks(size).map(<[u8]>::to_vec).collect();
        r
    }

    #[test]
    fn urls_follow_the_sdk() {
        assert_eq!(
            request_url(
                "https://generativelanguage.googleapis.com/v1beta/",
                "gemini-2.5-pro"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        assert_eq!(
            request_url("", "tunedModels/x"),
            "https://generativelanguage.googleapis.com/v1beta/tunedModels/x:streamGenerateContent?alt=sse"
        );
    }

    #[tokio::test]
    async fn golden_request_and_fragmented_stream_with_thinking_text_and_tools() {
        let body = sse(&[
            json!({"responseId": "resp-1", "candidates": [{"content": {"role": "model", "parts": [
                {"text": "Let me th", "thought": true, "thoughtSignature": "c2lnMQ=="}]}}]}),
            json!({"candidates": [{"content": {"parts": [{"text": "ink \u{e9}\u{1f600}", "thought": true}]}}]}),
            json!({"candidates": [{"content": {"parts": [{"text": "Answer: h\u{e9}llo"}]}}]}),
            json!({"candidates": [{"content": {"parts": [
                {"functionCall": {"name": "read", "args": {"path": "a.txt"}, "id": "fc-1"}, "thoughtSignature": "c2lnMg=="},
                {"functionCall": {"name": "read", "args": {"path": "b.txt"}, "id": "fc-1"}}]},
                "finishReason": "STOP"}],
              "usageMetadata": {"promptTokenCount": 100, "cachedContentTokenCount": 40,
                "candidatesTokenCount": 20, "thoughtsTokenCount": 5, "totalTokenCount": 125}}),
        ]);
        let server = MockServer::start(move |_, _| fragmented(&body, 7)).await;
        let mut req = request(gemini("gemini-3-pro-preview"));
        req.base_url = Some(format!("{}/v1beta", server.url));
        req.thinking = ThinkingLevel::Medium;
        req.max_tokens = Some(4000);
        req.headers.insert("x-extra".into(), Some("1".into()));
        req.tools = vec![ToolSpec {
            constrained_sampling: None,
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        }];
        req.messages = vec![
            Message::User {
                content: vec![Block::text("look"), Block::image("image/png", "iVBO")],
                timestamp: 1,
                extra: Default::default(),
            },
            assistant(
                "google",
                "google-generative-ai",
                "gemini-3-pro-preview",
                vec![
                    Block::Thinking {
                        thinking: "plan".into(),
                        signature: Some("c2ln".into()),
                        redacted: false,
                        extra: Default::default(),
                    },
                    Block::tool_call("call|1", "read", json!({"path": "x"})),
                ],
            ),
            tool_result("call|1", "read", vec![Block::text("contents")], false),
        ];
        let (msg, events) = run_with(&req, CancellationToken::new()).await;

        let recorded = server.requests();
        assert_eq!(recorded.len(), 1);
        let r = &recorded[0];
        assert_eq!(r.method, "POST");
        assert_eq!(
            r.path,
            "/v1beta/models/gemini-3-pro-preview:streamGenerateContent?alt=sse"
        );
        assert_eq!(r.header("x-goog-api-key"), Some("test-key"));
        assert_eq!(r.header("x-extra"), Some("1"));
        assert_eq!(r.header("content-type"), Some("application/json"));
        assert_eq!(
            r.json(),
            json!({
                "contents": [
                    {"role": "user", "parts": [{"text": "look"},
                        {"inlineData": {"mimeType": "image/png", "data": "iVBO"}}]},
                    {"role": "model", "parts": [
                        {"thought": true, "text": "plan", "thoughtSignature": "c2ln"},
                        {"functionCall": {"name": "read", "args": {"path": "x"}, "id": "call|1"}}]},
                    {"role": "user", "parts": [{"functionResponse": {"name": "read",
                        "response": {"output": "contents"}, "id": "call|1"}}]}
                ],
                "generationConfig": {"maxOutputTokens": 4000,
                    "thinkingConfig": {"includeThoughts": true, "thinkingLevel": "MEDIUM"}},
                "systemInstruction": {"role": "user", "parts": [{"text": "You are helpful."}]},
                "tools": [{"functionDeclarations": [{"name": "read", "description": "Read a file",
                    "parametersJsonSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}]}]
            })
        );

        let Message::Assistant {
            content,
            stop_reason,
            usage,
            extra,
            error_message,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::ToolUse, "{error_message:?}");
        assert_eq!(content.len(), 4);
        assert_eq!(
            content[0],
            Block::Thinking {
                thinking: "Let me think \u{e9}\u{1f600}".into(),
                signature: Some("c2lnMQ==".into()),
                redacted: false,
                extra: Default::default()
            }
        );
        assert_eq!(content[1].as_text(), Some("Answer: h\u{e9}llo"));
        let Block::ToolCall {
            id,
            arguments,
            extra: call_extra,
            ..
        } = &content[2]
        else {
            panic!()
        };
        assert_eq!(id, "fc-1");
        assert_eq!(arguments, &json!({"path": "a.txt"}));
        assert_eq!(call_extra["thoughtSignature"], "c2lnMg==");
        let Block::ToolCall { id: dup, .. } = &content[3] else {
            panic!()
        };
        assert!(dup.starts_with("read_") && dup != "fc-1", "{dup}");
        assert_eq!(
            (
                usage.input,
                usage.output,
                usage.cache_read,
                usage.total_tokens
            ),
            (60, 25, 40, 125)
        );
        assert_eq!(usage.reasoning, Some(5));
        assert!((usage.cost.input - 60.0 / 1e6).abs() < 1e-12);
        assert_eq!(extra["responseId"], "resp-1");
        assert_eq!(extra["rawStopReason"], "STOP");
        assert!(events.iter().any(
            |e| matches!(e, StreamEvent::Delta(Delta::Text { text, .. }) if text == "Answer: h\u{e9}llo")
        ));
    }

    #[tokio::test]
    async fn thinking_off_budget_models_and_tool_choice() {
        let server = MockServer::start(|_, _| {
            MockResponse::sse(vec![
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"\",\"thoughtSignature\":\"QUJD\"}]},\"finishReason\":\"MAX_TOKENS\"}]}\n\n".into(),
            ])
        })
        .await;
        let mut req = request(gemini("gemini-2.5-flash"));
        req.base_url = Some(server.url.clone());
        req.options.insert("temperature".into(), json!(0.5));
        req.options.insert("toolChoice".into(), json!("any"));
        req.tools = vec![ToolSpec {
            constrained_sampling: None,
            name: "t".into(),
            description: "d".into(),
            parameters: json!({"type": "object", "$schema": "x", "properties": {}}),
        }];
        let (msg, _) = run_with(&req, CancellationToken::new()).await;
        let body = server.requests()[0].json();
        assert_eq!(
            body["generationConfig"],
            json!({"temperature": 0.5, "maxOutputTokens": 65536, "thinkingConfig": {"thinkingBudget": 0}})
        );
        assert_eq!(
            body["toolConfig"],
            json!({"functionCallingConfig": {"mode": "ANY"}})
        );
        // An empty text part that carries a signature survives for replay.
        let Message::Assistant {
            stop_reason,
            content,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::Length);
        assert_eq!(content.len(), 1);
        let Block::Text { text, extra } = &content[0] else {
            panic!()
        };
        assert!(text.is_empty());
        assert_eq!(extra["textSignature"], "QUJD");

        req.thinking = ThinkingLevel::High;
        let (_, _) = run_with(&req, CancellationToken::new()).await;
        assert_eq!(
            server.requests()[1].json()["generationConfig"]["thinkingConfig"],
            json!({"includeThoughts": true, "thinkingBudget": 24576})
        );
    }

    #[tokio::test]
    async fn safety_stop_http_errors_in_stream_errors_and_missing_key() {
        let server = MockServer::start(|_, n| match n {
            0 => MockResponse::sse(vec![
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]},\"finishReason\":\"SAFETY\"}]}\n\n".into(),
            ]),
            1 => MockResponse::json(
                429,
                &json!({"error": {"code": 429, "message": "Resource exhausted", "status": "RESOURCE_EXHAUSTED"}}),
            ),
            2 => MockResponse::sse(vec![
                "data: {\"error\":{\"code\":500,\"message\":\"boom\",\"status\":\"INTERNAL\"}}\n\n".into(),
            ]),
            _ => MockResponse::sse(vec![
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"no finish\"}]}}]}\n\n".into(),
            ]),
        })
        .await;
        let mut req = request(gemini("gemini-2.5-pro"));
        req.base_url = Some(server.url.clone());
        let err = |m: &Message| match m {
            Message::Assistant {
                stop_reason: StopReason::Error,
                error_message: Some(e),
                ..
            } => e.clone(),
            other => panic!("{other:?}"),
        };
        let (m, _) = run_with(&req, CancellationToken::new()).await;
        assert_eq!(err(&m), "Provider stopped with: SAFETY");
        assert_eq!(m.text(), "partial");
        let (m, _) = run_with(&req, CancellationToken::new()).await;
        let text = err(&m);
        assert!(text.starts_with("429: {"), "{text}");
        assert!(crate::provider::errors::is_retryable(&m));
        let (m, _) = run_with(&req, CancellationToken::new()).await;
        assert!(
            err(&m).starts_with("got status: INTERNAL. {"),
            "{}",
            err(&m)
        );
        let (m, _) = run_with(&req, CancellationToken::new()).await;
        assert_eq!(err(&m), "Google stream ended without a finish reason");
        assert_eq!(server.requests().len(), 4);

        req.credential = Credential::None;
        let (m, _) = run_with(&req, CancellationToken::new()).await;
        assert_eq!(err(&m), "No API key for provider: google");
        assert_eq!(server.requests().len(), 4);
    }

    #[tokio::test]
    async fn request_retry_honours_max_retries() {
        let server = MockServer::start(|_, n| {
            if n == 0 {
                MockResponse::json(503, &json!({"error": {"code": 503}}))
                    .with_header("retry-after-ms", "1")
            } else {
                MockResponse::sse(vec![
                    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\"}]}\n\n".into(),
                ])
            }
        })
        .await;
        let mut req = request(gemini("gemini-2.5-pro"));
        req.base_url = Some(server.url.clone());
        req.options.insert("maxRetries".into(), json!(2));
        let (m, _) = run_with(&req, CancellationToken::new()).await;
        assert_eq!(m.text(), "ok");
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn cancellation_mid_stream_is_aborted() {
        let server = MockServer::start(|_, _| {
            let mut r = MockResponse::sse(vec![
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n"
                    .into(),
            ]);
            r.hang = true;
            r
        })
        .await;
        let mut req = request(gemini("gemini-2.5-pro"));
        req.base_url = Some(server.url.clone());
        let cancel = CancellationToken::new();
        let http = crate::provider::http::client();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ctx = StreamCtx {
            net: None,
            req: &req,
            http: &http,
            events: Some(tx.into()),
            cancel: cancel.clone(),
        };
        let task = async {
            rx.recv().await;
            cancel.cancel();
        };
        let (msg, _) = {
            let adapter = adapter();
            tokio::join!(adapter.stream(ctx), task)
        };
        let Message::Assistant {
            stop_reason,
            error_message,
            ..
        } = &msg
        else {
            panic!()
        };
        assert_eq!(*stop_reason, StopReason::Aborted);
        assert_eq!(error_message, &None);
        assert_eq!(msg.text(), "partial");
    }
}
