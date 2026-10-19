use pipkin_core::*;

#[path = "local_cli/assistance.rs"]
mod assistance;
#[path = "local_cli/creation_recovery.rs"]
mod creation_recovery;
#[path = "local_cli/guided.rs"]
mod guided;

fn boot(ids: &[u64]) -> Bootstrap {
    Bootstrap {
        projects: vec![Project {
            id: ProjectId(1),
            name: "project".into(),
            path: "/project".into(),
        }],
        models: vec![],
        conversations: ids
            .iter()
            .map(|id| {
                (
                    ConversationId(*id),
                    ProjectId(1),
                    format!("session {id}"),
                    *id as i64,
                )
            })
            .collect(),
        now: 1,
    }
}
fn state() -> AppState {
    let mut s = AppState::new(boot(&[1, 2]), Prefs::default());
    s.mode = Mode::Real;
    s.local_cli.available = true;
    s
}
fn draft() -> LocalCliDraft {
    LocalCliDraft {
        thinking: None,
        mode: CliMode::ChatOnly,
        executable: "/bin/claude".into(),
        project: "/project".into(),
        model: CliModel::Sonnet,
    }
}
fn cmd(s: &mut AppState, c: LocalCliCommand) -> Outcome {
    let out = s.dispatch(Command::LocalCli(c));
    // Existing reducer tests emulate a successful writer; dedicated tests below exercise
    // delayed/failed acknowledgments directly, before any backend request may escape.
    if let Some(Effect::JournalLocalCliCreation {
        conversation,
        generation,
        op,
        identity,
    }) = out.effects.first()
    {
        s.local_cli_creation_persisted(*conversation, *generation, *op, identity, Ok(()))
    } else {
        out
    }
}
#[derive(Clone, Debug)]
struct Call {
    conversation: ConversationId,
    generation: u64,
    op: OperationId,
    request: LocalCliRequest,
}
fn call(out: Outcome) -> Call {
    let calls: Vec<_> = out
        .effects
        .into_iter()
        .filter_map(|e| match e {
            Effect::Backend(BackendRequest::LocalCli {
                conversation,
                generation,
                op,
                request,
            }) => Some(Call {
                conversation,
                generation,
                op,
                request,
            }),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 1, "one explicit setup request");
    calls[0].clone()
}
fn event(c: &Call, e: LocalCliEvent) -> BackendEvent {
    BackendEvent {
        conversation: c.conversation,
        generation: c.generation,
        op: Some(c.op),
        kind: EventKind::LocalCli(e),
    }
}
fn answer(s: &mut AppState, c: &Call, e: LocalCliEvent) -> Outcome {
    s.apply_event(event(c, e))
}
fn view(c: &Call) -> LocalCliSetupView {
    let LocalCliRequest::Inspect { target, mode, .. } = &c.request else {
        panic!()
    };
    LocalCliSetupView {
        mode: *mode,
        setup: SetupId("550e8400-e29b-41d4-a716-446655440000".into()),
        target: target.clone(),
        root_candidate: matches!(target, LocalCliInspectTarget::New { .. })
            .then(|| EngineSessionId("cli-550e8400-e29b-41d4-a716-446655440000".into())),
        executable: "/canonical/claude".into(),
        project: "/project".into(),
        qualified_version: "1.0".into(),
        stage: LocalCliMetadataStage::AwaitingTrust,
        expires_at: Some(100),
        reason: None,
    }
}
fn inspect(s: &mut AppState) -> LocalCliSetupView {
    let c = call(cmd(s, LocalCliCommand::Inspect));
    let v = view(&c);
    answer(s, &c, LocalCliEvent::Inspected(v.clone()));
    v
}
fn begin(s: &mut AppState) {
    assert!(
        cmd(s, LocalCliCommand::BeginNew(draft()))
            .effects
            .is_empty()
    );
}
fn check(s: &mut AppState) -> Call {
    cmd(s, LocalCliCommand::TrustExecutable(true));
    cmd(s, LocalCliCommand::TrustProject(true));
    call(cmd(s, LocalCliCommand::Check))
}
fn verified(s: &mut AppState) {
    begin(s);
    let mut v = inspect(s);
    let c = check(s);
    v.stage = LocalCliMetadataStage::MetadataVerified;
    answer(s, &c, LocalCliEvent::Checked(v));
    assert_eq!(s.local_cli.stage, LocalCliStage::Verified);
}
fn unknown(s: &mut AppState) -> LocalCliCreationId {
    verified(s);
    let c = call(cmd(s, LocalCliCommand::Create));
    let LocalCliRequest::Create { identity } = &c.request else {
        panic!()
    };
    answer(
        s,
        &c,
        LocalCliEvent::Failed {
            phase: LocalCliPhase::Create,
            failure: LocalCliFailure::Unknown("lost ack".into()),
        },
    );
    identity.clone()
}
fn no_effect(out: Outcome) {
    assert!(out.effects.is_empty(), "{out:?}");
}

#[test]
fn setup_mode_is_captured_and_cannot_change_in_async_replies() {
    let mut s = state();
    let mut d = draft();
    d.mode = CliMode::RestrictedWrite;
    cmd(&mut s, LocalCliCommand::BeginNew(d));
    let inspect = call(cmd(&mut s, LocalCliCommand::Inspect));
    let v = view(&inspect);
    assert_eq!(v.mode, CliMode::RestrictedWrite);
    let mut wrong = v.clone();
    wrong.mode = CliMode::ChatOnly;
    answer(&mut s, &inspect, LocalCliEvent::Inspected(wrong));
    assert_eq!(s.local_cli.stage, LocalCliStage::Inspecting);
    answer(&mut s, &inspect, LocalCliEvent::Inspected(v.clone()));
    let checked = check(&mut s);
    let mut verified = v.clone();
    verified.stage = LocalCliMetadataStage::MetadataVerified;
    let mut wrong = verified.clone();
    wrong.mode = CliMode::ChatOnly;
    answer(&mut s, &checked, LocalCliEvent::Checked(wrong));
    assert_eq!(s.local_cli.stage, LocalCliStage::Checking);
    answer(&mut s, &checked, LocalCliEvent::Checked(verified));
    assert_eq!(s.local_cli.stage, LocalCliStage::Verified);
    assert!(!s.availability().submit);
    cmd(&mut s, LocalCliCommand::Edit(draft()));
    assert!(s.local_cli.ticket.is_none());
    assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
    answer(&mut s, &inspect, LocalCliEvent::Inspected(v));
    assert!(s.local_cli.ticket.is_none());
    let mut d = draft();
    d.mode = CliMode::Unsupported;
    cmd(&mut s, LocalCliCommand::Edit(d));
    assert!(!s.local_cli_availability().inspect);
    no_effect(cmd(&mut s, LocalCliCommand::Inspect));
}

#[test]
fn global_tuple_and_phase_checked_before_conversation_and_prompt_run() {
    let mut s = state();
    begin(&mut s);
    let c = call(cmd(&mut s, LocalCliCommand::Inspect));
    assert_eq!(c.conversation, ConversationId(0));
    assert!(s.conversation(c.conversation).is_none());
    let good = event(&c, LocalCliEvent::Inspected(view(&c)));
    let before = s.local_cli.clone();
    for bad in [
        BackendEvent {
            conversation: ConversationId(1),
            ..good.clone()
        },
        BackendEvent {
            generation: c.generation + 1,
            ..good.clone()
        },
        BackendEvent {
            op: Some(OperationId(c.op.0 + 1)),
            ..good.clone()
        },
        BackendEvent {
            op: None,
            ..good.clone()
        },
        event(&c, LocalCliEvent::Checked(view(&c))),
        event(
            &c,
            LocalCliEvent::Failed {
                phase: LocalCliPhase::Create,
                failure: LocalCliFailure::Conflict,
            },
        ),
    ] {
        no_effect(s.apply_event(bad));
        assert_eq!(s.local_cli, before);
    }
    s.selected = Some(ConversationId(2));
    s.apply_event(good.clone());
    assert_eq!(s.local_cli.stage, LocalCliStage::AwaitingTrust);
    let settled = s.local_cli.clone();
    s.apply_event(good);
    assert_eq!(s.local_cli, settled);
    assert_eq!(s.selected, Some(ConversationId(2)));
}

#[test]
fn separate_trust_double_clicks_edit_and_expiry() {
    let mut s = state();
    begin(&mut s);
    let c = call(cmd(&mut s, LocalCliCommand::Inspect));
    no_effect(cmd(&mut s, LocalCliCommand::Inspect));
    let mut v = view(&c);
    answer(&mut s, &c, LocalCliEvent::Inspected(v.clone()));
    assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
    no_effect(cmd(&mut s, LocalCliCommand::Check));
    cmd(&mut s, LocalCliCommand::TrustExecutable(true));
    no_effect(cmd(&mut s, LocalCliCommand::Check));
    cmd(&mut s, LocalCliCommand::TrustProject(true));
    let check = call(cmd(&mut s, LocalCliCommand::Check));
    no_effect(cmd(&mut s, LocalCliCommand::Check));
    cmd(&mut s, LocalCliCommand::Edit(draft()));
    assert!(s.local_cli.ticket.is_none());
    assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
    v.stage = LocalCliMetadataStage::MetadataVerified;
    answer(&mut s, &check, LocalCliEvent::Checked(v));
    assert_eq!(s.local_cli.stage, LocalCliStage::Editing);
    let next = call(cmd(&mut s, LocalCliCommand::Inspect));
    assert!(next.generation > c.generation && next.op.0 > check.op.0);
    answer(&mut s, &next, LocalCliEvent::Inspected(view(&next)));
    s.set_now(100);
    assert!(s.local_cli.ticket.is_none());
    assert_eq!(s.local_cli.error, Some(LocalCliFailure::Expired));
    no_effect(cmd(&mut s, LocalCliCommand::Check));
}

#[test]
fn lost_check_requires_explicit_status_never_rechecks() {
    let mut s = state();
    begin(&mut s);
    let mut v = inspect(&mut s);
    let c = check(&mut s);
    no_effect(answer(
        &mut s,
        &c,
        LocalCliEvent::Failed {
            phase: LocalCliPhase::Check,
            failure: LocalCliFailure::Unknown("timeout".into()),
        },
    ));
    assert_eq!(s.local_cli.stage, LocalCliStage::CheckUnknown);
    no_effect(cmd(&mut s, LocalCliCommand::Check));
    no_effect(cmd(&mut s, LocalCliCommand::Create));
    let status = call(cmd(&mut s, LocalCliCommand::Status));
    no_effect(cmd(&mut s, LocalCliCommand::Status));
    assert_ne!(status.op, c.op);
    v.stage = LocalCliMetadataStage::MetadataVerified;
    answer(&mut s, &status, LocalCliEvent::Status(v));
    assert_eq!(s.local_cli.stage, LocalCliStage::Verified);
}

#[test]
fn lost_create_lookup_none_requires_explicit_repeat_of_identical_pair() {
    let mut s = state();
    let identity = unknown(&mut s);
    assert_eq!(s.local_cli.stage, LocalCliStage::CreateUnknown);
    no_effect(cmd(&mut s, LocalCliCommand::RepeatCreate));
    no_effect(cmd(&mut s, LocalCliCommand::Create));
    no_effect(cmd(&mut s, LocalCliCommand::BeginNew(draft())));
    no_effect(cmd(&mut s, LocalCliCommand::Edit(draft())));
    let lookup = call(cmd(&mut s, LocalCliCommand::LookupCreation));
    assert_eq!(
        lookup.request,
        LocalCliRequest::LookupCreation {
            identity: identity.clone()
        }
    );
    no_effect(cmd(&mut s, LocalCliCommand::LookupCreation));
    no_effect(answer(
        &mut s,
        &lookup,
        LocalCliEvent::CreationLookup {
            identity: identity.clone(),
            found: None,
        },
    ));
    assert!(s.local_cli.lookup_absent);
    let repeat = call(cmd(&mut s, LocalCliCommand::RepeatCreate));
    assert_eq!(repeat.request, LocalCliRequest::Create { identity });
    assert!(repeat.op.0 > lookup.op.0);
    no_effect(cmd(&mut s, LocalCliCommand::RepeatCreate));
    no_effect(cmd(&mut s, LocalCliCommand::Create));
}

#[test]
fn cancel_and_disconnect_preserve_unresolved_creation_but_invalidate_tickets() {
    for cancel in [false, true] {
        let mut s = state();
        verified(&mut s);
        let c = call(cmd(&mut s, LocalCliCommand::Create));
        let identity = s.local_cli.creation.clone().unwrap();
        if cancel {
            let revoke = call(cmd(&mut s, LocalCliCommand::Cancel));
            assert!(matches!(revoke.request, LocalCliRequest::Revoke { .. }));
            answer(&mut s, &revoke, LocalCliEvent::Revoked);
        } else {
            s.set_connection(Connection::Reconnecting);
            no_effect(s.set_connection(Connection::Ready));
        }
        assert!(s.local_cli.ticket.is_none());
        assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
        assert_eq!(s.local_cli.creation.as_ref(), Some(&identity));
        let created = LocalCliCreated {
            identity: identity.clone(),
            conversation: ConversationId(3),
        };
        no_effect(answer(&mut s, &c, LocalCliEvent::Created(created.clone())));
        assert_eq!(s.local_cli.stage, LocalCliStage::CreateUnknown);
        no_effect(cmd(&mut s, LocalCliCommand::Edit(draft())));
        let lookup = call(cmd(&mut s, LocalCliCommand::LookupCreation));
        answer(
            &mut s,
            &lookup,
            LocalCliEvent::CreationLookup {
                identity: identity.clone(),
                found: Some(created),
            },
        );
        assert_eq!(s.local_cli.stage, LocalCliStage::Reconciling);
        s.apply_catalog(boot(&[1, 2, 3]));
        assert_eq!(s.selected, Some(ConversationId(3)));
        assert_eq!(s.local_cli.stage, LocalCliStage::Created);
    }
}

#[test]
fn exact_creation_and_catalog_can_arrive_in_either_order_unrelated_creates_ignored() {
    for catalog_first in [false, true] {
        let mut s = state();
        verified(&mut s);
        let c = call(cmd(&mut s, LocalCliCommand::Create));
        no_effect(cmd(&mut s, LocalCliCommand::Create));
        let identity = s.local_cli.creation.clone().unwrap();
        let mut bad = identity.clone();
        bad.session = EngineSessionId("unrelated".into());
        no_effect(answer(
            &mut s,
            &c,
            LocalCliEvent::Created(LocalCliCreated {
                identity: bad,
                conversation: ConversationId(99),
            }),
        ));
        let mut bad = identity.clone();
        bad.setup = SetupId("other-ticket".into());
        no_effect(answer(
            &mut s,
            &c,
            LocalCliEvent::Created(LocalCliCreated {
                identity: bad,
                conversation: ConversationId(3),
            }),
        ));
        assert_eq!(s.local_cli.stage, LocalCliStage::Creating);
        s.apply_catalog(boot(&[1, 2, 99]));
        assert_eq!(s.selected, Some(ConversationId(1)));
        if catalog_first {
            s.apply_catalog(boot(&[1, 2, 3, 99]));
        }
        answer(
            &mut s,
            &c,
            LocalCliEvent::Created(LocalCliCreated {
                identity,
                conversation: ConversationId(3),
            }),
        );
        if !catalog_first {
            assert_eq!(s.local_cli.stage, LocalCliStage::Reconciling);
            s.apply_catalog(boot(&[1, 2, 99]));
            assert_eq!(s.selected, Some(ConversationId(1)));
            s.apply_catalog(boot(&[1, 2, 3, 99]));
        }
        assert_eq!(s.selected, Some(ConversationId(3)));
        assert_eq!(s.local_cli.stage, LocalCliStage::Created);
        s.selected = Some(ConversationId(2));
        s.apply_catalog(boot(&[1, 2, 3, 99]));
        assert_eq!(
            s.selected,
            Some(ConversationId(2)),
            "completion selects only once"
        );
    }
}

fn existing(s: &mut AppState) {
    s.conversations[0].generation = 12;
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: 12,
        op: None,
        kind: EventKind::ExecutionContext {
            profile: ExecutionProfile::LocalCliChatOnly,
            execution_blocked: Some("no grant".into()),
        },
    });
    cmd(
        s,
        LocalCliCommand::BeginExisting {
            conversation: ConversationId(1),
            session: EngineSessionId("saved-engine-session".into()),
        },
    );
}

