//! MCP against the scripted stdio server in `examples/mcp_fixture.rs` (built by
//! `cargo test`): discovery and calls through the client contract, classified errors,
//! lost replies and restarts (no blind re-send), catalog-change notifications, the idle
//! lifecycle, start failures, credential references and the `McpServers` view.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::json;

use super::tests::{Engine, Fixture};
use crate::config::Env;
use crate::provider::ProviderRequest;
use crate::provider::scripted::{Reply as R, Scripted};

/// The fixture server binary, built once per test binary with the same cargo (`cargo test`
/// only type-checks examples), located from cargo's own report rather than a guessed path.
pub(super) fn fixture_bin() -> String {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(|| {
        let out = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "--example",
                "mcp_fixture",
                "--message-format=json",
                "--manifest-path",
            ])
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .stderr(std::process::Stdio::inherit())
            .output()
            .expect("cargo runs");
        assert!(out.status.success(), "building the fixture server failed");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find_map(|msg| {
                (msg["reason"] == "compiler-artifact" && msg["target"]["name"] == "mcp_fixture")
                    .then(|| msg["executable"].as_str().map(str::to_owned))
                    .flatten()
            })
            .expect("cargo reported the fixture executable")
    })
    .clone()
}

pub(super) fn write_settings(fx: &Fixture, body: serde_json::Value) {
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(fx.dir.path().join("config/settings.json"), body.to_string()).unwrap();
}

pub(super) fn server(extra: serde_json::Value) -> serde_json::Value {
    let mut body = json!({"command": fixture_bin()});
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        b.extend(e.clone());
    }
    body
}

fn last_is_user(req: &ProviderRequest) -> bool {
    matches!(
        req.messages
            .iter()
            .rev()
            .find(|m| !matches!(m, Message::System(_))),
        Some(Message::User { .. })
    )
}

fn last_tool_result(req: &ProviderRequest) -> Option<String> {
    req.messages.iter().rev().find_map(|m| match m {
        Message::ToolResult { .. } => Some(m.text()),
        _ => None,
    })
}

/// After a user message `tool args-json`, call that tool; then echo its result.
pub(super) fn calling_from_prompt() -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    move |req, _| {
        if last_is_user(req) {
            let text = req
                .messages
                .iter()
                .rev()
                .find_map(|m| match m {
                    Message::User { .. } => Some(m.text()),
                    _ => None,
                })
                .unwrap_or_default();
            let (tool, args) = text.split_once(' ').unwrap_or((&text, "{}"));
            R::Tools(
                None,
                vec![(tool.to_owned(), serde_json::from_str(args).unwrap())],
            )
        } else {
            R::Text(format!(
                "model saw: {}",
                last_tool_result(req).unwrap_or_default()
            ))
        }
    }
}

fn tool_results(snap: &SessionSnapshot) -> Vec<(String, bool, String, Option<serde_json::Value>)> {
    snap.entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message {
                message:
                    m @ Message::ToolResult {
                        tool_name,
                        is_error,
                        extra,
                        ..
                    },
                ..
            } if tool_name.starts_with("mcp_") => Some((
                tool_name.clone(),
                *is_error,
                m.text(),
                extra.get("details").cloned(),
            )),
            _ => None,
        })
        .collect()
}

pub(super) fn run(
    e: &mut Engine,
    s: &str,
    key: &str,
    prompt: &str,
) -> (bool, String, serde_json::Value) {
    e.prompt(s, key, prompt).unwrap();
    assert_eq!(e.settled(key), RunOutcome::Completed);
    let snap = e.snapshot(s);
    let (_, is_error, text, details) = tool_results(&snap).into_iter().last().unwrap();
    (is_error, text, details.unwrap()["mcp"].clone())
}

pub(super) fn servers(e: &mut Engine, s: &str) -> McpServersView {
    match e.call(Call::McpServers {
        session: Some(s.into()),
    }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    }
}

fn server_state(e: &mut Engine, s: &str, name: &str) -> McpServerView {
    servers(e, s)
        .servers
        .into_iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no server {name}"))
}

