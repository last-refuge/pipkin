#[cfg(unix)]
use super::super::permission_wire::{self as wire, MAX_FRAME_BYTES};
use super::super::permission_wire::{MAX_STDIN_BYTES, ReplyFrame, RequestFrame};
use super::*;

fn binding() -> Binding {
    Binding::from_hex(&"ab".repeat(32)).unwrap()
}

fn request() -> RequestFrame {
    RequestFrame::new(binding(), " {\"a\":1,\"a\":2, \"text\":\"秘密\"}\n".into()).unwrap()
}

#[test]
fn closed_arguments_and_first_argument_only() {
    fn parse(values: &[&str]) -> Option<Result<PathBuf, WireError>> {
        parse_args(values.iter().map(OsString::from))
    }
    assert!(parse(&[]).is_none());
    assert!(parse(&["normal", HOOK_FLAG, "--socket", "/private/socket"]).is_none());
    let mut first = true;
    assert!(
        parse_args(std::iter::from_fn(|| {
            assert!(first, "normal mode consumed later arguments");
            first = false;
            Some("normal".into())
        }))
        .is_none()
    );
    assert_eq!(
        parse(&[HOOK_FLAG, "--socket", "/private/socket"])
            .unwrap()
            .unwrap(),
        PathBuf::from("/private/socket")
    );
    for values in [
        vec![HOOK_FLAG],
        vec![HOOK_FLAG, "--socket"],
        vec![HOOK_FLAG, "--unknown", "/private/socket"],
        vec![HOOK_FLAG, "--socket", "relative"],
        vec![HOOK_FLAG, "--socket", "/private/../socket"],
        vec![HOOK_FLAG, "--socket", "/private/./socket"],
        vec![HOOK_FLAG, "--socket", "/private//socket"],
        vec![HOOK_FLAG, "--socket", "/private/socket", "extra"],
        vec![
            HOOK_FLAG,
            "--socket",
            "/private/socket",
            "--socket",
            "/other",
        ],
    ] {
        assert!(parse(&values).unwrap().is_err());
    }
}

#[test]
fn environment_constructor_and_redacted_debug() {
    let launch = HelperLaunch::new(Path::new("/private/sensitive-path"), binding()).unwrap();
    assert_eq!(
        launch.arguments(),
        [HOOK_FLAG, "--socket", "/private/sensitive-path"].map(OsString::from)
    );
    assert_eq!(launch.environment(), (NONCE_ENV, binding().to_hex().into()));
    let request = request();
    let reply = ReplyFrame::for_request(&request, Decision::AllowOnce).unwrap();
    assert_eq!(format!("{launch:?}"), "HelperLaunch([redacted])");
    assert_eq!(format!("{:?}", binding()), "Binding([redacted])");
    assert_eq!(format!("{request:?}"), "RequestFrame([redacted])");
    assert_eq!(format!("{reply:?}"), "ReplyFrame([redacted])");
    assert_eq!(WireError.to_string(), "permission IPC rejected");
    assert!(Binding::from_hex("secret").is_err());
    assert!(Binding::from_hex(&"é".repeat(32)).is_err());
    assert!(Binding::from_hex(&"0".repeat(63)).is_err());
    assert!(Binding::from_hex(&"g".repeat(64)).is_err());
    assert_ne!(request.invocation, request.run_nonce);
    assert_ne!(request.invocation, self::request().invocation);
}

#[test]
fn only_fixed_historically_observed_json_shape() {
    for (decision, expected) in [(Decision::AllowOnce, "allow"), (Decision::Deny, "deny")] {
        let value: serde_json::Value = serde_json::from_slice(decision_json(decision)).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 1);
        let hook = &value["hookSpecificOutput"];
        assert_eq!(hook["hookEventName"], "PermissionRequest");
        assert_eq!(hook["decision"]["behavior"], expected);
        assert!(hook["decision"].get("updatedInput").is_none());
        assert!(hook["decision"].get("permissions").is_none());
        assert!(decision_json(decision).ends_with(b"\n"));
    }
}

