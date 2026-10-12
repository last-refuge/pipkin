//! Settings the engine applies at run time (queue modes, compaction overrides, model scope,
//! per-model thinking, network settings) and `RefreshModels`, through the client contract.

use std::collections::HashMap;
use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::config::Env;
use crate::provider::ProviderRequest;
use crate::provider::mock::{MockResponse, MockServer};
use crate::provider::scripted::{Gate, Reply as R, Scripted};

fn write_settings(fx: &Fixture, json: &serde_json::Value) {
    let dir = fx.dir.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("settings.json"), json.to_string()).unwrap();
}

#[test]
fn websocket_connect_timeout_setting_reaches_requests_including_zero_and_live_edits() {
    let fx = Fixture::new();
    let provider = Scripted::echo("ok");
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "ws-timeout");
    for (index, millis) in [200, 0, 1250].into_iter().enumerate() {
        write_settings(&fx, &json!({"websocketConnectTimeoutMs": millis}));
        let key = format!("timeout-{index}");
        e.prompt(&session, &key, "go").unwrap();
        assert_eq!(e.settled(&key), RunOutcome::Completed);
        assert_eq!(
            provider.requests()[index].options["websocketConnectTimeoutMs"],
            json!(millis)
        );
    }
}

#[test]
#[cfg(unix)]
fn custom_shell_setting_reaches_real_tools_and_changes_between_turns() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new();
    let shell = fx.dir.path().join("configured-shell");
    std::fs::write(
        &shell,
        "#!/bin/sh\nprintf configured >> shell-effects\nexec /bin/sh \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    write_settings(&fx, &json!({"shellPath":shell}));
    let provider = Scripted::new(|_, n| {
        if n % 2 == 0 {
            R::Tools(
                None,
                vec![("bash".into(), json!({"command":"printf tool-output"}))],
            )
        } else {
            R::Text("finished".into())
        }
    });
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "custom-shell");
    e.prompt(&session, "first", "run the configured shell")
        .unwrap();
    assert_eq!(e.settled("first"), RunOutcome::Completed);
    let effects = fx.project.join("shell-effects");
    assert_eq!(std::fs::read_to_string(&effects).unwrap(), "configured");
    write_settings(&fx, &json!({}));
    e.prompt(&session, "second", "use the default shell now")
        .unwrap();
    assert_eq!(e.settled("second"), RunOutcome::Completed);
    assert_eq!(std::fs::read_to_string(effects).unwrap(), "configured");
    let snapshot = e.snapshot(&session);
    let results: Vec<_> = snapshot
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Message {
                message:
                    m @ Message::ToolResult {
                        is_error: false, ..
                    },
                ..
            } => Some(m.text()),
            _ => None,
        })
        .collect();
    assert_eq!(results, ["tool-output", "tool-output"]);
}

fn write_models(fx: &Fixture, json: &serde_json::Value) {
    let dir = fx.dir.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("models.json"), json.to_string()).unwrap();
}

fn user_texts(req: &ProviderRequest) -> Vec<String> {
    req.messages
        .iter()
        .filter(|m| matches!(m, Message::User { .. }))
        .map(|m| m.text())
        .collect()
}

fn enqueue(e: &mut Engine, session: &str, key: &str, mode: InputMode, text: &str) {
    match e.call(Call::Enqueue {
        session: session.into(),
        request: key.into(),
        mode,
        text: text.into(),
        images: vec![],
    }) {
        Ok(Reply::Admitted { run: None, .. }) => {}
        other => panic!("{other:?}"),
    }
}

fn models(e: &mut Engine, session: Option<&str>) -> ModelsView {
    match e.call(Call::Models {
        session: session.map(str::to_owned),
    }) {
        Ok(Reply::Models(m)) => m,
        other => panic!("{other:?}"),
    }
}

fn session_model(e: &mut Engine, session: &str) -> (Option<String>, String) {
    let snap = e.snapshot(session);
    (snap.summary.model.clone(), snap.thinking.level.clone())
}

#[test]
fn thinking_cycles_match_the_executed_reference_service() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/native-thinking/reference.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let fx = Fixture::new();
        let mut config = fx.config();
        let model = &mut config.extra_models[0];
        if !case["model"].is_null() {
            model.reasoning = case["model"]["reasoning"].as_bool().unwrap();
            model.thinking_level_map = serde_json::from_value(
                case["model"]
                    .get("thinkingLevelMap")
                    .cloned()
                    .unwrap_or(json!({})),
            )
            .unwrap();
        }
        let mut e = fx.start_config(&Scripted::echo("done"), config);
        let session = e.create(&fx.project, "cycle-reference");
        if case["model"].is_null() {
            e.handle
                .shared()
                .store
                .set_session_field(
                    &session,
                    crate::store::SessionField::Model(Some("missing/model".into())),
                )
                .unwrap();
        }
        e.handle
            .shared()
            .store
            .set_session_field(
                &session,
                crate::store::SessionField::Thinking(case["current"].as_str().map(str::to_owned)),
            )
            .unwrap();
        let call = Call::CycleThinking {
            session: session.clone(),
        };
        let wire = serde_json::to_value(&call).unwrap();
        assert_eq!(wire["call"], "cycleThinking");
        let Reply::Thinking(view) = e.call(serde_json::from_value(wire).unwrap()).unwrap() else {
            panic!("{case}")
        };
        assert_eq!(
            serde_json::to_value(&view.levels).unwrap(),
            case["expected"]["levels"],
            "{case}"
        );
        assert_eq!(
            view.level,
            case["expected"]["thinking"].as_str().unwrap(),
            "{case}"
        );
        assert_eq!(
            e.handle
                .shared()
                .store
                .session(&session)
                .unwrap()
                .unwrap()
                .thinking
                .as_deref(),
            Some(view.level.as_str())
        );
    }
}

