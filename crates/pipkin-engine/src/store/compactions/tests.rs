use super::*;
use crate::provider::{CacheRetention, Credential, ProviderRequest, ThinkingLevel};
use std::time::Duration;

fn session(s: &Store, id: &str) {
    s.create_session(id, "/tmp", None, &Origin::Native, None, None, None, 1)
        .unwrap();
}
fn request() -> SummaryRequest {
    let req = ProviderRequest {
        model: crate::provider::scripted::model(),
        system: "summary system".into(),
        messages: vec![Message::user_text("selected original context", 100)],
        tools: vec![],
        thinking: ThinkingLevel::High,
        session_id: "affinity".into(),
        max_tokens: Some(321),
        cache_retention: CacheRetention::None,
        credential: Credential::ApiKey("secret-api-key".into()),
        headers: [("x-secret".into(), Some("secret-header".into()))].into(),
        base_url: Some("https://secret-endpoint/".into()),
        timeout: Duration::from_millis(1234),
        env: [("TOKEN".into(), "secret-env".into())].into(),
        options: [
            ("transport".into(), "sse".into()),
            ("futureAuthOption".into(), "secret-option".into()),
        ]
        .into_iter()
        .collect(),
    };
    SummaryRequest::capture(&req, 50, 40, 1000)
}
fn admit(s: &Store, key: &str, scope: CompactionScope) -> CompactionJob {
    s.tx(|c| Store::admit_compaction(c, "s", key, None, scope, Some("focus")))
        .unwrap()
}
fn read(s: &Store, id: u64) -> CompactionJob {
    s.read(|c| Store::compaction_in(c, id)).unwrap().unwrap()
}

