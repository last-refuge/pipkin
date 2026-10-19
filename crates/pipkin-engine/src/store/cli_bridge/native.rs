//! Atomic native tool journal joins, under an authenticated connection lease. These do not
//! relax native API execution guards. Orchestration session creation/CLI launch remain absent.
use super::*;
use crate::store::cli_agent_turns::projection::Update;
use crate::tools::{ToolResult, text::BoundedOutput};
use serde_json::{Map, json};

/// Exact model-visible tool output in MCP form, without flattening images or leaking internal
/// tool details into executable metadata. Unsupported block kinds/oversize output fail closed.
pub fn mcp_result(result: &ToolResult) -> Result<Value> {
    let mut content = Vec::new();
    for block in &result.content {
        content.push(match block {
            Block::Text { text, .. } => json!({"type":"text","text":text}),
            Block::Image {
                data, mime_type, ..
            } => json!({"type":"image","data":data,"mimeType":mime_type}),
            _ => return Err(StoreError::ExecutionConflict),
        });
    }
    let value = json!({"content":content,"isError":result.is_error});
    if canonical(&value).len() > MAX_RESULT_BODY_BYTES {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(value)
}
const MAX_NATIVE_BYTES: i64 = 512 * 1024 * 1024;
fn native_bytes(c: &Connection, lease: &Lease) -> Result<i64> {
    Ok(c.query_row("SELECT COALESCE((SELECT SUM(length(CAST(body AS BLOB))) FROM entries WHERE session=?1 AND run=?2),0)
        +COALESCE((SELECT SUM(length(CAST(o.text AS BLOB))) FROM tool_outputs o JOIN tool_calls t ON t.session=o.session AND t.call_id=o.call_id WHERE t.session=?1 AND t.run=?2),0)",params![lease.scope().session,lease.scope().run as i64],|r|r.get(0))?)
}
impl Store {
    /// A scoped host service must join the actual authenticated pending native call in its
    /// own transaction, not trust SDK observations or an arbitrary caller-provided call id.
    pub(crate) fn assert_cli_native_service_in(
        c: &Connection,
        lease: &Lease,
        call: &str,
        name: &str,
    ) -> Result<()> {
        super::super::cli_agent_turns::assert_dispatch(c, lease.scope(), &lease.0.id)?;
        if owned_state(c, lease)? != "connected" || !live_scope(c, lease.scope())? {
            return Err(StoreError::ExecutionConflict);
        }
        let valid: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM cli_bridge_calls b JOIN tool_calls t
            ON t.session=b.session AND t.call_id=b.call_id WHERE b.bridge=?1 AND b.call_id=?2
            AND b.state='intent' AND b.name=?3 AND t.name=?3 AND t.run=?4 AND t.state='intent')",
            params![lease.0.id, call, name, lease.scope().run as i64],
            |r| r.get(0),
        )?;
        if !valid {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(())
    }
    /// Capture context from the lease's stored session, never selected UI state/client args.
    pub fn cli_native_context(&self, lease: &Lease) -> Result<SessionRow> {
        self.read(|c| {
            super::super::cli_agent_turns::assert_dispatch(c, lease.scope(), &lease.0.id)?;
            if owned_state(c, lease)? != "connected" || !live_scope(c, lease.scope())? {
                return Err(StoreError::ExecutionConflict);
            }
            Self::cli_agent_binding_in(c, &lease.scope().session)?
                .ok_or(StoreError::ExecutionConflict)?;
            Ok(c.query_row(
                "SELECT * FROM sessions WHERE id=?1",
                [&lease.scope().session],
                Self::session_from_row,
            )?)
        })
    }
    /// One transaction reserves the bridge identity, native intent, canonical tool-call entry
    /// and history id. Replay-safe native tools are NOT replayable through this driver.
    pub fn admit_cli_native_call(
        &self,
        lease: &Lease,
        id: &Value,
        name: &str,
        args: &Value,
        now: i64,
    ) -> Result<Admission> {
        self.admit_cli_native_call_published(lease, id, name, args, now)
            .map(|(admission, _)| admission)
    }
    pub(crate) fn admit_cli_native_call_published(
        &self,
        lease: &Lease,
        id: &Value,
        name: &str,
        args: &Value,
        now: i64,
    ) -> Result<(Admission, Option<Update>)> {
        self.tx(|tx| {
            let mut update=None;
            super::super::cli_agent_turns::assert_dispatch(tx,lease.scope(),&lease.0.id)?;
            super::super::cli_agent_turns::assert_effects_allowed(tx,lease.scope())?;
            Self::cli_agent_binding_in(tx,&lease.scope().session)?.ok_or(StoreError::ExecutionConflict)?;
            if name != crate::execution::claude_cli::orchestration_child::context::NAME {
                Self::assert_cli_child_context_read_in(tx, &lease.scope().session, lease.scope().run)?;
            }
            let admission=Self::admit_cli_bridge_call_in(tx,lease,id,name,args,now)?;
            if let Admission::New(claim)=&admission {
                let scope=lease.scope();
                let used:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM tool_call_ids WHERE session=?1 AND call_id=?2)",params![scope.session,claim.call_id],|r|r.get(0))?;
                if used {return Err(StoreError::ExecutionConflict);}
                // Intentionally not Store::tool_intent: its native-model-only guard remains.
                // The scoped durable bridge admission above is the distinct authorization.
                tx.execute("INSERT INTO tool_calls(session,call_id,run,name,arguments,state,replay_safe,started_at)
                    VALUES(?1,?2,?3,?4,?5,'intent',0,?6)",params![scope.session,claim.call_id,scope.run as i64,name,canonical(args),now])?;
                let model:Option<String>=tx.query_row("SELECT model FROM sessions WHERE id=?1",[&scope.session],|r|r.get(0))?;
                let mut extra=Map::new();
                extra.insert("pipkinCliBridge".into(),json!({"bridge":lease.0.id,"source":"authenticatedToolChannel","generation":scope.generation,"operation":scope.operation}));
                let kind=EntryKind::Message {message:Message::Assistant {
                    content:vec![Block::tool_call(&claim.call_id,name,args.clone())],api:"claude-cli-tools".into(),provider:"claude-cli".into(),model:model.unwrap_or_default(),
                    usage:Usage::default(),stop_reason:StopReason::ToolUse,error_message:None,timestamp:now,extra,
                },mode:InputMode::Prompt,request:None,partial:false};
                if native_bytes(tx,lease)? + json(&kind).len() as i64 > MAX_NATIVE_BYTES {return Err(StoreError::ExecutionConflict);}
                let entry=Self::append_entry(tx,&scope.session,now,Some(scope.run),&kind)?;
                let revision=Self::bump(tx,&scope.session,now)?;
                update=Some(Update {scope:scope.clone(),revision,changes:vec![SessionChange::EntryAppended {entry}]});
            } else if matches!(admission,Admission::Pending|Admission::Completed(_)) {
                // A protocol-only fixture/old receipt cannot be upgraded into native authority.
                let key=rpc_key(id)?;
                let joined:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM cli_bridge_calls b JOIN tool_calls t
                    ON t.session=b.session AND t.call_id=b.call_id WHERE b.bridge=?1 AND b.rpc_key=?2
                    AND t.run=?3)",params![lease.0.id,key,lease.scope().run as i64],|r|r.get(0))?;
                if !joined {fence(tx,&lease.0.id,now)?;return Ok((Admission::Fenced,None));}
            }
            Ok((admission,update))
        })
    }
    /// Native result, retained output, revision/outbox and exact MCP receipt commit together.
    /// No transient completion is published/replied before this succeeds. Late results cannot
    /// replace Unknown/Interrupted state; duplicate results cannot add a second native entry.
    pub fn settle_cli_native_call(
        &self,
        lease: &Lease,
        claim: &Claim,
        result: &ToolResult,
        now: i64,
    ) -> Result<(Settlement, Value)> {
        self.settle_cli_native_call_published(lease, claim, result, now)
            .map(|(settlement, _)| settlement)
    }
    pub(crate) fn settle_cli_native_call_published(
        &self,
        lease: &Lease,
        claim: &Claim,
        result: &ToolResult,
        now: i64,
    ) -> Result<((Settlement, Value), Option<Update>)> {
        self.settle_cli_native_call_with_inputs(lease, claim, result, &[], now)
    }
    pub(crate) fn settle_cli_native_call_with_inputs(
        &self,
        lease: &Lease,
        claim: &Claim,
        result: &ToolResult,
        inputs: &[crate::store::cli_agent_turns::inputs::Queued],
        now: i64,
    ) -> Result<((Settlement, Value), Option<Update>)> {
        let original = mcp_result(result)?;
        self.tx(|tx| {
            let mut reply = original.clone();
            let mut update = None;
            let scope = lease.scope();
            let tool = Self::tool_call_in(tx, &scope.session, claim.call_id())?
                .ok_or(StoreError::ExecutionConflict)?;
            if tool.run != Some(scope.run) {
                return Err(StoreError::ExecutionConflict);
            }
            let mut deliver=false;
            if tool.state==ToolState::Intent && !inputs.is_empty() {
                crate::store::cli_agent_turns::assert_effects_allowed(tx,scope)?;
                let messages=inputs.iter().map(|q|(q.request.clone(),q.text.clone())).collect::<Vec<_>>();
                let text=format!("Pipkin user steering delivered at this native tool boundary. This is additional user input, separate from the tool's output.\n{}",crate::store::cli_agent_turns::inputs::envelope(&messages));
                reply["content"].as_array_mut().ok_or(StoreError::ExecutionConflict)?.push(json!({"type":"text","text":text}));
                for q in inputs {
                    reply["content"].as_array_mut().ok_or(StoreError::ExecutionConflict)?.extend(crate::execution::claude_cli::orchestration_input::image_blocks(&q.images,true));
                }
                // If output leaves no room, leave all steering queued for a later boundary.
                if canonical(&reply).len()>MAX_RESULT_BODY_BYTES {reply=original.clone();}
                else { deliver=true; }
            } else if tool.state==ToolState::Done {
                let raw:String=tx.query_row("SELECT result FROM cli_bridge_calls WHERE bridge=?1 AND call_id=?2 AND state='completed'",params![lease.0.id,claim.call_id()],|r|r.get(0))?;
                reply=parse_json(&raw,"native bridge receipt")?;
            }
            let settlement = Self::settle_cli_bridge_call_in(tx, lease, claim, &reply, now)?;
            if settlement == Settlement::Committed {
                if tool.state != ToolState::Intent {
                    return Err(StoreError::ExecutionConflict);
                }
                let mut extra = Map::new();
                if let Some(details) = &result.details {
                    extra.insert("details".into(), details.clone());
                }
                let kind = EntryKind::Message {
                    message: Message::ToolResult {
                        tool_call_id: claim.call_id.clone(),
                        tool_name: tool.name,
                        content: result.content.clone(),
                        is_error: result.is_error,
                        timestamp: now,
                        extra,
                    },
                    mode: InputMode::Prompt,
                    request: None,
                    partial: false,
                };
                if json(&kind).len() > MAX_RESULT_BYTES
                    || native_bytes(tx, lease)?
                        + json(&kind).len() as i64
                        + result.retained.len().min(MAX_RETAINED_OUTPUT_BYTES) as i64
                        > MAX_NATIVE_BYTES
                {
                    return Err(StoreError::ExecutionConflict);
                }
                let entry = Self::append_entry(tx, &scope.session, now, Some(scope.run), &kind)?;
                Self::tool_result(
                    tx,
                    &scope.session,
                    claim.call_id(),
                    ToolState::Done,
                    entry.id,
                    &result.retained,
                    result.dropped_bytes,
                    now,
                )?;
                let mut delivered=Vec::new();
                if deliver {
                    for q in inputs {
                        if q.kind!=crate::store::cli_agent_turns::inputs::Kind::Steer {return Err(StoreError::ExecutionConflict);}
                        delivered.push(crate::store::cli_agent_turns::inputs::consume(tx,q,scope,Some(claim.call_id()),now)?);
                    }
                    if native_bytes(tx,lease)?>MAX_NATIVE_BYTES {return Err(StoreError::ExecutionConflict);}
                }
                let revision = Self::bump(tx, &scope.session, now)?;
                update = Some(Update {
                    scope: scope.clone(),
                    revision,
                    changes: {
                        let mut changes=vec![SessionChange::EntryAppended {entry}];
                        if !delivered.is_empty() {
                            changes.extend(delivered);
                            changes.push(SessionChange::QueueChanged {queue:Self::queue_view(&Self::queue_in(tx,&scope.session)?)});
                        }
                        changes
                    },
                });
            } else if settlement == Settlement::AlreadyCommitted {
                let entry = Self::entry_in(
                    tx,
                    &scope.session,
                    tool.result_entry.ok_or(StoreError::ExecutionConflict)?,
                )?
                .ok_or(StoreError::ExecutionConflict)?;
                let matches = match entry.kind {
                    EntryKind::Message {
                        message:
                            Message::ToolResult {
                                content,
                                is_error,
                                extra,
                                ..
                            },
                        ..
                    } => {
                        content == result.content
                            && is_error == result.is_error
                            && extra.get("details") == result.details.as_ref()
                    }
                    _ => false,
                };
                let (kept, _) = clip(&result.retained, MAX_RETAINED_OUTPUT_BYTES);
                let (text, dropped): (String, i64) = tx.query_row(
                    "SELECT text,dropped FROM tool_outputs WHERE session=?1 AND call_id=?2",
                    params![scope.session, claim.call_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                if !matches
                    || text != kept
                    || dropped as u64
                        != result.dropped_bytes + (result.retained.len() - kept.len()) as u64
                {
                    fence(tx, &lease.0.id, now)?;
                    return Ok(((Settlement::Fenced, reply.clone()), None));
                }
            }
            Ok(((settlement, reply.clone()), update))
        })
    }
    /// Read-only committed receipt for the same authenticated lifetime. Never reconstructs a claim.
    pub fn cli_native_receipt(&self, lease: &Lease, id: &Value) -> Result<Value> {
        let key = rpc_key(id)?;
        self.read(|c| {
            if owned_state(c,lease)?!="connected" || !live_scope(c,lease.scope())? {return Err(StoreError::ExecutionConflict);}
            let raw:String=c.query_row("SELECT b.result FROM cli_bridge_calls b JOIN tool_calls t ON t.session=b.session AND t.call_id=b.call_id
                WHERE b.bridge=?1 AND b.rpc_key=?2 AND b.state='completed' AND t.state='done' AND t.run=?3 AND t.result_entry IS NOT NULL",
                params![lease.0.id,key,lease.scope().run as i64],|r|r.get(0))?;
            parse_json(&raw,"native bridge receipt")
        })
    }
    pub fn cli_native_progress(
        &self,
        lease: &Lease,
        call: &str,
        output: &BoundedOutput,
    ) -> Result<()> {
        self.tx(|tx| {
            if owned_state(tx,lease)?!="connected" || !live_scope(tx,lease.scope())? {return Err(StoreError::ExecutionConflict);}
            let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM cli_bridge_calls WHERE bridge=?1 AND call_id=?2 AND state='intent')",params![lease.0.id,call],|r|r.get(0))?;
            if !valid {return Err(StoreError::ExecutionConflict);}
            Self::save_tool_progress(tx,&lease.scope().session,call,output)
        })
    }
}

#[cfg(test)]
mod tests;
