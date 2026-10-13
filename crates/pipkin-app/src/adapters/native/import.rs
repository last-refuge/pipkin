//! The explicit Pi import, on the built-in engine's side of the desktop: discovery becomes an
//! offer the person reads, a confirmed offer becomes engine import calls, and the result names
//! the conversations whose desktop data the controller then copies from the Pi namespace.
//!
//! Credential values never pass through here: the engine reports provider ids and kinds only.

use std::time::Duration;

use pipkin_core::{ConversationId, ImportKind, ImportOffer, ImportStep, LifecycleEvent};
use pipkin_engine_protocol::{
    Call, ConfigItemOutcome, CredentialImportOutcome, ImportReport, ImportSessionOutcome,
    ImportSessionStatus, ImportedView, Reply,
};

use super::client::CallError;
use super::{Live, Worker};

/// Importing many sessions copies and checks each one; allow far more than a normal call.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(300);
/// Rows named in a summary before "and N more".
const LISTED: usize = 8;

/// What the rollback and ownership rules are, said before and after a conversation import.
pub const OWNERSHIP: &str = "The original legacy profile is unchanged and retained as a backup. \
     Anything added to an imported conversation here is kept by the built-in engine only.";

/// The answer to one import step.
#[derive(Clone, Debug, PartialEq)]
pub enum ImportResult {
    Offer(Result<ImportOffer, String>),
    Finished {
        message: String,
        /// Conversations imported now: their desktop data is copied and restored.
        imported: Vec<ConversationId>,
        /// Conversations imported earlier: their desktop data is copied (never over newer data).
        existing: Vec<ConversationId>,
    },
}

pub type ImportDone = Box<dyn FnOnce(ImportResult) + Send>;

