//! Sealed completion data, never permission to reconstruct a connected continuation owner.
use super::*;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WakeTarget {
    Child(String),
    Job(u64),
}
impl WakeTarget {
    pub(crate) fn tool(&self) -> &'static str {
        match self {
            Self::Child(_) => "subagent",
            Self::Job(_) => "job_start",
        }
    }
}
#[derive(Clone)]
pub(crate) struct Wake {
    target: WakeTarget,
    parent: String,
    call: String,
    run: u64,
    request: String,
    text: String,
    digest: [u8; 32],
}
impl Wake {
    pub(crate) fn capture(
        c: &Connection,
        target: WakeTarget,
        parent: &str,
        call: &str,
        run: u64,
        text: String,
    ) -> Result<Self> {
        let request = match &target {
            WakeTarget::Child(child) => {
                let task =
                    Store::task_by_session(c, child)?.ok_or(StoreError::ExecutionConflict)?;
                if task.parent_call == call {
                    format!("cli-child-wake:{child}")
                } else {
                    format!("cli-child-wake:{child}:{call}")
                }
            }
            WakeTarget::Job(job) => format!("cli-job-wake:{job}"),
        };
        let mut value = Self {
            target,
            parent: parent.into(),
            call: call.into(),
            run,
            request,
            text,
            digest: [0; 32],
        };
        value.digest = value.current_digest(c)?;
        value.check(c)?;
        Ok(value)
    }
    pub(crate) fn origin(&self) -> (&str, u64) {
        (&self.parent, self.run)
    }
    pub(crate) fn target(&self) -> &WakeTarget {
        &self.target
    }
    pub(crate) fn request(&self) -> &str {
        &self.request
    }
    fn current_digest(&self, c: &Connection) -> Result<[u8; 32]> {
        let data = match &self.target {
            WakeTarget::Child(child) => {
                let t = Store::task_by_session(c, child)?.ok_or(StoreError::ExecutionConflict)?;
                if !t.finished() || t.delivery != "wake" || t.parent_session != self.parent {
                    return Err(StoreError::ExecutionConflict);
                }
                let joined:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_child_policies p JOIN cli_agent_turns r ON r.session=p.child WHERE p.child=?1 AND p.parent=?2 AND r.request_key='child:' || ?3 AND r.run=(SELECT MAX(run) FROM cli_agent_turns WHERE session=p.child))",params![child,self.parent,self.call],|r|r.get(0))?;
                if !joined {
                    return Err(StoreError::ExecutionConflict);
                }
                serde_json::json!([
                    t.child_session,
                    t.parent_session,
                    t.parent_call,
                    t.child,
                    t.task,
                    t.status,
                    t.result,
                    t.note,
                    t.updated_at_ms
                ])
            }
            WakeTarget::Job(id) => {
                let j = Store::job(c, &self.parent, *id)?.ok_or(StoreError::ExecutionConflict)?;
                if j.active()
                    || j.delivery != "pending"
                    || !j.wake
                    || j.origin != "model"
                    || j.call_id.as_deref() != Some(self.call.as_str())
                    || j.run != Some(self.run)
                    || j.pid.is_none()
                    || j.started_at.is_none()
                    || j.ended_at.is_none()
                    || matches!(j.state.as_str(), "unknown" | "interrupted")
                {
                    return Err(StoreError::ExecutionConflict);
                }
                serde_json::json!([
                    j.session,
                    j.id,
                    j.call_id,
                    j.run,
                    j.engine_epoch,
                    j.nonce,
                    j.pid,
                    j.started_at,
                    j.ended_at,
                    j.state,
                    j.exit_code,
                    j.reason,
                    j.name,
                    j.command,
                    j.cwd,
                    j.log_bytes,
                    j.log_truncated
                ])
            }
        };
        Ok(Sha256::digest(serde_json::to_vec(&data).expect("wake digest")).into())
    }
    fn check(&self, c: &Connection) -> Result<()> {
        if self.text.trim().is_empty()
            || self.text.len() > MAX_PROMPT_BYTES
            || self.text.contains('\0')
            || self.current_digest(c)? != self.digest
        {
            return Err(StoreError::ExecutionConflict);
        }
        let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_bridge_calls bc
            JOIN cli_tool_bridges b ON b.id=bc.bridge AND b.session=bc.session
            JOIN tool_calls n ON n.session=bc.session AND n.call_id=bc.call_id AND n.run=b.run AND n.name=bc.name
            JOIN cli_agent_turns r ON r.session=bc.session AND r.run=b.run
            JOIN runs u ON u.id=r.run
            WHERE bc.session=?1 AND bc.call_id=?2 AND b.run=?3 AND bc.name=?4
            AND bc.state='completed' AND json_extract(bc.result,'$.isError')=0 AND n.state='done'
            AND r.state='completed' AND r.fenced=0 AND u.stop_requested_at IS NULL)",
            params![self.parent,self.call,self.run as i64,self.target.tool()],|r|r.get(0))?;
        if !valid {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(())
    }
    pub(crate) fn validate_in(&self, c: &Connection, input: &Input<'_>) -> Result<()> {
        if input.session != self.parent
            || input.request != self.request
            || input.text != self.text
            || input.mode != InputMode::FollowUp
        {
            return Err(StoreError::ExecutionConflict);
        }
        self.check(c)
    }
    pub(crate) fn deliver(&self, c: &Connection, now: i64) -> Result<()> {
        match &self.target {
            WakeTarget::Child(child) => Store::set_task_delivery(c, child, "woken", now),
            WakeTarget::Job(id) => {
                if !Store::job_set_delivery(c, &self.parent, *id, "woken", now)? {
                    return Err(StoreError::ExecutionConflict);
                }
                Ok(())
            }
        }
    }
}
