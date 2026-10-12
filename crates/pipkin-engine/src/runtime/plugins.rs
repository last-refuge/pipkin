//! Serialize profile selection with first session acquisition and retirement. A live session
//! owns an immutable selection; changing defaults affects only sessions not yet pinned.
use super::Shared;
use crate::extensions::profiles;
use pipkin_engine_protocol::{ErrorCode, ExtensionSpec, ProtoError, Reply};

impl Shared {
    pub(super) fn prepare_extensions(
        &self,
        session: Option<String>,
        explicit: Option<Vec<ExtensionSpec>>,
    ) -> Result<Reply, ProtoError> {
        // Validation does filesystem I/O outside the registry lock. No plugin is activated by
        // preparation, and validation failures cannot overwrite either stored selection.
        let explicit = explicit.map(profiles::normalize).transpose()?;
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let extensions = match &session {
            None => match explicit {
                Some(extensions) => {
                    profiles::set_default(&self.store, &extensions)?;
                    extensions
                }
                None => profiles::default(&self.store)?,
            },
            Some(id) => {
                let row = self.store.session(id)?.ok_or_else(|| {
                    ProtoError::new(ErrorCode::NotFound, format!("Session {id} does not exist"))
                })?;
                if row.parent.is_some() {
                    return Err(ProtoError::new(
                        ErrorCode::Invalid,
                        "Subagent sessions do not load extension profiles",
                    ));
                }
                if let Some(extensions) = explicit {
                    if sessions
                        .live
                        .get(id)
                        .is_some_and(|live| live.rt.extension_profile != extensions)
                    {
                        return Err(ProtoError::new(
                            ErrorCode::Conflict,
                            format!("Session {id} is active with a different plugin selection"),
                        ));
                    }
                    profiles::set_session(&self.store, id, &extensions)?;
                    extensions
                } else if let Some(live) = sessions.live.get(id) {
                    if let Some(why) = live.rt.failure() {
                        return Err(ProtoError::new(
                            ErrorCode::Storage,
                            format!(
                                "This session stopped after a storage error ({why}). Reopen it to continue."
                            ),
                        ));
                    }
                    live.rt.extension_profile.clone()
                } else {
                    profiles::pin(&self.store, id)?
                }
            }
        };
        Ok(Reply::ExtensionProfile {
            session,
            extensions,
        })
    }
}

#[cfg(test)]
mod tests;
