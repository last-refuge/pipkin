//! Inactive durable admission substrate for engine-owned CLI tools.
//!
//! No public session creator, helper authentication, tool execution or CLI launch is added
//! here. Only the reserved `claudeCliAgent` database identity may own a bridge; it remains
//! Unsupported to existing readers and cannot pass native API execution guards. The future
//! driver must authenticate its private transport, bind the preflight identity, validate the
//! native tool/schema, and atomically join this journal with native tool intent/result records
//! before enabling execution. A protocol Call by itself cannot obtain a Lease or Claim.
use super::*;
use crate::execution::claude_cli::orchestration_protocol::{
    MAX_FRAME_BYTES, MAX_PENDING, MAX_REQUESTS, MAX_RESULT_BODY_BYTES, MAX_RESULT_BYTES, Scope,
};
use sha2::{Digest, Sha256};

pub const ORCHESTRATION_BACKEND: &str = "claudeCliAgent";
pub const ORCHESTRATION_POLICY: &str = "cli-native-orchestration-v1";
/// Bound retained arguments/results across the whole run, not merely per request.
const MAX_JOURNAL_BYTES: i64 = 256 * 1024 * 1024;

/// Prepared by the trusted driver, never decoded from CLI output or client RPC parameters.
/// Digests are of independently captured binding identity and a random one-use capability.
/// A matching digest is NOT transport authentication; the owning driver must do that first.
#[derive(Clone, PartialEq, Eq)]
pub struct BridgeSpec {
    pub id: String,
    pub scope: Scope,
    pub owner: String,
    pub binding_hash: [u8; 32],
    pub capability_hash: [u8; 32],
}
/// Non-serializable owner handle, issued only after the one-use connection claim commits.
pub struct Lease(BridgeSpec);
impl Lease {
    pub fn scope(&self) -> &Scope {
        &self.0.scope
    }
}
/// One-time dispatch admission. Never reconstructed from stored intents, even replay-safe ones.
/// Not yet sufficient to execute: the native driver integration described above is still absent.
pub struct Claim {
    bridge: String,
    rpc_key: String,
    fingerprint: [u8; 32],
    call_id: String,
}
impl Claim {
    pub fn call_id(&self) -> &str {
        &self.call_id
    }
}
pub enum Admission {
    New(Claim),
    Pending,
    Completed(Value),
    Unknown,
    Fenced,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Settlement {
    Committed,
    AlreadyCommitted,
    Unknown,
    Fenced,
}
macro_rules! redacted_debug {
    ($($t:ty),*) => { $(impl std::fmt::Debug for $t {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(concat!(stringify!($t), "([redacted])"))
        }
    })* };
}
redacted_debug!(BridgeSpec, Lease, Claim, Admission);

