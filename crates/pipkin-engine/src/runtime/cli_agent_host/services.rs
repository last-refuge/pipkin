//! Existing service operations with original-turn/pending-call authority. No inference host.
use super::*;
use crate::mcp::{Elicitor, McpError};
use std::{future::Future, path::Path};
pub(super) type McpResult<T> = Result<Result<T, McpError>, String>;
pub(super) type Listing<T> = (Vec<T>, Vec<(String, String)>);

impl Host {
    pub(super) fn publish_progress(&self, text: &str) {
        // A bounded volatile view, not a transcript mutation or a receipt. Commit/delivery
        // serialization ensures no progress can be delivered after this call's final result.
        let mut end = text.len().min(32 * 1024);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let _ = self.publication.commit(self.authority.scope(), || {
            self.authority.authorize(|| {
                self.shared.store.read(|c| {
                    self.live(c)?;
                    self.shared.publish(pipkin_engine_protocol::Event::Session {
                        session: self.authority.scope().session.clone(),
                        revision: 0,
                        change: SessionChange::ToolProgress {
                            call_id: self.call.clone(),
                            text: text[..end].to_owned(),
                        },
                    });
                    Ok(((), None))
                })
            })
        });
    }
    pub(super) fn web_update(&self, tx: &rusqlite::Transaction) -> crate::store::Result<Update> {
        let scope = self.authority.scope();
        let records = Store::web_sources(tx, &scope.session)?;
        let start = records
            .len()
            .saturating_sub(super::super::web::WEB_SOURCES_IN_VIEW);
        let sources = super::super::web::source_views(&records[start..]);
        let revision = Store::bump(tx, &scope.session, now_ms())?;
        Ok(Update {
            scope: scope.clone(),
            revision,
            changes: vec![SessionChange::WebSourcesChanged { sources }],
        })
    }
    pub(super) async fn service<T>(
        &self,
        name: &str,
        call: Option<&str>,
        cancel: CancellationToken,
        operation: impl Future<Output = T>,
    ) -> Result<T, String> {
        self.named(name, call)?;
        tokio::task::block_in_place(|| self.check())?;
        let result = tokio::select! { biased;
            _ = self.cancel.cancelled() => return Err("Native service cancelled; any remote effect is unconfirmed; do not replay".into()),
            _ = cancel.cancelled() => return Err("Native service caller cancelled; any remote effect is unconfirmed; do not replay".into()),
            _ = self.shared.stopping.cancelled() => return Err("Engine stopping; remote effect unconfirmed; do not replay".into()),
            _ = tokio::time::sleep_until(self.deadline) => return Err("Native service deadline; remote effect unconfirmed; do not replay".into()),
            result = operation => result,
        };
        tokio::task::block_in_place(|| self.check())?;
        if cancel.is_cancelled() {
            return Err("Native service caller cancelled; do not replay".into());
        }
        Ok(result)
    }
    pub(super) fn refresh_mcp(&self) -> Result<(), String> {
        // Never call mcp_refresh(session): its API guard deliberately refuses CLI sessions,
        // and its void return would leave a previous project's configuration in use.
        self.check()?;
        let config = self.shared.mcp_config_at(Some(Path::new(
            &self.authority.binding().create.identity.project_path,
        )));
        self.check()?;
        for handle in self.shared.mcp.apply_config(config) {
            self.shared.spawn(async move { handle.stop().await });
        }
        Ok(())
    }
    pub(super) fn mcp_elicitor(&self) -> Arc<dyn Elicitor> {
        super::super::mcp::scoped_elicitor(
            &self.shared,
            self.authority.scope().session.clone(),
            self.call.clone(),
            Arc::new(McpQuestions(self.clone())),
        )
    }
}
struct McpQuestions(Host);
impl ToolHost for McpQuestions {
    fn subagent<'a>(
        &'a self,
        _call: &'a str,
        _launch: SubagentLaunch,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<(String, Value), String>> {
        Box::pin(async { Err("MCP elicitation does not authorize inference".into()) })
    }
    fn ask<'a>(
        &'a self,
        call: &'a str,
        question: crate::extensions::questions::Question,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, crate::tools::ask::AskOutcome> {
        Box::pin(async move {
            if let Err(reason) = self.0.named("mcp_call", Some(call)) {
                return crate::tools::ask::AskOutcome::Unavailable(reason);
            }
            self.0.ask_authorized(question, cancel).await
        })
    }
}
