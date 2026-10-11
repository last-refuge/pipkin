//! Importing a Pi profile into the built-in engine: an explicit, two-step flow.
//!
//! A command asks the engine what the Pi profile holds (`ImportStep::Discover`); the answer is
//! shown as an offer the person reads; only an explicit confirmation imports anything. Nothing
//! is imported automatically, and the commands exist only where the backend can import
//! ([`AppState::import_available`], the built-in engine). The engine and the desktop data copy
//! are the controller's; this module only holds the flow.

use crate::protocol::{Effect, Note, Outcome};
use crate::state::AppState;

/// What is being imported from Pi.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportKind {
    /// Conversations (sessions) and their desktop drafts, histories and saved copies.
    Conversations,
    /// Provider settings (`models.json`) and saved credentials.
    Credentials,
}

/// Work the controller does for the import flow.
#[derive(Clone, Debug, PartialEq)]
pub enum ImportStep {
    /// Read what the Pi profile holds (read-only).
    Discover(ImportKind),
    /// Import these Pi sessions, then copy their desktop data.
    Conversations(Vec<String>),
    /// Import these providers' credentials and these configuration items.
    Credentials {
        providers: Vec<String>,
        config: Vec<String>,
    },
}

/// What discovery found, as the person is asked to confirm it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportOffer {
    pub kind: ImportKind,
    /// The summary shown before confirming (counts, reasons, what stays where).
    pub summary: String,
    /// Sessions to import on confirmation.
    pub sessions: Vec<String>,
    /// Providers whose credentials are imported on confirmation (ids only).
    pub providers: Vec<String>,
    /// Configuration items copied on confirmation.
    pub config: Vec<String>,
}

impl ImportOffer {
    /// Whether confirming would import anything.
    pub fn has_work(&self) -> bool {
        match self.kind {
            ImportKind::Conversations => !self.sessions.is_empty(),
            ImportKind::Credentials => !self.providers.is_empty() || !self.config.is_empty(),
        }
    }

    /// The palette entry that confirms this offer.
    pub fn confirm_title(&self) -> String {
        match self.kind {
            ImportKind::Conversations => format!(
                "Confirm: import {} conversation{} from Pi",
                self.sessions.len(),
                if self.sessions.len() == 1 { "" } else { "s" }
            ),
            ImportKind::Credentials => {
                let mut parts = Vec::new();
                if !self.providers.is_empty() {
                    parts.push(format!("credentials for {}", self.providers.join(", ")));
                }
                if !self.config.is_empty() {
                    parts.push(self.config.join(", "));
                }
                format!("Confirm: import {} from Pi", parts.join(" and "))
            }
        }
    }
}

/// Where the import flow is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ImportFlow {
    #[default]
    Idle,
    Discovering(ImportKind),
    /// Waiting for the person to confirm or cancel.
    Offered(ImportOffer),
    Importing(ImportKind),
}

/// How to reach the confirmation without a pointer.
pub const CONFIRM_HINT: &str = "To go ahead, open the command palette (Ctrl+K) and choose the \u{201c}Confirm: import\u{2026}\u{201d} entry; \u{201c}Cancel the Pi import\u{201d} leaves everything as it is.";

impl AppState {
    /// Whether an import can be started now.
    pub fn can_start_import(&self) -> bool {
        self.import_available
            && self.connection.is_ready()
            && matches!(self.import, ImportFlow::Idle | ImportFlow::Offered(_))
    }

    pub(crate) fn start_import(&mut self, kind: ImportKind) -> Outcome {
        let mut out = Outcome::default();
        if !self.can_start_import() {
            return out;
        }
        self.import = ImportFlow::Discovering(kind);
        self.notice = None;
        out.effects.push(Effect::Import(ImportStep::Discover(kind)));
        out.notes.push(Note::Other);
        out
    }

    pub(crate) fn confirm_import(&mut self) -> Outcome {
        let mut out = Outcome::default();
        let ImportFlow::Offered(offer) = &self.import else {
            return out;
        };
        if !offer.has_work() || !self.connection.is_ready() {
            return out;
        }
        let step = match offer.kind {
            ImportKind::Conversations => ImportStep::Conversations(offer.sessions.clone()),
            ImportKind::Credentials => ImportStep::Credentials {
                providers: offer.providers.clone(),
                config: offer.config.clone(),
            },
        };
        self.import = ImportFlow::Importing(offer.kind);
        self.notice = Some("Importing from Pi\u{2026}".into());
        out.effects.push(Effect::Import(step));
        out.notes.push(Note::Other);
        out
    }

    pub(crate) fn cancel_import(&mut self) -> Outcome {
        let mut out = Outcome::default();
        if matches!(self.import, ImportFlow::Offered(_)) {
            self.import = ImportFlow::Idle;
            self.notice = Some("Nothing was imported from Pi.".into());
            out.notes.push(Note::Other);
        }
        out
    }

    /// Discovery answered: offer what can be imported (or say why nothing can).
    pub fn import_discovered(&mut self, result: Result<ImportOffer, String>) -> Outcome {
        let out = Outcome {
            notes: vec![Note::Other],
            ..Outcome::default()
        };
        if !matches!(self.import, ImportFlow::Discovering(_)) {
            return out;
        }
        match result {
            Ok(offer) if offer.has_work() => {
                self.notice = Some(format!("{}\n\n{CONFIRM_HINT}", offer.summary));
                self.import = ImportFlow::Offered(offer);
            }
            Ok(offer) => {
                self.notice = Some(offer.summary);
                self.import = ImportFlow::Idle;
            }
            Err(reason) => {
                self.notice = Some(format!("Could not read the Pi profile: {reason}"));
                self.import = ImportFlow::Idle;
            }
        }
        out
    }

