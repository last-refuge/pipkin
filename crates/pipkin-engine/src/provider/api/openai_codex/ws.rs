//! WebSocket transport with per-session connection reuse and `previous_response_id`
//! continuation (reference `openai-codex-responses.ts:861-1590`).
//!
//! Connections are pooled per (cache session id, account id). While a pooled connection is idle
//! a small task owns it: it answers pings, notices a server close (so a dead socket is never
//! reused) and closes it after the 5 minute idle TTL. Taking a connection back from the task is
//! the Rust equivalent of the reference's `readyState === OPEN` check.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_util::sync::CancellationToken;

use super::convert::convert_messages;
use super::events::{ResponsesState, Step};
use super::{Failure, FailureKind};
use crate::provider::{Emitter, Model};

pub(crate) type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Reference `OPENAI_BETA_RESPONSES_WEBSOCKETS`.
pub(crate) const OPENAI_BETA_RESPONSES_WEBSOCKETS: &str = "responses_websockets=2026-02-06";
/// Reference `SESSION_WEBSOCKET_CACHE_TTL_MS`.
const SESSION_WEBSOCKET_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// Reference `SESSION_WEBSOCKET_MAX_AGE_MS`.
const SESSION_WEBSOCKET_MAX_AGE: Duration = Duration::from_secs(55 * 60);
/// Reference `WEBSOCKET_MESSAGE_TOO_BIG_CLOSE_CODE`.
const MESSAGE_TOO_BIG: u16 = 1009;

/// Reference `OpenAICodexWebSocketDebugStats`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebSocketDebugStats {
    pub requests: u64,
    pub connections_created: u64,
    pub connections_reused: u64,
    pub cached_context_requests: u64,
    pub store_true_requests: u64,
    pub full_context_requests: u64,
    pub delta_requests: u64,
    pub last_input_items: usize,
    pub last_delta_input_items: Option<usize>,
    pub last_previous_response_id: Option<String>,
    pub websocket_failures: u64,
    pub sse_fallbacks: u64,
    pub websocket_fallback_active: Option<bool>,
    pub last_websocket_error: Option<String>,
}

/// Reference `CachedWebSocketContinuationState`.
struct Continuation {
    last_request_body: Value,
    last_response_id: String,
    last_response_items: Vec<Value>,
}

enum IdleCommand {
    Take,
    Close(&'static str),
}

struct IdleHandle {
    command: oneshot::Sender<IdleCommand>,
    task: tokio::task::JoinHandle<Option<WsStream>>,
}

struct Entry {
    id: u64,
    created: Instant,
    busy: bool,
    idle: Option<IdleHandle>,
    continuation: Option<Continuation>,
}

#[derive(Clone, Debug)]
struct EntryKey {
    session: String,
    account: String,
    id: u64,
}

/// Process-lifetime transport state (reference module-level maps), owned by the adapter.
#[derive(Default)]
pub(crate) struct WsState {
    pool: HashMap<String, HashMap<String, Entry>>,
    next_id: u64,
    fallback_sessions: HashSet<String>,
    stats: HashMap<String, WebSocketDebugStats>,
}

pub(crate) type Shared = Arc<Mutex<WsState>>;

fn lock(shared: &Shared) -> MutexGuard<'_, WsState> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

impl WsState {
    fn stats_mut(&mut self, session: &str) -> &mut WebSocketDebugStats {
        self.stats.entry(session.to_owned()).or_default()
    }

    fn entry_mut(&mut self, key: &EntryKey) -> Option<&mut Entry> {
        self.pool
            .get_mut(&key.session)
            .and_then(|m| m.get_mut(&key.account))
            .filter(|e| e.id == key.id)
    }

    fn remove(&mut self, session: &str, account: &str) -> Option<Entry> {
        let entries = self.pool.get_mut(session)?;
        let entry = entries.remove(account);
        if entries.is_empty() {
            self.pool.remove(session);
        }
        entry
    }