#[test]
fn thinking_cycles_are_serialized_notify_clients_and_survive_restart() {
    let fx = Fixture::new();
    let provider = Scripted::echo("done");
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "cycle");
    e.call(Call::SetThinking {
        session: session.clone(),
        level: "off".into(),
    })
    .unwrap();
    let before = e.snapshot(&session);
    let levels = before.thinking.levels.clone();
    assert!(levels.len() > 1);
    // Independent clients race on the same session; every accepted cycle must count.
    let connections: Vec<_> = (0..23).map(|_| e.handle.connect("cycle-racer")).collect();
    std::thread::scope(|scope| {
        for connection in connections {
            let session = &session;
            scope.spawn(move || {
                connection
                    .send(ClientFrame {
                        id: 1,
                        lane: Lane::Normal,
                        call: Call::CycleThinking {
                            session: session.clone(),
                        },
                    })
                    .unwrap();
                loop {
                    match connection.recv_timeout(Duration::from_secs(10)).unwrap() {
                        ServerFrame::Reply {
                            id: 1,
                            result: Ok(Reply::Thinking(_)),
                        } => break,
                        ServerFrame::Event { .. } => {}
                        other => panic!("{other:?}"),
                    }
                }
            });
        }
    });
    let expected = levels[23 % levels.len()].clone();
    let after = e.snapshot(&session);
    assert_eq!(after.thinking.level, expected);
    assert_eq!(after.revision, before.revision + 23);
    let changed: Vec<_> = e
        .events
        .iter()
        .filter_map(|event| match event {
            Event::Session {
                session: id,
                revision,
                change: SessionChange::ThinkingChanged { thinking },
            } if id == &session && *revision > before.revision => {
                Some((*revision, thinking.level.as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(changed.len(), 23);
    for (i, (revision, level)) in changed.iter().enumerate() {
        assert_eq!(*revision, before.revision + i as u64 + 1);
        assert_eq!(*level, levels[(i + 1) % levels.len()]);
    }
    e.prompt(&session, "coding", "use the cycled thinking level")
        .unwrap();
    assert_eq!(e.settled("coding"), RunOutcome::Completed);
    assert_eq!(provider.requests()[0].thinking.as_str(), expected);
    e.handle.shutdown(Duration::from_secs(1));
    drop(e);
    let mut e = fx.start(&Scripted::echo("resumed"));
    assert_eq!(e.snapshot(&session).thinking.level, expected);
}

#[test]
fn model_and_thinking_changes_commit_together_and_fail_without_partial_state() {
    let fx = Fixture::new();
    let mut config = fx.config();
    let mut plain = config.extra_models[0].clone();
    plain.id = "plain".into();
    plain.reasoning = false;
    config.extra_models.push(plain);
    let mut e = fx.start_config(&Scripted::echo("done"), config);
    let session = e.create(&fx.project, "atomic-config");
    let before = e.snapshot(&session);
    let cursor = e.handle.shared().store.catch_up(0, 256).unwrap().head;
    e.handle.shared().store.fail_next_write();
    assert_eq!(
        e.call(Call::SetModel {
            session: session.clone(),
            model: "scripted/plain".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::Storage
    );
    assert_eq!(e.snapshot(&session).summary, before.summary);
    assert_eq!(
        e.handle.shared().store.catch_up(cursor, 256).unwrap().head,
        cursor
    );
    e.call(Call::SetModel {
        session: session.clone(),
        model: "scripted/plain".into(),
    })
    .unwrap();
    let after = e.snapshot(&session);
    assert_eq!(after.summary.model.as_deref(), Some("scripted/plain"));
    assert_eq!(after.thinking.level, "off");
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(
        e.handle
            .shared()
            .store
            .catch_up(cursor, 256)
            .unwrap()
            .events
            .len(),
        1
    );
    e.handle.shared().store.fail_next_write();
    assert_eq!(
        e.call(Call::CycleThinking {
            session: session.clone()
        })
        .unwrap_err()
        .code,
        ErrorCode::Storage
    );
    assert_eq!(e.snapshot(&session).summary, after.summary);
    assert_eq!(
        e.call(Call::CycleThinking {
            session: "missing".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
}

#[test]
fn steering_mode_all_places_every_queued_steer_at_one_boundary() {
    let fx = Fixture::new();
    write_settings(&fx, &json!({"steeringMode": "all"}));
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, n| match n {
        0 => R::Gated(
            g.clone(),
            Box::new(R::Tools(None, vec![("no_such_tool".into(), json!({}))])),
        ),
        _ => R::Text("done".into()),
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "first").unwrap();
    enqueue(&mut e, &s, "s1", InputMode::Steer, "steer one");
    enqueue(&mut e, &s, "s2", InputMode::Steer, "steer two");
    gate.open();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let requests = provider.requests();
    assert_eq!(
        requests.len(),
        2,
        "both steers joined the same next request"
    );
    assert_eq!(
        user_texts(&requests[1]),
        vec!["first", "steer one", "steer two"]
    );
    assert!(e.snapshot(&s).queue.is_empty());
}

#[test]
fn steering_mode_one_at_a_time_places_only_the_first_steer() {
    let fx = Fixture::new();
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, n| match n {
        0 => R::Gated(
            g.clone(),
            Box::new(R::Tools(None, vec![("no_such_tool".into(), json!({}))])),
        ),
        _ => R::Text("done".into()),
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "first").unwrap();
    enqueue(&mut e, &s, "s1", InputMode::Steer, "steer one");
    enqueue(&mut e, &s, "s2", InputMode::Steer, "steer two");
    gate.open();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let requests = provider.requests();
    assert_eq!(user_texts(&requests[1]), vec!["first", "steer one"]);
    // The second steer starts the next run after the first completes.
    assert_eq!(e.settled("s2"), RunOutcome::Completed);
    assert_eq!(
        user_texts(provider.requests().last().unwrap()),
        vec!["first", "steer one", "steer two"]
    );
}

#[test]
fn follow_up_mode_all_starts_one_run_with_every_queued_follow_up() {
    let fx = Fixture::new();
    write_settings(&fx, &json!({"followUpMode": "all"}));
    let gate = Gate::new();
    let g = gate.clone();
    let provider = Scripted::new(move |_, n| match n {
        0 => R::Gated(g.clone(), Box::new(R::Text("one".into()))),
        _ => R::Text("two".into()),
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "first").unwrap();
    enqueue(&mut e, &s, "f1", InputMode::FollowUp, "then a");
    enqueue(&mut e, &s, "f2", InputMode::FollowUp, "then b");
    gate.open();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    assert_eq!(e.settled("f1"), RunOutcome::Completed);
    std::thread::sleep(Duration::from_millis(200));
    let requests = provider.requests();
    assert_eq!(requests.len(), 2, "one run for both follow-ups");
    assert_eq!(user_texts(&requests[1]), vec!["first", "then a", "then b"]);
    assert!(e.snapshot(&s).queue.is_empty());
}

fn is_summary(req: &ProviderRequest) -> bool {
    req.system == crate::context::compaction::SUMMARIZATION_SYSTEM_PROMPT
}

#[test]
fn compaction_model_overrides_apply_to_the_session_model() {
    let fx = Fixture::new();
    // The ordinary setting keeps everything; the override for this model keeps almost nothing.
    write_settings(
        &fx,
        &json!({"compaction": {"keepRecentTokens": 1_000_000, "reserveTokens": 100,
            "modelOverrides": {"scripted/scripted": {"keepRecentTokens": 5}}}}),
    );
    let provider = Scripted::new(|req, _| {
        if is_summary(req) {
            R::Text("## Goal\nsummary".into())
        } else {
            R::Text("reply".into())
        }
    });
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    for i in 0..3 {
        e.prompt(&s, &format!("r{i}"), &format!("turn number {i} with words"))
            .unwrap();
        e.settled(&format!("r{i}"));
    }
    e.call(Call::Compact {
        session: s.clone(),
        instructions: None,
    })
    .unwrap();
    let summary = provider.requests().into_iter().find(is_summary).unwrap();
    // `reserveTokens` (not overridden) still sizes the summary budget: 100 * 4 / 5.
    assert_eq!(summary.max_tokens, Some(80));

    // Without the override the same history is too short to cut.
    let fx2 = Fixture::new();
    write_settings(
        &fx2,
        &json!({"compaction": {"keepRecentTokens": 1_000_000,
            "modelOverrides": {"other/model": {"keepRecentTokens": 5}}}}),
    );
    let mut e2 = fx2.start(&Scripted::echo("reply"));
    let s2 = e2.create(&fx2.project, "c");
    for i in 0..3 {
        e2.prompt(
            &s2,
            &format!("r{i}"),
            &format!("turn number {i} with words"),
        )
        .unwrap();
        e2.settled(&format!("r{i}"));
    }
    let err = e2
        .call(Call::Compact {
            session: s2,
            instructions: None,
        })
        .unwrap_err();
    assert!(err.message.starts_with("Nothing to compact"), "{err:?}");
}

#[test]
fn invalid_compaction_policy_fails_before_generation_and_live_project_repair_recovers() {
    let fx = Fixture::new();
    let provider = Scripted::new(|req, _| {
        if is_summary(req) {
            R::Text("## Goal\nsummary".into())
        } else {
            R::Text("reply".into())
        }
    });
    write_settings(
        &fx,
        &json!({"steeringMode":"all","compaction":{"enabled":false,"reserveTokens":100,"keepRecentTokens":5}}),
    );
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "compaction validation");
    for i in 0..3 {
        let request = format!("before-{i}");
        e.prompt(&session, &request, "A turn with words to summarize later.")
            .unwrap();
        assert_eq!(e.settled(&request), RunOutcome::Completed);
    }
    let original = json!({"steeringMode":"all","compaction":{"enabled":false,"reserveTokens":100,"keepRecentTokens":5,
        "modelOverrides":{"scripted/scripted":{"keepRecentTokens":-3},"other/model":null}}});
    write_settings(&fx, &original);
    let calls = provider.requests().len();
    e.prompt(
        &session,
        "invalid",
        "Do not generate with invalid settings.",
    )
    .unwrap();
    let expected = "Invalid compaction.modelOverrides[\"scripted/scripted\"].keepRecentTokens setting: -3. Expected a non-negative safe integer.";
    assert_eq!(
        e.settled("invalid"),
        RunOutcome::ModelError {
            detail: expected.into()
        }
    );
    assert_eq!(provider.requests().len(), calls);
    let error = e
        .call(Call::Compact {
            session: session.clone(),
            instructions: None,
        })
        .unwrap_err();
    assert!(error.message.contains(expected), "{}", error.message);
    assert_eq!(provider.requests().len(), calls);
    let project_settings = fx.project.join(".pi/settings.json");
    std::fs::create_dir_all(project_settings.parent().unwrap()).unwrap();
    std::fs::write(
        &project_settings,
        json!({"compaction":{"modelOverrides":{"scripted/scripted":{"keepRecentTokens":5}}}})
            .to_string(),
    )
    .unwrap();
    e.prompt(&session, "repaired", "Continue after repairing settings.")
        .unwrap();
    assert_eq!(e.settled("repaired"), RunOutcome::Completed);
    e.call(Call::Compact {
        session,
        instructions: None,
    })
    .unwrap();
    let summary = provider.requests().into_iter().find(is_summary).unwrap();
    assert_eq!(summary.max_tokens, Some(80));
    let (loaded, warnings) =
        crate::config::Settings::load(&fx.dir.path().join("config"), Some(&fx.project));
    assert!(warnings.is_empty());
    assert_eq!(loaded.steering_mode, crate::config::QueueDelivery::All);
    assert!(!loaded.compaction.enabled);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(fx.dir.path().join("config/settings.json")).unwrap()
        )
        .unwrap(),
        original
    );
}

fn local_models(base: &str) -> serde_json::Value {
    json!({"providers": {"local": {
    "baseUrl": base, "api": "openai-completions", "apiKey": "k",
    "models": [
        {"id": "alpha", "reasoning": true},
        {"id": "beta", "reasoning": true},
        {"id": "gamma", "reasoning": true}
    ]}}})
}

#[test]
fn a_provider_configured_after_start_appears_after_refresh_models() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("x"));
    let before = models(&mut e, None);
    assert!(!before.models.iter().any(|m| m.key == "local/alpha"));
    write_models(&fx, &local_models("http://127.0.0.1:9/v1"));
    // Nothing changes until a refresh.
    assert!(
        !models(&mut e, None)
            .models
            .iter()
            .any(|m| m.key == "local/alpha")
    );
    e.events.clear();
    assert!(matches!(e.call(Call::RefreshModels), Ok(Reply::Ok)));
    e.until("ModelsChanged", |ev| {
        ev.iter().any(|e| matches!(e, Event::ModelsChanged))
    });
    let after = models(&mut e, None);
    let alpha = after
        .models
        .iter()
        .find(|m| m.key == "local/alpha")
        .expect("listed after refresh");
    assert!(alpha.available);
}

#[test]
fn refreshed_catalog_models_are_selectable_and_resume_offline_after_restart() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let provider = rt.block_on(MockServer::start(|_,_| MockResponse::sse(vec![
        "data: {\"id\":\"answer\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"catalog model worked\"},\"finish_reason\":null}]}\n\n".into(),
        "data: {\"id\":\"answer\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".into(),
    ])));
    let base = provider.url.clone();
    let catalog = rt.block_on(MockServer::start(move |_,_| {
        MockResponse::json(200,&json!([{"id":"catalog-test","api":"openai-completions","baseUrl":format!("{base}/v1"),"contextWindow":200000,"maxTokens":4096}]))
            .with_header("last-modified","Sun, 11 Oct 2026 12:00:00 GMT")
            .with_header("etag","\"native-test\"")
    }));
    let fx = Fixture::new();
    let mut config = fx.config();
    config.env = Env::isolated([("OPENAI_API_KEY".into(), "private-provider-key".into())].into());
    config.catalog_base_url = catalog.url.clone();
    let mut e = fx.start_config(&Scripted::echo("unused"), config.clone());
    let session = e.create(&fx.project, "catalog-session");
    assert!(
        !models(&mut e, None)
            .models
            .iter()
            .any(|m| m.key == "openai/catalog-test")
    );
    e.call(Call::RefreshModels).unwrap();
    e.until("catalog invalidation", |events| {
        events.iter().any(|ev| matches!(ev, Event::ModelsChanged))
    });
    assert!(
        models(&mut e, None)
            .models
            .iter()
            .any(|m| m.key == "openai/catalog-test" && m.available)
    );
    e.call(Call::SetModel {
        session: session.clone(),
        model: "openai/catalog-test".into(),
    })
    .unwrap();
    e.prompt(&session, "live", "use the refreshed model")
        .unwrap();
    assert_eq!(e.settled("live"), RunOutcome::Completed);
    assert_eq!(provider.requests()[0].json()["model"], "catalog-test");
    assert_eq!(
        provider.requests()[0].header("authorization"),
        Some("Bearer private-provider-key")
    );
    assert!(catalog.requests()[0].header("authorization").is_none());
    e.handle.shutdown(Duration::from_secs(1));
    drop(e);
    config.env = Env::isolated(
        [
            ("OPENAI_API_KEY".into(), "private-provider-key".into()),
            ("PI_OFFLINE".into(), "".into()),
        ]
        .into(),
    );
    let mut e = fx.start_config(&Scripted::echo("unused"), config);
    assert_eq!(
        e.snapshot(&session).summary.model.as_deref(),
        Some("openai/catalog-test")
    );
    e.call(Call::RefreshModels).unwrap();
    assert_eq!(
        catalog.requests().len(),
        1,
        "offline disables catalog traffic, not ordinary coding"
    );
    e.prompt(&session, "resumed", "continue offline catalog selection")
        .unwrap();
    assert_eq!(e.settled("resumed"), RunOutcome::Completed);
    assert_eq!(provider.requests().len(), 2);
}

#[test]
fn enabled_models_scope_the_listing_and_the_initial_model_with_pattern_thinking() {
    let fx = Fixture::new();
    write_models(&fx, &local_models("http://127.0.0.1:9/v1"));
    write_settings(
        &fx,
        &json!({
            "enabledModels": ["local/beta:high", "*gamma", "no-such-model"],
            "defaultProvider": "local", "defaultModel": "alpha",
            "defaultThinkingLevel": "low",
        }),
    );
    let mut cfg = fx.config();
    cfg.default_model = None;
    let mut e = fx.start_config(&Scripted::echo("x"), cfg);
    let view = models(&mut e, None);
    let keys: Vec<&str> = view.models.iter().map(|m| m.key.as_str()).collect();
    assert_eq!(keys, vec!["local/beta", "local/gamma"]);
    assert!(
        view.warnings
            .contains(&"No models match pattern \"no-such-model\"".to_owned()),
        "{:?}",
        view.warnings
    );
    // The saved default (alpha) is out of scope: the first scoped model, with its `:high`.
    let s = e.create(&fx.project, "c");
    assert_eq!(
        session_model(&mut e, &s),
        (Some("local/beta".into()), "high".into())
    );

    // The saved default is used when it is in scope; with no pattern level the global default
    // thinking applies.
    write_settings(
        &fx,
        &json!({"enabledModels": ["local/beta:high", "local/gamma"],
            "defaultProvider": "local", "defaultModel": "gamma",
            "defaultThinkingLevel": "low"}),
    );
    let s2 = e.create(&fx.project, "c2");
    assert_eq!(
        session_model(&mut e, &s2),
        (Some("local/gamma".into()), "low".into())
    );

    // Choosing a model outside a non-empty scope adds it to `enabledModels` (reference
    // `_addPersistedDefaultToNonEmptyScope`).
    e.call(Call::SetModel {
        session: s2.clone(),
        model: "local/alpha".into(),
    })
    .unwrap();
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fx.dir.path().join("config/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved["enabledModels"],
        json!(["local/beta:high", "local/gamma", "local/alpha"])
    );
    let keys: Vec<String> = models(&mut e, Some(&s2))
        .models
        .into_iter()
        .map(|m| m.key)
        .collect();
    assert_eq!(keys, vec!["local/beta", "local/gamma", "local/alpha"]);
}

#[test]
fn model_thinking_levels_apply_to_new_sessions_and_model_switches() {
    let fx = Fixture::new();
    write_models(&fx, &local_models("http://127.0.0.1:9/v1"));
    write_settings(
        &fx,
        &json!({"defaultThinkingLevel": "minimal",
            "modelThinkingLevels": {"scripted/scripted": "high", "local/beta": "low"}}),
    );
    let mut e = fx.start(&Scripted::echo("x"));
    // The engine's configured model gets its per-model level.
    let s = e.create(&fx.project, "c");
    assert_eq!(
        session_model(&mut e, &s),
        (Some("scripted/scripted".into()), "high".into())
    );
    // Switching: the target's own level, else the global default.
    e.call(Call::SetModel {
        session: s.clone(),
        model: "local/beta".into(),
    })
    .unwrap();
    assert_eq!(session_model(&mut e, &s).1, "low");
    e.call(Call::SetModel {
        session: s.clone(),
        model: "local/gamma".into(),
    })
    .unwrap();
    assert_eq!(session_model(&mut e, &s).1, "minimal");
}

/// Separate direct/proxied origins return a non-retryable 400; a real CONNECT proxy routes
/// to the second origin so the test can distinguish direct and tunneled traffic.
fn servers(
    rt: &tokio::runtime::Runtime,
) -> (MockServer, crate::provider::mock::TunnelProxy, MockServer) {
    let refuse = |_: &crate::provider::mock::Recorded, _| {
        MockResponse::json(400, &json!({"error": {"message": "refused by test"}}))
    };
    let provider = rt.block_on(MockServer::start(refuse));
    let proxied = rt.block_on(MockServer::start(refuse));
    let proxy = rt.block_on(crate::provider::mock::TunnelProxy::start(
        &proxied.url,
        false,
    ));
    (provider, proxy, proxied)
}

fn local_session(fx: &Fixture, provider_url: &str, env: Env) -> (Engine, String) {
    write_models(fx, &local_models(&format!("{provider_url}/v1")));
    let mut cfg = fx.config();
    cfg.env = env;
    cfg.default_model = Some("local/alpha".into());
    let mut e = fx.start_config(&Scripted::echo("x"), cfg);
    let s = e.create(&fx.project, "c");
    (e, s)
}

#[test]
fn engine_env_proxy_variables_route_model_requests() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (provider, proxy, proxied) = servers(&rt);
    let fx = Fixture::new();
    let env = Env::isolated(HashMap::from([("HTTP_PROXY".into(), proxy.url.clone())]));
    let (mut e, s) = local_session(&fx, &provider.url, env);
    e.prompt(&s, "r1", "hi").unwrap();
    assert!(matches!(e.settled("r1"), RunOutcome::ModelError { .. }));
    assert!(provider.requests().is_empty());
    let seen = proxy.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, provider.url.strip_prefix("http://").unwrap());
    assert_eq!(seen[0].method, "CONNECT");
    assert_eq!(proxied.requests().len(), 1);
    assert_eq!(proxied.requests()[0].path, "/v1/chat/completions");
}

#[test]
fn settings_http_proxy_is_applied_without_a_restart_and_unsupported_proxies_fail_clearly() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (provider, proxy, proxied) = servers(&rt);
    let fx = Fixture::new();
    let (mut e, s) = local_session(&fx, &provider.url, Env::isolated(HashMap::new()));
    e.prompt(&s, "r1", "hi").unwrap();
    e.settled("r1");
    assert_eq!(provider.requests().len(), 1, "direct without a proxy");

    // A global `httpProxy` takes effect at the next request.
    write_settings(&fx, &json!({"httpProxy": proxy.url}));
    e.prompt(&s, "r2", "again").unwrap();
    e.settled("r2");
    assert_eq!(provider.requests().len(), 1);
    assert_eq!(proxy.requests().len(), 1);

    // A SOCKS proxy is refused with the reference message, never bypassed.
    write_settings(&fx, &json!({"httpProxy": "socks5://127.0.0.1:1080"}));
    e.prompt(&s, "r3", "third").unwrap();
    assert_eq!(
        e.settled("r3"),
        RunOutcome::ModelError {
            detail: format!(
                "{} Got socks5:",
                crate::provider::proxy::UNSUPPORTED_PROXY_PROTOCOL_MESSAGE
            )
        }
    );
    assert_eq!(provider.requests().len(), 1);
    assert_eq!(proxy.requests().len(), 1);

    // `httpProxy` is global-only (reference `getGlobalSettings().httpProxy`): a project file
    // cannot set it.
    write_settings(&fx, &json!({}));
    std::fs::create_dir_all(fx.project.join(".pi")).unwrap();
    std::fs::write(
        fx.project.join(".pi/settings.json"),
        json!({"httpProxy": proxy.url}).to_string(),
    )
    .unwrap();
    e.prompt(&s, "r4", "fourth").unwrap();
    e.settled("r4");
    assert_eq!(provider.requests().len(), 2);
    assert_eq!(proxied.requests().len(), 1);
    assert_eq!(proxy.requests().len(), 1);
}

#[test]
fn the_request_timeout_follows_provider_timeout_then_http_idle_timeout() {
    let fx = Fixture::new();
    write_settings(&fx, &json!({"httpIdleTimeoutMs": 45_000}));
    let provider = Scripted::echo("x");
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "hi").unwrap();
    e.settled("r1");
    write_settings(
        &fx,
        &json!({"httpIdleTimeoutMs": 0, "retry": {"provider": {"timeoutMs": 9000}}}),
    );
    e.prompt(&s, "r2", "hi").unwrap();
    e.settled("r2");
    write_settings(&fx, &json!({"httpIdleTimeoutMs": "disabled"}));
    e.prompt(&s, "r3", "hi").unwrap();
    e.settled("r3");
    let timeouts: Vec<Duration> = provider.requests().iter().map(|r| r.timeout).collect();
    assert_eq!(
        timeouts,
        vec![
            Duration::from_secs(45),
            Duration::from_secs(9),
            Duration::from_secs(i32::MAX as u64)
        ]
    );
}