fn wait_state(e: &mut Engine, s: &str, name: &str, state: &str) -> McpServerView {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let view = server_state(e, s, name);
        if view.state == state {
            return view;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {name} to be {state}: {view:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn search_describe_and_call_reach_the_fixture_server() {
    let fx = Fixture::new();
    let mut config = fx.config();
    config.env = Env::isolated(HashMap::from([(
        "FX_GREETING".to_owned(),
        "hello-from-env".to_owned(),
    )]));
    write_settings(
        &fx,
        json!({"mcp": {"servers": {"fx": server(json!({
            "env": {"GREETING": "$FX_GREETING", "MCP_FIXTURE_ENV": "GREETING"}
        }))}}}),
    );
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start_config(&provider, config);
    let s = e.create(&fx.project, "c");
    // Before any use the server is configured but stopped, and secrets are not shown.
    let view = servers(&mut e, &s);
    assert_eq!(view.servers.len(), 1);
    assert_eq!(view.servers[0].state, "stopped");
    assert_eq!(view.servers[0].scope, "user");
    assert_eq!(view.servers[0].tools, None);
    assert_eq!(
        view.servers[0].env,
        ["GREETING=$FX_GREETING", "MCP_FIXTURE_ENV=…"]
    );
    assert!(view.servers[0].command.ends_with("mcp_fixture"));
    assert!(view.warnings.is_empty());

    let (is_error, text, details) = run(&mut e, &s, "r1", r#"mcp_search {"query": "echo"}"#);
    assert!(!is_error, "{text}");
    assert!(text.contains("1 MCP tool(s) matching \"echo\""), "{text}");
    assert!(text.contains("- fx/echo: Echo the text back"), "{text}");
    assert_eq!(details["outcome"], "search");
    assert_eq!(details["tools"][0]["qualified"], "fx/echo");
    let revision = details["tools"][0]["revision"].as_str().unwrap().to_owned();
    assert_eq!(revision.len(), 16);
    let view = server_state(&mut e, &s, "fx");
    assert_eq!(view.state, "ready");
    assert_eq!(view.tools, Some(9));
    assert_eq!(view.revision, revision);

    let (is_error, text, details) = run(&mut e, &s, "r2", r#"mcp_describe {"tool": "fx/echo"}"#);
    assert!(!is_error, "{text}");
    assert!(text.starts_with("fx/echo: Echo the text back"), "{text}");
    assert!(text.contains("Parameters (JSON schema)"), "{text}");
    assert_eq!(details["outcome"], "describe");
    assert_eq!(
        details["inputSchema"]["properties"]["text"]["type"],
        "string"
    );
    assert_eq!(details["inputSchema"]["required"], json!(["text"]));

    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r3",
        r#"mcp_call {"tool": "fx/echo", "arguments": {"text": "hi there"}}"#,
    );
    assert!(!is_error, "{text}");
    assert_eq!(text, "hi there env:hello-from-env");
    assert_eq!(details["outcome"], "call");
    assert_eq!(details["tool"]["server"], "fx");
    assert_eq!(details["tool"]["tool"], "echo");
    assert_eq!(details["tool"]["revision"], revision);
    // The model's next request saw the result; the secret value never reached a view.
    let last = provider.requests().into_iter().last().unwrap();
    assert!(
        last.messages
            .iter()
            .any(|m| m.text().contains("hi there env:hello-from-env"))
    );
    let dump = serde_json::to_string(&servers(&mut e, &s)).unwrap();
    assert!(!dump.contains("hello-from-env"), "{dump}");
    // An empty query lists everything; a server filter limits it.
    let (is_error, text, _) = run(&mut e, &s, "r4", r#"mcp_search {"server": "fx"}"#);
    assert!(!is_error && text.contains("9 MCP tool(s) on fx"), "{text}");
    let (is_error, text, details) = run(&mut e, &s, "r5", r#"mcp_search {"query": "zzz"}"#);
    assert!(!is_error && text.contains("No tools matched."), "{text}");
    assert_eq!(details["tools"], json!([]));
}

#[test]
fn errors_are_classified_and_remote_tool_errors_stay_separate_from_transport_failures() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    // Nothing configured.
    let (is_error, text, details) = run(&mut e, &s, "r0", r#"mcp_search {"query": "x"}"#);
    assert!(
        is_error && text.contains("no MCP servers are configured"),
        "{text}"
    );
    assert_eq!(details["kind"], "not_configured");
    write_settings(&fx, json!({"mcp": {"servers": {"fx": server(json!({}))}}}));
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            r#"mcp_call {"tool": "fx/echo", "arguments": {}}"#,
            "invalid_arguments",
            "text",
        ),
        (
            r#"mcp_call {"tool": "fx/echo", "arguments": {"text": {"no": 1}}}"#,
            "invalid_arguments",
            "text",
        ),
        (
            r#"mcp_call {"tool": "fx/nope"}"#,
            "unknown_tool",
            "has no tool \"nope\"",
        ),
        (
            r#"mcp_call {"tool": "nope/echo"}"#,
            "unknown_server",
            "no MCP server named \"nope\"",
        ),
        (
            r#"mcp_call {"tool": "echo"}"#,
            "invalid_arguments",
            "server/tool",
        ),
        (
            r#"mcp_describe {"tool": "fx/missing"}"#,
            "unknown_tool",
            "missing",
        ),
    ];
    for (i, (prompt, kind, needle)) in cases.into_iter().enumerate() {
        let (is_error, text, details) = run(&mut e, &s, &format!("c{i}"), prompt);
        assert!(is_error, "{prompt}: {text}");
        assert!(text.contains(needle), "{prompt}: {text}");
        assert_eq!(details["kind"], kind, "{prompt}: {text}");
        assert_eq!(details["outcome"], "error");
    }
    // The server's own error is the remote tool's failure, with its content.
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "f1",
        r#"mcp_call {"tool": "fx/fail", "arguments": {"message": "boom"}}"#,
    );
    assert!(is_error, "{text}");
    assert!(text.contains("fx/fail reported an error: boom"), "{text}");
    assert_eq!(details["outcome"], "tool_error");
    assert_eq!(details["tool"]["qualified"], "fx/fail");
    assert_eq!(server_state(&mut e, &s, "fx").state, "ready");
    // Large results are bounded by the tool runner, not refused.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "b1",
        r#"mcp_call {"tool": "fx/big", "arguments": {"bytes": 300000}}"#,
    );
    assert!(!is_error, "{text}");
    assert!(text.len() < 300_000, "{}", text.len());
}

#[test]
fn a_lost_reply_times_out_as_unknown_effect_and_is_never_resent() {
    let fx = Fixture::new();
    write_settings(
        &fx,
        json!({"mcp": {"callTimeoutSeconds": 1, "servers": {"fx": server(json!({}))}}}),
    );
    let counter = fx.project.join("counter");
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let prompt = format!(
        "mcp_call {}",
        json!({"tool": "fx/counter_increment", "arguments": {"path": counter, "hang_after": true}})
    );
    let (is_error, text, details) = run(&mut e, &s, "r1", &prompt);
    assert!(is_error, "{text}");
    assert!(
        text.contains("did not answer within 1 s; its effect is unknown"),
        "{text}"
    );
    assert_eq!(details["kind"], "timeout");
    assert_eq!(details["tool"], "fx/counter_increment");
    assert_eq!(std::fs::read_to_string(&counter).unwrap(), "1");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap(),
        "1",
        "the engine never re-sent the mutation"
    );
    // The server is still usable: the engine did not tear it down for a slow tool.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r2",
        &format!(
            "mcp_call {}",
            json!({"tool": "fx/counter_increment", "arguments": {"path": counter}})
        ),
    );
    assert!(!is_error, "{text}");
    assert_eq!(text, "2");
    // A slow-but-answering tool within the timeout works.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r3",
        r#"mcp_call {"tool": "fx/slow", "arguments": {"ms": 200}}"#,
    );
    assert!(!is_error && text == "slept 200 ms", "{text}");
}

