//! Background Git observations bind their own operation and captured source, not selection.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Source {
    pub conn: u64,
    pub epoch: Option<u64>,
    pub conversation: ConversationId,
    pub generation: u64,
    pub session: String,
    pub cwd: Option<String>,
    pub full: bool,
    pub revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Scope {
    pub source: Source,
    pub op: u64,
}
impl Scope {
    pub fn is_pending(&self, pending: Option<&Self>, conn: u64) -> bool {
        pending == Some(self) && self.source.conn == conn
    }
    pub fn belongs_to(&self, source: &Source) -> bool {
        self.source == *source
    }
}
impl Source {
    pub fn capture(conn: u64, epoch: Option<u64>, current: &Current) -> Self {
        Self {
            conn,
            epoch,
            conversation: current.conversation,
            generation: current.generation,
            session: current.session.clone(),
            cwd: current.cwd.clone(),
            full: current.full_workspace,
            revision: current.workspace_revision,
        }
    }
}
pub(super) fn full_workspace(summary: &SessionSummary) -> bool {
    matches!(mapping::execution_context(summary), EventKind::ExecutionContext { profile, .. } if profile.is_local_cli())
}
pub(super) fn external_action(change: &SessionChange) -> bool {
    use pipkin_engine_protocol::{EntryKind, ExternalBlock};
    match change {
        SessionChange::EntryAppended { entry } | SessionChange::EntryReplaced { entry } => {
            matches!(&entry.kind, EntryKind::ExternalMessage { blocks, .. } if blocks.iter().any(|b| matches!(b, ExternalBlock::Action { .. })))
        }
        _ => false,
    }
}
impl Current {
    pub(super) fn invalidate_workspace(&mut self, worker: &Worker) {
        self.workspace_ready = self.workspace_revision.checked_add(1).is_some_and(|next| {
            self.workspace_revision = next;
            true
        });
        self.last_workspace = None;
        self.publish_empty_changes = true;
        self.next_changes_scan = Instant::now();
        worker.emit(
            self.conversation,
            self.generation,
            EventKind::ChangesScanState(if self.workspace_ready {
                ChangesState::Loading
            } else {
                ChangesState::Unavailable(
                    "Changes observation capacity exhausted; reopen the conversation.".into(),
                )
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observation_identity_cannot_survive_replacement_connection_scope_or_operation() {
        let scope = Scope {
            source: Source {
                conn: 1,
                epoch: Some(7),
                conversation: ConversationId(2),
                generation: 3,
                session: "captured".into(),
                cwd: Some("/captured".into()),
                full: true,
                revision: 4,
            },
            op: 5,
        };
        for field in 0..9 {
            let mut stale = scope.clone();
            match field {
                0 => stale.source.conn += 1,
                1 => stale.source.conversation = ConversationId(9),
                2 => stale.source.generation += 1,
                3 => stale.source.session = "replacement".into(),
                4 => stale.source.cwd = Some("/replacement".into()),
                5 => stale.source.full = false,
                6 => stale.source.revision += 1,
                7 => stale.source.epoch = Some(8),
                _ => stale.op += 1,
            }
            assert!(!stale.is_pending(Some(&scope), scope.source.conn));
            if field != 8 {
                assert!(!scope.belongs_to(&stale.source));
            }
            assert!(!scope.is_pending(None, scope.source.conn));
            assert!(!scope.is_pending(Some(&scope), scope.source.conn + 1));
        }
    }
}
