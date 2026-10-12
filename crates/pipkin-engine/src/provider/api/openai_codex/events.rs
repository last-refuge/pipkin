//! Codex event mapping in front of the shared Responses stream processor.
//!
//! Ports reference `mapCodexEvents` (`openai-codex-responses.ts:743-793`): `error` and
//! `response.failed` become `CodexApiError`s, `response.done`/`completed`/`incomplete` become a
//! `response.completed` with the status normalized to the known set (and end the stream), and
//! every other event goes to [`ResponsesStream`] (`openai-responses-shared.ts:433-809`).

use pipkin_engine_protocol::{Block, Message, StopReason};
use serde_json::{Map, Value};

use super::super::openai_responses_shared::ResponsesStream;
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
    stream: ResponsesStream,
    /// Events handed to the Responses processor (reference "message stream started").
    events_processed: u64,
    pub(crate) end_turn: Option<bool>,
    /// The tier that prices this response (`None` = no terminal event seen).
    pub(crate) priced_tier: Option<Option<String>>,
    requested_tier: Option<String>,
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
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

impl ResponsesState {
    pub(crate) fn new(requested_tier: Option<String>) -> ResponsesState {
        ResponsesState {
            requested_tier,
            ..ResponsesState::default()
        }
    }

    pub(crate) fn for_request(
        req: &crate::provider::ProviderRequest,
        requested_tier: Option<String>,
    ) -> Result<Self, String> {
        Ok(Self {
            stream: ResponsesStream::for_request(req)?,
            requested_tier,
            ..Self::default()
        })
    }

    pub(crate) fn started(&self) -> bool {
        self.events_processed > 0
    }

    pub(crate) fn response_id(&self) -> Option<&str> {
        self.stream.response_id.as_deref()
    }

    pub(crate) fn stop(&self) -> Option<StopReason> {
        self.stream.stop
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
                let mut response = event.get("response").cloned().unwrap_or(Value::Null);
                if let Some(end_turn) = response.get("end_turn").and_then(Value::as_bool) {
                    self.end_turn = Some(end_turn);
                }
                if let Value::Object(map) = &mut response {
                    let known = map
                        .get("status")
                        .and_then(Value::as_str)
                        .is_some_and(|st| CODEX_RESPONSE_STATUSES.contains(&st));
                    if !known {
                        map.remove("status");
                    }
                }
                let mut completed = Map::new();
                completed.insert("type".into(), Value::String("response.completed".into()));
                completed.insert("response".into(), response.clone());
                self.events_processed += 1;
                self.stream
                    .handle(emitter, &Value::Object(completed))
                    .map_err(Failure::error)?;
                self.priced_tier = Some(resolve_codex_service_tier(
                    s(&response, "service_tier"),
                    self.requested_tier.as_deref(),
                ));
                return Ok(Step::Done);
            }
            _ => {}
        }
        self.events_processed += 1;
        self.stream
            .handle(emitter, &event)
            .map_err(Failure::error)?;
        Ok(Step::Continue)
    }

    /// Settle unfinished tool calls (failure paths).
    pub(crate) fn settle(&mut self, emitter: &mut Emitter) {
        self.stream.settle(emitter);
    }

    /// Post-stream checks (reference `processResponsesStream` tail + `assertSuccessfulOutput`).
    /// Returns the successful stop reason or the error text.
    pub(crate) fn outcome(&mut self, emitter: &mut Emitter) -> Result<StopReason, String> {
        self.stream.end(emitter)?;
        self.stream.outcome("Codex")
    }

    /// The assistant message as it will be persisted, for WebSocket continuation bookkeeping
    /// (empty text blocks dropped like `Emitter::finish`; finished tool calls already carry
    /// their parsed arguments on the emitter).
    pub(crate) fn snapshot(
        &self,
        emitter: &Emitter,
        model: &crate::provider::Model,
        stop: StopReason,
    ) -> Message {
        let mut content: Vec<Block> = emitter.content().to_vec();
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
