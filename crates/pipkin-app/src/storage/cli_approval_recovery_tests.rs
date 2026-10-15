use super::*;
fn record(conversation: u64, invocation: usize) -> Record {
    let run = CliApprovalRun {
        epoch: 1,
        session: format!("s{conversation}"),
        binding_generation: 1,
        op: "operation".into(),
        prompt: 1,
    };
    Record {
        context: CliApprovalContext {
            conversation: ConversationId(conversation),
            generation: 3,
            run_op: OperationId(10),
            run: run.clone(),
        },
        id: CliApprovalId {
            run,
            invocation: format!("{invocation:064x}"),
            arguments_sha256: "cd".repeat(32),
        },
        decision: D::AllowOnce,
        receipt: None,
    }
}
fn save(s: &Storage, r: Record) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    s.journal_cli_approval(r, move |r| {
        let _ = tx.send(r);
    });
    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap()
}
#[test]
fn exact_attempts_survive_reopen_and_unknown_scopes_cannot_be_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let s = Storage::open(&path, "a").unwrap();
    let old = record(1, 0);
    save(&s, old.clone()).unwrap();
    save(&s, old.clone()).unwrap();
    let mut conflict = old.clone();
    conflict.id.arguments_sha256 = "ab".repeat(32);
    assert!(save(&s, conflict).is_err());
    let mut next = old.clone();
    next.context.run.prompt += 1;
    next.context.run_op.0 += 1;
    next.id.run = next.context.run.clone();
    assert!(save(&s, next.clone()).is_err());
    s.shutdown();
    let s = Storage::open(&path, "a").unwrap();
    assert_eq!(s.load_cli_approvals().unwrap(), vec![old.clone()]);
    let mut known = old.clone();
    known.receipt = Some(S::Consumed);
    save(&s, known.clone()).unwrap();
    save(&s, old).unwrap();
    assert_eq!(s.load_cli_approvals().unwrap(), vec![known.clone()]);
    let mut reattached = known.clone();
    reattached.context.generation += 1;
    save(&s, reattached.clone()).unwrap();
    assert!(save(&s, known).is_err());
    save(&s, next.clone()).unwrap();
    assert_eq!(s.load_cli_approvals().unwrap(), vec![next]);
    let json: String = Connection::open(&path)
        .unwrap()
        .query_row("SELECT record FROM cli_approval_recovery", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(
        !json.contains("arguments") && !json.contains("file_path") && !json.contains("content")
    );
}
#[test]
fn capacity_is_durable_namespace_scoped_and_never_evicts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let s = Storage::open(&path, "a").unwrap();
    for n in 0..CLI_APPROVAL_REQUESTS {
        save(&s, record(1, n)).unwrap();
    }
    assert!(save(&s, record(1, CLI_APPROVAL_REQUESTS)).is_err());
    for n in 2..=CLI_APPROVAL_CONTEXTS {
        save(&s, record(n as u64, 0)).unwrap();
    }
    assert!(save(&s, record(CLI_APPROVAL_CONTEXTS as u64 + 1, 0)).is_err());
    let before = s.load_cli_approvals().unwrap();
    s.shutdown();
    let s = Storage::open(&path, "a").unwrap();
    assert_eq!(s.load_cli_approvals().unwrap(), before);
    let other = Storage::open(&path, "b").unwrap();
    assert!(other.load_cli_approvals().unwrap().is_empty());
    save(&other, record(1, 0)).unwrap();
    assert_eq!(s.load_cli_approvals().unwrap(), before);
}
#[test]
fn write_failure_and_corruption_are_not_empty_or_successful_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let s = Storage::open(&path, "a").unwrap();
    s.set_write_failure(true);
    assert!(save(&s, record(1, 0)).is_err());
    assert!(s.load_cli_approvals().unwrap().is_empty());
    s.set_write_failure(false);
    let old = record(1, 0);
    save(&s, old.clone()).unwrap();
    s.set_background_failure(true);
    let mut known = old.clone();
    known.receipt = Some(S::Consumed);
    assert!(save(&s, known).is_err());
    assert_eq!(s.load_cli_approvals().unwrap(), vec![old]);
    s.shutdown();
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE cli_approval_recovery SET record=?1",
        ["x".repeat(1024 * 1024)],
    )
    .unwrap();
    let s = Storage::open(&path, "a").unwrap();
    assert!(s.load_cli_approvals().is_err());
    assert!(save(&s, record(1, 0)).is_err());
}
#[test]
fn crash_fixture_child() {
    let Some(path) = std::env::var_os("PIPKIN_APPROVAL_CRASH_FIXTURE") else {
        return;
    };
    let s = Storage::open(Path::new(&path), "a").unwrap();
    save(&s, record(1, 0)).unwrap();
    use std::io::Write;
    println!("APPROVAL_DURABLE");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
fn killed_writer_retains_uncertainty_before_any_decision_rpc() {
    use std::{
        io::BufRead,
        process::{Command, Stdio},
        time::Duration,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    struct Owned(std::process::Child);
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Owned(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::cli_approvals::tests::crash_fixture_child",
                "--nocapture",
            ])
            .env("PIPKIN_APPROVAL_CRASH_FIXTURE", &path)
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
            if line.contains("APPROVAL_DURABLE") {
                let _ = tx.send(());
                break;
            }
        }
    });
    let committed = rx.recv_timeout(Duration::from_secs(10));
    drop(child);
    reader.join().unwrap();
    committed.unwrap();
    let s = Storage::open(&path, "a").unwrap();
    assert_eq!(s.load_cli_approvals().unwrap(), vec![record(1, 0)]);
}