fn token(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn canonical(v: &Value) -> String {
    let mut v = v.clone();
    v.sort_all_objects();
    v.to_string()
}
fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn rpc_key(id: &Value) -> Result<String> {
    if id.as_i64().is_none() && !id.as_str().is_some_and(token) {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(id.to_string())
}
fn validate(spec: &BridgeSpec) -> Result<()> {
    if [
        &spec.id,
        &spec.scope.session,
        &spec.scope.operation,
        &spec.owner,
    ]
    .into_iter()
    .any(|s| !token(s))
        || spec.scope.generation == 0
        || spec.scope.generation > i64::MAX as u64
        || spec.scope.run == 0
        || spec.scope.run > i64::MAX as u64
    {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}
fn stored(c: &Connection, id: &str) -> Result<Option<(BridgeSpec, String)>> {
    Ok(
        c.query_row("SELECT * FROM cli_tool_bridges WHERE id=?1", [id], |r| {
            Ok((
                BridgeSpec {
                    id: r.get("id")?,
                    scope: Scope {
                        session: r.get("session")?,
                        generation: r.get::<_, i64>("generation")? as u64,
                        operation: r.get("operation")?,
                        run: r.get::<_, i64>("run")? as u64,
                    },
                    owner: r.get("owner")?,
                    binding_hash: r.get("binding_hash")?,
                    capability_hash: r.get("capability_hash")?,
                },
                r.get("state")?,
            ))
        })
        .optional()?,
    )
}
fn live_scope(c: &Connection, scope: &Scope) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sessions s JOIN runs r ON r.session=s.id
        WHERE s.id=?1 AND s.execution_backend=?2 AND s.blocked IS NULL AND r.id=?3
        AND r.state='running' AND r.stop_requested_at IS NULL
        AND NOT EXISTS(SELECT 1 FROM session_removals m WHERE m.root=s.id OR m.root=s.root)
        AND NOT EXISTS(SELECT 1 FROM sessions a WHERE a.id IN(s.parent,s.root) AND a.execution_backend<>?2))",
        params![scope.session, ORCHESTRATION_BACKEND, scope.run as i64], |r| r.get(0))?)
}
fn owned_state(c: &Connection, lease: &Lease) -> Result<String> {
    let Some((spec, state)) = stored(c, &lease.0.id)? else {
        return Err(StoreError::ExecutionConflict);
    };
    if spec != lease.0 {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(state)
}
pub(super) fn fence(c: &Connection, id: &str, now: i64) -> Result<()> {
    let changed = c.execute("UPDATE tool_calls SET state='interrupted',ended_at=?2
        WHERE state='intent' AND EXISTS(SELECT 1 FROM cli_bridge_calls b JOIN cli_tool_bridges g ON g.id=b.bridge
        WHERE b.bridge=?1 AND b.state='intent' AND b.session=tool_calls.session AND b.call_id=tool_calls.call_id AND g.run=tool_calls.run)", params![id,now])?;
    if changed > 0 {
        let session: String = c.query_row(
            "SELECT session FROM cli_tool_bridges WHERE id=?1",
            [id],
            |r| r.get(0),
        )?;
        Store::bump(c, &session, now)?;
    }
    c.execute(
        "UPDATE cli_bridge_calls SET state='unknown',updated_at=MAX(updated_at,?2)
        WHERE bridge=?1 AND state='intent'",
        params![id, now],
    )?;
    c.execute(
        "UPDATE cli_tool_bridges SET state='fenced',updated_at=MAX(updated_at,?2)
        WHERE id=?1 AND state IN ('prepared','connected')",
        params![id, now],
    )?;
    Ok(())
}
fn journal_bytes(c: &Connection, id: &str) -> Result<i64> {
    Ok(c.query_row("SELECT COALESCE(SUM(length(CAST(arguments AS BLOB))+COALESCE(length(CAST(result AS BLOB)),0)),0)
        FROM cli_bridge_calls WHERE bridge=?1", [id], |r| r.get(0))?)
}

impl Store {
    /// Freeze a new identity once. Exact repeats are lookup only, never a new grant.
    pub fn prepare_cli_tool_bridge(&self, spec: &BridgeSpec, now: i64) -> Result<bool> {
        validate(spec)?;
        self.tx(|tx| {
            if let Some((old, _)) = stored(tx, &spec.id)? {
                return if old == *spec { Ok(false) } else { Err(StoreError::ExecutionConflict) };
            }
            if !live_scope(tx, &spec.scope)? { return Err(StoreError::UnsupportedExecution); }
            tx.execute("INSERT INTO cli_tool_bridges(id,session,generation,operation,run,owner,binding_hash,capability_hash,policy,state,created_at,updated_at)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'prepared',?10,?10)",
                params![spec.id,spec.scope.session,spec.scope.generation as i64,spec.scope.operation,spec.scope.run as i64,
                    spec.owner,spec.binding_hash,spec.capability_hash,ORCHESTRATION_POLICY,now])?;
            Ok(true)
        })
    }

    /// After independent transport authentication, consume the one connection claim.
    /// Wrong identities do not revoke a legitimate owner's channel. No reconnection grant.
    pub fn claim_cli_tool_bridge(&self, spec: &BridgeSpec, now: i64) -> Result<Option<Lease>> {
        validate(spec)?;
        self.tx(|tx| {
            let Some((old,state)) = stored(tx, &spec.id)? else { return Err(StoreError::ExecutionConflict); };
            if old != *spec { return Err(StoreError::ExecutionConflict); }
            if state != "prepared" { return Ok(None); }
            if !live_scope(tx, &spec.scope)? { fence(tx, &spec.id, now)?; return Ok(None); }
            tx.execute("UPDATE cli_tool_bridges SET state='connected',updated_at=MAX(updated_at,?2) WHERE id=?1", params![spec.id,now])?;
            Ok(Some(Lease(spec.clone())))
        })
    }

    /// Commit intent before returning New. A lost response leaves Pending, never another New.
    /// Fences are successful transactions even when the returned admission is Fenced.
    pub fn admit_cli_bridge_call(
        &self,
        lease: &Lease,
        id: &Value,
        name: &str,
        arguments: &Value,
        now: i64,
    ) -> Result<Admission> {
        self.tx(|tx| Self::admit_cli_bridge_call_in(tx, lease, id, name, arguments, now))
    }

    fn admit_cli_bridge_call_in(
        tx: &Connection,
        lease: &Lease,
        id: &Value,
        name: &str,
        arguments: &Value,
        now: i64,
    ) -> Result<Admission> {
        let key = rpc_key(id)?;
        if !token(name) || !arguments.is_object() {
            return Err(StoreError::ExecutionConflict);
        }
        let fingerprint = digest(canonical(&serde_json::json!([name, arguments])).as_bytes());
        let arguments = canonical(arguments);
        if arguments.len() > MAX_FRAME_BYTES {
            return Err(StoreError::ExecutionConflict);
        }
        {
            let state = owned_state(tx, lease)?;
            if state != "connected" || !live_scope(tx, &lease.0.scope)? {
                fence(tx, &lease.0.id, now)?;
                return Ok(Admission::Fenced);
            }
            let old: Option<(Vec<u8>,String,Option<String>)> = tx.query_row(
                "SELECT fingerprint,state,result FROM cli_bridge_calls WHERE bridge=?1 AND rpc_key=?2",
                params![lease.0.id,key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((hash, state, result)) = old {
                if hash.as_slice() != fingerprint {
                    fence(tx, &lease.0.id, now)?;
                    return Ok(Admission::Fenced);
                }
                return Ok(match state.as_str() {
                    "intent" => Admission::Pending,
                    "completed" => Admission::Completed(parse_json(
                        &result
                            .ok_or_else(|| StoreError::Corrupt("missing bridge result".into()))?,
                        "bridge result",
                    )?),
                    _ => Admission::Unknown,
                });
            }
            let (count,pending): (i64,i64) = tx.query_row("SELECT COUNT(*),COALESCE(SUM(state='intent'),0) FROM cli_bridge_calls WHERE bridge=?1",
                [&lease.0.id], |r| Ok((r.get(0)?,r.get(1)?)))?;
            if count >= MAX_REQUESTS as i64
                || pending >= MAX_PENDING as i64
                || journal_bytes(tx, &lease.0.id)? + arguments.len() as i64 > MAX_JOURNAL_BYTES
            {
                fence(tx, &lease.0.id, now)?;
                return Ok(Admission::Fenced);
            }
            let call_id = format!(
                "cb-{:x}",
                Sha256::digest(canonical(&serde_json::json!([lease.0.id, key])).as_bytes())
            );
            tx.execute("INSERT INTO cli_bridge_calls(bridge,session,rpc_key,call_id,fingerprint,name,arguments,state,created_at,updated_at)
                VALUES(?1,?2,?3,?4,?5,?6,?7,'intent',?8,?8)", params![lease.0.id,lease.0.scope.session,key,call_id,fingerprint,name,arguments,now])?;
            Ok(Admission::New(Claim {
                bridge: lease.0.id.clone(),
                rpc_key: key,
                fingerprint,
                call_id,
            }))
        }
    }

    /// Commit the actual result before a protocol reply. Contradictory results fence the
    /// connection but do not replace the original receipt. Unknown is never upgraded.
    pub fn settle_cli_bridge_call(
        &self,
        lease: &Lease,
        claim: &Claim,
        result: &Value,
        now: i64,
    ) -> Result<Settlement> {
        self.tx(|tx| Self::settle_cli_bridge_call_in(tx, lease, claim, result, now))
    }

    fn settle_cli_bridge_call_in(
        tx: &Connection,
        lease: &Lease,
        claim: &Claim,
        result: &Value,
        now: i64,
    ) -> Result<Settlement> {
        if claim.bridge != lease.0.id {
            return Err(StoreError::ExecutionConflict);
        }
        let is_object = result.is_object();
        let result = canonical(result);
        let invalid_result = !is_object || result.len() > MAX_RESULT_BODY_BYTES;
        {
            let state = owned_state(tx, lease)?;
            let (hash, call_state, old): (Vec<u8>,String,Option<String>) = tx.query_row(
                "SELECT fingerprint,state,result FROM cli_bridge_calls WHERE bridge=?1 AND rpc_key=?2 AND call_id=?3",
                params![claim.bridge,claim.rpc_key,claim.call_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            if hash.as_slice() != claim.fingerprint {
                return Err(StoreError::ExecutionConflict);
            }
            if invalid_result {
                fence(tx, &lease.0.id, now)?;
                return Ok(Settlement::Fenced);
            }
            if call_state == "completed" {
                if old.as_deref() == Some(&result) {
                    return Ok(Settlement::AlreadyCommitted);
                }
                fence(tx, &lease.0.id, now)?;
                return Ok(Settlement::Fenced);
            }
            if call_state == "unknown" {
                return Ok(Settlement::Unknown);
            }
            if state != "connected"
                || !live_scope(tx, &lease.0.scope)?
                || journal_bytes(tx, &lease.0.id)? + result.len() as i64 > MAX_JOURNAL_BYTES
            {
                fence(tx, &lease.0.id, now)?;
                return Ok(Settlement::Fenced);
            }
            tx.execute("UPDATE cli_bridge_calls SET state='completed',result=?3,updated_at=MAX(updated_at,?4)
                WHERE bridge=?1 AND rpc_key=?2", params![claim.bridge,claim.rpc_key,result,now])?;
            Ok(Settlement::Committed)
        }
    }

    /// Stop, disconnect or uncertain delivery: revoke first; late workers cannot settle Unknown.
    pub fn fence_cli_tool_bridge(&self, lease: &Lease, now: i64) -> Result<()> {
        self.tx(|tx| {
            owned_state(tx, lease)?;
            fence(tx, &lease.0.id, now)
        })
    }
    /// Clean terminal only with no outstanding effects; otherwise revoke as uncertain.
    pub fn close_cli_tool_bridge(&self, lease: &Lease, now: i64) -> Result<bool> {
        self.tx(|tx| {
            let state = owned_state(tx, lease)?;
            if state == "closed" { return Ok(true); }
            if state != "connected" { return Ok(false); }
            let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM cli_bridge_calls WHERE bridge=?1 AND state<>'completed')", [&lease.0.id], |r| r.get(0))?;
            if pending || !live_scope(tx, &lease.0.scope)? { fence(tx, &lease.0.id, now)?; return Ok(false); }
            tx.execute("UPDATE cli_tool_bridges SET state='closed',updated_at=MAX(updated_at,?2) WHERE id=?1", params![lease.0.id,now])?;
            Ok(true)
        })
    }
    /// Trusted owner cleanup for an unclaimed or connected identity. This is not a client
    /// authentication endpoint; callers retain the exact engine-captured identity.
    pub fn retire_cli_tool_bridge(&self, spec: &BridgeSpec, now: i64) -> Result<()> {
        validate(spec)?;
        self.tx(|tx| {
            let Some((old, _)) = stored(tx, &spec.id)? else {
                return Err(StoreError::ExecutionConflict);
            };
            if old != *spec {
                return Err(StoreError::ExecutionConflict);
            }
            fence(tx, &spec.id, now)
        })
    }

    /// Startup retirement only. No reconnection, receipt promotion, execution or result replay.
    pub fn recover_cli_tool_bridges(&self, now: i64) -> Result<usize> {
        self.tx(|tx| {
            let ids = tx
                .prepare("SELECT id FROM cli_tool_bridges WHERE state IN ('prepared','connected')")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for id in &ids {
                fence(tx, id, now)?;
            }
            Ok(ids.len())
        })
    }
}

pub mod native;

#[cfg(test)]
mod tests;
