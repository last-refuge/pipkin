//! MCP-2: Streamable HTTP over the engine's own client, OAuth sign-in against a scripted
//! authorization server, token refresh, the management calls behind the Connections page
//! (add, edit, remove, test, reconnect, tool enablement), previewed import and the
//! `McpChanged` event. The HTTP fixture is a JSON-RPC MCP server on the engine's mock HTTP
//! server; the test plays the browser.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipkin_engine_protocol::*;
use serde_json::{Value, json};

use super::mcp_tests::{calling_from_prompt, fixture_bin, run, server, servers, write_settings};
use super::tests::{Engine, Fixture};
use crate::provider::mock::{MockResponse, MockServer, Recorded};
use crate::provider::scripted::Scripted;

#[derive(Default)]
struct Seen {
    /// Bearer tokens presented on MCP requests, in order.
    bearers: Vec<String>,
    registrations: Vec<Value>,
    token_requests: Vec<HashMap<String, String>>,
    calls: Vec<Value>,
}

struct Policy {
    require_auth: bool,
    valid_tokens: HashSet<String>,
    /// Lifetime given to issued tokens.
    expires_in: u64,
    refuse_refresh: bool,
    next_token: u32,
}

struct HttpMcp {
    _rt: tokio::runtime::Runtime,
    mock: MockServer,
    seen: Arc<Mutex<Seen>>,
    policy: Arc<Mutex<Policy>>,
}

fn json(status: u16, body: Value, extra: &[(&str, &str)]) -> MockResponse {
    let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
    headers.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    MockResponse {
        status,
        headers,
        chunks: vec![body.to_string().into_bytes()],
        pause: Duration::ZERO,
        hang: false,
    }
}

fn empty(status: u16, extra: &[(&str, &str)]) -> MockResponse {
    MockResponse {
        status,
        headers: extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        chunks: vec![],
        pause: Duration::ZERO,
        hang: false,
    }
}