    /// Remove the keyed entry when `pred` holds. True when it was removed or is already gone.
    fn remove_if(&mut self, key: &EntryKey, pred: impl FnOnce(&Entry) -> bool) -> bool {
        match self.entry_mut(key) {
            None => true,
            Some(entry) if pred(entry) => {
                self.remove(&key.session, &key.account);
                true
            }
            Some(_) => false,
        }
    }
}

pub(crate) fn debug_stats(shared: &Shared, session: &str) -> Option<WebSocketDebugStats> {
    lock(shared).stats.get(session).cloned()
}

pub(crate) fn reset_debug_stats(shared: &Shared, session: Option<&str>) {
    let mut st = lock(shared);
    match session {
        Some(s) => {
            st.stats.remove(s);
            st.fallback_sessions.remove(s);
        }
        None => {
            st.stats.clear();
            st.fallback_sessions.clear();
        }
    }
}

/// Reference `closeOpenAICodexWebSocketSessions`.
pub(crate) fn close_sessions(shared: &Shared, session: Option<&str>) {
    let mut st = lock(shared);
    let removed: Vec<Entry> = match session {
        Some(s) => st
            .pool
            .remove(s)
            .map(|m| m.into_values().collect())
            .unwrap_or_default(),
        None => st.pool.drain().flat_map(|(_, m)| m.into_values()).collect(),
    };
    drop(st);
    for entry in removed {
        if let Some(idle) = entry.idle {
            let _ = idle.command.send(IdleCommand::Close("debug_close"));
        }
    }
}

pub(crate) fn is_fallback_active(shared: &Shared, session: Option<&str>) -> bool {
    session.is_some_and(|s| lock(shared).fallback_sessions.contains(s))
}

/// Reference `recordWebSocketSseFallback`.
pub(crate) fn record_sse_fallback(shared: &Shared, session: Option<&str>) {
    let Some(session) = session else { return };
    let mut st = lock(shared);
    let active = st.fallback_sessions.contains(session);
    let stats = st.stats_mut(session);
    stats.sse_fallbacks += 1;
    stats.websocket_fallback_active = Some(active);
}

/// Reference `recordWebSocketFailure`: the session uses SSE from now on.
pub(crate) fn record_failure(shared: &Shared, session: Option<&str>, error: &str) {
    let Some(session) = session else { return };
    let mut st = lock(shared);
    st.fallback_sessions.insert(session.to_owned());
    let stats = st.stats_mut(session);
    stats.websocket_failures += 1;
    stats.last_websocket_error = Some(error.to_owned());
    stats.websocket_fallback_active = Some(true);
}

fn close_quietly(mut ws: WsStream, reason: &'static str) {
    tokio::spawn(async move {
        let frame = CloseFrame {
            code: CloseCode::Normal,
            reason: reason.into(),
        };
        let _ = tokio::time::timeout(Duration::from_secs(5), ws.close(Some(frame))).await;
    });
}

/// Own an idle pooled socket until it is taken back, closed by the server, or expires.
fn park(shared: &Shared, key: EntryKey, ws: WsStream) -> IdleHandle {
    let (tx, rx) = oneshot::channel();
    let task = tokio::spawn(idle_loop(shared.clone(), key, ws, rx));
    IdleHandle { command: tx, task }
}

async fn idle_loop(
    shared: Shared,
    key: EntryKey,
    mut ws: WsStream,
    mut command: oneshot::Receiver<IdleCommand>,
) -> Option<WsStream> {
    let expiry = tokio::time::sleep(SESSION_WEBSOCKET_CACHE_TTL);
    tokio::pin!(expiry);
    let mut expired = false;
    loop {
        tokio::select! {
            cmd = &mut command => {
                return match cmd {
                    Ok(IdleCommand::Take) => Some(ws),
                    Ok(IdleCommand::Close(reason)) => {
                        close_quietly(ws, reason);
                        None
                    }
                    Err(_) => {
                        close_quietly(ws, "debug_close");
                        None
                    }
                };
            }
            _ = &mut expiry, if !expired => {
                expired = true;
                // Reference timer: `if (entry.busy) return;` (a request is taking it).
                if lock(&shared).remove_if(&key, |e| !e.busy) {
                    close_quietly(ws, "idle_timeout");
                    return None;
                }
            }
            msg = ws.next() => match msg {
                Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => {
                    lock(&shared).remove_if(&key, |e| !e.busy);
                    return None;
                }
                // Pings are answered by the protocol layer; stray frames while idle are dropped
                // like the reference (no listener is attached between requests).
                Some(Ok(_)) => {}
            }
        }
    }
}

fn header_pairs(
    mut request: tokio_tungstenite::tungstenite::handshake::client::Request,
    headers: &[(String, String)],
) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    for (k, v) in headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            request.headers_mut().insert(name, value);
        }
    }
    request
}

