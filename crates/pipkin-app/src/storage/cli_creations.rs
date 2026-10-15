//! Write-ahead public creation identities only. No ticket contents, arguments or grants.
use super::*;
use pipkin_core::{EngineSessionId, LOCAL_CLI_RECOVERY_CAPACITY, LocalCliCreationId, SetupId};
use rusqlite::OptionalExtension;

impl Storage {
    pub fn load_cli_creations(&self) -> Result<Vec<LocalCliCreationId>, String> {
        let conn = Connection::open(&self.path).map_err(sql_err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(sql_err)?;
        // Bound values in SQLite before decoding: a corrupt row must not allocate an
        // arbitrarily large String just to discover that its identity is invalid.
        let mut stmt = conn.prepare("SELECT
            CASE WHEN typeof(session)='text' AND length(CAST(session AS BLOB)) BETWEEN 1 AND 128 THEN session END,
            CASE WHEN typeof(setup)='text' AND length(CAST(setup AS BLOB)) BETWEEN 1 AND 124 THEN setup END
            FROM cli_creation_recovery WHERE namespace=?1 ORDER BY session LIMIT ?2").map_err(sql_err)?;
        let rows = stmt
            .query_map(
                params![&self.namespace, (LOCAL_CLI_RECOVERY_CAPACITY + 2) as i64],
                |r| {
                    Ok(LocalCliCreationId {
                        session: EngineSessionId(r.get(0)?),
                        setup: SetupId(r.get(1)?),
                    })
                },
            )
            .map_err(sql_err)?;
        let ids = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql_err)?;
        if ids.len() > LOCAL_CLI_RECOVERY_CAPACITY + 1
            || ids.iter().any(|i| !i.valid_recovery_identity())
        {
            return Err("CLI creation recovery is invalid or exceeds its bound.".into());
        }
        Ok(ids)
    }

    pub fn journal_cli_creation(
        &self,
        identity: LocalCliCreationId,
        retire: bool,
        ack: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        if let Err(Msg::CliCreation { ack, .. }) = self.send(Msg::CliCreation {
            identity,
            retire,
            ack: Box::new(ack),
        }) {
            ack(Err(
                "The recovery writer is unavailable; creation was not released.".into(),
            ));
        }
    }
}

pub(super) fn write(
    conn: &mut Connection,
    ns: &str,
    identity: &LocalCliCreationId,
    retire: bool,
) -> Result<(), String> {
    if !identity.valid_recovery_identity() {
        return Err("Invalid CLI creation recovery identity.".into());
    }
    let tx = conn.transaction().map_err(sql_err)?;
    if retire {
        tx.execute(
            "DELETE FROM cli_creation_recovery WHERE namespace=?1 AND session=?2 AND setup=?3",
            params![ns, identity.session.0, identity.setup.0],
        )
        .map_err(sql_err)?;
    } else {
        let existing: Option<String> = tx
            .query_row(
                "SELECT CASE WHEN typeof(setup)='text' AND length(CAST(setup AS BLOB)) BETWEEN 1 AND 124 THEN setup END
                 FROM cli_creation_recovery WHERE namespace=?1 AND session=?2",
                params![ns, identity.session.0],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)?;
        if let Some(existing) = existing {
            if existing != identity.setup.0 {
                return Err("CLI creation identity conflicts with its saved record.".into());
            }
        } else {
            let count: usize = tx
                .query_row(
                    "SELECT count(*) FROM cli_creation_recovery WHERE namespace=?1",
                    [ns],
                    |r| r.get(0),
                )
                .map_err(sql_err)?;
            if count > LOCAL_CLI_RECOVERY_CAPACITY {
                return Err(
                    "CLI creation recovery is full; resolve saved identities first.".into(),
                );
            }
            tx.execute(
                "INSERT INTO cli_creation_recovery(namespace,session,setup) VALUES (?1,?2,?3)",
                params![ns, identity.session.0, identity.setup.0],
            )
            .map_err(sql_err)?;
        }
    }
    tx.commit().map_err(sql_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(n: usize) -> LocalCliCreationId {
        LocalCliCreationId {
            session: EngineSessionId(format!("cli-{n}")),
            setup: SetupId(format!("setup-{n}")),
        }
    }
    fn save(s: &Storage, id: LocalCliCreationId, retire: bool) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        s.journal_cli_creation(id, retire, move |r| {
            let _ = tx.send(r);
        });
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap()
    }
    #[test]
    fn failed_writes_and_corrupt_records_never_look_like_successful_empty_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let s = Storage::open(&path, "native:test").unwrap();
        s.set_write_failure(true);
        assert!(save(&s, id(1), false).is_err());
        assert!(s.load_cli_creations().unwrap().is_empty());
        s.set_write_failure(false);
        save(&s, id(1), false).unwrap();
        s.set_write_failure(true);
        assert!(save(&s, id(1), true).is_err());
        assert_eq!(s.load_cli_creations().unwrap(), vec![id(1)]);
        s.shutdown();
        let conn = Connection::open(&path).unwrap();
        conn.execute("UPDATE cli_creation_recovery SET setup='../invalid'", [])
            .unwrap();
        let reopened = Storage::open(&path, "native:test").unwrap();
        assert!(reopened.load_cli_creations().is_err());
        conn.execute(
            "UPDATE cli_creation_recovery SET setup=?1",
            ["x".repeat(1024 * 1024)],
        )
        .unwrap();
        assert!(reopened.load_cli_creations().is_err());
        assert!(save(&reopened, id(1), false).is_err());
        conn.execute(
            "UPDATE cli_creation_recovery SET setup=zeroblob(1024*1024)",
            [],
        )
        .unwrap();
        assert!(reopened.load_cli_creations().is_err());
    }

