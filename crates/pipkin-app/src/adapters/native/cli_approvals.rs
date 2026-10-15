//! Explicit approval RPC mapping on the existing connection. Never a prompt retry or tool.
use super::*;
use pipkin_core::cli_approvals as core;
use pipkin_core::{CliApprovalEvent, CliApprovalPortRequest};
use pipkin_engine_protocol as wire;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PromptReceipt {
    conn: u64,
    epoch: u64,
    session: String,
    prompt: u64,
}
impl PromptReceipt {
    pub fn capture(
        conn: u64,
        epoch: Option<u64>,
        summary: &SessionSummary,
        run: Option<u64>,
    ) -> Option<Self> {
        if summary.execution_backend != wire::ExecutionBackend::LocalClaudeCli {
            return None;
        }
        let prompt = run.filter(|id| *id > 0 && *id <= i64::MAX as u64)?;
        Some(Self {
            conn,
            epoch: epoch?,
            session: summary.id.clone(),
            prompt,
        })
    }
    fn matches(
        &self,
        conn: u64,
        epoch: Option<u64>,
        session: &str,
        run: &core::CliApprovalRun,
    ) -> bool {
        self.conn == conn
            && Some(self.epoch) == epoch
            && self.session == session
            && run.epoch == self.epoch
            && run.session == self.session
            && run.prompt == self.prompt
    }
}
fn run(r: wire::LocalCliApprovalRun) -> core::CliApprovalRun {
    core::CliApprovalRun {
        epoch: r.epoch,
        session: r.session,
        binding_generation: r.generation,
        op: r.op,
        prompt: r.prompt,
    }
}
fn wire_run(r: &core::CliApprovalRun) -> wire::LocalCliApprovalRun {
    wire::LocalCliApprovalRun {
        epoch: r.epoch,
        session: r.session.clone(),
        generation: r.binding_generation,
        op: r.op.clone(),
        prompt: r.prompt,
    }
}
fn id(i: wire::LocalCliApprovalId) -> core::CliApprovalId {
    core::CliApprovalId {
        run: run(i.run),
        invocation: i.invocation,
        arguments_sha256: i.arguments_sha256,
    }
}
fn wire_id(i: &core::CliApprovalId) -> wire::LocalCliApprovalId {
    wire::LocalCliApprovalId {
        run: wire_run(&i.run),
        invocation: i.invocation.clone(),
        arguments_sha256: i.arguments_sha256.clone(),
    }
}
fn decision(d: wire::LocalCliApprovalDecision) -> core::CliApprovalDecision {
    match d {
        wire::LocalCliApprovalDecision::AllowOnce => core::CliApprovalDecision::AllowOnce,
        wire::LocalCliApprovalDecision::Deny => core::CliApprovalDecision::Deny,
    }
}
fn wire_decision(d: core::CliApprovalDecision) -> wire::LocalCliApprovalDecision {
    match d {
        core::CliApprovalDecision::AllowOnce => wire::LocalCliApprovalDecision::AllowOnce,
        core::CliApprovalDecision::Deny => wire::LocalCliApprovalDecision::Deny,
    }
}
fn receipt(r: wire::LocalCliApprovalReceipt) -> core::CliApprovalReceipt {
    use core::CliApprovalReceiptState as T;
    use wire::LocalCliApprovalState as S;
    core::CliApprovalReceipt {
        id: id(r.id),
        decision: r.decision.map(decision),
        state: match r.state {
            S::Pending => T::Pending,
            S::Granted => T::Granted,
            S::Consumed => T::Consumed,
            S::Denied => T::Denied,
            S::Expired => T::Expired,
            S::Revoked => T::Revoked,
        },
    }
}
fn reviews(
    rows: Vec<wire::LocalCliApprovalReview>,
    scope: &core::CliApprovalRun,
) -> Option<Vec<core::CliApprovalReview>> {
    if rows.len() > core::CLI_APPROVAL_PENDING
        || rows.iter().any(|r| {
            r.id.run != wire_run(scope)
                || r.id.invocation.len() != 64
                || r.id.arguments_sha256.len() != 64
                || r.arguments.len() > core::CLI_APPROVAL_DISPLAY_BYTES
        })
    {
        return None;
    }
    Some(
        rows.into_iter()
            .map(|r| core::CliApprovalReview {
                id: id(r.id),
                arguments: r.arguments,
                expires_at_ms: r.expires_at_ms,
            })
            .collect(),
    )
}
impl Live {
    fn approval_source(
        &self,
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        active: bool,
    ) -> Option<&SessionSummary> {
        if !self.cli_approvals_supported
            || worker.attachment_generations.get(&conversation) != Some(&generation)
        {
            return None;
        }
        self.session_of(conversation).filter(|s| {
            s.execution_backend == wire::ExecutionBackend::LocalClaudeCli
                && s.local_cli_mode == Some(wire::LocalCliMode::RestrictedWrite)
                && (!active || s.blocked.is_none())
        })
    }
    fn approval_proof(
        &self,
        worker: &Worker,
        conversation: ConversationId,
        generation: u64,
        op: OperationId,
    ) -> Option<PromptReceipt> {
        let r = worker
            .runs
            .get(&op)
            .filter(|r| r.conversation == conversation && r.generation == generation)?;
        r.cli_receipt
            .clone()
            .filter(|p| p.conn == worker.conn && Some(p.epoch) == self.seq.epoch())
    }
    pub(super) fn cli_approval_request(
        &mut self,
        worker: &mut Worker,
        request: CliApprovalPortRequest,
    ) {
        let (conversation, generation, op) = request.tuple();
        let event = self
            .cli_approval_rpc(worker, &request)
            .unwrap_or_else(|| request.failed());
        worker.emit_op(
            conversation,
            generation,
            op,
            EventKind::CliApproval(Box::new(event)),
        );
    }
    fn cli_approval_rpc(
        &self,
        worker: &Worker,
        request: &CliApprovalPortRequest,
    ) -> Option<CliApprovalEvent> {
        match request {
            CliApprovalPortRequest::Discover(q) => {
                let summary = self.approval_source(worker, q.conversation, q.generation, true)?;
                let proof = self.approval_proof(worker, q.conversation, q.generation, q.run_op)?;
                if proof.session != summary.id {
                    return None;
                }
                let Reply::LocalCliApprovals {
                    run: identity,
                    reviews: rows,
                } = self
                    .call(Call::LocalCliApprovals {
                        session: summary.id.clone(),
                    })
                    .ok()?
                else {
                    return None;
                };
                let (identity, rows) = match identity {
                    Some(r) => {
                        let r = run(r);
                        if !r.valid()
                            || !proof.matches(worker.conn, self.seq.epoch(), &summary.id, &r)
                        {
                            return None;
                        }
                        let rows = reviews(rows, &r)?;
                        (Some(r), rows)
                    }
                    None if rows.is_empty() => (None, vec![]),
                    None => return None,
                };
                Some(CliApprovalEvent::Discovered {
                    query: q.clone(),
                    run: identity,
                    reviews: rows,
                })
            }
            CliApprovalPortRequest::Known(r) => {
                let c = &r.context;
                let lookup = matches!(r.action, core::CliApprovalAction::Lookup { .. });
                let summary =
                    self.approval_source(worker, c.conversation, c.generation, !lookup)?;
                if !c.run.valid()
                    || summary.id != c.run.session
                    || self.seq.epoch() != Some(c.run.epoch)
                {
                    return None;
                }
                if !lookup
                    && !self
                        .approval_proof(worker, c.conversation, c.generation, c.run_op)?
                        .matches(worker.conn, self.seq.epoch(), &summary.id, &c.run)
                {
                    return None;
                }
                let call = match &r.action {
                    core::CliApprovalAction::List => Call::LocalCliApprovals {
                        session: summary.id.clone(),
                    },
                    core::CliApprovalAction::Decide { id, decision } if id.run == c.run => {
                        Call::DecideLocalCliApproval {
                            id: wire_id(id),
                            decision: wire_decision(*decision),
                        }
                    }
                    core::CliApprovalAction::Lookup { id } if id.run == c.run => {
                        Call::LookupLocalCliApproval { id: wire_id(id) }
                    }
                    _ => return None,
                };
                let reply = self.call(call).ok()?;
                let result = match (&r.action, reply) {
                    (
                        core::CliApprovalAction::List,
                        Reply::LocalCliApprovals {
                            run: identity,
                            reviews: rows,
                        },
                    ) => {
                        let identity = identity.map(run);
                        if identity.as_ref() != Some(&c.run)
                            && !(identity.is_none() && rows.is_empty())
                        {
                            return None;
                        }
                        core::CliApprovalResult::Listed {
                            run: identity,
                            reviews: reviews(rows, &c.run)?,
                        }
                    }
                    (
                        core::CliApprovalAction::Decide { id: expected, .. }
                        | core::CliApprovalAction::Lookup { id: expected },
                        Reply::LocalCliApproval { receipt: record },
                    ) => {
                        if record.as_ref().is_some_and(|r| r.id != wire_id(expected)) {
                            return None;
                        }
                        core::CliApprovalResult::Receipt(record.map(receipt))
                    }
                    _ => return None,
                };
                Some(CliApprovalEvent::Response(core::CliApprovalResponse {
                    context: c.clone(),
                    op: r.op,
                    result,
                }))
            }
        }
    }
}
