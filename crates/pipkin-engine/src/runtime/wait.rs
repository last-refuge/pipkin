//! Durable prompt waits. Notifications only wake readers; committed state is authoritative.
use super::Shared;
use crate::store::prompt_result::PromptState;
use pipkin_engine_protocol::{ErrorCode, PromptResult, ProtoError};

impl Shared {
    pub(super) async fn wait_for_prompt(
        &self,
        session: &str,
        request: &str,
    ) -> Result<PromptResult, ProtoError> {
        let _permit = self.prompt_waits.clone().try_acquire_owned().map_err(|_| {
            ProtoError::new(ErrorCode::Busy, "Too many prompt waits; try again later")
        })?;
        let sess = self.session(session)?;
        loop {
            // Register before reading: a settlement between the read and await cannot be lost.
            let changed = sess.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if sess.removed.is_cancelled() {
                return Err(crate::store::StoreError::Removed.into());
            }
            if self.stopping.is_cancelled() {
                return Err(ProtoError::new(
                    ErrorCode::ShuttingDown,
                    "The engine is closing",
                ));
            }
            match self.store.prompt_result(session, request)? {
                PromptState::Unknown => {
                    return Err(ProtoError::new(
                        ErrorCode::NotFound,
                        format!("Unknown prompt: {request}"),
                    ));
                }
                PromptState::Settled(result) => return Ok(result),
                PromptState::Pending => {}
            }
            if let Some(why) = sess.failure() {
                return Err(ProtoError::new(
                    ErrorCode::Storage,
                    format!(
                        "This session stopped after a storage error ({why}). Reopen it to continue."
                    ),
                ));
            }
            tokio::select! {
                _ = self.stopping.cancelled() => {},
                _ = changed => {},
            }
        }
    }
}

#[cfg(test)]
mod tests;
