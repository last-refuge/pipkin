#![cfg(unix)] // Captured filesystem paths follow the supported Unix CLI policy.
use super::super::tests::{Engine, Fixture};
use super::*;
use crate::{
    provider::scripted::Scripted,
    store::{CliBindingSpec, ExternalRunState},
};
use serde_json::json;

#[cfg(unix)]
#[path = "external_permission_server_tests.rs"]
mod server;

const UUID: &str = "11111111-1111-4111-8111-111111111111";
struct Setup {
    _engine: Engine,
    shared: Arc<Shared>,
    _fixture: Fixture,
    receipt: ExternalPromptReceipt,
    revoked: CancellationToken,
    cancel: CancellationToken,
    provider: Scripted,
}
impl Setup {
    fn new(policy: &str) -> Self {
        let f = Fixture::new();
        let provider = Scripted::echo("never used");
        let engine = f.start(&provider);
        let shared = engine.handle.shared().clone();
        shared
            .store
            .create_cli_session_with_model(
                "cli",
                "create",
                &CliBindingSpec {
                    cli_session_id: UUID.into(),
                    project_path: f.project.to_str().unwrap().into(),
                    executable_path: "/fixture/claude".into(),
                    cli_version: "2.1.296".into(),
                    config_ref: "config".into(),
                    account_scope: "account".into(),
                    launch_policy: policy.into(),
                },
                "sonnet",
                1,
            )
            .unwrap();
        let receipt = shared
            .store
            .tx(|tx| Store::admit_external_prompt_in(tx, "cli", "request", "question", "op", 1, 2))
            .unwrap()
            .0
            .receipt;
        shared
            .store
            .tx(|tx| Store::begin_external_launch_in(tx, &receipt.key, 3))
            .unwrap();
        Self {
            _engine: engine,
            shared,
            _fixture: f,
            receipt,
            revoked: CancellationToken::new(),
            cancel: CancellationToken::new(),
            provider,
        }
    }
    fn lease(&self) -> Lease {
        Lease {
            owner: 7,
            expires: Instant::now() + Duration::from_secs(30),
            revoked: self.revoked.clone(),
            run_cancel: self.cancel.clone(),
        }
    }
    fn broker(&self) -> Arc<Broker> {
        Broker::reserve(&self.shared, &self.receipt, self.lease()).unwrap()
    }
    fn frame(&self, b: &Broker) -> RequestFrame {
        RequestFrame::new(b.run_nonce(), json!({"session_id":UUID,"cwd":self._fixture.project.to_str().unwrap(),"hook_event_name":"PermissionRequest","tool_name":"Write","tool_input":{"file_path":"/fixture/file","content":"private argument canary"}}).to_string()).unwrap()
    }
    fn state(&self, b: &Broker, review: &Review) -> ExternalPermissionState {
        let key = b.request(&review.ticket, review.expires_at_ms).key;
        self.shared
            .store
            .read(|c| Store::external_permission_in(c, &key))
            .unwrap()
            .unwrap()
            .state
    }
}

#[test]
fn expired_request_and_pending_commit_failure_never_publish_or_deliver() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let review = b.admit(&s.frame(&b)).unwrap();
    b.state
        .lock()
        .unwrap()
        .pending
        .get_mut(&review.ticket.nonce)
        .unwrap()
        .expires = Instant::now();
    assert!(b.reviews(7).unwrap().is_empty());
    assert!(
        b.decide(
            7,
            &review.ticket,
            Decision::AllowOnce,
            |_| -> Result<(), ()> { panic!("expired") }
        )
        .is_err()
    );
    assert_eq!(s.state(&b, &review), ExternalPermissionState::Revoked);
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    s.shared.store.fail_next_write();
    assert!(b.admit(&s.frame(&b)).is_err());
    assert!(b.reviews(7).unwrap().is_empty());
    assert!(s.cancel.is_cancelled());
}

#[test]
fn panic_after_consumption_cancels_fences_and_remains_read_only() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let review = b.admit(&s.frame(&b)).unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.decide(
                7,
                &review.ticket,
                Decision::AllowOnce,
                |_| -> Result<(), ()> { panic!("synthetic delivery panic") },
            )
        }))
        .is_err()
    );
    assert!(s.cancel.is_cancelled());
    assert!(b.reviews(7).unwrap().is_empty());
    assert!(b.fence());
    assert_eq!(
        b.lookup(7, &review.ticket).unwrap().unwrap().state,
        ExternalPermissionState::Consumed
    );
    assert!(b.lookup(8, &review.ticket).is_err());
    let mut wrong = review.ticket.clone();
    wrong.fingerprint[0] ^= 1;
    assert!(b.lookup(7, &wrong).is_err());
}