#[test]
fn existing_status_is_only_hints_and_verified_still_blocked_return_uses_captured_target() {
    let mut s = state();
    existing(&mut s);
    let status = call(cmd(&mut s, LocalCliCommand::SessionStatus));
    assert_eq!(
        (status.conversation, status.generation),
        (ConversationId(1), 12)
    );
    answer(
        &mut s,
        &status,
        LocalCliEvent::SessionStatus(LocalCliSessionView {
            mode: CliMode::ChatOnly,
            session: EngineSessionId("saved-engine-session".into()),
            executable: "/saved/claude".into(),
            project: "/project".into(),
            model: Some(CliModel::Opus),
            context_valid: true,
        }),
    );
    assert_eq!(s.local_cli.stage, LocalCliStage::Editing);
    assert!(s.local_cli.ticket.is_none());
    assert!(!s.local_cli.trust_project && !s.local_cli.trust_executable);
    cmd(&mut s, LocalCliCommand::Edit(draft()));
    let mut v = inspect(&mut s);
    let c = check(&mut s);
    s.selected = Some(ConversationId(2));
    v.stage = LocalCliMetadataStage::MetadataVerified;
    no_effect(answer(&mut s, &c, LocalCliEvent::Checked(v)));
    assert_eq!(s.local_cli.stage, LocalCliStage::Verified);
    no_effect(cmd(&mut s, LocalCliCommand::Create));
    let out = cmd(&mut s, LocalCliCommand::ReturnToConversation);
    assert_eq!(s.selected, Some(ConversationId(1)));
    assert!(out.effects.iter().any(|e| matches!(
        e,
        Effect::Backend(BackendRequest::Open {
            conversation: ConversationId(1),
            generation: 13
        })
    )));
    assert_eq!(
        s.conversation(ConversationId(1))
            .unwrap()
            .execution_blocked
            .as_deref(),
        Some("no grant")
    );
    assert!(!s.availability().submit);
    no_effect(s.dispatch(Command::SetModel("native".into())));
}

