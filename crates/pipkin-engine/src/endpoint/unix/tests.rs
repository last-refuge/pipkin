use super::*;
use crate::config::{EngineConfig, Env, ExtensionSpec};
use crate::provider::scripted::{self, Gate, Reply as R, Scripted};
use crate::runtime::registry_with;
use pipkin_engine_protocol::*;
use std::os::unix::fs::symlink;
use std::time::Instant;

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    project: PathBuf,
    engine: Arc<EngineHandle>,
}
impl Fixture {
    fn new(provider: Scripted, questions: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("endpoint");
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let mut config = EngineConfig::new(dir.path().join("data"), dir.path().join("config"));
        config.env = Env::isolated(Default::default());
        config.extra_models = vec![scripted::model()];
        config.default_model = Some("scripted/scripted".into());
        if questions {
            config.extensions = vec![ExtensionSpec::Builtin("questions".into())];
        }
        let engine = Arc::new(
            EngineHandle::start_with(
                config,
                registry_with(Arc::new(provider)),
                crate::tools::builtin_tools(),
            )
            .unwrap(),
        );
        Self {
            _dir: dir,
            path,
            project,
            engine,
        }
    }
    fn server(&self) -> Server {
        Server::start(self.engine.clone(), &self.path).unwrap()
    }
    fn connect(&self) -> UnixStream {
        let mut stream = connect(&self.path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert!(matches!(
            call(
                &mut stream,
                Call::Hello {
                    protocol: PROTOCOL_VERSION,
                    client: "test".into()
                }
            ),
            Ok(Reply::Welcome(_))
        ));
        stream
    }
    fn create(&self, stream: &mut UnixStream) -> String {
        match call(
            stream,
            Call::CreateSession {
                cwd: self.project.display().to_string(),
                request: "create".into(),
            },
        )
        .unwrap()
        {
            Reply::Session { summary } => summary.id,
            other => panic!("{other:?}"),
        }
    }
}
fn call(stream: &mut UnixStream, call: Call) -> Result<Reply, ProtoError> {
    frame::write(
        stream,
        &ClientFrame {
            id: 10,
            lane: call.lane(),
            call,
        },
    )
    .unwrap();
    loop {
        if let ServerFrame::Reply { id: 10, result } = frame::read(stream).unwrap() {
            return result;
        }
    }
}
fn wait(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        thread::sleep(Duration::from_millis(5));
    }
}
fn prompt(id: &str, key: &str) -> Call {
    Call::Prompt {
        session: id.into(),
        request: key.into(),
        text: "work".into(),
        images: vec![],
        model: None,
    }
}

#[test]
fn endpoint_clients_share_work_and_disconnect_without_owning_shutdown() {
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, _| R::Gated(g.clone(), Box::new(R::Text("done".into()))));
    let fx = Fixture::new(provider.clone(), false);
    let server = fx.server();
    assert_eq!(fs::metadata(&fx.path).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(fx.path.join(SOCKET)).unwrap().mode() & 0o777,
        0o600
    );
    let mut a = fx.connect();
    let mut b = fx.connect();
    let id = fx.create(&mut a);
    call(
        &mut a,
        Call::OpenSession {
            session: id.clone(),
            window: None,
        },
    )
    .unwrap();
    call(
        &mut b,
        Call::OpenSession {
            session: id.clone(),
            window: None,
        },
    )
    .unwrap();
    call(&mut a, prompt(&id, "r1")).unwrap();
    wait("provider active", || !provider.requests().is_empty());
    drop(a);
    assert_eq!(
        call(&mut b, Call::Shutdown).unwrap_err().code,
        ErrorCode::Invalid
    );
    assert!(matches!(
        call(
            &mut b,
            Call::Lookup {
                session: id.clone(),
                request: "r1".into()
            }
        ),
        Ok(Reply::Lookup(LookupResult::Known { open: true, .. }))
    ));
    gate.open();
    wait("settled", || {
        matches!(
            call(
                &mut b,
                Call::Lookup {
                    session: id.clone(),
                    request: "r1".into()
                }
            ),
            Ok(Reply::Lookup(LookupResult::Known {
                outcome: Some(RunOutcome::Completed),
                ..
            }))
        )
    });
    // Closing the listener cannot stop an engine owned by another caller.
    drop(server);
    assert!(!fx.path.join(SOCKET).exists());
    let server = fx.server();
    let mut c = fx.connect();
    assert!(matches!(
        call(
            &mut c,
            Call::Lookup {
                session: id,
                request: "r1".into()
            }
        ),
        Ok(Reply::Lookup(LookupResult::Known { open: false, .. }))
    ));
    drop(server);
}

