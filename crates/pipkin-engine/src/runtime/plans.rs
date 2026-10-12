//! The plan service: the engine owns a session's plan; the model (`plan_update`/`plan_read`)
//! and clients (`UpdatePlan`/`Plan`) change and read it through the same transactional,
//! revisioned path. Every accepted batch commits the task rows, the change log and the
//! receipt together with the session revision bump and a `PlanChanged` event.
//!
//! Idempotency keys are namespaced so a client key can never collide with a tool call id:
//! `call:<call id>` for the model, `client:<request key>` for clients.

use std::sync::Arc;

use pipkin_engine_protocol::*;

use super::Shared;
use crate::store::Store;
use crate::tools::Tool;
use crate::tools::plan::{PlanReadTool, PlanUpdateTool};
use crate::util::now_ms;

impl Shared {
    pub fn plan_tools(&self) -> [Arc<dyn Tool>; 2] {
        [Arc::new(PlanUpdateTool), Arc::new(PlanReadTool)]
    }

    pub fn plan_view(&self, session: &str, include_archived: bool) -> Result<PlanView, ProtoError> {
        if self.store.session(session)?.is_none() {
            return Err(ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            ));
        }
        Ok(self
            .store
            .read(|c| Store::plan_view(c, session, include_archived))?)
    }

    /// Apply a batch under `key` (already namespaced) and publish the change.
    pub fn update_plan(
        &self,
        session: &str,
        key: &str,
        ops: &[PlanOp],
        expected_revision: Option<u64>,
    ) -> Result<std::result::Result<PlanReceipt, PlanRejected>, ProtoError> {
        if key.len() > limits::MAX_KEY_BYTES + 8 {
            return Err(ProtoError::new(ErrorCode::TooLarge, "request key too long"));
        }
        let sess = self.session(session)?;
        self.commit(&sess, |tx| {
            let (outcome, applied) =
                Store::apply_plan(tx, session, key, ops, expected_revision, now_ms())?;
            let changes = if applied {
                vec![SessionChange::PlanChanged {
                    plan: Store::plan_view(tx, session, false)?,
                }]
            } else {
                vec![]
            };
            Ok((outcome, changes))
        })
    }

    /// The `plan` system section for the next model request, if the plan has live tasks.
    pub fn plan_section(&self, session: &str) -> Option<String> {
        let view = self
            .store
            .read(|c| Store::plan_view(c, session, false))
            .ok()?;
        crate::plans::summary(&view)
    }
}