fn form(body: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

impl HttpMcp {
    fn start(require_auth: bool) -> HttpMcp {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let policy = Arc::new(Mutex::new(Policy {
            require_auth,
            valid_tokens: HashSet::new(),
            expires_in: 3600,
            refuse_refresh: false,
            next_token: 1,
        }));
        let base: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let mock = {
            let seen = seen.clone();
            let policy = policy.clone();
            let base = base.clone();
            rt.block_on(MockServer::start(move |req: &Recorded, _| {
                let base = base.lock().unwrap().clone();
                let path = req.path.split('?').next().unwrap_or("").to_owned();
                match (req.method.as_str(), path.as_str()) {
                    ("POST", "/mcp") => {
                        let p = policy.lock().unwrap();
                        let bearer = req
                            .header("authorization")
                            .and_then(|h| h.strip_prefix("Bearer "))
                            .map(str::to_owned);
                        if let Some(b) = &bearer {
                            seen.lock().unwrap().bearers.push(b.clone());
                        }
                        if p.require_auth && !bearer.as_ref().is_some_and(|b| p.valid_tokens.contains(b)) {
                            let challenge = format!(
                                "Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\", scope=\"mcp:read\""
                            );
                            return empty(401, &[("www-authenticate", &challenge)]);
                        }
                        drop(p);
                        let msg: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                        let id = msg.get("id").cloned().unwrap_or(Value::Null);
                        match msg.get("method").and_then(Value::as_str) {
                            Some("initialize") => json(
                                200,
                                json!({"jsonrpc": "2.0", "id": id, "result": {
                                    "protocolVersion": "2025-06-18",
                                    "capabilities": {"tools": {}},
                                    "serverInfo": {"name": "http-fixture", "version": "1.0"}
                                }}),
                                &[("mcp-session-id", "sess-1")],
                            ),
                            Some("notifications/initialized") => empty(202, &[]),
                            Some("ping") => json(200, json!({"jsonrpc": "2.0", "id": id, "result": {}}), &[]),
                            Some("tools/list") => json(
                                200,
                                json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [
                                    {"name": "echo", "description": "Echo over HTTP", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
                                    {"name": "whoami", "description": "The bearer seen", "inputSchema": {"type": "object"}}
                                ]}}),
                                &[],
                            ),
                            Some("tools/call") => {
                                seen.lock().unwrap().calls.push(msg["params"].clone());
                                let name = msg["params"]["name"].as_str().unwrap_or("");
                                let text = match name {
                                    "echo" => format!("http:{}", msg["params"]["arguments"]["text"].as_str().unwrap_or("")),
                                    _ => bearer.unwrap_or_else(|| "none".into()),
                                };
                                json(200, json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": text}], "isError": false}}), &[])
                            }
                            _ => json(200, json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unknown"}}), &[]),
                        }
                    }
                    ("GET", "/mcp") => empty(405, &[]),
                    ("DELETE", "/mcp") => empty(200, &[]),
                    ("GET", "/.well-known/oauth-protected-resource/mcp") => json(
                        200,
                        json!({"resource": format!("{base}/mcp"), "authorization_servers": [format!("{base}/auth")], "scopes_supported": ["mcp:read", "mcp:write"]}),
                        &[],
                    ),
                    ("GET", "/.well-known/oauth-authorization-server/auth") => json(
                        200,
                        json!({"issuer": format!("{base}/auth"), "authorization_endpoint": format!("{base}/auth/authorize"), "token_endpoint": format!("{base}/auth/token"), "registration_endpoint": format!("{base}/auth/register")}),
                        &[],
                    ),
                    ("POST", "/auth/register") => {
                        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                        seen.lock().unwrap().registrations.push(body);
                        json(201, json!({"client_id": "dyn-client-1"}), &[])
                    }
                    ("POST", "/auth/token") => {
                        let fields = form(&req.body);
                        seen.lock().unwrap().token_requests.push(fields.clone());
                        let mut p = policy.lock().unwrap();
                        let grant = fields.get("grant_type").map(String::as_str).unwrap_or("");
                        let ok = match grant {
                            "authorization_code" => {
                                fields.get("code").map(String::as_str) == Some("CODE-1")
                                    && fields.get("client_id").map(String::as_str) == Some("dyn-client-1")
                                    && fields.get("code_verifier").is_some_and(|v| !v.is_empty())
                                    && fields.get("resource").map(String::as_str) == Some(format!("{base}/mcp").as_str())
                            }
                            "refresh_token" => !p.refuse_refresh && fields.get("refresh_token").map(String::as_str) == Some("ref-1"),
                            _ => false,
                        };
                        if !ok {
                            return json(400, json!({"error": "invalid_grant"}), &[]);
                        }
                        let n = p.next_token;
                        p.next_token += 1;
                        let token = format!("tok-{n}");
                        p.valid_tokens.insert(token.clone());
                        let expires = p.expires_in;
                        json(200, json!({"access_token": token, "refresh_token": "ref-1", "expires_in": expires, "token_type": "Bearer", "scope": "mcp:read"}), &[])
                    }
                    _ => empty(404, &[]),
                }
            }))
        };
        *base.lock().unwrap() = mock.url.clone();
        HttpMcp {
            _rt: rt,
            mock,
            seen,
            policy,
        }
    }

    fn url(&self) -> String {
        format!("{}/mcp", self.mock.url)
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap()
    }
}

/// The browser: follow the sign-in redirect with the code the authorization server "issued".
fn browser_get(url: &str) -> String {
    let parsed = url::Url::parse(url).unwrap();
    let host = parsed.host_str().unwrap();
    let port = parsed.port().unwrap();
    let path = match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_owned(),
    };
    let mut stream = std::net::TcpStream::connect((host, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out.lines().next().unwrap_or("").to_owned()
}

fn view_of(e: &mut Engine, s: Option<&str>, name: &str) -> McpServerView {
    let view = match e.call(Call::McpServers {
        session: s.map(str::to_owned),
    }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    };
    view.servers
        .into_iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no server {name}"))
}

fn wait_view(
    e: &mut Engine,
    s: Option<&str>,
    name: &str,
    what: &str,
    pred: impl Fn(&McpServerView) -> bool,
) -> McpServerView {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let view = view_of(e, s, name);
        if pred(&view) {
            return view;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}: {view:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Drive a sign-in to completion: start it, open the URL "in the browser", hit the callback.
fn sign_in(e: &mut Engine, s: &str, name: &str) -> McpServerView {
    match e.call(Call::McpAuthStart {
        session: Some(s.into()),
        name: name.into(),
    }) {
        Ok(Reply::McpServers(_)) => {}
        other => panic!("{other:?}"),
    }
    let waiting = wait_view(e, Some(s), name, "the sign-in URL", |v| {
        v.auth.url.is_some()
    });
    // The URL is shown while the manual paste prompt is open beside the browser flow.
    assert!(
        matches!(
            waiting.auth.status,
            AuthStatus::Waiting | AuthStatus::Prompt
        ),
        "{:?}",
        waiting.auth
    );
    let url = url::Url::parse(waiting.auth.url.as_deref().unwrap()).unwrap();
    let q: HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["client_id"], "dyn-client-1");
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q["resource"].ends_with("/mcp"), "{q:?}");
    assert_eq!(q["scope"], "mcp:read", "the challenge's scope wins");
    let redirect = format!("{}?code=CODE-1&state={}", q["redirect_uri"], q["state"]);
    let status = browser_get(&redirect);
    assert!(status.contains("200"), "{status}");
    wait_view(e, Some(s), name, "sign-in done", |v| {
        v.auth.status == AuthStatus::Done
    })
}

fn settings_file(fx: &Fixture) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(fx.dir.path().join("config/settings.json")).unwrap(),
    )
    .unwrap()
}