#[test]
fn hello_is_mandatory_bounded_and_version_checked_before_any_session_call() {
    let fx = Fixture::new(Scripted::echo("ok"), false);
    let _server = fx.server();
    let mut raw = connect(&fx.path).unwrap();
    raw.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    assert_eq!(
        call(
            &mut raw,
            Call::CreateSession {
                cwd: fx.project.display().to_string(),
                request: "must-not-exist".into()
            }
        )
        .unwrap_err()
        .code,
        ErrorCode::Invalid
    );
    let mut raw = connect(&fx.path).unwrap();
    assert_eq!(
        call(
            &mut raw,
            Call::Hello {
                protocol: PROTOCOL_VERSION - 1,
                client: "old".into()
            }
        )
        .unwrap_err()
        .code,
        ErrorCode::Incompatible
    );
    let mut raw = connect(&fx.path).unwrap();
    use std::io::Write;
    raw.write_all(&((HELLO_LIMIT + 1) as u32).to_be_bytes())
        .unwrap();
    raw.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    assert!(frame::read::<ServerFrame>(&mut raw).is_err());
    let mut good = fx.connect();
    assert_eq!(
        call(&mut good, Call::ListSessions).unwrap(),
        Reply::Sessions { sessions: vec![] }
    );
}

#[test]
fn unsafe_endpoints_and_foreign_path_types_are_refused_without_overwriting_them() {
    let fx = Fixture::new(Scripted::echo("ok"), false);
    fs::create_dir(&fx.path).unwrap();
    fs::set_permissions(&fx.path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Server::start(fx.engine.clone(), &fx.path).is_err());
    fs::set_permissions(&fx.path, fs::Permissions::from_mode(0o700)).unwrap();
    let target = fx._dir.path().join("keep");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, fx.path.join(SOCKET)).unwrap();
    assert!(Server::start(fx.engine.clone(), &fx.path).is_err());
    assert!(connect(&fx.path).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    fs::remove_file(fx.path.join(SOCKET)).unwrap();
    fs::write(fx.path.join(SOCKET), b"ordinary file").unwrap();
    assert!(Server::start(fx.engine.clone(), &fx.path).is_err());
    assert_eq!(fs::read(fx.path.join(SOCKET)).unwrap(), b"ordinary file");
    fs::remove_file(fx.path.join(SOCKET)).unwrap();
    fs::remove_file(fx.path.join("owner.lock")).unwrap();
    symlink(&target, fx.path.join("owner.lock")).unwrap();
    assert!(Server::start(fx.engine.clone(), &fx.path).is_err());
    let alias = fx._dir.path().join("alias");
    symlink(&fx.path, &alias).unwrap();
    assert!(Server::start(fx.engine.clone(), &alias).is_err());
}

#[test]
fn single_owner_reclaims_only_stale_sockets_and_does_not_delete_a_replacement() {
    let fx = Fixture::new(Scripted::echo("ok"), false);
    fs::DirBuilder::new().mode(0o700).create(&fx.path).unwrap();
    let stale = UnixListener::bind(fx.path.join(SOCKET)).unwrap();
    fs::set_permissions(fx.path.join(SOCKET), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        Server::start(fx.engine.clone(), &fx.path).is_err(),
        "must not steal a listening socket even without our lock"
    );
    drop(stale);
    let server = fx.server();
    assert!(
        Server::start(fx.engine.clone(), &fx.path).is_err(),
        "the endpoint lock admits one owner"
    );
    fs::remove_file(fx.path.join(SOCKET)).unwrap();
    fs::write(fx.path.join(SOCKET), b"replacement").unwrap();
    drop(server);
    assert_eq!(fs::read(fx.path.join(SOCKET)).unwrap(), b"replacement");
}

