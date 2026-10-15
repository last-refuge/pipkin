//! Abrupt engine death with an owned fake CLI. This explicitly does NOT qualify orphan
//! containment: an already delivered allow can still have effects after its engine dies.
use super::*;
use std::{
    io::{BufRead, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    process::{Child, Command, Stdio},
};

#[test]
fn engine_crash_fixture_child() {
    let Ok(mode) = std::env::var("PIPKIN_ENGINE_CLI_CRASH_FIXTURE") else {
        return;
    };
    let f = Fixture::new();
    let provider = Scripted::echo("forbidden native inference");
    let engine = f.start(&provider);
    let shared = engine.handle.shared().clone();
    let cli = fake(&f, &mode);
    seed(&cli, &shared);
    let (control, mut reviews) = submit(
        &cli,
        &shared,
        f.dir.path().into(),
        "crash",
        CancellationToken::new(),
    );
    let review = review(&shared, &mut reviews);
    if mode == "crash-after-allow" {
        control
            .decide(7, &review.ticket, Decision::AllowOnce)
            .unwrap();
        super::super::wait(|| f.project.join("allow-paused").exists());
    }
    // One assistant frame projects three separately committed blocks. Observing *any*
    // final block is insufficient: SIGKILL could then precede the second text commit.
    // Pin the durable text boundary the parent subsequently expects recovery to retain.
    super::super::wait(|| {
        let finals = shared.store.read(|c| Ok(c.query_row(
            "SELECT COALESCE(MAX(final_blocks),'[]') FROM external_projection_messages WHERE message_id='answer'",
            [], |r| r.get::<_, String>(0),
        )?)).unwrap();
        let finals: Vec<usize> = serde_json::from_str(&finals).unwrap();
        finals.contains(&0) && finals.contains(&2)
    });
    no_native(&shared, &provider);
    println!("ENGINE_CRASH_READY {}", f.dir.path().display());
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
struct OwnedEngine(Child);
impl Drop for OwnedEngine {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct OwnedFake(OwnedFd);
impl OwnedFake {
    fn capture(project: &std::path::Path) -> Self {
        let pid: i32 = std::fs::read_to_string(project.join("fixture-pid"))
            .unwrap()
            .parse()
            .unwrap();
        // Pin the process before engine death. Validate the unique owned fixture cwd on
        // both sides of pidfd_open; never signal an arbitrary numeric PID after a restart.
        let cwd = PathBuf::from(format!("/proc/{pid}/cwd"));
        assert_eq!(std::fs::read_link(&cwd).unwrap(), project);
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        assert!(fd >= 0, "pidfd_open: {}", std::io::Error::last_os_error());
        let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        assert_eq!(std::fs::read_link(&cwd).unwrap(), project);
        Self(fd)
    }
    fn exited(&self, timeout: i32) -> bool {
        let mut p = libc::pollfd {
            fd: self.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut p, 1, timeout) > 0 && p.revents & libc::POLLIN != 0 }
    }
}
impl Drop for OwnedFake {
    fn drop(&mut self) {
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.0.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
        let _ = self.exited(2000);
    }
}

#[test]
fn engine_sigkill_recovers_pending_and_consumed_without_replay_or_false_action_proof() {
    for consumed in [false, true] {
        let owned = tempfile::tempdir().unwrap();
        let mut child = OwnedEngine(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runtime::external_tests::coding::crash::engine_crash_fixture_child",
                    "--nocapture",
                ])
                .env(
                    "PIPKIN_ENGINE_CLI_CRASH_FIXTURE",
                    if consumed {
                        "crash-after-allow"
                    } else {
                        "crash-pending"
                    },
                )
                .env("TMPDIR", owned.path())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
                if let Some((_, path)) = line.split_once("ENGINE_CRASH_READY ") {
                    let _ = tx.send(PathBuf::from(path));
                    break;
                }
            }
        });
        let root = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        reader.join().unwrap();
        assert!(root.starts_with(owned.path()));
        let project = root.join("project");
        let fake = OwnedFake::capture(&project);
        assert!(!project.join("effect").exists());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let provider = Scripted::echo("forbidden native inference");
        let mut config = crate::config::EngineConfig::new(root.join("data"), root.join("config"));
        config.env = crate::config::Env::isolated(Default::default());
        let engine = crate::runtime::EngineHandle::start_with(
            config,
            crate::runtime::registry_with(Arc::new(provider.clone())),
            crate::tools::builtin_tools(),
        )
        .unwrap();
        let shared = engine.shared();
        let receipt = shared
            .store
            .external_prompt("cli", "crash")
            .unwrap()
            .unwrap();
        assert_eq!(receipt.state, ExternalRunState::Unknown);
        assert!(
            matches!(&receipt.result,Some(PromptResult::Unanswered {text:Some(text),..}) if text=="beforeafter")
        );
        assert!(
            !shared
                .store
                .cli_binding("cli")
                .unwrap()
                .unwrap()
                .context_valid
        );
        assert!(
            shared
                .external_duplicate("cli", "crash", "synthetic prompt")
                .unwrap()
                .is_some()
        );
        let state = shared
            .store
            .read(|c| {
                Ok(
                    c.query_row("SELECT state FROM external_permissions", [], |r| {
                        r.get::<_, String>(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(state, if consumed { "consumed" } else { "revoked" });
        assert_eq!(
            std::fs::read_to_string(project.join("launches"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert!(
            !project.join("effect").exists(),
            "receipt metadata cannot assert an action happened"
        );
        if consumed {
            // A real limitation, not successful containment: the detached fake is still
            // alive and can finish its already-authorized action after engine death.
            assert!(!fake.exited(0));
            std::fs::write(project.join("release-after-crash"), "yes").unwrap();
        }
        assert!(
            fake.exited(5000),
            "owned fake must exit on broker EOF or fixture release"
        );
        assert_eq!(project.join("effect").exists(), consumed);
        assert_eq!(
            shared
                .store
                .external_prompt("cli", "crash")
                .unwrap()
                .unwrap()
                .state,
            ExternalRunState::Unknown
        );
        no_native(shared, &provider);
        engine.shutdown(Duration::from_secs(5));
    }
}