#[test]
fn a_restart_during_a_call_records_it_interrupted_and_does_not_rerun_it() {
    let fx = Fixture::new();
    write_settings(&fx, json!({"mcp": {"servers": {"fx": server(json!({}))}}}));
    let counter = fx.project.join("counter");
    let s;
    {
        let provider = Scripted::new(calling_from_prompt());
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        e.prompt(
            &s,
            "r1",
            &format!(
                "mcp_call {}",
                json!({"tool": "fx/counter_increment", "arguments": {"path": counter, "hang_after": true}})
            ),
        )
        .unwrap();
        let path = counter.clone();
        e.until("the server performed the mutation", move |_| {
            std::fs::read_to_string(&path).ok().as_deref() == Some("1")
        });
        e.handle.shutdown(Duration::from_millis(300));
    }
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let results = loop {
        let results = tool_results(&e.snapshot(&s));
        if !results.is_empty() {
            break results;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no result after restart"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].1, "{results:?}");
    assert!(
        results[0]
            .2
            .contains("Tool mcp_call was interrupted and may have partially run"),
        "{results:?}"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap(),
        "1",
        "the restarted engine never re-sent the call"
    );
    // The recovery registry says the same: catalog reads may rerun, calls may not.
    let shared = e.handle.shared();
    assert!(shared.recoverable_tool("mcp_search").is_some());
    assert!(shared.recoverable_tool("mcp_describe").is_some());
    assert!(shared.recoverable_tool("mcp_call").is_none());
}

#[test]
fn a_catalog_change_notification_refreshes_the_cached_catalog() {
    let fx = Fixture::new();
    write_settings(&fx, json!({"mcp": {"servers": {"fx": server(json!({}))}}}));
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let (is_error, text, _) = run(&mut e, &s, "r1", r#"mcp_search {"query": "added"}"#);
    assert!(!is_error && text.contains("No tools matched."), "{text}");
    let (is_error, text, _) = run(&mut e, &s, "r2", r#"mcp_call {"tool": "fx/change_tools"}"#);
    assert!(!is_error && text == "changed", "{text}");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut n = 0;
    loop {
        n += 1;
        let (is_error, text, _) = run(
            &mut e,
            &s,
            &format!("s{n}"),
            r#"mcp_search {"query": "added"}"#,
        );
        assert!(!is_error, "{text}");
        if text.contains("- fx/added_tool: A tool that appeared after change_tools") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "catalog never refreshed: {text}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(server_state(&mut e, &s, "fx").tools, Some(10));
    // The new tool is callable only through the cached (refreshed) schema.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "d1",
        r#"mcp_describe {"tool": "fx/added_tool"}"#,
    );
    assert!(!is_error && text.contains("fx/added_tool"), "{text}");
}

#[test]
fn idle_servers_stop_and_restart_on_use_while_kept_alive_ones_stay() {
    let fx = Fixture::new();
    write_settings(
        &fx,
        json!({"mcp": {"servers": {
            "fx": server(json!({"idleSeconds": 1})),
            "keep": server(json!({"idleSeconds": 1, "keepAlive": true}))
        }}}),
    );
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r1",
        r#"mcp_call {"tool": "fx/echo", "arguments": {"text": "a"}}"#,
    );
    assert!(!is_error && text == "a", "{text}");
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r2",
        r#"mcp_call {"tool": "keep/echo", "arguments": {"text": "b"}}"#,
    );
    assert!(!is_error && text == "b", "{text}");
    assert_eq!(server_state(&mut e, &s, "fx").state, "ready");
    let stopped = wait_state(&mut e, &s, "fx", "stopped");
    assert_eq!(stopped.tools, Some(9), "the catalog outlives the process");
    assert_eq!(server_state(&mut e, &s, "keep").state, "ready");
    // A search answers from the cached catalog without starting the process.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r3",
        r#"mcp_search {"query": "echo", "server": "fx"}"#,
    );
    assert!(!is_error && text.contains("- fx/echo"), "{text}");
    assert_eq!(server_state(&mut e, &s, "fx").state, "stopped");
    // A call starts it again.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r4",
        r#"mcp_call {"tool": "fx/echo", "arguments": {"text": "c"}}"#,
    );
    assert!(!is_error && text == "c", "{text}");
    assert_eq!(server_state(&mut e, &s, "fx").state, "ready");
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(server_state(&mut e, &s, "keep").state, "ready", "keepAlive");
    // A call in flight is never reaped: a slow call longer than the idle allowance works.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r5",
        r#"mcp_call {"tool": "fx/slow", "arguments": {"ms": 2200}}"#,
    );
    assert!(!is_error && text == "slept 2200 ms", "{text}");
}

