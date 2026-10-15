//! Which engine may write a conversation.
//!
//! A conversation is writable by exactly one engine. When a Pi conversation is imported into the
//! built-in engine, the desktop records (in its own database, never in either engine's data) that
//! the built-in engine now owns it. With Pi as the engine, such a conversation stays readable
//! (history, search, its saved copy) but nothing that would change it is sent: prompts, queued
//! input, retries, goal turns, model/thinking/subagent settings, answers to extension questions
//! and withdrawing queued input are all refused here, before any effect. Stopping a run that is
//! already in flight stays possible, because stopping is safe.
//!
//! The owner can hand a conversation back explicitly ("Release this conversation back to Pi…"),
//! with a confirmation that says what the Pi copy lacks. Releasing removes only the desktop's
//! ownership record. The controller restores ownership at startup (Pi mode only) and executes
//! [`Effect::ReleaseOwnership`].

use crate::ids::ConversationId;
use crate::protocol::{Effect, Note, Outcome};
use crate::state::AppState;

/// Who, other than the engine this window runs, writes a conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationOwner {
    /// Imported into the built-in engine (`--engine native`) at this time (Unix seconds).
    NativeEngine { imported_at: i64 },
}

/// Shown wherever sending is offered for a conversation the built-in engine owns.
pub const NATIVE_OWNED_NOTICE: &str = "This conversation now continues in the built-in engine (Pipkin --engine native). It is read-only here; the copy shown is the history as of the import.";

/// The palette entry that starts releasing a conversation (Pi mode, owned conversations only).
pub const RELEASE_TITLE: &str = "Release this conversation back to the legacy engine\u{2026}";
/// The palette entry that releases it.
pub const RELEASE_CONFIRM_TITLE: &str =
    "Confirm: release this conversation back to the legacy engine";
/// The palette entry that keeps it with the built-in engine.
pub const RELEASE_CANCEL_TITLE: &str = "Keep this conversation in the built-in engine";

/// What releasing does, shown before the confirmation.
pub const RELEASE_WARNING: &str = "Release this conversation back to the legacy engine? The legacy engine can then continue it from the history it had at the import. Anything written in the built-in engine after the import is not in the legacy copy and will not appear here. Nothing is copied or deleted in either engine: only Pipkin's record that the built-in engine owns this conversation is removed. The built-in engine keeps its own copy, so continuing in both makes them differ.\n\nTo go ahead, open the command palette (Ctrl+K) and choose \u{201c}Confirm: release this conversation back to the legacy engine\u{201d}; \u{201c}Keep this conversation in the built-in engine\u{201d} changes nothing.";

/// Where releasing a conversation is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ReleaseFlow {
    #[default]
    Idle,
    /// Waiting for the person to confirm or keep it.
    Confirming(ConversationId),
    /// The ownership record is being removed.
    Releasing(ConversationId),
}

impl AppState {
    /// Record, from the desktop database, that another engine owns `id`. Restored at startup;
    /// applies whenever the conversation appears (from a saved copy or the engine's list).
    pub fn restore_owner(&mut self, id: ConversationId, owner: ConversationOwner) {
        self.owners.insert(id, owner);
    }

    /// The engine that owns `id`, when it is not the one this window runs.
    pub fn owner(&self, id: ConversationId) -> Option<&ConversationOwner> {
        self.owners.get(&id)
    }

    /// Whether another engine owns `id`, so nothing that changes it may be sent from here.
    pub fn owned_elsewhere(&self, id: ConversationId) -> bool {
        self.owners.contains_key(&id)
    }

