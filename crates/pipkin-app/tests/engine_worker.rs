//! The real `pipkin --engine-worker` mode: the built binary serves the engine over its private
//! stdin/stdout frames, before (and without) any GPUI initialization, and exits when its input
//! closes.

use std::io::{BufReader, BufWriter};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pipkin_engine_protocol::frame;
use pipkin_engine_protocol::{Call, ClientFrame, PROTOCOL_VERSION, Reply, ServerFrame};

fn reply(read: &mut impl std::io::Read, id: u64) -> Reply {
    loop {
        match frame::read::<ServerFrame>(read).expect("a frame") {
            ServerFrame::Reply { id: got, result } if got == id => return result.expect("ok"),
            _ => {}
        }
    }
}

#[test]
fn the_worker_answers_over_its_pipes_and_exits_when_its_input_closes() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pipkin"))
        .args(["--engine-worker", "--engine-data-dir"])
        .arg(dir.path().join("engine"))
        .arg("--engine-config-dir")
        .arg(dir.path().join("engine-config"))
        // No display: proves the worker never touches GPUI.
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the worker starts");
    let mut input = BufWriter::new(child.stdin.take().unwrap());
    let mut output = BufReader::new(child.stdout.take().unwrap());
    frame::write(
        &mut input,
        &ClientFrame {
            id: 1,
            lane: Default::default(),
            call: Call::Hello {
                protocol: PROTOCOL_VERSION,
                client: "test".into(),
            },
        },
    )
    .unwrap();
    assert!(matches!(reply(&mut output, 1), Reply::Welcome(w) if w.protocol == PROTOCOL_VERSION));
    frame::write(
        &mut input,
        &ClientFrame {
            id: 2,
            lane: Default::default(),
            call: Call::ListSessions,
        },
    )
    .unwrap();
    assert_eq!(reply(&mut output, 2), Reply::Sessions { sessions: vec![] });
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the worker did not exit after its input closed");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
    assert!(
        dir.path().join("engine").is_dir(),
        "the engine kept its data where it was told"
    );
}

#[test]
fn a_worker_without_its_directories_refuses_to_start() {
    let output = Command::new(env!("CARGO_BIN_EXE_pipkin"))
        .args(["--engine-worker", "--engine-data-dir", "/tmp/x"])
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--engine-config-dir"));
}

#[cfg(unix)]
mod external {
    use super::*;
    use pipkin_engine_protocol::{ErrorCode, ProtoError};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};

    struct ServerProcess(std::process::Child);
    impl Drop for ServerProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn start(root: &Path) -> ServerProcess {
        ServerProcess(
            Command::new(env!("CARGO_BIN_EXE_pipkin"))
                .arg("--engine-server")
                .arg(root.join("endpoint"))
                .arg("--engine-data-dir")
                .arg(root.join("data"))
                .arg("--engine-config-dir")
                .arg(root.join("config"))
                .env_clear()
                .env("PATH", std::env::var("PATH").unwrap_or_default())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }
    fn call(stream: &mut UnixStream, call: Call) -> Result<Reply, ProtoError> {
        frame::write(
            stream,
            &ClientFrame {
                id: 7,
                lane: call.lane(),
                call,
            },
        )
        .unwrap();
        loop {
            if let ServerFrame::Reply { id: 7, result } = frame::read(stream).unwrap() {
                return result;
            }
        }
    }
    fn connect(root: &Path) -> (UnixStream, u64) {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut stream = loop {
            match pipkin_engine::endpoint::connect(&root.join("endpoint")) {
                Ok(stream) => break stream,
                Err(error) => {
                    assert!(Instant::now() < deadline, "endpoint: {error}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let epoch = match call(
            &mut stream,
            Call::Hello {
                protocol: PROTOCOL_VERSION,
                client: "process-test".into(),
            },
        )
        .unwrap()
        {
            Reply::Welcome(w) => w.epoch,
            other => panic!("{other:?}"),
        };
        (stream, epoch)
    }
    #[test]
    fn foreground_server_is_external_to_clients_and_recovers_its_stale_socket_after_sigkill() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut first = start(root);
        let (mut a, epoch) = connect(root);
        let (mut b, _) = connect(root);
        let session = match call(
            &mut a,
            Call::CreateSession {
                cwd: root.display().to_string(),
                request: "create".into(),
            },
        )
        .unwrap()
        {
            Reply::Session { summary } => summary.id,
            other => panic!("{other:?}"),
        };
        drop(a);
        assert_eq!(
            call(&mut b, Call::Shutdown).unwrap_err().code,
            ErrorCode::Invalid
        );
        assert!(first.0.try_wait().unwrap().is_none());
        let result = Command::new(env!("CARGO_BIN_EXE_pipkin"))
            .args(["--engine", "native", "--engine-attach"])
            .arg(root.join("endpoint"))
            .args(["--diagnose", "--probe"])
            .env_clear()
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("Probe: OK (connected; 1 sessions)")
        );
        assert!(
            first.0.try_wait().unwrap().is_none(),
            "diagnosis cannot stop the external server"
        );
        first.0.kill().unwrap();
        first.0.wait().unwrap();
        assert!(
            root.join("endpoint/engine.sock").exists(),
            "SIGKILL leaves a stale socket"
        );
        let mut second = start(root);
        let (mut c, next_epoch) = connect(root);
        assert!(next_epoch > epoch);
        assert!(
            matches!(call(&mut c,Call::ListSessions).unwrap(),Reply::Sessions{sessions} if sessions.iter().any(|s|s.id==session))
        );
        // SAFETY: this is the live child handle we just spawned, not a discovered process.
        assert_eq!(
            unsafe { libc::kill(second.0.id() as i32, libc::SIGTERM) },
            0
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = second.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "owner SIGTERM must stop the server"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success(), "{status}");
        assert!(!root.join("endpoint/engine.sock").exists());
        assert!(PathBuf::from(root).join("data").exists());
    }
}
