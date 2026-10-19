//! Inactive root admission/launch journal. No process launch, preflight approval or retry.
//! Only a new committed admission mints a nonserializable owner lease. Commit launch intent
//! before spawning; uncertain launch/effects permanently prohibit resume on this binding.
use super::*;
use crate::execution::claude_cli::orchestration_input as input_data;
mod goals;
pub(crate) mod inputs;
pub(crate) mod projection;
mod wake;
use crate::execution::claude_cli::{
    orchestration_policy::MAX_PROMPT_BYTES, orchestration_protocol::Scope, policy::TurnMode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub(crate) use wake::{Wake, WakeTarget};

pub(crate) struct GoalTurn {
    pub view: GoalView,
}
pub struct Input<'a> {
    pub mode: InputMode,
    pub session: &'a str,
    pub request: &'a str,
    pub generation: u64,
    pub operation: &'a str,
    pub binding: [u8; 32],
    pub text: &'a str,
    pub images: &'a [ImageInput],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Admitted,
    LaunchIntent,
    Running,
    Completed,
    Rejected,
    Unknown,
}
impl State {
    fn active(self) -> bool {
        matches!(self, Self::Admitted | Self::LaunchIntent | Self::Running)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub scope: Scope,
    pub request: String,
    pub user_entry: u64,
    pub mode: TurnMode,
    pub state: State,
    pub bridge: Option<String>,
    pub fenced: bool,
}
/// Immutable stored launch inputs. No constructor/serialization or mutable fields are exposed.
/// This snapshot is NOT execution authority: the owner must still commit launch intent once.
pub struct LaunchContext {
    binding: cli_agent::Binding,
    receipt: Receipt,
    prompt: String,
    images: Vec<ImageInput>,
    compact: bool,
}
impl LaunchContext {
    pub fn binding(&self) -> &cli_agent::Binding {
        &self.binding
    }
    pub fn scope(&self) -> &Scope {
        &self.receipt.scope
    }
    pub fn mode(&self) -> TurnMode {
        self.receipt.mode
    }
    pub(crate) fn compact(&self) -> bool {
        self.compact
    }
    pub fn images(&self) -> &[ImageInput] {
        &self.images
    }
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
}
impl std::fmt::Debug for LaunchContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CliAgentLaunchContext([redacted])")
    }
}
/// Owner token never comes from a model or reconnecting client. This is not a preflight grant.
pub struct Lease {
    receipt: Receipt,
    owner: String,
    binding: [u8; 32],
}
impl Lease {
    pub(crate) fn matches_owner_binding(&self, owner: &str, binding: &[u8; 32]) -> bool {
        self.owner == owner && self.binding == *binding
    }
    pub fn receipt(&self) -> &Receipt {
        &self.receipt
    }
}
impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CliAgentTurnLease([redacted])")
    }
}
#[derive(Debug)]
pub struct Admission {
    pub receipt: Receipt,
    pub lease: Option<Lease>,
    /// Actual parent task-view revision committed atomically with first child admission.
    pub parent_task_revision: Option<(String, u64)>,
    pub job_revision: Option<(String, u64)>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settlement {
    Committed,
    AlreadyCommitted,
    Conflict,
}
/// Supervisor-only facts; model text/EOF/MCP disconnect cannot supply these facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completion {
    pub cli_session_id: String,
    pub terminal: ExternalTerminal,
    pub supervisor: ExternalSettlementEvidence,
}
impl Completion {
    fn clean(&self) -> bool {
        let e = &self.supervisor;
        self.terminal.outcome == ExternalTerminalOutcome::Success
            && e.input_delivered
            && e.root_reaped
            && e.pipes_drained
            && e.projection_finished
            && e.exit_code == Some(0)
            && !e.stop_requested
            && !e.cancelled_clean_confirmed
            && !e.cleanup_timed_out
            && e.supervisor_stop.is_none()
            && e.projection_error.is_none()
            && e.rejected_record.is_none()
            && e.parse_failure.is_none()
    }
}
fn stored(c: &Connection, session: &str, request: &str) -> Result<Option<Receipt>> {
    Ok(c.query_row(
        "SELECT * FROM cli_agent_turns WHERE session=?1 AND request_key=?2",
        params![session, request],
        |r| {
            let state: String = r.get("state")?;
            Ok(Receipt {
                scope: Scope {
                    session: r.get("session")?,
                    generation: r.get("generation")?,
                    operation: r.get("operation")?,
                    run: r.get("run")?,
                },
                request: r.get("request_key")?,
                user_entry: r.get("user_entry")?,
                mode: if r.get::<_, String>("mode")? == "fresh" {
                    TurnMode::Fresh
                } else {
                    TurnMode::Resume
                },
                state: serde_json::from_value(Value::String(state))
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                bridge: r.get("bridge")?,
                fenced: r.get("fenced")?,
            })
        },
    )
    .optional()?)
}
fn live(c: &Connection, session: &str, binding: [u8; 32]) -> Result<cli_agent::Binding> {
    Store::assert_session_live(c, session)?;
    let b = Store::cli_agent_binding_in(c, session)?.ok_or(StoreError::UnsupportedExecution)?;
    let allowed:bool=c.query_row("SELECT blocked IS NULL AND NOT EXISTS(SELECT 1 FROM sessions a WHERE a.id IN(s.parent,s.root) AND (a.execution_backend<>'claudeCliAgent' OR a.blocked IS NOT NULL OR a.subagents_enabled=0)) FROM sessions s WHERE id=?1",[session],|r|r.get(0))?;
    if !allowed || b.digest != binding {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(b)
}
fn owned(c: &Connection, lease: &Lease) -> Result<Receipt> {
    let r = stored(c, &lease.receipt.scope.session, &lease.receipt.request)?
        .ok_or(StoreError::ExecutionConflict)?;
    let valid:bool=c.query_row("SELECT owner=?3 AND binding_hash=?4 FROM cli_agent_turns WHERE session=?1 AND request_key=?2",params![r.scope.session,r.request,lease.owner,lease.binding],|r|r.get(0))?;
    if !valid || r.scope != lease.receipt.scope {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(r)
}
fn running(c: &Connection, r: &Receipt) -> Result<()> {
    let ok:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND session=?2 AND request=?3 AND state='running' AND stop_requested_at IS NULL)",params![r.scope.run,r.scope.session,r.request],|row|row.get(0))?;
    if !ok {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}
fn retire(c: &Connection, r: &Receipt, now: i64) -> Result<()> {
    if !r.state.active() {
        return Ok(());
    }
    let bridges = c
        .prepare("SELECT id FROM cli_tool_bridges WHERE run=?1")?
        .query_map([r.scope.run], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for bridge in bridges {
        cli_bridge::fence(c, &bridge, now)?;
    }
    let (state, outcome) = if r.state == State::Admitted {
        ("rejected", RunOutcome::Aborted)
    } else {
        (
            "unknown",
            RunOutcome::Interrupted {
                detail: "Native CLI root outcome is unconfirmed; do not replay".into(),
            },
        )
    };
    c.execute("UPDATE cli_agent_turns SET state=?3,updated_at=MAX(updated_at,?4) WHERE session=?1 AND request_key=?2",params![r.scope.session,r.request,state,now])?;
    Store::settle_run(c, r.scope.run, &outcome, now)?;
    Store::settle_request(c, &r.scope.session, &r.request, &outcome, now)?;
    inputs::settle(c, r, &outcome, now)?;
    Store::retire_entries(c, &r.scope.session, &[r.user_entry])?;
    // Keep known native tool effects; retire only this root's display observations.
    c.execute("UPDATE entries SET retired=1 WHERE session=?1 AND run=?2 AND json_extract(body,'$.message.api')='claude-cli-native'",params![r.scope.session,r.scope.run])?;
    Store::bump(c, &r.scope.session, now)?;
    Ok(())
}
fn poison(c: &Connection, r: &Receipt, now: i64) -> Result<()> {
    c.execute("UPDATE cli_agent_turns SET fenced=1,updated_at=MAX(updated_at,?3) WHERE session=?1 AND request_key=?2",params![r.scope.session,r.request,now])?;
    let active=c.prepare("SELECT request_key FROM cli_agent_turns WHERE session=?1 AND state IN ('admitted','launchIntent','running')")?.query_map([&r.scope.session],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for k in active {
        retire(
            c,
            &stored(c, &r.scope.session, &k)?.ok_or(StoreError::ExecutionConflict)?,
            now,
        )?;
    }
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
        [&r.scope.session],
        |r| r.get(0),
    )?;
    if exists {
        Store::bump(c, &r.scope.session, now)?;
    }
    Ok(())
}
/// Removal is a permanent fence even if payload cleanup subsequently fails.
pub(super) fn retire_group(c: &Connection, root: &str, now: i64) -> Result<()> {
    let rows=c.prepare("SELECT t.session,t.request_key FROM cli_agent_turns t JOIN sessions s ON s.id=t.session WHERE (s.id=?1 OR s.root=?1) AND t.state IN ('admitted','launchIntent','running')")?
        .query_map([root],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (s, k) in rows {
        retire(
            c,
            &stored(c, &s, &k)?.ok_or(StoreError::ExecutionConflict)?,
            now,
        )?;
    }
    Ok(())
}
/// Existing protocol fixtures have no root row. Once a root is journaled, native dispatch
/// must use its committed launch and exact bridge, never its merely-admitted state.
pub(super) fn assert_dispatch(c: &Connection, scope: &Scope, bridge: &str) -> Result<()> {
    let valid:bool=c.query_row("SELECT NOT EXISTS(SELECT 1 FROM cli_agent_turns WHERE run=?1 AND (session<>?2 OR generation<>?3 OR operation<>?4 OR state NOT IN ('launchIntent','running') OR bridge IS NOT ?5))",params![scope.run,scope.session,scope.generation,scope.operation,bridge],|r|r.get(0))?;
    if !valid {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}
pub(super) fn assert_effects_allowed(c: &Connection, scope: &Scope) -> Result<()> {
    let compact:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_turns t JOIN cli_agent_inputs i ON i.session=t.session AND i.request_key=t.request_key WHERE t.run=?1 AND i.kind='compact')",[scope.run],|r|r.get(0))?;
    if compact {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}

fn input_digest(input: &Input<'_>) -> [u8; 32] {
    let mut identity = serde_json::json!([
        match input.mode {
            InputMode::Prompt => "pipkin-cli-agent-turn-v1",
            InputMode::FollowUp => "pipkin-cli-agent-followup-v1",
            InputMode::Steer => "pipkin-cli-agent-steer-v1",
        },
        input.session,
        input.request,
        input.generation,
        input.operation,
        input.binding,
        input.text
    ]);
    if !input.images.is_empty() {
        identity
            .as_array_mut()
            .expect("array")
            .push(serde_json::json!(input.images));
    }
    Sha256::digest(serde_json::to_vec(&identity).expect("root fingerprint")).into()
}
/// The generic history reader tolerates malformed legacy image JSON as an empty list.
/// An executable input cannot use that fallback: validate the bounded stored value first.
fn strict_request(c: &Connection, session: &str, key: &str) -> Result<Option<RequestRow>> {
    let raw: Option<Option<String>> = c.query_row(
        "SELECT CASE WHEN length(CAST(images AS BLOB))<=?3 THEN images ELSE NULL END FROM requests WHERE session=?1 AND key=?2",
        params![session, key, pipkin_engine_protocol::limits::MAX_FRAME_BYTES as i64],
        |r| r.get::<_, Option<String>>(0),
    ).optional()?;
    match raw {
        Some(Some(raw)) => {
            serde_json::from_str::<Vec<ImageInput>>(&raw)
                .map_err(|_| StoreError::ExecutionConflict)?;
        }
        Some(None) => return Err(StoreError::ExecutionConflict),
        None => return Ok(None),
    }
    Store::request_in(c, session, key)
}
fn launch_context(c: &Connection, lease: &Lease) -> Result<LaunchContext> {
    let r = owned(c, lease)?;
    if r.state != State::Admitted || r.fenced {
        return Err(StoreError::ExecutionConflict);
    }
    let b = live(c, &r.scope.session, lease.binding)?;
    running(c, &r)?;
    let bounded: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM requests q JOIN entries e ON e.session=q.session
        WHERE q.session=?1 AND q.key=?2 AND e.id=?3 AND length(CAST(q.text AS BLOB))<=?4
        AND length(CAST(q.images AS BLOB))<=?7 AND length(CAST(e.body AS BLOB))<=?5
        AND e.partial=0 AND q.model=?6 AND length(CAST(q.fingerprint AS BLOB))=64
        AND q.outcome IS NULL AND (q.queue_entry IS NULL OR EXISTS(SELECT 1 FROM cli_agent_inputs i WHERE i.session=q.session AND i.request_key=q.key)))",
        params![
            r.scope.session,
            r.request,
            r.user_entry,
            MAX_PROMPT_BYTES as i64,
            pipkin_engine_protocol::limits::MAX_FRAME_BYTES as i64,
            b.create.model,
            pipkin_engine_protocol::limits::MAX_FRAME_BYTES as i64
        ],
        |row| row.get(0),
    )?;
    if !bounded {
        return Err(StoreError::ExecutionConflict);
    }
    let q =
        strict_request(c, &r.scope.session, &r.request)?.ok_or(StoreError::ExecutionConflict)?;
    let e =
        Store::entry_in(c, &r.scope.session, r.user_entry)?.ok_or(StoreError::ExecutionConflict)?;
    let saved: Vec<u8> = c.query_row(
        "SELECT fingerprint FROM cli_agent_turns WHERE session=?1 AND request_key=?2",
        params![r.scope.session, r.request],
        |r| r.get(0),
    )?;
    let digest = input_digest(&Input {
        mode: q.mode,
        session: &r.scope.session,
        request: &r.request,
        generation: r.scope.generation,
        operation: &r.scope.operation,
        binding: lease.binding,
        text: &q.text,
        images: &q.images,
    });
    let expected = EntryKind::Message {
        message: Message::User {
            content: input_data::content(&q.text, &q.images),
            timestamp: q.created_at_ms,
            extra: Default::default(),
        },
        mode: q.mode,
        request: Some(r.request.clone()),
        partial: false,
    };
    if q.state != RequestState::Running
        || !matches!(
            q.mode,
            InputMode::Prompt | InputMode::FollowUp | InputMode::Steer
        )
        || q.run != Some(r.scope.run)
        || input_data::validate(&q.text, &q.images).is_err()
        || q.model.as_deref() != Some(b.create.model.as_str())
        || q.fingerprint != fingerprint(q.mode, &q.text, &q.images)
        || saved != digest
        || e.retired
        || e.run != Some(r.scope.run)
        || e.kind != expected
    {
        return Err(StoreError::ExecutionConflict);
    }
    let compact:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_inputs WHERE session=?1 AND request_key=?2 AND kind='compact')",params![r.scope.session,r.request],|r|r.get(0))?;
    Ok(LaunchContext {
        compact,
        images: q.images,
        binding: b,
        prompt: inputs::prompt(c, r.scope.run, &q.text)?,
        receipt: r,
    })
}
fn eligible_history(c: &Connection, session: &str) -> Result<()> {
    let bad:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_turns WHERE session=?1 AND (state NOT IN ('completed','rejected') OR fenced=1))
        OR EXISTS(SELECT 1 FROM runs r WHERE r.session=?1 AND (r.state<>'settled' OR NOT EXISTS(SELECT 1 FROM cli_agent_turns t WHERE t.run=r.id)))
        OR EXISTS(SELECT 1 FROM requests q WHERE q.session=?1 AND NOT EXISTS(SELECT 1 FROM cli_agent_turns t WHERE t.session=q.session AND t.request_key=q.key) AND NOT EXISTS(SELECT 1 FROM cli_agent_input_deliveries d JOIN cli_agent_turns t ON t.run=d.run WHERE d.session=q.session AND d.request_key=q.key AND q.run=d.run) AND NOT (q.state IN ('queued','withdrawn') AND EXISTS(SELECT 1 FROM cli_agent_inputs i WHERE i.session=q.session AND i.request_key=q.key)))
        OR EXISTS(SELECT 1 FROM entries e WHERE e.session=?1 AND NOT EXISTS(SELECT 1 FROM cli_agent_turns t WHERE t.run=e.run))",[session],|r|r.get(0))?;
    if bad {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}
impl Store {
    /// Read-only readiness hint for explicit owner reapproval. Admission repeats these checks
    /// in its transaction; this snapshot can never authorize execution or recover an owner.
    pub(crate) fn check_cli_agent_ready(&self, binding: &cli_agent::Binding) -> Result<()> {
        self.read(|c| {
            live(c, &binding.create.session, binding.digest)?;
            eligible_history(c, &binding.create.session)
        })
    }
    /// Reconcile request/body/identity against the permanent admission digest; never accept
    /// a replacement prompt or fresh/resume choice from the launcher/client.
    pub fn cli_agent_launch_context(&self, lease: &Lease) -> Result<LaunchContext> {
        self.read(|c| launch_context(c, lease))
    }
    pub fn cli_agent_turn(&self, session: &str, request: &str) -> Result<Option<Receipt>> {
        self.read(|c| stored(c, session, request))
    }
    /// Exact receipt lookup only: no lease, owner, write, preflight or capacity required.
    pub(crate) fn lookup_cli_agent_turn(&self, input: &Input<'_>) -> Result<Option<Receipt>> {
        if input_data::validate(input.text, input.images).is_err() {
            return Err(StoreError::ExecutionConflict);
        }
        let expected = input_digest(input);
        self.read(|c| {
            let receipt = stored(c, input.session, input.request)?;
            if receipt.is_some() {
                let actual: Vec<u8> = c.query_row(
                    "SELECT fingerprint FROM cli_agent_turns WHERE session=?1 AND request_key=?2",
                    params![input.session, input.request],
                    |r| r.get(0),
                )?;
                if actual != expected {
                    return Err(StoreError::ExecutionConflict);
                }
            }
            Ok(receipt)
        })
    }
    /// Atomic native request, run, canonical user entry, revision/outbox and root receipt.
    /// Duplicate admission is lookup only, even if the original owner acknowledgement was lost.
    pub fn admit_cli_agent_turn(
        &self,
        input: &Input<'_>,
        owner: &str,
        now: i64,
    ) -> Result<Admission> {
        self.admit_cli_agent_turn_with_wake(input, owner, None, now)
    }
    pub(crate) fn admit_cli_agent_turn_with_wake(
        &self,
        input: &Input<'_>,
        owner: &str,
        wake: Option<&Wake>,
        now: i64,
    ) -> Result<Admission> {
        self.admit_cli_agent_turn_with_child(input, owner, wake, None, now)
    }
    pub(crate) fn admit_cli_agent_turn_with_child(
        &self,
        input: &Input<'_>,
        owner: &str,
        wake: Option<&Wake>,
        child: Option<&cli_agent_children::Continuation>,
        now: i64,
    ) -> Result<Admission> {
        self.admit_cli_agent_turn_with_goal(input, owner, wake, child, None, now)
    }
    pub(crate) fn admit_cli_agent_turn_with_goal(
        &self,
        input: &Input<'_>,
        owner: &str,
        wake: Option<&Wake>,
        child: Option<&cli_agent_children::Continuation>,
        goal: Option<&GoalTurn>,
        now: i64,
    ) -> Result<Admission> {
        self.admit_cli_agent_turn_with_input(input, owner, wake, child, goal, None, now)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn admit_cli_agent_turn_with_input(
        &self,
        input: &Input<'_>,
        owner: &str,
        wake: Option<&Wake>,
        child: Option<&cli_agent_children::Continuation>,
        goal: Option<&GoalTurn>,
        queued: Option<&inputs::Queued>,
        now: i64,
    ) -> Result<Admission> {
        self.admit_cli_agent_turn_with_batch(input, owner, wake, child, goal, queued, &[], now)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn admit_cli_agent_turn_with_batch(
        &self,
        input: &Input<'_>,
        owner: &str,
        wake: Option<&Wake>,
        child: Option<&cli_agent_children::Continuation>,
        goal: Option<&GoalTurn>,
        queued: Option<&inputs::Queued>,
        batch: &[inputs::Queued],
        now: i64,
    ) -> Result<Admission> {
        if !batch.is_empty()
            && (queued.is_none()
                || !input.images.is_empty()
                || batch.iter().any(|q| !q.images.is_empty()))
        {
            return Err(StoreError::ExecutionConflict);
        }
        if input.mode
            != if let Some(q) = queued {
                q.kind.mode()
            } else if wake.is_some() || goal.is_some() {
                InputMode::FollowUp
            } else {
                InputMode::Prompt
            }
        {
            return Err(StoreError::ExecutionConflict);
        }
        if [input.session, input.request, input.operation, owner]
            .iter()
            .any(|s| !external::token(s))
            || input.generation == 0
            || input.generation > i64::MAX as u64
            || input_data::validate(input.text, input.images).is_err()
            || (!input.images.is_empty() && (wake.is_some() || child.is_some() || goal.is_some()))
        {
            return Err(StoreError::ExecutionConflict);
        }
        let fp = input_digest(input);
        self.tx(|c| {
            if let Some(receipt)=stored(c,input.session,input.request)? {
                let old:Vec<u8>=c.query_row("SELECT fingerprint FROM cli_agent_turns WHERE session=?1 AND request_key=?2",params![input.session,input.request],|r|r.get(0))?;
                if old!=fp {return Err(StoreError::ExecutionConflict);} return Ok(Admission {receipt,lease:None,parent_task_revision:None,job_revision:None});
            }
            let b=live(c,input.session,input.binding)?;
            eligible_history(c,input.session)?;
            if input.request.starts_with("goal:") && goal.is_none()
                || (input.request.starts_with("cli-job-wake:") || input.request.starts_with("cli-child-wake:")) && wake.is_none() {
                return Err(StoreError::ExecutionConflict);
            }

            if let Some(q)=queued {
                if wake.is_some() || child.is_some() || goal.is_some() || b.create.parent.is_some() {return Err(StoreError::ExecutionConflict);}
                q.validate(c,input)?;
            }
            if let Some(g)=goal {Self::cli_goal_allow_in(c,input.session,g.view.id,true)?;}
            if let Some(w)=wake {let (session,run)=w.origin();Self::cli_goal_allow_origin_in(c,session,run,true)?;}
            if let Some(child)=child {
                let scope=child.lease.scope();Self::cli_goal_allow_origin_in(c,&scope.session,scope.run,false)?;
            } else if wake.is_none() && b.create.parent.is_some() {
                let person:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_child_policies p JOIN subagent_tasks t ON t.child_session=p.child AND t.parent_session=p.parent AND t.parent_call=p.parent_call WHERE p.child=?1 AND t.origin='user' AND json_extract(p.snapshot,'$.cliNative.origin')='user' AND json_extract(p.snapshot,'$.cliNative.parent.run')=0)",[input.session],|r|r.get(0))?;
                if !person {
                    let origin:(String,u64)=c.query_row("SELECT p.parent,b.run FROM cli_agent_child_policies p JOIN cli_bridge_calls bc ON bc.session=p.parent AND bc.call_id=p.parent_call JOIN cli_tool_bridges b ON b.id=bc.bridge WHERE p.child=?1",[input.session],|r|Ok((r.get(0)?,r.get(1)?)))?;
                    Self::cli_goal_allow_origin_in(c,&origin.0,origin.1,false)?;
                }
            }
            if let Some(wake)=wake {wake.validate_in(c,input)?;}
            if let Some(child)=child {
                if wake.is_some() || child.child!=input.session || child.text!=input.text || input.request!=format!("child:{}",child.call) {return Err(StoreError::ExecutionConflict);}
                child.validate(c)?;
                c.execute("UPDATE subagent_tasks SET status='pending',result=NULL,delivery=?2,updated_at=?3 WHERE child_session=?1",params![input.session,child.delivery,now])?;
            }
            if let Some(turn)=goal {
                let goal=&turn.view;
                if wake.is_some() || child.is_some() || b.create.parent.is_some()
                    || Self::goal_current(c,input.session)?.as_ref()!=Some(goal)
                    || goal.state!=GoalState::Active
                    || input.request!=crate::goals::request_key(goal.id,goal.epoch+1)
                    || !Self::queue_in(c,input.session)?.is_empty()
                {return Err(StoreError::ExecutionConflict);}
                let mut text=crate::goals::turn_prompt_structured(&goal.objective,&goal.criteria);
                if let Some(findings)=crate::goals::findings_section(goal) {text.push_str(&findings);}
                if input.text!=text {return Err(StoreError::ExecutionConflict);}
                Self::goal_schedule_turn(c,input.session,goal.id,goal.epoch,input.request,now)?;
                let revision=Self::plan_state(c,input.session)?.revision;
                let signature=crate::goals::progress_signature(&goal.criteria,goal.note.as_deref(),&goal.linked_tasks,revision);
                Self::goal_set_turn_marks(c,input.session,goal.id,Some(&signature),Some(0),None)?;
            }
            let resume:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_agent_turns WHERE session=?1 AND state='completed')",[input.session],|r|r.get(0))?;
            c.execute("INSERT INTO runs(session,request,state,started_at) VALUES(?1,?2,'running',?3)",params![input.session,input.request,now])?;let run=c.last_insert_rowid() as u64;
            if queued.is_some() {
                c.execute("UPDATE requests SET state='running',run=?3,created_at=?4 WHERE session=?1 AND key=?2 AND state='queued'",params![input.session,input.request,run,now])?;
            } else {
            c.execute("INSERT INTO requests(session,key,fingerprint,mode,state,run,text,images,model,created_at) VALUES(?1,?2,?3,?8,'running',?4,?5,?9,?6,?7)",params![input.session,input.request,fingerprint(input.mode,input.text,input.images),run,input.text,b.create.model,now,match input.mode {InputMode::FollowUp=>"followUp",_=>"prompt"},json(&input.images)])?;
            }
            let entry=Self::append_entry(c,input.session,now,Some(run),&EntryKind::Message {message:Message::User {content:input_data::content(input.text,input.images),timestamp:now,extra:Default::default()},mode:input.mode,request:Some(input.request.into()),partial:false})?;
            c.execute("INSERT INTO cli_agent_turns(session,request_key,generation,operation,run,owner,binding_hash,fingerprint,user_entry,mode,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'admitted',?11,?11)",params![input.session,input.request,input.generation,input.operation,run,owner,input.binding,fp,entry.id,if resume {"resume"}else{"fresh"},now])?;
            if let Some(q)=queued {
                for member in batch {
                    if member.kind!=q.kind || member.kind==inputs::Kind::Compact || member.request==q.request {return Err(StoreError::ExecutionConflict);}
                    inputs::consume(c, member, &Scope {session:input.session.into(),run,generation:input.generation,operation:input.operation.into()}, None, now)?;
                }
                inputs::prompt(c, run, input.text)?; // Bound the exact wire envelope before committing.
            }
            Self::bump(c,input.session,now)?;
            let receipt=stored(c,input.session,input.request)?.ok_or(StoreError::ExecutionConflict)?;
            let mut job_revision = None;
            let parent_task_revision = if let Some(wake)=wake {
                wake.deliver(c,now)?;
                let revision = Some((input.session.into(),Self::bump(c,input.session,now)?));
                if matches!(wake.target(), WakeTarget::Job(_)) {
                    job_revision=revision;
                    if let Some(parent)=&b.create.parent {
                        let task=Self::task_by_session(c,input.session)?.ok_or(StoreError::ExecutionConflict)?;
                        if task.status!="done" || matches!(task.delivery.as_str(),"wake"|"notify") { return Err(StoreError::ExecutionConflict); }
                        Self::set_task_status(c,input.session,"running",None,now)?;
                        Some((parent.clone(),Self::bump(c,parent,now)?))
                    } else {None}
                } else { revision }
            } else if b.create.parent.is_some() {
                let parent:Option<String>=c.query_row("SELECT p.parent FROM cli_agent_child_policies p JOIN subagent_tasks t ON t.child_session=p.child AND t.parent_session=p.parent AND t.parent_call=p.parent_call WHERE p.child=?1 AND t.status='pending'",[input.session],|r|r.get(0)).optional()?;
                if let Some(parent)=parent {
                    Self::set_task_status(c,input.session,"running",None,now)?;
                    Some((parent.clone(),Self::bump(c,&parent,now)?))
                } else {None}
            } else {None};
            Ok(Admission {lease:Some(Lease {receipt:receipt.clone(),owner:owner.into(),binding:input.binding}),receipt,parent_task_revision,job_revision})
        })
    }
    /// Strict one-use CAS. This commits before spawn; a lost ack MUST NOT repeat spawn.
    pub fn begin_cli_agent_launch(&self, lease: &Lease, bridge: &str, now: i64) -> Result<Receipt> {
        self.tx(|c| {
            let context=launch_context(c,lease)?;let r=&context.receipt;
            Self::cli_goal_allow_origin_in(c,&r.scope.session,r.scope.run,false)?;
            if r.request.starts_with("goal:") || r.request.starts_with("cli-job-wake:") || r.request.starts_with("cli-child-wake:") {
                for (root,id) in Self::cli_run_goals_in(c,&r.scope.session,r.scope.run)? {
                    let goal=Self::goal_current(c,&root)?.ok_or(StoreError::ExecutionConflict)?;
                    let usage=Self::cli_goal_usage_in(c,&root,id)?;
                    if goal.limits.max_turns.is_some_and(|n|usage.turns.max(goal.turns)>n) {return Err(StoreError::ExecutionConflict);}
                }
            }

            let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_tool_bridges WHERE id=?1 AND session=?2 AND generation=?3 AND operation=?4 AND run=?5 AND owner=?6 AND state='prepared')",params![bridge,r.scope.session,r.scope.generation,r.scope.operation,r.scope.run,lease.owner],|row|row.get(0))?;
            if !valid {return Err(StoreError::ExecutionConflict);}
            c.execute("UPDATE cli_agent_turns SET state='launchIntent',bridge=?3,updated_at=MAX(updated_at,?4) WHERE session=?1 AND request_key=?2",params![r.scope.session,r.request,bridge,now])?;
            Self::bump(c,&r.scope.session,now)?;stored(c,&r.scope.session,&r.request)?.ok_or(StoreError::ExecutionConflict)
        })
    }
    pub fn mark_cli_agent_running(&self, lease: &Lease, now: i64) -> Result<()> {
        self.tx(|c| {let r=owned(c,lease)?;live(c,&r.scope.session,lease.binding)?;running(c,&r)?;
            if r.state!=State::LaunchIntent {return Err(StoreError::ExecutionConflict);}
            c.execute("UPDATE cli_agent_turns SET state='running',updated_at=MAX(updated_at,?3) WHERE session=?1 AND request_key=?2",params![r.scope.session,r.request,now])?;
            Self::bump(c,&r.scope.session,now)?;Ok(())})
    }
    /// Actual clean root evidence PLUS a cleanly closed native channel and all actual receipts.
    /// Refusal does not synthesize failure/success; the owner must revoke work then abandon.
    pub fn complete_cli_agent_turn(
        &self,
        lease: &Lease,
        evidence: &Completion,
        now: i64,
    ) -> Result<Settlement> {
        self.tx(|c| {
            let r=owned(c,lease)?;
            if r.fenced {return Ok(Settlement::Conflict);}
            if r.state==State::Completed {
                let saved:String=c.query_row("SELECT evidence FROM cli_agent_turns WHERE session=?1 AND request_key=?2",params![r.scope.session,r.request],|r|r.get(0))?;
                if saved!=json(evidence) {poison(c,&r,now)?;return Ok(Settlement::Conflict);} return Ok(Settlement::AlreadyCommitted);
            }
            let b=live(c,&r.scope.session,lease.binding)?;running(c,&r)?;
            Self::assert_cli_child_context_read_in(c, &r.scope.session, r.scope.run)?;
            Self::assert_cli_auditor_inspected_in(c,&r.scope.session,r.scope.run)?;
            inputs::assert_compacted(c,&r)?;
            if r.state!=State::Running || !evidence.clean() || evidence.cli_session_id!=b.create.identity.cli_session_id {return Err(StoreError::ExecutionConflict);}
            let clean:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cli_tool_bridges WHERE id=?1 AND run=?2 AND state='closed')
                AND NOT EXISTS(SELECT 1 FROM cli_bridge_calls b LEFT JOIN tool_calls t ON t.session=b.session AND t.call_id=b.call_id WHERE b.bridge=?1 AND (b.state<>'completed' OR t.state IS NOT 'done' OR t.run IS NOT ?2))
                AND NOT EXISTS(SELECT 1 FROM tool_calls WHERE run=?2 AND state<>'done')",params![r.bridge,r.scope.run],|row|row.get(0))?;
            if !clean {return Err(StoreError::ExecutionConflict);}
            c.execute("UPDATE cli_agent_turns SET state='completed',evidence=?3,updated_at=MAX(updated_at,?4) WHERE session=?1 AND request_key=?2",params![r.scope.session,r.request,json(evidence),now])?;
            inputs::settle(c,&r,&RunOutcome::Completed,now)?;Self::settle_run(c,r.scope.run,&RunOutcome::Completed,now)?;Self::settle_request(c,&r.scope.session,&r.request,&RunOutcome::Completed,now)?;Self::bump(c,&r.scope.session,now)?;Ok(Settlement::Committed)
        })
    }
    /// Owner revokes sockets/workers/processes FIRST, then durably retires. No late promotion.
    pub fn abandon_cli_agent_turn(&self, lease: &Lease, now: i64) -> Result<()> {
        self.tx(|c| retire(c, &owned(c, lease)?, now))
    }
    /// Invoke before generic native recovery. No lease is reconstructed, no CLI is resumed.
    pub fn recover_cli_agent_turns(&self, now: i64) -> Result<usize> {
        self.tx(|c| {
            let rows=c.prepare("SELECT session,request_key FROM cli_agent_turns WHERE state IN ('admitted','launchIntent','running')")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            for (s,k) in &rows {retire(c,&stored(c,s,k)?.ok_or(StoreError::ExecutionConflict)?,now)?;}
            let sessions=c.prepare("SELECT DISTINCT q.session FROM requests q JOIN cli_agent_inputs i ON i.session=q.session AND i.request_key=q.key WHERE q.state='queued'")?.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            c.execute("UPDATE requests SET state='withdrawn',outcome=?1 WHERE state='queued' AND EXISTS(SELECT 1 FROM cli_agent_inputs i WHERE i.session=requests.session AND i.request_key=requests.key)",[json(&RunOutcome::Aborted)])?;
            for s in sessions {Self::bump(c,&s,now)?;}
            Self::cli_goal_recover_usage_in(c,now)?;
            Ok(rows.len())
        })
    }
}
#[cfg(test)]
pub(crate) fn remove_schema_for_test(c: &Connection) -> Result<()> {
    cli_agent_children::remove_schema_for_test(c)?;
    c.execute_batch("DROP TABLE cli_agent_turns;")?;
    Ok(())
}
#[cfg(test)]
mod tests;
