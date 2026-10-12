//! Snapshots of committed state for clients.

use pipkin_engine_protocol::*;

use super::{Shared, models};
use crate::store::Store;

impl Shared {
    /// Everything needed to show a session, read at one revision.
    pub fn snapshot(&self, session: &str, window: u32) -> Result<SessionSnapshot, ProtoError> {
        // Writers serialize publication with their state commit. Field changes outside that
        // path are detected by checking the revision again before accepting the snapshot.
        let _publication = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..16 {
            let snapshot = self.snapshot_once(session, window)?;
            if self
                .store
                .session(session)?
                .is_some_and(|r| r.revision == snapshot.revision)
            {
                return Ok(snapshot);
            }
        }
        Err(ProtoError::new(
            ErrorCode::Busy,
            "Session changed while reading; retry the snapshot",
        ))
    }

    fn snapshot_once(&self, session: &str, window: u32) -> Result<SessionSnapshot, ProtoError> {
        let row = self.store.session(session)?.ok_or_else(|| {
            ProtoError::new(
                ErrorCode::NotFound,
                format!("Session {session} does not exist"),
            )
        })?;
        let (entries, has_older) = self
            .store
            .window(session, window.clamp(1, limits::MAX_PAGE_ENTRIES))?;
        let run = self.store.open_run(session)?.and_then(|r| r.view());
        let queue = Store::queue_view(&self.store.queue(session)?);
        let usage = self.store.usage(session)?;
        let catalog = self.auth.catalog();
        let model = row.model.as_deref().and_then(|k| catalog.find(k));
        let thinking = models::thinking_view(model, row.thinking.as_deref());
        let subagents = if row.parent.is_none() {
            Some(self.subagents_view(session)?)
        } else {
            None
        };
        let mut summary = row.summary();
        if let Some(m) = model
            && let Err(crate::auth::AuthError::Unavailable(why)) = self.auth.availability(m)
        {
            summary.blocked = Some(why);
        }
        Ok(SessionSnapshot {
            summary,
            revision: row.revision,
            entries,
            has_older,
            run,
            queue,
            usage,
            thinking,
            ui: self
                .session(session)
                .map(|s| s.questions.view())
                .unwrap_or_default(),
            subagents,
        })
    }
}
