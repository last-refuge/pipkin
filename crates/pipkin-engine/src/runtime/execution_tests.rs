//! Storage-only CLI identities must remain inert through the real engine/client contract.
//! All executors/configuration are local test fixtures; no installed CLI or live model.

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::Fixture;
use crate::provider::scripted::Scripted;
use crate::store::{
    CliBindingSpec, ExternalActionStatus, ExternalRunKey, ExternalRunState as S, Store,
};

fn binding(fixture: &Fixture) -> CliBindingSpec {
    CliBindingSpec {
        cli_session_id: "11111111-1111-4111-8111-111111111111".into(),
        project_path: fixture.project.to_str().unwrap().into(),
        executable_path: "/not-executed/claude".into(),
        cli_version: "2.1.296".into(),
        config_ref: "fixture-profile".into(),
        account_scope: "fixture-account".into(),
        launch_policy: "qualification-only".into(),
    }
}

#[test]
fn disabled_cli_history_opens_without_extensions_and_native_conversations_still_run() {
    let fixture = Fixture::new();
    let provider = Scripted::echo("native still works");
    let mut config = fixture.config();
    config.extensions = vec![ExtensionSpec::Builtin("greeting".into())];
    let mut engine = fixture.start_config(&provider, config);
    engine
        .handle
        .shared()
        .store
        .create_cli_session("cli", "cli-key", &binding(&fixture), 1)
        .unwrap();
    engine
        .handle
        .shared()
        .store
        .tx(|tx| {
            // Legacy/foreign native operational rows must not become CLI controls.
            let op: PlanOp =
                serde_json::from_value(json!({"op": "create", "title": "foreign native task"}))
                    .unwrap();
            Store::apply_plan(tx, "cli", "foreign-plan", &[op], None, 1)?
                .0
                .unwrap();
            assert_ne!(Store::plan_view(tx, "cli", false)?, PlanView::default());
            Store::append_entry(
                tx,
                "cli",
                1,
                None,
                &EntryKind::Notice {
                    level: NoticeLevel::Info,
                    text: "readable external history".into(),
                },
            )?;
            Ok(())
        })
        .unwrap();
    let revision = engine
        .handle
        .shared()
        .store
        .session("cli")
        .unwrap()
        .unwrap()
        .revision;
    for _ in 0..2 {
        let view = engine.snapshot("cli");
        assert_eq!(
            view.summary.execution_backend,
            ExecutionBackend::LocalClaudeCli
        );
        assert!(
            view.summary
                .blocked
                .as_deref()
                .unwrap()
                .contains("not enabled")
        );
        assert_eq!(view.entries.len(), 1);
        assert!(view.run.is_none() && view.queue.is_empty());
        assert!(
            view.context.tokens.is_none()
                && view.context.capacity.is_none()
                && view.subagents.is_none()
        );
        assert_eq!(view.context.source, "unavailable");
        assert!(view.thinking.levels.is_empty());
        assert!(view.ui.notices.is_empty());
        assert_eq!(view.plan, PlanView::default());
        assert_eq!(view.jobs, JobsView::default());
        assert!(view.goal.is_none() && view.web_sources.is_empty());
    }
    let sess = engine.handle.shared().session("cli").unwrap();
    assert!(sess.extension_profile.is_empty());
    assert!(sess.extensions.try_lock().unwrap().is_none());
    assert!(
        engine
            .handle
            .shared()
            .tools_for(
                &engine
                    .handle
                    .shared()
                    .store
                    .session("cli")
                    .unwrap()
                    .unwrap()
            )
            .is_empty()
    );
    let calls = vec![
        Call::Prompt {
            session: "cli".into(),
            request: "p".into(),
            text: "do not execute".into(),
            images: vec![],
            model: None,
        },
        Call::Enqueue {
            session: "cli".into(),
            request: "q".into(),
            mode: InputMode::FollowUp,
            text: "do not queue".into(),
            images: vec![],
        },
        Call::Stop {
            session: "cli".into(),
        },
        Call::Compact {
            session: "cli".into(),
            instructions: None,
        },
        Call::SetModel {
            session: "cli".into(),
            model: "scripted/scripted".into(),
        },
        Call::SetThinking {
            session: "cli".into(),
            level: "high".into(),
        },
        Call::SetGoal {
            session: "cli".into(),
            objective: "do not automate".into(),
        },
        Call::SetSubagentsEnabled {
            session: "cli".into(),
            enabled: true,
        },
        Call::LaunchSubagent {
            session: "cli".into(),
            request: "child".into(),
            task: "do not delegate".into(),
            profile: None,
            context: None,
        },
        Call::JobStart {
            session: "cli".into(),
            request: "job".into(),
            command: "touch must-not-exist".into(),
            name: None,
            cwd: None,
            timeout_secs: None,
        },
        Call::PrepareExtensions {
            session: Some("cli".into()),
            extensions: Some(vec![]),
        },
        Call::McpTestServer {
            session: Some("cli".into()),
            name: "do-not-start".into(),
        },
    ];
    for call in calls {
        assert_eq!(engine.call(call).unwrap_err().code, ErrorCode::Unsupported);
    }
    assert!(
        matches!(engine.call(Call::Models { session: Some("cli".into()) }).unwrap(), Reply::Models(v) if v.models.is_empty() && v.selected.is_none())
    );
    assert!(matches!(
        engine
            .call(Call::Lookup {
                session: "cli".into(),
                request: "p".into()
            })
            .unwrap(),
        Reply::Lookup(LookupResult::Unknown)
    ));
    assert_eq!(
        engine
            .handle
            .shared()
            .store
            .session("cli")
            .unwrap()
            .unwrap()
            .revision,
        revision
    );
    assert!(provider.requests().is_empty());
    assert!(!fixture.project.join("must-not-exist").exists());

    // The run-level guard cannot be bypassed by skipping RPC admission.
    engine.handle.shared().spawn_run(sess.clone(), 123);
    assert!(sess.active_run().is_none());
    let shared = engine.handle.shared();
    assert!(shared.handle.block_on(shared.prepare(&sess)).is_err());
    assert!(
        shared
            .handle
            .block_on(shared.compact_manual(
                &sess,
                None,
                &tokio_util::sync::CancellationToken::new()
            ))
            .is_err()
    );
    assert!(sess.failure().is_none());
    drop(sess);
    engine
        .call(Call::RenameSession {
            session: "cli".into(),
            title: "External history".into(),
        })
        .unwrap();
    assert!(
        matches!(engine.call(Call::ListSessions).unwrap(), Reply::Sessions { sessions } if sessions.iter().any(|s| s.id == "cli" && s.title.as_deref() == Some("External history") && s.execution_backend == ExecutionBackend::LocalClaudeCli))
    );
    assert!(
        matches!(engine.call(Call::HistoryPage { session: "cli".into(), before: None, limit: 10 }).unwrap(), Reply::Page { entries, .. } if entries.len() == 1)
    );

    let native = engine.create(&fixture.project, "native-key");
    assert_eq!(
        engine.snapshot(&native).summary.execution_backend,
        ExecutionBackend::NativeAgent
    );
    engine.prompt(&native, "native-prompt", "hello").unwrap();
    assert_eq!(engine.settled("native-prompt"), RunOutcome::Completed);
    assert_eq!(provider.requests().len(), 1);
    engine
        .call(Call::RemoveSession {
            session: "cli".into(),
        })
        .unwrap();
    assert!(
        engine
            .handle
            .shared()
            .store
            .session("cli")
            .unwrap()
            .is_none()
    );
}