#[test]
fn start_failures_are_reported_with_backoff_and_references_never_leak() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.dir.path().join("config")).unwrap();
    std::fs::write(
        fx.dir.path().join("config/mcp-secrets.json"),
        r#"{"greeting": "top-secret-value"}"#,
    )
    .unwrap();
    write_settings(
        &fx,
        json!({"mcp": {"startTimeoutSeconds": 5, "servers": {
            "broken": server(json!({"env": {"MCP_FIXTURE_FAIL_INIT": "1"}})),
            "noenv": server(json!({"env": {"T": "$MISSING_VARIABLE"}})),
            "nosecret": server(json!({"env": {"T": "secret:absent"}})),
            "nobin": {"command": "/nonexistent/mcp-server"},
            "off": server(json!({"enabled": false})),
            "sec": server(json!({"env": {"GREETING": "secret:greeting", "MCP_FIXTURE_ENV": "GREETING"}}))
        }}}),
    );
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let cases = [
        ("broken", "start_failed", "could not start"),
        (
            "noenv",
            "start_failed",
            "environment variable MISSING_VARIABLE is not set",
        ),
        (
            "nosecret",
            "start_failed",
            "secret \"absent\" is not in mcp-secrets.json",
        ),
        (
            "nobin",
            "start_failed",
            "cannot spawn /nonexistent/mcp-server",
        ),
        ("off", "disabled", "is disabled in settings"),
    ];
    for (name, kind, needle) in cases {
        let (is_error, text, details) = run(
            &mut e,
            &s,
            &format!("c-{name}"),
            &format!("mcp_describe {}", json!({"tool": format!("{name}/echo")})),
        );
        assert!(is_error && text.contains(needle), "{name}: {text}");
        assert_eq!(details["kind"], kind, "{name}: {text}");
    }
    let view = server_state(&mut e, &s, "broken");
    assert_eq!(view.state, "failed");
    assert!(view.error.is_some());
    // The failure is remembered: the next attempt reports the backoff instead of spawning.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "again",
        r#"mcp_describe {"tool": "broken/echo"}"#,
    );
    assert!(
        is_error && text.contains("retrying after a backoff"),
        "{text}"
    );
    // A search across servers reports each start failure beside the results.
    let (is_error, text, details) = run(&mut e, &s, "all", r#"mcp_search {"query": "echo"}"#);
    assert!(!is_error, "{text}");
    assert!(text.contains("- sec/echo"), "{text}");
    assert!(
        text.contains("! broken:") && text.contains("! nobin:"),
        "{text}"
    );
    assert_eq!(details["problems"].as_array().unwrap().len(), 4);
    assert!(!text.contains("off/"), "disabled servers are not searched");
    // The secret reached the process and nothing else.
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "sec",
        r#"mcp_call {"tool": "sec/echo", "arguments": {"text": "x"}}"#,
    );
    assert!(!is_error && text == "x env:top-secret-value", "{text}");
    let dump = serde_json::to_string(&servers(&mut e, &s)).unwrap();
    assert!(!dump.contains("top-secret-value"), "{dump}");
    assert!(dump.contains("GREETING=secret:greeting"), "{dump}");
}