#[test]
fn existing_return_keeps_exact_ticket_revocable_with_stale_attachment() {
    for failed in [false, true] {
        let mut s = state();
        existing(&mut s);
        cmd(&mut s, LocalCliCommand::Edit(draft()));
        let mut v = inspect(&mut s);
        let checked = check(&mut s);
        v.stage = LocalCliMetadataStage::MetadataVerified;
        answer(&mut s, &checked, LocalCliEvent::Checked(v.clone()));
        cmd(&mut s, LocalCliCommand::ReturnToConversation);
        assert_eq!(s.conversations[0].generation, 13);
        let a = s.local_cli_availability();
        assert!(a.revoke);
        assert!(!a.check && !a.create && !a.inspect);
        no_effect(cmd(&mut s, LocalCliCommand::Check));
        no_effect(cmd(&mut s, LocalCliCommand::Create));
        let revoke = call(cmd(&mut s, LocalCliCommand::Revoke));
        assert_eq!(
            (revoke.conversation, revoke.generation),
            (ConversationId(1), 12)
        );
        assert_eq!(revoke.request, LocalCliRequest::Revoke { setup: v.setup });
        let reply = if failed {
            LocalCliEvent::Failed {
                phase: LocalCliPhase::Revoke,
                failure: LocalCliFailure::Unknown("lost ack".into()),
            }
        } else {
            LocalCliEvent::Revoked
        };
        let before = s.local_cli.clone();
        let mut wrong = event(&revoke, reply.clone());
        wrong.generation = 13;
        no_effect(s.apply_event(wrong));
        assert_eq!(s.local_cli, before);
        let mut wrong = event(&revoke, reply.clone());
        wrong.op = Some(OperationId(revoke.op.0 + 1));
        no_effect(s.apply_event(wrong));
        assert_eq!(s.local_cli, before);
        assert!(!answer(&mut s, &revoke, reply).notes.is_empty());
        assert_eq!(s.local_cli.error.is_some(), failed);
        assert!(s.local_cli.ticket.is_none());
    }
}

