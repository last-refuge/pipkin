//! Real private Unix sockets + real SQLite, fake hook clients only. No CLI/model calls.
use super::*;
use crate::{
    execution::claude_cli::permission_wire as wire, runtime::external_permission_server::Server,
};
use tokio::{
    net::UnixStream,
    sync::mpsc,
    time::{Instant as TokioInstant, timeout},
};

fn endpoint(
    s: &Setup,
    b: Arc<Broker>,
) -> (
    Server,
    Arc<crate::runtime::external_permission_server::Control>,
    std::path::PathBuf,
) {
    let _enter = s.shared.handle.enter();
    let (server, control, launch) = Server::bind(s._fixture.dir.path(), b).unwrap();
    (
        server,
        control,
        std::path::PathBuf::from(&launch.arguments()[2]),
    )
}
async fn submit(path: &Path, frame: &RequestFrame) -> UnixStream {
    let mut stream = UnixStream::connect(path).await.unwrap();
    wire::write_request(
        &mut stream,
        frame,
        TokioInstant::now() + Duration::from_secs(2),
    )
    .await
    .unwrap();
    stream
}
use std::path::Path;

#[test]
fn private_socket_roundtrip_is_owner_scoped_and_consumed_before_reply() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let (server, control, path) = endpoint(&s, b.clone());
    s.shared.handle.block_on(async {
        let (send, mut reviews) = mpsc::channel(16);
        let task = tokio::spawn(server.run(send));
        let frame = s.frame(&b);
        let mut client = submit(&path, &frame).await;
        let review = timeout(Duration::from_secs(2), reviews.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            control.lookup(7, &review.ticket).unwrap().unwrap().state,
            ExternalPermissionState::Pending
        );
        assert!(control.reviews(8).is_err());
        assert!(
            control
                .decide(8, &review.ticket, Decision::AllowOnce)
                .is_err()
        );
        let mut wrong = review.ticket.clone();
        wrong.fingerprint[0] ^= 1;
        assert!(control.decide(7, &wrong, Decision::AllowOnce).is_err());
        control
            .decide(7, &review.ticket, Decision::AllowOnce)
            .unwrap();
        assert_eq!(
            control.lookup(7, &review.ticket).unwrap().unwrap().state,
            ExternalPermissionState::Consumed
        );
        assert_eq!(
            wire::read_reply(
                &mut client,
                &frame,
                TokioInstant::now() + Duration::from_secs(2)
            )
            .await
            .unwrap(),
            wire::Decision::AllowOnce
        );
        assert!(
            control
                .decide(7, &review.ticket, Decision::AllowOnce)
                .is_err()
        );
        assert!(control.reviews(7).unwrap().is_empty());
        assert!(!b.seal().unresolved);
        control.fence();
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    });
    assert!(!path.exists());
    assert!(s.provider.requests().is_empty());
}

#[test]
fn denial_disconnect_and_extra_data_retire_authority() {
    for mode in 0..3 {
        let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
        let b = s.broker();
        let (server, control, path) = endpoint(&s, b.clone());
        s.shared.handle.block_on(async {
            let (send, mut reviews) = mpsc::channel(16);
            let task = tokio::spawn(server.run(send));
            let frame = s.frame(&b);
            let mut client = submit(&path, &frame).await;
            let review = timeout(Duration::from_secs(2), reviews.recv())
                .await
                .unwrap()
                .unwrap();
            match mode {
                0 => {
                    control.decide(7, &review.ticket, Decision::Deny).unwrap();
                    assert_eq!(
                        wire::read_reply(
                            &mut client,
                            &frame,
                            TokioInstant::now() + Duration::from_secs(2)
                        )
                        .await
                        .unwrap(),
                        wire::Decision::Deny
                    );
                }
                1 => drop(client),
                _ => {
                    use tokio::io::AsyncWriteExt;
                    client.write_all(b"unexpected second frame").await.unwrap();
                    assert!(
                        wire::read_reply(
                            &mut client,
                            &frame,
                            TokioInstant::now() + Duration::from_secs(2)
                        )
                        .await
                        .is_err()
                    );
                }
            }
            timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap();
            assert!(s.cancel.is_cancelled());
            assert_eq!(
                control.lookup(7, &review.ticket).unwrap().unwrap().state,
                if mode == 0 {
                    ExternalPermissionState::Denied
                } else {
                    ExternalPermissionState::Revoked
                }
            );
            assert!(
                control
                    .decide(7, &review.ticket, Decision::AllowOnce)
                    .is_err()
            );
        });
    }
}

