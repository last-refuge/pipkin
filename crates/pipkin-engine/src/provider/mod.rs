//! Provider contract: normalized requests in, incremental content blocks and a final assistant
//! message out.
//!
//! A wire adapter ([`ApiAdapter`]) implements one API kind (`anthropic-messages`,
//! `openai-completions`, ...). It receives a fully resolved [`ProviderRequest`] (model,
//! context, tools, thinking level, credential, headers) and reports progress through an
//! [`Emitter`], which also builds the final [`Message`]. Adapters never touch storage, never
//! choose credentials and never retry a whole turn; the engine does those.
//!
//! Provider-specific requirements stay behind `Model::compat`, `ProviderRequest::options`
//! and opaque fields on blocks (`extra`), so normalization never erases a capability.

pub mod api;
pub mod data;
pub mod errors;
pub mod http;
#[cfg(any(test, feature = "test-support"))]
pub mod mock;
pub mod model;
pub mod proxy;
pub mod scripted;
pub mod sse;
pub mod trust;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use pipkin_engine_protocol::{Block, Delta, Message, StopReason, Usage};
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use model::{Model, ThinkingLevel};

/// A tool offered to the model.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema object.
    pub parameters: Value,
    /// Reference `Tool.constrainedSampling`: JSON-schema strictness or grammar variants.
    pub constrained_sampling: Option<Value>,
}

impl From<&pipkin_engine_protocol::ToolDeclaration> for ToolSpec {
    fn from(t: &pipkin_engine_protocol::ToolDeclaration) -> Self {
        Self {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
            constrained_sampling: t.constrained_sampling.clone(),
        }
    }
}
impl From<&ToolSpec> for pipkin_engine_protocol::ToolDeclaration {
    fn from(t: &ToolSpec) -> Self {
        Self {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
            constrained_sampling: t.constrained_sampling.clone(),
        }
    }
}

/// How long a provider should keep a prompt cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheRetention {
    None,
    #[default]
    Short,
    Long,
}

/// A credential resolved for one request. Its kind is part of the account/billing identity:
/// an API key is never used where a subscription token was configured, and vice versa.
#[derive(Clone, PartialEq)]
pub enum Credential {
    /// No credential (local endpoints, or headers configured by the user carry auth).
    None,
    ApiKey(String),
    /// A bearer token sent as `Authorization: Bearer`.
    Bearer(String),
    /// An OAuth access token plus provider-specific non-secret metadata (account id, ...).
    OAuth {
        access: String,
        metadata: Map<String, Value>,
    },
    /// Ambient platform credentials the adapter resolves itself (AWS chain, Google ADC).
    Ambient(Map<String, Value>),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Credential::None => "Credential::None",
            Credential::ApiKey(_) => "Credential::ApiKey([redacted])",
            Credential::Bearer(_) => "Credential::Bearer([redacted])",
            Credential::OAuth { .. } => "Credential::OAuth([redacted])",
            Credential::Ambient(_) => "Credential::Ambient",
        })
    }
}

impl Credential {
    /// The secret to send as a key/token, if any.
    pub fn secret(&self) -> Option<&str> {
        match self {
            Credential::ApiKey(k) | Credential::Bearer(k) => Some(k),
            Credential::OAuth { access, .. } => Some(access),
            Credential::None | Credential::Ambient(_) => None,
        }
    }
}

/// `ProviderRequest::options` key set by the engine when its environment is isolated: then
/// [`ProviderRequest::env_var`] sees only `env`, never the process environment.
pub const ISOLATED_ENV: &str = "pipkin.isolatedEnv";

/// One model request, fully resolved.
#[derive(Clone, Debug)]
pub struct ProviderRequest {
    pub model: Model,
    /// Compatibility leading prompt. Durable runtime uses positional system messages.
    pub system: String,
    /// Context messages, already filtered and ordered (tool results follow their calls).
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub thinking: ThinkingLevel,
    /// Stable per-conversation id for prompt caching / affinity (never a secret).
    pub session_id: String,
    pub max_tokens: Option<u64>,
    pub cache_retention: CacheRetention,
    pub credential: Credential,
    /// Extra headers (configuration, attribution). A `None` value deletes a default header.
    pub headers: BTreeMap<String, Option<String>>,
    /// Base URL override from configuration or credential (e.g. a token's endpoint).
    pub base_url: Option<String>,
    /// Header/idle timeout for the HTTP exchange.
    pub timeout: Duration,
    /// Environment overrides for adapters that read provider env (region, project, ...).
    pub env: HashMap<String, String>,
    /// Provider-specific request options (thinking budgets, transport preferences, ...).
    pub options: Map<String, Value>,
}