#[test]
fn new_signin_attempts_refresh_http_timeout_settings_before_opening_authorization() {
    let fx = Fixture::new();
    let mut e = fx.start(&Scripted::echo("unused"));
    write_settings(&fx, &json!({"httpIdleTimeoutMs":null}));
    for call in [
        Call::AuthStart {
            provider: "anthropic".into(),
        },
        Call::AuthReuse {
            provider: "anthropic".into(),
        },
    ] {
        let error = e.call(call).unwrap_err();
        assert_eq!(error.code, ErrorCode::Invalid);
        assert_eq!(error.message, "Invalid httpIdleTimeoutMs setting: null");
    }
    match e.call(Call::AuthState).unwrap() {
        Reply::Auth(state) => assert!(state.attempt.is_none()),
        _ => panic!("expected auth state"),
    }
    write_settings(&fx, &json!({"httpIdleTimeoutMs":0}));
    let attempt = match e
        .call(Call::AuthStart {
            provider: "anthropic".into(),
        })
        .unwrap()
    {
        Reply::Auth(state) => state.attempt.expect("started without restarting engine"),
        _ => panic!("expected auth state"),
    };
    e.call(Call::AuthCancel { attempt }).unwrap();
    assert_eq!(
        e.handle.shared().refresh_net_snapshot().0.validation_error,
        None
    );
}