#[test]
fn wrong_run_nonce_is_closed_without_publishing_or_interrupting_a_valid_peer() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let (server, control, path) = endpoint(&s, b.clone());
    s.shared.handle.block_on(async {
        let (send, mut reviews) = mpsc::channel(16);
        let task = tokio::spawn(server.run(send));
        let mut wrong = s.frame(&b);
        wrong.run_nonce = Binding::fresh().unwrap();
        let mut bad = submit(&path, &wrong).await;
        assert!(
            wire::read_reply(
                &mut bad,
                &wrong,
                TokioInstant::now() + Duration::from_secs(2)
            )
            .await
            .is_err()
        );
        assert!(reviews.try_recv().is_err());
        assert!(!s.cancel.is_cancelled());
        let frame = s.frame(&b);
        let mut client = submit(&path, &frame).await;
        let review = timeout(Duration::from_secs(2), reviews.recv())
            .await
            .unwrap()
            .unwrap();
        control
            .decide(7, &review.ticket, Decision::AllowOnce)
            .unwrap();
        assert_eq!(
            wire::read_reply(
                &mut client,
                &frame,
                TokioInstant::now() + Duration::from_secs(2)
            )
            .await
            .unwrap(),
            wire::Decision::AllowOnce
        );
        control.fence();
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    });
}

