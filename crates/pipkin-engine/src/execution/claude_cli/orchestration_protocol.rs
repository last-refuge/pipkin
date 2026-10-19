//! Bounded MCP control-channel state for the future engine-owned CLI tool bridge.
//!
//! NOT wired into CLI launch or advertised as a capability. This is a protocol decoder,
//! not execution authority: only an independently authenticated, run-scoped channel may
//! feed it. CLI stdout/assistant markup must never feed it. The driver must durably admit
//! each Call before effects, persist its result before complete(), and service Replay by
//! reading that receipt, never by executing again. Disconnect fences the owning run; a
//! new instance does not authorize reconnect/replay. No filesystem, process or tool I/O.
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::provider::ToolSpec;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const MAX_FRAME_BYTES: usize = pipkin_engine_protocol::limits::MAX_FRAME_BYTES;
pub const MAX_RESULT_BYTES: usize = pipkin_engine_protocol::limits::MAX_FRAME_BYTES;
/// Reserve room for the typed RPC id and JSON-RPC response envelope.
pub const MAX_RESULT_BODY_BYTES: usize = MAX_RESULT_BYTES - 1024;
pub const MAX_CATALOG_BYTES: usize = 256 * 1024;
pub const MAX_REQUESTS: usize = 4096;
pub const MAX_PENDING: usize = 64;
const MAX_TOOLS: usize = 128;

/// Engine-captured routing identity. The client cannot replace it in an RPC payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub session: String,
    pub generation: u64,
    pub operation: String,
    pub run: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RpcId {
    Number(i64),
    Text(String),
}
impl RpcId {
    fn parse(value: &Value) -> Option<Self> {
        if let Some(n) = value.as_i64() {
            Some(Self::Number(n))
        } else {
            value
                .as_str()
                .filter(|s| token(s, 128))
                .map(|s| Self::Text(s.into()))
        }
    }
    fn value(&self) -> Value {
        match self {
            Self::Number(n) => json!(n),
            Self::Text(s) => json!(s),
        }
    }
}

