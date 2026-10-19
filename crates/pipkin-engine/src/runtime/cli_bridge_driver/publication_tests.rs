use super::Publication;
use crate::{
    execution::claude_cli::orchestration_protocol::Scope,
    store::{self, cli_agent_turns::projection::Update},
};
use pipkin_engine_protocol::{EntryKind, NoticeLevel, SessionChange};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
fn scope() -> Scope {
    Scope {
        session: "s".into(),
        generation: 1,
        operation: "op".into(),
        run: 7,
    }
}
fn update(scope: Scope, revision: u64) -> Update {
    Update {
        scope,
        revision,
        changes: vec![SessionChange::RunChanged { run: None }],
    }
}
#[test]
fn foreign_or_revoked_scope_cannot_even_enter_a_commit() {
    let live = Arc::new(AtomicBool::new(true));
    let current = live.clone();
    let publisher = Publication::new(
        Arc::default(),
        scope(),
        move || current.load(Ordering::SeqCst),
        |_| panic!("must not publish"),
    );
    for dimension in 0..4 {
        let mut foreign = scope();
        match dimension {
            0 => foreign.session.push('x'),
            1 => foreign.generation += 1,
            2 => foreign.operation.push('x'),
            _ => foreign.run += 1,
        };
        assert!(
            publisher
                .commit(&foreign, || -> store::Result<((), Option<Update>)> {
                    panic!("must not commit")
                })
                .is_err()
        );
    }
    live.store(false, Ordering::SeqCst);
    assert!(
        publisher
            .commit(&scope(), || -> store::Result<((), Option<Update>)> {
                panic!("revoked")
            })
            .is_err()
    );
}
#[test]
fn failed_transaction_emits_nothing_and_after_commit_revocation_never_retries() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let emitted = events.clone();
    let live = Arc::new(AtomicBool::new(true));
    let current = live.clone();
    let publisher = Publication::new(
        Arc::default(),
        scope(),
        move || current.load(Ordering::SeqCst),
        move |u| emitted.lock().unwrap().push(u.revision),
    );
    let store = crate::store::Store::memory().unwrap();
    store
        .create_session(
            "s",
            "/project",
            None,
            &Default::default(),
            None,
            None,
            None,
            1,
        )
        .unwrap();
    store.fail_next_write();
    let result = publisher.commit(&scope(), || {
        store.tx(|c| {
            crate::store::Store::append_entry(
                c,
                "s",
                2,
                None,
                &EntryKind::Notice {
                    level: NoticeLevel::Info,
                    text: "not committed".into(),
                },
            )?;
            Ok(((), Some(update(scope(), 2))))
        })
    });
    assert!(result.is_err());
    assert!(events.lock().unwrap().is_empty());
    let mut effects = 0;
    assert!(
        publisher
            .commit(&scope(), || {
                effects += 1;
                live.store(false, Ordering::SeqCst);
                Ok(((), Some(update(scope(), 3))))
            })
            .is_err()
    );
    assert_eq!(effects, 1);
    assert!(events.lock().unwrap().is_empty());
}
#[test]
fn native_gate_holds_through_delivery_and_orders_competing_writers() {
    let gate = Arc::new(Mutex::new(()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let emitted = events.clone();
    let (entered, seen) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let publisher = Publication::new(
        gate.clone(),
        scope(),
        || true,
        move |u| emitted.lock().unwrap().push(u.revision),
    );
    let first = std::thread::spawn(move || {
        publisher
            .commit(&scope(), || {
                entered.send(()).unwrap();
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                Ok(((), Some(update(scope(), 1))))
            })
            .unwrap()
    });
    seen.recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(gate.try_lock().is_err());
    let ordinary_events = events.clone();
    let second = std::thread::spawn(move || {
        let _gate = gate.lock().unwrap();
        ordinary_events.lock().unwrap().push(2);
    });
    release.send(()).unwrap();
    first.join().unwrap();
    second.join().unwrap();
    assert_eq!(*events.lock().unwrap(), vec![1, 2]);
}
