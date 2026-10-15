//! Real core + adapter + engine, but only a disposable fake metadata executable.
use super::*;
use pipkin_core::local_cli::{CliModel, LocalCliCommand as C, LocalCliDraft, LocalCliStage};
use std::os::unix::fs::PermissionsExt;

#[test]
fn fake_cli_inspect_check_create_keeps_execution_disabled_and_uses_exact_root() {
    let fx = Fixture::start(|_, _| panic!("setup must never request native inference"));
    let executable = fx.root.join("fake-cli");
    let marker = fx.root.join("metadata-calls");
    let policy = pipkin_engine::execution::claude_cli::policy::REQUIRED_FLAGS.join("\n");
    let version = pipkin_engine::execution::claude_cli::policy::QUALIFIED_VERSION;
    let auth = r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fixture@example.invalid","orgId":"fixture"}"#;
    std::fs::write(&executable, format!(
        "#!/bin/sh\nprintf 'metadata\\n' >> '{}'\ncase \"$1\" in\n--version) printf '%s\\n' '{version} (Claude Code)';;\n--help) printf '%s\\n' '{policy}';;\n*) for arg in \"$@\"; do if [ \"$arg\" = '--print' ]; then exit 99; fi; done; printf '%s\\n' '{auth}';;\nesac\n",
        marker.display(),
    )).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = fx.config();
    assert!(
        !config.enable_local_claude_cli,
        "public execution remains default off"
    );
    config
        .env
        .vars
        .insert("HOME".into(), fx.root.display().to_string());
    config
        .env
        .vars
        .insert("PATH".into(), "/usr/bin:/bin".into());
    {
        let mut slot = fx.engine.lock().unwrap();
        slot.take().unwrap().shutdown(Duration::from_secs(5));
        *slot = Some(Arc::new(
            EngineHandle::start_with(
                config,
                registry_with(Arc::new(fx.provider.clone())),
                pipkin_engine::tools::builtin_tools(),
            )
            .unwrap(),
        ));
    }
    let mut h = Harness::start(&fx, &fx.root.join("desktop.db"));
    h.until("Hello setup capability", |s| {
        s.connection.is_ready() && s.local_cli.available
    });
    h.dispatch(Command::LocalCli(C::BeginNew(LocalCliDraft {
        mode: pipkin_core::CliMode::ChatOnly,
        executable: executable.display().to_string(),
        project: fx.project.display().to_string(),
        model: CliModel::Opus,
    })));
    h.dispatch(Command::LocalCli(C::Inspect));
    h.until("inspected without executing", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    assert!(
        !marker.exists(),
        "Inspect must not execute even the fake CLI"
    );
    assert!(!h.state.local_cli.trust_executable && !h.state.local_cli.trust_project);
    let ticket = h.state.local_cli.ticket.clone().unwrap();
    assert_eq!(ticket.executable, executable.display().to_string());
    let root = ticket.root_candidate.clone().unwrap();
    assert_eq!(root.0, format!("cli-{}", ticket.setup.0));
    h.dispatch(Command::LocalCli(C::TrustExecutable(true)));
    h.dispatch(Command::LocalCli(C::TrustProject(true)));
    h.dispatch(Command::LocalCli(C::Check));
    h.until("metadata verified", |s| {
        s.local_cli.stage == LocalCliStage::Verified
    });
    assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 3);
    assert!(fx.requests().is_empty());
    h.dispatch(Command::LocalCli(C::Create));
    h.until("exact CLI conversation attached", |s| {
        s.local_cli.stage == LocalCliStage::Created && s.current().is_some_and(|c| c.opened)
    });
    let created = h.state.local_cli.created.as_ref().unwrap();
    assert_eq!(created.identity.session, root);
    assert_eq!(created.identity.setup, ticket.setup);
    let current = h.state.current().unwrap();
    assert_eq!(current.id, created.conversation);
    assert_eq!(
        current.profile,
        pipkin_core::ExecutionProfile::LocalCliChatOnly
    );
    assert!(
        current.execution_blocked.is_some(),
        "metadata is not an execution grant"
    );
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap().lines().count(),
        3,
        "Create cannot rerun metadata or inference"
    );
    assert!(fx.requests().is_empty());
    h.stop();
}