#[test]
fn revoke_racing_a_committed_decision_prevents_delivery() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let review = b.admit(&s.frame(&b)).unwrap();
    let (entered, at_commit) = std::sync::mpsc::sync_channel(1);
    let (release, wait) = std::sync::mpsc::sync_channel(1);
    *b.after_commit.lock().unwrap() = Some(Box::new(move || {
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
    }));
    let deciding = b.clone();
    let ticket = review.ticket.clone();
    let decision = std::thread::spawn(move || {
        deciding.decide(7, &ticket, Decision::AllowOnce, |_| -> Result<(), ()> {
            panic!("allow after revoke")
        })
    });
    at_commit.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(s.state(&b, &review), ExternalPermissionState::Consumed);
    s.revoked.cancel();
    let retiring = b.clone();
    let fence = std::thread::spawn(move || retiring.fence());
    release.send(()).unwrap();
    assert!(decision.join().unwrap().is_err());
    assert!(fence.join().unwrap());
    assert!(s.cancel.is_cancelled());
    assert_eq!(s.state(&b, &review), ExternalPermissionState::Consumed);
}

#[test]
fn a_preexisting_store_decision_is_not_another_delivery_grant() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let review = b.admit(&s.frame(&b)).unwrap();
    let request = b.request(&review.ticket, review.expires_at_ms);
    s.shared
        .store
        .tx(|tx| {
            Store::decide_external_permission_in(
                tx,
                &request,
                ExternalPermissionDecision::AllowOnce,
                now_ms(),
            )
        })
        .unwrap();
    assert!(
        b.decide(
            7,
            &review.ticket,
            Decision::AllowOnce,
            |_| -> Result<(), ()> { panic!("recreated authority") }
        )
        .is_err()
    );
    assert_eq!(s.state(&b, &review), ExternalPermissionState::Revoked);
}

#[test]
fn durable_pending_and_consumption_precede_display_and_delivery() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let review = b.admit(&s.frame(&b)).unwrap();
    assert_eq!(s.state(&b, &review), ExternalPermissionState::Pending);
    assert!(review.arguments.contains("private argument canary"));
    assert_eq!(b.reviews(7).unwrap().len(), 1);
    assert!(matches!(b.reviews(8), Err(Error::Scope)));
    b.decide(7, &review.ticket, Decision::AllowOnce, |bytes| {
        assert_eq!(s.state(&b, &review), ExternalPermissionState::Consumed);
        assert_eq!(serde_json::from_slice::<serde_json::Value>(bytes).unwrap()["hookSpecificOutput"]["decision"]["behavior"], "allow");
        Ok::<_, ()>(())
    }).unwrap();
    assert!(b.reviews(7).unwrap().is_empty());
    assert!(
        b.decide(
            7,
            &review.ticket,
            Decision::AllowOnce,
            |_| -> Result<(), ()> { panic!("reissued") }
        )
        .is_err()
    );
    assert_eq!(
        b.seal(),
        Seal {
            revocation_confirmed: true,
            unresolved: false
        }
    );
    assert!(s.provider.requests().is_empty());
    for table in ["runs", "requests", "tool_calls"] {
        assert_eq!(
            s.shared
                .store
                .read(|c| Ok(c
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))?))
                .unwrap(),
            0
        );
    }
}

#[test]
fn reservation_is_exclusive_policy_bound_and_epoch_checked() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let _b = s.broker();
    assert!(Broker::reserve(&s.shared, &s.receipt, s.lease()).is_err());
    let chat = Setup::new(crate::execution::claude_cli::policy::POLICY_ID);
    assert!(matches!(
        Broker::reserve(&chat.shared, &chat.receipt, chat.lease()),
        Err(Error::Scope)
    ));
    let old = Setup::new(EXTERNAL_PERMISSION_POLICY);
    old.shared.store.next_epoch().unwrap();
    assert!(Broker::reserve(&old.shared, &old.receipt, old.lease()).is_err());
    assert_eq!(
        old.shared
            .store
            .external_prompt_by_id(old.receipt.id)
            .unwrap()
            .unwrap()
            .state,
        ExternalRunState::LaunchIntent
    );
}