/// Reference `connectWebSocket`.
async fn connect(
    url: &str,
    headers: &[(String, String)],
    cancel: &CancellationToken,
    connect_timeout: Option<Duration>,
) -> Result<WsStream, Failure> {
    let request = url
        .into_client_request()
        .map_err(|e| Failure::error(e.to_string()))?;
    let request = header_pairs(request, headers);
    let connect = tokio_tungstenite::connect_async(request);
    let result = tokio::select! {
        _ = cancel.cancelled() => return Err(Failure::aborted()),
        r = async {
            match connect_timeout {
                Some(t) => tokio::time::timeout(t, connect).await.map_err(|_| t),
                None => Ok(connect.await),
            }
        } => r,
    };
    match result {
        Err(t) => Err(Failure::error(format!(
            "WebSocket connect timeout after {}ms",
            t.as_millis()
        ))),
        Ok(Err(e)) => Err(Failure::error(nonempty(e.to_string(), "WebSocket error"))),
        Ok(Ok((ws, _response))) => Ok(ws),
    }
}

fn nonempty(text: String, fallback: &str) -> String {
    if text.is_empty() {
        fallback.to_owned()
    } else {
        text
    }
}

/// A connection in use by one request.
pub(crate) struct Lease {
    socket: WsStream,
    key: Option<EntryKey>,
    reused: bool,
}

enum Plan {
    Take(EntryKey, IdleHandle),
    Unregistered,
    Register,
}

/// Reference `acquireWebSocket`.
async fn acquire(
    shared: &Shared,
    url: &str,
    headers: &[(String, String)],
    session: Option<&str>,
    account: &str,
    cancel: &CancellationToken,
    connect_timeout: Option<Duration>,
) -> Result<Lease, Failure> {
    let Some(session) = session else {
        let socket = connect(url, headers, cancel, connect_timeout).await?;
        return Ok(Lease {
            socket,
            key: None,
            reused: false,
        });
    };
    let plan = {
        let mut st = lock(shared);
        let state = st.pool.get(session).and_then(|m| m.get(account)).map(|e| {
            (
                e.busy,
                e.created.elapsed() >= SESSION_WEBSOCKET_MAX_AGE,
                e.idle.is_some(),
            )
        });
        match state {
            None => Plan::Register,
            Some((false, true, _)) => {
                if let Some(entry) = st.remove(session, account)
                    && let Some(idle) = entry.idle
                {
                    let _ = idle
                        .command
                        .send(IdleCommand::Close("connection_age_limit"));
                }
                Plan::Register
            }
            Some((false, false, true)) => {
                match st.pool.get_mut(session).and_then(|m| m.get_mut(account)) {
                    Some(entry) => {
                        entry.busy = true;
                        let key = EntryKey {
                            session: session.to_owned(),
                            account: account.to_owned(),
                            id: entry.id,
                        };
                        match entry.idle.take() {
                            Some(idle) => Plan::Take(key, idle),
                            None => Plan::Register,
                        }
                    }
                    None => Plan::Register,
                }
            }
            Some((true, _, _)) => Plan::Unregistered,
            Some((false, false, false)) => {
                st.remove(session, account);
                Plan::Register
            }
        }
    };
    match plan {
        Plan::Take(key, idle) => {
            let _ = idle.command.send(IdleCommand::Take);
            if let Ok(Some(socket)) = idle.task.await {
                return Ok(Lease {
                    socket,
                    key: Some(key),
                    reused: true,
                });
            }
            // The server closed it while idle: drop the entry and connect fresh.
            lock(shared).remove_if(&key, |_| true);
        }
        Plan::Unregistered => {
            let socket = connect(url, headers, cancel, connect_timeout).await?;
            return Ok(Lease {
                socket,
                key: None,
                reused: false,
            });
        }
        Plan::Register => {}
    }
    let socket = connect(url, headers, cancel, connect_timeout).await?;
    let mut st = lock(shared);
    st.next_id += 1;
    let id = st.next_id;
    st.pool.entry(session.to_owned()).or_default().insert(
        account.to_owned(),
        Entry {
            id,
            created: Instant::now(),
            busy: true,
            idle: None,
            continuation: None,
        },
    );
    Ok(Lease {
        socket,
        key: Some(EntryKey {
            session: session.to_owned(),
            account: account.to_owned(),
            id,
        }),
        reused: false,
    })
}