    /// The import ended: say what happened.
    pub fn import_finished(&mut self, message: String) -> Outcome {
        self.import = ImportFlow::Idle;
        self.notice = Some(message);
        Outcome {
            notes: vec![Note::Other],
            ..Outcome::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bootstrap, Command, Connection, Mode, Prefs};

    fn state(available: bool) -> AppState {
        let mut s = AppState::new(
            Bootstrap {
                projects: vec![],
                models: vec![],
                conversations: vec![],
                now: 1,
            },
            Prefs::default(),
        );
        s.mode = Mode::Real;
        s.import_available = available;
        s
    }

    fn offer(sessions: &[&str]) -> ImportOffer {
        ImportOffer {
            kind: ImportKind::Conversations,
            summary: "2 conversations can be imported.".into(),
            sessions: sessions.iter().map(|s| s.to_string()).collect(),
            providers: vec![],
            config: vec![],
        }
    }

    #[test]
    fn nothing_happens_where_the_backend_cannot_import() {
        let mut s = state(false);
        let out = s.dispatch(Command::ImportFromPi(ImportKind::Conversations));
        assert!(out.effects.is_empty());
        assert_eq!(s.import, ImportFlow::Idle);
        let mut s = state(true);
        s.set_connection(Connection::Connecting);
        assert!(
            s.dispatch(Command::ImportFromPi(ImportKind::Conversations))
                .effects
                .is_empty()
        );
    }

    #[test]
    fn discovery_offers_and_only_confirmation_imports() {
        let mut s = state(true);
        let out = s.dispatch(Command::ImportFromPi(ImportKind::Conversations));
        assert_eq!(
            out.effects,
            vec![Effect::Import(ImportStep::Discover(
                ImportKind::Conversations
            ))]
        );
        assert!(!s.can_start_import(), "one discovery at a time");
        s.import_discovered(Ok(offer(&["a", "b"])));
        assert!(matches!(s.import, ImportFlow::Offered(_)));
        let notice = s.notice.clone().unwrap();
        assert!(notice.starts_with("2 conversations can be imported."));
        assert!(notice.contains("Confirm: import"));
        let out = s.dispatch(Command::ConfirmImport);
        assert_eq!(
            out.effects,
            vec![Effect::Import(ImportStep::Conversations(vec![
                "a".into(),
                "b".into()
            ]))]
        );
        assert_eq!(s.import, ImportFlow::Importing(ImportKind::Conversations));
        assert!(
            s.dispatch(Command::ConfirmImport).effects.is_empty(),
            "once"
        );
        s.import_finished("Imported 2 conversations from Pi.".into());
        assert_eq!(s.import, ImportFlow::Idle);
        assert_eq!(
            s.notice.as_deref(),
            Some("Imported 2 conversations from Pi.")
        );
    }

    #[test]
    fn cancelling_dismissing_or_an_empty_offer_imports_nothing() {
        let mut s = state(true);
        s.dispatch(Command::ImportFromPi(ImportKind::Conversations));
        s.import_discovered(Ok(offer(&["a"])));
        assert!(s.dispatch(Command::CancelImport).effects.is_empty());
        assert_eq!(s.import, ImportFlow::Idle);
        assert!(s.dispatch(Command::ConfirmImport).effects.is_empty());

        s.dispatch(Command::ImportFromPi(ImportKind::Conversations));
        s.import_discovered(Ok(offer(&["a"])));
        s.dispatch(Command::DismissNotice);
        assert_eq!(
            s.import,
            ImportFlow::Idle,
            "dismissing the offer declines it"
        );

        s.dispatch(Command::ImportFromPi(ImportKind::Conversations));
        let mut empty = offer(&[]);
        empty.summary = "Nothing to import.".into();
        s.import_discovered(Ok(empty));
        assert_eq!(s.import, ImportFlow::Idle);
        assert_eq!(s.notice.as_deref(), Some("Nothing to import."));

        s.dispatch(Command::ImportFromPi(ImportKind::Credentials));
        s.import_discovered(Err("the engine is offline".into()));
        assert_eq!(s.import, ImportFlow::Idle);
        assert!(s.notice.as_deref().unwrap().contains("engine is offline"));
        // A late answer for no discovery is ignored.
        s.import_discovered(Ok(offer(&["a"])));
        assert_eq!(s.import, ImportFlow::Idle);
    }

    #[test]
    fn a_credentials_offer_confirms_providers_and_settings() {
        let mut s = state(true);
        s.dispatch(Command::ImportFromPi(ImportKind::Credentials));
        let offer = ImportOffer {
            kind: ImportKind::Credentials,
            summary: "x".into(),
            sessions: vec![],
            providers: vec!["openrouter".into()],
            config: vec!["models.json".into()],
        };
        assert_eq!(
            offer.confirm_title(),
            "Confirm: import credentials for openrouter and models.json from Pi"
        );
        s.import_discovered(Ok(offer));
        assert_eq!(
            s.dispatch(Command::ConfirmImport).effects,
            vec![Effect::Import(ImportStep::Credentials {
                providers: vec!["openrouter".into()],
                config: vec!["models.json".into()]
            })]
        );
    }
}