#[test]
fn invalid_http_timeout_refuses_generation_until_live_global_and_project_settings_are_repaired() {
    let fx = Fixture::new();
    let provider = Scripted::echo("ok");
    let mut e = fx.start(&provider);
    let session = e.create(&fx.project, "timeout validation");
    e.prompt(&session, "valid", "hello").unwrap();
    assert_eq!(e.settled("valid"), RunOutcome::Completed);
    write_settings(
        &fx,
        &json!({"httpIdleTimeoutMs":null,"retry":{"provider":{"timeoutMs":7000}}}),
    );
    let project = fx.project.join(".pi/settings.json");
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    std::fs::write(&project, json!({"httpIdleTimeoutMs":500}).to_string()).unwrap();
    e.prompt(&session, "invalid-global", "must not dispatch")
        .unwrap();
    assert_eq!(
        e.settled("invalid-global"),
        RunOutcome::ModelError {
            detail: "Invalid httpIdleTimeoutMs setting: null".into()
        }
    );
    assert_eq!(
        provider.requests().len(),
        1,
        "provider override cannot hide invalid global dispatcher settings"
    );
    write_settings(&fx, &json!({"httpIdleTimeoutMs":1500}));
    e.prompt(&session, "repaired-global", "continue").unwrap();
    assert_eq!(e.settled("repaired-global"), RunOutcome::Completed);
    assert_eq!(
        provider.requests().last().unwrap().timeout,
        Duration::from_millis(500)
    );
    let (net, _) = e.handle.shared().refresh_net_snapshot();
    assert_eq!(net.idle, Some(Duration::from_millis(1500)));
    assert_eq!(net.validation_error, None);
    std::fs::write(&project, json!({"httpIdleTimeoutMs":-9}).to_string()).unwrap();
    e.prompt(&session, "invalid-project", "must not dispatch")
        .unwrap();
    assert_eq!(
        e.settled("invalid-project"),
        RunOutcome::ModelError {
            detail: "Invalid httpIdleTimeoutMs setting: -9".into()
        }
    );
    assert_eq!(provider.requests().len(), 2);
    std::fs::write(
        &project,
        json!({"httpIdleTimeoutMs":"disabled"}).to_string(),
    )
    .unwrap();
    e.prompt(&session, "repaired-project", "continue").unwrap();
    assert_eq!(e.settled("repaired-project"), RunOutcome::Completed);
}

