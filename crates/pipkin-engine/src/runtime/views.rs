//! Snapshots of committed state for clients.

use pipkin_engine_protocol::*;

use super::{Shared, models};
use crate::store::Store;

#[cfg(test)]
#[path = "views_tests.rs"]
mod tests;

impl Shared {
    /// Everything needed to show a session, read at one revision.
    pub fn snapshot(&self, session: &str, window: u32) -> Result<SessionSnapshot, ProtoError> {
        // Volatile caches are not part of the stored revision. Capture them before
        // publication: removal takes the session registry before publication.
        let runtime = self.session(session).ok();
        let ui = runtime
            .as_ref()
            .map(|s| s.questions.view())
            .unwrap_or_default();
        let system_tokens = runtime
            .as_ref()
            .map(|s| Self::cached_system_tokens(s))
            .unwrap_or(0);
        self.snapshot_from_cache(session, window, &ui, system_tokens)
    }

    // Requires captured volatile values; no session-registry access is allowed here.
    fn snapshot_from_cache(
        &self,
        session: &str,
        window: u32,
        ui: &UiView,
        system_tokens: u64,
    ) -> Result<SessionSnapshot, ProtoError> {
        // Writers serialize publication with their state commit. Field changes outside that
        // path are detected by checking the revision again before accepting the snapshot.
        let _publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..16 {
            let mut snapshot = self.snapshot_once(session, window, ui, system_tokens)?;
            if self
                .store
                .session(session)?
                .is_some_and(|r| r.revision == snapshot.revision)
            {
                // Trust/readiness is ephemeral, not part of the stored revision. Never
                // take the trust registry under publication: admission takes those locks
                // in the opposite direction. The dispatcher revalidates authority anyway.
                drop(_publication);
                if snapshot.summary.execution_backend == ExecutionBackend::LocalClaudeCli
                    && let Some(current) = self.store.session(session)?
                    && current.revision == snapshot.revision
                {
                    let execution = self.execution_summary(&current);
                    snapshot.summary.blocked = execution.blocked;
                    // Immutable mode was captured in the committed snapshot below.
                }
                return Ok(snapshot);
            }
        }
        Err(ProtoError::new(
            ErrorCode::Busy,
            "Session changed while reading; retry the snapshot",
        ))
    }

    fn snapshot_once(
        &self,
        session: &str,
        window: u32,
        ui: &UiView,
        system_tokens: u64,
    ) -> Result<SessionSnapshot, ProtoError> {
        let row = self.store.session(session)?.ok_or_else(|| {
            ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            )
        })?;
        let (entries, has_older) = self
            .store
            .window(session, window.clamp(1, limits::MAX_PAGE_ENTRIES))?;
        let native = matches!(
            row.execution_backend,
            ExecutionBackend::NativeAgent | ExecutionBackend::ClaudeCliAgent
        );
        // Never project foreign/native journal rows as a runnable external-agent turn.
        let run = match row.execution_backend {
            ExecutionBackend::NativeAgent | ExecutionBackend::ClaudeCliAgent => {
                self.store.open_run(session)?.and_then(|r| r.view())
            }
            ExecutionBackend::LocalClaudeCli => self.external_run_view(session)?,
            ExecutionBackend::Unsupported => None,
        };
        let queue = if native {
            Store::queue_view(&self.store.queue(session)?)
        } else {
            Vec::new()
        };
        let usage = self.store.usage(session)?;
        let catalog = self.auth.catalog();
        let model = row
            .model
            .as_deref()
            .filter(|_| row.execution_backend.native_execution())
            .and_then(|k| catalog.find(k));
        let thinking = if row.execution_backend.native_execution() {
            models::thinking_view(model, row.thinking.as_deref())
        } else {
            ThinkingView::default()
        };
        let subagents = if native && row.parent.is_none() {
            Some(self.subagents_view(session)?)
        } else {
            None
        };
        let mut summary = row.summary();
        // Unlike ephemeral readiness, immutable policy must survive a revision change
        // after publication unlock. This read acquires neither trust nor session registry.
        summary.local_cli_mode = self.local_cli_summary_mode(&row);
        if let Some(m) = model
            && let Err(crate::auth::AuthError::Unavailable(why)) = self.auth.availability(m)
        {
            summary.blocked = Some(why);
        }
        Ok(SessionSnapshot {
            native_history_start: self.store.native_history_start(session)?,
            summary,
            revision: row.revision,
            entries,
            has_older,
            run,
            queue,
            usage,
            thinking,
            ui: if native {
                ui.clone()
            } else {
                UiView::default()
            },
            subagents,
            // Foreign native operational rows are not live CLI controls or automation.
            web_sources: if native {
                self.web_sources_bounded(session)?
            } else {
                Vec::new()
            },
            plan: if native {
                self.store.read(|c| Store::plan_view(c, session, false))?
            } else {
                PlanView::default()
            },
            goal: if native {
                self.store.read(|c| Store::goal_current(c, session))?
            } else {
                None
            },
            jobs: if native {
                self.jobs_view(session)?
            } else {
                JobsView::default()
            },
            context: if native {
                self.context_usage_with_system_tokens(session, system_tokens)?
            } else {
                ContextUsageView {
                    source: "unavailable".into(),
                    compaction: "off".into(),
                    revision: row.revision,
                    ..Default::default()
                }
            },
        })
    }
}