#[test]
fn two_socket_clients_share_questions_and_only_the_first_answer_wins() {
    let fx = Fixture::new(Scripted::echo("ok"), true);
    let _server = fx.server();
    let mut a = fx.connect();
    let mut b = fx.connect();
    let id = fx.create(&mut a);
    call(
        &mut a,
        Call::OpenSession {
            session: id.clone(),
            window: None,
        },
    )
    .unwrap();
    let mut question = None;
    wait("question", || {
        if let Reply::Snapshot(s) = call(
            &mut b,
            Call::OpenSession {
                session: id.clone(),
                window: None,
            },
        )
        .unwrap()
        {
            question = s.ui.questions.first().cloned();
        }
        question.is_some()
    });
    let q = question.unwrap();
    let answer = Call::AnswerQuestion {
        session: id,
        question: q.id,
        answer: Answer::Choice {
            value: q.options[0].value.clone(),
        },
    };
    call(&mut a, answer.clone()).unwrap();
    assert!(call(&mut b, answer).is_err());
}

#[test]
fn a_silent_handshake_does_not_prevent_other_clients_or_owner_shutdown() {
    let fx = Fixture::new(Scripted::echo("ok"), false);
    let server = fx.server();
    let silent = connect(&fx.path).unwrap();
    let mut other = fx.connect();
    assert!(matches!(
        call(&mut other, Call::ListSessions),
        Ok(Reply::Sessions { .. })
    ));
    let before = Instant::now();
    drop(server);
    assert!(
        before.elapsed() < Duration::from_secs(2),
        "owner stop interrupts silent client reads"
    );
    drop(silent);
}

#[test]
fn attachment_capacity_is_bounded_and_released_clients_allow_new_connections() {
    let fx = Fixture::new(Scripted::echo("ok"), false);
    let server = fx.server();
    let mut clients: Vec<_> = (0..MAX_CLIENTS).map(|_| fx.connect()).collect();
    assert_eq!(server.peers.lock().unwrap().len(), MAX_CLIENTS);
    let mut overflow = connect(&fx.path).unwrap();
    overflow
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    assert!(frame::read::<ServerFrame>(&mut overflow).is_err());
    clients.pop();
    wait("slot released", || {
        server.peers.lock().unwrap().len() < MAX_CLIENTS
    });
    let mut replacement = fx.connect();
    assert!(matches!(
        call(&mut replacement, Call::ListSessions),
        Ok(Reply::Sessions { .. })
    ));
}

#[test]
fn peer_identity_requires_the_current_os_user_and_handshake_deadlines_are_absolute() {
    assert!(require_owner(owner()).is_ok());
    assert!(require_owner(owner().wrapping_add(1)).is_err());
    let (mut input, mut output) = UnixStream::pair().unwrap();
    let writer = thread::spawn(move || {
        use std::io::Write;
        for _ in 0..20 {
            if output.write_all(b"x").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    let start = Instant::now();
    assert!(read_before(&mut input, &mut [0; 20], start + Duration::from_millis(100)).is_err());

    drop(input);
    writer.join().unwrap();
}

#[test]
fn an_inherited_endpoint_lock_descriptor_cannot_keep_a_closed_owner_alive() {
    let fx = Fixture::new(Scripted::echo("ok"), false);
    let server = fx.server();
    let inherited = server._endpoint._lock.try_clone().unwrap();
    drop(server);
    let next = fx.server();
    drop(inherited);
    assert!(Server::start(fx.engine.clone(), &fx.path).is_err());
    drop(next);
}