#[test]
fn radius_discovery_supports_a_real_tool_turn_and_cached_resume_without_catalog_network() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let messages = Arc::new(AtomicUsize::new(0));
    let counted = messages.clone();
    let server=rt.block_on(MockServer::start(move |req,_| {
        assert_eq!(req.header("authorization"),Some("Bearer radius-test-key"));
        if req.path=="/v1/config" {
            return MockResponse::json(200,&json!({"baseUrl":format!("http://{}/v1",req.header("host").unwrap()),"models":[{"id":"discovered","name":"Discovered gateway model","reasoning":false,"input":["text"],"cost":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0},"contextWindow":200000,"maxTokens":4096}]}));
        }
        assert_eq!(req.path,"/v1/messages");
        assert_eq!(req.json()["model"],"discovered");
        let n=counted.fetch_add(1,Ordering::SeqCst);
        let events = if n==0 {
            vec![json!({"type":"toolcall_start","contentIndex":0,"id":"write-proof","toolName":"write"}),json!({"type":"toolcall_end","contentIndex":0,"toolCall":{"type":"toolCall","id":"write-proof","name":"write","arguments":{"path":"radius-proof.txt","content":"native gateway tool executed\n"}}}),json!({"type":"done","reason":"toolUse"})]
        } else {
            assert!(req.json()["context"]["messages"].as_array().unwrap().iter().any(|m|m["role"]=="toolResult" && m["toolCallId"]=="write-proof"));
            vec![json!({"type":"text_start","contentIndex":0}),json!({"type":"text_end","contentIndex":0,"content":"Gateway coding turn completed"}),json!({"type":"done","reason":"stop"})]
        };
        MockResponse::sse(events.into_iter().map(|v|format!("data: {v}\n\n")).collect())
    }));
    let fx = Fixture::new();
    let mut cfg = fx.config();
    cfg.env = Env::isolated(
        [
            ("PI_RADIUS_GATEWAY".into(), server.url.clone()),
            ("RADIUS_API_KEY".into(), "radius-test-key".into()),
        ]
        .into(),
    );
    let scripted = Scripted::echo("unused");
    let mut e = fx.start_config(&scripted, cfg.clone());
    e.call(Call::RefreshModels).unwrap();
    assert!(
        models(&mut e, None)
            .models
            .iter()
            .any(|m| m.key == "radius/discovered" && m.available)
    );
    let session = e.create(&fx.project, "radius-session");
    e.call(Call::SetModel {
        session: session.clone(),
        model: "radius/discovered".into(),
    })
    .unwrap();
    e.prompt(&session, "write", "write a file using the discovered model")
        .unwrap();
    assert_eq!(e.settled("write"), RunOutcome::Completed);
    assert_eq!(
        std::fs::read_to_string(fx.project.join("radius-proof.txt")).unwrap(),
        "native gateway tool executed\n"
    );
    e.handle.shutdown(Duration::from_secs(1));
    drop(e);
    cfg.env = Env::isolated(
        [
            ("PI_RADIUS_GATEWAY".into(), server.url.clone()),
            ("RADIUS_API_KEY".into(), "radius-test-key".into()),
            ("PI_OFFLINE".into(), "".into()),
        ]
        .into(),
    );
    let mut reopened = fx.start_config(&scripted, cfg);
    reopened.call(Call::RefreshModels).unwrap();
    reopened
        .prompt(&session, "resume", "continue after restart")
        .unwrap();
    assert_eq!(reopened.settled("resume"), RunOutcome::Completed);
    assert_eq!(messages.load(Ordering::SeqCst), 3);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r.path == "/v1/config")
            .count(),
        1
    );
    assert!(scripted.requests().is_empty());
    reopened.handle.shutdown(Duration::from_secs(1));
}

