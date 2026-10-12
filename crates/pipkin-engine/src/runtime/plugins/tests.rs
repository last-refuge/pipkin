use crate::extensions::profiles;
use crate::provider::scripted::Scripted;
use crate::runtime::tests::{Engine, Fixture};
use pipkin_engine_protocol::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn selected(
    e: &mut Engine,
    session: Option<&str>,
    specs: Option<Vec<ExtensionSpec>>,
) -> Vec<ExtensionSpec> {
    match e
        .call(Call::PrepareExtensions {
            session: session.map(str::to_owned),
            extensions: specs,
        })
        .unwrap()
    {
        Reply::ExtensionProfile { extensions, .. } => extensions,
        r => panic!("{r:?}"),
    }
}
fn questions() -> Vec<ExtensionSpec> {
    vec![ExtensionSpec::Builtin("questions".into())]
}
fn greeting() -> Vec<ExtensionSpec> {
    vec![ExtensionSpec::Builtin("greeting".into())]
}

#[test]
fn defaults_are_sticky_sessions_are_pinned_and_empty_overrides_survive_restart() {
    let fx = Fixture::new();
    let provider = Scripted::echo("answer");
    let mut config = fx.config();
    config.extensions = questions();
    let mut e = fx.start_config(&provider, config);
    let a = e.create(&fx.project, "a");
    let empty = e.create(&fx.project, "empty");
    assert_eq!(selected(&mut e, None, None), questions());
    assert_eq!(selected(&mut e, Some(&a), None), questions());
    assert!(selected(&mut e, Some(&empty), Some(vec![])).is_empty());
    // Preparing a profile must not start an extension or load a runtime session.
    assert!(e.handle.shared().sessions.lock().unwrap().live.is_empty());
    drop(e);
    let mut e = fx.start(&provider); // no launch list -> restore
    assert_eq!(selected(&mut e, None, None), questions());
    assert_eq!(selected(&mut e, None, Some(greeting())), greeting());
    let b = e.create(&fx.project, "b");
    assert_eq!(selected(&mut e, Some(&a), None), questions());
    assert_eq!(selected(&mut e, Some(&b), None), greeting());
    assert!(selected(&mut e, Some(&empty), None).is_empty());
    assert!(e.snapshot(&empty).ui.questions.is_empty());
    e.snapshot(&a);
    e.until("pinned questions activate", |events| events.iter().any(|event| matches!(event,
        Event::Session { session, change: SessionChange::UiChanged { ui }, .. } if session == &a && !ui.questions.is_empty())));
    assert!(selected(&mut e, None, Some(vec![])).is_empty());
    drop(e);
    let mut e = fx.start(&provider);
    assert!(selected(&mut e, None, None).is_empty());
    assert_eq!(selected(&mut e, Some(&a), None), questions());
    assert_eq!(selected(&mut e, Some(&b), None), greeting());
    assert!(selected(&mut e, Some(&empty), None).is_empty());
    let c = e.create(&fx.project, "c");
    assert!(selected(&mut e, Some(&c), None).is_empty());
}