/// Opaque correlation, including the protocol instance. Not a permission to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallKey {
    instance: u64,
    id: RpcId,
    scope: Arc<Scope>,
}
impl CallKey {
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// Stable, typed RPC identity for the owning runtime's durable receipt key.
    /// Must be combined with the independently persisted bridge/run identity.
    pub fn request_id(&self) -> Value {
        self.id.value()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Closed,
    FrameLimit,
    RequestLimit,
    ConflictingId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupError {
    Scope,
    Catalog,
    InstanceLimit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionError {
    Closed,
    ForeignCall,
    AlreadyCompleted,
    InvalidResult,
    ResultLimit,
    ConflictingResult,
}

#[derive(PartialEq)]
pub enum Incoming {
    Response(Value),
    /// Intent only: the runtime must independently validate authority and commit admission.
    Call {
        key: CallKey,
        name: String,
        arguments: Value,
    },
    /// Duplicate pending request: join the original result, never dispatch another call.
    Pending(CallKey),
    /// Duplicate settled request: read the durable receipt, never dispatch another call.
    Replay(CallKey),
    Cancel(CallKey),
    Ignore,
    /// The driver must revoke the channel, stop admission and cancel its owned work.
    Fence(Fault),
}

impl std::fmt::Debug for Incoming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Arguments and receipt bodies may contain project/private content.
        match self {
            Self::Response(_) => f.write_str("Response([redacted])"),
            Self::Call { .. } => f.write_str("Call([redacted])"),
            Self::Pending(_) => f.write_str("Pending([redacted])"),
            Self::Replay(_) => f.write_str("Replay([redacted])"),
            Self::Cancel(_) => f.write_str("Cancel([redacted])"),
            Self::Ignore => f.write_str("Ignore"),
            Self::Fence(reason) => f.debug_tuple("Fence").field(reason).finish(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    New,
    Initializing,
    Ready,
    Closed,
}
enum Status {
    Response(Arc<Value>),
    Pending { key: CallKey, cancelled: bool },
    Completed { key: CallKey, result_hash: [u8; 32] },
}
struct Record {
    fingerprint: [u8; 32],
    status: Status,
}

pub struct Session {
    scope: Arc<Scope>,
    instance: u64,
    phase: Phase,
    names: HashSet<String>,
    catalog: Arc<Value>,
    records: HashMap<RpcId, Record>,
    pending: usize,
}

fn fingerprint(mut value: Value) -> [u8; 32] {
    value.sort_all_objects();
    Sha256::digest(serde_json::to_vec(&value).expect("JSON value")).into()
}

fn token(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.trim() == s && !s.chars().any(char::is_control)
}
fn response(id: Value, payload: &Value) -> Value {
    let mut envelope = payload.clone();
    envelope["jsonrpc"] = json!("2.0");
    envelope["id"] = id;
    envelope
}
fn error(code: i64, message: &'static str) -> Value {
    json!({"error":{"code":code,"message":message}})
}

impl Session {
    /// Only a fully initialized protocol with no outstanding calls can quiesce. This is
    /// necessary, not sufficient: the owner also checks workers, queued frames and storage.
    pub fn is_quiescent(&self) -> bool {
        self.phase == Phase::Ready && self.pending == 0
    }
    pub fn new(scope: Scope, tools: &[ToolSpec]) -> Result<Self, SetupError> {
        if !token(&scope.session, 256) || !token(&scope.operation, 256) || scope.run == 0 {
            return Err(SetupError::Scope);
        }
        if tools.len() > MAX_TOOLS {
            return Err(SetupError::Catalog);
        }
        let mut names = HashSet::new();
        let mut catalog = Vec::new();
        let mut bytes = 0;
        for t in tools {
            if !token(&t.name, 64)
                || !t
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
                || !names.insert(t.name.clone())
                || t.description.len() > MAX_CATALOG_BYTES
                || t.parameters.get("type").and_then(Value::as_str) != Some("object")
                || t.constrained_sampling.is_some()
            {
                return Err(SetupError::Catalog);
            }
            let value =
                json!({"name":t.name,"description":t.description,"inputSchema":t.parameters});
            bytes += serde_json::to_vec(&value)
                .map_err(|_| SetupError::Catalog)?
                .len();
            if bytes > MAX_CATALOG_BYTES {
                return Err(SetupError::Catalog);
            }
            catalog.push(value);
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let instance = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| SetupError::InstanceLimit)?;
        Ok(Self {
            scope: Arc::new(scope),
            instance,
            phase: Phase::New,
            names,
            catalog: Arc::new(json!({"result":{"tools":catalog}})),
            records: HashMap::new(),
            pending: 0,
        })
    }

    /// Immutable offered schema identity. Authentication alone does not establish that the
    /// runtime's actual tool catalog matches the owner's captured authorization.
    pub fn catalog_hash(&self) -> [u8; 32] {
        fingerprint(
            json!({"domain":"pipkin-native-tool-catalog-v1","protocol":PROTOCOL_VERSION,"catalog":self.catalog}),
        )
    }

    /// Protocol slots only, never proof that effects or process cleanup have finished.
    pub fn pending(&self) -> usize {
        self.pending
    }
    pub fn is_closed(&self) -> bool {
        self.phase == Phase::Closed
    }

    /// Terminal for this channel, including after EOF or a failed reply write.
    pub fn close(&mut self) {
        self.phase = Phase::Closed;
        self.records.clear();
        self.pending = 0;
    }
    fn fence(&mut self, reason: Fault) -> Incoming {
        self.close();
        Incoming::Fence(reason)
    }

    pub fn receive(&mut self, bytes: &[u8]) -> Incoming {
        if self.is_closed() {
            return Incoming::Fence(Fault::Closed);
        }
        if bytes.len() > MAX_FRAME_BYTES {
            return self.fence(Fault::FrameLimit);
        }
        let value: Value = match serde_json::from_slice(bytes) {
            Ok(v) => v,
            Err(_) => {
                return Incoming::Response(response(Value::Null, &error(-32700, "Invalid JSON")));
            }
        };
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            return Incoming::Response(response(Value::Null, &error(-32600, "Invalid request")));
        };
        if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Incoming::Response(response(Value::Null, &error(-32600, "Invalid request")));
        }
        let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(raw_id) = value.get("id") else {
            return self.notification(method, &params);
        };
        let Some(id) = RpcId::parse(raw_id) else {
            return Incoming::Response(response(Value::Null, &error(-32600, "Invalid request id")));
        };
        let fingerprint = fingerprint(json!([method, params]));
        if let Some(prior) = self.records.get(&id) {
            if prior.fingerprint != fingerprint {
                return self.fence(Fault::ConflictingId);
            }
            return match &prior.status {
                Status::Response(payload) => Incoming::Response(response(id.value(), payload)),
                Status::Pending { key, .. } => Incoming::Pending(key.clone()),
                Status::Completed { key, .. } => Incoming::Replay(key.clone()),
            };
        }
        if self.records.len() >= MAX_REQUESTS {
            return self.fence(Fault::RequestLimit);
        }
        let payload = if !params.is_object() {
            Arc::new(error(-32602, "Parameters must be an object"))
        } else if method == "initialize" && self.phase == Phase::New {
            if params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .is_none_or(|v| !token(v, 64))
                || !params.get("capabilities").is_some_and(Value::is_object)
                || ["name", "version"].iter().any(|field| {
                    params
                        .get("clientInfo")
                        .and_then(|i| i.get(field))
                        .and_then(Value::as_str)
                        .is_none_or(|v| !token(v, 128))
                })
            {
                Arc::new(error(-32602, "Invalid initialization"))
            } else {
                self.phase = Phase::Initializing;
                Arc::new(json!({"result":{"protocolVersion":PROTOCOL_VERSION,
                    "capabilities":{"tools":{"listChanged":false}},
                    "serverInfo":{"name":"pipkin-orchestration","version":"1"}}}))
            }
        } else if method == "ping" {
            Arc::new(json!({"result":{}}))
        } else if self.phase != Phase::Ready {
            Arc::new(error(-32002, "Initialization required"))
        } else {
            match method {
                "tools/list" if params.get("cursor").is_none() => self.catalog.clone(),
                "tools/list" => Arc::new(error(-32602, "Unexpected catalog cursor")),
                "tools/call" => {
                    let name = params
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let arguments = params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    if !self.names.contains(name) || !arguments.is_object() {
                        Arc::new(error(-32602, "Unknown tool or invalid arguments"))
                    } else if self.pending >= MAX_PENDING {
                        Arc::new(error(-32000, "Too many pending calls"))
                    } else {
                        let key = CallKey {
                            instance: self.instance,
                            id: id.clone(),
                            scope: self.scope.clone(),
                        };
                        self.records.insert(
                            id,
                            Record {
                                fingerprint,
                                status: Status::Pending {
                                    key: key.clone(),
                                    cancelled: false,
                                },
                            },
                        );
                        self.pending += 1;
                        return Incoming::Call {
                            key,
                            name: name.into(),
                            arguments,
                        };
                    }
                }
                _ => Arc::new(error(-32601, "Method not supported")),
            }
        };
        let reply = response(id.value(), &payload);
        self.records.insert(
            id,
            Record {
                fingerprint,
                status: Status::Response(payload),
            },
        );
        Incoming::Response(reply)
    }

    fn notification(&mut self, method: &str, params: &Value) -> Incoming {
        if !params.is_object() {
            return Incoming::Ignore;
        }
        if method == "notifications/initialized" && self.phase == Phase::Initializing {
            self.phase = Phase::Ready;
        } else if method == "notifications/cancelled"
            && self.phase == Phase::Ready
            && let Some(id) = params.get("requestId").and_then(RpcId::parse)
            && let Some(Record {
                status: Status::Pending { key, cancelled },
                ..
            }) = self.records.get_mut(&id)
            && !*cancelled
        {
            *cancelled = true;
            return Incoming::Cancel(key.clone());
        }
        Incoming::Ignore
    }

    /// Call only AFTER the owning runtime durably commits the actual result.
    pub fn complete(&mut self, key: &CallKey, result: Value) -> Result<Value, CompletionError> {
        self.check_key(key, false)?;
        let reply = self.encode_result(key, result)?;
        self.records.get_mut(&key.id).expect("checked key").status = Status::Completed {
            key: key.clone(),
            result_hash: fingerprint(reply["result"].clone()),
        };
        self.pending -= 1;
        Ok(reply)
    }

    /// Encode a previously committed receipt for a duplicate, without another execution.
    pub fn replay(&mut self, key: &CallKey, result: Value) -> Result<Value, CompletionError> {
        self.check_key(key, true)?;
        let reply = self.encode_result(key, result)?;
        let matches = matches!(&self.records.get(&key.id).expect("checked key").status,
            Status::Completed { result_hash, .. } if *result_hash == fingerprint(reply["result"].clone()));
        if !matches {
            self.close();
            return Err(CompletionError::ConflictingResult);
        }
        Ok(reply)
    }

    fn check_key(&self, key: &CallKey, completed: bool) -> Result<(), CompletionError> {
        if self.is_closed() {
            return Err(CompletionError::Closed);
        }
        if key.instance != self.instance || key.scope != self.scope {
            return Err(CompletionError::ForeignCall);
        }
        match self.records.get(&key.id).map(|r| &r.status) {
            Some(Status::Pending { key: own, .. }) if own == key && !completed => Ok(()),
            Some(Status::Completed { key: own, .. }) if own == key && completed => Ok(()),
            Some(Status::Completed { .. }) => Err(CompletionError::AlreadyCompleted),
            _ => Err(CompletionError::ForeignCall),
        }
    }
    fn encode_result(&mut self, key: &CallKey, result: Value) -> Result<Value, CompletionError> {
        if !result.is_object() {
            return Err(CompletionError::InvalidResult);
        }
        let reply = response(key.id.value(), &json!({"result":result}));
        if serde_json::to_vec(&reply).expect("JSON value").len() > MAX_RESULT_BYTES {
            self.close();
            return Err(CompletionError::ResultLimit);
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests;