impl ProviderRequest {
    /// Resolve positional systems only at the transport boundary. The durable transcript
    /// remains unchanged; incapable transports receive the replayed current prompt.
    pub fn resolve_systems(&self, mid_conversation: bool) -> std::borrow::Cow<'_, Self> {
        use crate::context::system;
        use pipkin_engine_protocol::{SystemContent, SystemMessage};
        if !self
            .messages
            .iter()
            .any(|m| matches!(m, Message::System(_)))
        {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut resolved = self.clone();
        let mut messages = std::mem::take(&mut resolved.messages);
        if !self.system.is_empty() {
            messages.insert(
                0,
                Message::System(SystemMessage {
                    content: SystemContent::Text(self.system.clone()),
                    ..Default::default()
                }),
            );
        }
        if !mid_conversation {
            messages = system::collapse_owned(messages);
        }
        resolved.system.clear();
        resolved.tools = system::tools(&messages)
            .iter()
            .map(ToolSpec::from)
            .collect();
        let initial = match messages.first() {
            Some(Message::System(head)) => head.tools_added.as_slice(),
            _ => &[],
        };
        let native_tools = mid_conversation
            && self.model.api == "anthropic-messages"
            && self.model.compat_bool("supportsMidConvoToolChanges") == Some(true)
            && !initial.is_empty();
        let additions = mid_conversation
            && ((self.model.api == "openai-completions"
                && self.model.compat_bool("supportsMidConvoToolAdditions") == Some(true))
                || (matches!(
                    self.model.api.as_str(),
                    "openai-responses" | "azure-openai-responses" | "openai-codex-responses"
                ) && (self.model.compat_bool("supportsAdditionalTools") == Some(true)
                    || self.model.compat_bool("supportsToolSearch") == Some(true))))
            && system::additive_tools(&messages);
        resolved
            .options
            .insert("pipkin.systemNativeTools".into(), native_tools.into());
        resolved
            .options
            .insert("pipkin.systemToolAdditions".into(), additions.into());
        if native_tools || additions {
            resolved.tools = initial.iter().map(ToolSpec::from).collect();
        }
        if let Some(Message::System(head)) = messages.first() {
            resolved
                .options
                .insert("pipkin.leadingSystem".into(), true.into());
            resolved.system = system::text(head);
            messages.remove(0);
        }
        resolved.messages = messages;
        std::borrow::Cow::Owned(resolved)
    }

    pub fn base_url(&self) -> &str {
        self.base_url.as_deref().unwrap_or(&self.model.base_url)
    }

    /// An environment value: request overrides, then the process environment.
    pub fn env_var(&self, name: &str) -> Option<String> {
        let ambient = !matches!(self.options.get(ISOLATED_ENV), Some(Value::Bool(true)));
        self.env
            .get(name)
            .cloned()
            .or_else(|| {
                if ambient {
                    std::env::var(name).ok()
                } else {
                    None
                }
            })
            .filter(|v| !v.is_empty())
    }
}

/// Incremental progress, published by the engine only after it is committed.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    Delta(Delta),
}

/// Engine-owned progress receiver. Delivery is synchronous: the runtime can apply bounded
/// batching and commit backpressure without an unbounded producer/consumer queue. Callbacks
/// must not await the adapter's own future or block on a consumer in the same task.
#[derive(Clone)]
pub struct StreamSink(Arc<dyn Fn(StreamEvent) + Send + Sync>);
impl StreamSink {
    pub fn new(receive: impl Fn(StreamEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(receive))
    }
    pub fn send(&self, event: StreamEvent) {
        (self.0)(event);
    }
}
// Adapter fixture collectors deliberately retain all deltas for assertions. The production
// runtime installs its bounded commit sink instead of this opt-in channel compatibility path.
impl From<mpsc::UnboundedSender<StreamEvent>> for StreamSink {
    fn from(tx: mpsc::UnboundedSender<StreamEvent>) -> Self {
        Self::new(move |event| {
            let _ = tx.send(event);
        })
    }
}