#[test]
fn an_active_selection_conflicts_until_retirement_and_old_questions_cannot_answer_the_replacement()
{
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("answer"));
    let session = e.create(&fx.project, "a");
    selected(&mut e, Some(&session), Some(questions()));
    e.snapshot(&session);
    e.until("question", |events| {
        events.iter().any(|event| {
            matches!(event,
        Event::Session { change: SessionChange::UiChanged { ui }, .. } if !ui.questions.is_empty())
        })
    });
    let old = e.snapshot(&session).ui.questions[0].id.clone();
    assert_eq!(
        selected(&mut e, Some(&session), Some(questions())),
        questions()
    );
    let error = e
        .call(Call::PrepareExtensions {
            session: Some(session.clone()),
            extensions: Some(vec![]),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(
        error.message,
        format!("Session {session} is active with a different plugin selection")
    );
    assert_eq!(selected(&mut e, Some(&session), None), questions());
    e.call(Call::ReleaseSession {
        session: session.clone(),
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while e
        .handle
        .shared()
        .sessions
        .lock()
        .unwrap()
        .live
        .contains_key(&session)
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(selected(&mut e, Some(&session), Some(vec![])).is_empty());
    assert!(e.snapshot(&session).ui.questions.is_empty());
    assert!(
        e.call(Call::AnswerQuestion {
            session: session.clone(),
            question: old,
            answer: Answer::Choice {
                value: "vanilla".into()
            }
        })
        .is_err()
    );
    assert!(e.snapshot(&session).ui.questions.is_empty());
}

#[test]
fn failed_profile_writes_and_invalid_selections_preserve_previous_defaults_and_pins() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("answer"));
    selected(&mut e, None, Some(greeting()));
    let session = e.create(&fx.project, "a");
    selected(&mut e, Some(&session), Some(vec![]));
    for target in [None, Some(session.clone())] {
        e.handle.shared().store.fail_next_write();
        assert!(
            e.call(Call::PrepareExtensions {
                session: target.clone(),
                extensions: Some(questions())
            })
            .is_err()
        );
        let invalid = [
            ExtensionSpec::Builtin("questions".into()),
            ExtensionSpec::Builtin("questions".into()),
        ];
        assert_eq!(
            e.call(Call::PrepareExtensions {
                session: target.clone(),
                extensions: Some(invalid.into())
            })
            .unwrap_err()
            .code,
            ErrorCode::Invalid
        );
        assert_eq!(
            e.call(Call::PrepareExtensions {
                session: target,
                extensions: Some(vec![ExtensionSpec::Builtin("unknown".into())])
            })
            .unwrap_err()
            .code,
            ErrorCode::Invalid
        );
    }
    assert_eq!(selected(&mut e, None, None), greeting());
    assert!(selected(&mut e, Some(&session), None).is_empty());
    assert!(e.handle.shared().sessions.lock().unwrap().live.is_empty());
}

#[test]
fn simultaneous_first_acquisition_and_profile_replacement_never_disagree() {
    for _ in 0..12 {
        let fx = Fixture::new();
        let mut e = fx.start(&Scripted::echo("answer"));
        let session = e.create(&fx.project, "a");
        let shared = e.handle.shared().clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let s = shared.clone();
        let id = session.clone();
        let go = barrier.clone();
        let acquire = std::thread::spawn(move || {
            go.wait();
            s.session(&id).unwrap()
        });
        barrier.wait();
        let prepared = shared.prepare_extensions(Some(session.clone()), Some(questions()));
        let runtime = acquire.join().unwrap();
        let pinned = profiles::pin(&shared.store, &session).unwrap();
        assert_eq!(runtime.extension_profile, pinned);
        match prepared {
            Ok(_) => assert_eq!(pinned, questions()),
            Err(e) => {
                assert_eq!(e.code, ErrorCode::Conflict);
                assert!(pinned.is_empty());
            }
        }
    }
}

#[test]
fn malformed_and_future_profiles_fail_closed_without_starting_a_session() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("answer"));
    let session = e.create(&fx.project, "a");
    for raw in [
        "not json",
        r#"{"version":2,"extensions":[]}"#,
        r#"{"version":1,"extensions":[],"extra":true}"#,
    ] {
        e.handle
            .shared()
            .store
            .tx(|tx| crate::store::Store::kv_set(tx, &session, profiles::KEY, raw))
            .unwrap();
        assert!(
            e.call(Call::OpenSession {
                session: session.clone(),
                window: None
            })
            .is_err()
        );
        assert!(e.handle.shared().sessions.lock().unwrap().live.is_empty());
        assert_eq!(
            e.handle
                .shared()
                .store
                .kv_get(&session, profiles::KEY)
                .unwrap()
                .as_deref(),
            Some(raw)
        );
    }
    // Explicit replacement can repair an inactive profile; implicit open never does.
    selected(&mut e, Some(&session), Some(vec![]));
    assert!(e.snapshot(&session).ui.questions.is_empty());
}