/// Return a connection: park it for reuse (`keep`) or close it.
fn release(shared: &Shared, lease: Lease, keep: bool) {
    let Lease { socket, key, .. } = lease;
    let Some(key) = key else {
        close_quietly(socket, "done");
        return;
    };
    let mut st = lock(shared);
    if !keep {
        st.remove_if(&key, |_| true);
        drop(st);
        close_quietly(socket, "done");
        return;
    }
    if st.entry_mut(&key).is_none() {
        drop(st);
        close_quietly(socket, "done");
        return;
    }
    let handle = park(shared, key.clone(), socket);
    if let Some(entry) = st.entry_mut(&key) {
        entry.busy = false;
        entry.idle = Some(handle);
    }
}

fn without_input(body: &Value) -> Value {
    let mut body = body.clone();
    if let Value::Object(map) = &mut body {
        map.remove("input");
        map.remove("previous_response_id");
    }
    body
}

fn input_items(body: &Value) -> &[Value] {
    body.get("input")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Reference `getCachedWebSocketInputDelta`.
fn input_delta(body: &Value, continuation: &Continuation) -> Option<Vec<Value>> {
    if without_input(body) != without_input(&continuation.last_request_body) {
        return None;
    }
    let current = input_items(body);
    let baseline: Vec<&Value> = input_items(&continuation.last_request_body)
        .iter()
        .chain(continuation.last_response_items.iter())
        .collect();
    if current.len() < baseline.len() {
        return None;
    }
    if !current.iter().zip(baseline.iter()).all(|(a, b)| a == *b) {
        return None;
    }
    Some(current[baseline.len()..].to_vec())
}

/// Reference `buildCachedWebSocketRequestBody`.
fn cached_request_body(shared: &Shared, key: &EntryKey, body: &Value) -> Value {
    let mut st = lock(shared);
    let Some(entry) = st.entry_mut(key) else {
        return body.clone();
    };
    let Some(continuation) = &entry.continuation else {
        return body.clone();
    };
    let delta = input_delta(body, continuation);
    let (Some(delta), false) = (delta, continuation.last_response_id.is_empty()) else {
        entry.continuation = None;
        return body.clone();
    };
    let mut out = body.clone();
    if let Value::Object(map) = &mut out {
        map.insert(
            "previous_response_id".into(),
            Value::String(continuation.last_response_id.clone()),
        );
        map.insert("input".into(), Value::Array(delta));
    }
    out
}

/// Everything one WebSocket attempt needs.
pub(crate) struct WsRequest<'a> {
    pub(crate) url: &'a str,
    pub(crate) body: &'a Value,
    pub(crate) headers: &'a [(String, String)],
    pub(crate) model: &'a Model,
    pub(crate) idle_timeout: Option<Duration>,
    pub(crate) connect_timeout: Option<Duration>,
    pub(crate) session: Option<&'a str>,
    pub(crate) account: &'a str,
    pub(crate) transport: &'a str,
    pub(crate) cancel: &'a CancellationToken,
}

/// Reference `processWebSocketStream`.
pub(crate) async fn run(
    shared: &Shared,
    req: WsRequest<'_>,
    emitter: &mut Emitter,
    state: &mut ResponsesState,
) -> Result<(), Failure> {
    let mut lease = acquire(
        shared,
        req.url,
        req.headers,
        req.session,
        req.account,
        req.cancel,
        req.connect_timeout,
    )
    .await?;
    let use_cached = matches!(req.transport, "websocket-cached" | "auto");
    let request_body = match (&lease.key, use_cached) {
        (Some(key), true) => cached_request_body(shared, key, req.body),
        _ => req.body.clone(),
    };
    if let Some(session) = req.session {
        let mut st = lock(shared);
        let stats = st.stats_mut(session);
        stats.requests += 1;
        if lease.reused {
            stats.connections_reused += 1;
        } else {
            stats.connections_created += 1;
        }
        if use_cached {
            stats.cached_context_requests += 1;
        }
        if request_body.get("store") == Some(&Value::Bool(true)) {
            stats.store_true_requests += 1;
        }
        stats.last_input_items = input_items(&request_body).len();
        if let Some(prev) = request_body
            .get("previous_response_id")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
        {
            stats.delta_requests += 1;
            stats.last_delta_input_items = Some(input_items(&request_body).len());
            stats.last_previous_response_id = Some(prev.to_owned());
        } else {
            stats.full_context_requests += 1;
            stats.last_delta_input_items = None;
            stats.last_previous_response_id = None;
        }
    }

    let result = stream_once(
        &mut lease.socket,
        &request_body,
        emitter,
        state,
        req.idle_timeout,
        req.cancel,
    )
    .await;
    let keep = match &result {
        Ok(()) if req.cancel.is_cancelled() => false,
        Ok(()) => {
            if use_cached
                && let Some(key) = &lease.key
                && let Some(response_id) = state.response_id.clone()
            {
                let stop = state
                    .stop
                    .unwrap_or(pipkin_engine_protocol::StopReason::Stop);
                let snapshot = state.snapshot(emitter, req.model, stop);
                let items: Vec<Value> = convert_messages(req.model, &[snapshot])
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|item| {
                        !matches!(
                            item.get("type").and_then(Value::as_str),
                            Some("function_call_output" | "custom_tool_call_output")
                        )
                    })
                    .collect();
                if let Some(entry) = lock(shared).entry_mut(key) {
                    entry.continuation = Some(Continuation {
                        last_request_body: req.body.clone(),
                        last_response_id: response_id,
                        last_response_items: items,
                    });
                }
            }
            true
        }
        Err(_) => {
            if let Some(key) = &lease.key
                && let Some(entry) = lock(shared).entry_mut(key)
            {
                entry.continuation = None;
            }
            false
        }
    };
    release(shared, lease, keep);
    result
}

