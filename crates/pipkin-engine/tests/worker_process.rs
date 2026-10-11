//! The headless worker over real pipes: handshake, durable admission, SIGKILL, restart and
//! lookup. No GUI, network or credentials.

use std::io::{BufReader, BufWriter};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use pipkin_engine_protocol::frame;
use pipkin_engine_protocol::*;

struct Worker {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Worker {
    fn start(data: &std::path::Path, config: &std::path::Path) -> Worker {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pipkin-engine"))
            .args(["--data-dir"])
            .arg(data)
            .arg("--config-dir")
            .arg(config)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn worker");
        let input = BufWriter::new(child.stdin.take().unwrap());
        let output = BufReader::new(child.stdout.take().unwrap());
        Worker { child, input, output, next: 0 }
    }

    fn call(&mut self, call: Call) -> Result<Reply, ProtoError> {
        self.next += 1;
        let id = self.next;
        frame::write(&mut self.input, &ClientFrame { id, lane: call.lane(), call }).unwrap();
        loop {
            match frame::read::<ServerFrame>(&mut self.output).expect("frame") {
                ServerFrame::Reply { id: got, result } if got == id => return result,
                _ => {}
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn admission_survives_sigkill_and_is_found_by_lookup_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (data, config, project) = (dir.path().join("d"), dir.path().join("c"), dir.path().join("p"));
    std::fs::create_dir_all(&project).unwrap();
    let mut w = Worker::start(&data, &config);
    let Ok(Reply::Welcome(welcome)) = w.call(Call::Hello { protocol: PROTOCOL_VERSION, client: "t".into() }) else {
        panic!()
    };
    assert_eq!(welcome.protocol, PROTOCOL_VERSION);
    let wrong = w.call(Call::Hello { protocol: PROTOCOL_VERSION + 1, client: "t".into() }).unwrap_err();
    assert_eq!(wrong.code, ErrorCode::Incompatible);
    let Ok(Reply::Session { summary }) = w.call(Call::CreateSession { cwd: project.display().to_string(), request: "c1".into() }) else {
        panic!()
    };
    // No model is configured in this isolated profile: the prompt is admitted and settles
    // NoModel — admission itself is what must survive.
    let Ok(Reply::Admitted { .. }) = w.call(Call::Prompt {
        session: summary.id.clone(),
        request: "k1".into(),
        text: "hello".into(),
        images: vec![],
        model: None,
    }) else {
        panic!()
    };
    // SIGKILL: no orderly shutdown.
    w.child.kill().unwrap();
    w.child.wait().unwrap();
    let mut w = Worker::start(&data, &config);
    w.call(Call::Hello { protocol: PROTOCOL_VERSION, client: "t".into() }).unwrap();
    let Ok(Reply::Lookup(LookupResult::Known { .. })) = w.call(Call::Lookup { session: summary.id.clone(), request: "k1".into() }) else {
        panic!("the admitted prompt is known after a kill")
    };
    let Ok(Reply::Sessions { sessions }) = w.call(Call::ListSessions) else { panic!() };
    assert_eq!(sessions.len(), 1);
    // Closing stdin is an orderly exit.
    drop(w.input);
    let status = w.child.wait().unwrap();
    assert!(status.success());
}

#[test]
fn a_second_worker_on_the_same_data_refuses_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let (data, config) = (dir.path().join("d"), dir.path().join("c"));
    let mut first = Worker::start(&data, &config);
    first.call(Call::Hello { protocol: PROTOCOL_VERSION, client: "t".into() }).unwrap();
    let second = Command::new(env!("CARGO_BIN_EXE_pipkin-engine"))
        .arg("--data-dir")
        .arg(&data)
        .arg("--config-dir")
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("another engine process owns"));
    drop(first.input);
    first.child.wait().unwrap();
}