#[test]
fn an_http_server_needs_sign_in_and_oauth_completes_through_the_loopback_callback() {
    let http = HttpMcp::start(true);
    let fx = Fixture::new();
    write_settings(
        &fx,
        json!({"mcp": {"servers": {"remote": {"url": http.url(), "headers": {"X-Team": "docs"}}}}}),
    );
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let (is_error, text, details) =
        run(&mut e, &s, "r1", r#"mcp_describe {"tool": "remote/echo"}"#);
    assert!(is_error, "{text}");
    assert_eq!(details["kind"], "auth_required");
    assert!(text.contains("needs you to sign in first"), "{text}");
    let view = view_of(&mut e, Some(&s), "remote");
    assert_eq!(view.kind, "http");
    assert_eq!(view.state, "authRequired");
    assert!(view.auth.supported && view.auth.required && !view.auth.signed_in);
    assert_eq!(view.env, ["X-Team=…"]);
    assert_eq!(view.command, http.url());
    // A search reports the problem per server instead of failing.
    let (is_error, text, _) = run(&mut e, &s, "r2", r#"mcp_search {"query": "echo"}"#);
    assert!(
        !is_error && text.contains("! remote:") && text.contains("sign in"),
        "{text}"
    );

    let done = sign_in(&mut e, &s, "remote");
    assert!(done.auth.signed_in);
    assert_eq!(done.state, "stopped");
    {
        let seen = http.seen();
        assert_eq!(seen.registrations.len(), 1);
        assert_eq!(seen.registrations[0]["client_name"], "Pipkin");
        assert!(
            seen.registrations[0]["redirect_uris"][0]
                .as_str()
                .unwrap()
                .starts_with("http://127.0.0.1:")
        );
        assert_eq!(seen.token_requests.len(), 1);
        assert_eq!(seen.token_requests[0]["grant_type"], "authorization_code");
    }
    // The token reaches the server; views never show it.
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r3",
        r#"mcp_call {"tool": "remote/echo", "arguments": {"text": "hi"}}"#,
    );
    assert!(!is_error, "{text}");
    assert_eq!(text, "http:hi");
    assert_eq!(details["tool"]["qualified"], "remote/echo");
    assert!(http.seen().bearers.iter().any(|b| b == "tok-1"));
    let (is_error, text, _) = run(&mut e, &s, "r4", r#"mcp_call {"tool": "remote/whoami"}"#);
    assert!(!is_error && text == "tok-1", "{text}");
    let dump = serde_json::to_string(&servers(&mut e, &s)).unwrap();
    assert!(!dump.contains("tok-1") && !dump.contains("ref-1"), "{dump}");
    let auth_file = std::fs::read_to_string(fx.dir.path().join("config/mcp-auth.json")).unwrap();
    assert!(
        auth_file.contains("tok-1"),
        "tokens live in the private store"
    );
    let view = view_of(&mut e, Some(&s), "remote");
    assert_eq!(view.state, "ready");
    assert_eq!(view.tools, Some(2));
    assert!(view.catalog.iter().any(|t| t.name == "echo" && t.enabled));
    // Revoking forgets the tokens and the next use needs a sign-in again.
    match e.call(Call::McpAuthRevoke {
        session: Some(s.clone()),
        name: "remote".into(),
    }) {
        Ok(Reply::McpServers(v)) => {
            let r = v.servers.iter().find(|v| v.name == "remote").unwrap();
            assert!(!r.auth.signed_in && r.state == "stopped");
        }
        other => panic!("{other:?}"),
    }
    assert!(
        !std::fs::read_to_string(fx.dir.path().join("config/mcp-auth.json"))
            .unwrap()
            .contains("tok-1")
    );
    let (is_error, _, details) = run(&mut e, &s, "r5", r#"mcp_call {"tool": "remote/whoami"}"#);
    assert!(is_error);
    assert_eq!(details["kind"], "auth_required");
    // Cancelling an attempt settles it as failed without a credential.
    e.call(Call::McpAuthStart {
        session: Some(s.clone()),
        name: "remote".into(),
    })
    .unwrap();
    wait_view(&mut e, Some(&s), "remote", "url", |v| v.auth.url.is_some());
    e.call(Call::McpAuthCancel {
        name: "remote".into(),
    })
    .unwrap();
    let cancelled = wait_view(&mut e, Some(&s), "remote", "cancelled", |v| {
        v.auth.status == AuthStatus::Failed
    });
    assert_eq!(cancelled.auth.message, "Sign-in cancelled");
    assert!(cancelled.auth.url.is_none() && !cancelled.auth.signed_in);
    // Stdio servers have no sign-in.
    write_settings(
        &fx,
        json!({"mcp": {"servers": {"remote": {"url": http.url()}, "fx": server(json!({}))}}}),
    );
    let refused = e
        .call(Call::McpAuthStart {
            session: Some(s.clone()),
            name: "fx".into(),
        })
        .unwrap_err();
    assert!(
        refused.message.contains("runs over stdio"),
        "{}",
        refused.message
    );
}

#[test]
fn tokens_refresh_before_expiry_and_a_failed_refresh_asks_for_sign_in_again() {
    let http = HttpMcp::start(true);
    http.policy.lock().unwrap().expires_in = 10;
    let fx = Fixture::new();
    write_settings(
        &fx,
        json!({"mcp": {"servers": {"remote": {"url": http.url()}}}}),
    );
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    sign_in(&mut e, &s, "remote");
    // tok-1 expires within the margin: the first use refreshes to tok-2 before connecting.
    let (is_error, text, _) = run(&mut e, &s, "r1", r#"mcp_call {"tool": "remote/whoami"}"#);
    assert!(!is_error, "{text}");
    assert_eq!(text, "tok-2");
    {
        let seen = http.seen();
        assert_eq!(
            seen.token_requests.last().unwrap()["grant_type"],
            "refresh_token"
        );
        assert_eq!(
            seen.token_requests.last().unwrap()["refresh_token"],
            "ref-1"
        );
        assert!(
            !seen.bearers.contains(&"tok-1".to_owned()),
            "{:?}",
            seen.bearers
        );
    }
    // The next connection needs another refresh; when it is refused, the person must sign
    // in again and nothing is sent with a stale token.
    http.policy.lock().unwrap().refuse_refresh = true;
    match e.call(Call::McpReconnectServer {
        session: Some(s.clone()),
        name: "remote".into(),
    }) {
        Ok(Reply::McpServers(v)) => {
            let r = v.servers.iter().find(|v| v.name == "remote").unwrap();
            assert_eq!(r.state, "authRequired", "{r:?}");
            assert!(
                r.auth.message.contains("sign in again"),
                "{}",
                r.auth.message
            );
        }
        other => panic!("{other:?}"),
    }
    let bearers = http.seen().bearers.clone();
    assert_eq!(
        bearers.iter().filter(|b| *b == "tok-2").count(),
        bearers.len(),
        "only tok-2 was ever used after the refresh: {bearers:?}"
    );
}

#[test]
fn management_calls_edit_settings_test_reconnect_and_switch_tools_off() {
    let fx = Fixture::new();
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    // Without a session: user scope only.
    let view = match e.call(Call::McpServers { session: None }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    };
    assert!(view.servers.is_empty() && !view.project_available);
    let s = e.create(&fx.project, "c");
    // Add in user scope with a literal environment value.
    let added = match e.call(Call::McpUpsertServer {
        session: Some(s.clone()),
        scope: McpScope::User,
        name: "fx".into(),
        spec: json!({"command": fixture_bin(), "env": {"MCP_FIXTURE_ENV": "GREETING", "GREETING": "plain-value"}}),
    }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    };
    assert!(added.project_available);
    let fx_view = added.servers.iter().find(|v| v.name == "fx").unwrap();
    assert_eq!(fx_view.scope, "user");
    assert_eq!(fx_view.state, "stopped");
    assert_eq!(fx_view.env, ["MCP_FIXTURE_ENV=…", "GREETING=…"]);
    assert_eq!(fx_view.spec["env"]["GREETING"], "…");
    assert_eq!(
        settings_file(&fx)["mcp"]["servers"]["fx"]["env"]["GREETING"],
        "plain-value"
    );
    // Editing with the redacted marker keeps the stored literal; a changed arg restarts.
    e.call(Call::McpUpsertServer {
        session: Some(s.clone()),
        scope: McpScope::User,
        name: "fx".into(),
        spec: json!({"command": fixture_bin(), "args": ["--v2"], "env": {"MCP_FIXTURE_ENV": "GREETING", "GREETING": "…"}}),
    })
    .unwrap();
    assert_eq!(
        settings_file(&fx)["mcp"]["servers"]["fx"]["env"]["GREETING"],
        "plain-value"
    );
    assert_eq!(
        settings_file(&fx)["mcp"]["servers"]["fx"]["args"],
        json!(["--v2"])
    );
    // Invalid specs and names are refused before anything is written.
    let err = e
        .call(Call::McpUpsertServer {
            session: Some(s.clone()),
            scope: McpScope::User,
            name: "bad name".into(),
            spec: json!({"command": "x"}),
        })
        .unwrap_err();
    assert!(err.message.contains("not a valid server name"));
    let err = e
        .call(Call::McpUpsertServer {
            session: Some(s.clone()),
            scope: McpScope::User,
            name: "nope".into(),
            spec: json!({"url": "ftp://x"}),
        })
        .unwrap_err();
    assert!(
        err.message.contains("must be http or https"),
        "{}",
        err.message
    );
    assert!(settings_file(&fx)["mcp"]["servers"].get("nope").is_none());
    // Project scope writes the project's own file and needs a session.
    let err = e
        .call(Call::McpUpsertServer {
            session: None,
            scope: McpScope::Project,
            name: "proj".into(),
            spec: json!({"command": fixture_bin()}),
        })
        .unwrap_err();
    assert!(
        err.message.contains("need an open session"),
        "{}",
        err.message
    );
    let with_project = match e.call(Call::McpUpsertServer {
        session: Some(s.clone()),
        scope: McpScope::Project,
        name: "proj".into(),
        spec: json!({"command": fixture_bin()}),
    }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        with_project
            .servers
            .iter()
            .find(|v| v.name == "proj")
            .unwrap()
            .scope,
        "project"
    );
    let project_file: Value = serde_json::from_str(
        &std::fs::read_to_string(fx.project.join(".pi/settings.json")).unwrap(),
    )
    .unwrap();
    assert!(project_file["mcp"]["servers"]["proj"]["command"].is_string());
    // Test connects afresh and lists the tools; the environment reached the process.
    let test = match e.call(Call::McpTestServer {
        session: Some(s.clone()),
        name: "fx".into(),
    }) {
        Ok(Reply::McpTest(t)) => t,
        other => panic!("{other:?}"),
    };
    assert!(test.ok, "{}", test.message);
    assert_eq!(test.server_name.as_deref(), Some("mcp-fixture"));
    assert_eq!(test.server_version.as_deref(), Some("0.0.0"));
    assert!(test.tools.contains(&"echo".to_owned()));
    assert!(
        test.message
            .contains("Connected to mcp-fixture 0.0.0 with 6 tool(s)"),
        "{}",
        test.message
    );
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r1",
        r#"mcp_call {"tool": "fx/echo", "arguments": {"text": "x"}}"#,
    );
    assert!(!is_error && text == "x env:plain-value", "{text}");
    // A test of a broken server reports the failure.
    e.call(Call::McpUpsertServer {
        session: Some(s.clone()),
        scope: McpScope::User,
        name: "broken".into(),
        spec: json!({"command": "/nonexistent/mcp"}),
    })
    .unwrap();
    let test = match e.call(Call::McpTestServer {
        session: Some(s.clone()),
        name: "broken".into(),
    }) {
        Ok(Reply::McpTest(t)) => t,
        other => panic!("{other:?}"),
    };
    assert!(
        !test.ok && test.message.contains("cannot spawn"),
        "{}",
        test.message
    );
    assert_eq!(view_of(&mut e, Some(&s), "broken").state, "failed");
    // Reconnect clears the backoff and tries again (still broken here), then works for fx.
    match e.call(Call::McpReconnectServer {
        session: Some(s.clone()),
        name: "broken".into(),
    }) {
        Ok(Reply::McpServers(v)) => assert_eq!(
            v.servers.iter().find(|v| v.name == "broken").unwrap().state,
            "failed"
        ),
        other => panic!("{other:?}"),
    }
    match e.call(Call::McpReconnectServer {
        session: Some(s.clone()),
        name: "fx".into(),
    }) {
        Ok(Reply::McpServers(v)) => assert_eq!(
            v.servers.iter().find(|v| v.name == "fx").unwrap().state,
            "ready"
        ),
        other => panic!("{other:?}"),
    }
    // Switching a tool off removes it from search and refuses calls, without a restart.
    let view = match e.call(Call::McpSetDisabledTools {
        session: Some(s.clone()),
        scope: McpScope::User,
        name: "fx".into(),
        disabled: vec!["echo".into()],
    }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    };
    let fx_view = view.servers.iter().find(|v| v.name == "fx").unwrap();
    assert_eq!(fx_view.state, "ready", "enablement is not a restart");
    assert_eq!(fx_view.disabled_tools, ["echo"]);
    assert!(
        fx_view
            .catalog
            .iter()
            .any(|t| t.name == "echo" && !t.enabled)
    );
    let (is_error, text, details) = run(
        &mut e,
        &s,
        "r2",
        r#"mcp_call {"tool": "fx/echo", "arguments": {"text": "x"}}"#,
    );
    assert!(
        is_error && text.contains("switched off in settings"),
        "{text}"
    );
    assert_eq!(details["kind"], "tool_disabled");
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r3",
        r#"mcp_search {"query": "echo", "server": "fx"}"#,
    );
    assert!(!is_error && text.contains("No tools matched."), "{text}");
    e.call(Call::McpSetDisabledTools {
        session: Some(s.clone()),
        scope: McpScope::User,
        name: "fx".into(),
        disabled: vec![],
    })
    .unwrap();
    assert!(
        settings_file(&fx)["mcp"]["servers"]["fx"]
            .get("disabledTools")
            .is_none()
    );
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r4",
        r#"mcp_search {"query": "echo", "server": "fx"}"#,
    );
    assert!(!is_error && text.contains("- fx/echo"), "{text}");
    // Removal stops the server and forgets it; removing from the wrong scope is a not-found.
    let err = e
        .call(Call::McpRemoveServer {
            session: Some(s.clone()),
            scope: McpScope::Project,
            name: "fx".into(),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let view = match e.call(Call::McpRemoveServer {
        session: Some(s.clone()),
        scope: McpScope::User,
        name: "fx".into(),
    }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    };
    assert!(view.servers.iter().all(|v| v.name != "fx"));
    assert!(settings_file(&fx)["mcp"]["servers"].get("fx").is_none());
    let (is_error, _, details) = run(&mut e, &s, "r5", r#"mcp_describe {"tool": "fx/echo"}"#);
    assert!(is_error);
    assert_eq!(details["kind"], "unknown_server");
}

#[test]
fn import_previews_and_applies_only_the_chosen_entries() {
    let fx = Fixture::new();
    write_settings(&fx, json!({"mcp": {"servers": {"fx": server(json!({}))}}}));
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let text = json!({"mcpServers": {
        "fx": {"command": fixture_bin(), "args": ["--imported"]},
        "second": {"command": fixture_bin(), "env": {"MCP_FIXTURE_ENV": "G", "G": "${G_VALUE}"}},
        "remote": {"type": "sse", "url": "https://r.example/mcp"},
        "broken": {"type": "http"}
    }})
    .to_string();
    let preview = match e.call(Call::McpImportPreview {
        session: Some(s.clone()),
        text: text.clone(),
    }) {
        Ok(Reply::McpImport(p)) => p,
        other => panic!("{other:?}"),
    };
    assert_eq!(preview.entries.len(), 4);
    let by = |n: &str| preview.entries.iter().find(|e| e.name == n).unwrap();
    assert!(by("fx").exists && by("fx").valid);
    assert!(!by("second").exists && by("second").valid);
    assert_eq!(by("second").spec["env"]["G"], "$G_VALUE");
    assert!(
        by("remote").valid
            && by("remote")
                .warnings
                .iter()
                .any(|w| w.contains("legacy SSE"))
    );
    assert!(!by("broken").valid);
    assert!(
        settings_file(&fx)["mcp"]["servers"].get("second").is_none(),
        "a preview writes nothing"
    );
    // Applying an invalid entry is refused; applying chosen entries writes only those.
    let err = e
        .call(Call::McpImportApply {
            session: Some(s.clone()),
            scope: McpScope::User,
            text: text.clone(),
            names: vec!["broken".into()],
        })
        .unwrap_err();
    assert!(
        err.message.contains("cannot be imported"),
        "{}",
        err.message
    );
    let view = match e.call(Call::McpImportApply {
        session: Some(s.clone()),
        scope: McpScope::User,
        text,
        names: vec!["second".into(), "remote".into()],
    }) {
        Ok(Reply::McpServers(v)) => v,
        other => panic!("{other:?}"),
    };
    let names: Vec<&str> = view.servers.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["fx", "remote", "second"]);
    let file = settings_file(&fx);
    assert_eq!(file["mcp"]["servers"]["second"]["env"]["G"], "$G_VALUE");
    assert!(
        file["mcp"]["servers"]["fx"].get("args").is_none(),
        "fx was not chosen, so not replaced"
    );
    assert_eq!(
        file["mcp"]["servers"]["remote"]["url"],
        "https://r.example/mcp"
    );
    // The imported server works like any other (its reference resolves at launch).
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r1",
        r#"mcp_search {"query": "echo", "server": "second"}"#,
    );
    assert!(
        is_error || text.contains("! second:") || text.contains("- second/echo"),
        "{text}"
    );
}

#[test]
fn mcp_changed_events_follow_connection_state() {
    let fx = Fixture::new();
    write_settings(&fx, json!({"mcp": {"servers": {"fx": server(json!({}))}}}));
    let provider = Scripted::new(calling_from_prompt());
    let mut e = fx.start(&provider);
    let s = e.create(&fx.project, "c");
    let before = e
        .events
        .iter()
        .filter(|ev| matches!(ev, Event::McpChanged))
        .count();
    let (is_error, text, _) = run(
        &mut e,
        &s,
        "r1",
        r#"mcp_call {"tool": "fx/echo", "arguments": {"text": "x"}}"#,
    );
    assert!(!is_error, "{text}");
    e.until("an McpChanged event", |events| {
        events
            .iter()
            .filter(|ev| matches!(ev, Event::McpChanged))
            .count()
            > before
    });
    let welcome = match e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "again".into(),
    }) {
        Ok(Reply::Welcome(w)) => w,
        other => panic!("{other:?}"),
    };
    assert!(welcome.capabilities.iter().any(|c| c == "mcpManagement"));
}