    #[test]
    fn crash_fixture_child() {
        let Some(path) = std::env::var_os("PIPKIN_CREATION_CRASH_FIXTURE") else {
            return;
        };
        let s = Storage::open(Path::new(&path), "native:test").unwrap();
        save(&s, id(42), false).unwrap();
        use std::io::Write;
        println!("CREATION_DURABLE");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn committed_identity_survives_kill_before_any_creation_rpc() {
        use std::{
            io::BufRead,
            process::{Command, Stdio},
            time::Duration,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::cli_creations::tests::crash_fixture_child",
                "--nocapture",
            ])
            .env("PIPKIN_CREATION_CRASH_FIXTURE", &path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        struct Owned(std::process::Child);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Owned(child);
        let stdout = child.0.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
                if line.contains("CREATION_DURABLE") {
                    let _ = tx.send(());
                    break;
                }
            }
        });
        let committed = rx.recv_timeout(Duration::from_secs(10));
        drop(child);
        reader.join().unwrap();
        committed.unwrap();
        let reopened = Storage::open(&path, "native:test").unwrap();
        assert_eq!(reopened.load_cli_creations().unwrap(), vec![id(42)]);
    }

    #[test]
    fn durable_bounded_exact_identities_survive_reopen_without_eviction_or_namespace_leak() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let s = Storage::open(&path, "native:test").unwrap();
        for n in 0..=LOCAL_CLI_RECOVERY_CAPACITY {
            save(&s, id(n), false).unwrap();
        }
        save(&s, id(0), false).unwrap();
        assert!(save(&s, id(99), false).is_err());
        let mut changed = id(0);
        changed.setup = SetupId("other".into());
        assert!(save(&s, changed.clone(), false).is_err());
        save(&s, changed, true).unwrap();
        assert_eq!(
            s.load_cli_creations().unwrap().len(),
            LOCAL_CLI_RECOVERY_CAPACITY + 1
        );
        s.shutdown();
        let reopened = Storage::open(&path, "native:test").unwrap();
        assert_eq!(
            reopened.load_cli_creations().unwrap().len(),
            LOCAL_CLI_RECOVERY_CAPACITY + 1
        );
        let other = Storage::open(&path, "demo").unwrap();
        assert!(other.load_cli_creations().unwrap().is_empty());
        save(&reopened, id(0), true).unwrap();
        save(&reopened, id(99), false).unwrap();
        assert!(!reopened.load_cli_creations().unwrap().contains(&id(0)));
    }
}
