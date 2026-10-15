//! Bounded, namespace-scoped receipt facts. No plaintext review, ticket, grant or owner proof.
use super::*;
use pipkin_core::{
    CLI_APPROVAL_CONTEXTS, CLI_APPROVAL_REQUESTS, CliApprovalContext, CliApprovalDecision as D,
    CliApprovalId, CliApprovalReceiptState as S, CliApprovalRecovery as Record, CliApprovalRun,
    OperationId,
};

#[cfg(test)]
#[path = "cli_approval_recovery_tests.rs"]
mod tests;

const MAX_RECORD: usize = 2048;
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    conversation: u64,
    generation: u64,
    run_op: u64,
    epoch: u64,
    session: String,
    binding_generation: u64,
    op: String,
    prompt: u64,
    invocation: String,
    digest: String,
    allow: bool,
    receipt: u8,
}
impl Saved {
    fn from_record(r: &Record) -> Self {
        let run = &r.context.run;
        Self {
            conversation: r.context.conversation.0,
            generation: r.context.generation,
            run_op: r.context.run_op.0,
            epoch: run.epoch,
            session: run.session.clone(),
            binding_generation: run.binding_generation,
            op: run.op.clone(),
            prompt: run.prompt,
            invocation: r.id.invocation.clone(),
            digest: r.id.arguments_sha256.clone(),
            allow: r.decision == D::AllowOnce,
            receipt: match r.receipt {
                None => 0,
                Some(S::Consumed) => 1,
                Some(S::Denied) => 2,
                Some(S::Expired) => 3,
                Some(S::Revoked) => 4,
                _ => 255,
            },
        }
    }
    fn into_record(self) -> Result<Record, String> {
        let run = CliApprovalRun {
            epoch: self.epoch,
            session: self.session,
            binding_generation: self.binding_generation,
            op: self.op,
            prompt: self.prompt,
        };
        let record = Record {
            context: CliApprovalContext {
                conversation: ConversationId(self.conversation),
                generation: self.generation,
                run_op: OperationId(self.run_op),
                run: run.clone(),
            },
            id: CliApprovalId {
                run,
                invocation: self.invocation,
                arguments_sha256: self.digest,
            },
            decision: if self.allow { D::AllowOnce } else { D::Deny },
            receipt: match self.receipt {
                0 => None,
                1 => Some(S::Consumed),
                2 => Some(S::Denied),
                3 => Some(S::Expired),
                4 => Some(S::Revoked),
                _ => return Err("Invalid saved approval state.".into()),
            },
        };
        if !record.valid() {
            return Err("Invalid saved approval identity.".into());
        }
        Ok(record)
    }
}
fn load(conn: &Connection, ns: &str) -> Result<Vec<Record>, String> {
    let mut stmt=conn.prepare("SELECT
        CASE WHEN typeof(record)='text' AND length(CAST(record AS BLOB)) BETWEEN 1 AND ?2 THEN record END,
        CASE WHEN typeof(conversation)='text' AND length(CAST(conversation AS BLOB)) BETWEEN 1 AND 20 THEN conversation END,
        CASE WHEN typeof(invocation)='text' AND length(CAST(invocation AS BLOB))=64 THEN invocation END
        FROM cli_approval_recovery WHERE namespace=?1 ORDER BY conversation, invocation LIMIT ?3").map_err(sql_err)?;
    let rows = stmt
        .query_map(
            params![
                ns,
                MAX_RECORD as i64,
                (CLI_APPROVAL_CONTEXTS * CLI_APPROVAL_REQUESTS + 1) as i64
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(sql_err)?;
    let mut records = Vec::new();
    for row in rows {
        let (json, conversation, invocation) = row.map_err(sql_err)?;
        let saved: Saved = serde_json::from_str(&json)
            .map_err(|_| "Invalid saved approval record.".to_string())?;
        let record = saved.into_record()?;
        if record.context.conversation.0.to_string() != conversation
            || record.id.invocation != invocation
        {
            return Err("Saved approval key mismatch.".into());
        }
        records.push(record);
    }
    // Reuse the pure startup validator, including context/session uniqueness and bounds.
    if !Record::valid_set(&records) {
        return Err("Saved approval recovery is invalid or full.".into());
    }
    Ok(records)
}
impl Storage {
    pub fn load_cli_approvals(&self) -> Result<Vec<Record>, String> {
        let conn = Connection::open(&self.path).map_err(sql_err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(sql_err)?;
        load(&conn, &self.namespace)
    }
    pub fn journal_cli_approval(
        &self,
        record: Record,
        ack: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        if let Err(Msg::CliApproval { ack, .. }) = self.send(Msg::CliApproval {
            record: Box::new(record),
            ack: Box::new(ack),
        }) {
            ack(Err(
                "Approval recovery writer unavailable; no decision released.".into(),
            ));
        }
    }
}
pub(super) fn write(conn: &mut Connection, ns: &str, record: &Record) -> Result<(), String> {
    if !record.valid() {
        return Err("Invalid approval recovery record.".into());
    }
    let tx = conn.transaction().map_err(sql_err)?;
    let records = load(&tx, ns)?;
    let same: Vec<_> = records
        .iter()
        .filter(|r| r.context.conversation == record.context.conversation)
        .collect();
    if records.iter().any(|r| {
        r.context.conversation != record.context.conversation
            && r.context.run.session == record.context.run.session
    }) {
        return Err("Approval recovery session conflict.".into());
    }
    if let Some(old) = same.first() {
        if old.context.run != record.context.run {
            // Only fully resolved older scopes may be replaced. Never forget uncertainty.
            if same.iter().any(|r| r.receipt.is_none())
                || old.context.run.session != record.context.run.session
                || (record.context.run.epoch, record.context.run.prompt)
                    <= (old.context.run.epoch, old.context.run.prompt)
                || record.context.run_op <= old.context.run_op
            {
                return Err("Unresolved or stale approval recovery scope.".into());
            }
            tx.execute(
                "DELETE FROM cli_approval_recovery WHERE namespace=?1 AND conversation=?2",
                params![ns, record.context.conversation.0.to_string()],
            )
            .map_err(sql_err)?;
        } else {
            if same.iter().any(|r| {
                r.context.generation > record.context.generation
                    || r.context.run_op != record.context.run_op
            }) {
                return Err("Stale approval attachment.".into());
            }
            if let Some(old) = same
                .iter()
                .find(|r| r.id.invocation == record.id.invocation)
            {
                if old.id != record.id
                    || old.decision != record.decision
                    || (old.receipt.is_some()
                        && record.receipt.is_some()
                        && old.receipt != record.receipt)
                {
                    return Err("Approval recovery identity conflict.".into());
                }
                if old.receipt.is_some() && record.receipt.is_none() {
                    return Ok(());
                }
            } else if same.len() >= CLI_APPROVAL_REQUESTS {
                return Err("Approval recovery request bound reached.".into());
            }
            // Read-only reattachment changes no run/decision identity. Keep one exact context.
            for old in &same {
                if old.context != record.context {
                    let mut rebound = (*old).clone();
                    rebound.context = record.context.clone();
                    put(&tx, ns, &rebound)?;
                }
            }
        }
    } else if records
        .iter()
        .map(|r| r.context.conversation)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        >= CLI_APPROVAL_CONTEXTS
    {
        return Err("Approval recovery context bound reached.".into());
    }
    put(&tx, ns, record)?;
    tx.commit().map_err(sql_err)
}
fn put(conn: &Connection, ns: &str, record: &Record) -> Result<(), String> {
    let json = serde_json::to_string(&Saved::from_record(record))
        .map_err(|_| "Could not encode approval recovery.".to_string())?;
    if json.len() > MAX_RECORD {
        return Err("Approval recovery record exceeds bound.".into());
    }
    conn.execute("INSERT INTO cli_approval_recovery(namespace,conversation,invocation,record) VALUES(?1,?2,?3,?4)
        ON CONFLICT(namespace,conversation,invocation) DO UPDATE SET record=excluded.record",params![ns,record.context.conversation.0.to_string(),record.id.invocation,json]).map_err(sql_err)?;
    Ok(())
}