#[cfg(unix)]
#[test]
fn process_profiles_preserve_argv_and_absolute_identity_and_reject_missing_executables() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new();
    let executable = fx.dir.path().join("extension with spaces");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let alias = fx.dir.path().join("alias");
    std::os::unix::fs::symlink(&executable, &alias).unwrap();
    let mut e = fx.start(&Scripted::echo("answer"));
    let spec = ExtensionSpec::Process {
        id: "custom".into(),
        argv: vec![
            alias.display().to_string(),
            "argument with spaces".into(),
            "$(literal)".into(),
        ],
    };
    let saved = selected(&mut e, None, Some(vec![spec]));
    let ExtensionSpec::Process { argv, .. } = &saved[0] else {
        panic!()
    };
    assert_eq!(
        argv,
        &[
            executable.display().to_string(),
            "argument with spaces".into(),
            "$(literal)".into()
        ]
    );
    drop(e);
    let mut e = fx.start(&Scripted::echo("answer"));
    assert_eq!(selected(&mut e, None, None), saved);
    let duplicate = ExtensionSpec::Process {
        id: crate::extensions::examples::QUESTIONS_EXAMPLE_ID.into(),
        argv: vec![executable.display().to_string()],
    };
    assert_eq!(
        e.call(Call::PrepareExtensions {
            session: None,
            extensions: Some([questions(), vec![duplicate]].concat())
        })
        .unwrap_err()
        .code,
        ErrorCode::Invalid
    );
    assert_eq!(selected(&mut e, None, None), saved);
    let shell = profiles::normalize(vec![ExtensionSpec::Process {
        id: "shell".into(),
        argv: vec!["sh".into(), "argument with spaces".into()],
    }])
    .unwrap();
    let ExtensionSpec::Process { argv, .. } = &shell[0] else {
        panic!()
    };
    assert!(std::path::Path::new(&argv[0]).is_absolute());
    assert_eq!(argv[1], "argument with spaces");
    std::fs::remove_file(executable).unwrap();
    let session = e.create(&fx.project, "a");
    assert!(
        e.call(Call::OpenSession {
            session,
            window: None
        })
        .is_err()
    );
    assert!(e.handle.shared().sessions.lock().unwrap().live.is_empty());
    drop(e);
    assert!(
        crate::runtime::EngineHandle::start(fx.config()).is_err(),
        "missing sticky executable cannot silently disable itself"
    );
}

#[test]
fn native_profile_results_match_the_pinned_pi_profile_helper_trace() {
    let reference: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/pi-extension-profiles/profiles.json"
    )))
    .unwrap();
    let expected = reference["observations"].as_object().unwrap();
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("answer"));
    let one = e.create(&fx.project, "one");
    let empty = e.create(&fx.project, "empty");
    let mut actual = serde_json::Map::new();
    let mut record = |name: &str, selected: Vec<ExtensionSpec>| {
        let names: Vec<String> = selected
            .into_iter()
            .map(|s| match s {
                ExtensionSpec::Builtin(name) => name,
                _ => panic!("native fixture labels"),
            })
            .collect();
        actual.insert(name.into(), serde_json::json!(names));
    };
    record("server_absent", selected(&mut e, None, None));
    record(
        "server_explicit",
        selected(&mut e, None, Some([questions(), greeting()].concat())),
    );
    record("server_restored", selected(&mut e, None, None));
    record(
        "session_pinned",
        selected(&mut e, Some(&one), Some(questions())),
    );
    record(
        "session_empty",
        selected(&mut e, Some(&empty), Some(vec![])),
    );
    record("server_changed", selected(&mut e, None, Some(greeting())));
    record(
        "session_after_default_change",
        selected(&mut e, Some(&one), None),
    );
    record(
        "empty_after_default_change",
        selected(&mut e, Some(&empty), None),
    );
    record("server_cleared", selected(&mut e, None, Some(vec![])));
    record("server_after_clear", selected(&mut e, None, None));
    record(
        "session_after_server_clear",
        selected(&mut e, Some(&one), None),
    );
    actual.insert(
        "duplicate_rejected".into(),
        serde_json::json!(
            e.call(Call::PrepareExtensions {
                session: None,
                extensions: Some([questions(), questions()].concat())
            })
            .is_err()
        ),
    );
    e.handle
        .shared()
        .store
        .tx(|tx| {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1, ?2)",
                rusqlite::params![profiles::KEY, r#"{"version":2,"extensions":[]}"#],
            )?;
            Ok(())
        })
        .unwrap();
    actual.insert(
        "future_profile_rejected".into(),
        serde_json::json!(
            e.call(Call::PrepareExtensions {
                session: None,
                extensions: None
            })
            .is_err()
        ),
    );
    assert_eq!(&actual, expected);
}

