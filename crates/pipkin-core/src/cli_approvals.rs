//! Pure external-approval reducer. No engine protocol, transport, persistence, or GPUI.
//! Requests are descriptions for the adapter, never executable/native tools. See
//! `CLI_APPROVAL_API.md` for the still-required AppState command/adapter/UI wiring.
use crate::{ConversationId, OperationId};
mod recovery;
pub use recovery::CliApprovalRecovery;

pub const CLI_APPROVAL_CONTEXTS: usize = 8;
pub const CLI_APPROVAL_REQUESTS: usize = 128;
pub const CLI_APPROVAL_PENDING: usize = 16;
pub const CLI_APPROVAL_DISPLAY_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliApprovalRun {
    pub epoch: u64,
    pub session: String,
    pub binding_generation: u64,
    pub op: String,
    pub prompt: u64,
}
impl CliApprovalRun {
    pub fn valid(&self) -> bool {
        valid_run(self)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliApprovalContext {
    pub conversation: ConversationId,
    pub generation: u64,
    pub run_op: OperationId,
    pub run: CliApprovalRun,
}
#[derive(Clone, PartialEq, Eq)]
pub struct CliApprovalId {
    pub run: CliApprovalRun,
    pub invocation: String,
    pub arguments_sha256: String,
}
impl std::fmt::Debug for CliApprovalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CliApprovalId([redacted])")
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct CliApprovalReview {
    pub id: CliApprovalId,
    pub arguments: String,
    pub expires_at_ms: i64,
}
impl std::fmt::Debug for CliApprovalReview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CliApprovalReview([redacted])")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliApprovalDecision {
    AllowOnce,
    Deny,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliApprovalReceiptState {
    Pending,
    Granted,
    Consumed,
    Denied,
    Expired,
    Revoked,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliApprovalReceipt {
    pub id: CliApprovalId,
    pub state: CliApprovalReceiptState,
    pub decision: Option<CliApprovalDecision>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliApprovalStatus {
    Reviewable,
    Unavailable,
    Submitting,
    Unknown,
    /// Consumed is NOT proof of delivery or action completion.
    Recorded(CliApprovalReceiptState),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliApprovalItem {
    pub id: CliApprovalId,
    pub review: Option<CliApprovalReview>,
    pub attempted: Option<CliApprovalDecision>,
    pub status: CliApprovalStatus,
    // Kept after dropping plaintext, so a refreshed deadline cannot extend authority.
    expires_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliApprovalAction {
    List,
    Decide {
        id: CliApprovalId,
        decision: CliApprovalDecision,
    },
    Lookup {
        id: CliApprovalId,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliApprovalRequest {
    pub context: CliApprovalContext,
    pub op: OperationId,
    pub action: CliApprovalAction,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliApprovalResult {
    Listed {
        run: Option<CliApprovalRun>,
        reviews: Vec<CliApprovalReview>,
    },
    Receipt(Option<CliApprovalReceipt>),
    /// Includes NotSent, rejection, timeout and transport loss. None permits a resend.
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliApprovalResponse {
    pub context: CliApprovalContext,
    pub op: OperationId,
    pub result: CliApprovalResult,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliApprovalApply {
    Ignored,
    Applied,
    Invalid,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CliApprovalAvailability {
    pub list: bool,
    pub decide: bool,
    pub lookup: bool,
}

#[derive(Debug)]
struct Run {
    context: CliApprovalContext,
    retired: bool,
    pending: Option<CliApprovalRequest>,
    items: Vec<CliApprovalItem>,
}
impl Run {
    fn retire(&mut self) {
        self.retired = true;
        self.pending = None;
        for item in &mut self.items {
            item.review = None;
            item.status = match item.status {
                CliApprovalStatus::Reviewable => CliApprovalStatus::Unavailable,
                CliApprovalStatus::Submitting => CliApprovalStatus::Unknown,
                other => other,
            };
        }
    }
}

/// Bounded in-memory view owned by AppState. Saved attempts restore only retired metadata.
/// No silent eviction or automatic requests; plaintext reviews remain ephemeral.
#[derive(Debug, Default)]
pub struct CliApprovals {
    runs: Vec<Run>,
    // Preserve across retirement/forget: operation reuse cannot accept a late reply.
    last_op: u64,
    now_ms: Option<i64>,
    clock_failed: bool,
    recovery_unavailable: bool,
}
fn token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}
fn hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_run(r: &CliApprovalRun) -> bool {
    [r.epoch, r.binding_generation, r.prompt]
        .into_iter()
        .all(|n| n > 0 && n <= i64::MAX as u64)
        && token(&r.session)
        && token(&r.op)
}
fn valid_review(r: &CliApprovalReview, run: &CliApprovalRun) -> bool {
    r.id.run == *run
        && hash(&r.id.invocation)
        && hash(&r.id.arguments_sha256)
        && !r.arguments.is_empty()
        && r.arguments.len() <= CLI_APPROVAL_DISPLAY_BYTES
        && r.arguments
            .bytes()
            .all(|b| b == b'\n' || (b' '..=b'~').contains(&b))
        && r.expires_at_ms > 0
}
fn valid_receipt(r: &CliApprovalReceipt, choice: CliApprovalDecision) -> bool {
    use CliApprovalDecision as D;
    use CliApprovalReceiptState as S;
    r.decision.is_none_or(|d| d == choice)
        && match r.state {
            S::Pending => r.decision.is_none(),
            S::Granted | S::Consumed => r.decision == Some(D::AllowOnce),
            S::Denied => r.decision == Some(D::Deny),
            S::Expired | S::Revoked => r.decision != Some(D::Deny),
        }
}
impl CliApprovals {
    /// Only a caller with authoritative backend run/receipt and desktop context may bind.
    /// Rebinding the same external run to a new attachment never revives decisions.
    pub fn bind(&mut self, context: CliApprovalContext) -> bool {
        if self.recovery_unavailable
            || context.conversation.0 == 0
            || context.run_op.0 == 0
            || !valid_run(&context.run)
        {
            return false;
        }
        if self
            .runs
            .iter()
            .any(|r| r.context.run == context.run && r.context.conversation != context.conversation)
        {
            return false;
        }
        if let Some(run) = self
            .runs
            .iter_mut()
            .find(|r| r.context.conversation == context.conversation)
        {
            if context.generation < run.context.generation || context.run_op < run.context.run_op {
                return false;
            }
            if run.context.run == context.run {
                if run.context != context {
                    run.retire();
                    run.context = context;
                }
                return true;
            }
            // Receipt IDs increase within an engine incarnation. Retain this high-water
            // identity even after dropping old resolved views, so old runs cannot revive.
            if self.clock_failed
                || context.run.session != run.context.run.session
                || (context.run.epoch, context.run.prompt)
                    <= (run.context.run.epoch, run.context.run.prompt)
                || context.run_op <= run.context.run_op
            {
                return false;
            }
            run.retire();
            if run.items.iter().any(|i| {
                matches!(
                    i.status,
                    CliApprovalStatus::Unknown | CliApprovalStatus::Submitting
                )
            }) {
                return false;
            }
            *run = Run {
                context,
                retired: false,
                pending: None,
                items: vec![],
            };
            return true;
        }
        if self.clock_failed || self.runs.len() >= CLI_APPROVAL_CONTEXTS {
            return false;
        }
        self.runs.push(Run {
            context,
            retired: false,
            pending: None,
            items: vec![],
        });
        true
    }
    /// UI clock is injected. Regression fails closed permanently for this state instance.
    pub fn tick(&mut self, now_ms: i64) {
        if self.clock_failed {
            return;
        }
        if self.now_ms.is_some_and(|previous| now_ms < previous) {
            self.clock_failed = true;
            self.retire_all();
            return;
        }
        self.now_ms = Some(now_ms);
        for run in &mut self.runs {
            for item in &mut run.items {
                if now_ms >= item.expires_at_ms {
                    item.review = None;
                    if item.status == CliApprovalStatus::Reviewable {
                        item.status = CliApprovalStatus::Unavailable;
                    }
                }
            }
        }
    }
    pub fn retire_all(&mut self) {
        for run in &mut self.runs {
            run.retire();
        }
    }
    pub fn retire(&mut self, context: &CliApprovalContext) -> bool {
        if let Some(run) = self
            .runs
            .iter_mut()
            .find(|r| &r.context == context && !r.retired)
        {
            run.retire();
            return true;
        }
        false
    }
    /// Attachment/connection/ownership loss also invalidates pending read-only operations.
    pub fn invalidate(&mut self, context: &CliApprovalContext) -> bool {
        if let Some(run) = self.runs.iter_mut().find(|r| &r.context == context) {
            let changed = !run.retired || run.pending.is_some();
            run.retire();
            return changed;
        }
        false
    }
    pub fn contexts(&self) -> impl Iterator<Item = &CliApprovalContext> {
        self.runs.iter().map(|r| &r.context)
    }
    pub fn clock_failed(&self) -> bool {
        self.clock_failed
    }
    pub fn items(&self, context: &CliApprovalContext) -> &[CliApprovalItem] {
        self.runs
            .iter()
            .find(|r| &r.context == context)
            .map_or(&[], |r| &r.items)
    }
    pub fn availability(
        &self,
        context: &CliApprovalContext,
        id: Option<&CliApprovalId>,
    ) -> CliApprovalAvailability {
        let Some(run) = self.runs.iter().find(|r| &r.context == context) else {
            return Default::default();
        };
        let free = run.pending.is_none();
        let item = id.and_then(|id| run.items.iter().find(|item| &item.id == id));
        CliApprovalAvailability {
            list: free && !run.retired && !self.clock_failed && self.now_ms.is_some(),
            decide: free
                && !run.retired
                && !self.clock_failed
                && !self.recovery_unavailable
                && item.is_some_and(|i| {
                    i.status == CliApprovalStatus::Reviewable
                        && i.attempted.is_none()
                        && i.review.is_some()
                        && self.now_ms.is_some_and(|n| n < i.expires_at_ms)
                }),
            lookup: free && item.is_some_and(|i| i.attempted.is_some()),
        }
    }
    /// At most one outstanding operation per external run. Mark attempted BEFORE returning.
    pub fn request(
        &mut self,
        context: &CliApprovalContext,
        op: OperationId,
        action: CliApprovalAction,
    ) -> Option<CliApprovalRequest> {
        if op.0 <= self.last_op {
            return None;
        }
        let id = match &action {
            CliApprovalAction::List => None,
            CliApprovalAction::Decide { id, .. } | CliApprovalAction::Lookup { id } => Some(id),
        };
        let a = self.availability(context, id);
        if !match action {
            CliApprovalAction::List => a.list,
            CliApprovalAction::Decide { .. } => a.decide,
            CliApprovalAction::Lookup { .. } => a.lookup,
        } {
            return None;
        }
        let run = self.runs.iter_mut().find(|r| &r.context == context)?;
        if let CliApprovalAction::Decide { id, decision } = &action {
            let item = run.items.iter_mut().find(|i| &i.id == id)?;
            item.attempted = Some(*decision);
            item.status = CliApprovalStatus::Submitting;
        }
        let request = CliApprovalRequest {
            context: context.clone(),
            op,
            action,
        };
        run.pending = Some(request.clone());
        self.last_op = op.0;
        Some(request)
    }
    pub(crate) fn busy(&self, conversation: ConversationId) -> bool {
        self.runs
            .iter()
            .any(|r| r.context.conversation == conversation && r.pending.is_some())
    }
    pub(crate) fn reserve_discovery(&mut self, op: OperationId) -> bool {
        if op.0 <= self.last_op {
            return false;
        }
        self.last_op = op.0;
        true
    }
    /// Only the port's matched outstanding discovery may install a reserved operation.
    /// Other conversations may have issued newer operations while this read was pending.
    pub(crate) fn install_discovered(
        &mut self,
        context: CliApprovalContext,
        op: OperationId,
        result: CliApprovalResult,
    ) -> CliApprovalApply {
        if op.0 == 0
            || op.0 > self.last_op
            || self.busy(context.conversation)
            || !self.bind(context.clone())
        {
            return CliApprovalApply::Invalid;
        }
        let run = self.runs.iter_mut().find(|r| r.context == context).unwrap();
        if run.retired {
            return CliApprovalApply::Invalid;
        }
        run.pending = Some(CliApprovalRequest {
            context: context.clone(),
            op,
            action: CliApprovalAction::List,
        });
        self.apply(CliApprovalResponse {
            context,
            op,
            result,
        })
    }
    pub fn apply(&mut self, response: CliApprovalResponse) -> CliApprovalApply {
        let Some(run) = self.runs.iter_mut().find(|r| r.context == response.context) else {
            return CliApprovalApply::Ignored;
        };
        let Some(pending) = run.pending.as_ref().filter(|p| p.op == response.op) else {
            return CliApprovalApply::Ignored;
        };
        // Validate the entire reply before changing any view or consuming an operation.
        let valid = match (&pending.action, &response.result) {
            (_, CliApprovalResult::Failed) => true,
            (
                CliApprovalAction::List,
                CliApprovalResult::Listed {
                    run: identity,
                    reviews,
                },
            ) => {
                (identity.as_ref() == Some(&run.context.run)
                    || (identity.is_none() && reviews.is_empty()))
                    && reviews.len() <= CLI_APPROVAL_PENDING
                    && reviews.iter().enumerate().all(|(index, review)| {
                        valid_review(review, &run.context.run)
                            && !reviews[..index]
                                .iter()
                                .any(|r| r.id.invocation == review.id.invocation)
                            && run
                                .items
                                .iter()
                                .find(|i| i.id.invocation == review.id.invocation)
                                .is_none_or(|i| {
                                    i.id == review.id
                                        && i.expires_at_ms == review.expires_at_ms
                                        && i.review
                                            .as_ref()
                                            .is_none_or(|old| old.arguments == review.arguments)
                                })
                    })
                    && run.items.len()
                        + reviews
                            .iter()
                            .filter(|r| {
                                !run.items.iter().any(|i| i.id.invocation == r.id.invocation)
                            })
                            .count()
                        <= CLI_APPROVAL_REQUESTS
            }
            (
                CliApprovalAction::Decide { id, .. } | CliApprovalAction::Lookup { id },
                CliApprovalResult::Receipt(receipt),
            ) => receipt.as_ref().is_none_or(|r| {
                &r.id == id
                    && run
                        .items
                        .iter()
                        .find(|i| &i.id == id)
                        .and_then(|i| i.attempted)
                        .is_some_and(|choice| valid_receipt(r, choice))
            }),
            _ => false,
        };
        if !valid {
            run.retire();
            return CliApprovalApply::Invalid;
        }
        let pending = run.pending.take().unwrap();
        match (pending.action, response.result) {
            (
                CliApprovalAction::List,
                CliApprovalResult::Listed {
                    run: identity,
                    reviews,
                },
            ) => {
                if identity.is_none() {
                    run.retire();
                    return CliApprovalApply::Applied;
                }
                for item in &mut run.items {
                    if !reviews.iter().any(|r| r.id == item.id) {
                        item.review = None;
                        if item.status == CliApprovalStatus::Reviewable {
                            item.status = CliApprovalStatus::Unavailable;
                        }
                    }
                }
                for review in reviews {
                    // Never restore a previously absent/expired/attempted view, even if it reappears.
                    if run
                        .items
                        .iter()
                        .any(|i| i.id.invocation == review.id.invocation)
                    {
                        continue;
                    }
                    let live = self.now_ms.is_some_and(|n| n < review.expires_at_ms);
                    run.items.push(CliApprovalItem {
                        id: review.id.clone(),
                        expires_at_ms: review.expires_at_ms,
                        review: live.then_some(review),
                        attempted: None,
                        status: if live {
                            CliApprovalStatus::Reviewable
                        } else {
                            CliApprovalStatus::Unavailable
                        },
                    });
                }
            }
            (CliApprovalAction::Decide { id, .. } | CliApprovalAction::Lookup { id }, result) => {
                let item = run.items.iter_mut().find(|i| i.id == id).unwrap();
                item.review = None;
                // Knowledge never regresses on a failed/absent/pending read-only lookup.
                if !matches!(item.status, CliApprovalStatus::Recorded(_)) {
                    item.status = match result {
                        CliApprovalResult::Receipt(Some(r))
                            if !matches!(
                                r.state,
                                CliApprovalReceiptState::Pending | CliApprovalReceiptState::Granted
                            ) =>
                        {
                            CliApprovalStatus::Recorded(r.state)
                        }
                        _ => CliApprovalStatus::Unknown,
                    };
                }
            }
            (CliApprovalAction::List, CliApprovalResult::Failed) => {
                for item in &mut run.items {
                    item.review = None;
                    if item.status == CliApprovalStatus::Reviewable {
                        item.status = CliApprovalStatus::Unavailable;
                    }
                }
            }
            _ => unreachable!("validated reply shape"),
        }
        CliApprovalApply::Applied
    }
}