#[test]
fn scope_digest_and_duplicate_invocations_cannot_release_authority() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let mut frame = s.frame(&b);
    let original = frame.run_nonce;
    frame.run_nonce = Binding::fresh().unwrap();
    assert!(matches!(b.admit(&frame), Err(Error::Scope)));
    frame.run_nonce = original;
    let review = b.admit(&frame).unwrap();
    assert!(b.admit(&frame).is_err());
    assert!(
        b.decide(
            8,
            &review.ticket,
            Decision::AllowOnce,
            |_| -> Result<(), ()> { panic!("foreign owner") }
        )
        .is_err()
    );
    for variant in 0..4 {
        let mut ticket = review.ticket.clone();
        match variant {
            0 => ticket.fingerprint[0] ^= 1,
            1 => ticket.scope.key.generation += 1,
            2 => ticket.scope.epoch += 1,
            _ => ticket.nonce[0] ^= 1,
        }
        assert!(
            b.decide(7, &ticket, Decision::AllowOnce, |_| -> Result<(), ()> {
                panic!("stale answer")
            })
            .is_err()
        );
    }
    assert_eq!(s.state(&b, &review), ExternalPermissionState::Pending);
}

#[test]
fn lost_delivery_and_failed_persistence_never_reissue() {
    for fail_commit in [false, true] {
        let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
        let b = s.broker();
        let review = b.admit(&s.frame(&b)).unwrap();
        if fail_commit {
            s.shared.store.fail_next_write();
        }
        let mut delivered = false;
        assert!(
            b.decide(7, &review.ticket, Decision::AllowOnce, |_| {
                delivered = true;
                Err::<(), _>(())
            })
            .is_err()
        );
        assert_eq!(delivered, !fail_commit);
        assert!(s.cancel.is_cancelled());
        assert!(
            b.decide(
                7,
                &review.ticket,
                Decision::AllowOnce,
                |_| -> Result<(), ()> { panic!("retry") }
            )
            .is_err()
        );
        if !fail_commit {
            assert_eq!(s.state(&b, &review), ExternalPermissionState::Consumed);
        }
    }
}

#[test]
fn cancellation_after_commit_blocks_delivery_but_keeps_consumed_receipt() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let review = b.admit(&s.frame(&b)).unwrap();
    let token = s.revoked.clone();
    *b.after_commit.lock().unwrap() = Some(Box::new(move || token.cancel()));
    assert!(
        b.decide(
            7,
            &review.ticket,
            Decision::AllowOnce,
            |_| -> Result<(), ()> { panic!("late allow") }
        )
        .is_err()
    );
    assert_eq!(s.state(&b, &review), ExternalPermissionState::Consumed);
    assert!(s.cancel.is_cancelled());
}

#[test]
fn deny_and_fence_revoke_other_requests_and_preserve_uncertainty() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let first = b.admit(&s.frame(&b)).unwrap();
    let second = b.admit(&s.frame(&b)).unwrap();
    b.decide(7, &first.ticket, Decision::Deny, |_| Ok::<_, ()>(()))
        .unwrap();
    assert_eq!(s.state(&b, &first), ExternalPermissionState::Denied);
    assert_eq!(s.state(&b, &second), ExternalPermissionState::Revoked);
    assert!(s.cancel.is_cancelled());
    assert_eq!(
        b.seal(),
        Seal {
            revocation_confirmed: true,
            unresolved: true
        }
    );
    assert!(b.fence());
    assert!(b.admit(&s.frame(&b)).is_err());
}

#[test]
fn failed_revocation_still_cancels_and_cannot_claim_confirmation() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let review = b.admit(&s.frame(&b)).unwrap();
    s.shared.store.fail_next_write();
    assert!(!b.fence());
    assert!(s.cancel.is_cancelled());
    assert_eq!(
        b.seal(),
        Seal {
            revocation_confirmed: false,
            unresolved: true
        }
    );
    assert_eq!(s.state(&b, &review), ExternalPermissionState::Pending);
    assert!(
        b.decide(
            7,
            &review.ticket,
            Decision::AllowOnce,
            |_| -> Result<(), ()> { panic!("allow after failed fence") }
        )
        .is_err()
    );
}

#[test]
fn removal_and_drop_retire_pending_authority() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let review = b.admit(&s.frame(&b)).unwrap();
    s.shared.store.begin_removal("cli").unwrap();
    assert!(b.fence());
    assert_eq!(s.state(&b, &review), ExternalPermissionState::Revoked);
    let other = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = other.broker();
    let review = b.admit(&other.frame(&b)).unwrap();
    let key = b.request(&review.ticket, review.expires_at_ms).key;
    drop(b);
    assert!(other.cancel.is_cancelled());
    assert_eq!(
        other
            .shared
            .store
            .read(|c| Store::external_permission_in(c, &key))
            .unwrap()
            .unwrap()
            .state,
        ExternalPermissionState::Revoked
    );
}
