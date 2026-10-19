use super::*;
use tokio::io::{AsyncWriteExt, BufReader, duplex};

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn stalled_stdio_output_is_bounded_and_never_replayed() {
    use super::super::orchestration_wire as wire;
    use tokio::time::Instant;
    let deadline = Instant::now() + Duration::from_secs(60);
    let (a, b) = duplex(1024);
    let secret = wire::Secret::fresh().unwrap();
    let (host, helper) = tokio::join!(
        async {
            wire::authenticate(a, secret.clone(), [0; 32], deadline)
                .await
                .unwrap()
                .acknowledge(deadline)
                .await
                .unwrap()
        },
        wire::connect(b, secret.clone(), [0; 32], deadline)
    );
    let (mut host_read, mut host_write) = host.split();
    let (reader, writer) = helper.unwrap().split();
    let (input, _keep_input_open) = duplex(32);
    let (output, _do_not_drain) = duplex(1);
    let task = tokio::spawn(relay(
        input,
        output,
        reader,
        writer,
        deadline,
        std::future::pending(),
    ));
    host_write
        .write(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}", deadline)
        .await
        .unwrap();
    assert!(task.await.unwrap().is_err());
    assert!(Instant::now() < deadline);
    assert!(host_read.read(deadline).await.is_err());
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn idle_helper_stops_at_lifetime_and_rejects_multi_line_host_output() {
    use super::super::orchestration_wire as wire;
    use tokio::time::Instant;
    for invalid in [false, true] {
        let deadline = Instant::now() + Duration::from_secs(5);
        let (a, b) = duplex(1024);
        let secret = wire::Secret::fresh().unwrap();
        let (host, helper) = tokio::join!(
            async {
                wire::authenticate(a, secret.clone(), [0; 32], deadline)
                    .await
                    .unwrap()
                    .acknowledge(deadline)
                    .await
                    .unwrap()
            },
            wire::connect(b, secret.clone(), [0; 32], deadline)
        );
        let (_keep_host_reader, mut host_write) = host.split();
        let (reader, writer) = helper.unwrap().split();
        let (input, _keep_input_open) = duplex(32);
        let (output, _keep_output_open) = duplex(64);
        let task = tokio::spawn(relay(
            input,
            output,
            reader,
            writer,
            deadline,
            std::future::pending(),
        ));
        if invalid {
            host_write
                .write(b"{\"jsonrpc\":\"2.0\"}\n{}", deadline)
                .await
                .unwrap();
        }
        assert!(task.await.unwrap().is_err());
        if invalid {
            assert!(Instant::now() < deadline);
        } else {
            assert!(Instant::now() >= deadline);
        }
    }
}
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn private_close_control_requires_exact_host_messages_and_a_bounded_final_commit() {
    use super::super::{orchestration_control::Control, orchestration_wire as wire};
    use tokio::{io::AsyncReadExt, time::Instant};
    for case in 0..4 {
        let deadline = Instant::now() + Duration::from_secs(60);
        let (a, b) = duplex(1024);
        let secret = wire::Secret::fresh().unwrap();
        let (host, helper) = tokio::join!(
            async {
                wire::authenticate(a, secret.clone(), [0; 32], deadline)
                    .await
                    .unwrap()
                    .acknowledge(deadline)
                    .await
                    .unwrap()
            },
            wire::connect(b, secret.clone(), [0; 32], deadline)
        );
        let (mut host_read, mut host_write) = host.split();
        let (reader, writer) = helper.unwrap().split();
        let (input, _keep_input) = duplex(32);
        let (output, mut observed) = duplex(128);
        let task = tokio::spawn(relay(
            input,
            output,
            reader,
            writer,
            deadline,
            std::future::pending(),
        ));
        let mut request = Control::Quiesce.value();
        if case == 3 {
            request["extra"] = serde_json::json!(true);
        }
        host_write
            .write(&serde_json::to_vec(&request).unwrap(), deadline)
            .await
            .unwrap();
        if case != 3 {
            let ack: serde_json::Value =
                serde_json::from_slice(&host_read.read(deadline).await.unwrap()).unwrap();
            assert!(Control::Quiesced.matches(&ack));
            if case < 2 {
                let final_frame = if case == 0 {
                    Control::Closed.value()
                } else {
                    Control::Quiesced.value()
                };
                host_write
                    .write(&serde_json::to_vec(&final_frame).unwrap(), deadline)
                    .await
                    .unwrap();
            }
        }
        assert_eq!(task.await.unwrap().is_ok(), case == 0);
        let mut bytes = Vec::new();
        observed.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
        assert!(Instant::now() < deadline);
    }
}
#[test]
fn mode_is_exact_and_ordinary_args_are_not_helper_configuration() {
    for args in [vec![], vec!["--help"], vec!["--engine-worker", FLAG]] {
        assert_eq!(requested(args.into_iter().map(OsString::from)), None);
    }
    assert_eq!(
        requested([FLAG].into_iter().map(OsString::from)),
        Some(true)
    );
    assert_eq!(
        requested([FLAG, "extra"].into_iter().map(OsString::from)),
        Some(false)
    );
}
#[tokio::test]
async fn newline_framing_preserves_coalesced_messages_and_rejects_partial_or_empty() {
    let mut reader = BufReader::new(b"{\"id\":1}\n{\"id\":2}\n".as_slice());
    assert_eq!(line(&mut reader).await.unwrap().unwrap(), b"{\"id\":1}");
    assert_eq!(line(&mut reader).await.unwrap().unwrap(), b"{\"id\":2}");
    assert!(line(&mut reader).await.unwrap().is_none());
    for bad in [b"unfinished".as_slice(), b"\n"] {
        assert!(line(&mut BufReader::new(bad)).await.is_err());
    }
}
#[tokio::test]
async fn huge_unterminated_line_is_bounded_before_eof() {
    let (mut send, receive) = duplex(32);
    let size = super::super::orchestration_wire::REQUEST_BYTES;
    let writer = tokio::spawn(async move {
        let _ = send.write_all(&vec![b'x'; size + 1]).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
    });
    assert!(line(&mut BufReader::new(receive)).await.is_err());
    writer.abort();
}
#[cfg(unix)]
#[tokio::test]
async fn stdio_regular_files_are_refused_and_socket_flags_restored() {
    use std::os::fd::AsRawFd;
    let file = tempfile::tempfile().unwrap();
    assert!(pipe::Pipe::new(file.as_raw_fd(), true).is_err());
    assert!(pipe::Pipe::new(file.as_raw_fd(), false).is_err());
    let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
    let before = unsafe { libc::fcntl(a.as_raw_fd(), libc::F_GETFL) };
    let owned = pipe::Pipe::new(a.as_raw_fd(), false).unwrap();
    assert_ne!(
        unsafe { libc::fcntl(a.as_raw_fd(), libc::F_GETFL) } & libc::O_NONBLOCK,
        0
    );
    drop(owned);
    assert_eq!(unsafe { libc::fcntl(a.as_raw_fd(), libc::F_GETFL) }, before);
}
