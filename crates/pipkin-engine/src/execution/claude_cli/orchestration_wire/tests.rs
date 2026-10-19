use super::*;
use tokio::io::{DuplexStream, duplex};
fn end() -> Instant {
    Instant::now() + Duration::from_secs(10)
}
async fn pair() -> (Channel<DuplexStream>, Channel<DuplexStream>) {
    let (a, b) = duplex(1024);
    let secret = Secret::fresh().unwrap();
    let scope = [7; 32];
    let (host, client) = tokio::join!(
        async {
            authenticate(a, secret.clone(), scope, end())
                .await
                .unwrap()
                .acknowledge(end())
                .await
                .unwrap()
        },
        connect(b, secret.clone(), scope, end())
    );
    (host, client.unwrap())
}
#[tokio::test]
async fn mutual_proof_and_bidirectional_frames_preserve_exact_bytes() {
    let (host, client) = pair().await;
    let (mut hr, mut hw) = host.split();
    let (mut cr, mut cw) = client.split();
    let payload = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}";
    for _ in 0..3 {
        let (sent, got) = tokio::join!(cw.write(payload, end()), hr.read(end()));
        sent.unwrap();
        assert_eq!(got.unwrap(), payload);
        let (sent, got) = tokio::join!(hw.write(b"result", end()), cr.read(end()));
        sent.unwrap();
        assert_eq!(got.unwrap(), b"result");
    }
}
#[tokio::test]
async fn wrong_secret_or_scope_never_authenticates_either_side() {
    for wrong_scope in [false, true] {
        let (a, b) = duplex(1024);
        let secret = Secret::fresh().unwrap();
        let other = if wrong_scope {
            secret.clone()
        } else {
            Secret::fresh().unwrap()
        };
        let (host, client) = tokio::join!(
            authenticate(a, secret, [0; 32], end()),
            connect(b, other, if wrong_scope { [1; 32] } else { [0; 32] }, end())
        );
        assert!(host.is_err());
        assert!(client.is_err());
    }
}
#[tokio::test]
async fn client_never_gets_a_channel_without_commit_acknowledgement() {
    let (a, b) = duplex(1024);
    let secret = Secret::fresh().unwrap();
    let ((), client) = tokio::join!(
        async {
            drop(
                authenticate(a, secret.clone(), [0; 32], end())
                    .await
                    .unwrap(),
            );
        },
        connect(b, secret.clone(), [0; 32], end())
    );
    assert!(client.is_err());
}
#[test]
fn role_and_transcript_proofs_are_not_interchangeable() {
    let secret = Secret::fresh().unwrap();
    let transcript = [3; 32];
    let tag = secret.sign(b"server", &[&transcript]);
    assert!(secret.verify(b"client", &[&transcript], &tag).is_err());
    assert!(secret.verify(b"committed", &[&transcript], &tag).is_err());
    assert!(secret.verify(b"server", &[&[4; 32]], &tag).is_err());
    let frame = secret.sign(b"request", &[&transcript, &0u64.to_be_bytes(), b"data"]);
    for (role, seq, body) in [
        (b"response".as_slice(), 0, b"data".as_slice()),
        (b"request", 1, b"data"),
        (b"request", 0, b"changed"),
    ] {
        assert!(
            secret
                .verify(
                    role,
                    &[&transcript, &(seq as u64).to_be_bytes(), body],
                    &frame
                )
                .is_err()
        );
    }
    assert!(!format!("{secret:?}").contains(&secret.to_hex()));
    assert_eq!(format!("{Rejected}"), "CLI bridge channel rejected");
}
fn raw_packet(secret: &Secret, transcript: [u8; 32], sequence: u64, payload: &[u8]) -> Vec<u8> {
    let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
    bytes.extend(sequence.to_be_bytes());
    bytes.extend(secret.sign(b"request", &[&transcript, &sequence.to_be_bytes(), payload]));
    bytes.extend(payload);
    bytes
}
#[tokio::test]
async fn tampering_sequence_replay_and_direction_confusion_revoke_both_halves() {
    for case in 0..4 {
        let (a, mut b) = duplex(1024);
        let secret = Secret::fresh().unwrap();
        let (mut read, mut write) = Channel::new(a, secret.clone(), [3; 32], true, end()).split();
        let mut packet = raw_packet(&secret, [3; 32], if case == 1 { 1 } else { 0 }, b"payload");
        if case == 0 {
            *packet.last_mut().unwrap() ^= 1;
        }
        if case == 2 {
            packet[12..44].copy_from_slice(
                &secret.sign(b"response", &[&[3; 32], &0u64.to_be_bytes(), b"payload"]),
            );
        }
        b.write_all(&packet).await.unwrap();
        if case == 3 {
            assert_eq!(read.read(end()).await.unwrap(), b"payload");
            b.write_all(&packet).await.unwrap();
        }
        assert!(read.read(end()).await.is_err());
        assert!(write.write(b"reply", end()).await.is_err());
    }
}
#[tokio::test]
async fn oversized_header_is_rejected_without_waiting_for_body() {
    let (a, mut b) = duplex(64);
    let secret = Secret::fresh().unwrap();
    let (mut r, mut w) = Channel::new(a, secret, [0; 32], true, end()).split();
    b.write_all(&((REQUEST_BYTES + 1) as u32).to_be_bytes())
        .await
        .unwrap();
    assert!(r.read(end()).await.is_err());
    assert!(w.write(b"x", end()).await.is_err());
}
#[tokio::test]
async fn bounded_maximum_frames_cross_small_buffers_without_deadlock() {
    let (host, client) = pair().await;
    let (mut hr, mut hw) = host.split();
    let (mut cr, mut cw) = client.split();
    let request = vec![b'x'; REQUEST_BYTES];
    let reply = vec![b'y'; RESPONSE_BYTES];
    let (sent, got) = tokio::join!(cw.write(&request, end()), hr.read(end()));
    sent.unwrap();
    assert_eq!(got.unwrap(), request);
    let (sent, got) = tokio::join!(hw.write(&reply, end()), cr.read(end()));
    sent.unwrap();
    assert_eq!(got.unwrap(), reply);
    assert!(cw.write(&vec![0; REQUEST_BYTES + 1], end()).await.is_err());
    assert!(cr.read(end()).await.is_err());
}
#[tokio::test]
async fn cancelling_partial_read_poisoned_both_halves_instead_of_reparsing_suffix() {
    let (a, mut b) = duplex(64);
    let secret = Secret::fresh().unwrap();
    let (mut r, mut w) = Channel::new(a, secret, [0; 32], true, end()).split();
    b.write_all(&[0]).await.unwrap();
    let mut read = Box::pin(r.read(end()));
    assert!(futures_util::poll!(&mut read).is_pending());
    drop(read);
    assert!(r.read(end()).await.is_err());
    assert!(w.write(b"reply", end()).await.is_err());
}
#[tokio::test]
async fn cancelling_backpressured_write_revokes_read_and_prevents_retry() {
    let (a, _b) = duplex(1);
    let secret = Secret::fresh().unwrap();
    let (mut r, mut w) = Channel::new(a, secret, [0; 32], true, end()).split();
    let mut send = Box::pin(w.write(b"reply", end()));
    assert!(futures_util::poll!(&mut send).is_pending());
    drop(send);
    assert!(w.write(b"reply", end()).await.is_err());
    assert!(r.read(end()).await.is_err());
}
#[tokio::test(start_paused = true)]
async fn handshake_and_partial_frame_and_lifetime_deadlines_are_absolute() {
    let (a, _b) = duplex(64);
    assert!(
        authenticate(a, Secret::fresh().unwrap(), [0; 32], end())
            .await
            .is_err()
    );
    let (a, mut b) = duplex(64);
    let expires = Instant::now() + Duration::from_secs(1);
    let (mut r, mut w) = Channel::new(a, Secret::fresh().unwrap(), [0; 32], true, expires).split();
    b.write_all(&[0]).await.unwrap();
    assert!(
        r.read(Instant::now() + Duration::from_secs(100))
            .await
            .is_err()
    );
    assert!(Instant::now() <= expires + Duration::from_millis(1));
    assert!(w.write(b"x", end()).await.is_err());
}
#[tokio::test]
async fn dropping_a_half_and_sequence_exhaustion_revoke_without_more_bytes() {
    let (host, client) = pair().await;
    let (hr, mut hw) = host.split();
    drop(hr);
    assert!(hw.write(b"x", end()).await.is_err());
    let (mut cr, mut cw) = client.split();
    cw.direction.next = u64::MAX;
    assert!(cw.write(b"x", end()).await.is_err());
    assert!(cr.read(end()).await.is_err());
}