fn close_error(frame: Option<&CloseFrame>) -> Failure {
    let code = frame.map(|f| u16::from(f.code)).unwrap_or(1006);
    let mut reason = frame
        .map(|f| f.reason.as_str().to_owned())
        .filter(|r| !r.is_empty())
        .map(|r| format!(" {r}"))
        .unwrap_or_default();
    if reason.is_empty() && code == MESSAGE_TOO_BIG {
        reason = " message too big".into();
    }
    Failure {
        kind: FailureKind::WsClose(Some(code)),
        message: format!("WebSocket closed {code}{reason}").trim().to_owned(),
        code: Some(code.to_string()),
    }
}

/// Send `response.create` and process frames until the terminal response event (reference
/// `parseWebSocket` + `mapCodexEvents` + `processResponsesStream`).
async fn stream_once(
    ws: &mut WsStream,
    request_body: &Value,
    emitter: &mut Emitter,
    state: &mut ResponsesState,
    idle: Option<Duration>,
    cancel: &CancellationToken,
) -> Result<(), Failure> {
    let mut frame = Map::new();
    frame.insert("type".into(), Value::String("response.create".into()));
    if let Value::Object(body) = request_body {
        for (k, v) in body {
            frame.insert(k.clone(), v.clone());
        }
    }
    let text = Value::Object(frame).to_string();
    tokio::select! {
        _ = cancel.cancelled() => return Err(Failure::aborted()),
        r = ws.send(WsMessage::Text(text.into())) => {
            r.map_err(|e| Failure::error(nonempty(e.to_string(), "WebSocket error")))?;
        }
    }
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(Failure::aborted()),
            n = async {
                match idle {
                    Some(t) => tokio::time::timeout(t, ws.next()).await.map_err(|_| t),
                    None => Ok(ws.next().await),
                }
            } => n,
        };
        let data = match next {
            Err(t) => {
                return Err(Failure::error(format!(
                    "WebSocket idle timeout after {}ms",
                    t.as_millis()
                )));
            }
            Ok(None) => return Err(close_error(None)),
            Ok(Some(Err(tokio_tungstenite::tungstenite::Error::Protocol(
                tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
            ))))
            | Ok(Some(Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed)))
            | Ok(Some(Err(tokio_tungstenite::tungstenite::Error::AlreadyClosed))) => {
                return Err(close_error(None));
            }
            Ok(Some(Err(e))) => {
                return Err(Failure::error(nonempty(e.to_string(), "WebSocket error")));
            }
            Ok(Some(Ok(WsMessage::Close(frame)))) => return Err(close_error(frame.as_ref())),
            Ok(Some(Ok(WsMessage::Text(t)))) => t.as_str().to_owned(),
            Ok(Some(Ok(WsMessage::Binary(b)))) => String::from_utf8_lossy(&b).into_owned(),
            Ok(Some(Ok(_))) => continue,
        };
        if data.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&data).map_err(|e| Failure {
            kind: FailureKind::Protocol,
            message: format!("Invalid Codex WebSocket JSON: {e}"),
            code: None,
        })?;
        if state.handle(emitter, value)? == Step::Done {
            return Ok(());
        }
    }
}