    /// Why nothing can be sent in `id`: a read-only build, a disabled execution backend,
    /// or another engine owning the conversation. Shared by existing disabled hints.
    pub fn read_only_reason(&self, id: ConversationId) -> Option<String> {
        if let Some(reason) = &self.read_only {
            return Some(reason.clone());
        }
        if let Some(reason) = self
            .conversation(id)
            .and_then(|c| c.execution_blocked.as_ref())
        {
            return Some(reason.clone());
        }
        if self.mode == crate::Mode::Real
            && !self.owned_elsewhere(id)
            && self
                .conversation(id)
                .is_some_and(|c| !c.execution_context_known)
        {
            return Some(
                "Waiting for the authoritative execution context. History remains available."
                    .into(),
            );
        }
        if self
            .conversation(id)
            .is_some_and(|c| c.profile == crate::ExecutionProfile::Unsupported)
        {
            return Some(
                "This execution profile is unsupported. History remains available.".into(),
            );
        }
        self.owner(id).map(|owner| match owner {
            ConversationOwner::NativeEngine { .. } => NATIVE_OWNED_NOTICE.into(),
        })
    }

    /// Temporary readiness gate, independent of which conversation is selected.
    /// This does not determine the fixed execution profile.
    pub(crate) fn refuse_if_execution_blocked(
        &mut self,
        id: ConversationId,
        out: &mut Outcome,
    ) -> bool {
        let Some(reason) = self
            .conversation(id)
            .and_then(|c| c.execution_blocked.clone())
        else {
            return false;
        };
        self.notice = Some(reason);
        out.notes.push(Note::Other);
        true
    }

    /// Refuse a change to a conversation another engine owns: say why, send nothing.
    /// Returns true when the change must not go ahead.
    pub(crate) fn refuse_if_owned(&mut self, id: ConversationId, out: &mut Outcome) -> bool {
        if !self.owned_elsewhere(id) {
            return false;
        }
        self.notice = Some(NATIVE_OWNED_NOTICE.into());
        out.notes.push(Note::Other);
        true
    }

    /// Whether the open conversation can be released back to Pi now.
    pub fn can_release(&self) -> bool {
        self.selected.is_some_and(|id| self.owned_elsewhere(id))
            && matches!(self.release, ReleaseFlow::Idle | ReleaseFlow::Confirming(_))
    }

    pub(crate) fn request_release(&mut self) -> Outcome {
        let mut out = Outcome::default();
        if !self.can_release() {
            return out;
        }
        let id = self.selected.unwrap();
        self.release = ReleaseFlow::Confirming(id);
        self.notice = Some(RELEASE_WARNING.into());
        out.notes.push(Note::Other);
        out
    }

    pub(crate) fn confirm_release(&mut self) -> Outcome {
        let mut out = Outcome::default();
        let ReleaseFlow::Confirming(id) = self.release else {
            return out;
        };
        // Confirming names the conversation it was asked about, whatever is selected now.
        if !self.owned_elsewhere(id) {
            self.release = ReleaseFlow::Idle;
            return out;
        }
        self.release = ReleaseFlow::Releasing(id);
        self.notice = Some("Releasing the conversation back to the legacy engine\u{2026}".into());
        out.effects
            .push(Effect::ReleaseOwnership { conversation: id });
        out.notes.push(Note::Other);
        out
    }

    pub(crate) fn cancel_release(&mut self) -> Outcome {
        let mut out = Outcome::default();
        if matches!(self.release, ReleaseFlow::Confirming(_)) {
            self.release = ReleaseFlow::Idle;
            self.notice =
                Some("The conversation stays with the built-in engine; nothing changed.".into());
            out.notes.push(Note::Other);
        }
        out
    }

    /// The controller removed (or could not remove) the ownership record.
    pub fn ownership_released(
        &mut self,
        id: ConversationId,
        result: Result<(), String>,
    ) -> Outcome {
        let out = Outcome {
            notes: vec![Note::Other],
            ..Outcome::default()
        };
        if self.release != ReleaseFlow::Releasing(id) {
            return out;
        }
        self.release = ReleaseFlow::Idle;
        match result {
            Ok(()) => {
                self.owners.remove(&id);
                self.notice = Some(
                    "Released: the legacy engine can continue this conversation again. Native-only history \
                     after the import is not in this copy."
                        .into(),
                );
            }
            Err(e) => {
                self.notice = Some(format!(
                    "Could not release the conversation; it stays read-only here: {e}"
                ));
            }
        }
        out
    }
}
