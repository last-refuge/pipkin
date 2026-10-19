//! Single-owner, non-recoverable projection cursor. Committed text survives crashes; a cursor
//! never does. Root launch CAS prevents a second producer. No SDK tool observation reaches here.
use super::*;
use crate::execution::claude_cli::{
    orchestration_observer::{Observation, Outcome, TextKind},
    orchestration_policy::MAX_PROJECTED_BYTES,
};
use std::{collections::BTreeMap, sync::Arc};
#[cfg(test)]
mod tests;
#[derive(Clone)]
struct Cell {
    id: u64,
    bytes: usize,
    kind: TextKind,
}
pub(crate) struct Projection {
    lease: Arc<Lease>,
    cells: BTreeMap<(String, usize), Cell>,
    bytes: usize,
    initialized: bool,
    terminal: Option<ExternalTerminal>,
    finished: bool,
}
#[allow(dead_code)] // Bounded publication receiver is staged until SessionRt route activation.
pub(crate) struct Update {
    pub scope: Scope,
    pub revision: u64,
    pub changes: Vec<SessionChange>,
}
impl Projection {
    pub(crate) fn new(lease: Arc<Lease>) -> Self {
        Self {
            lease,
            cells: BTreeMap::new(),
            bytes: 0,
            initialized: false,
            terminal: None,
            finished: false,
        }
    }
    pub(crate) fn terminal(&self) -> Option<ExternalTerminal> {
        self.terminal.clone()
    }
    pub(crate) fn apply(
        &mut self,
        store: &Store,
        scope: &Scope,
        events: Vec<Observation>,
        now: i64,
    ) -> Result<Update> {
        if scope != &self.lease.receipt.scope || self.finished {
            return Err(StoreError::ExecutionConflict);
        }
        let mut cells = self.cells.clone();
        let mut bytes = self.bytes;
        let mut initialized = self.initialized;
        let mut terminal = self.terminal.clone();
        let update = store.tx(|c| {
            let r = owned(c, &self.lease)?;
            let binding = live(c, &scope.session, self.lease.binding)?;
            running(c, &r)?;
            if r.state != State::Running || r.fenced {
                return Err(StoreError::ExecutionConflict);
            }
            let mut changes = Vec::new();
            for event in events {
                if terminal.is_some() {
                    return Err(StoreError::ExecutionConflict);
                }
                let (key, kind, text, offset) = match event {
                    Observation::Context { tokens, compact_pre_tokens } => {
                        if !initialized || tokens.is_some_and(|n|n>i64::MAX as u64) || compact_pre_tokens.is_some_and(|n|n>i64::MAX as u64) { return Err(StoreError::ExecutionConflict); }
                        c.execute("INSERT INTO cli_agent_context(run,tokens,compact_count,pre_tokens) VALUES(?1,?2,?3,?4) ON CONFLICT(run) DO UPDATE SET tokens=excluded.tokens,compact_count=compact_count+excluded.compact_count,pre_tokens=COALESCE(excluded.pre_tokens,pre_tokens)",params![scope.run,tokens,usize::from(compact_pre_tokens.is_some()),compact_pre_tokens])?;
                        continue;
                    }
                    Observation::Initialized => {
                        if initialized {
                            return Err(StoreError::ExecutionConflict);
                        }
                        initialized = true;
                        continue;
                    }
                    Observation::Terminal { outcome, usage, metadata } => {
                        if !initialized {
                            return Err(StoreError::ExecutionConflict);
                        }
                        terminal = Some(ExternalTerminal {
                            metadata,
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
                        });
                        continue;
                    }
                    Observation::Delta {
                        message_id,
                        block_index,
                        kind,
                        text,
                        offset,
                    } => ((message_id, block_index), kind, text, Some(offset)),
                    Observation::Snapshot {
                        message_id,
                        block_index,
                        kind,
                        text,
                    } => ((message_id, block_index), kind, text, None),
                };
                bytes = bytes
                    .checked_add(text.len())
                    .filter(|n| *n <= MAX_PROJECTED_BYTES)
                    .ok_or(StoreError::ExecutionConflict)?;
                if !initialized || key.0.is_empty() || key.0.len() > 128 || key.1 >= 4096 {
                    return Err(StoreError::ExecutionConflict);
                }
                if !cells.contains_key(&key) {
                    if cells.len() >= 4096 || offset.is_some_and(|n| n != 0) {
                        return Err(StoreError::ExecutionConflict);
                    }
                    let block = match kind {
                        TextKind::Text => Block::text(""),
                        TextKind::Thinking => Block::Thinking {
                            thinking: String::new(),
                            signature: None,
                            redacted: false,
                            extra: Default::default(),
                        },
                    };
                    // One entry per SDK display block preserves observed arrival order around
                    // native tool ledger entries; this is not reconstructed SDK causal history.
                    let mut extra = serde_json::Map::new();
                    extra.insert("sdkMessageId".into(), Value::String(key.0.clone()));
                    extra.insert("sdkBlock".into(), serde_json::json!(key.1));
                    extra.insert("usageUnavailable".into(), Value::Bool(true));
                    if let Some(thinking) = &binding.create.thinking { extra.insert("cliRequestedEffort".into(), thinking.clone().into()); }
                    let entry = Store::append_entry(
                        c,
                        &scope.session,
                        now,
                        Some(scope.run),
                        &EntryKind::Message {
                            message: Message::Assistant {
                                content: vec![block],
                                api: "claude-cli-native".into(),
                                provider: "claude-cli".into(),
                                model: binding.create.model.clone(),
                                usage: Usage::default(),
                                stop_reason: StopReason::Stop,
                                error_message: None,
                                timestamp: now,
                                extra,
                            },
                            mode: InputMode::Prompt,
                            request: None,
                            partial: true,
                        },
                    )?;
                    cells.insert(
                        key.clone(),
                        Cell {
                            id: entry.id,
                            bytes: 0,
                            kind,
                        },
                    );
                    changes.push(SessionChange::EntryAppended { entry });
                }
                let cell = cells.get_mut(&key).ok_or(StoreError::ExecutionConflict)?;
                if cell.kind != kind {
                    return Err(StoreError::ExecutionConflict);
                }
                if let Some(offset) = offset {
                    if offset != cell.bytes {
                        return Err(StoreError::ExecutionConflict);
                    }
                    cell.bytes += text.len();
                    let delta = match kind {
                        TextKind::Text => Delta::Text { block: 0, text },
                        TextKind::Thinking => Delta::Thinking { block: 0, text },
                    };
                    Store::append_chunks(c, &scope.session, cell.id, std::slice::from_ref(&delta))?;
                    changes.push(SessionChange::EntryDelta { id: cell.id, delta });
                } else {
                    cell.bytes = text.len();
                    let mut entry = Store::entry_in(c, &scope.session, cell.id)?
                        .ok_or(StoreError::ExecutionConflict)?;
                    let EntryKind::Message {
                        message: Message::Assistant { content, .. },
                        ..
                    } = &mut entry.kind
                    else {
                        return Err(StoreError::ExecutionConflict);
                    };
                    content[0] = match kind {
                        TextKind::Text => Block::text(text),
                        TextKind::Thinking => Block::Thinking {
                            thinking: text,
                            signature: None,
                            redacted: false,
                            extra: Default::default(),
                        },
                    };
                    Store::replace_entry(c, &scope.session, &entry)?;
                    c.execute(
                        "DELETE FROM entry_chunks WHERE session=?1 AND entry=?2",
                        params![scope.session, cell.id],
                    )?;
                    changes.push(SessionChange::EntryReplaced { entry });
                }
            }
            let revision = Store::bump(c, &scope.session, now)?;
            Ok(Update {
                scope: scope.clone(),
                revision,
                changes,
            })
        })?;
        self.cells = cells;
        self.bytes = bytes;
        self.initialized = initialized;
        self.terminal = terminal;
        Ok(update)
    }
    pub(crate) fn finish(&mut self, store: &Store, now: i64, incomplete: bool) -> Result<Update> {
        if self.finished
            || self
                .terminal
                .as_ref()
                .is_none_or(|t| t.outcome != ExternalTerminalOutcome::Success)
        {
            return Err(StoreError::ExecutionConflict);
        }
        let scope = self.lease.receipt.scope.clone();
        let update = store.tx(|c| {
            let r = owned(c, &self.lease)?;
            live(c, &scope.session, self.lease.binding)?;
            running(c, &r)?;
            if r.state != State::Running || r.fenced {
                return Err(StoreError::ExecutionConflict);
            }
            let mut changes = Vec::new();
            for cell in self.cells.values() {
                let mut entry = Store::entry_in(c, &scope.session, cell.id)?
                    .ok_or(StoreError::ExecutionConflict)?;
                let EntryKind::Message { partial, .. } = &mut entry.kind else {
                    return Err(StoreError::ExecutionConflict);
                };
                *partial = false;
                // A CLI invocation can emit many display blocks. Publish its usage once,
                // on the last entry, rather than charging every thinking/text block.
                if Some(cell.id) == self.cells.values().map(|c| c.id).max()
                    && let EntryKind::Message { message: Message::Assistant { extra, usage, .. }, .. } = &mut entry.kind
                    && let Some(terminal) = &self.terminal
                {
                    extra.insert("cliInvocationUsage".into(), serde_json::to_value(&terminal.usage).map_err(|_| StoreError::ExecutionConflict)?);
                    if let Some(metadata) = &terminal.metadata {
                        extra.insert("cliUsageMetadata".into(), serde_json::to_value(metadata).map_err(|_| StoreError::ExecutionConflict)?);
                    }
                    extra.insert("costUnavailable".into(), true.into());
                    if let Some(reported) = &terminal.usage
                        && let (Some(input), Some(output), Some(cache_read), Some(cache_write)) = (reported.input_tokens, reported.output_tokens, reported.cache_read_input_tokens, reported.cache_creation_input_tokens)
                        && let Some(total) = input.checked_add(output).and_then(|n|n.checked_add(cache_read)).and_then(|n|n.checked_add(cache_write))
                    {
                        *usage = Usage {input, output, cache_read, cache_write, total_tokens:total, reasoning:reported.reasoning_tokens, cache_write_1h:reported.cache_write_1h, ..Usage::default()};
                        extra.insert("usageUnavailable".into(), false.into());
                    }
                }
                Store::replace_entry(c, &scope.session, &entry)?;
                changes.push(SessionChange::EntryReplaced { entry });
            }
            if incomplete {
                let entry=Store::append_entry(c,&scope.session,now,Some(scope.run),&EntryKind::Notice {level:NoticeLevel::Warning,text:"Some Claude CLI output could not be displayed. Native tool receipts and execution settlement are recorded separately.".into()})?;
                changes.push(SessionChange::EntryAppended {entry});
            }
            Ok(Update {
                scope: scope.clone(),
                revision: Store::bump(c, &scope.session, now)?,
                changes,
            })
        })?;
        self.finished = true;
        Ok(update)
    }
}
