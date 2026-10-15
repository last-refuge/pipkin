use super::tests::Fixture;
use crate::host::Connection;
use crate::provider::scripted::{Gate, Reply as R, Scripted};
use pipkin_engine_protocol::*;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub(super) fn send(conn: &Connection, id: u64, call: Call) {
    conn.send(ClientFrame {
        id,
        lane: call.lane(),
        call,
    })
    .unwrap();
}
pub(super) fn replies(conn: &Connection, count: usize) -> BTreeMap<u64, Result<Reply, ProtoError>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut out = BTreeMap::new();
    while out.len() < count {
        if let ServerFrame::Reply { id, result } = conn
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("reply")
        {
            assert!(out.insert(id, result).is_none(), "one reply per request");
        }
    }
    out
}
pub(super) fn until(done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn cancelled_input_rechecks_after_a_ready_lock_even_without_an_outer_select_poll() {
    use std::{future::Future, task::Poll};
    for mode in [InputMode::Prompt, InputMode::Steer, InputMode::FollowUp] {
        let fx = Fixture::new();
        let provider = Scripted::echo("never");
        let mut e = fx.start(&provider);
        let session = e.create(&fx.project, "queued cancellation");
        let shared = e.handle.shared().clone();
        let sess = shared.session(&session).unwrap();
        let lock = sess.lock.blocking_lock();
        let cancel = tokio_util::sync::CancellationToken::new();
        let call = if mode == InputMode::Prompt {
            Call::Prompt {
                session: session.clone(),
                request: "cancel-gap".into(),
                text: "work".into(),
                images: vec![],
                model: None,
            }
        } else {
            Call::Enqueue {
                session: session.clone(),
                request: "cancel-gap".into(),
                text: "work".into(),
                images: vec![],
                mode,
            }
        };
        let result = shared.handle.block_on(async {
            let mut admission = Box::pin(super::dispatch_with_cancel(
                &shared,
                e.conn.client_id(),
                call,
                &cancel,
            ));
            std::future::poll_fn(|cx| {
                assert!(admission.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            // Reproduce the select gap: cancellation was checked before this lock became
            // ready; now directly resume admission, without polling an outer cancel branch.
            drop(lock);
            cancel.cancel();
            admission.await
        });
        assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
        assert!(
            shared
                .store
                .request(&session, "cancel-gap")
                .unwrap()
                .is_none()
        );
        assert!(provider.requests().is_empty());
        e.prompt(&session, "after-cancel", "usable").unwrap();
        assert!(matches!(e.settled("after-cancel"), RunOutcome::Completed));
    }
}

#[test]
fn cancellation_reaches_a_queued_prompt_even_with_every_normal_slot_occupied() {
    let fx = Fixture::new();
    let provider = Scripted::echo("answer");
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "s");
    let sess = e.handle.shared().session(&session).unwrap();
    let lock = sess.lock.blocking_lock();
    let conn = e.handle.connect("saturated");
    for id in 1..=17 {
        send(
            &conn,
            id,
            Call::Prompt {
                session: session.clone(),
                request: format!("p{id}"),
                text: "work".into(),
                images: vec![],
                model: None,
            },
        );
    }
    // Sixteen active calls are blocked on the session lock; the seventeenth stays queued.
    until(|| conn.tx.capacity() == 63);
    send(&conn, 100, Call::CancelRequest { request: 17 });
    assert_eq!(replies(&conn, 1).remove(&100), Some(Ok(Reply::Ok)));
    // Cancel the in-flight requests too. None may journal an input after the lock releases.
    let mut got = BTreeMap::new();
    for id in 1..=16 {
        send(&conn, 100 + id, Call::CancelRequest { request: id });
        // The queued cancelled request can also finish as slots become free.
        while !got.contains_key(&(100 + id)) {
            got.extend(replies(&conn, 1));
        }
    }
    drop(lock);
    while got.len() < 33 {
        got.extend(replies(&conn, 1));
    }
    for id in 1..=17 {
        assert_eq!(got[&id].as_ref().unwrap_err().code, ErrorCode::Cancelled);
        assert!(
            e.handle
                .shared()
                .store
                .request(&session, &format!("p{id}"))
                .unwrap()
                .is_none()
        );
    }
    assert!(provider.requests().is_empty());
    e.prompt(&session, "after-cancel", "still usable").unwrap();
    assert!(matches!(e.settled("after-cancel"), RunOutcome::Completed));
}

#[test]
fn request_cancellation_is_connection_scoped_and_never_aborts_an_admitted_run() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider =
        Scripted::new(move |_, _| R::Gated(g.clone(), Box::new(R::Text("finished".into()))));
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "s");
    e.prompt(&session, "input", "keep working").unwrap();
    let a = e.handle.connect("a");
    let b = e.handle.connect("b");
    for c in [&a, &b] {
        send(
            c,
            1,
            Call::WaitForPrompt {
                session: session.clone(),
                request: "input".into(),
            },
        );
    }
    until(|| e.handle.shared().prompt_waits.available_permits() == 62);
    send(&a, 2, Call::CancelRequest { request: 1 });
    let got = replies(&a, 2);
    assert_eq!(got[&1].as_ref().unwrap_err().code, ErrorCode::Cancelled);
    assert_eq!(got[&2], Ok(Reply::Ok));
    until(|| e.handle.shared().prompt_waits.available_permits() == 63);
    // Cancelling an unknown or already completed RPC has no future effect.
    send(&a, 3, Call::CancelRequest { request: 999 });
    send(&a, 4, Call::CancelRequest { request: 1 });
    assert!(replies(&a, 2).values().all(|r| *r == Ok(Reply::Ok)));
    send(&a, 999, Call::ListSessions);
    assert!(matches!(replies(&a, 1)[&999], Ok(Reply::Sessions { .. })));
    assert!(
        e.handle
            .shared()
            .store
            .request(&session, "input")
            .unwrap()
            .unwrap()
            .outcome
            .is_none()
    );
    gate.open();
    assert!(
        matches!(replies(&b, 1)[&1], Ok(Reply::PromptResult(PromptResult::Done { ref text, .. })) if text == "finished")
    );
    assert!(matches!(e.settled("input"), RunOutcome::Completed));
    assert_eq!(provider.requests().len(), 1);
}

#[test]
fn cancelling_manual_compaction_stops_the_summary_and_preserves_context() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(
        fx.dir.path().join("config/settings.json"),
        r#"{"compaction":{"enabled":false,"keepRecentTokens":5}}"#,
    )
    .unwrap();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, n| {
        if n < 4 {
            R::Text("answer with some words".into())
        } else {
            R::Gated(g.clone(), Box::new(R::Text("summary".into())))
        }
    });
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "s");
    for i in 0..4 {
        e.prompt(&session, &format!("p{i}"), "turn with some words")
            .unwrap();
        e.settled(&format!("p{i}"));
    }
    let before = e.handle.shared().store.context_entries(&session).unwrap();
    let conn = e.handle.connect("compact");
    send(
        &conn,
        1,
        Call::Compact {
            session: session.clone(),
            instructions: None,
        },
    );
    until(|| provider.requests().len() == 5);
    send(&conn, 2, Call::CancelRequest { request: 1 });
    let got = replies(&conn, 2);
    assert_eq!(got[&1].as_ref().unwrap_err().code, ErrorCode::Cancelled);
    assert_eq!(got[&2], Ok(Reply::Ok));
    assert_eq!(
        e.handle.shared().store.context_entries(&session).unwrap(),
        before
    );
}
