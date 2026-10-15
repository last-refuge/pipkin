//! Pure database/readiness views. A status check never creates a session, probes the CLI,
//! reapproves trust, or submits a prompt.
use super::Shared;
use pipkin_engine_protocol::*;

impl Shared {
    /// Readiness is connection-scoped. Apply only after dispatch has released all
    /// publication/registry locks; this read-only hint never substitutes for admission.
    pub(super) fn filter_local_cli_readiness(&self, owner: u64, reply: &mut Reply) {
        let filter = |summary: &mut SessionSummary| {
            if summary.execution_backend == ExecutionBackend::LocalClaudeCli
                && summary.blocked.is_none()
                && !self.local_cli_setups.owns_grant(owner, &summary.id)
            {
                summary.blocked =
                    Some("Local CLI approval is required for this connection.".into());
            }
        };
        match reply {
            Reply::Sessions { sessions } => sessions.iter_mut().for_each(filter),
            Reply::Session { summary }
            | Reply::LocalCliCreation {
                summary: Some(summary),
            } => filter(summary),
            Reply::Snapshot(snapshot) => filter(&mut snapshot.summary),
            Reply::LocalCliSession(view) => filter(&mut view.summary),
            _ => {}
        }
    }
}

impl Shared {
    pub(super) fn lookup_local_cli_creation(
        &self,
        session: &str,
        setup: &str,
    ) -> Result<Reply, ProtoError> {
        Ok(Reply::LocalCliCreation {
            summary: self
                .store
                .cli_creation(session, setup)?
                .map(|row| self.execution_summary(&row)),
        })
    }

    pub(super) fn local_cli_session_status(&self, session: &str) -> Result<Reply, ProtoError> {
        let row = self
            .store
            .session(session)?
            .ok_or_else(|| ProtoError::new(ErrorCode::NotFound, "Session does not exist"))?;
        if row.execution_backend != ExecutionBackend::LocalClaudeCli
            || row.parent.is_some()
            || row.root.is_some()
        {
            return Err(ProtoError::new(
                ErrorCode::Unsupported,
                "This is not a local CLI root conversation",
            ));
        }
        let binding = self
            .store
            .cli_binding(session)?
            .ok_or_else(|| ProtoError::new(ErrorCode::Conflict, "CLI binding is unavailable"))?;
        let model = match row.model.as_deref() {
            Some("haiku") => Some(LocalCliModel::Haiku),
            Some("sonnet") => Some(LocalCliModel::Sonnet),
            Some("opus") => Some(LocalCliModel::Opus),
            _ => None,
        };
        Ok(Reply::LocalCliSession(LocalCliSessionView {
            summary: self.execution_summary(&row),
            mode: super::local_cli_setup::binding_mode(&binding.identity.launch_policy),
            executable: binding.identity.executable_path,
            cli_version: binding.identity.cli_version,
            model,
            context_valid: binding.context_valid,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{provider::scripted::Scripted, runtime::tests::Fixture, store::CliBindingSpec};

    #[test]
    fn snapshot_releases_publication_before_waiting_for_the_trust_registry() {
        use std::time::{Duration, Instant};
        let fixture = Fixture::new();
        let provider = Scripted::echo("must not run");
        let mut config = fixture.config();
        config.enable_local_claude_cli = true;
        let engine = fixture.start_config(&provider, config);
        let shared = engine.handle.shared().clone();
        shared
            .store
            .create_cli_session(
                "cli",
                "approval",
                &CliBindingSpec {
                    cli_session_id: "11111111-1111-4111-8111-111111111111".into(),
                    executable_path: "/not-executed".into(),
                    project_path: fixture.project.to_str().unwrap().into(),
                    cli_version: "2.1.296".into(),
                    config_ref: "fixture".into(),
                    account_scope: "fixture".into(),
                    launch_policy: "fixture".into(),
                },
                1,
            )
            .unwrap();
        // Keep volatile session caches resident; their capture needs no store access.
        let _runtime = shared.session("cli").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let result = shared.local_cli_setups.with_admission(None, || {
            // Hold the store until the snapshot has acquired publication. It cannot yet
            // reach the readiness overlay, so the ordering is deterministic.
            shared.store.read(|_| {
                let reader = shared.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(reader.snapshot("cli", 20));
                });
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    if shared.publication.try_lock().is_err() {
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "snapshot did not acquire publication"
                    );
                    std::thread::yield_now();
                }
                Ok(())
            })?;
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if shared.publication.try_lock().is_ok() {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(ProtoError::new(
                        ErrorCode::Internal,
                        "snapshot retained publication while waiting for trust",
                    ));
                }
                std::thread::yield_now();
            }
        });
        // Even the failure path releases trust before joining the reader.
        rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        result.unwrap();
    }

    #[test]
    fn lookup_and_reapproval_hints_never_create_probe_or_expose_private_binding_fields() {
        let fixture = Fixture::new();
        let provider = Scripted::echo("must not run");
        let mut engine = fixture.start(&provider);
        assert!(matches!(
            engine
                .call(Call::LookupLocalCliCreation {
                    session: "cli".into(),
                    setup: "approval".into()
                })
                .unwrap(),
            Reply::LocalCliCreation { summary: None }
        ));
        assert!(
            engine
                .handle
                .shared()
                .store
                .session("cli")
                .unwrap()
                .is_none()
        );
        let binding = CliBindingSpec {
            cli_session_id: "11111111-1111-4111-8111-111111111111".into(),
            executable_path: "/not-an-installed-cli".into(),
            project_path: fixture.project.to_str().unwrap().into(),
            cli_version: "2.1.296".into(),
            account_scope: "private-marker-account".into(),
            config_ref: "private-marker-config".into(),
            launch_policy: "fixture".into(),
        };
        engine
            .handle
            .shared()
            .store
            .create_cli_session_with_model("cli", "approval", &binding, "sonnet", 1)
            .unwrap();
        let reply = engine
            .call(Call::LocalCliSessionStatus {
                session: "cli".into(),
            })
            .unwrap();
        let json = serde_json::to_string(&reply).unwrap();
        for hidden in [
            &binding.cli_session_id,
            &binding.account_scope,
            &binding.config_ref,
        ] {
            assert!(!json.contains(hidden));
        }
        assert!(matches!(
            reply,
            Reply::LocalCliSession(LocalCliSessionView {
                model: Some(LocalCliModel::Sonnet),
                context_valid: true,
                summary: SessionSummary {
                    blocked: Some(_),
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            engine
                .call(Call::LookupLocalCliCreation {
                    session: "cli".into(),
                    setup: "approval".into()
                })
                .unwrap(),
            Reply::LocalCliCreation { summary: Some(_) }
        ));
        assert!(provider.requests().is_empty());
    }
}
