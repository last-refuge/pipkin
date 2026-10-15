//! Durable public attempt/receipt facts, never permission authority or argument text.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliApprovalRecovery {
    pub context: CliApprovalContext,
    pub id: CliApprovalId,
    pub decision: CliApprovalDecision,
    /// Only terminal receipt metadata. Even Consumed is not delivery/action proof.
    pub receipt: Option<CliApprovalReceiptState>,
}
impl CliApprovalRecovery {
    pub fn attempt(request: &CliApprovalRequest) -> Option<Self> {
        let CliApprovalAction::Decide { id, decision } = &request.action else {
            return None;
        };
        Some(Self {
            context: request.context.clone(),
            id: id.clone(),
            decision: *decision,
            receipt: None,
        })
    }
    pub fn valid_set(records: &[Self]) -> bool {
        records.len() <= CLI_APPROVAL_CONTEXTS * CLI_APPROVAL_REQUESTS
            && records.iter().enumerate().all(|(index, r)| {
                r.valid()
                    && records[..index].iter().all(|old| {
                        if old.context.conversation == r.context.conversation {
                            old.context == r.context && old.id.invocation != r.id.invocation
                        } else {
                            old.context.run.session != r.context.run.session
                        }
                    })
                    && records
                        .iter()
                        .filter(|old| old.context.conversation == r.context.conversation)
                        .count()
                        <= CLI_APPROVAL_REQUESTS
            })
            && records
                .iter()
                .map(|r| r.context.conversation)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                <= CLI_APPROVAL_CONTEXTS
    }
    pub fn valid(&self) -> bool {
        self.context.conversation.0 > 0
            && self.context.run_op.0 > 0
            && self.context.run_op.0 < i64::MAX as u64
            && self.context.generation < i64::MAX as u64
            && valid_run(&self.context.run)
            && self.id.run == self.context.run
            && hash(&self.id.invocation)
            && hash(&self.id.arguments_sha256)
            && self.receipt.is_none_or(|state| match state {
                CliApprovalReceiptState::Consumed => {
                    self.decision == CliApprovalDecision::AllowOnce
                }
                CliApprovalReceiptState::Denied => self.decision == CliApprovalDecision::Deny,
                CliApprovalReceiptState::Expired | CliApprovalReceiptState::Revoked => true,
                _ => false,
            })
    }
}
impl CliApprovals {
    pub fn recovery_unavailable(&self) -> bool {
        self.recovery_unavailable
    }
    /// Startup only. Validate the entire bounded set before installing anything.
    /// Missing owner/connection proof is never reconstructed from these records.
    pub(crate) fn restore_recovery(
        &mut self,
        records: Result<Vec<CliApprovalRecovery>, String>,
    ) -> Option<u64> {
        let valid = records
            .as_ref()
            .is_ok_and(|records| CliApprovalRecovery::valid_set(records));
        if !valid || !self.runs.is_empty() || self.last_op != 0 {
            self.recovery_unavailable = true;
            self.retire_all();
            return None;
        }
        let records = records.unwrap();
        let mut high = 0;
        for mut record in records {
            high = high.max(record.context.run_op.0);
            // Desktop attachments do not survive process restart. Retain the exact external
            // run and high-water operation, but restore an unattached, retired view only.
            record.context.generation = 0;
            let index = self
                .runs
                .iter()
                .position(|r| r.context == record.context)
                .unwrap_or_else(|| {
                    self.runs.push(Run {
                        context: record.context.clone(),
                        retired: true,
                        pending: None,
                        items: vec![],
                    });
                    self.runs.len() - 1
                });
            self.runs[index].items.push(CliApprovalItem {
                id: record.id,
                review: None,
                attempted: Some(record.decision),
                status: record
                    .receipt
                    .map_or(CliApprovalStatus::Unknown, CliApprovalStatus::Recorded),
                expires_at_ms: 0,
            });
        }
        self.last_op = high;
        Some(high)
    }
    pub(crate) fn persisted_request_live(&self, request: &CliApprovalRequest) -> bool {
        let CliApprovalAction::Decide { id, .. } = &request.action else {
            return false;
        };
        !self.recovery_unavailable
            && !self.clock_failed
            && self.runs.iter().any(|r| {
                !r.retired
                    && r.context == request.context
                    && r.pending.as_ref() == Some(request)
                    && r.items.iter().any(|i| {
                        &i.id == id
                            && i.status == CliApprovalStatus::Submitting
                            && self.now_ms.is_some_and(|n| n < i.expires_at_ms)
                    })
            })
    }
    pub(crate) fn recovery_record(
        &self,
        context: &CliApprovalContext,
        id: &CliApprovalId,
    ) -> Option<CliApprovalRecovery> {
        let item = self.items(context).iter().find(|i| &i.id == id)?;
        Some(CliApprovalRecovery {
            context: context.clone(),
            id: id.clone(),
            decision: item.attempted?,
            receipt: match item.status {
                CliApprovalStatus::Recorded(state) => Some(state),
                _ => None,
            },
        })
    }
}