#[test]
fn restart_marks_external_work_unknown_without_touching_foreign_native_replay_records() {
    let fixture = Fixture::new();
    let key = ExternalRunKey {
        session: "cli".into(),
        generation: 1,
        op: "external-op".into(),
    };
    let store = Store::open(&fixture.config().data_dir).unwrap();
    store
        .create_cli_session("cli", "cli-key", &binding(&fixture), 1)
        .unwrap();
    assert!(store.admit_external_run(&key, 2).unwrap().1);
    store
        .transition_external_run(&key, S::Admitted, S::LaunchIntent, 3)
        .unwrap();
    store
        .transition_external_run(&key, S::LaunchIntent, S::Running, 4)
        .unwrap();
    store
        .record_external_action(
            &key,
            "observed-write",
            "Write",
            ExternalActionStatus::Running,
            5,
        )
        .unwrap();
    let run = store.tx(|tx| {
        // Deliberately seed foreign native journal rows, bypassing the guarded store APIs.
        // Neither recovery nor repeated opens may treat these as execution authority.
        tx.execute("INSERT INTO runs(session,state,started_at) VALUES('cli','running',1)", [])?;
        let run = tx.last_insert_rowid() as u64;
        Store::append_entry(tx, "cli", 1, Some(run), &EntryKind::Message {
            message: Message::Assistant {
                content: vec![Block::tool_call("pending-write", "write", json!({"path":"must-not-exist", "content":"not authorized"}))],
                api: "scripted".into(), provider: "scripted".into(), model: "scripted".into(),
                usage: Default::default(), stop_reason: StopReason::ToolUse, error_message: None,
                timestamp: 1, extra: Default::default(),
            }, mode: InputMode::Prompt, request: None, partial: false,
        })?;
        tx.execute("INSERT INTO tool_calls(session,call_id,run,name,arguments,state,replay_safe,started_at) VALUES('cli','foreign-intent',?1,'write','{}','intent',1,1)", [run])?;
        tx.execute("INSERT INTO generation_state(run,failures,retry_until,overflow_compacted) VALUES(?1,1,9999999999999,0)", [run])?;
        // A pinned compaction bypasses provider `prepare`; the executor itself must be gated.
        let progress = crate::store::compactions::CompactionProgress::Summarize {
            request: Box::new(crate::store::compactions::SummaryRequest {
                model: "scripted/scripted".into(), thinking: "off".into(),
                provider_session: "native-affinity-must-not-be-used".into(),
                messages: vec![Message::user_text("must not summarize", 1)],
                max_tokens: 64, timeout_ms: 30_000, stream_options: Default::default(),
                tail: 1, first_kept: 1, tokens_before: 1,
            }), attempt: 0, response: None,
        };
        tx.execute("INSERT INTO compactions(session,operation_key,run,scope,state,progress) VALUES('cli','foreign-compaction',?1,'blocking','running',?2)", rusqlite::params![run, serde_json::to_string(&progress).unwrap()])?;
        tx.execute("INSERT INTO requests(session,key,fingerprint,mode,state,queue_entry,text,images,created_at) VALUES('cli','foreign-queue','fake','followUp','queued',100,'must not submit','[]',1)", [])?;
        Ok(run)
    }).unwrap();
    let before = store.page("cli", None, 100).unwrap();
    drop(store);
    let provider = Scripted::echo("must never be requested");
    let mut engine = fixture.start(&provider);
    for _ in 0..2 {
        engine.snapshot("cli");
    }
    let shared = engine.handle.shared();
    assert_eq!(
        shared.store.external_run(&key).unwrap().unwrap().state,
        S::Unknown
    );
    let recovered = shared.store.cli_binding("cli").unwrap().unwrap();
    assert!(!recovered.context_valid && recovered.generation > key.generation);
    assert_eq!(
        shared.store.external_actions(&key).unwrap()[0].status,
        ExternalActionStatus::Unknown
    );
    assert!(
        shared
            .store
            .admit_external_run(
                &ExternalRunKey {
                    op: "retry".into(),
                    generation: recovered.generation,
                    ..key.clone()
                },
                9
            )
            .is_err()
    );
    assert!(
        shared
            .store
            .transition_external_run(&key, S::Running, S::Completed, 9)
            .is_err()
    );
    assert_eq!(shared.store.page("cli", None, 100).unwrap(), before);
    assert_eq!(shared.store.open_run("cli").unwrap().unwrap().id, run);
    assert_eq!(shared.store.queue("cli").unwrap().len(), 1);
    assert!(
        shared
            .store
            .tool_call("cli", "pending-write")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        shared
            .store
            .tool_call("cli", "foreign-intent")
            .unwrap()
            .unwrap()
            .state,
        crate::store::ToolState::Intent
    );
    assert!(shared.session("cli").unwrap().active_run().is_none());
    let sess = shared.session("cli").unwrap();
    let compaction = shared.store.pending_compactions_page(0, 10).unwrap()[0].0;
    // Exercise the pinned-request executor directly too, not just a scheduler that may
    // not have been polled yet. No claim/checkpoint/result is written by this refusal.
    assert!(
        shared
            .handle
            .block_on(shared.execute_compaction(
                &sess,
                compaction,
                &tokio_util::sync::CancellationToken::new()
            ))
            .is_err()
    );
    assert_eq!(
        shared.store.pending_compactions_page(0, 10).unwrap().len(),
        1
    );
    assert!(provider.requests().is_empty());
    assert!(!fixture.project.join("must-not-exist").exists());
}