/// Builds the assistant message as an adapter streams, and forwards deltas.
///
/// Block indexes are assigned in arrival order. Tool-call arguments are accumulated as raw JSON
/// text and parsed when the block ends; an incomplete or invalid argument string never becomes
/// an executable call (it is recorded as `{}` with an error on the message).
pub struct Emitter {
    content: Vec<Block>,
    tool_json: HashMap<usize, String>,
    usage: Usage,
    tx: Option<StreamSink>,
    response_id: Option<String>,
    extra: Map<String, Value>,
}

impl Emitter {
    pub fn new(tx: Option<StreamSink>) -> Emitter {
        Emitter {
            content: Vec::new(),
            tool_json: HashMap::new(),
            usage: Usage::default(),
            tx,
            response_id: None,
            extra: Map::new(),
        }
    }

    fn send(&self, delta: Delta) {
        if let Some(tx) = &self.tx {
            tx.send(StreamEvent::Delta(delta));
        }
    }

    /// Start a new text block; returns its index.
    pub fn start_text(&mut self) -> usize {
        self.content.push(Block::text(""));
        self.content.len() - 1
    }

    pub fn text(&mut self, index: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(Block::Text { text: t, .. }) = self.content.get_mut(index) {
            t.push_str(text);
            self.send(Delta::Text {
                block: index as u32,
                text: text.to_owned(),
            });
        }
    }

    /// Append to the last block when it is text, else start one.
    pub fn append_text(&mut self, text: &str) -> usize {
        let index = match self.content.last() {
            Some(Block::Text { .. }) => self.content.len() - 1,
            _ => self.start_text(),
        };
        self.text(index, text);
        index
    }

    pub fn start_thinking(&mut self) -> usize {
        self.content.push(Block::Thinking {
            thinking: String::new(),
            signature: None,
            redacted: false,
            extra: Map::new(),
        });
        self.content.len() - 1
    }

    pub fn thinking(&mut self, index: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(Block::Thinking { thinking, .. }) = self.content.get_mut(index) {
            thinking.push_str(text);
            self.send(Delta::Thinking {
                block: index as u32,
                text: text.to_owned(),
            });
        }
    }

    /// Append to the last block when it is thinking, else start one.
    pub fn append_thinking(&mut self, text: &str) -> usize {
        let index = match self.content.last() {
            Some(Block::Thinking { .. }) => self.content.len() - 1,
            _ => self.start_thinking(),
        };
        self.thinking(index, text);
        index
    }

    /// Replace a text or thinking block's content with the provider's final version (APIs that
    /// resend the whole block when it ends). No delta is sent for the replacement.
    pub fn set_block_text(&mut self, index: usize, text: &str) {
        match self.content.get_mut(index) {
            Some(Block::Text { text: t, .. }) | Some(Block::Thinking { thinking: t, .. }) => {
                *t = text.to_owned();
            }
            _ => {}
        }
    }

    /// Set or extend a thinking block's opaque signature.
    pub fn thinking_signature(&mut self, index: usize, signature: &str, append: bool) {
        if let Some(Block::Thinking { signature: s, .. }) = self.content.get_mut(index) {
            match (s.as_mut(), append) {
                (Some(existing), true) => existing.push_str(signature),
                _ => *s = Some(signature.to_owned()),
            }
        }
    }

    /// A redacted/encrypted reasoning block (not readable; kept for replay).
    pub fn redacted_thinking(&mut self, payload: &str) -> usize {
        self.content.push(Block::Thinking {
            thinking: "[Reasoning redacted]".into(),
            signature: Some(payload.to_owned()),
            redacted: true,
            extra: Map::new(),
        });
        self.content.len() - 1
    }

