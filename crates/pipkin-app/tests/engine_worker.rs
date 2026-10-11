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