#[test]
fn a_failed_first_pin_keeps_its_selection_and_refuses_work_until_explicit_reopen() {
    let fx = Fixture::new();
    let provider = Scripted::echo("answer");
    let mut e = fx.start(&provider);
    selected(&mut e, None, Some(questions()));
    let session = e.create(&fx.project, "pending");
    e.handle.shared().store.fail_next_write();
    assert_eq!(
        e.prompt(&session, "not-admitted", "no").unwrap_err().code,
        ErrorCode::Storage
    );
    assert!(
        e.handle
            .shared()
            .store
            .kv_get(&session, profiles::KEY)
            .unwrap()
            .is_none()
    );
    assert!(provider.requests().is_empty());
    selected(&mut e, None, Some(greeting()));
    assert_eq!(
        e.call(Call::PrepareExtensions {
            session: Some(session.clone()),
            extensions: None
        })
        .unwrap_err()
        .code,
        ErrorCode::Storage
    );
    assert_eq!(
        e.prompt(&session, "still-refused", "no").unwrap_err().code,
        ErrorCode::Storage
    );
    assert_eq!(
        e.call(Call::Lookup {
            session: session.clone(),
            request: "not-admitted".into()
        })
        .unwrap(),
        Reply::Lookup(LookupResult::Unknown)
    );
    e.snapshot(&session); // Persist the first selected profile, then clear the failure.
    assert_eq!(selected(&mut e, Some(&session), None), questions());
    assert_eq!(
        profiles::pin(&e.handle.shared().store, &session).unwrap(),
        questions()
    );
    e.prompt(&session, "after-reopen", "yes").unwrap();
    assert_eq!(e.settled("after-reopen"), RunOutcome::Completed);
    assert_eq!(provider.requests().len(), 1);
}

#[test]
fn explicit_startup_clear_repairs_a_bad_default_without_changing_session_pins() {
    let fx = Fixture::new();
    let provider = Scripted::echo("answer");
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "pinned");
    selected(&mut e, Some(&session), Some(questions()));
    e.handle
        .shared()
        .store
        .tx(|tx| {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1,'not json')",
                [profiles::KEY],
            )?;
            Ok(())
        })
        .unwrap();
    drop(e);
    assert!(crate::runtime::EngineHandle::start(fx.config()).is_err());
    let mut config = fx.config();
    config.clear_extensions = true;
    let mut e = fx.start_config(&provider, config.clone());
    assert!(selected(&mut e, None, None).is_empty());
    assert_eq!(selected(&mut e, Some(&session), None), questions());
    drop(e);
    config.extensions = greeting();
    assert!(
        crate::runtime::EngineHandle::start(config).is_err(),
        "clear and explicit selection conflict"
    );
    let mut e = fx.start(&provider);
    assert!(selected(&mut e, None, None).is_empty());
}
