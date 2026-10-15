use super::*;

type Journal = (ConversationId, u64, OperationId, LocalCliCreationId);
fn pending(s: &mut AppState) -> Journal {
    verified(s);
    let out = s.dispatch(Command::LocalCli(LocalCliCommand::Create));
    assert_eq!(out.effects.len(), 1);
    let Effect::JournalLocalCliCreation {
        conversation,
        generation,
        op,
        identity,
    } = &out.effects[0]
    else {
        panic!("journal before backend")
    };
    (*conversation, *generation, *op, identity.clone())
}
fn ack(s: &mut AppState, j: &Journal, result: Result<(), String>) -> Outcome {
    s.local_cli_creation_persisted(j.0, j.1, j.2, &j.3, result)
}
fn identity(n: usize) -> LocalCliCreationId {
    LocalCliCreationId {
        session: EngineSessionId(format!("cli-{n}")),
        setup: SetupId(format!("ticket-{n}")),
    }
}

#[test]
fn creation_waits_for_exact_durable_ack_and_releases_only_once() {
    let mut s = state();
    let j = pending(&mut s);
    let fake = Call {
        conversation: j.0,
        generation: j.1,
        op: j.2,
        request: LocalCliRequest::Create {
            identity: j.3.clone(),
        },
    };
    no_effect(answer(
        &mut s,
        &fake,
        LocalCliEvent::Created(LocalCliCreated {
            identity: j.3.clone(),
            conversation: ConversationId(1),
        }),
    ));
    assert_eq!(s.local_cli.stage, LocalCliStage::Creating);
    let mut wrong = j.clone();
    wrong.1 += 1;
    no_effect(ack(&mut s, &wrong, Ok(())));
    wrong = j.clone();
    wrong.3 = identity(99);
    no_effect(ack(&mut s, &wrong, Ok(())));
    let request = call(ack(&mut s, &j, Ok(())));
    assert_eq!(request.request, fake.request);
    no_effect(ack(&mut s, &j, Ok(())));
    let out = answer(
        &mut s,
        &request,
        LocalCliEvent::Created(LocalCliCreated {
            identity: j.3.clone(),
            conversation: ConversationId(1),
        }),
    );
    assert!(
        out.effects
            .iter()
            .any(|e| matches!(e,Effect::RetireLocalCliCreation { identity } if identity==&j.3))
    );
}

#[test]
fn failed_or_retired_ack_cannot_send_create_or_drop_uncertainty() {
    for case in 0..9 {
        let mut s = state();
        let j = pending(&mut s);
        let result = match case {
            0 => Err("disk failed".into()),
            1 => {
                s.dispatch(Command::LocalCli(LocalCliCommand::Cancel));
                Ok(())
            }
            2 => {
                s.set_connection(Connection::Offline("gone".into()));
                Ok(())
            }
            3 => {
                s.set_now(101);
                Ok(())
            }
            4 => {
                s.set_local_cli_available(false);
                Ok(())
            }
            5 => {
                s.read_only = Some("storage fault".into());
                Ok(())
            }
            6 => {
                s.local_cli.ticket.as_mut().unwrap().root_candidate =
                    Some(EngineSessionId("other-root".into()));
                Ok(())
            }
            7 => {
                s.local_cli.ticket.as_mut().unwrap().mode = CliMode::RestrictedWrite;
                Ok(())
            }
            _ => {
                s.local_cli.trust_project = false;
                Ok(())
            }
        };
        assert!(!ack(&mut s, &j, result).effects.iter().any(|e| matches!(
            e,
            Effect::Backend(BackendRequest::LocalCli {
                request: LocalCliRequest::Create { .. },
                ..
            })
        )));
        assert_eq!(s.local_cli.creation, Some(j.3));
        assert!(!s.local_cli_availability().create);
        assert!(!s.local_cli_availability().repeat_create);
    }
}

#[test]
fn restart_retains_nine_identities_and_swaps_read_only_recovery_without_eviction() {
    let ids: Vec<_> = (0..=LOCAL_CLI_RECOVERY_CAPACITY).map(identity).collect();
    let mut s = state();
    s.restore_local_cli_creations(Ok(ids.clone()));
    assert_eq!(s.local_cli.stage, LocalCliStage::CreateUnknown);
    assert!(s.local_cli.ticket.is_none());
    assert!(!s.local_cli.trust_executable && !s.local_cli.trust_project);
    assert!(s.local_cli_availability().lookup_creation);
    assert!(!s.local_cli_availability().repeat_create);
    assert_eq!(
        s.local_cli.parked_creations().len(),
        LOCAL_CLI_RECOVERY_CAPACITY
    );
    no_effect(cmd(
        &mut s,
        LocalCliCommand::RecoverCreation(ids[0].clone()),
    ));
    assert_eq!(s.local_cli.creation, Some(ids[0].clone()));
    for id in &ids {
        assert!(
            s.local_cli.creation.as_ref() == Some(id)
                || s.local_cli.parked_creations().contains(id)
        );
    }
    let lookup = call(cmd(&mut s, LocalCliCommand::LookupCreation));
    assert_eq!(
        lookup.request,
        LocalCliRequest::LookupCreation {
            identity: ids[0].clone()
        }
    );
    answer(
        &mut s,
        &lookup,
        LocalCliEvent::CreationLookup {
            identity: ids[0].clone(),
            found: None,
        },
    );
    assert!(
        !s.local_cli_availability().repeat_create,
        "absence must not restore a ticket"
    );
    no_effect(cmd(&mut s, LocalCliCommand::RepeatCreate));
}

#[test]
fn unreadable_malformed_duplicate_or_overflowed_journal_disables_new_creation() {
    for records in [
        Err("unreadable".into()),
        Ok(vec![identity(1), identity(1)]),
        Ok((0..=LOCAL_CLI_RECOVERY_CAPACITY + 1)
            .map(identity)
            .collect()),
        Ok(vec![LocalCliCreationId {
            session: EngineSessionId("../bad".into()),
            setup: SetupId("s".into()),
        }]),
    ] {
        let mut s = state();
        s.restore_local_cli_creations(records);
        assert!(s.local_cli.recovery_unavailable);
        verified(&mut s);
        assert!(!s.local_cli_availability().create);
        no_effect(s.dispatch(Command::LocalCli(LocalCliCommand::Create)));
    }
}