#[test]
fn stale_attachment_cleanup_still_requires_current_owner() {
    let mut s = state();
    existing(&mut s);
    cmd(&mut s, LocalCliCommand::Edit(draft()));
    inspect(&mut s);
    s.conversations[0].generation += 1;
    s.restore_owner(
        ConversationId(1),
        ConversationOwner::NativeEngine { imported_at: 1 },
    );
    assert!(!s.local_cli_availability().revoke);
    no_effect(cmd(&mut s, LocalCliCommand::Revoke));
    let mut s = state();
    existing(&mut s);
    cmd(&mut s, LocalCliCommand::Edit(draft()));
    inspect(&mut s);
    s.conversations[0].generation += 1;
    let revoke = call(cmd(&mut s, LocalCliCommand::Revoke));
    s.restore_owner(
        ConversationId(1),
        ConversationOwner::NativeEngine { imported_at: 1 },
    );
    let before = s.local_cli.clone();
    no_effect(answer(&mut s, &revoke, LocalCliEvent::Revoked));
    assert_eq!(s.local_cli, before);
}

#[test]
fn existing_attachment_change_and_disconnect_reject_late_results() {
    for disconnect in [true, false] {
        let mut s = state();
        existing(&mut s);
        cmd(&mut s, LocalCliCommand::Edit(draft()));
        let v = inspect(&mut s);
        let c = check(&mut s);
        if disconnect {
            s.set_connection(Connection::Reconnecting);
            no_effect(s.set_connection(Connection::Ready));
        } else {
            s.conversations[0].generation += 1;
        }
        let before = s.local_cli.clone();
        let v = LocalCliSetupView {
            stage: LocalCliMetadataStage::MetadataVerified,
            ..v
        };
        no_effect(answer(&mut s, &c, LocalCliEvent::Checked(v)));
        assert_eq!(s.local_cli, before);
        assert!(!s.local_cli_availability().create);
    }
}