/// Only this fixture opts into execution. No inherited environment, installed CLI,
/// account, or native provider participates in either turn.
#[path = "local_cli_creation_recovery_e2e.rs"]
mod creation_recovery;

struct FakeShell {
    executable: PathBuf,
    metadata: PathBuf,
    launches: PathBuf,
}

impl FakeShell {
    fn install(fx: &Fixture) -> Self {
        let fake = Self {
            executable: fx.root.join("fake-chat-cli"),
            metadata: fx.project.join("metadata"),
            launches: fx.project.join("launches"),
        };
        let help = pipkin_engine::execution::claude_cli::policy::REQUIRED_FLAGS.join("\n")
            + "\n--settings";
        let version = pipkin_engine::execution::claude_cli::policy::QUALIFIED_VERSION;
        std::fs::write(
            &fake.executable,
            format!(
                r##"#!/bin/sh
cd '{project}' || exit 90
# Never log argv or stdin: only the launch mode and generated UUID are recorded.
printing=no
previous=''
uuid=''
mode=''
for arg in "$@"; do
    if [ "$arg" = '--print' ]; then printing=yes; fi
    if [ "$previous" = '--session-id' ]; then uuid="$arg"; mode=fresh; fi
    if [ "$previous" = '--resume' ]; then uuid="$arg"; mode=resume; fi
    previous="$arg"
done
if [ "$printing" = no ]; then
    case "$1" in
        --version) printf 'version\n' >> metadata; printf '%s\n' '{version} (Claude Code)';;
        --help) printf 'help\n' >> metadata; printf '%s\n' '{help}';;
        *)
            auth=no
            for arg in "$@"; do if [ "$arg" = 'auth' ]; then auth=yes; fi; done
            [ "$auth" = yes ] || exit 91
            printf 'auth\n' >> metadata
            printf '%s\n' '{{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fixture@example.invalid","orgId":"fixture"}}';;
    esac
    exit 0
fi
[ -n "$uuid" ] && [ -n "$mode" ] || exit 92
cat > /dev/null
printf '%s %s\n' "$mode" "$uuid" >> launches
# Deliberately nonsecret stderr, never an echo of the prompt.
printf 'synthetic CLI diagnostic\n' >&2
cat <<EOF
{{"type":"system","subtype":"init","session_id":"$uuid","tools":[]}}
{{"type":"assistant","session_id":"$uuid","message":{{"id":"answer-$mode","role":"assistant","content":[{{"type":"text","text":"Synthetic external $mode answer"}}]}}}}
{{"type":"result","session_id":"$uuid","subtype":"success","is_error":false,"result":"Synthetic external $mode answer"}}
EOF
"##,
                project = fx.project.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake.executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut config = fx.config();
        assert!(!config.enable_local_claude_cli);
        config.enable_local_claude_cli = true;
        // Fixture::config already uses Env::isolated; these paths are disposable too.
        config
            .env
            .vars
            .insert("HOME".into(), fx.root.display().to_string());
        config
            .env
            .vars
            .insert("PATH".into(), "/usr/bin:/bin".into());
        config.env.vars.insert(
            "XDG_CONFIG_HOME".into(),
            fx.root.join("xdg-config").display().to_string(),
        );
        let mut slot = fx.engine.lock().unwrap();
        slot.take().unwrap().shutdown(Duration::from_secs(5));
        *slot = Some(Arc::new(
            EngineHandle::start_with(
                config,
                registry_with(Arc::new(fx.provider.clone())),
                pipkin_engine::tools::builtin_tools(),
            )
            .unwrap(),
        ));
        fake
    }

    fn draft(&self, fx: &Fixture) -> LocalCliDraft {
        LocalCliDraft {
            mode: pipkin_core::CliMode::ChatOnly,
            executable: self.executable.display().to_string(),
            project: fx.project.display().to_string(),
            model: CliModel::Opus,
        }
    }

    fn lines(path: &Path) -> Vec<String> {
        match std::fs::read_to_string(path) {
            Ok(text) => text.lines().map(str::to_owned).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => panic!("reading fixture marker {}: {e}", path.display()),
        }
    }

    fn assert_counts(&self, metadata: usize, launches: usize) {
        assert_eq!(
            Self::lines(&self.metadata).len(),
            metadata,
            "metadata calls"
        );
        assert_eq!(
            Self::lines(&self.launches).len(),
            launches,
            "print launches"
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn coding_setup_crosses_core_adapter_engine_but_never_gains_chat_authority() {
    use pipkin_core::CliMode;
    let fx = Fixture::start(|_, _| panic!("coding setup must never request native inference"));
    let fake = FakeShell::install(&fx); // Chat opt-in deliberately enabled.
    let mut h = Harness::start(&fx, &fx.root.join("coding-setup.db"));
    h.until("setup capability", |s| {
        s.connection.is_ready() && s.local_cli.available
    });
    let mut draft = fake.draft(&fx);
    draft.mode = CliMode::RestrictedWrite;
    h.dispatch(Command::LocalCli(C::BeginNew(draft.clone())));
    h.dispatch(Command::LocalCli(C::Inspect));
    h.until("coding inspection", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    assert_eq!(
        h.state.local_cli.ticket.as_ref().unwrap().mode,
        CliMode::RestrictedWrite
    );
    fake.assert_counts(0, 0);
    h.dispatch(Command::LocalCli(C::TrustExecutable(true)));
    h.dispatch(Command::LocalCli(C::TrustProject(true)));
    h.dispatch(Command::LocalCli(C::Check));
    h.until("coding metadata", |s| {
        s.local_cli.stage == LocalCliStage::Verified
    });
    h.dispatch(Command::LocalCli(C::Create));
    h.until("coding root", |s| {
        s.local_cli.stage == LocalCliStage::Created && s.current().is_some_and(|c| c.opened)
    });
    let created = h.state.local_cli.created.clone().unwrap();
    assert_eq!(
        h.state.current().unwrap().profile,
        pipkin_core::ExecutionProfile::LocalCliRestrictedWrite
    );
    assert!(!h.state.execution_capabilities().model_settings);
    assert!(!h.state.execution_capabilities().attachments);
    assert!(h.state.current().unwrap().execution_blocked.is_some());
    h.dispatch(Command::EditDraft("never execute from metadata".into()));
    assert!(!h.state.availability().submit);
    h.dispatch(Command::Submit);
    fake.assert_counts(3, 0);
    h.dispatch(Command::LocalCli(C::BeginExisting {
        conversation: created.conversation,
        session: created.identity.session.clone(),
    }));
    h.dispatch(Command::LocalCli(C::SessionStatus));
    h.until("saved coding mode", |s| {
        s.local_cli.session_status.is_some()
    });
    let hints = h.state.local_cli.session_status.as_ref().unwrap();
    assert_eq!(hints.mode, CliMode::RestrictedWrite);
    assert_eq!(
        h.state.local_cli.draft.mode,
        CliMode::ChatOnly,
        "hints do not edit or approve automatically"
    );
    h.dispatch(Command::LocalCli(C::Edit(draft.clone())));
    h.dispatch(Command::LocalCli(C::Inspect));
    h.until("existing coding inspection", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    h.dispatch(Command::LocalCli(C::TrustExecutable(true)));
    h.dispatch(Command::LocalCli(C::TrustProject(true)));
    h.dispatch(Command::LocalCli(C::Check));
    h.until("existing coding metadata", |s| {
        s.local_cli.stage == LocalCliStage::Verified
    });
    assert!(h.state.current().unwrap().execution_blocked.is_some());
    fake.assert_counts(6, 0);
    draft.mode = CliMode::ChatOnly;
    h.dispatch(Command::LocalCli(C::Edit(draft)));
    h.dispatch(Command::LocalCli(C::Inspect));
    h.until("cross-policy reapproval rejected", |s| {
        s.local_cli.stage == LocalCliStage::Failed
    });
    fake.assert_counts(6, 0);
    assert!(fx.requests().is_empty());
    h.stop();
}

#[test]
fn fake_cli_chat_revoke_reapprove_resumes_exact_root_without_native_inference() {
    let fx = Fixture::start(|_, _| panic!("CLI chat must never request native inference"));
    let fake = FakeShell::install(&fx);
    let db = fx.root.join("chat-desktop.db");
    // A real, valid native catalog preference must survive CLI setup and both turns.
    // Leaving this None would miss accidental forwarding of native model overrides.
    let storage = Storage::open(&db, NAMESPACE).unwrap();
    storage.save_prefs(&pipkin_core::Prefs {
        model: Some("scripted/scripted".into()),
        ..Default::default()
    });
    storage.shutdown();
    let mut h = Harness::start(&fx, &db);
    h.until("CLI setup capability and native model catalog", |s| {
        s.connection.is_ready()
            && s.local_cli.available
            && s.models.iter().any(|m| m.id == "scripted/scripted")
    });
    assert_eq!(h.state.prefs.model.as_deref(), Some("scripted/scripted"));
    h.dispatch(Command::LocalCli(C::BeginNew(fake.draft(&fx))));
    fake.assert_counts(0, 0);
    h.dispatch(Command::LocalCli(C::Inspect));
    h.until("fresh inspection", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    fake.assert_counts(0, 0);
    assert!(!h.state.local_cli.trust_executable && !h.state.local_cli.trust_project);
    let ticket = h.state.local_cli.ticket.clone().unwrap();
    let root = ticket.root_candidate.clone().unwrap();
    assert_eq!(root.0, format!("cli-{}", ticket.setup.0));
    h.dispatch(Command::LocalCli(C::TrustExecutable(true)));
    h.dispatch(Command::LocalCli(C::TrustProject(true)));
    h.dispatch(Command::LocalCli(C::Check));
    h.until("fresh metadata approval", |s| {
        s.local_cli.stage == LocalCliStage::Verified
    });
    fake.assert_counts(3, 0);
    h.dispatch(Command::LocalCli(C::Create));
    h.until("authoritative CLI attachment ready", |s| {
        s.local_cli.stage == LocalCliStage::Created
            && s.current().is_some_and(|c| {
                c.opened
                    && c.profile == pipkin_core::ExecutionProfile::LocalCliChatOnly
                    && c.execution_blocked.is_none()
            })
    });
    let created = h.state.local_cli.created.clone().unwrap();
    assert_eq!(created.identity.session, root);
    assert_eq!(created.identity.setup, ticket.setup);
    let conversation = created.conversation;
    assert_eq!(h.state.selected, Some(conversation));
    fake.assert_counts(3, 0); // Create/Open must not launch a turn.
    assert!(fx.requests().is_empty());

    h.send("synthetic private first prompt");
    let intent = h
        .state
        .current()
        .unwrap()
        .pending_intent
        .as_ref()
        .expect("journal admission");
    assert_eq!(intent.model, None);
    h.until(
        "visible external fresh answer and settled journaled submission",
        |s| is_idle(s) && assistant_texts(s) == ["Synthetic external fresh answer"],
    );
    // Engine CLI admission rejects a native model override. Success therefore also
    // proves the adapter sent None, not merely that the core journal omitted it.
    assert_eq!(h.user_messages(), ["synthetic private first prompt"]);
    fake.assert_counts(6, 1); // Check + per-turn metadata verification.
    assert!(fx.requests().is_empty());
    assert_eq!(h.state.prefs.model.as_deref(), Some("scripted/scripted"));

    assert!(h.state.local_cli_availability().revoke);
    h.dispatch(Command::LocalCli(C::Revoke));
    h.until("idle revocation reaches the authoritative snapshot", |s| {
        is_idle(s)
            && s.local_cli_availability().begin
            && s.current().is_some_and(|c| c.execution_blocked.is_some())
    });
    assert_eq!(
        h.state.current().unwrap().profile,
        pipkin_core::ExecutionProfile::LocalCliChatOnly
    );
    fake.assert_counts(6, 1);
    h.dispatch(Command::LocalCli(C::BeginExisting {
        conversation,
        session: root.clone(),
    }));
    h.dispatch(Command::LocalCli(C::Edit(fake.draft(&fx))));
    h.dispatch(Command::LocalCli(C::Inspect));
    h.until("existing exact-root inspection", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    let reapproval = h.state.local_cli.ticket.as_ref().unwrap();
    assert_eq!(
        reapproval.target,
        pipkin_core::local_cli::LocalCliInspectTarget::Existing {
            session: root.clone(),
            model: CliModel::Opus,
        }
    );
    assert!(reapproval.root_candidate.is_none());
    assert_ne!(reapproval.setup, ticket.setup);
    assert!(!h.state.local_cli.trust_executable && !h.state.local_cli.trust_project);
    fake.assert_counts(6, 1);
    h.dispatch(Command::LocalCli(C::TrustExecutable(true)));
    h.dispatch(Command::LocalCli(C::TrustProject(true)));
    h.dispatch(Command::LocalCli(C::Check));
    h.until("existing metadata reapproved", |s| {
        s.local_cli.stage == LocalCliStage::Verified
    });
    fake.assert_counts(9, 1);
    assert!(!h.state.local_cli_availability().create);
    assert!(h.state.local_cli_availability().return_to_conversation);
    let generation = h.state.current().unwrap().generation;
    h.dispatch(Command::LocalCli(C::ReturnToConversation));
    h.until("new attachment restores authoritative CLI readiness", |s| {
        s.selected == Some(conversation)
            && s.current().is_some_and(|c| {
                c.opened
                    && c.generation > generation
                    && c.profile == pipkin_core::ExecutionProfile::LocalCliChatOnly
                    && c.execution_blocked.is_none()
            })
    });
    fake.assert_counts(9, 1); // Reapproval/Return never submit automatically.
    assert_eq!(h.state.prefs.model.as_deref(), Some("scripted/scripted"));
    h.send("synthetic private second prompt");
    assert_eq!(
        h.state
            .current()
            .unwrap()
            .pending_intent
            .as_ref()
            .unwrap()
            .model,
        None
    );
    h.until("visible external resumed answer", |s| {
        is_idle(s)
            && assistant_texts(s)
                == [
                    "Synthetic external fresh answer",
                    "Synthetic external resume answer",
                ]
    });
    assert_eq!(
        h.user_messages(),
        [
            "synthetic private first prompt",
            "synthetic private second prompt"
        ]
    );
    assert!(
        h.state
            .current()
            .unwrap()
            .items
            .iter()
            .filter_map(|item| {
                match &item.kind {
                    ItemKind::User { delivery, .. } => Some(delivery),
                    _ => None,
                }
            })
            .all(|delivery| *delivery == Delivery::Sent)
    );
    fake.assert_counts(12, 2);
    let launches = FakeShell::lines(&fake.launches);
    let uuid = launches[0]
        .strip_prefix("fresh ")
        .expect("one fresh launch");
    assert!(pipkin_engine::execution::claude_cli::policy::valid_uuid(
        uuid
    ));
    assert_eq!(
        launches[1],
        format!("resume {uuid}"),
        "same CLI UUID resumes"
    );
    assert!(
        fx.requests().is_empty(),
        "no native fallback on either turn"
    );
    assert_eq!(h.state.prefs.model.as_deref(), Some("scripted/scripted"));
    h.stop();
    assert_eq!(
        open_requests(&db),
        0,
        "both journal entries reached terminal state"
    );
}
