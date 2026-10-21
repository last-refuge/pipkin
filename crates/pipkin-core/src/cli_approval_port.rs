//! Dedicated CLI approval command/event port. Never native questions or prompt operations.
use crate::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliApprovalQuery {
    pub conversation: ConversationId,
    pub generation: u64,
    pub run_op: OperationId,
    pub op: OperationId,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliApprovalCommand {
    Discover(ConversationId),
    Act {
        context: CliApprovalContext,
        action: Box<CliApprovalAction>,
    },
    /// Explicitly move retired receipts to the current attachment for read-only recovery.
    Reattach(ConversationId),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliApprovalPortRequest {
    Discover(CliApprovalQuery),
    Known(CliApprovalRequest),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliApprovalEvent {
    Discovered {
        query: CliApprovalQuery,
        run: Option<CliApprovalRun>,
        reviews: Vec<CliApprovalReview>,
    },
    DiscoveryFailed(CliApprovalQuery),
    Response(CliApprovalResponse),
}
impl CliApprovalPortRequest {
    pub fn tuple(&self) -> (ConversationId, u64, OperationId) {
        match self {
            Self::Discover(q) => (q.conversation, q.generation, q.op),
            Self::Known(r) => (r.context.conversation, r.context.generation, r.op),
        }
    }
    pub fn failed(self) -> CliApprovalEvent {
        match self {
            Self::Discover(q) => CliApprovalEvent::DiscoveryFailed(q),
            Self::Known(r) => CliApprovalEvent::Response(CliApprovalResponse {
                context: r.context,
                op: r.op,
                result: CliApprovalResult::Failed,
            }),
        }
    }
}
#[derive(Debug, Default)]
pub struct CliApprovalPort {
    available: bool,
    pending: BTreeMap<ConversationId, CliApprovalQuery>,
    persisting: BTreeMap<ConversationId, CliApprovalRequest>,
}
impl AppState {
    pub fn restore_cli_approval_recovery(
        &mut self,
        records: Result<Vec<CliApprovalRecovery>, String>,
    ) {
        if let Some(high) = self.cli_approvals.restore_recovery(records) {
            self.reserve_recovered_operation(high);
        }
    }
    /// Release exactly once after durable commit, never after scope/deadline/authority loss.
    pub fn cli_approval_persisted(
        &mut self,
        request: &CliApprovalRequest,
        result: Result<(), String>,
    ) -> Outcome {
        let mut out = Outcome::default();
        if self
            .cli_approval_port
            .persisting
            .get(&request.context.conversation)
            != Some(request)
        {
            return out;
        }
        self.cli_approval_port
            .persisting
            .remove(&request.context.conversation);
        if result.is_ok()
            && self.cli_approvals.persisted_request_live(request)
            && self.cli_approval_target(
                request.context.conversation,
                request.context.generation,
                Some(request.context.run_op),
            )
        {
            out.effects
                .push(Effect::Backend(BackendRequest::CliApproval(
                    CliApprovalPortRequest::Known(request.clone()),
                )));
        } else {
            self.cli_approvals.apply(CliApprovalResponse {
                context: request.context.clone(),
                op: request.op,
                result: CliApprovalResult::Failed,
            });
            self.notice=Some("CLI decision was not released: recovery could not be saved or the review scope expired. Never resend; inspect files and use read-only lookup only with the original owner connection.".into());
        }
        out.notes.push(Note::Other);
        out
    }
    pub fn cli_approval_receipt_persisted(
        &mut self,
        record: &CliApprovalRecovery,
        result: Result<(), String>,
    ) -> Outcome {
        if result.is_err()
            && self
                .cli_approvals
                .recovery_record(&record.context, &record.id)
                .as_ref()
                == Some(record)
        {
            self.notice=Some("CLI receipt was observed but could not be saved. Restart may retain an unknown outcome. Never resend the decision; a consumed receipt is not proof of delivery or action.".into());
            return Outcome {
                notes: vec![Note::Other],
                ..Default::default()
            };
        }
        Outcome::default()
    }
    pub fn set_cli_approvals_available(&mut self, available: bool) -> Outcome {
        self.cli_approval_port.available = available;
        if !available {
            self.cli_approval_port.pending.clear();
            self.cli_approval_port.persisting.clear();
            self.cli_approvals.retire_all();
        }
        Outcome {
            notes: vec![Note::Other],
            ..Default::default()
        }
    }
    fn cli_approval_target(
        &self,
        id: ConversationId,
        generation: u64,
        run_op: Option<OperationId>,
    ) -> bool {
        self.mode == Mode::Real
            && self.connection.is_ready()
            && self.cli_approval_port.available
            && !self.owned_elsewhere(id)
            && self.conversation(id).is_some_and(|c| {
                c.generation == generation
                    && c.opened
                    && c.execution_profile(self.mode) == ExecutionProfile::LocalCliRestrictedWrite
                    && run_op.is_none_or(|op| {
                        self.read_only.is_none()
                            && c.execution_blocked.is_none()
                            && c.run == RunState::Running { op }
                    })
            })
    }
    pub fn cli_approval_panel_available(&self, id: ConversationId) -> bool {
        self.mode == Mode::Real
            && !self.owned_elsewhere(id)
            && self.conversation(id).is_some_and(|c| {
                c.execution_profile(self.mode) == ExecutionProfile::LocalCliRestrictedWrite
            })
    }
    pub fn cli_approval_reattach_available(&self, id: ConversationId) -> bool {
        self.conversation(id).is_some_and(|c| {
            self.cli_approval_target(id, c.generation, None)
                && !self.cli_approval_port.pending.contains_key(&id)
                && self
                    .cli_approval_context(id)
                    .is_some_and(|old| old.generation != c.generation)
        })
    }
    /// Stop is still the normal run command, but an old panel must never stop a new selection/run.
    pub fn cli_approval_stop_command(
        &self,
        id: ConversationId,
        generation: u64,
        op: Option<OperationId>,
    ) -> Option<Command> {
        if !self.cli_approval_panel_available(id)
            || !self
                .conversation(id)
                .is_some_and(|c| c.generation == generation && op.is_some() && c.run.op() == op)
        {
            return None;
        }
        if self.dashboard.open {
            let context = self
                .cli_approval_context(id)
                .filter(|c| c.generation == generation && Some(c.run_op) == op)?;
            let row = self.dashboard.selected_row().filter(|r| {
                r.conversation == Some(id)
                    && r.worker.session == context.run.session
                    && r.worker
                        .run
                        .as_ref()
                        .is_some_and(|run| run.id == context.run.prompt)
            })?;
            if self
                .dashboard_action_unavailable(crate::dashboard::Action::Stop)
                .is_some()
            {
                return None;
            }
            return Some(Command::Dashboard(crate::dashboard::Command::Act {
                session: row.worker.session.clone(),
                generation: self.dashboard.detail.generation,
                revision: row.worker.revision,
                draft_revision: self.dashboard_draft().1,
                action: crate::dashboard::Action::Stop,
            }));
        }
        if self.selected != Some(id) {
            return None;
        }
        let a = self.availability();
        if a.cancel {
            Some(Command::Cancel)
        } else if a.retry_stop {
            Some(Command::RetryStop)
        } else {
            None
        }
    }
    pub fn cli_approval_context(&self, id: ConversationId) -> Option<&CliApprovalContext> {
        self.cli_approvals.contexts().find(|c| c.conversation == id)
    }
    pub fn cli_approval_discovery_available(&self, id: ConversationId) -> bool {
        let Some(c) = self.conversation(id) else {
            return false;
        };
        let Some(op) = c.run.op() else {
            return false;
        };
        if self.cli_approvals.recovery_unavailable()
            || !self.cli_approval_target(id, c.generation, Some(op))
            || self.cli_approval_port.pending.contains_key(&id)
        {
            return false;
        }
        if let Some(context) = self.cli_approval_context(id) {
            if context.generation == c.generation && context.run_op == op {
                return self.cli_approvals.availability(context, None).list;
            }
            if self.cli_approvals.busy(id)
                || self.cli_approvals.items(context).iter().any(|i| {
                    matches!(
                        i.status,
                        CliApprovalStatus::Unknown | CliApprovalStatus::Submitting
                    )
                })
            {
                return false;
            }
        } else if self.cli_approvals.contexts().count() >= CLI_APPROVAL_CONTEXTS {
            return false;
        }
        self.cli_approval_port.pending.len() < CLI_APPROVAL_CONTEXTS
            && !self.cli_approvals.clock_failed()
    }
    pub fn cli_approval_availability(
        &self,
        context: &CliApprovalContext,
        id: Option<&CliApprovalId>,
    ) -> CliApprovalAvailability {
        if !self.cli_approval_target(context.conversation, context.generation, None)
            || self
                .cli_approval_port
                .pending
                .contains_key(&context.conversation)
        {
            return Default::default();
        }
        let mut a = self.cli_approvals.availability(context, id);
        if !self.cli_approval_target(
            context.conversation,
            context.generation,
            Some(context.run_op),
        ) {
            a.list = false;
            a.decide = false;
        }
        a
    }
    pub(crate) fn reconcile_cli_approval_queries(&mut self) {
        let stale: Vec<_> = self
            .cli_approval_port
            .pending
            .values()
            .filter(|q| !self.cli_approval_target(q.conversation, q.generation, Some(q.run_op)))
            .map(|q| q.conversation)
            .collect();
        for id in stale {
            self.cli_approval_port.pending.remove(&id);
        }
    }
    pub(crate) fn dispatch_cli_approval(&mut self, command: CliApprovalCommand) -> Outcome {
        let mut out = Outcome::default();
        let request = match command {
            CliApprovalCommand::Discover(id) => {
                if !self.cli_approval_discovery_available(id) {
                    return out;
                }
                let c = self.conversation(id).unwrap();
                let (generation, run_op) = (c.generation, c.run.op().unwrap());
                if let Some(context) = self
                    .cli_approval_context(id)
                    .filter(|c| c.generation == generation && c.run_op == run_op)
                    .cloned()
                {
                    return self.dispatch_cli_approval(CliApprovalCommand::Act {
                        context,
                        action: Box::new(CliApprovalAction::List),
                    });
                }
                let q = CliApprovalQuery {
                    conversation: id,
                    generation,
                    run_op,
                    op: self.alloc_op(),
                };
                if !self.cli_approvals.reserve_discovery(q.op) {
                    return out;
                }
                self.cli_approval_port.pending.insert(id, q.clone());
                Some(CliApprovalPortRequest::Discover(q))
            }
            CliApprovalCommand::Act { context, action } => {
                let id = match action.as_ref() {
                    CliApprovalAction::List => None,
                    CliApprovalAction::Decide { id, .. } | CliApprovalAction::Lookup { id } => {
                        Some(id)
                    }
                };
                let a = self.cli_approval_availability(&context, id);
                if !match action.as_ref() {
                    CliApprovalAction::List => a.list,
                    CliApprovalAction::Decide { .. } => a.decide,
                    CliApprovalAction::Lookup { .. } => a.lookup,
                } {
                    return out;
                }
                let op = self.alloc_op();
                self.cli_approvals
                    .request(&context, op, *action)
                    .map(CliApprovalPortRequest::Known)
            }
            CliApprovalCommand::Reattach(id) => {
                if !self.cli_approval_reattach_available(id) {
                    return out;
                }
                let Some(c) = self.conversation(id) else {
                    return out;
                };
                if !self.cli_approval_target(id, c.generation, None)
                    || self.cli_approval_port.pending.contains_key(&id)
                {
                    return out;
                }
                let generation = c.generation;
                let Some(mut context) = self.cli_approval_context(id).cloned() else {
                    return out;
                };
                self.cli_approvals.invalidate(&context);
                context.generation = generation;
                self.cli_approvals.bind(context);
                None
            }
        };
        if let Some(request) = request {
            if let CliApprovalPortRequest::Known(r) = &request
                && matches!(r.action, CliApprovalAction::Decide { .. })
            {
                self.cli_approval_port
                    .persisting
                    .insert(r.context.conversation, r.clone());
                out.effects
                    .push(Effect::JournalCliApproval(Box::new(r.clone())));
            } else {
                out.effects
                    .push(Effect::Backend(BackendRequest::CliApproval(request)));
            }
        }
        out.notes.push(Note::Other);
        out
    }
    pub(crate) fn apply_cli_approval_event(&mut self, ev: BackendEvent) -> Outcome {
        let mut out = Outcome::default();
        let EventKind::CliApproval(event) = ev.kind else {
            return out;
        };
        let event = *event;
        let query = match &event {
            CliApprovalEvent::Discovered { query, .. }
            | CliApprovalEvent::DiscoveryFailed(query) => Some(query),
            _ => None,
        };
        if let Some(q) = query {
            if (ev.conversation, ev.generation, ev.op) != (q.conversation, q.generation, Some(q.op))
                || self.cli_approval_port.pending.get(&q.conversation) != Some(q)
                || !self.cli_approval_target(q.conversation, q.generation, Some(q.run_op))
            {
                return out;
            }
            self.cli_approval_port.pending.remove(&q.conversation);
        }
        match event {
            CliApprovalEvent::Discovered {
                query,
                run: Some(run),
                reviews,
            } => {
                let context = CliApprovalContext {
                    conversation: query.conversation,
                    generation: query.generation,
                    run_op: query.run_op,
                    run: run.clone(),
                };
                let result = self.cli_approvals.install_discovered(
                    context,
                    query.op,
                    CliApprovalResult::Listed {
                        run: Some(run),
                        reviews,
                    },
                );
                if result != CliApprovalApply::Applied {
                    self.notice = Some(
                        "CLI approval review is unavailable or invalid; no decision was sent."
                            .into(),
                    );
                }
            }
            CliApprovalEvent::Discovered { .. } | CliApprovalEvent::DiscoveryFailed(_) => {
                self.notice = Some("No verified CLI approval review is available. Refresh explicitly; no decision was sent.".into());
            }
            CliApprovalEvent::Response(response) => {
                let c = &response.context;
                if self
                    .cli_approval_port
                    .persisting
                    .get(&c.conversation)
                    .is_some_and(|p| p.context == *c && p.op == response.op)
                {
                    return out;
                }
                if (ev.conversation, ev.generation, ev.op)
                    != (c.conversation, c.generation, Some(response.op))
                    || !self.cli_approval_target(c.conversation, c.generation, None)
                {
                    return out;
                }
                let receipt_id = match &response.result {
                    CliApprovalResult::Receipt(Some(r)) => Some(r.id.clone()),
                    _ => None,
                };
                let context = response.context.clone();
                if self.cli_approvals.apply(response) == CliApprovalApply::Ignored {
                    return out;
                }
                if let Some(record) =
                    receipt_id.and_then(|id| self.cli_approvals.recovery_record(&context, &id))
                    && record.receipt.is_some()
                {
                    out.effects
                        .push(Effect::SaveCliApprovalRecovery(Box::new(record)));
                }
            }
        }
        out.notes.push(Note::Other);
        out
    }
}