#[test]
fn default_capability_read_only_and_profile_fail_closed() {
    let mut s = state();
    s.local_cli.available = false;
    no_effect(cmd(&mut s, LocalCliCommand::BeginNew(draft())));
    assert!(s.local_cli.target.is_none());
    s.local_cli.available = true;
    no_effect(cmd(
        &mut s,
        LocalCliCommand::BeginExisting {
            conversation: ConversationId(1),
            session: EngineSessionId("native".into()),
        },
    ));
    assert!(s.local_cli.target.is_none());
    s.read_only = Some("read only".into());
    no_effect(cmd(&mut s, LocalCliCommand::BeginNew(draft())));
    assert!(s.local_cli.target.is_none());
    s.read_only = None;
    s.conversations[0].profile = ExecutionProfile::Unsupported;
    begin(&mut s); // Global setup does not inherit selected Unsupported profile.
    assert_eq!(s.local_cli.target, Some(LocalCliTarget::New));
}

#[test]
fn reservations_and_metadata_identity_are_validated_before_consuming_operation() {
    let mut s = state();
    begin(&mut s);
    let c = call(cmd(&mut s, LocalCliCommand::Inspect));
    let v = view(&c);
    let before = s.local_cli.clone();
    for invalid in [
        LocalCliSetupView {
            root_candidate: Some(EngineSessionId("different-root".into())),
            ..v.clone()
        },
        LocalCliSetupView {
            setup: SetupId("x".repeat(125)),
            ..v.clone()
        },
        LocalCliSetupView {
            stage: LocalCliMetadataStage::MetadataVerified,
            ..v.clone()
        },
    ] {
        answer(&mut s, &c, LocalCliEvent::Inspected(invalid));
        assert_eq!(s.local_cli, before);
    }
    answer(&mut s, &c, LocalCliEvent::Inspected(v.clone()));
    let c = check(&mut s);
    let before = s.local_cli.clone();
    answer(
        &mut s,
        &c,
        LocalCliEvent::Checked(LocalCliSetupView {
            executable: "/swapped/executable".into(),
            stage: LocalCliMetadataStage::MetadataVerified,
            ..v.clone()
        }),
    );
    assert_eq!(s.local_cli, before);
    answer(
        &mut s,
        &c,
        LocalCliEvent::Checked(LocalCliSetupView {
            stage: LocalCliMetadataStage::MetadataVerified,
            ..v
        }),
    );
    assert_eq!(s.local_cli.stage, LocalCliStage::Verified);
}

#[test]
fn expiry_during_create_preserves_identity_and_absent_lookup_does_not_mint_new_ticket() {
    let mut s = state();
    verified(&mut s);
    let c = call(cmd(&mut s, LocalCliCommand::Create));
    let identity = s.local_cli.creation.clone().unwrap();
    s.set_now(100);
    assert_eq!(s.local_cli.creation.as_ref(), Some(&identity));
    assert_eq!(s.local_cli.stage, LocalCliStage::CreateUnknown);
    no_effect(answer(
        &mut s,
        &c,
        LocalCliEvent::Created(LocalCliCreated {
            identity: identity.clone(),
            conversation: ConversationId(3),
        }),
    ));
    let lookup = call(cmd(&mut s, LocalCliCommand::LookupCreation));
    let before = s.local_cli.clone();
    let mut wrong = identity.clone();
    wrong.setup = SetupId("wrong-ticket".into());
    no_effect(answer(
        &mut s,
        &lookup,
        LocalCliEvent::CreationLookup {
            identity: wrong,
            found: None,
        },
    ));
    assert_eq!(s.local_cli, before);
    no_effect(answer(
        &mut s,
        &lookup,
        LocalCliEvent::CreationLookup {
            identity: identity.clone(),
            found: None,
        },
    ));
    no_effect(cmd(&mut s, LocalCliCommand::RepeatCreate));
    no_effect(cmd(&mut s, LocalCliCommand::Inspect));
    no_effect(cmd(&mut s, LocalCliCommand::BeginNew(draft())));
    assert_eq!(s.local_cli.creation, Some(identity));
}

fn absent(s: &mut AppState, identity: &LocalCliCreationId) -> Call {
    let lookup = call(cmd(s, LocalCliCommand::LookupCreation));
    no_effect(answer(
        s,
        &lookup,
        LocalCliEvent::CreationLookup {
            identity: identity.clone(),
            found: None,
        },
    ));
    lookup
}