#[test]
fn project_settings_add_servers_with_project_scope_and_a_changed_revision_restarts() {
    let fx = Fixture::new();
    write_settings(&fx, json!({"mcp": {"servers": {"fx": server(json!({}))}}}));
    std::fs::create_dir_all(fx.project.join(".pi")).unwrap();
    std::fs::write(
        fx.project.join(".pi/settings.json"),
        json!({"mcp": {"servers": {"proj": server(json!({"args": ["--proj"]}))}}}).to_string(),
    )
    .unwrap();
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let view = servers(&mut e, &s);
    let scopes: Vec<(String, String)> = view
        .servers
        .iter()
        .map(|v| (v.name.clone(), v.scope.clone()))
        .collect();
    assert_eq!(
        scopes,
        [
            ("fx".to_owned(), "user".to_owned()),
            ("proj".to_owned(), "project".to_owned())
        ]
    );
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r1",
        r#"mcp_call {"tool": "proj/echo", "arguments": {"text": "p"}}"#,
    );
    assert!(!is_error && text == "p", "{text}");
    let before = server_state(&mut e, &s, "proj");
    assert_eq!(before.state, "ready");
    // Changing launch-relevant configuration changes the revision and stops the old process.
    std::fs::write(
        fx.project.join(".pi/settings.json"),
        json!({"mcp": {"servers": {"proj": server(json!({"args": ["--proj", "--v2"]}))}}})
            .to_string(),
    )
    .unwrap();
    let after = server_state(&mut e, &s, "proj");
    assert_ne!(after.revision, before.revision);
    assert_eq!(after.state, "stopped");
    assert_eq!(
        after.tools, None,
        "the old catalog is not trusted for a new revision"
    );
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r2",
        r#"mcp_call {"tool": "proj/echo", "arguments": {"text": "q"}}"#,
    );
    assert!(!is_error && text == "q", "{text}");
    assert_eq!(details["tool"]["revision"], after.revision);
    // Removing the entry stops the server and makes it unknown.
    std::fs::write(fx.project.join(".pi/settings.json"), "{}").unwrap();
    let (is_error, text, details) = run(&mut e, &s, "r3", r#"mcp_describe {"tool": "proj/echo"}"#);
    assert!(
        is_error && text.contains("no MCP server named \"proj\""),
        "{text}"
    );
    assert_eq!(details["kind"], "unknown_server");
    assert!(servers(&mut e, &s).servers.iter().all(|v| v.name != "proj"));
    // Invalid entries are warnings, not failures.
    std::fs::write(
        fx.project.join(".pi/settings.json"),
        json!({"mcp": {"servers": {"bad name": {"command": "x"}, "nocmd": {}}}}).to_string(),
    )
    .unwrap();
    let view = servers(&mut e, &s);
    assert_eq!(view.warnings.len(), 2, "{:?}", view.warnings);
    assert_eq!(view.servers.len(), 1);
}