#[test]
fn federation_environment_supports_native_turns_and_reexchange_after_engine_restart() {
    let http_runtime = tokio::runtime::Runtime::new().unwrap();
    let server = http_runtime.block_on(MockServer::start(|r, _| {
        if r.path == "/v1/oauth/token" {
            MockResponse::json(200, &json!({"access_token":r.json()["assertion"], "expires_in":3600, "token_type":"Bearer"}))
        } else {
            MockResponse::sse(vec![
                json!({"type":"message_start","message":{"id":"m","model":"scripted","usage":{"input_tokens":1,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"federated answer"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
                json!({"type":"message_stop"})
            ].into_iter().map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap())).collect())
        }
    }));
    let fx = Fixture::new();
    let mut config = fx.config();
    let identity = fx.dir.path().join("identity-token");
    std::fs::write(&identity, "initial-assertion").unwrap();
    let mut model = crate::provider::scripted::model();
    model.provider = "anthropic".into();
    model.api = "anthropic-messages".into();
    model.base_url = server.url.clone();
    config.default_model = Some(model.key());
    config.extra_models.push(model);
    config.env = Env::isolated(
        [
            ("ANTHROPIC_FEDERATION_RULE_ID".into(), "rule".into()),
            ("ANTHROPIC_ORGANIZATION_ID".into(), "org".into()),
            (
                "ANTHROPIC_IDENTITY_TOKEN_FILE".into(),
                identity.display().to_string(),
            ),
        ]
        .into(),
    );
    let scripted = Scripted::echo("must not be used");
    let mut e = fx.start_config(&scripted, config.clone());
    let session = e.create(&fx.project, "federation");
    for key in ["first", "second"] {
        e.prompt(&session, key, "hello").unwrap();
        assert_eq!(e.settled(key), RunOutcome::Completed);
    }
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r.path == "/v1/oauth/token")
            .count(),
        1
    );
    e.handle.shutdown(Duration::from_secs(1));
    drop(e);
    std::fs::write(&identity, "rotated-after-restart").unwrap();
    let mut e = fx.start_config(&scripted, config);
    e.prompt(&session, "reopened", "continue").unwrap();
    assert_eq!(e.settled("reopened"), RunOutcome::Completed);
    let requests = server.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.path == "/v1/oauth/token")
            .count(),
        2
    );
    assert_eq!(
        requests.last().unwrap().header("authorization"),
        Some("Bearer rotated-after-restart")
    );
    assert!(scripted.requests().is_empty());
    e.handle.shutdown(Duration::from_secs(1));
}