fn unknown_named(s: &mut AppState, number: usize) -> LocalCliCreationId {
    begin(s);
    let inspect = call(cmd(s, LocalCliCommand::Inspect));
    let mut v = view(&inspect);
    v.setup = SetupId(format!("ticket-{number}"));
    v.root_candidate = Some(EngineSessionId(format!("cli-ticket-{number}")));
    answer(s, &inspect, LocalCliEvent::Inspected(v.clone()));
    let check = check(s);
    v.stage = LocalCliMetadataStage::MetadataVerified;
    answer(s, &check, LocalCliEvent::Checked(v));
    let create = call(cmd(s, LocalCliCommand::Create));
    answer(
        s,
        &create,
        LocalCliEvent::Failed {
            phase: LocalCliPhase::Create,
            failure: LocalCliFailure::Unknown("lost".into()),
        },
    );
    s.set_connection(Connection::Reconnecting);
    s.set_connection(Connection::Ready);
    s.local_cli.creation.clone().unwrap()
}

#[test]
fn absent_invalid_ticket_can_be_explicitly_parked_then_recovered_without_approval() {
    for invalidation in 0..3 {
        let mut s = state();
        let identity = unknown(&mut s);
        match invalidation {
            0 => {
                s.set_now(100);
            }
            1 => {
                let revoke = call(cmd(&mut s, LocalCliCommand::Revoke));
                answer(&mut s, &revoke, LocalCliEvent::Revoked);
            }
            _ => {
                s.set_connection(Connection::Reconnecting);
                s.set_connection(Connection::Ready);
            }
        }
        assert!(!s.local_cli_availability().park_creation);
        let before = s.local_cli.clone();
        no_effect(cmd(
            &mut s,
            LocalCliCommand::ParkCreationAndBeginNew(draft()),
        ));
        assert_eq!(s.local_cli, before);
        let old_lookup = absent(&mut s, &identity);
        assert!(s.local_cli_availability().park_creation);
        assert!(!s.local_cli_availability().begin);
        no_effect(cmd(
            &mut s,
            LocalCliCommand::ParkCreationAndBeginNew(draft()),
        ));
        assert_eq!(
            s.local_cli.parked_creations(),
            std::slice::from_ref(&identity)
        );
        assert!(s.local_cli.creation.is_none());
        assert_eq!(s.local_cli.stage, LocalCliStage::Editing);
        assert!(s.local_cli_availability().inspect);
        assert!(s.local_cli_availability().begin);
        // A catalog is not a creation receipt, even for a retained pair.
        s.apply_catalog(boot(&[1, 2, 3]));
        assert_eq!(s.selected, Some(ConversationId(1)));
        existing(&mut s);
        s.read_only = Some("read only recovery allowed".into());
        assert!(s.local_cli_availability().recover_creation);
        no_effect(cmd(
            &mut s,
            LocalCliCommand::RecoverCreation(identity.clone()),
        ));
        assert_eq!(s.local_cli.target, Some(LocalCliTarget::New));
        assert_eq!(s.local_cli.creation.as_ref(), Some(&identity));
        assert!(s.local_cli.ticket.is_none());
        assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
        assert!(!s.local_cli.lookup_absent);
        assert!(s.local_cli_availability().lookup_creation);
        for command in [
            LocalCliCommand::Create,
            LocalCliCommand::RepeatCreate,
            LocalCliCommand::Inspect,
            LocalCliCommand::Check,
        ] {
            no_effect(cmd(&mut s, command));
        }
        let lookup = call(cmd(&mut s, LocalCliCommand::LookupCreation));
        assert_eq!(lookup.conversation, LOCAL_CLI_GLOBAL);
        assert!(lookup.generation > old_lookup.generation);
        assert_eq!(
            lookup.request,
            LocalCliRequest::LookupCreation {
                identity: identity.clone()
            }
        );
        let before = s.local_cli.clone();
        let receipt = LocalCliCreated {
            identity: identity.clone(),
            conversation: ConversationId(3),
        };
        no_effect(answer(
            &mut s,
            &old_lookup,
            LocalCliEvent::CreationLookup {
                identity: identity.clone(),
                found: Some(receipt.clone()),
            },
        ));
        assert_eq!(s.local_cli, before);
        s.apply_catalog(boot(&[1, 2, 3]));
        assert_eq!(s.selected, Some(ConversationId(1)));
        assert_eq!(
            s.local_cli.parked_creations(),
            std::slice::from_ref(&identity)
        );
        answer(
            &mut s,
            &lookup,
            LocalCliEvent::CreationLookup {
                identity,
                found: Some(receipt),
            },
        );
        assert_eq!(s.selected, Some(ConversationId(3)));
        assert_eq!(s.local_cli.stage, LocalCliStage::Created);
        assert!(s.local_cli.parked_creations().is_empty());
    }
}

