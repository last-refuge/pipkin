//! Atomic child metadata under an authenticated native subagent intent. This creates no
//! launch/return grant. Runtime must additionally hold original parent service authority,
//! worker liveness and the publication gate, and track admission through cleanup.
use super::*;
use crate::execution::claude_cli::orchestration_child::{Delivery, Frozen};

pub(crate) struct Created {
    pub binding: cli_agent::Binding,
    pub task: TaskRow,
    pub created: bool,
    /// Only first creation produces a publication. Duplicate metadata lookup produces none.
    pub revision: Option<u64>,
}
/// Captured continuation intent; metadata is not an in-memory owner grant.
pub(crate) struct Continuation {
    pub lease: std::sync::Arc<cli_bridge::Lease>,
    pub call: String,
    pub child: String,
    pub text: String,
    pub delivery: String,
    pub digest: [u8; 32],
}
impl Continuation {
    pub(crate) fn validate(&self, c: &Connection) -> Result<()> {
        let task = Store::task_by_session(c, &self.child)?.ok_or(StoreError::ExecutionConflict)?;
        let policy = Store::cli_native_child_policy_in(c, &self.child)?
            .ok_or(StoreError::ExecutionConflict)?;
        let call: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM cli_bridge_calls b JOIN cli_tool_bridges l ON l.id=b.bridge JOIN tool_calls t ON t.session=b.session AND t.call_id=b.call_id AND t.run=l.run JOIN runs r ON r.id=l.run WHERE b.session=?1 AND b.call_id=?2 AND l.run=?3 AND b.name='subagent' AND t.name=b.name AND r.stop_requested_at IS NULL AND ((b.state='intent' AND t.state='intent') OR (b.state='completed' AND t.state='done' AND json_extract(b.result,'$.isError')=0)))", params![self.lease.scope().session,self.call,self.lease.scope().run],|r|r.get(0))?;
        let duplicate: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM cli_agent_turns WHERE session=?1 AND request_key=?2)",
            params![self.child, format!("child:{}", self.call)],
            |r| r.get(0),
        )?;
        if !call
            || duplicate
            || task.parent_session != self.lease.scope().session
            || task.status != "done"
            || matches!(task.delivery.as_str(), "wake" | "notify")
            || policy.0 != self.digest
        {
            return Err(StoreError::ExecutionConflict);
        }
        let (active, _) = Store::child_counts(c, &task.root)?;
        let snapshot: Value =
            serde_json::from_str(&policy.1).map_err(|_| StoreError::ExecutionConflict)?;
        let cap = snapshot["cliNative"]["maxConcurrent"]
            .as_u64()
            .ok_or(StoreError::ExecutionConflict)?;
        if active as u64 >= cap {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(())
    }
}
impl Store {
    /// A model-authored verdict alone is not an independent workspace inspection.
    pub(crate) fn assert_cli_auditor_inspected_in(
        c: &Connection,
        child: &str,
        run: u64,
    ) -> Result<()> {
        let auditor:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_child_policies WHERE child=?1 AND json_extract(snapshot,'$.cliNative.service')='propose_goal_completion')",[child],|r|r.get(0))?;
        if auditor {
            let inspected:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_bridge_calls b JOIN cli_tool_bridges l ON l.id=b.bridge JOIN tool_calls t ON t.session=b.session AND t.call_id=b.call_id AND t.run=l.run WHERE b.session=?1 AND l.run=?2 AND b.name IN ('read','list_files','search_files','plan_read') AND t.name=b.name AND t.state='done' AND b.state='completed' AND json_extract(b.result,'$.isError')=0)",params![child,run],|r|r.get(0))?;
            if !inspected {
                return Err(StoreError::ExecutionConflict);
            }
        }
        Ok(())
    }

    /// Bounded, consistent snapshot under the authenticated parent call. No partial
    /// materialization or tool/thinking payload loading is needed for inherited text.
    pub(crate) fn cli_child_context_in(
        c: &Connection,
        lease: &cli_bridge::Lease,
        call: &str,
    ) -> Result<(Vec<Entry>, u64)> {
        Self::assert_cli_native_service_in(c, lease, call, "subagent")?;
        Self::cli_child_context_at(c, &lease.scope().session)
    }
    pub(crate) fn cli_child_context_at(c: &Connection, parent: &str) -> Result<(Vec<Entry>, u64)> {
        let revision = c.query_row("SELECT revision FROM sessions WHERE id=?1", [parent], |r| {
            r.get(0)
        })?;
        let mut stmt =
            c.prepare("SELECT * FROM entries WHERE session=?1 ORDER BY id DESC LIMIT 50")?;
        // Check size before allocating any body. Oversize snapshots fail, never silently widen.
        let large: bool = c.query_row("SELECT COALESCE(SUM(n),0)>2097152 FROM (SELECT length(CAST(body AS BLOB)) n FROM entries WHERE session=?1 ORDER BY id DESC LIMIT 50)", [parent], |r| r.get(0))?;
        if large {
            return Err(StoreError::ExecutionConflict);
        }
        let mut entries = stmt
            .query_map([parent], Self::entry_from_row)?
            .map(|r| r.map(|(e, _)| e))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        entries.reverse();
        Ok((entries, revision))
    }
    /// Every inherited child turn must read the frozen native snapshot before effects
    /// and before it can be considered completed. SDK text/EOF cannot satisfy this.
    pub(crate) fn assert_cli_child_context_read_in(
        c: &Connection,
        child: &str,
        run: u64,
    ) -> Result<()> {
        let required: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_child_policies WHERE child=?1 AND json_extract(snapshot,'$.context')='inherit')", [child], |r|r.get(0))?;
        if required {
            let read: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM cli_bridge_calls b JOIN cli_tool_bridges l ON l.id=b.bridge JOIN tool_calls t ON t.session=b.session AND t.call_id=b.call_id AND t.run=l.run WHERE b.session=?1 AND l.run=?2 AND b.name='parent_context' AND t.name=b.name AND t.state='done' AND b.state='completed' AND json_extract(b.result,'$.isError')=0)", params![child,run], |r|r.get(0))?;
            if !read {
                return Err(StoreError::ExecutionConflict);
            }
        }
        Ok(())
    }
    /// Only an actual successful native tool receipt makes background delegation survive
    /// cancellation of the parent turn. This is cleanup classification, never new authority.
    #[cfg(test)]
    pub(crate) fn cli_native_child_handed_off(&self, child: &str) -> Result<bool> {
        self.read(|c| Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_child_policies p
            JOIN subagent_tasks t ON t.child_session=p.child AND t.parent_session=p.parent AND t.parent_call=p.parent_call
            JOIN cli_bridge_calls bc ON bc.session=p.parent AND bc.call_id=p.parent_call AND bc.name IN ('subagent','propose_goal_completion')
            JOIN cli_tool_bridges b ON b.id=bc.bridge AND b.session=p.parent
            JOIN tool_calls n ON n.session=p.parent AND n.call_id=p.parent_call AND n.run=b.run AND n.name=bc.name
            WHERE p.child=?1 AND bc.state='completed' AND json_extract(bc.result,'$.isError')=0 AND n.state='done')",
            [child], |r|r.get(0))?))
    }
    pub(crate) fn cli_native_child_call_handed_off(
        &self,
        parent: &str,
        call: &str,
    ) -> Result<bool> {
        self.read(|c| Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM cli_bridge_calls b JOIN cli_tool_bridges l ON l.id=b.bridge JOIN tool_calls t ON t.session=b.session AND t.call_id=b.call_id AND t.run=l.run WHERE b.session=?1 AND b.call_id=?2 AND b.name='subagent' AND t.name=b.name AND b.state='completed' AND json_extract(b.result,'$.isError')=0 AND t.state='done')", params![parent,call], |r|r.get(0))?))
    }
    /// Record a joined child's result, or classify an orphan at startup. This only reads
    /// durable evidence; it never reconstructs a launch/return grant. The caller must have
    /// joined the original root actor, or be in startup recovery before new work is allowed.
    pub(crate) fn settle_cli_native_child_in(
        c: &Connection,
        child: &str,
        now: i64,
    ) -> Result<(bool, String, String, u64)> {
        let task = Self::task_by_session(c, child)?.ok_or(StoreError::ExecutionConflict)?;
        let joined: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_child_policies p
            JOIN cli_agent_bindings a ON a.session=p.child
            JOIN sessions s ON s.id=a.session AND s.parent=p.parent AND s.root=p.parent
            JOIN cli_bridge_calls bc ON bc.session=p.parent AND bc.call_id=p.parent_call AND bc.name IN ('subagent','propose_goal_completion')
            JOIN cli_tool_bridges b ON b.id=bc.bridge AND b.session=p.parent
            JOIN tool_calls t ON t.session=p.parent AND t.call_id=p.parent_call AND t.run=b.run AND t.name=bc.name
            WHERE p.child=?1 AND p.parent=?2 AND p.parent_call=?3 AND s.execution_backend='claudeCliAgent')",
            params![child,task.parent_session,task.parent_call], |r|r.get(0))?;
        let person: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_child_policies p JOIN subagent_tasks t ON t.child_session=p.child AND t.parent_session=p.parent AND t.parent_call=p.parent_call JOIN sessions s ON s.id=p.child AND s.parent=p.parent AND s.root=p.parent WHERE p.child=?1 AND t.origin='user' AND json_extract(p.snapshot,'$.cliNative.origin')='user' AND json_extract(p.snapshot,'$.cliNative.parent.run')=0 AND s.execution_backend='claudeCliAgent')", [child], |r|r.get(0))?;
        if !joined && !person {
            return Err(StoreError::ExecutionConflict);
        }
        let run: Option<i64> = c.query_row("SELECT CASE WHEN state='completed' AND fenced=0 THEN run ELSE NULL END FROM cli_agent_turns WHERE session=?1 ORDER BY run DESC LIMIT 1", [child], |r|r.get(0)).optional()?.flatten();
        let confirmed = run.is_some();
        let answer = if let Some(run) = run {
            let mut statement=c.prepare("SELECT * FROM entries WHERE session=?1 AND run=?2 AND retired=0 AND partial=0 AND length(CAST(body AS BLOB))<=8388608 ORDER BY id DESC LIMIT 128")?;
            let mut answer = None;
            for row in statement.query_map(params![child, run], Self::entry_from_row)? {
                let (entry, _) = row?;
                if let EntryKind::Message {
                    message: m @ Message::Assistant { .. },
                    partial: false,
                    ..
                } = entry.kind
                {
                    let text = m.text();
                    if !text.is_empty() {
                        answer = Some(text);
                        break;
                    }
                }
            }
            answer.unwrap_or_else(|| "Child completed without a textual response.".into())
        } else {
            "CLI child did not finish with a confirmed result; inspect its retained effects and do not replay.".into()
        };
        Self::set_task_status(
            c,
            child,
            if confirmed { "done" } else { "failed" },
            Some(&answer),
            now,
        )?;
        if !confirmed {
            // Existing result views reserve `result` for successful answers. Keep the
            // uncertainty warning visible in their note, without replacing a person's note.
            Self::set_task_note(c, child, &answer, now)?;
        }
        if task.delivery == "notify" {
            Self::set_task_delivery(c, child, "notified", now)?;
        }
        let revision = Self::bump(c, &task.parent_session, now)?;
        Ok((confirmed, answer, task.parent_session, revision))
    }
    /// Startup-only cleanup. Metadata without an authenticated native parent call is not
    /// adopted. Finished rows with delivered results are left untouched; no inference runs.
    pub(crate) fn recover_cli_native_children(&self, now: i64) -> Result<usize> {
        self.tx(|c| {
            let children=c.prepare("SELECT p.child FROM cli_agent_child_policies p
                JOIN subagent_tasks t ON t.child_session=p.child AND t.parent_session=p.parent AND t.parent_call=p.parent_call
                JOIN cli_bridge_calls bc ON bc.session=p.parent AND bc.call_id=p.parent_call AND bc.name IN ('subagent','propose_goal_completion')
                JOIN cli_tool_bridges b ON b.id=bc.bridge AND b.session=p.parent
                JOIN tool_calls n ON n.session=p.parent AND n.call_id=p.parent_call AND n.run=b.run AND n.name=bc.name
                WHERE t.status NOT IN ('done','failed','aborted') OR t.delivery IN ('notify','wake')
                UNION SELECT p.child FROM cli_agent_child_policies p JOIN subagent_tasks t ON t.child_session=p.child AND t.parent_session=p.parent AND t.parent_call=p.parent_call WHERE t.origin='user' AND json_extract(p.snapshot,'$.cliNative.origin')='user' AND (t.status NOT IN ('done','failed','aborted') OR t.delivery IN ('notify','wake'))")?
                .query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            for child in &children {
                c.execute("UPDATE subagent_tasks SET delivery='suppressed' WHERE child_session=?1 AND delivery='wake'",[child])?;
                Self::settle_cli_native_child_in(c,child,now)?;
            }
            Ok(children.len())
        })
    }
    /// Snapshot data only; not an executable owner/return grant. Not used by task-list views.
    pub(crate) fn cli_native_child_policy_in(
        c: &Connection,
        child: &str,
    ) -> Result<Option<([u8; 32], String)>> {
        use sha2::{Digest, Sha256};
        let row = c
            .query_row(
                "SELECT digest,CASE WHEN length(CAST(snapshot AS BLOB))<=2097152 THEN snapshot ELSE NULL END FROM cli_agent_child_policies WHERE child=?1",
                [child],
                |r| Ok((r.get::<_, [u8; 32]>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((digest, snapshot)) = &row {
            let actual: [u8; 32] = Sha256::digest(snapshot.as_bytes()).into();
            if actual != *digest {
                return Err(StoreError::ExecutionConflict);
            }
        }
        Ok(row)
    }
    /// `child` and `uuid` are engine-minted proposals, never SDK/model-supplied authority.
    /// A duplicate call returns metadata only, ignoring unused identity proposals.
    /// The caller must propagate errors to roll back this transaction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_cli_native_child_in(
        c: &rusqlite::Transaction<'_>,
        lease: &cli_bridge::Lease,
        policy: &Frozen,
        child: &str,
        uuid: &str,
        now: i64,
    ) -> Result<Created> {
        Self::assert_cli_native_service_in(c, lease, policy.call(), policy.service())?;
        let parent = &lease.scope().session;
        if let Some(goal) = policy.audit()
            && Self::goal_current(c, parent)?.as_ref() != Some(goal)
        {
            return Err(StoreError::ExecutionConflict);
        }
        Self::create_cli_child_in(c, lease.scope(), policy, child, uuid, "model", now)
    }
    /// Explicit person intent, separately authorized by the connected root registry.
    pub(crate) fn create_cli_person_child_in(
        c: &rusqlite::Transaction<'_>,
        scope: &crate::execution::claude_cli::orchestration_protocol::Scope,
        policy: &Frozen,
        child: &str,
        uuid: &str,
        now: i64,
    ) -> Result<Created> {
        if scope.run != 0 || policy.audit().is_some() || policy.delivery() != Delivery::Notify {
            return Err(StoreError::ExecutionConflict);
        }
        Self::assert_session_live(c, &scope.session)?;
        Self::create_cli_child_in(c, scope, policy, child, uuid, "user", now)
    }
    #[allow(clippy::too_many_arguments)]
    fn create_cli_child_in(
        c: &rusqlite::Transaction<'_>,
        scope: &crate::execution::claude_cli::orchestration_protocol::Scope,
        policy: &Frozen,
        child: &str,
        uuid: &str,
        origin: &str,
        now: i64,
    ) -> Result<Created> {
        let parent = &scope.session;
        let enabled:bool=c.query_row("SELECT subagents_enabled=1 AND parent IS NULL AND root IS NULL AND blocked IS NULL FROM sessions WHERE id=?1",[parent],|r|r.get(0))?;
        if !enabled {
            return Err(StoreError::ExecutionConflict);
        }
        let binding =
            Self::cli_agent_binding_in(c, parent)?.ok_or(StoreError::ExecutionConflict)?;
        if !policy.matches(scope, &binding, policy.tools()) {
            return Err(StoreError::ExecutionConflict);
        }
        let task = policy.task();
        let delivery = match policy.delivery() {
            Delivery::Inline => "inline",
            Delivery::Notify => "notify",
            Delivery::Wake => "wake",
        };
        let key = policy.create_key();
        let launch = policy.record().to_json();
        if let Some(row) = Self::task_for_call(c, parent, policy.call())? {
            let b = Self::cli_agent_binding_in(c, &row.child_session)?
                .ok_or(StoreError::ExecutionConflict)?;
            let mut identity = binding.create.identity.clone();
            identity.cli_session_id = b.create.identity.cli_session_id.clone();
            let stored = Self::cli_native_child_policy_in(c, &row.child_session)?
                .ok_or(StoreError::ExecutionConflict)?;
            if origin == "user" && row.origin == "user" {
                let old: Value =
                    serde_json::from_str(&stored.1).map_err(|_| StoreError::ExecutionConflict)?;
                let new: Value = serde_json::from_str(policy.snapshot())
                    .map_err(|_| StoreError::ExecutionConflict)?;
                if old["cliNative"]["personIntent"] != new["cliNative"]["personIntent"] {
                    return Err(StoreError::ExecutionConflict);
                }
                return Ok(Created {
                    binding: b,
                    task: row,
                    created: false,
                    revision: None,
                });
            }
            if stored.0 != policy.digest()
                || stored.1 != policy.snapshot()
                || row.launch.as_deref() != Some(launch.as_str())
                || row.root != *parent
                || row.task != task
                || row.delivery != delivery
                || row.origin != origin
                || b.create.request_key != key
                || b.create.parent.as_deref() != Some(parent)
                || b.root.as_deref() != Some(parent)
                || b.create.model != policy.model().as_str()
                || b.create.thinking != policy.record().thinking
                || b.create.identity != identity
            {
                return Err(StoreError::ExecutionConflict);
            }
            return Ok(Created {
                binding: b,
                task: row,
                created: false,
                revision: None,
            });
        }
        let caps = policy.caps();
        let (live, total) = Self::child_counts(c, parent)?;
        if live >= caps.max_concurrent || total >= caps.max_total {
            return Err(StoreError::ExecutionConflict);
        }
        let mut identity = binding.create.identity;
        identity.cli_session_id = uuid.into();
        let create = cli_agent::Create {
            session: child.into(),
            request_key: key,
            identity,
            model: policy.model().as_str().into(),
            thinking: policy.record().thinking.clone(),
            parent: Some(parent.clone()),
        };
        let (binding, created) = Self::create_cli_agent_session_in(c, &create, now)?;
        // A detached session receipt never grants permission to attach it to a new task.
        if !created {
            return Err(StoreError::ExecutionConflict);
        }
        c.execute(
            "UPDATE sessions SET subagents_enabled=0 WHERE id=?1",
            [child],
        )?;
        let (_, created) = Self::create_task_with(
            c,
            parent,
            parent,
            policy.call(),
            child,
            task,
            origin,
            delivery,
            now,
        )?;
        if !created {
            return Err(StoreError::ExecutionConflict);
        }
        Self::set_task_launch(c, child, &launch)?;
        c.execute("INSERT INTO cli_agent_child_policies(child,parent,parent_call,digest,snapshot) VALUES(?1,?2,?3,?4,?5)",
            params![child,parent,policy.call(),policy.digest(),policy.snapshot()])?;
        let task = Self::task_by_session(c, child)?.ok_or(StoreError::ExecutionConflict)?;
        let revision = Self::bump(c, parent, now)?;
        Ok(Created {
            binding,
            task,
            created: true,
            revision: Some(revision),
        })
    }
}
#[cfg(test)]
pub(crate) fn remove_schema_for_test(c: &Connection) -> Result<()> {
    c.execute_batch(
        "DROP TABLE IF EXISTS cli_agent_input_deliveries; DROP TABLE IF EXISTS cli_agent_inputs; DROP TABLE IF EXISTS cli_agent_context;",
    )?;
    c.execute_batch(
        "DROP TRIGGER cli_agent_child_policy_retained; DROP TABLE cli_agent_child_policies;",
    )?;
    Ok(())
}
#[cfg(test)]
mod tests;