#[test]
fn selected_request_and_absolute_retry_deadline_survive_reopen_without_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    session(&s, "s");
    let job = admit(&s, "manual-1", CompactionScope::Manual);
    let selected = request();
    let serialized = json(&selected);
    for secret in [
        "secret-api-key",
        "secret-header",
        "secret-endpoint",
        "secret-env",
        "secret-option",
    ] {
        assert!(!serialized.contains(secret));
    }
    assert_eq!(selected.stream_options.len(), 1);
    let progress = CompactionProgress::Retry {
        request: Box::new(selected),
        attempt: 3,
        until: 123456789,
    };
    s.tx(|c| {
        assert!(Store::save_compaction(c, &job, &progress)?);
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    let restored = read(&s, job.id);
    assert_eq!(restored.progress, progress);
    assert_eq!(restored.instructions.as_deref(), Some("focus"));
    assert_eq!(restored.revision, 1);
    assert_eq!(s.pending_compaction_ids().unwrap(), vec![restored.id]);
}

#[test]
fn response_usage_summary_and_settlement_commit_once_or_roll_back_together() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    session(&s, "s");
    let job = admit(&s, "response", CompactionScope::Manual);
    let response = CompactionProgress::Summarize {
        request: Box::new(request()),
        attempt: 2,
        response: Some(Box::new(SummaryResponse {
            message: Message::Assistant {
                content: vec![Block::text("saved provider result")],
                api: "scripted".into(),
                provider: "scripted".into(),
                model: "scripted".into(),
                usage: Usage {
                    input: 10,
                    output: 5,
                    total_tokens: 15,
                    ..Default::default()
                },
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 77,
                extra: Default::default(),
            },
            received_at: 77,
        })),
    };
    s.tx(|c| {
        assert!(Store::save_compaction(c, &job, &response)?);
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    let saved = read(&s, job.id);
    assert_eq!(saved.progress, response);
    let place = |c: &Connection| -> Result<bool> {
        let usage = Usage {
            input: 10,
            output: 5,
            total_tokens: 15,
            ..Default::default()
        };
        let entry_id = 1;
        let outcome = CompactionProgress::Terminal {
            result: CompactionOutcome::Done { entry: entry_id },
        };
        if !Store::save_compaction(c, &saved, &outcome)? {
            return Ok(false);
        }
        Store::add_usage(c, "s", "model", "scripted/scripted", &usage)?;
        // This intentionally uses the same transaction as the completion checkpoint.
        let entry = Store::append_entry(
            c,
            "s",
            80,
            None,
            &EntryKind::Compaction {
                summary: "saved summary".into(),
                first_kept: 40,
                tokens_before: 1000,
            },
        )?;
        assert_eq!(entry.id, entry_id);
        Ok(true)
    };
    s.fail_next_write();
    assert!(s.tx(|c| place(c)).is_err());
    assert_eq!(read(&s, job.id).progress, response);
    assert!(s.read(|c| Store::entry_in(c, "s", 1)).unwrap().is_none());
    assert!(s.tx(|c| place(c)).unwrap());
    assert!(!s.tx(|c| place(c)).unwrap());
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert!(s.pending_compaction_ids().unwrap().is_empty());
    let raw: String = s
        .read(|c| {
            Ok(
                c.query_row("SELECT value FROM usage WHERE session='s'", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Usage>(&raw).unwrap().total_tokens,
        15
    );
    assert!(matches!(
        read(&s, job.id).progress,
        CompactionProgress::Terminal {
            result: CompactionOutcome::Done { entry: 1 }
        }
    ));
}

#[test]
fn durable_stop_excludes_background_and_prevents_stale_response_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    session(&s, "s");
    let manual = admit(&s, "manual", CompactionScope::Manual);
    let background = admit(&s, "background", CompactionScope::Background);
    // This lower-level path is also used when aborting orphaned subagents.
    assert!(s.request_stop("s", 2).unwrap().is_none());
    let done = CompactionProgress::Terminal {
        result: CompactionOutcome::Done { entry: 1 },
    };
    assert!(!s.tx(|c| Store::save_compaction(c, &manual, &done)).unwrap());
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    let stopped = read(&s, manual.id);
    assert!(stopped.stopping);
    assert!(!read(&s, background.id).stopping);
    assert!(
        !s.tx(|c| Store::save_compaction(c, &stopped, &done))
            .unwrap()
    );
    assert!(
        s.tx(|c| Store::save_compaction(
            c,
            &stopped,
            &CompactionProgress::Terminal {
                result: CompactionOutcome::Aborted
            }
        ))
        .unwrap()
    );
    assert_eq!(s.pending_compaction_ids().unwrap().len(), 1);
}

#[test]
fn operation_identity_is_idempotent_and_cannot_move_between_runs_or_scopes() {
    let s = Store::memory().unwrap();
    session(&s, "s");
    session(&s, "other");
    let job = admit(&s, "same", CompactionScope::Manual);
    assert_eq!(admit(&s, "same", CompactionScope::Manual), job);
    assert!(
        s.tx(|c| Store::admit_compaction(
            c,
            "s",
            "same",
            None,
            CompactionScope::Background,
            Some("focus")
        ))
        .is_err()
    );
    assert!(
        s.tx(|c| Store::admit_compaction(
            c,
            "s",
            "same",
            None,
            CompactionScope::Manual,
            Some("changed")
        ))
        .is_err()
    );
    let run = s.tx(|c| Store::start_run(c, "other", None, 1)).unwrap();
    assert!(
        s.tx(|c| Store::admit_compaction(
            c,
            "s",
            "wrong-run",
            Some(run),
            CompactionScope::Blocking,
            None
        ))
        .is_err()
    );
    assert!(
        s.tx(|c| Store::admit_compaction(c, "s", "no-run", None, CompactionScope::Blocking, None))
            .is_err()
    );
}

#[test]
fn v5_upgrade_preserves_history_and_makes_a_private_backup_before_compaction_journal() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    session(&s, "s");
    s.tx(|c| {
        Store::append_entry(
            c,
            "s",
            1,
            None,
            &EntryKind::Message {
                message: Message::user_text("existing history", 1),
                mode: InputMode::Prompt,
                request: None,
                partial: false,
            },
        )?;
        c.execute_batch("DROP TABLE web_sources; DROP TABLE plan_receipts; DROP TABLE plan_events; DROP TABLE plan_tasks; DROP TABLE plans; DROP TABLE tool_call_ids; DROP TRIGGER event_session_delete; DROP TRIGGER removed_session_insert; DROP TABLE session_removals; DROP TABLE summary_writes; DROP TABLE tool_boundaries; DROP TABLE compactions; PRAGMA user_version = 5;")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(
        s.read(super::super::schema::version).unwrap(),
        crate::store::SCHEMA_VERSION
    );
    assert!(s.read(|c| Store::entry_in(c, "s", 1)).unwrap().is_some());
    assert!(s.pending_compaction_ids().unwrap().is_empty());
    let backup_path = dir.path().join("engine.sqlite.v5.bak");
    let backup =
        Connection::open_with_flags(&backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(super::super::schema::version(&backup).unwrap(), 5);
    assert!(backup.prepare("SELECT * FROM compactions").is_err());
    let history: String = backup
        .query_row("SELECT body FROM entries", [], |r| r.get(0))
        .unwrap();
    assert!(history.contains("existing history"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(backup_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn v6_activation_upgrade_preserves_pending_jobs_and_private_backup() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    session(&s, "s");
    let job = admit(&s, "pending-at-activation", CompactionScope::Manual);
    s.read(|c| {
        c.execute_batch("DROP TABLE web_sources; DROP TABLE plan_receipts; DROP TABLE plan_events; DROP TABLE plan_tasks; DROP TABLE plans; DROP TABLE tool_call_ids; DROP TRIGGER event_session_delete; DROP TRIGGER removed_session_insert; DROP TABLE session_removals; DROP TABLE summary_writes; DROP TABLE tool_boundaries; DROP INDEX compactions_recovery; PRAGMA user_version = 6")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = Store::open(dir.path()).unwrap();
    assert_eq!(
        s.read(super::super::schema::version).unwrap(),
        crate::store::SCHEMA_VERSION
    );
    assert_eq!(read(&s, job.id), job);
    let backup = dir.path().join("engine.sqlite.v6.bak");
    let c =
        Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(super::super::schema::version(&c).unwrap(), 6);
    assert_eq!(Store::compaction_in(&c, job.id).unwrap().unwrap(), job);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