/// The answer when the engine cannot be asked at all.
pub fn unavailable(step: &ImportStep, reason: &str) -> ImportResult {
    match step {
        ImportStep::Discover(_) => ImportResult::Offer(Err(reason.to_owned())),
        _ => ImportResult::Finished {
            message: format!("Nothing was imported: {reason}"),
            imported: vec![],
            existing: vec![],
        },
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn listed(rows: &[String]) -> String {
    let mut text = rows
        .iter()
        .take(LISTED)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if rows.len() > LISTED {
        text.push_str(&format!("; and {} more", rows.len() - LISTED));
    }
    text
}

fn short(id: &str) -> String {
    id.chars().take(13).collect()
}

/// The conversation import offer for a discovery report.
pub fn conversations_offer(report: &ImportReport) -> ImportOffer {
    let mut sessions = Vec::new();
    let (mut already, mut locked, mut blocked) = (0usize, Vec::new(), Vec::new());
    for s in &report.sessions {
        match &s.status {
            ImportSessionStatus::Importable => sessions.push(s.id.clone()),
            ImportSessionStatus::AlreadyImported { .. } => already += 1,
            ImportSessionStatus::Locked { detail } => locked.push(match detail {
                Some(d) => format!("{} ({d})", short(&s.id)),
                None => short(&s.id),
            }),
            ImportSessionStatus::Blocked { reasons } => {
                blocked.push(format!("{}: {}", short(&s.id), reasons.join(", ")))
            }
        }
    }
    let mut lines = vec![if report.sessions.is_empty() {
        format!(
            "No legacy conversations were found in {}.",
            report.agent_dir
        )
    } else if sessions.is_empty() {
        format!(
            "No legacy conversation in {} can be imported now.",
            report.agent_dir
        )
    } else {
        format!(
            "{} from {} can be imported into the built-in engine.",
            plural(
                sessions.len(),
                "legacy conversation",
                "legacy conversations"
            ),
            report.agent_dir
        )
    }];
    if already > 0 {
        lines.push(format!(
            "{} imported earlier.",
            plural(already, "conversation was", "conversations were")
        ));
    }
    if !locked.is_empty() {
        lines.push(format!(
            "In use by a running legacy engine (close it and try again): {}.",
            listed(&locked)
        ));
    }
    if !blocked.is_empty() {
        lines.push(format!("Cannot be imported: {}.", listed(&blocked)));
    }
    if !sessions.is_empty() {
        lines.push(OWNERSHIP.into());
    }
    ImportOffer {
        kind: ImportKind::Conversations,
        summary: lines.join("\n"),
        sessions,
        providers: vec![],
        config: vec![],
    }
}

/// The provider settings and credentials offer for a discovery report (provider ids only).
pub fn credentials_offer(report: &ImportReport) -> ImportOffer {
    let mut providers = Vec::new();
    let (mut kept, mut reserved) = (Vec::new(), Vec::new());
    for c in &report.credentials {
        if c.reserved.is_some() {
            reserved.push(c.provider.clone());
        } else if c.native_exists {
            kept.push(c.provider.clone());
        } else {
            providers.push(c.provider.clone());
        }
    }
    let config: Vec<String> = report
        .config
        .iter()
        .filter(|c| *c == "models.json")
        .cloned()
        .collect();
    let mut lines = Vec::new();
    if providers.is_empty() && config.is_empty() {
        lines.push(format!(
            "There are no legacy provider settings or credentials in {} to import.",
            report.agent_dir
        ));
    } else {
        let mut what = Vec::new();
        if !providers.is_empty() {
            what.push(format!("saved credentials for {}", providers.join(", ")));
        }
        if !config.is_empty() {
            what.push("provider settings (models.json)".to_owned());
        }
        lines.push(format!(
            "From {}: {} can be imported into the built-in engine.",
            report.agent_dir,
            what.join(" and ")
        ));
    }
    if !kept.is_empty() {
        lines.push(format!(
            "Already set up here and kept as they are: {}.",
            kept.join(", ")
        ));
    }
    if !reserved.is_empty() {
        lines.push(format!(
            "Kept in the legacy profile for a later integration, not imported: {}.",
            reserved.join(", ")
        ));
    }
    if let Some(e) = &report.credentials_error {
        lines.push(format!(
            "The legacy profile's saved credentials could not be read: {e}."
        ));
    }
    if !providers.is_empty() || !config.is_empty() {
        lines.push(
            "Existing settings here are never overwritten, and the legacy profile is not changed."
                .into(),
        );
    }
    ImportOffer {
        kind: ImportKind::Credentials,
        summary: lines.join("\n"),
        sessions: vec![],
        providers,
        config,
    }
}

/// What a session import did, in words, with the native session ids imported now and before.
/// `after` (a discovery made after the import) adds the sessions that were not offered because
/// they are in use or cannot be imported, so the account is complete.
pub fn conversations_result(
    view: &ImportedView,
    after: Option<&ImportReport>,
) -> (String, Vec<String>, Vec<String>) {
    let (mut imported, mut existing) = (Vec::new(), Vec::new());
    let (mut locked, mut blocked, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    let asked = |id: &str| view.sessions.iter().any(|s| s.source == id);
    for s in after.map_or(&[][..], |r| &r.sessions[..]) {
        if asked(&s.id) {
            continue;
        }
        match &s.status {
            ImportSessionStatus::Locked { detail } => locked.push(match detail {
                Some(d) => format!("{} ({d})", short(&s.id)),
                None => short(&s.id),
            }),
            ImportSessionStatus::Blocked { reasons } => {
                blocked.push(format!("{}: {}", short(&s.id), reasons.join(", ")))
            }
            _ => {}
        }
    }
    for s in &view.sessions {
        match &s.outcome {
            ImportSessionOutcome::Imported { native, .. } => imported.push(native.clone()),
            ImportSessionOutcome::AlreadyImported { native } => existing.push(native.clone()),
            ImportSessionOutcome::Locked { detail } => {
                locked.push(format!("{} ({detail})", short(&s.source)))
            }
            ImportSessionOutcome::Blocked { reasons } => {
                blocked.push(format!("{}: {}", short(&s.source), reasons.join(", ")))
            }
            ImportSessionOutcome::Failed { detail } => {
                failed.push(format!("{}: {detail}", short(&s.source)))
            }
        }
    }
    let mut lines = vec![format!(
        "Imported {} from the legacy profile.",
        plural(imported.len(), "conversation", "conversations")
    )];
    if !existing.is_empty() {
        lines.push(format!(
            "{} imported earlier.",
            plural(existing.len(), "conversation was", "conversations were")
        ));
    }
    if !locked.is_empty() {
        lines.push(format!(
            "In use by a running legacy engine, not imported: {}.",
            listed(&locked)
        ));
    }
    if !blocked.is_empty() {
        lines.push(format!("Cannot be imported: {}.", listed(&blocked)));
    }
    if !failed.is_empty() {
        lines.push(format!("Failed: {}.", listed(&failed)));
    }
    if !imported.is_empty() {
        lines.push(OWNERSHIP.into());
    }
    (lines.join("\n"), imported, existing)
}

fn call_failed(what: &str, e: &CallError) -> String {
    match e {
        CallError::Refused(p) => format!("{what}: {}", p.message),
        CallError::NotSent(m) => format!("{what}: {m}"),
        CallError::Lost(m) => format!(
            "{what}: the engine did not answer ({m}). Whether it finished is unknown; running \
             the import again is safe."
        ),
    }
}

impl Live {
    fn import_call(&self, call: Call) -> Result<Reply, CallError> {
        self.client.call(call, IMPORT_TIMEOUT)
    }

    fn discover(&self, worker: &Worker) -> Result<ImportReport, String> {
        match self.import_call(Call::ImportDiscover {
            server_dir: None,
            agent_dir: worker.config.import_agent_dir.clone(),
        }) {
            Ok(Reply::ImportReport(report)) => Ok(report),
            Ok(other) => Err(format!("unexpected reply {other:?}")),
            Err(e) => Err(call_failed("The legacy profile could not be read", &e)),
        }
    }

    pub(super) fn import(&mut self, worker: &mut Worker, step: ImportStep, done: ImportDone) {
        let result = match step {
            ImportStep::Discover(kind) => {
                ImportResult::Offer(self.discover(worker).map(|r| match kind {
                    ImportKind::Conversations => conversations_offer(&r),
                    ImportKind::Credentials => credentials_offer(&r),
                }))
            }
            ImportStep::Conversations(sessions) => self.import_conversations(worker, sessions),
            ImportStep::Credentials { providers, config } => {
                self.import_credentials(worker, providers, config)
            }
        };
        done(result);
    }

    fn import_conversations(&mut self, worker: &mut Worker, sessions: Vec<String>) -> ImportResult {
        let view = match self.import_call(Call::ImportSessions {
            server_dir: None,
            agent_dir: worker.config.import_agent_dir.clone(),
            sessions,
        }) {
            Ok(Reply::Imported(view)) => view,
            Ok(other) => {
                return unavailable(
                    &ImportStep::Conversations(vec![]),
                    &format!("unexpected reply {other:?}"),
                );
            }
            Err(e) => {
                return ImportResult::Finished {
                    message: call_failed("The conversations were not imported", &e),
                    imported: vec![],
                    existing: vec![],
                };
            }
        };
        let after = self.discover(worker).ok();
        let (message, imported, existing) = conversations_result(&view, after.as_ref());
        // The catalogue now lists the imported sessions under the ids the Pi adapter gave them.
        if let Err(e) = self.refresh_sessions() {
            log::warn!("could not re-read the conversations after an import: {e}");
        }
        (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
        let ids = |sessions: &[String]| {
            sessions
                .iter()
                .filter_map(|s| self.conversation_of(s))
                .collect::<Vec<_>>()
        };
        ImportResult::Finished {
            message,
            imported: ids(&imported),
            existing: ids(&existing),
        }
    }

    fn import_credentials(
        &mut self,
        worker: &mut Worker,
        providers: Vec<String>,
        config: Vec<String>,
    ) -> ImportResult {
        let mut lines = Vec::new();
        if !config.is_empty() {
            match self.import_call(Call::ImportConfig {
                agent_dir: worker.config.import_agent_dir.clone(),
                items: config,
            }) {
                Ok(Reply::ConfigImported(view)) => {
                    for item in view.items {
                        lines.push(match item.outcome {
                            ConfigItemOutcome::Copied { .. }
                            | ConfigItemOutcome::Partial { .. } => {
                                format!("Copied {}.", item.name)
                            }
                            ConfigItemOutcome::KeptExisting => {
                                format!("Kept the existing {} here.", item.name)
                            }
                            ConfigItemOutcome::NotFound => format!("{} was not found.", item.name),
                            ConfigItemOutcome::Failed { detail } => {
                                format!("{} was not copied: {detail}", item.name)
                            }
                        });
                    }
                }
                Ok(other) => lines.push(format!("Unexpected reply {other:?}")),
                Err(e) => lines.push(call_failed("Provider settings were not imported", &e)),
            }
        }
        if !providers.is_empty() {
            match self.import_call(Call::ImportCredentials {
                agent_dir: worker.config.import_agent_dir.clone(),
                providers,
            }) {
                Ok(Reply::CredentialsImported(view)) => {
                    let pick = |want: fn(&CredentialImportOutcome) -> bool| {
                        view.providers
                            .iter()
                            .filter(|p| want(&p.outcome))
                            .map(|p| p.provider.clone())
                            .collect::<Vec<_>>()
                    };
                    let imported = pick(|o| *o == CredentialImportOutcome::Imported);
                    let kept = pick(|o| *o == CredentialImportOutcome::KeptExisting);
                    let reserved = pick(|o| matches!(o, CredentialImportOutcome::Reserved { .. }));
                    let missing = pick(|o| *o == CredentialImportOutcome::NotFound);
                    if !imported.is_empty() {
                        lines.push(format!(
                            "Imported saved credentials for {}.",
                            imported.join(", ")
                        ));
                    }
                    if !kept.is_empty() {
                        lines.push(format!(
                            "Kept the credentials already set up here for {}.",
                            kept.join(", ")
                        ));
                    }
                    if !reserved.is_empty() {
                        lines.push(format!(
                            "Kept in the legacy profile for a later integration: {}.",
                            reserved.join(", ")
                        ));
                    }
                    if !missing.is_empty() {
                        lines.push(format!(
                            "No longer saved in the legacy profile: {}.",
                            missing.join(", ")
                        ));
                    }
                }
                Ok(other) => lines.push(format!("Unexpected reply {other:?}")),
                Err(e) => lines.push(call_failed("Credentials were not imported", &e)),
            }
        }
        self.refresh_models(worker, None);
        (worker.sink)(LifecycleEvent::Catalog(self.catalog(worker)));
        if lines.is_empty() {
            lines.push("Nothing was imported from the legacy profile.".into());
        }
        lines.push("The legacy profile was not changed.".into());
        ImportResult::Finished {
            message: lines.join("\n"),
            imported: vec![],
            existing: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use pipkin_engine_protocol::{ImportCredentialView, ImportSessionResult, ImportSessionView};

    use super::*;

    fn session(id: &str, status: ImportSessionStatus) -> ImportSessionView {
        ImportSessionView {
            id: id.into(),
            cwd: None,
            created_at_ms: None,
            status,
        }
    }

    #[test]
    fn a_conversation_offer_lists_what_can_and_cannot_be_imported() {
        let report = ImportReport {
            agent_dir: "/home/u/.pi/agent".into(),
            sessions: vec![
                session("a", ImportSessionStatus::Importable),
                session(
                    "b",
                    ImportSessionStatus::AlreadyImported { native: "b".into() },
                ),
                session(
                    "c",
                    ImportSessionStatus::Locked {
                        detail: Some("lock refreshed 3 ms ago".into()),
                    },
                ),
                session(
                    "d",
                    ImportSessionStatus::Blocked {
                        reasons: vec!["a fork".into()],
                    },
                ),
            ],
            ..ImportReport::default()
        };
        let offer = conversations_offer(&report);
        assert_eq!(offer.sessions, vec!["a"]);
        assert!(offer.has_work());
        assert!(!offer.summary.contains("--engine pi"));
        for part in [
            "1 legacy conversation from /home/u/.pi/agent can be imported",
            "1 conversation was imported earlier",
            "c (lock refreshed 3 ms ago)",
            "d: a fork",
            "original legacy profile is unchanged and retained as a backup",
            "built-in engine only",
        ] {
            assert!(offer.summary.contains(part), "{part}: {}", offer.summary);
        }
        let none = conversations_offer(&ImportReport::default());
        assert!(!none.has_work());
        assert!(
            none.summary
                .starts_with("No legacy conversations were found")
        );
    }

    #[test]
    fn a_credentials_offer_names_providers_only_and_keeps_reserved_ones_out() {
        let cred = |provider: &str, reserved: bool, native: bool| ImportCredentialView {
            provider: provider.into(),
            kind: "api_key".into(),
            reserved: reserved.then(|| "later".into()),
            native_exists: native,
        };
        let report = ImportReport {
            agent_dir: "/a".into(),
            config: vec!["models.json".into(), "AGENTS.md".into()],
            credentials: vec![
                cred("openrouter", false, false),
                cred("groq", false, true),
                cred("anthropic", true, false),
            ],
            ..ImportReport::default()
        };
        let offer = credentials_offer(&report);
        assert_eq!(offer.providers, vec!["openrouter"]);
        assert_eq!(offer.config, vec!["models.json"]);
        assert!(offer.summary.contains("saved credentials for openrouter"));
        assert!(offer.summary.contains("kept as they are: groq"));
        assert!(
            offer
                .summary
                .contains("later integration, not imported: anthropic")
        );
        assert!(!credentials_offer(&ImportReport::default()).has_work());
    }

    #[test]
    fn an_import_result_names_counts_and_reasons() {
        let row = |source: &str, outcome| ImportSessionResult {
            source: source.into(),
            outcome,
        };
        let view = ImportedView {
            sessions: vec![
                row(
                    "a",
                    ImportSessionOutcome::Imported {
                        native: "a".into(),
                        children: 0,
                        entries: 3,
                        requests: 1,
                    },
                ),
                row(
                    "b",
                    ImportSessionOutcome::AlreadyImported { native: "b".into() },
                ),
                row(
                    "c",
                    ImportSessionOutcome::Locked {
                        detail: "busy".into(),
                    },
                ),
                row(
                    "d",
                    ImportSessionOutcome::Blocked {
                        reasons: vec!["a fork".into()],
                    },
                ),
                row(
                    "e",
                    ImportSessionOutcome::Failed {
                        detail: "io".into(),
                    },
                ),
            ],
            backup_dir: None,
        };
        let after = ImportReport {
            sessions: vec![
                session(
                    "a",
                    ImportSessionStatus::AlreadyImported { native: "a".into() },
                ),
                session("f", ImportSessionStatus::Locked { detail: None }),
                session(
                    "g",
                    ImportSessionStatus::Blocked {
                        reasons: vec!["never opened".into()],
                    },
                ),
            ],
            ..ImportReport::default()
        };
        let (message, imported, existing) = conversations_result(&view, Some(&after));
        assert_eq!((imported, existing), (vec!["a".into()], vec!["b".into()]));
        assert!(!message.contains("--engine pi"));
        for part in [
            "Imported 1 conversation from the legacy profile.",
            "1 conversation was imported earlier.",
            "not imported: f; c (busy)",
            "Cannot be imported: g: never opened; d: a fork",
            "Failed: e: io",
            "original legacy profile is unchanged and retained as a backup",
        ] {
            assert!(message.contains(part), "{part}: {message}");
        }
    }
}
