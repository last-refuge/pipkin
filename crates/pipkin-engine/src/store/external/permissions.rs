//! Offline external authorization ledger, not native tool execution or action projection.
//! `_in` methods never bump revisions or publish; all mutations need a caller-owned transaction.
//! Roll back on error and commit successfully BEFORE publishing a
//! decision or releasing an allow. Only `newly_consumed` authorizes that release; a receipt
//! alone never does. Consumed means authorization may have escaped, NOT tool completion.
//! The actor must independently check the live connection owner, invocation and engine epoch,
//! hash the FULL argument bytes (not a preview), and serialize release against Stop/disconnect.
//! No raw arguments, credentials, free-form descriptions or tool results are stored here.
use super::*;

pub const EXTERNAL_PERMISSION_POLICY: &str = "restricted-write-approval-v1";
const MAX_PER_RUN: u64 = 4096;

#[derive(Clone, PartialEq, Eq)]
pub struct ExternalPermissionKey {
    /// The incarnation returned by Store::next_epoch at engine startup.
    pub engine_epoch: u64,
    /// Opaque, nonsecret connection identity; the actor must authenticate it separately.
    pub owner: String,
    pub run: ExternalRunKey,
    /// External prompt receipt ID, never a native run ID.
    pub prompt: u64,
    /// Host-generated identity, not an untrusted provider call ID.
    pub invocation_nonce: String,
}

impl std::fmt::Debug for ExternalPermissionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExternalPermissionKey([redacted])")
    }
}

