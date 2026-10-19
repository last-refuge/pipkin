//! Receipt-scoped inert action projection. No approval/action correlation is guessed.
//! Only `project_external_event_in` calls this after its run/cursor/terminal checks.
use super::*;

impl Store {
    pub(super) fn project_external_action_in(
        tx: &Transaction<'_>,
        key: &ExternalRunKey,
        receipt: &ExternalPromptReceipt,
        event: &Event,
        now: i64,
    ) -> Result<Vec<SessionChange>> {
        let policy: String = tx.query_row(
            "SELECT launch_policy FROM cli_bindings WHERE session=?1",
            [&key.session],
            |r| r.get(0),
        )?;
        let unattended = policy == crate::execution::claude_cli::coding_policy::ID;
        let allowed = |name: &str| {
            if unattended {
                token(name)
            } else {
                name == "Write"
            }
        };
        if !unattended && policy != super::super::permissions::EXTERNAL_PERMISSION_POLICY {
            return Err(StoreError::UnsupportedExecution);
        }
        match event {
            Event::ExternalToolObserved {
                message_id,
                block_index,
                tool_id,
                name,
            } => {
                if message_id.is_empty()
                    || message_id.len() > 128
                    || !message_id.bytes().all(|b| b.is_ascii_graphic())
                    || *block_index >= MAX_BLOCKS
                    || !token(tool_id)
                    || !allowed(name)
                {
                    return Err(StoreError::ExecutionConflict);
                }
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM external_actions WHERE session=?1 AND generation=?2 AND op=?3 AND call_id=?4)", params![key.session,key.generation,key.op,tool_id], |r| r.get(0))?;
                let count: usize = tx.query_row(
                    "SELECT COUNT(*) FROM external_projection_actions WHERE prompt=?1",
                    [receipt.id],
                    |r| r.get(0),
                )?;
                if exists || count >= 4096 {
                    return Err(StoreError::ExecutionConflict);
                }
                spend(tx, receipt.id, tool_id.len() + name.len())?;
                let mapping: Option<(u64,String)> = tx.query_row("SELECT entry,final_blocks FROM external_projection_messages WHERE prompt=?1 AND message_id=?2", params![receipt.id,message_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
                let (entry, mut finals) = if let Some((id, finals)) = mapping {
                    (
                        Some(
                            Self::entry_in(tx, &key.session, id)?
                                .ok_or(StoreError::ExecutionConflict)?,
                        ),
                        parse_json::<Vec<usize>>(&finals, "external final blocks")?,
                    )
                } else {
                    (None, vec![])
                };
                let mut blocks = match entry.as_ref().map(|e| &e.kind) {
                    Some(EntryKind::ExternalMessage { blocks, .. }) => blocks.clone(),
                    None => vec![],
                    _ => return Err(StoreError::ExecutionConflict),
                };
                // No holes, replacements of another block, or action-ID reattachment.
                if *block_index != blocks.len() || finals.contains(block_index) {
                    return Err(StoreError::ExecutionConflict);
                }
                blocks.push(ExternalBlock::Action {
                    source_tool_id: tool_id.clone(),
                    name: name.clone(),
                    state: ExternalActionState::Observed,
                    output: String::new(),
                });
                finals.push(*block_index);
                let kind = EntryKind::ExternalMessage {
                    backend: ExecutionBackend::LocalClaudeCli,
                    role: ExternalRole::Assistant,
                    source_message_id: Some(message_id.clone()),
                    request: Some(receipt.request.clone()),
                    blocks,
                    partial: true,
                };
                let (id, change) = if let Some(mut entry) = entry {
                    entry.kind = kind;
                    Self::replace_entry(tx, &key.session, &entry)?;
                    (entry.id, SessionChange::EntryReplaced { entry })
                } else {
                    let messages: usize = tx.query_row(
                        "SELECT COUNT(*) FROM external_projection_messages WHERE prompt=?1",
                        [receipt.id],
                        |r| r.get(0),
                    )?;
                    if messages >= 4096 {
                        return Err(StoreError::ExecutionConflict);
                    }
                    let entry =
                        Self::append_entry_owned(tx, &key.session, now, Some(receipt.id), kind)?;
                    tx.execute("INSERT INTO external_projection_messages(prompt,session,message_id,entry) VALUES(?1,?2,?3,?4)", params![receipt.id,key.session,message_id,entry.id])?;
                    (entry.id, SessionChange::EntryAppended { entry })
                };
                tx.execute("UPDATE external_projection_messages SET final_blocks=?3 WHERE prompt=?1 AND message_id=?2", params![receipt.id,message_id,json(&finals)])?;
                tx.execute("INSERT INTO external_projection_actions(prompt,session,tool_id,entry,block_index) VALUES(?1,?2,?3,?4,?5)", params![receipt.id,key.session,tool_id,id,block_index])?;
                tx.execute("INSERT INTO external_actions(session,generation,op,call_id,tool_name,status,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'observed',?6,?6)", params![key.session,key.generation,key.op,tool_id,name,now])?;
                Ok(vec![change])
            }
            Event::ExternalToolResult {
                tool_id,
                is_error,
                text,
            } => {
                if !token(tool_id) {
                    return Err(StoreError::ExecutionConflict);
                }
                let (id, index): (u64,usize) = tx.query_row("SELECT entry,block_index FROM external_projection_actions WHERE prompt=?1 AND session=?2 AND tool_id=?3", params![receipt.id,key.session,tool_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(StoreError::ExecutionConflict)?;
                let mut entry =
                    Self::entry_in(tx, &key.session, id)?.ok_or(StoreError::ExecutionConflict)?;
                let EntryKind::ExternalMessage { blocks, .. } = &mut entry.kind else {
                    return Err(StoreError::ExecutionConflict);
                };
                let Some(ExternalBlock::Action {
                    source_tool_id,
                    name,
                    state,
                    output,
                }) = blocks.get_mut(index)
                else {
                    return Err(StoreError::ExecutionConflict);
                };
                if source_tool_id != tool_id
                    || !allowed(name)
                    || *state != ExternalActionState::Observed
                {
                    return Err(StoreError::ExecutionConflict);
                }
                spend(tx, receipt.id, text.len())?;
                let status = match is_error {
                    Some(false) => {
                        *state = ExternalActionState::ReportedSuccess;
                        "completed"
                    }
                    Some(true) => {
                        *state = ExternalActionState::ReportedFailure;
                        "failed"
                    }
                    None => {
                        *state = ExternalActionState::Unknown;
                        "unknown"
                    }
                };
                *output = text.clone();
                if tx.execute("UPDATE external_actions SET status=?5,updated_at=MAX(updated_at,?6) WHERE session=?1 AND generation=?2 AND op=?3 AND call_id=?4 AND tool_name=?7 AND status='observed'", params![key.session,key.generation,key.op,tool_id,status,now,name.as_str()])? != 1 {
                    return Err(StoreError::ExecutionConflict);
                }
                Self::replace_entry(tx, &key.session, &entry)?;
                Ok(vec![SessionChange::EntryReplaced { entry }])
            }
            _ => Err(StoreError::ExecutionConflict),
        }
    }
    pub(super) fn assert_external_actions_resolved(
        c: &Connection,
        key: &ExternalRunKey,
    ) -> Result<()> {
        let unresolved: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM external_actions WHERE session=?1 AND generation=?2 AND op=?3 AND status NOT IN ('completed','failed'))", params![key.session,key.generation,key.op], |r| r.get(0))?;
        if unresolved {
            Err(StoreError::ExecutionConflict)
        } else {
            Ok(())
        }
    }
}
fn spend(tx: &Transaction<'_>, prompt: u64, bytes: usize) -> Result<()> {
    if bytes > MAX_TEXT {
        return Err(StoreError::ExecutionConflict);
    }
    if tx.execute("UPDATE external_prompts SET projected_bytes=projected_bytes+?2 WHERE id=?1 AND projected_bytes<=?3", params![prompt,bytes,MAX_TEXT-bytes])? != 1 {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}
