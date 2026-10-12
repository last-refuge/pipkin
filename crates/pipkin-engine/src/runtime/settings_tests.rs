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

/// A local "provider" and a recording "proxy"; both answer with a non-retryable 400 so a run
/// settles at once.
fn servers(rt: &tokio::runtime::Runtime) -> (MockServer, MockServer) {
    let refuse = |_: &crate::provider::mock::Recorded, _| {
        MockResponse::json(400, &json!({"error": {"message": "refused by test"}}))
    };
    (
        rt.block_on(MockServer::start(refuse)),
        rt.block_on(MockServer::start(refuse)),
    )
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
    let (provider, proxy) = servers(&rt);
    let fx = Fixture::new();
    let env = Env::isolated(HashMap::from([("HTTP_PROXY".into(), proxy.url.clone())]));
    let (mut e, s) = local_session(&fx, &provider.url, env);
    e.prompt(&s, "r1", "hi").unwrap();
    assert!(matches!(e.settled("r1"), RunOutcome::ModelError { .. }));
    assert!(provider.requests().is_empty());
    let seen = proxy.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].path,
        format!("{}/v1/chat/completions", provider.url)
    );
}

#[test]
fn settings_http_proxy_is_applied_without_a_restart_and_unsupported_proxies_fail_clearly() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (provider, proxy) = servers(&rt);
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