#[test]
fn recovery_capacity_fails_closed_and_reparking_does_not_evict_or_duplicate() {
    let mut s = state();
    let mut retained = Vec::new();
    for n in 0..LOCAL_CLI_RECOVERY_CAPACITY {
        let identity = unknown_named(&mut s, n);
        absent(&mut s, &identity);
        no_effect(cmd(
            &mut s,
            LocalCliCommand::ParkCreationAndBeginNew(draft()),
        ));
        retained.push(identity);
        assert_eq!(s.local_cli.parked_creations(), retained);
    }
    // An arbitrary pair is not recoverable, and does not alter state.
    let before = s.local_cli.clone();
    let bogus = LocalCliCreationId {
        session: EngineSessionId("bogus".into()),
        setup: SetupId("bogus".into()),
    };
    no_effect(cmd(&mut s, LocalCliCommand::RecoverCreation(bogus)));
    assert_eq!(s.local_cli, before);
    let identity = retained[0].clone();
    no_effect(cmd(
        &mut s,
        LocalCliCommand::RecoverCreation(identity.clone()),
    ));
    let lookup = call(cmd(&mut s, LocalCliCommand::LookupCreation));
    assert!(!s.local_cli_availability().park_creation);
    answer(
        &mut s,
        &lookup,
        LocalCliEvent::Failed {
            phase: LocalCliPhase::LookupCreation,
            failure: LocalCliFailure::Unknown("lost lookup".into()),
        },
    );
    assert!(!s.local_cli_availability().park_creation);
    absent(&mut s, &identity);
    assert!(!s.local_cli_availability().repeat_create);
    assert!(s.local_cli_availability().park_creation);
    no_effect(cmd(
        &mut s,
        LocalCliCommand::ParkCreationAndBeginNew(draft()),
    ));
    assert_eq!(s.local_cli.parked_creations(), retained);
    let ninth = unknown_named(&mut s, LOCAL_CLI_RECOVERY_CAPACITY);
    absent(&mut s, &ninth);
    assert!(!s.local_cli_availability().park_creation);
    let before = s.local_cli.clone();
    no_effect(cmd(
        &mut s,
        LocalCliCommand::ParkCreationAndBeginNew(draft()),
    ));
    no_effect(cmd(&mut s, LocalCliCommand::BeginNew(draft())));
    assert_eq!(s.local_cli, before);
    let recovery = cmd(&mut s, LocalCliCommand::RecoverCreation(identity.clone()));
    assert!(recovery.effects.iter().all(|e| matches!(
        e,
        Effect::Backend(BackendRequest::LocalCli {
            request: LocalCliRequest::Revoke { .. },
            ..
        })
    )));
    assert_eq!(s.local_cli.creation, Some(identity));
    assert_eq!(
        s.local_cli.parked_creations().len(),
        LOCAL_CLI_RECOVERY_CAPACITY
    );
    for saved in retained.iter().chain(std::iter::once(&ninth)) {
        assert!(
            s.local_cli.creation.as_ref() == Some(saved)
                || s.local_cli.parked_creations().contains(saved)
        );
    }
}

#[test]
fn recovered_record_survives_none_cancel_disconnect_and_waiting_for_exact_catalog() {
    let mut s = state();
    let identity = unknown_named(&mut s, 1);
    absent(&mut s, &identity);
    cmd(&mut s, LocalCliCommand::ParkCreationAndBeginNew(draft()));
    cmd(&mut s, LocalCliCommand::RecoverCreation(identity.clone()));
    absent(&mut s, &identity);
    no_effect(cmd(&mut s, LocalCliCommand::Cancel));
    s.set_connection(Connection::Reconnecting);
    s.set_connection(Connection::Ready);
    assert_eq!(
        s.local_cli.parked_creations(),
        std::slice::from_ref(&identity)
    );
    let lookup = call(cmd(&mut s, LocalCliCommand::LookupCreation));
    let mut wrong = identity.clone();
    wrong.setup = SetupId("other".into());
    let before = s.local_cli.clone();
    no_effect(answer(
        &mut s,
        &lookup,
        LocalCliEvent::CreationLookup {
            identity: identity.clone(),
            found: Some(LocalCliCreated {
                identity: wrong,
                conversation: ConversationId(3),
            }),
        },
    ));
    assert_eq!(s.local_cli, before);
    answer(
        &mut s,
        &lookup,
        LocalCliEvent::CreationLookup {
            identity: identity.clone(),
            found: Some(LocalCliCreated {
                identity: identity.clone(),
                conversation: ConversationId(3),
            }),
        },
    );
    assert_eq!(s.local_cli.stage, LocalCliStage::Reconciling);
    assert_eq!(
        s.local_cli.parked_creations(),
        std::slice::from_ref(&identity)
    );
    no_effect(cmd(
        &mut s,
        LocalCliCommand::ParkCreationAndBeginNew(draft()),
    ));
    s.apply_catalog(boot(&[1, 2, 99]));
    assert_eq!(s.selected, Some(ConversationId(1)));
    assert_eq!(s.local_cli.parked_creations(), &[identity]);
    s.apply_catalog(boot(&[1, 2, 3, 99]));
    assert!(s.local_cli.parked_creations().is_empty());
    assert_eq!(s.selected, Some(ConversationId(3)));
}