#[test]
fn the_catalog_persists_across_restarts_and_the_capability_is_advertised() {
    let fx = Fixture::new();
    write_settings(&fx, json!({"mcp": {"servers": {"fx": server(json!({}))}}}));
    let s;
    {
        let provider = Scripted::new(calling_from_prompt());
        let mut e = fx.start(&provider);
        s = e.create(&fx.project, "c");
        let (is_error, text, _) = run(&mut e, &s, "r1", r#"mcp_search {"query": "echo"}"#);
        assert!(!is_error && text.contains("- fx/echo"), "{text}");
        e.handle.shutdown(Duration::from_millis(300));
    }
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let welcome = match e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "again".into(),
    }) {
        Ok(Reply::Welcome(w)) => w,
        other => panic!("{other:?}"),
    };
    assert!(welcome.capabilities.iter().any(|c| c == "mcp"));
    let view = server_state(&mut e, &s, "fx");
    assert_eq!(view.state, "stopped");
    assert_eq!(
        view.tools,
        Some(9),
        "the cached catalog survived the restart"
    );
    let (is_error, text, _) = run(&mut e, &s, "r2", r#"mcp_describe {"tool": "fx/echo"}"#);
    assert!(!is_error && text.contains("fx/echo"), "{text}");
    assert_eq!(
        server_state(&mut e, &s, "fx").state,
        "stopped",
        "answered from the cache"
    );
    // Children keep the MCP tools only when their profile allows them.
    let profiles = crate::profiles::builtin_profiles();
    let by_name = |n: &str| profiles.iter().find(|p| p.name == n).unwrap();
    assert!(by_name("worker").allows("mcp_call"));
    for restricted in ["reviewer", "scout", "auditor"] {
        for tool in ["mcp_search", "mcp_describe", "mcp_call"] {
            assert!(!by_name(restricted).allows(tool), "{restricted} {tool}");
        }
    }
}
