//! A deterministic in-process provider for tests and the headless harness.
//!
//! A script decides each reply from the request it receives. Replies stream in small chunks so
//! committed-progress paths are exercised, and a reply can be held until a gate opens (a model
//! that is slow to answer) so Stop and steering can be tested at a known point. It never
//! touches the network. Registered under API kind `scripted`.

use std::sync::{Arc, Mutex};

use pipkin_engine_protocol::{Message, StopReason};
use serde_json::Value;
use tokio::sync::watch;

use super::{ApiAdapter, BoxFuture, ProviderRequest, StreamCtx};

/// What the scripted model does for one request.
#[derive(Clone, Debug)]
pub enum Reply {
    Text(String),
    /// Optional lead text, then tool calls `(name, arguments)`.
    Tools(Option<String>, Vec<(String, Value)>),
    Thinking(String, String),
    /// Fail with this error text (classified like a provider error).
    Error(String),
    /// Hold until the gate opens (or the request is cancelled), then reply.
    Gated(Gate, Box<Reply>),
    /// Never answer until cancelled.
    Hang,
}

/// A latch a test opens to let a held reply through.
#[derive(Clone, Debug)]
pub struct Gate(Arc<watch::Sender<bool>>);

impl Default for Gate {
    fn default() -> Self {
        Gate(Arc::new(watch::channel(false).0))
    }
}

impl Gate {
    pub fn new() -> Gate {
        Gate::default()
    }

    pub fn open(&self) {
        self.0.send_replace(true);
    }

    async fn wait(&self) {
        let mut rx = self.0.subscribe();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

type Script = dyn Fn(&ProviderRequest, usize) -> Reply + Send + Sync;

/// The scripted adapter. `requests()` returns every request it received, in order.
#[derive(Clone)]
pub struct Scripted {
    script: Arc<Script>,
    seen: Arc<Mutex<Vec<ProviderRequest>>>,
    chunk: usize,
    pace: std::time::Duration,
}

impl Scripted {
    pub fn new(script: impl Fn(&ProviderRequest, usize) -> Reply + Send + Sync + 'static) -> Self {
        Scripted {
            script: Arc::new(script),
            seen: Arc::default(),
            chunk: 3,
            pace: std::time::Duration::ZERO,
        }
    }

    /// Wait this long between streamed chunks (a slow model).
    pub fn paced(mut self, pace: std::time::Duration) -> Self {
        self.pace = pace;
        self
    }

    /// Always answer with the same text.
    pub fn echo(text: &str) -> Self {
        let text = text.to_owned();
        Scripted::new(move |_, _| Reply::Text(text.clone()))
    }

    pub fn requests(&self) -> Vec<ProviderRequest> {
        self.seen.lock().unwrap().clone()
    }

    async fn play(&self, ctx: &StreamCtx<'_>, reply: Reply) -> Message {
        let mut e = ctx.emitter();
        let mut reply = reply;
        loop {
            match reply {
                Reply::Gated(gate, inner) => {
                    tokio::select! {
                        _ = ctx.cancel.cancelled() => return ctx.failed(e, "aborted"),
                        _ = gate.wait() => {}
                    }
                    reply = *inner;
                }
                Reply::Hang => {
                    ctx.cancel.cancelled().await;
                    return ctx.failed(e, "aborted");
                }
                other => {
                    reply = other;
                    break;
                }
            }
        }
        let usage = |e: &mut super::Emitter| {
            let u = e.usage_mut();
            u.input = 10;
            u.output = 5;
        };
        match reply {
            Reply::Text(text) => {
                let index = e.start_text();
                for piece in chunks(&text, self.chunk) {
                    if ctx.cancel.is_cancelled() {
                        return ctx.failed(e, "aborted");
                    }
                    e.text(index, piece);
                    if self.pace.is_zero() {
                        tokio::task::yield_now().await;
                    } else {
                        tokio::select! {
                            _ = ctx.cancel.cancelled() => return ctx.failed(e, "aborted"),
                            _ = tokio::time::sleep(self.pace) => {}
                        }
                    }
                }
                usage(&mut e);
                e.finish(ctx.req, StopReason::Stop, None)
            }
            Reply::Thinking(thought, text) => {
                let t = e.start_thinking();
                e.thinking(t, &thought);
                e.thinking_signature(t, "scripted-signature", false);
                e.append_text(&text);
                usage(&mut e);
                e.finish(ctx.req, StopReason::Stop, None)
            }
            Reply::Tools(lead, calls) => {
                if let Some(lead) = lead {
                    e.append_text(&lead);
                }
                for (i, (name, args)) in calls.into_iter().enumerate() {
                    let id = format!("call_{}_{}", self.seen.lock().unwrap().len(), i);
                    let index = e.start_tool_call(&id, &name);
                    e.tool_args(index, &args.to_string());
                }
                usage(&mut e);
                e.finish(ctx.req, StopReason::ToolUse, None)
            }
            Reply::Error(text) => e.finish(ctx.req, StopReason::Error, Some(text)),
            Reply::Gated(..) | Reply::Hang => unreachable!(),
        }
    }
}

fn chunks(text: &str, n: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for (i, _) in text.char_indices() {
        if count == n {
            out.push(&text[start..i]);
            start = i;
            count = 0;
        }
        count += 1;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

impl ApiAdapter for Scripted {
    fn api(&self) -> &'static str {
        "scripted"
    }

    fn stream<'a>(&'a self, ctx: StreamCtx<'a>) -> BoxFuture<'a, Message> {
        Box::pin(async move {
            let n = {
                let mut seen = self.seen.lock().unwrap();
                seen.push(ctx.req.clone());
                seen.len() - 1
            };
            let reply = (self.script)(ctx.req, n);
            self.play(&ctx, reply).await
        })
    }
}

/// A model entry for the scripted provider.
pub fn model() -> super::Model {
    serde_json::from_value(serde_json::json!({
        "id": "scripted",
        "name": "Scripted model",
        "api": "scripted",
        "provider": "scripted",
        "baseUrl": "",
        "reasoning": true,
        "input": ["text", "image"],
        "contextWindow": 200000,
        "maxTokens": 8192
    }))
    .expect("static model")
}