    pub fn start_tool_call(&mut self, id: &str, name: &str) -> usize {
        self.content
            .push(Block::tool_call(id, name, Value::Object(Map::new())));
        let index = self.content.len() - 1;
        self.tool_json.insert(index, String::new());
        self.send(Delta::ToolCall {
            block: index as u32,
            id: id.to_owned(),
            name: name.to_owned(),
        });
        index
    }

    pub fn tool_args(&mut self, index: usize, json: &str) {
        if let Some(buf) = self.tool_json.get_mut(&index) {
            buf.push_str(json);
        }
    }

    /// Replace a tool call's id or name (some APIs send them late).
    pub fn set_tool_identity(&mut self, index: usize, id: Option<&str>, name: Option<&str>) {
        if let Some(Block::ToolCall { id: i, name: n, .. }) = self.content.get_mut(index) {
            if let Some(id) = id {
                *i = id.to_owned();
            }
            if let Some(name) = name {
                *n = name.to_owned();
            }
        }
    }

    /// Set complete arguments directly (APIs that send them whole).
    pub fn tool_args_value(&mut self, index: usize, value: Value) {
        self.tool_json.remove(&index);
        if let Some(Block::ToolCall { arguments, .. }) = self.content.get_mut(index) {
            *arguments = value;
        }
    }

    /// Opaque provider metadata on a block (signatures, item ids).
    pub fn block_extra(&mut self, index: usize, key: &str, value: Value) {
        let extra = match self.content.get_mut(index) {
            Some(Block::Text { extra, .. })
            | Some(Block::Thinking { extra, .. })
            | Some(Block::Image { extra, .. })
            | Some(Block::ToolCall { extra, .. }) => extra,
            None => return,
        };
        extra.insert(key.to_owned(), value);
    }

    /// Opaque metadata on the message (response id, ...).
    pub fn message_extra(&mut self, key: &str, value: Value) {
        self.extra.insert(key.to_owned(), value);
    }

    pub fn set_response_id(&mut self, id: &str) {
        self.response_id = Some(id.to_owned());
    }

    pub fn usage_mut(&mut self) -> &mut Usage {
        &mut self.usage
    }

    pub fn set_cache_write_1h(&mut self, tokens: u64) {
        self.usage.cache_write_1h = Some(tokens);
    }

    pub fn content(&self) -> &[Block] {
        &self.content
    }

    pub fn has_tool_calls(&self) -> bool {
        self.content
            .iter()
            .any(|b| matches!(b, Block::ToolCall { .. }))
    }

    /// Finish the message. Tool arguments are parsed here; an unparseable argument string on
    /// a successful stop turns the message into an error rather than an executable call.
    pub fn finish(
        mut self,
        req: &ProviderRequest,
        mut stop: StopReason,
        mut error: Option<String>,
    ) -> Message {
        let mut bad = Vec::new();
        for (index, raw) in std::mem::take(&mut self.tool_json) {
            let parsed = if raw.trim().is_empty() {
                Ok(Value::Object(Map::new()))
            } else {
                serde_json::from_str::<Value>(&raw)
            };
            if let Some(Block::ToolCall {
                arguments, name, ..
            }) = self.content.get_mut(index)
            {
                match parsed {
                    Ok(v) if v.is_object() => *arguments = v,
                    _ => {
                        bad.push(name.clone());
                        *arguments = Value::Object(Map::new());
                    }
                }
            }
        }
        if !bad.is_empty() && matches!(stop, StopReason::ToolUse | StopReason::Stop) {
            stop = StopReason::Error;
            error.get_or_insert_with(|| {
                format!(
                    "The model sent incomplete or invalid arguments for {}",
                    bad.join(", ")
                )
            });
        }
        if self.usage.total_tokens == 0 {
            self.usage.total_tokens = self.usage.input
                + self.usage.output
                + self.usage.cache_read
                + self.usage.cache_write;
        }
        req.model.cost_of(&mut self.usage);
        if let Some(id) = self.response_id {
            self.extra.insert("responseId".into(), Value::String(id));
        }
        // Drop empty text blocks a stream opened but never filled (an empty block that carries
        // provider metadata, such as a Gemini thought signature, is kept for replay).
        self.content.retain(
            |b| !matches!(b, Block::Text { text, extra } if text.is_empty() && extra.is_empty()),
        );
        Message::Assistant {
            content: self.content,
            api: req.model.api.clone(),
            provider: req.model.provider.clone(),
            model: req.model.id.clone(),
            usage: self.usage,
            stop_reason: stop,
            error_message: error,
            timestamp: crate::util::now_ms(),
            extra: self.extra,
        }
    }
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Everything an adapter gets for one exchange.
pub struct StreamCtx<'a> {
    pub req: &'a ProviderRequest,
    pub http: &'a reqwest::Client,
    /// Settings used to build this client; retained by identity-specific TLS transports.
    pub net: Option<&'a http::NetSettings>,
    pub events: Option<StreamSink>,
    pub cancel: CancellationToken,
}

