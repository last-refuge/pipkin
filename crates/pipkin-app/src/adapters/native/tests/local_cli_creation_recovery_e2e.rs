use super::*;

fn verified(fx: &Fixture, fake: &FakeShell, db: &Path) -> Harness {
    let mut h = Harness::start(fx, db);
    h.until("setup capability", |s| {
        s.connection.is_ready() && s.local_cli.available
    });
    h.dispatch(Command::LocalCli(C::BeginNew(fake.draft(fx))));
    h.dispatch(Command::LocalCli(C::Inspect));
    h.until("inspection", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    h.dispatch(Command::LocalCli(C::TrustExecutable(true)));
    h.dispatch(Command::LocalCli(C::TrustProject(true)));
    h.dispatch(Command::LocalCli(C::Check));
    h.until("verified metadata", |s| {
        s.local_cli.stage == LocalCliStage::Verified
    });
    h
}

#[test]
fn lost_creation_reply_survives_app_and_engine_restart_as_explicit_read_only_recovery() {
    let fx = Fixture::start(|_, _| panic!("no native inference"));
    let fake = FakeShell::install(&fx);
    let db = fx.root.join("creation.db");
    let mut h = verified(&fx, &fake, &db);
    fx.faults.drop_creation_reply.store(true, Ordering::SeqCst);
    h.dispatch(Command::LocalCli(C::Create));
    let identity = h.state.local_cli.creation.clone().unwrap();
    h.until("engine creation committed despite lost reply", |_| {
        fx.engine()
            .shared()
            .store
            .cli_creation(&identity.session.0, &identity.setup.0)
            .unwrap()
            .is_some()
    });
    assert_eq!(
        h.storage.load_cli_creations().unwrap(),
        vec![identity.clone()]
    );
    assert_eq!(fx.faults.creations.load(Ordering::SeqCst), 1);
    assert_eq!(fx.faults.creation_lookups.load(Ordering::SeqCst), 0);
    let metadata = FakeShell::lines(&fake.metadata).len();
    h.stop();
    fx.restart_engine();
    let mut restored = Harness::start(&fx, &db);
    restored.until("reconnected metadata API", |s| {
        s.connection.is_ready() && s.local_cli.available
    });
    assert_eq!(restored.state.local_cli.creation, Some(identity));
    assert_eq!(restored.state.local_cli.stage, LocalCliStage::CreateUnknown);
    assert!(restored.state.local_cli.ticket.is_none());
    assert!(!restored.state.local_cli.trust_project && !restored.state.local_cli.trust_executable);
    assert_eq!(fx.faults.creations.load(Ordering::SeqCst), 1);
    assert_eq!(fx.faults.creation_lookups.load(Ordering::SeqCst), 0);
    fake.assert_counts(metadata, 0);
    restored.dispatch(Command::LocalCli(C::LookupCreation));
    restored.until("explicit lookup found immutable conversation", |s| {
        s.local_cli.stage == LocalCliStage::Created
            && s.current().is_some_and(|c| {
                c.opened && c.profile.is_local_cli() && c.execution_blocked.is_some()
            })
    });
    let storage = restored.storage.clone();
    restored.until("exact recovery record retired", |_| {
        storage.load_cli_creations().unwrap().is_empty()
    });
    assert_eq!(fx.faults.creations.load(Ordering::SeqCst), 1);
    assert_eq!(fx.faults.creation_lookups.load(Ordering::SeqCst), 1);
    fake.assert_counts(metadata, 0);
    assert!(fx.requests().is_empty());
    restored.stop();
}

#[test]
fn explicit_fresh_handoff_uses_new_uuid_without_context_or_prompt_transfer() {
    let fx = Fixture::start(|_, _| panic!("no native inference"));
    let fake = FakeShell::install(&fx);
    let mut h = verified(&fx, &fake, &fx.root.join("handoff.db"));
    h.dispatch(Command::LocalCli(C::Create));
    h.until("initial metadata root", |s| {
        s.local_cli.stage == LocalCliStage::Created && s.current().is_some_and(|c| c.opened)
    });
    let old = h.state.local_cli.created.clone().unwrap();
    h.state
        .conversations
        .iter_mut()
        .find(|c| c.id == old.conversation)
        .unwrap()
        .draft
        .text = "source draft must not transfer".into();
    h.dispatch(Command::LocalCli(C::BeginExisting {
        conversation: old.conversation,
        session: old.identity.session.clone(),
    }));
    h.dispatch(Command::LocalCli(C::SessionStatus));
    h.until("read-only source hints", |s| {
        s.local_cli_availability().fresh_setup
    });
    let metadata = FakeShell::lines(&fake.metadata).len();
    let target = h.state.local_cli.target.clone().unwrap();
    h.dispatch(Command::LocalCli(C::FreshSetup(target)));
    assert_eq!(h.state.local_cli.stage, LocalCliStage::Editing);
    assert!(h.state.local_cli.ticket.is_none());
    assert!(!h.state.local_cli.trust_project && !h.state.local_cli.trust_executable);
    assert_eq!(fx.faults.creations.load(Ordering::SeqCst), 1);
    fake.assert_counts(metadata, 0);
    h.dispatch(Command::LocalCli(C::Inspect));
    h.until("separate inspection", |s| {
        s.local_cli.stage == LocalCliStage::AwaitingTrust
    });
    h.dispatch(Command::LocalCli(C::TrustExecutable(true)));
    h.dispatch(Command::LocalCli(C::TrustProject(true)));
    h.dispatch(Command::LocalCli(C::Check));
    h.until("separate verification", |s| {
        s.local_cli.stage == LocalCliStage::Verified
    });
    h.dispatch(Command::LocalCli(C::Create));
    h.until("fresh metadata root", |s| {
        s.local_cli.stage == LocalCliStage::Created
            && s.current()
                .is_some_and(|c| c.opened && c.id != old.conversation)
    });
    let fresh = h.state.local_cli.created.as_ref().unwrap();
    assert_ne!(fresh.identity, old.identity);
    let a = fx
        .engine()
        .shared()
        .store
        .cli_binding(&old.identity.session.0)
        .unwrap()
        .unwrap();
    let b = fx
        .engine()
        .shared()
        .store
        .cli_binding(&fresh.identity.session.0)
        .unwrap()
        .unwrap();
    assert_ne!(a.identity.cli_session_id, b.identity.cli_session_id);
    assert!(h.state.current().unwrap().draft.text.is_empty());
    assert!(h.state.current().unwrap().items.is_empty());
    assert_eq!(
        h.state.conversation(old.conversation).unwrap().draft.text,
        "source draft must not transfer"
    );
    assert_eq!(fx.faults.creations.load(Ordering::SeqCst), 2);
    assert!(FakeShell::lines(&fake.launches).is_empty());
    assert!(fx.requests().is_empty());
    h.stop();
}

#[test]
fn failed_recovery_journal_never_sends_create_and_does_not_release_it_later() {
    let fx = Fixture::start(|_, _| panic!("no native inference"));
    let fake = FakeShell::install(&fx);
    let db = fx.root.join("creation.db");
    let mut h = verified(&fx, &fake, &db);
    h.storage.set_write_failure(true);
    h.dispatch(Command::LocalCli(C::Create));
    assert_eq!(h.state.local_cli.stage, LocalCliStage::CreateUnknown);
    assert_eq!(fx.faults.creations.load(Ordering::SeqCst), 0);
    assert!(h.storage.load_cli_creations().unwrap().is_empty());
    h.storage.set_write_failure(false);
    h.pump();
    assert_eq!(fx.faults.creations.load(Ordering::SeqCst), 0);
    assert!(FakeShell::lines(&fake.launches).is_empty());
    assert!(fx.requests().is_empty());
    h.stop();
}
