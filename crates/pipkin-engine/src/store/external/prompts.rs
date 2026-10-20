//! External prompt receipts (inert text/action projection). `_in` methods never bump revisions or publish: the caller
//! must commit the transaction and its returned changes together (and roll back on error).
//! Cursor starts at zero; the first accepted decoder event has cursor one. A receipt, not
//! a native run/request, owns this lifecycle. Terminal observations are only provisional.
use super::*;
use crate::execution::claude_cli::events::{Event, Outcome, TextKind};
use serde::{Deserialize, Serialize};

#[path = "action_projection.rs"]
mod actions;

const MAX_TEXT: usize = 1024 * 1024;
const MAX_BLOCKS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalPromptInput {
    pub session: String,
    pub request: String,
    pub text: String,
    pub op: String,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExternalPromptMode {
    Fresh,
    Resume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExternalTerminalOutcome {
    Success,
    Error,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalTerminalUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalTerminal {
    pub outcome: ExternalTerminalOutcome,
    pub usage: Option<ExternalTerminalUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<crate::execution::claude_cli::usage_metadata::Metadata>,
}

/// Independent supervisor facts, never inferred from projected text or terminal metadata.
/// Default is deliberately uncertain and is used for startup recovery.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalSettlementEvidence {
    /// Acceptance witness: decoder finished and every event committed AND supervised
    /// transport finished without a stop. Success vs reported error additionally checks
    /// terminal/exit evidence below. EOF/decoder completion alone never establishes this.
    pub projection_finished: bool,
    pub input_delivered: bool,
    pub root_reaped: bool,
    pub pipes_drained: bool,
    pub exit_code: Option<i32>,
    pub stop_requested: bool,
    pub cleanup_timed_out: bool,
    pub cancelled_clean_confirmed: bool,
    /// The connection/consent is still authorized after cancellation cleanup. False
    /// for disconnect, revoked consent, shutdown and historical receipts without proof.
    #[serde(default)]
    pub continuation_authorized: bool,
    /// Closed diagnostic facts, never positive success/authority evidence. Errors can
    /// veto completion. Optional for old receipts; no raw payloads or type names.
    #[serde(default)]
    pub supervisor_stop: Option<crate::execution::claude_cli::process::Stop>,
    #[serde(default)]
    pub projection_error: Option<crate::execution::claude_cli::events::Error>,
    #[serde(default)]
    pub rejected_record: Option<crate::execution::claude_cli::events::RecordClass>,
    #[serde(default)]
    pub parse_failure: Option<crate::execution::claude_cli::events::ParseFailure>,
    /// Inert display loss; never vetoes independently verified execution completion.
    #[serde(default)]
    pub presentation_incomplete: bool,
    #[serde(default)]
    pub stream_diagnostic: Option<crate::execution::claude_cli::events::StreamDiagnostic>,
}

impl ExternalSettlementEvidence {
    /// A user Stop may leave unfinished tools, but continuing requires a validated CLI
    /// session and confirmed owned-process cleanup. Truncated EOF is expected on Stop;
    /// malformed output, observer failure, early cancellation and lost cleanup are not.
    fn allows_cancel_continuation(&self, cursor: u64) -> bool {
        self.continuation_authorized && self.clean_cancelled_session(cursor)
    }

    pub(super) fn clean_cancelled_session(&self, cursor: u64) -> bool {
        use crate::execution::claude_cli::{events, process};
        cursor > 0
            && self.input_delivered
            && self.stop_requested
            && self.cancelled_clean_confirmed
            && self.supervisor_stop == Some(process::Stop::Cancelled)
            && self.root_reaped
            && self.pipes_drained
            && !self.cleanup_timed_out
            && self.parse_failure.is_none()
            && matches!(
                self.projection_error,
                None | Some(events::Error::Incomplete)
            )
            && matches!(
                self.rejected_record,
                None | Some(events::RecordClass::EndOfStream)
            )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalPromptReceipt {
    /// External protocol run ID; never a native `runs.id`.
    pub id: u64,
    pub key: ExternalRunKey,
    pub request: String,
    pub fingerprint: String,
    pub user_entry: u64,
    pub cursor: u64,
    pub mode: ExternalPromptMode,
    pub state: ExternalRunState,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub terminal: Option<ExternalTerminal>,
    pub evidence: Option<ExternalSettlementEvidence>,
    pub outcome: Option<RunOutcome>,
    pub result: Option<PromptResult>,
}

impl ExternalPromptReceipt {
    pub fn run_view(&self) -> Option<RunView> {
        self.state.active().then(|| RunView {
            id: self.id,
            request: Some(self.request.clone()),
            phase: if self.state == ExternalRunState::Stopping {
                RunPhase::Stopping
            } else {
                RunPhase::Running
            },
            started_at_ms: self.created_at_ms,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalAdmission {
    pub receipt: ExternalPromptReceipt,
    /// Only true authorizes a new launch sequence. A repeated request never does.
    pub newly_admitted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalSettlement {
    pub receipt: ExternalPromptReceipt,
    pub newly_settled: bool,
}

impl Store {
    pub fn external_prompt_in(
        c: &Connection,
        session: &str,
        request: &str,
    ) -> Result<Option<ExternalPromptReceipt>> {
        let id = c
            .query_row(
                "SELECT id FROM external_prompts WHERE session=?1 AND request_key=?2",
                params![session, request],
                |r| r.get::<_, u64>(0),
            )
            .optional()?;
        id.map(|id| Self::external_prompt_id_in(c, id)).transpose()
    }

    pub fn external_prompt(
        &self,
        session: &str,
        request: &str,
    ) -> Result<Option<ExternalPromptReceipt>> {
        self.read(|c| Self::external_prompt_in(c, session, request))
    }

    pub fn external_prompt_for_run_in(
        c: &Connection,
        key: &ExternalRunKey,
    ) -> Result<Option<ExternalPromptReceipt>> {
        let id = c
            .query_row(
                "SELECT id FROM external_prompts WHERE session=?1 AND generation=?2 AND op=?3",
                params![key.session, key.generation, key.op],
                |r| r.get::<_, u64>(0),
            )
            .optional()?;
        id.map(|id| Self::external_prompt_id_in(c, id)).transpose()
    }

    pub fn external_prompt_for_run(
        &self,
        key: &ExternalRunKey,
    ) -> Result<Option<ExternalPromptReceipt>> {
        self.read(|c| Self::external_prompt_for_run_in(c, key))
    }

    pub fn active_external_prompts(&self) -> Result<Vec<ExternalPromptReceipt>> {
        self.read(|c| {
            let ids = c.prepare("SELECT p.id FROM external_prompts p JOIN external_runs r USING(session,generation,op) WHERE r.state IN ('admitted','launchIntent','running','stopping') ORDER BY p.id")?
                .query_map([], |r| r.get::<_, u64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter().map(|id| Self::external_prompt_id_in(c, id)).collect()
        })
    }

    /// Read a durable external protocol run ID without consulting the native run table.
    pub fn external_prompt_by_id(&self, id: u64) -> Result<Option<ExternalPromptReceipt>> {
        self.read(|c| {
            let exists: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM external_prompts WHERE id=?1)",
                [id],
                |r| r.get(0),
            )?;
            if exists {
                Ok(Some(Self::external_prompt_id_in(c, id)?))
            } else {
                Ok(None)
            }
        })
    }

    pub fn external_prompt_id_in(c: &Connection, id: u64) -> Result<ExternalPromptReceipt> {
        let (mut receipt, terminal, evidence, outcome, result) = c.query_row(
            "SELECT p.*,r.state,r.created_at,r.updated_at FROM external_prompts p JOIN external_runs r USING(session,generation,op) WHERE p.id=?1", [id], |r| {
                Ok((ExternalPromptReceipt {
                    id, key: ExternalRunKey { session: r.get("session")?, generation: r.get("generation")?, op: r.get("op")? },
                    request: r.get("request_key")?, fingerprint: r.get("fingerprint")?, user_entry: r.get("user_entry")?, cursor: r.get("cursor")?,
                    mode: match r.get::<_, String>("mode")?.as_str() { "fresh" => ExternalPromptMode::Fresh, "resume" => ExternalPromptMode::Resume, _ => return Err(rusqlite::Error::InvalidQuery) },
                    state: ExternalRunState::parse(&r.get::<_, String>("state")?)?, created_at_ms: r.get("created_at")?, updated_at_ms: r.get("updated_at")?,
                    terminal: None, evidence: None, outcome: None, result: None,
                }, r.get::<_, Option<String>>("terminal")?, r.get::<_, Option<String>>("evidence")?, r.get::<_, Option<String>>("outcome")?, r.get::<_, Option<String>>("result")?))
            })?;
        receipt.terminal = terminal
            .map(|s| parse_json(&s, "external terminal"))
            .transpose()?;
        receipt.evidence = evidence
            .map(|s| parse_json(&s, "external evidence"))
            .transpose()?;
        receipt.outcome = outcome
            .map(|s| parse_json(&s, "external outcome"))
            .transpose()?;
        receipt.result = result
            .map(|s| parse_json(&s, "external result"))
            .transpose()?;
        Ok(receipt)
    }

    pub fn admit_external_prompt_input_in(
        tx: &Transaction<'_>,
        input: &ExternalPromptInput,
        now: i64,
    ) -> Result<(ExternalAdmission, Vec<SessionChange>)> {
        Self::admit_external_prompt_in(
            tx,
            &input.session,
            &input.request,
            &input.text,
            &input.op,
            input.generation,
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn admit_external_prompt_in(
        tx: &Transaction<'_>,
        session: &str,
        request: &str,
        text: &str,
        op: &str,
        generation: u64,
        now: i64,
    ) -> Result<(ExternalAdmission, Vec<SessionChange>)> {
        if request.is_empty() || request.len() > limits::MAX_KEY_BYTES || text.len() > MAX_TEXT {
            return Err(StoreError::ExecutionConflict);
        }
        use sha2::{Digest, Sha256};
        let fingerprint = format!("{:x}", Sha256::digest(text.as_bytes()));
        // A lost acknowledgement is resolvable even after recovery invalidates the binding.
        // Op/generation are launch identities, not part of the idempotent request payload.
        if let Some(receipt) = Self::external_prompt_in(tx, session, request)? {
            if receipt.fingerprint != fingerprint {
                return Err(StoreError::ExecutionConflict);
            }
            return Ok((
                ExternalAdmission {
                    receipt,
                    newly_admitted: false,
                },
                vec![],
            ));
        }
        let key = ExternalRunKey {
            session: session.into(),
            generation,
            op: op.into(),
        };
        Self::assert_external_binding(tx, &key)?;
        // Legacy ledger rows are not evidence-backed prompt receipts. Their completion
        // cannot authorize resume, nor may their existing context be treated as fresh.
        let unresolved = tx.prepare(
            "SELECT p.id FROM external_runs r LEFT JOIN external_prompts p USING(session,generation,op)
                WHERE r.session=?1 AND (r.op=?2 OR p.id IS NULL OR p.evidence IS NULL OR NOT (
                    (r.state='completed' AND json_extract(p.outcome,'$.reason') IS 'completed'
                        AND json_extract(p.result,'$.status') IS 'done'
                        AND json_extract(p.terminal,'$.outcome') IS 'success')
                    OR (r.state='interrupted' AND json_extract(p.outcome,'$.reason') IS 'modelError'
                        AND json_extract(p.result,'$.status') IS 'unanswered'
                        AND json_extract(p.terminal,'$.outcome') IS 'error')
                    ))",
        )?.query_map(params![session, op], |r| r.get::<_, Option<u64>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in unresolved {
            let prior = Self::external_prompt_id_in(tx, id.ok_or(StoreError::ExecutionConflict)?)?;
            if prior.key.op == op
                || prior.state != ExternalRunState::Interrupted
                || prior.outcome != Some(RunOutcome::Aborted)
                || !matches!(&prior.result, Some(PromptResult::Unanswered { reason, .. }) if reason == "aborted")
                || !prior
                    .evidence
                    .as_ref()
                    .is_some_and(|e| e.allows_cancel_continuation(prior.cursor))
                    && !Self::recovered_cli_stop_in(tx, &prior, generation)?
            {
                return Err(StoreError::ExecutionConflict);
            }
        }
        let completed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM external_runs WHERE session=?1)",
            [session],
            |r| r.get(0),
        )?;
        tx.execute("INSERT INTO external_runs(session,generation,op,state,created_at,updated_at) VALUES(?1,?2,?3,'admitted',?4,?4)", params![session,generation,op,now])?;
        let entry = Self::append_entry_owned(
            tx,
            session,
            now,
            None,
            EntryKind::ExternalMessage {
                backend: ExecutionBackend::LocalClaudeCli,
                role: ExternalRole::User,
                source_message_id: None,
                request: Some(request.into()),
                blocks: vec![ExternalBlock::Text { text: text.into() }],
                partial: false,
            },
        )?;
        tx.execute("INSERT INTO external_prompts(session,generation,op,request_key,fingerprint,user_entry,mode) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![session,generation,op,request,fingerprint,entry.id,if completed {"resume"} else {"fresh"}])?;
        let id = tx.last_insert_rowid() as u64;
        // Entry.run carries the external protocol ID, but there is intentionally no native run.
        tx.execute(
            "UPDATE entries SET run=?3 WHERE session=?1 AND id=?2",
            params![session, entry.id, id],
        )?;
        let entry = Entry {
            run: Some(id),
            ..entry
        };
        let receipt = Self::external_prompt_id_in(tx, id)?;
        let changes = vec![
            SessionChange::EntryAppended { entry },
            SessionChange::RunChanged {
                run: receipt.run_view(),
            },
        ];
        Ok((
            ExternalAdmission {
                receipt,
                newly_admitted: true,
            },
            changes,
        ))
    }

    /// Strict CAS: duplicate launch/run acknowledgements are conflicts, never launch grants.
    pub fn begin_external_launch_in(
        tx: &Transaction<'_>,
        key: &ExternalRunKey,
        now: i64,
    ) -> Result<(ExternalPromptReceipt, Vec<SessionChange>)> {
        Self::external_prompt_cas_in(
            tx,
            key,
            ExternalRunState::Admitted,
            ExternalRunState::LaunchIntent,
            now,
        )
    }

    pub fn begin_external_run_in(
        tx: &Transaction<'_>,
        key: &ExternalRunKey,
        now: i64,
    ) -> Result<(ExternalPromptReceipt, Vec<SessionChange>)> {
        Self::external_prompt_cas_in(
            tx,
            key,
            ExternalRunState::LaunchIntent,
            ExternalRunState::Running,
            now,
        )
    }

    fn external_prompt_cas_in(
        tx: &Transaction<'_>,
        key: &ExternalRunKey,
        expected: ExternalRunState,
        next: ExternalRunState,
        now: i64,
    ) -> Result<(ExternalPromptReceipt, Vec<SessionChange>)> {
        Self::assert_external_binding(tx, key)?;
        let receipt =
            Self::external_prompt_for_run_in(tx, key)?.ok_or(StoreError::ExecutionConflict)?;
        let n = tx.execute("UPDATE external_runs SET state=?4,updated_at=MAX(updated_at,?5) WHERE session=?1 AND generation=?2 AND op=?3 AND state=?6", params![key.session,key.generation,key.op,next.as_str(),now,expected.as_str()])?;
        if n != 1 {
            return Err(StoreError::ExecutionConflict);
        }
        // Admission already published Running; no duplicate protocol phase notification.
        Ok((Self::external_prompt_id_in(tx, receipt.id)?, vec![]))
    }

    /// Persist this intent before signalling any cancellation token. Repeated Stop is a no-op.
    pub fn stop_external_prompt_in(
        tx: &Transaction<'_>,
        key: &ExternalRunKey,
        now: i64,
    ) -> Result<(ExternalPromptReceipt, Vec<SessionChange>)> {
        validate_key(key)?;
        let receipt =
            Self::external_prompt_for_run_in(tx, key)?.ok_or(StoreError::ExecutionConflict)?;
        Self::revoke_external_permissions_for_run_in(tx, key, now)?;
        if receipt.state == ExternalRunState::Stopping || !receipt.state.active() {
            return Ok((receipt, vec![]));
        }
        Self::assert_external_binding(tx, key)?;
        let (receipt, _) =
            Self::external_prompt_cas_in(tx, key, receipt.state, ExternalRunState::Stopping, now)?;
        let changes = vec![SessionChange::RunChanged {
            run: receipt.run_view(),
        }];
        Ok((receipt, changes))
    }

    pub fn project_external_event_in(
        tx: &Transaction<'_>,
        key: &ExternalRunKey,
        cursor: u64,
        event: &Event,
        now: i64,
    ) -> Result<Vec<SessionChange>> {
        Self::assert_external_binding(tx, key)?;
        let receipt =
            Self::external_prompt_for_run_in(tx, key)?.ok_or(StoreError::ExecutionConflict)?;
        if !matches!(
            receipt.state,
            ExternalRunState::Running | ExternalRunState::Stopping
        ) || receipt.terminal.is_some()
            || cursor > i64::MAX as u64
            || receipt.cursor.checked_add(1) != Some(cursor)
            || (receipt.cursor == 0) != matches!(event, Event::Initialized)
        {
            return Err(StoreError::ExecutionConflict);
        }
        let mut changes = vec![];
        match event {
            Event::Initialized | Event::Context { .. } => {}
            Event::ExternalToolObserved { .. } | Event::ExternalToolResult { .. } => {
                changes.extend(Self::project_external_action_in(
                    tx, key, &receipt, event, now,
                )?);
            }
            Event::Terminal {
                outcome,
                usage,
                metadata,
            } => {
                let terminal = ExternalTerminal {
                    metadata: metadata.clone(),
                    outcome: match outcome {
                        Outcome::Success => ExternalTerminalOutcome::Success,
                        Outcome::Error => ExternalTerminalOutcome::Error,
                        Outcome::Unknown => ExternalTerminalOutcome::Unknown,
                    },
                    usage: usage.map(|u| ExternalTerminalUsage {
                        input_tokens: u.input_tokens,
                        output_tokens: u.output_tokens,
                        cache_read_input_tokens: u.cache_read_input_tokens,
                        cache_creation_input_tokens: u.cache_creation_input_tokens,
                        reasoning_tokens: u.reasoning_tokens,
                        cache_write_1h: u.cache_write_1h,
                    }),
                };
                tx.execute(
                    "UPDATE external_prompts SET terminal=?2 WHERE id=?1",
                    params![receipt.id, json(&terminal)],
                )?;
            }
            Event::BlockDelta {
                message_id,
                block_index,
                kind,
                offset: _,
                text,
            }
            | Event::BlockSnapshot {
                message_id,
                block_index,
                kind,
                text,
            } => {
                if message_id.is_empty()
                    || message_id.len() > 128
                    || !message_id.bytes().all(|b| b.is_ascii_graphic())
                    || *block_index >= MAX_BLOCKS
                    || text.len() > MAX_TEXT
                {
                    return Err(StoreError::ExecutionConflict);
                }
                let spent: usize = tx.query_row(
                    "SELECT projected_bytes FROM external_prompts WHERE id=?1",
                    [receipt.id],
                    |r| r.get(0),
                )?;
                if text.len() > MAX_TEXT - spent {
                    return Err(StoreError::ExecutionConflict);
                }
                let mapping: Option<(u64,String)> = tx.query_row("SELECT entry,final_blocks FROM external_projection_messages WHERE prompt=?1 AND message_id=?2", params![receipt.id,message_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
                let (mut entry, mut finals) = if let Some((id, finals)) = mapping {
                    (
                        Some(Self::entry_in(tx, &key.session, id)?.ok_or_else(|| {
                            StoreError::Corrupt("missing external projection".into())
                        })?),
                        parse_json::<Vec<usize>>(&finals, "external final blocks")?,
                    )
                } else {
                    (None, vec![])
                };
                let mut blocks = match entry.as_ref().map(|e| &e.kind) {
                    Some(EntryKind::ExternalMessage { blocks, .. }) => blocks.clone(),
                    None => vec![],
                    _ => return Err(StoreError::Corrupt("nonexternal projection entry".into())),
                };
                if *block_index > blocks.len() {
                    return Err(StoreError::ExecutionConflict);
                }
                let block = match kind {
                    TextKind::Text => ExternalBlock::Text { text: text.clone() },
                    TextKind::Thinking => ExternalBlock::Thinking { text: text.clone() },
                };
                if let Event::BlockDelta { offset, .. } = event {
                    if finals.contains(block_index) {
                        return Err(StoreError::ExecutionConflict);
                    }
                    if *block_index == blocks.len() {
                        if *offset != 0 {
                            return Err(StoreError::ExecutionConflict);
                        }
                        blocks.push(block);
                    } else {
                        let target = match (&mut blocks[*block_index], kind) {
                            (ExternalBlock::Text { text }, TextKind::Text)
                            | (ExternalBlock::Thinking { text }, TextKind::Thinking) => text,
                            _ => return Err(StoreError::ExecutionConflict),
                        };
                        if target.len() != *offset {
                            return Err(StoreError::ExecutionConflict);
                        }
                        target.push_str(text);
                    }
                } else {
                    // Ordered authoritative replacement (including terminal reconciliation),
                    // not a replayed delta. It never grants tool or completion authority.
                    if *block_index == blocks.len() {
                        blocks.push(block);
                    } else {
                        if std::mem::discriminant(&blocks[*block_index])
                            != std::mem::discriminant(&block)
                        {
                            return Err(StoreError::ExecutionConflict);
                        }
                        blocks[*block_index] = block;
                    }
                    if !finals.contains(block_index) {
                        finals.push(*block_index);
                    }
                }
                if blocks.iter().map(block_text).map(str::len).sum::<usize>() > MAX_TEXT {
                    return Err(StoreError::ExecutionConflict);
                }
                let kind = EntryKind::ExternalMessage {
                    backend: ExecutionBackend::LocalClaudeCli,
                    role: ExternalRole::Assistant,
                    source_message_id: Some(message_id.clone()),
                    request: Some(receipt.request.clone()),
                    blocks,
                    partial: true,
                };
                if let Some(entry) = entry.as_mut() {
                    entry.kind = kind;
                    Self::replace_entry(tx, &key.session, entry)?;
                    changes.push(SessionChange::EntryReplaced {
                        entry: entry.clone(),
                    });
                } else {
                    let count: usize = tx.query_row(
                        "SELECT COUNT(*) FROM external_projection_messages WHERE prompt=?1",
                        [receipt.id],
                        |r| r.get(0),
                    )?;
                    if count >= 4096 {
                        return Err(StoreError::ExecutionConflict);
                    }
                    let entry =
                        Self::append_entry_owned(tx, &key.session, now, Some(receipt.id), kind)?;
                    tx.execute("INSERT INTO external_projection_messages(prompt,session,message_id,entry) VALUES(?1,?2,?3,?4)", params![receipt.id,key.session,message_id,entry.id])?;
                    changes.push(SessionChange::EntryAppended { entry });
                }
                tx.execute("UPDATE external_projection_messages SET final_blocks=?3 WHERE prompt=?1 AND message_id=?2", params![receipt.id,message_id,json(&finals)])?;
                tx.execute(
                    "UPDATE external_prompts SET projected_bytes=projected_bytes+?2 WHERE id=?1",
                    params![receipt.id, text.len()],
                )?;
            }
        }
        tx.execute(
            "UPDATE external_prompts SET cursor=?2 WHERE id=?1",
            params![receipt.id, cursor],
        )?;
        tx.execute("UPDATE external_runs SET updated_at=MAX(updated_at,?4) WHERE session=?1 AND generation=?2 AND op=?3", params![key.session,key.generation,key.op,now])?;
        Ok(changes)
    }

    pub fn settle_external_prompt_in(
        tx: &Transaction<'_>,
        key: &ExternalRunKey,
        evidence: &ExternalSettlementEvidence,
        now: i64,
    ) -> Result<(ExternalSettlement, Vec<SessionChange>)> {
        Self::settle_external_prompt_impl(tx, key, evidence, now, false)
    }

    /// Startup alone may close an unfinished receipt whose binding was already fenced by a
    /// prior storage/cleanup failure. It never accepts success evidence or restores context.
    pub(super) fn recover_external_prompt_in(
        tx: &Transaction<'_>,
        key: &ExternalRunKey,
        now: i64,
    ) -> Result<(ExternalSettlement, Vec<SessionChange>)> {
        Self::settle_external_prompt_impl(
            tx,
            key,
            &ExternalSettlementEvidence::default(),
            now,
            true,
        )
    }

    fn settle_external_prompt_impl(
        tx: &Transaction<'_>,
        key: &ExternalRunKey,
        evidence: &ExternalSettlementEvidence,
        now: i64,
        recovery: bool,
    ) -> Result<(ExternalSettlement, Vec<SessionChange>)> {
        validate_key(key)?;
        let receipt =
            Self::external_prompt_for_run_in(tx, key)?.ok_or(StoreError::ExecutionConflict)?;
        if !receipt.state.active() {
            return Ok((
                ExternalSettlement {
                    receipt,
                    newly_settled: false,
                },
                vec![],
            ));
        }
        if !recovery {
            Self::assert_external_binding(tx, key)?;
        }
        let clean = evidence.root_reaped && evidence.pipes_drained && !evidence.cleanup_timed_out;
        let completed = receipt.state == ExternalRunState::Running
            && receipt
                .terminal
                .as_ref()
                .is_some_and(|t| t.outcome == ExternalTerminalOutcome::Success)
            && clean
            && evidence.projection_finished
            && evidence.supervisor_stop.is_none()
            && evidence.projection_error.is_none()
            && evidence.rejected_record.is_none()
            && evidence.parse_failure.is_none()
            && evidence.input_delivered
            && evidence.exit_code == Some(0)
            && !evidence.stop_requested
            && !evidence.cancelled_clean_confirmed;
        if completed {
            Self::assert_external_permissions_resolved(tx, key)?;
            Self::assert_external_actions_resolved(tx, key)?;
        } else {
            Self::revoke_external_permissions_for_run_in(tx, key, now)?;
        }
        // An explicit CLI error with clean, owned completion is a known failed turn,
        // not lost execution. A new instruction may resume; the failed prompt is never retried.
        let mut provider_error = receipt.state == ExternalRunState::Running
            && receipt
                .terminal
                .as_ref()
                .is_some_and(|t| t.outcome == ExternalTerminalOutcome::Error)
            && clean
            && evidence.projection_finished
            && evidence.input_delivered
            && matches!(evidence.exit_code, Some(0 | 1))
            && evidence.supervisor_stop.is_none()
            && evidence.projection_error.is_none()
            && evidence.rejected_record.is_none()
            && evidence.parse_failure.is_none()
            && !evidence.stop_requested
            && !evidence.cancelled_clean_confirmed;
        if provider_error {
            for resolved in [
                Self::assert_external_permissions_resolved(tx, key),
                Self::assert_external_actions_resolved(tx, key),
            ] {
                match resolved {
                    Ok(()) => {}
                    Err(StoreError::ExecutionConflict) => provider_error = false,
                    Err(error) => return Err(error),
                }
            }
        }
        let cancelled = receipt.state == ExternalRunState::Stopping
            && evidence.cancelled_clean_confirmed
            && evidence
                .supervisor_stop
                .is_none_or(|stop| stop == crate::execution::claude_cli::process::Stop::Cancelled)
            && clean;
        let (state, outcome, reason) = if completed {
            (
                ExternalRunState::Completed,
                RunOutcome::Completed,
                "completed",
            )
        } else if provider_error {
            (ExternalRunState::Interrupted, RunOutcome::ModelError {
                detail: "Claude CLI reported an error. This conversation can continue with a new prompt; inspect any prior effects before repeating work.".into(),
            }, "providerError")
        } else if cancelled {
            (
                ExternalRunState::Interrupted,
                RunOutcome::Aborted,
                "aborted",
            )
        } else {
            (
                ExternalRunState::Unknown,
                RunOutcome::Interrupted {
                    detail: unconfirmed_detail(evidence),
                },
                "interrupted",
            )
        };
        let ids = tx
            .prepare(
                "SELECT entry FROM external_projection_messages WHERE prompt=?1 ORDER BY entry",
            )?
            .query_map([receipt.id], |r| r.get::<_, u64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut text = String::new();
        let mut changes = vec![];
        for id in ids {
            let mut entry = Self::entry_in(tx, &key.session, id)?
                .ok_or_else(|| StoreError::Corrupt("missing external entry".into()))?;
            let EntryKind::ExternalMessage {
                blocks, partial, ..
            } = &mut entry.kind
            else {
                return Err(StoreError::Corrupt("nonexternal projection".into()));
            };
            for block in blocks {
                match block {
                    ExternalBlock::Text { text: t } => text.push_str(t),
                    ExternalBlock::Action { state, .. }
                        if !completed && *state == ExternalActionState::Observed =>
                    {
                        *state = ExternalActionState::Unknown;
                    }
                    _ => {}
                }
            }
            *partial = false;
            entry.retired = !completed;
            Self::replace_entry(tx, &key.session, &entry)?;
            changes.push(SessionChange::EntryReplaced { entry });
        }
        if !completed {
            let mut user = Self::entry_in(tx, &key.session, receipt.user_entry)?
                .ok_or_else(|| StoreError::Corrupt("missing external input".into()))?;
            user.retired = true;
            Self::replace_entry(tx, &key.session, &user)?;
            changes.push(SessionChange::EntryReplaced { entry: user });
        }
        let result = if completed {
            PromptResult::Done { text, reason: None }
        } else {
            PromptResult::Unanswered {
                text: (!text.is_empty()).then_some(text),
                reason: reason.into(),
            }
        };
        tx.execute(
            "UPDATE external_prompts SET evidence=?2,outcome=?3,result=?4 WHERE id=?1",
            params![receipt.id, json(evidence), json(&outcome), json(&result)],
        )?;
        tx.execute("UPDATE external_runs SET state=?4,updated_at=MAX(updated_at,?5) WHERE session=?1 AND generation=?2 AND op=?3", params![key.session,key.generation,key.op,state.as_str(),now])?;
        let continuable_cancel = cancelled && evidence.allows_cancel_continuation(receipt.cursor);
        if !completed && !provider_error {
            if continuable_cancel {
                // Unfinished actions remain unknown forever; preserving context must not
                // leave them running or grant permission to retry the stopped request.
                Self::mark_external_actions_unknown(tx, key, now)?;
            } else {
                Self::invalidate_external_binding(tx, key, now)?;
            }
        }
        if let RunOutcome::Interrupted { detail } | RunOutcome::ModelError { detail } = &outcome {
            // Same transaction as settlement, once only. Survives snapshot replacement,
            // navigation and restart; never model context, a retry or execution authority.
            let entry = Self::append_entry_owned(
                tx,
                &key.session,
                now,
                Some(receipt.id),
                EntryKind::Notice {
                    level: pipkin_engine_protocol::NoticeLevel::Error,
                    text: detail.clone(),
                },
            )?;
            changes.push(SessionChange::EntryAppended { entry });
        }
        if completed && evidence.presentation_incomplete {
            let diagnostic = evidence
                .stream_diagnostic
                .map(|d| format!(" Display issue: {} ({:?}).", d.problem, d.record))
                .unwrap_or_default();
            let entry = Self::append_entry_owned(
                tx,
                &key.session,
                now,
                Some(receipt.id),
                EntryKind::Notice {
                    level: pipkin_engine_protocol::NoticeLevel::Warning,
                    text: format!(
                        "Claude CLI completed, but some live output could not be displayed. Available complete output was reconciled; this conversation can continue.{diagnostic}"
                    ),
                },
            )?;
            changes.push(SessionChange::EntryAppended { entry });
        }
        changes.push(SessionChange::RunChanged { run: None });
        changes.push(SessionChange::RunSettled {
            run: receipt.id,
            request: Some(receipt.request),
            outcome,
        });
        Ok((
            ExternalSettlement {
                receipt: Self::external_prompt_id_in(tx, receipt.id)?,
                newly_settled: true,
            },
            changes,
        ))
    }

    /// Protect the phase-1 raw ledger APIs from bypassing receipt evidence/projection rules.
    pub(super) fn assert_raw_external_run(c: &Connection, key: &ExternalRunKey) -> Result<()> {
        if Self::external_prompt_for_run_in(c, key)?.is_some() {
            return Err(StoreError::ExecutionConflict);
        }
        Ok(())
    }
}

/// Only closed diagnostic categories: never raw records, stderr, schema labels or values.
fn unconfirmed_detail(evidence: &ExternalSettlementEvidence) -> String {
    let mut detail = "External prompt outcome is unconfirmed.".to_owned();
    if let Some(error) = evidence.projection_error {
        detail.push_str(&format!(" CLI output was rejected: {error}."));
    }
    if let Some(record) = evidence.rejected_record {
        detail.push_str(&format!(" Record: {record:?}."));
    }
    if let Some(failure) = evidence.parse_failure {
        detail.push_str(&format!(" Parse failure: {:?}.", failure.kind));
    }
    if let Some(stop) = evidence.supervisor_stop {
        detail.push_str(&format!(" Supervisor: {stop:?}."));
    }
    if let Some(code) = evidence.exit_code.filter(|code| *code != 0) {
        detail.push_str(&format!(" CLI exit code: {code}."));
    }
    if !evidence.root_reaped || !evidence.pipes_drained || evidence.cleanup_timed_out {
        detail.push_str(" CLI cleanup was not fully confirmed.");
    }
    detail.push_str(" This conversation cannot resume; do not repeat uncertain work.");
    detail
}

fn block_text(block: &ExternalBlock) -> &str {
    match block {
        ExternalBlock::Text { text } | ExternalBlock::Thinking { text } => text,
        ExternalBlock::Action { output, .. } => output,
    }
}