#[test]
fn full_review_queue_revokes_committed_pending_requests() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let (server, control, path) = endpoint(&s, b.clone());
    s.shared.handle.block_on(async {
        let (send, mut reviews) = mpsc::channel(1);
        let task = tokio::spawn(server.run(send));
        let first = s.frame(&b);
        let mut one = submit(&path, &first).await;
        timeout(Duration::from_secs(2), async {
            while reviews.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let second = s.frame(&b);
        let _two = submit(&path, &second).await;
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        let review = reviews.recv().await.unwrap();
        assert_eq!(
            control.lookup(7, &review.ticket).unwrap().unwrap().state,
            ExternalPermissionState::Revoked
        );
        assert!(
            wire::read_reply(
                &mut one,
                &first,
                TokioInstant::now() + Duration::from_secs(2)
            )
            .await
            .is_err()
        );
        assert!(control.reviews(7).unwrap().is_empty());
        assert!(b.seal().unresolved);
    });
}

#[test]
fn unpolled_or_aborted_server_fences_and_tracks_connection_cleanup() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let (server, _control, path) = endpoint(&s, b);
    drop(server);
    assert!(s.cancel.is_cancelled());
    assert!(!path.exists());
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let (server, control, path) = endpoint(&s, b.clone());
    s.shared.handle.block_on(async {
        let (send, mut reviews) = mpsc::channel(16);
        let task = tokio::spawn(server.run(send));
        let frame = s.frame(&b);
        let _client = submit(&path, &frame).await;
        let review = timeout(Duration::from_secs(2), reviews.recv())
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.is_err());
        timeout(Duration::from_secs(2), async {
            while !s.shared.external_tasks.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(s.cancel.is_cancelled());
        assert_eq!(
            control.lookup(7, &review.ticket).unwrap().unwrap().state,
            ExternalPermissionState::Revoked
        );
    });
    assert!(!path.exists());
}

#[test]
fn grant_revocation_and_expiry_close_pending_sockets_without_allow() {
    for expire in [false, true] {
        let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
        let mut lease = s.lease();
        if expire {
            lease.expires = Instant::now() + Duration::from_secs(2);
        }
        let b = Broker::reserve(&s.shared, &s.receipt, lease).unwrap();
        let (server, control, path) = endpoint(&s, b.clone());
        s.shared.handle.block_on(async {
            let (send, mut reviews) = mpsc::channel(16);
            let task = tokio::spawn(server.run(send));
            let frame = s.frame(&b);
            let mut client = submit(&path, &frame).await;
            let review = timeout(Duration::from_secs(2), reviews.recv())
                .await
                .unwrap()
                .unwrap();
            if !expire {
                s.revoked.cancel();
            }
            timeout(Duration::from_secs(4), task)
                .await
                .unwrap()
                .unwrap();
            assert!(
                wire::read_reply(
                    &mut client,
                    &frame,
                    TokioInstant::now() + Duration::from_secs(1)
                )
                .await
                .is_err()
            );
            assert_eq!(
                control.lookup(7, &review.ticket).unwrap().unwrap().state,
                ExternalPermissionState::Revoked
            );
            assert!(
                control
                    .decide(7, &review.ticket, Decision::AllowOnce)
                    .is_err()
            );
            assert!(s.cancel.is_cancelled());
        });
    }
}

#[test]
fn connection_capacity_is_bounded_without_eviction() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let (server, control, path) = endpoint(&s, b);
    s.shared.handle.block_on(async {
        let (send, mut reviews) = mpsc::channel(16);
        let task = tokio::spawn(server.run(send));
        let mut clients = Vec::new();
        for _ in 0..16 {
            clients.push(UnixStream::connect(&path).await.unwrap());
        }
        timeout(Duration::from_secs(1), async {
            while s.shared.external_tasks.len() < 17 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        clients.push(UnixStream::connect(&path).await.unwrap());
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(s.cancel.is_cancelled());
        assert!(s.shared.external_tasks.is_empty());
        assert!(reviews.try_recv().is_err());
        assert!(control.reviews(7).unwrap().is_empty());
    });
}

#[test]
fn authenticated_malformed_payload_stops_without_review() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let (server, control, path) = endpoint(&s, b.clone());
    s.shared.handle.block_on(async {
        let (send, mut reviews) = mpsc::channel(16);
        let task = tokio::spawn(server.run(send));
        let frame = RequestFrame::new(b.run_nonce(), "not JSON".into()).unwrap();
        let mut client = submit(&path, &frame).await;
        assert!(
            wire::read_reply(
                &mut client,
                &frame,
                TokioInstant::now() + Duration::from_secs(2)
            )
            .await
            .is_err()
        );
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(reviews.try_recv().is_err());
        assert!(control.reviews(7).unwrap().is_empty());
        assert!(s.cancel.is_cancelled());
    });
}

#[test]
fn listener_claim_cannot_be_duplicated_or_recreated_after_retirement() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let (server, _control, path) = endpoint(&s, b.clone());
    let _enter = s.shared.handle.enter();
    assert!(Server::bind(s._fixture.dir.path(), b.clone()).is_err());
    assert!(!s.cancel.is_cancelled()); // A failed duplicate does not own the first listener.
    assert!(path.exists());
    drop(server);
    assert!(s.cancel.is_cancelled());
    assert!(Server::bind(s._fixture.dir.path(), b).is_err());
    assert!(!path.exists());
}

#[test]
fn immediate_peer_close_after_allow_does_not_retire_a_completed_exchange() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let (server, control, path) = endpoint(&s, b.clone());
    s.shared.handle.block_on(async {
        let (send, mut reviews) = mpsc::channel(16);
        let server = tokio::spawn(server.run(send));
        for _ in 0..16 {
            let frame = s.frame(&b);
            let mut client = submit(&path, &frame).await;
            let reader = tokio::spawn(async move {
                let decision = wire::read_reply(
                    &mut client,
                    &frame,
                    TokioInstant::now() + Duration::from_secs(2),
                )
                .await
                .unwrap();
                drop(client);
                decision
            });
            let review = timeout(Duration::from_secs(2), reviews.recv())
                .await
                .unwrap()
                .unwrap();
            control
                .decide(7, &review.ticket, Decision::AllowOnce)
                .unwrap();
            assert_eq!(reader.await.unwrap(), wire::Decision::AllowOnce);
            tokio::task::yield_now().await;
            assert!(!s.cancel.is_cancelled());
        }
        control.fence();
        timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    });
}

#[test]
fn project_endpoint_rejection_fences_the_reserved_run() {
    let s = Setup::new(EXTERNAL_PERMISSION_POLICY);
    let b = s.broker();
    let _enter = s.shared.handle.enter();
    assert!(Server::bind(&s._fixture.project, b).is_err());
    assert!(s.cancel.is_cancelled());
}
