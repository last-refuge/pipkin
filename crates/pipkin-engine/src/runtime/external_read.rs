//! Read-only dispatch by execution identity. External receipts are authoritative for external
//! prompts; native request/message tables are never a fallback, including after restart.
use super::Shared;
use crate::store::prompt_result::PromptState;
use pipkin_engine_protocol::*;

impl Shared {
    pub(super) fn lookup_prompt(
        &self,
        session: &str,
        request: &str,
    ) -> Result<LookupResult, ProtoError> {
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Session does not exist"))?;
        match row.execution_backend {
            ExecutionBackend::NativeAgent => Ok(self
                .store
                .request(session, request)?
                .map(|r| r.lookup())
                .unwrap_or(LookupResult::Unknown)),
            ExecutionBackend::LocalClaudeCli => {
                Ok(match self.store.external_prompt(session, request)? {
                    Some(receipt) => LookupResult::Known {
                        mode: InputMode::Prompt,
                        entry: None,
                        run: Some(receipt.id),
                        open: receipt.run_view().is_some(),
                        outcome: receipt.outcome,
                    },
                    None => LookupResult::Unknown,
                })
            }
            ExecutionBackend::Unsupported => Err(ProtoError::new(
                ErrorCode::Unsupported,
                "This execution backend is unsupported",
            )),
        }
    }

    pub(super) fn prompt_state(
        &self,
        session: &str,
        request: &str,
    ) -> Result<PromptState, ProtoError> {
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Session does not exist"))?;
        match row.execution_backend {
            ExecutionBackend::NativeAgent => Ok(self.store.prompt_result(session, request)?),
            ExecutionBackend::LocalClaudeCli => Ok(match self.store.external_prompt(session, request)? {
                None => PromptState::Unknown,
                Some(receipt) => match receipt.result {
                    Some(result) => PromptState::Settled(result),
                    None if receipt.run_view().is_some() => PromptState::Pending,
                    None => PromptState::Settled(PromptResult::Unanswered {
                        text: None, reason: "External execution ended without a confirmed answer; use a fresh conversation.".into(),
                    }),
                },
            }),
            ExecutionBackend::Unsupported => Err(ProtoError::new(ErrorCode::Unsupported, "This execution backend is unsupported")),
        }
    }

    pub(super) fn external_run_view(&self, session: &str) -> Result<Option<RunView>, ProtoError> {
        Ok(self
            .store
            .active_external_prompts()?
            .into_iter()
            .find(|r| r.key.session == session)
            .and_then(|r| r.run_view()))
    }
}