#[test]
fn all_bindings_are_checked_and_payload_is_exact() {
    let request = request();
    let reply = ReplyFrame::for_request(&request, Decision::AllowOnce).unwrap();
    assert_eq!(request.decision_from(&reply).unwrap(), Decision::AllowOnce);
    for field in 0..4 {
        let mut bad = reply.clone();
        match field {
            0 => bad.version += 1,
            1 => bad.run_nonce = Binding::fresh().unwrap(),
            2 => bad.invocation = Binding::fresh().unwrap(),
            _ => bad.payload_sha256 = Binding::fresh().unwrap(),
        }
        assert!(request.decision_from(&bad).is_err());
    }
    let mut modified = request.clone();
    modified.payload.push(' ');
    assert!(modified.validate().is_err());
    assert!(RequestFrame::new(binding(), "x".repeat(MAX_STDIN_BYTES + 1)).is_err());
    assert!(RequestFrame::new(binding(), "x".repeat(MAX_STDIN_BYTES)).is_ok());
    assert_eq!(
        Binding::payload_digest(b"abc").to_hex(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[cfg(unix)]
mod local {
    use super::*;
    use std::{
        io::Write,
        os::{
            fd::AsRawFd,
            unix::{fs::PermissionsExt, net::UnixStream as StdUnixStream},
        },
        time::Instant as StdInstant,
    };
    use tokio::{io::AsyncWriteExt, net::UnixListener, time::Instant};

    struct Endpoint {
        _directory: tempfile::TempDir,
        path: PathBuf,
        listener: UnixListener,
    }

    impl Endpoint {
        fn new() -> Self {
            // Short private path under home, never /tmp (including default test runs).
            let home = std::env::var_os("HOME").expect("tests require a trusted home directory");
            let directory = tempfile::Builder::new()
                .prefix("pwh")
                .tempdir_in(home)
                .unwrap();
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let path = directory.path().join("s");
            let listener = UnixListener::bind(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            Self {
                _directory: directory,
                path,
                listener,
            }
        }
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(2)
    }

    #[tokio::test]
    async fn fake_server_roundtrips_exact_bytes_allow_and_deny() {
        for decision in [Decision::AllowOnce, Decision::Deny] {
            let endpoint = Endpoint::new();
            let request = request();
            let expected = request.clone();
            let server = async {
                let (mut stream, _) = endpoint.listener.accept().await.unwrap();
                let received = wire::read_request(&mut stream, deadline()).await.unwrap();
                assert_eq!(received.payload.as_bytes(), expected.payload.as_bytes());
                assert_eq!(received.invocation, expected.invocation);
                let reply = ReplyFrame::for_request(&received, decision).unwrap();
                wire::write_reply(&mut stream, &reply, deadline())
                    .await
                    .unwrap();
            };
            let (result, ()) = tokio::join!(
                unix::exchange(&endpoint.path, &request, Duration::from_secs(1)),
                server
            );
            assert_eq!(result.unwrap(), decision);
        }
    }

    async fn raw_response(response: Vec<u8>) -> Result<Decision, WireError> {
        let endpoint = Endpoint::new();
        let request = request();
        let server = async {
            let (mut stream, _) = endpoint.listener.accept().await.unwrap();
            wire::read_request(&mut stream, deadline()).await.unwrap();
            let _ = stream.write_all(&response).await;
        };
        let (result, ()) = tokio::join!(
            unix::exchange(&endpoint.path, &request, Duration::from_millis(100)),
            server
        );
        result
    }

    fn frame(bytes: &[u8]) -> Vec<u8> {
        let mut framed = (bytes.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(bytes);
        framed
    }

    #[tokio::test]
    async fn disconnect_truncation_oversize_and_malformed_deny() {
        for bytes in [
            vec![],
            vec![0, 0],
            10u32.to_be_bytes().to_vec(),
            [10u32.to_be_bytes().as_slice(), b"{}"].concat(),
            ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec(),
            0u32.to_be_bytes().to_vec(),
            frame(b"not-json"),
            frame(b"{}"),
            frame(b"[]"),
            frame(b"\xff"),
        ] {
            assert_eq!(
                raw_response(bytes).await.unwrap_or(Decision::Deny),
                Decision::Deny
            );
        }
    }

    #[tokio::test]
    async fn mismatched_unknown_duplicate_or_extended_reply_denies() {
        for mutation in 0..12 {
            let endpoint = Endpoint::new();
            let request = request();
            let server = async {
                let (mut stream, _) = endpoint.listener.accept().await.unwrap();
                let received = wire::read_request(&mut stream, deadline()).await.unwrap();
                let reply = ReplyFrame::for_request(&received, Decision::AllowOnce).unwrap();
                let mut value = serde_json::to_value(&reply).unwrap();
                match mutation {
                    0 => value["version"] = 2.into(),
                    1 => value["run_nonce"] = binding().to_hex().replace('a', "0").into(),
                    2 => value["invocation"] = binding().to_hex().into(),
                    3 => value["payload_sha256"] = binding().to_hex().into(),
                    4 => value["decision"] = "AllowAlways".into(),
                    5 => value["updatedInput"] = serde_json::json!({}),
                    6 => value["permissions"] = serde_json::json!([]),
                    7 => {
                        value.as_object_mut().unwrap().remove("invocation");
                    }
                    8 => value["decision"] = serde_json::json!({"AllowOnce":null}),
                    11 => {
                        value = serde_json::json!([
                            reply.version,
                            reply.run_nonce,
                            reply.invocation,
                            reply.payload_sha256,
                            reply.decision,
                        ])
                    }
                    _ => {}
                }
                let mut bytes = serde_json::to_vec(&value).unwrap();
                if mutation == 9 {
                    bytes.pop();
                    bytes.extend_from_slice(b",\"decision\":\"AllowOnce\"}");
                }
                if mutation == 10 {
                    bytes.extend_from_slice(b" {}");
                }
                stream.write_all(&frame(&bytes)).await.unwrap();
            };
            let (result, ()) = tokio::join!(
                unix::exchange(&endpoint.path, &request, Duration::from_secs(1)),
                server
            );
            assert!(result.is_err(), "accepted invalid reply variant {mutation}");
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn saturated_connect_is_nonblocking_and_bounded() {
        let endpoint = Endpoint::new();
        // On Linux backlog zero allows one queued connection. Keep it occupied,
        // so a blocking connect would hang; nonblocking EAGAIN must fail closed.
        assert_eq!(unsafe { libc::listen(endpoint.listener.as_raw_fd(), 0) }, 0);
        let _queued = tokio::time::timeout(
            Duration::from_secs(1),
            tokio::net::UnixStream::connect(&endpoint.path),
        )
        .await
        .unwrap()
        .unwrap();
        let request = request();
        let start = StdInstant::now();
        assert!(
            unix::exchange(&endpoint.path, &request, Duration::from_millis(20))
                .await
                .is_err()
        );
        assert!(start.elapsed() < CONNECT_WRITE_TIMEOUT + Duration::from_millis(200));
    }

    #[test]
    fn metadata_deadline_before_and_after_connect_has_no_late_authorization() {
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        };

        for blocked_check in [0, 1] {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let endpoint = {
                let _guard = runtime.enter();
                Endpoint::new()
            };
            let calls = Arc::new(AtomicUsize::new(0));
            let (release, gate) = mpsc::sync_channel(1);
            let gate = Arc::new(Mutex::new(gate));
            let (finished, completion) = mpsc::sync_channel(1);
            let check = {
                let calls = calls.clone();
                move |path: &Path| {
                    if calls.fetch_add(1, Ordering::SeqCst) == blocked_check {
                        // Model symlink_metadata stuck until AFTER runtime drop.
                        gate.lock().unwrap().recv().unwrap();
                        let result = unix::private_endpoint(path);
                        finished.send(result.is_ok()).unwrap();
                        result
                    } else {
                        unix::private_endpoint(path)
                    }
                }
            };
            let start = StdInstant::now();
            let result = runtime.block_on(unix::exchange_checked(
                &endpoint.path,
                &request(),
                Duration::from_secs(1),
                Instant::now() + Duration::from_millis(100),
                check,
            ));
            assert!(result.is_err());
            assert_eq!(calls.load(Ordering::SeqCst), blocked_check + 1);
            let listener = endpoint.listener.into_std().unwrap();
            // Must return while metadata worker is still blocked, unlike a Tokio
            // blocking-pool job. No runtime remains to consume the late success.
            drop(runtime);
            assert!(start.elapsed() < Duration::from_secs(1));
            release.send(()).unwrap();
            assert!(completion.recv_timeout(Duration::from_secs(1)).unwrap());
            if blocked_check == 1 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut byte = [0];
                assert_eq!(std::io::Read::read(&mut stream, &mut byte).unwrap(), 0);
            }
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock,
                "late metadata result connected or reconnected"
            );
            assert_eq!(calls.load(Ordering::SeqCst), blocked_check + 1);
        }
    }

    #[tokio::test]
    async fn expired_connect_budget_never_starts_metadata_or_connects() {
        let endpoint = Endpoint::new();
        assert!(
            unix::exchange_checked(
                &endpoint.path,
                &request(),
                Duration::from_secs(1),
                Instant::now(),
                |_| panic!("expired budget started metadata worker"),
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn disconnect_never_reconnects() {
        let endpoint = Endpoint::new();
        let request = request();
        let server = async {
            let (mut stream, _) = endpoint.listener.accept().await.unwrap();
            wire::read_request(&mut stream, deadline()).await.unwrap();
            drop(stream);
            assert!(
                tokio::time::timeout(Duration::from_millis(30), endpoint.listener.accept(),)
                    .await
                    .is_err()
            );
        };
        let (result, ()) = tokio::join!(
            unix::exchange(&endpoint.path, &request, Duration::from_millis(100)),
            server,
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn decision_timeout_is_total_not_per_read() {
        let endpoint = Endpoint::new();
        let request = request();
        let server = async {
            let (mut stream, _) = endpoint.listener.accept().await.unwrap();
            wire::read_request(&mut stream, deadline()).await.unwrap();
            stream.write_all(&100u32.to_be_bytes()).await.unwrap();
            for _ in 0..10 {
                if stream.write_all(b" ").await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        let start = StdInstant::now();
        let (result, ()) = tokio::join!(
            unix::exchange(&endpoint.path, &request, Duration::from_millis(60)),
            server
        );
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn frame_bounds_and_request_validation() {
        let request = request();
        for mutation in 0..5 {
            let mut value = serde_json::to_value(&request).unwrap();
            match mutation {
                0 => value["payload"] = "different bytes".into(),
                1 => value["version"] = 0.into(),
                2 => value["extra"] = true.into(),
                3 => value["payload"] = "x".repeat(MAX_STDIN_BYTES + 1).into(),
                _ => {
                    value = serde_json::json!([
                        1,
                        request.run_nonce,
                        request.invocation,
                        request.payload_sha256,
                        request.payload
                    ])
                }
            }
            let bytes = frame(&serde_json::to_vec(&value).unwrap());
            assert!(
                wire::read_request(&mut bytes.as_slice(), deadline())
                    .await
                    .is_err()
            );
        }
        // Worst-case JSON escaping still fits the wire bound.
        let maximum = RequestFrame::new(binding(), "\0".repeat(MAX_STDIN_BYTES)).unwrap();
        let mut bytes = Vec::new();
        wire::write_request(&mut bytes, &maximum, deadline())
            .await
            .unwrap();
        assert!(bytes.len() <= MAX_FRAME_BYTES + 4);
        assert_eq!(
            wire::read_request(&mut bytes.as_slice(), deadline())
                .await
                .unwrap()
                .payload,
            maximum.payload
        );
        // Write backpressure uses the total deadline too.
        let (mut writer, _reader) = tokio::io::duplex(1);
        assert!(
            wire::write_request(
                &mut writer,
                &request,
                Instant::now() + Duration::from_millis(20)
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn private_endpoint_rejects_permissions_symlinks_and_non_sockets() {
        let endpoint = Endpoint::new();
        assert!(unix::private_endpoint(&endpoint.path).is_ok());
        std::fs::set_permissions(&endpoint.path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(unix::private_endpoint(&endpoint.path).is_err());
        std::fs::set_permissions(&endpoint.path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let parent = endpoint.path.parent().unwrap();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(unix::private_endpoint(&endpoint.path).is_err());
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = parent.join("link");
        std::os::unix::fs::symlink(&endpoint.path, &link).unwrap();
        assert!(unix::private_endpoint(&link).is_err());
        let directory_link = parent.join("dirlink");
        std::os::unix::fs::symlink(parent, &directory_link).unwrap();
        assert!(unix::private_endpoint(&directory_link.join("s")).is_err());
        let file = parent.join("file");
        std::fs::write(&file, b"").unwrap();
        assert!(unix::private_endpoint(&file).is_err());
        assert!(unix::private_endpoint(&parent.join("absent")).is_err());
    }

    #[test]
    fn stdout_classification_timeout_cannot_write_after_late_metadata() {
        let (reader, writer) = StdUnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        let (finished, done) = std::sync::mpsc::sync_channel(1);
        let start = StdInstant::now();
        assert!(
            unix::output_before(
                writer.as_raw_fd(),
                start + Duration::from_millis(30),
                move |_| {
                    // Simulate even fstat being blocked by a filesystem-backed descriptor.
                    let _ = wait.recv_timeout(Duration::from_secs(2));
                    let _ = finished.send(());
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut byte = [0];
        assert_eq!(
            std::io::Read::read(&mut &reader, &mut byte)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn stdout_rejects_regular_files_without_bytes_or_flag_changes() {
        use std::io::{Read, Seek};
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"unchanged").unwrap();
        file.rewind().unwrap();
        let fd = file.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        for decision in [Decision::AllowOnce, Decision::Deny] {
            assert!(
                unix::write_fd(
                    fd,
                    decision_json(decision),
                    StdInstant::now() + Duration::from_secs(1),
                )
                .is_err()
            );
        }
        assert_eq!(file.stream_position().unwrap(), 0);
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFL) }, flags);
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"unchanged");
    }

    #[test]
    fn stdout_rejects_devices_without_bytes() {
        use std::os::fd::{FromRawFd, OwnedFd};
        let null = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/null")
            .unwrap();
        assert!(
            unix::write_fd(
                null.as_raw_fd(),
                ALLOW_JSON,
                StdInstant::now() + Duration::from_secs(1),
            )
            .is_err()
        );
        // A PTY lets us observe that rejection wrote no bytes to the device.
        let (mut master, mut slave) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        let slave = unsafe { OwnedFd::from_raw_fd(slave) };
        for decision in [Decision::AllowOnce, Decision::Deny] {
            assert!(
                unix::write_fd(
                    slave.as_raw_fd(),
                    decision_json(decision),
                    StdInstant::now() + Duration::from_secs(1),
                )
                .is_err()
            );
        }
        let mut poll = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 0) }, 0);
    }

    #[test]
    fn stdout_pipe_and_socket_write_exact_json_and_restore_flags() {
        use std::io::Read;
        use std::os::fd::{FromRawFd, OwnedFd};
        let mut pipe = [-1; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        let pipe_reader = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
        let pipe_writer = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
        let (socket_reader, socket_writer) = StdUnixStream::pair().unwrap();
        for (reader, writer) in [
            (pipe_reader, pipe_writer),
            (socket_reader.into(), socket_writer.into()),
        ] {
            let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
            unix::write_fd(
                writer.as_raw_fd(),
                ALLOW_JSON,
                StdInstant::now() + Duration::from_secs(1),
            )
            .unwrap();
            assert_eq!(
                unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) },
                flags
            );
            drop(writer);
            let mut bytes = Vec::new();
            std::fs::File::from(reader).read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, ALLOW_JSON);
        }
    }

    #[test]
    fn stdout_backpressure_and_expired_budget_fail_bounded() {
        let (reader, writer) = StdUnixStream::pair().unwrap();
        let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
        assert!(unix::write_fd(writer.as_raw_fd(), ALLOW_JSON, StdInstant::now()).is_err());
        reader.set_nonblocking(true).unwrap();
        let mut byte = [0];
        assert_eq!(
            std::io::Read::read(&mut &reader, &mut byte)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        let start = StdInstant::now();
        assert!(
            unix::write_fd(
                writer.as_raw_fd(),
                &vec![b'x'; 4 * 1024 * 1024],
                start + Duration::from_millis(30),
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(
            unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) },
            flags
        );
    }

    #[test]
    fn stdin_preserves_bytes_rejects_invalid_utf8_and_bounds() {
        for bytes in [
            b" {\"a\": 1}\n".to_vec(),
            vec![b'x'; MAX_STDIN_BYTES],
            vec![b'x'; MAX_STDIN_BYTES + 1],
            vec![0xff],
        ] {
            let (reader, mut writer) = StdUnixStream::pair().unwrap();
            let expected = bytes.clone();
            let thread = std::thread::spawn(move || {
                let _ = writer.write_all(&bytes);
            });
            let result = unix::read_fd(
                reader.as_raw_fd(),
                StdInstant::now() + Duration::from_secs(1),
            );
            thread.join().unwrap();
            if expected.len() > MAX_STDIN_BYTES || std::str::from_utf8(&expected).is_err() {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap().as_bytes(), expected);
            }
        }
    }

    #[test]
    fn trickling_stdin_does_not_restart_deadline() {
        let (reader, mut writer) = StdUnixStream::pair().unwrap();
        let thread = std::thread::spawn(move || {
            for _ in 0..10 {
                if writer.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let start = StdInstant::now();
        assert!(unix::read_fd(reader.as_raw_fd(), start + Duration::from_millis(40)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        drop(reader);
        thread.join().unwrap();
    }

    #[test]
    fn idle_stdin_has_a_deadline_and_restores_flags() {
        let (reader, _writer) = StdUnixStream::pair().unwrap();
        let fd = reader.as_raw_fd();
        let original = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        let start = StdInstant::now();
        assert!(unix::read_fd(fd, start + Duration::from_millis(30)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFL) }, original);
    }
}