impl StreamCtx<'_> {
    pub fn emitter(&self) -> Emitter {
        Emitter::new(self.events.clone())
    }

    /// The message for a failure before or during streaming. A cancelled exchange is
    /// `Aborted`, never an error.
    pub fn failed(&self, emitter: Emitter, message: impl Into<String>) -> Message {
        if self.cancel.is_cancelled() {
            emitter.finish(self.req, StopReason::Aborted, None)
        } else {
            emitter.finish(self.req, StopReason::Error, Some(message.into()))
        }
    }
}

/// One wire protocol.
pub trait ApiAdapter: Send + Sync {
    /// The API kind this adapter serves, e.g. `openai-completions`.
    fn api(&self) -> &'static str;
    /// Run one exchange. Always returns a message: errors and cancellation are expressed in
    /// its `stop_reason`/`error_message`, never as a panic.
    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message>;
    /// Release per-session transport resources (e.g. pooled WebSockets) for one provider
    /// session id, or all of them with `None`. Most adapters hold none.
    fn close_sessions(&self, _session_id: Option<&str>) {}
    /// End an idle runtime lifetime, also discarding session-local fallback/debug state.
    /// Called only after all exchanges and session operations finish. Must not block or
    /// call back into the engine: retirement serializes this against a new lifetime.
    fn retire_session(&self, session_id: &str) {
        self.close_sessions(Some(session_id));
    }
}

/// The adapters this build provides, by API kind.
#[derive(Clone, Default)]
pub struct Registry {
    adapters: HashMap<&'static str, Arc<dyn ApiAdapter>>,
}

impl Registry {
    pub fn empty() -> Registry {
        Registry::default()
    }

    /// Every built-in wire adapter.
    pub fn builtin() -> Registry {
        let mut r = Registry::default();
        for adapter in crate::provider::builtin_adapters() {
            r.install(adapter);
        }
        r
    }

    pub fn install(&mut self, adapter: Arc<dyn ApiAdapter>) {
        self.adapters.insert(adapter.api(), adapter);
    }

    pub fn get(&self, api: &str) -> Option<Arc<dyn ApiAdapter>> {
        self.adapters.get(api).cloned()
    }

    /// Every installed adapter.
    pub fn adapters(&self) -> Vec<Arc<dyn ApiAdapter>> {
        self.adapters.values().cloned().collect()
    }

    pub fn apis(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.adapters.keys().copied().collect();
        v.sort();
        v
    }
}

/// Built-in wire adapters. Each API kind module registers here.
pub fn builtin_adapters() -> Vec<Arc<dyn ApiAdapter>> {
    api::all()
}

#[cfg(test)]
mod env_tests {
    use super::*;

    #[test]
    fn an_isolated_request_never_reads_the_process_environment() {
        let mut req = ProviderRequest {
            model: scripted::model(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            thinking: ThinkingLevel::Off,
            session_id: String::new(),
            max_tokens: None,
            cache_retention: CacheRetention::Short,
            credential: Credential::None,
            headers: BTreeMap::new(),
            base_url: None,
            timeout: Duration::from_secs(1),
            env: HashMap::from([("ONLY_HERE".to_owned(), "1".to_owned())]),
            options: Map::new(),
        };
        // PATH is always set in the process environment.
        assert!(req.env_var("PATH").is_some());
        req.options.insert(ISOLATED_ENV.into(), Value::Bool(true));
        assert_eq!(req.env_var("PATH"), None);
        assert_eq!(req.env_var("ONLY_HERE").as_deref(), Some("1"));
    }
}