#[test]
fn recovery_of_foreign_native_child_under_cli_root_has_no_effects() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.config().data_dir).unwrap();
    store
        .create_cli_session("cli", "cli-key", &binding(&fixture), 1)
        .unwrap();
    store.tx(|tx| {
        tx.execute("INSERT INTO sessions(id,cwd,created_at,updated_at,origin,parent,root) VALUES('foreign-child',?1,1,1,?2,'cli','cli')", rusqlite::params![fixture.project.to_str().unwrap(), serde_json::to_string(&Origin::default()).unwrap()])?;
        tx.execute("INSERT INTO runs(session,state,started_at,stop_requested_at) VALUES('foreign-child','stopping',1,1)", [])?;
        Ok(())
    }).unwrap();
    let revision = store.session("foreign-child").unwrap().unwrap().revision;
    drop(store);
    let provider = Scripted::echo("must not execute");
    let mut engine = fixture.start(&provider);
    engine.snapshot("foreign-child");
    let shared = engine.handle.shared();
    shared.recover_session("foreign-child").unwrap();
    assert_eq!(
        shared
            .store
            .session("foreign-child")
            .unwrap()
            .unwrap()
            .revision,
        revision
    );
    assert!(shared.store.open_run("foreign-child").unwrap().is_some());
    assert!(
        shared
            .store
            .page("foreign-child", None, 100)
            .unwrap()
            .0
            .is_empty()
    );
    assert!(provider.requests().is_empty());
}

#[test]
fn native_creation_key_cannot_select_cli_backend_and_old_protocol_is_refused() {
    let fixture = Fixture::new();
    let provider = Scripted::echo("unused");
    let mut engine = fixture.start(&provider);
    engine
        .handle
        .shared()
        .store
        .create_cli_session("cli", "same-key", &binding(&fixture), 1)
        .unwrap();
    assert_eq!(
        engine
            .call(Call::CreateSession {
                cwd: fixture.project.to_str().unwrap().into(),
                request: "same-key".into()
            })
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        engine
            .call(Call::Hello {
                protocol: PROTOCOL_VERSION - 1,
                client: "old".into()
            })
            .unwrap_err()
            .code,
        ErrorCode::Incompatible
    );
    assert!(provider.requests().is_empty());
}