#[test]
fn failed_revoke_never_downgrades_a_confirmed_creation_waiting_for_catalog() {
    for lookup_receipt in [false, true] {
        for failure in [
            LocalCliFailure::Unknown("lost revoke".into()),
            LocalCliFailure::Rejected("refused".into()),
            LocalCliFailure::Conflict,
            LocalCliFailure::Expired,
        ] {
            let mut s = state();
            verified(&mut s);
            let create = call(cmd(&mut s, LocalCliCommand::Create));
            let identity = s.local_cli.creation.clone().unwrap();
            let receipt = LocalCliCreated {
                identity: identity.clone(),
                conversation: ConversationId(3),
            };
            if lookup_receipt {
                answer(
                    &mut s,
                    &create,
                    LocalCliEvent::Failed {
                        phase: LocalCliPhase::Create,
                        failure: LocalCliFailure::Unknown("lost".into()),
                    },
                );
                let lookup = call(cmd(&mut s, LocalCliCommand::LookupCreation));
                answer(
                    &mut s,
                    &lookup,
                    LocalCliEvent::CreationLookup {
                        identity,
                        found: Some(receipt.clone()),
                    },
                );
            } else {
                answer(&mut s, &create, LocalCliEvent::Created(receipt.clone()));
            }
            let revoke = call(cmd(&mut s, LocalCliCommand::Revoke));
            no_effect(answer(
                &mut s,
                &revoke,
                LocalCliEvent::Failed {
                    phase: LocalCliPhase::Revoke,
                    failure,
                },
            ));
            assert_eq!(s.local_cli.created, Some(receipt));
            assert_eq!(s.local_cli.stage, LocalCliStage::Reconciling);
            assert!(!s.local_cli_availability().lookup_creation);
            assert!(!s.local_cli_availability().repeat_create);
            s.apply_catalog(boot(&[1, 2, 99]));
            assert_eq!(s.local_cli.stage, LocalCliStage::Reconciling);
            s.apply_catalog(boot(&[1, 2, 3, 99]));
            assert_eq!(s.selected, Some(ConversationId(3)));
            assert_eq!(s.local_cli.stage, LocalCliStage::Created);
        }
    }
}

#[test]
fn cancel_before_inspect_reply_emits_exact_retirement_and_rejects_late_ticket() {
    let mut s = state();
    begin(&mut s);
    let inspect = call(cmd(&mut s, LocalCliCommand::Inspect));
    let out = cmd(&mut s, LocalCliCommand::Cancel);
    assert_eq!(
        out.effects,
        vec![Effect::Backend(BackendRequest::RetireLocalCliInspect {
            conversation: inspect.conversation,
            generation: inspect.generation,
            op: inspect.op,
        })]
    );
    let before = s.local_cli.clone();
    no_effect(answer(
        &mut s,
        &inspect,
        LocalCliEvent::Inspected(view(&inspect)),
    ));
    assert_eq!(s.local_cli, before);
    no_effect(cmd(&mut s, LocalCliCommand::Cancel));
    assert!(s.local_cli.ticket.is_none());
}

#[test]
fn lifecycle_capability_setter_preserves_recovery_and_invalidates_approval() {
    let mut s = state();
    let identity = unknown(&mut s);
    let LifecycleEvent::LocalCliSetupAvailable(available) =
        LifecycleEvent::LocalCliSetupAvailable(false)
    else {
        unreachable!()
    };
    no_effect(s.set_local_cli_available(available));
    assert!(!s.local_cli_availability().lookup_creation);
    assert!(s.local_cli.ticket.is_none());
    assert_eq!(s.local_cli.creation, Some(identity));
    no_effect(s.set_local_cli_available(true));
    assert!(s.local_cli_availability().lookup_creation);
    assert!(!s.local_cli_availability().repeat_create);
}

#[test]
fn conflict_is_not_success_or_permission_to_retry() {
    let mut s = state();
    verified(&mut s);
    let c = call(cmd(&mut s, LocalCliCommand::Create));
    let identity = s.local_cli.creation.clone();
    no_effect(answer(
        &mut s,
        &c,
        LocalCliEvent::Failed {
            phase: LocalCliPhase::Create,
            failure: LocalCliFailure::Conflict,
        },
    ));
    assert_eq!(s.local_cli.error, Some(LocalCliFailure::Conflict));
    assert_eq!(s.local_cli.creation, identity);
    no_effect(cmd(&mut s, LocalCliCommand::RepeatCreate));
    assert!(s.local_cli_availability().lookup_creation);
}

#[test]
fn desktop_controls_effort_change_retires_old_confirmation_and_freezes_new_intent() {
    let mut s = state();
    let mut d = draft();
    d.mode = CliMode::Orchestration;
    d.thinking = Some("high".into());
    cmd(&mut s, LocalCliCommand::BeginNew(d.clone()));
    let first = call(cmd(&mut s, LocalCliCommand::Inspect));
    let ticket = view(&first);
    assert!(matches!(&ticket.target,LocalCliInspectTarget::New{thinking:Some(v),..} if v=="high"));
    answer(&mut s, &first, LocalCliEvent::Inspected(ticket.clone()));
    d.thinking = Some("max".into());
    cmd(&mut s, LocalCliCommand::Edit(d));
    assert!(!s.local_cli.trust_project);
    assert!(!s.local_cli.trust_executable);
    assert!(s.local_cli.ticket.is_none());
    assert!(
        cmd(
            &mut s,
            LocalCliCommand::ConfirmAndConnect {
                setup: ticket.setup
            }
        )
        .effects
        .is_empty()
    );
    let second = call(cmd(&mut s, LocalCliCommand::Inspect));
    let ticket = view(&second);
    assert!(matches!(&ticket.target,LocalCliInspectTarget::New{thinking:Some(v),..} if v=="max"));
    let mut stale = ticket.clone();
    if let LocalCliInspectTarget::New { thinking, .. } = &mut stale.target {
        *thinking = Some("high".into());
    }
    answer(&mut s, &second, LocalCliEvent::Inspected(stale));
    assert!(s.local_cli.ticket.is_none());
    answer(&mut s, &second, LocalCliEvent::Inspected(ticket));
    assert!(s.local_cli.ticket.is_some());
}