/// Immutable bounded metadata. The digest covers the full arguments supplied to the host.
#[derive(Clone, PartialEq, Eq)]
pub struct ExternalPermissionRequest {
    pub key: ExternalPermissionKey,
    pub arguments_sha256: String,
    pub tool_name: String,
    pub policy_id: String,
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for ExternalPermissionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExternalPermissionRequest([redacted])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalPermissionDecision {
    AllowOnce,
    Deny,
}
impl ExternalPermissionDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::AllowOnce => "allowOnce",
            Self::Deny => "deny",
        }
    }
    fn parse(s: &str) -> rusqlite::Result<Self> {
        match s {
            "allowOnce" => Ok(Self::AllowOnce),
            "deny" => Ok(Self::Deny),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalPermissionState {
    Pending,
    Granted,
    Consumed,
    Denied,
    Expired,
    Revoked,
}
impl ExternalPermissionState {
    fn parse(s: &str) -> rusqlite::Result<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "granted" => Ok(Self::Granted),
            "consumed" => Ok(Self::Consumed),
            "denied" => Ok(Self::Denied),
            "expired" => Ok(Self::Expired),
            "revoked" => Ok(Self::Revoked),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalPermissionReceipt {
    pub id: u64,
    pub request: ExternalPermissionRequest,
    pub state: ExternalPermissionState,
    pub decision: Option<ExternalPermissionDecision>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalPermissionConsumption {
    pub receipt: ExternalPermissionReceipt,
    /// False on every duplicate, including a lost acknowledgement. Never reissue an allow.
    pub newly_consumed: bool,
}

fn validate(c: &Connection, key: &ExternalPermissionKey) -> Result<()> {
    validate_key(&key.run)?;
    if !token(&key.owner)
        || !token(&key.invocation_nonce)
        || key.prompt == 0
        || key.prompt > i64::MAX as u64
        || key.engine_epoch == 0
        || key.engine_epoch > i64::MAX as u64
    {
        return Err(StoreError::ExecutionConflict);
    }
    let epoch: Option<String> = c
        .query_row("SELECT value FROM meta WHERE key='epoch'", [], |r| r.get(0))
        .optional()?;
    if epoch.as_deref() != Some(key.engine_epoch.to_string().as_str()) {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}

fn validate_request(c: &Connection, request: &ExternalPermissionRequest) -> Result<()> {
    validate(c, &request.key)?;
    if request.tool_name != "Write"
        || request.policy_id != EXTERNAL_PERMISSION_POLICY
        || request.arguments_sha256.len() != 64
        || !request
            .arguments_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}

fn live(c: &Connection, key: &ExternalPermissionKey) -> Result<()> {
    Store::assert_external_binding(c, &key.run)?;
    let binding =
        Store::cli_binding_in(c, &key.run.session)?.ok_or(StoreError::ExecutionConflict)?;
    let prompt =
        Store::external_prompt_for_run_in(c, &key.run)?.ok_or(StoreError::ExecutionConflict)?;
    if binding.identity.launch_policy != EXTERNAL_PERMISSION_POLICY
        || prompt.id != key.prompt
        || prompt.state != ExternalRunState::Running
        || prompt.terminal.is_some()
    {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ExternalPermissionReceipt> {
    Ok(ExternalPermissionReceipt {
        id: r.get("id")?,
        request: ExternalPermissionRequest {
            key: ExternalPermissionKey {
                engine_epoch: r.get("engine_epoch")?,
                owner: r.get("owner")?,
                run: ExternalRunKey {
                    session: r.get("session")?,
                    generation: r.get("generation")?,
                    op: r.get("op")?,
                },
                prompt: r.get("prompt")?,
                invocation_nonce: r.get("invocation_nonce")?,
            },
            arguments_sha256: r.get("arguments_sha256")?,
            tool_name: r.get("tool_name")?,
            policy_id: r.get("policy_id")?,
            expires_at_ms: r.get("expires_at")?,
        },
        state: ExternalPermissionState::parse(&r.get::<_, String>("state")?)?,
        decision: r
            .get::<_, Option<String>>("decision")?
            .as_deref()
            .map(ExternalPermissionDecision::parse)
            .transpose()?,
        created_at_ms: r.get("created_at")?,
        updated_at_ms: r.get("updated_at")?,
    })
}

fn exact(c: &Connection, request: &ExternalPermissionRequest) -> Result<ExternalPermissionReceipt> {
    validate_request(c, request)?;
    let receipt =
        Store::external_permission_in(c, &request.key)?.ok_or(StoreError::ExecutionConflict)?;
    if receipt.request != *request {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(receipt)
}

fn reload(c: &Connection, key: &ExternalPermissionKey) -> Result<ExternalPermissionReceipt> {
    Store::external_permission_in(c, key)?.ok_or(StoreError::ExecutionConflict)
}

impl Store {
    /// The broker's exclusive reservation shares the existing strict Running CAS.
    /// Callers must roll back on error: validate the epoch, coding policy and receipt
    /// in the same transaction, never reserve from an already Running row.
    pub(crate) fn begin_external_permission_broker_in(
        tx: &Transaction<'_>,
        key: &ExternalPermissionKey,
        now: i64,
    ) -> Result<(
        crate::store::ExternalPromptReceipt,
        Vec<pipkin_engine_protocol::SessionChange>,
    )> {
        validate(tx, key)?;
        let result = Self::begin_external_run_in(tx, &key.run, now)?;
        live(tx, key)?;
        Ok(result)
    }

    pub fn external_permission_in(
        c: &Connection,
        key: &ExternalPermissionKey,
    ) -> Result<Option<ExternalPermissionReceipt>> {
        validate(c, key)?;
        let receipt = c.query_row("SELECT * FROM external_permissions WHERE engine_epoch=?1 AND owner=?2 AND session=?3 AND generation=?4 AND op=?5 AND invocation_nonce=?6",
            params![key.engine_epoch,key.owner,key.run.session,key.run.generation,key.run.op,key.invocation_nonce], from_row).optional()?;
        if receipt.as_ref().is_some_and(|r| r.request.key != *key) {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(receipt)
    }

    /// At most 256 receipts per page, confined to an exact owner/epoch/run/prompt scope.
    pub fn external_permissions_in(
        c: &Connection,
        key: &ExternalPermissionKey,
        after: u64,
        limit: usize,
    ) -> Result<Vec<ExternalPermissionReceipt>> {
        validate(c, key)?;
        if limit == 0 || limit > 256 || after > i64::MAX as u64 {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(c.prepare("SELECT * FROM external_permissions WHERE engine_epoch=?1 AND owner=?2 AND session=?3 AND generation=?4 AND op=?5 AND prompt=?6 AND id>?7 ORDER BY id LIMIT ?8")?
            .query_map(params![key.engine_epoch,key.owner,key.run.session,key.run.generation,key.run.op,key.prompt,after,limit], from_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn request_external_permission_in(
        tx: &Transaction<'_>,
        request: &ExternalPermissionRequest,
        now: i64,
    ) -> Result<ExternalPermissionReceipt> {
        validate_request(tx, request)?;
        if Self::external_permission_in(tx, &request.key)?.is_some() {
            return exact(tx, request);
        }
        live(tx, &request.key)?;
        if request.expires_at_ms <= now {
            return Err(StoreError::ExecutionConflict);
        }
        let k = &request.key;
        let count: u64 = tx.query_row("SELECT COUNT(*) FROM external_permissions WHERE session=?1 AND generation=?2 AND op=?3", params![k.run.session,k.run.generation,k.run.op], |r| r.get(0))?;
        if count >= MAX_PER_RUN {
            return Err(StoreError::ExecutionConflict);
        }
        tx.execute("INSERT INTO external_permissions(engine_epoch,owner,session,generation,op,prompt,invocation_nonce,arguments_sha256,tool_name,policy_id,expires_at,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending',?12,?12)",
            params![k.engine_epoch,k.owner,k.run.session,k.run.generation,k.run.op,k.prompt,k.invocation_nonce,request.arguments_sha256,request.tool_name,request.policy_id,request.expires_at_ms,now])?;
        reload(tx, k)
    }

    /// Exact duplicate decisions return a receipt ONLY, even after consumption/revocation.
    /// Conflicting decision or immutable payload fails. Expiry is a successful persisted receipt.
    pub fn decide_external_permission_in(
        tx: &Transaction<'_>,
        request: &ExternalPermissionRequest,
        decision: ExternalPermissionDecision,
        now: i64,
    ) -> Result<ExternalPermissionReceipt> {
        let receipt = exact(tx, request)?;
        if let Some(old) = receipt.decision {
            if old != decision {
                return Err(StoreError::ExecutionConflict);
            }
            return Ok(receipt);
        }
        if receipt.state != ExternalPermissionState::Pending {
            return Err(StoreError::ExecutionConflict);
        }
        if now >= request.expires_at_ms {
            return Self::expire_external_permission_in(tx, &request.key, now);
        }
        live(tx, &request.key)?;
        tx.execute("UPDATE external_permissions SET state=?2,decision=?3,updated_at=MAX(updated_at,?4) WHERE id=?1", params![receipt.id,if decision == ExternalPermissionDecision::AllowOnce {"granted"} else {"denied"},decision.as_str(),now])?;
        reload(tx, &request.key)
    }

    pub fn consume_external_permission_in(
        tx: &Transaction<'_>,
        request: &ExternalPermissionRequest,
        now: i64,
    ) -> Result<ExternalPermissionConsumption> {
        let receipt = exact(tx, request)?;
        if receipt.state == ExternalPermissionState::Consumed {
            return Ok(ExternalPermissionConsumption {
                receipt,
                newly_consumed: false,
            });
        }
        if receipt.state != ExternalPermissionState::Granted {
            return Err(StoreError::ExecutionConflict);
        }
        if now >= request.expires_at_ms {
            return Ok(ExternalPermissionConsumption {
                receipt: Self::expire_external_permission_in(tx, &request.key, now)?,
                newly_consumed: false,
            });
        }
        live(tx, &request.key)?;
        tx.execute("UPDATE external_permissions SET state='consumed',updated_at=MAX(updated_at,?2) WHERE id=?1", params![receipt.id,now])?;
        Ok(ExternalPermissionConsumption {
            receipt: reload(tx, &request.key)?,
            newly_consumed: true,
        })
    }

    pub fn revoke_external_permission_in(
        tx: &Transaction<'_>,
        key: &ExternalPermissionKey,
        now: i64,
    ) -> Result<ExternalPermissionReceipt> {
        let receipt = reload(tx, key)?;
        tx.execute("UPDATE external_permissions SET state='revoked',updated_at=MAX(updated_at,?2) WHERE id=?1 AND state IN ('pending','granted')", params![receipt.id,now])?;
        reload(tx, key)
    }

    pub fn expire_external_permission_in(
        tx: &Transaction<'_>,
        key: &ExternalPermissionKey,
        now: i64,
    ) -> Result<ExternalPermissionReceipt> {
        let receipt = reload(tx, key)?;
        tx.execute("UPDATE external_permissions SET state='expired',updated_at=MAX(updated_at,?2) WHERE id=?1 AND expires_at<=?2 AND state IN ('pending','granted')", params![receipt.id,now])?;
        reload(tx, key)
    }

    /// Broker cleanup is receipt-scoped but remains available after context invalidation
    /// or removal intent. It never creates authority or changes the run outcome.
    pub(crate) fn revoke_external_prompt_permissions_in(
        tx: &Transaction<'_>,
        key: &ExternalRunKey,
        prompt: u64,
        now: i64,
    ) -> Result<usize> {
        validate_key(key)?;
        let receipt =
            Self::external_prompt_for_run_in(tx, key)?.ok_or(StoreError::ExecutionConflict)?;
        let binding =
            Self::cli_binding_in(tx, &key.session)?.ok_or(StoreError::ExecutionConflict)?;
        if receipt.id != prompt || binding.identity.launch_policy != EXTERNAL_PERMISSION_POLICY {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(tx.execute("UPDATE external_permissions SET state='revoked',updated_at=MAX(updated_at,?4) WHERE session=?1 AND generation=?2 AND op=?3 AND state IN ('pending','granted')", params![key.session,key.generation,key.op,now])?)
    }

    pub(super) fn revoke_external_permissions_for_run_in(
        c: &Connection,
        key: &ExternalRunKey,
        now: i64,
    ) -> Result<()> {
        c.execute("UPDATE external_permissions SET state='revoked',updated_at=MAX(updated_at,?4) WHERE session=?1 AND generation=?2 AND op=?3 AND state IN ('pending','granted')", params![key.session,key.generation,key.op,now])?;
        Ok(())
    }

    pub(super) fn assert_external_permissions_resolved(
        c: &Connection,
        key: &ExternalRunKey,
    ) -> Result<()> {
        if c.query_row("SELECT EXISTS(SELECT 1 FROM external_permissions WHERE session=?1 AND generation=?2 AND op=?3 AND state IN ('pending','granted'))", params![key.session,key.generation,key.op], |r| r.get::<_, bool>(0))? {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(())
    }
}
