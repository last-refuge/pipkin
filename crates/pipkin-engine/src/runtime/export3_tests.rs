//! Export configuration (OBS-3) through the client contract: the settings calls write the
//! user settings file and the health follows at once; the collector credential lives in the
//! engine's private credential file (0600) and reaches the request as a header without ever
//! appearing in the health; the content level adds prompts, replies and tool texts that the
//! metadata level never carries; the redactor strips the credential and token shapes from
//! exported text; the preview shows a payload at either level without sending anything.

use std::sync::Arc;
use std::sync::atomic::AtomicU16;
use std::time::{Duration, Instant};

use pipkin_engine_protocol::*;
use serde_json::{Value, json};

use super::tests::{Engine, Fixture};
use crate::provider::ProviderRequest;
use crate::provider::mock::{MockResponse, MockServer, Recorded};
use crate::provider::scripted::{Reply as R, Scripted};

const SECRET: &str = "Basic c2VjcmV0LWtleS12YWx1ZQ==";

fn last_is_user(req: &ProviderRequest) -> bool {
    matches!(
        req.messages
            .iter()
            .rev()
            .find(|m| !matches!(m, Message::System(_))),
        Some(Message::User { .. })
    )
}

/// One tool call that echoes the credential and an API key, then a reply.
fn script() -> impl Fn(&ProviderRequest, usize) -> R + Send + Sync {
    move |req, _| {
        if last_is_user(req) {
            R::Tools(
                None,
                vec![(
                    "bash".into(),
                    json!({"command": format!("echo 'token {SECRET} and sk-abcdefghijklmnop'")}),
                )],
            )
        } else {
            R::Text("the reply text".into())
        }
    }
}

struct Collector {
    rt: Option<tokio::runtime::Runtime>,
    server: MockServer,
}

impl Drop for Collector {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

impl Collector {
    fn start() -> Collector {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let status = Arc::new(AtomicU16::new(200));
        let server = {
            let status = status.clone();
            rt.block_on(MockServer::start(move |_: &Recorded, _| {
                MockResponse::json(
                    status.load(std::sync::atomic::Ordering::Relaxed),
                    &json!({}),
                )
            }))
        };
        Collector {
            rt: Some(rt),
            server,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/traces", self.server.url)
    }

    fn posts(&self) -> Vec<Recorded> {
        self.server
            .requests()
            .into_iter()
            .filter(|r| r.method == "POST")
            .collect()
    }

    /// Posts until one carries the run span of `request`.
    fn until_run(&self, request: &str) -> Vec<Recorded> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let posts = self.posts();
            if spans_of(&posts)
                .iter()
                .any(|sp| sp["name"] == "run" && span_attr(sp, "pipkin.request") == Some(request))
            {
                return posts;
            }
            assert!(Instant::now() < deadline, "no run span for {request}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn until_posts(&self, n: usize) -> Vec<Recorded> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let posts = self.posts();
            if posts.len() >= n {
                return posts;
            }
            assert!(Instant::now() < deadline, "only {} posts", posts.len());
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

fn health(e: &mut Engine) -> ExportHealthView {
    match e.call(Call::ExportHealth) {
        Ok(Reply::ExportHealth(h)) => h,
        other => panic!("{other:?}"),
    }
}

fn configure(
    e: &mut Engine,
    enabled: bool,
    endpoint: Option<&str>,
    content: &str,
) -> Result<ExportHealthView, ProtoError> {
    match e.call(Call::ExportConfigure {
        enabled,
        endpoint: endpoint.map(str::to_owned),
        content: content.into(),
        service_name: Some("my-app".into()),
    }) {
        Ok(Reply::ExportHealth(h)) => Ok(h),
        Ok(other) => panic!("{other:?}"),
        Err(e) => Err(e),
    }
}

fn preview(e: &mut Engine, session: Option<&str>, content: &str) -> String {
    match e.call(Call::ExportPreview {
        session: session.map(str::to_owned),
        content: content.into(),
    }) {
        Ok(Reply::ExportPreview { payload, .. }) => payload,
        other => panic!("{other:?}"),
    }
}

fn settings_file(fx: &Fixture) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(fx.dir.path().join("config/settings.json")).unwrap_or("{}".into()),
    )
    .unwrap()
}

fn span_attr<'a>(span: &'a Value, key: &str) -> Option<&'a str> {
    span["attributes"]
        .as_array()?
        .iter()
        .find(|a| a["key"] == key)?["value"]["stringValue"]
        .as_str()
}

fn spans_of(posts: &[Recorded]) -> Vec<Value> {
    posts
        .iter()
        .flat_map(|r| {
            r.json()["resourceSpans"][0]["scopeSpans"][0]["spans"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

#[test]
fn configuration_and_the_credential_are_written_where_they_belong_and_reported_safely() {
    let collector = Collector::start();
    let fx = Fixture::new();
    let provider = Scripted::new(script());
    let mut e = fx.start(&provider);
    // Off by default; the capability is advertised.
    let h = health(&mut e);
    assert!(!h.enabled && h.state == "off" && h.credential.is_none());
    let Ok(Reply::Welcome(w)) = e.call(Call::Hello {
        protocol: PROTOCOL_VERSION,
        client: "x".into(),
    }) else {
        panic!()
    };
    assert!(w.capabilities.iter().any(|c| c == "exportConfig"));
    // Enabling needs an endpoint; a bad level is refused.
    assert_eq!(
        configure(&mut e, true, None, "metadata").unwrap_err().code,
        ErrorCode::Invalid
    );
    assert_eq!(
        configure(&mut e, true, Some("ftp://x"), "metadata")
            .unwrap_err()
            .code,
        ErrorCode::Invalid
    );
    assert_eq!(
        configure(&mut e, false, None, "everything")
            .unwrap_err()
            .code,
        ErrorCode::Invalid
    );
    // A good configuration lands in the user settings file and the health at once.
    let endpoint = collector.endpoint();
    let h = configure(&mut e, true, Some(&format!("{endpoint}?x=1")), "metadata").unwrap();
    assert!(h.enabled);
    assert_eq!(h.state, "idle");
    assert_eq!(h.kind, "otlp");
    assert_eq!(h.content, "metadata");
    assert_eq!(h.service_name.as_deref(), Some("my-app"));
    assert_eq!(h.endpoint_url.as_deref(), Some(endpoint.as_str()));
    assert_eq!(
        h.endpoint.as_deref(),
        Some(collector.server.url.trim_start_matches("http://"))
    );
    let file = settings_file(&fx);
    assert_eq!(file["export"]["enabled"], true);
    assert_eq!(file["export"]["endpoint"], format!("{endpoint}?x=1"));
    assert_eq!(file["export"]["content"], "metadata");
    assert_eq!(file["export"]["serviceName"], "my-app");
    assert!(file["export"].get("headers").is_none());
    // The credential: private file, 0600, never in settings or the health.
    assert_eq!(
        e.call(Call::ExportSetCredential {
            header: "bad header".into(),
            value: Secret(SECRET.into())
        })
        .unwrap_err()
        .code,
        ErrorCode::Invalid
    );
    let Ok(Reply::ExportHealth(h)) = e.call(Call::ExportSetCredential {
        header: "Authorization".into(),
        value: Secret(SECRET.into()),
    }) else {
        panic!()
    };
    assert_eq!(h.credential.as_deref(), Some("Authorization"));
    assert!(!format!("{h:?}").contains("c2VjcmV0"));
    let auth_file = fx.dir.path().join("config/export-auth.json");
    assert!(auth_file.exists());
    assert!(
        std::fs::read_to_string(&auth_file)
            .unwrap()
            .contains("c2VjcmV0")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&auth_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(
        !std::fs::read_to_string(fx.dir.path().join("config/settings.json"))
            .unwrap()
            .contains("c2VjcmV0")
    );
    // It reaches the collector as the header.
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "go").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let posts = collector.until_posts(1);
    assert_eq!(
        posts[0].header("authorization"),
        Some(SECRET),
        "{:?}",
        posts[0].headers
    );
    assert_eq!(
        posts[0].json()["resourceSpans"][0]["resource"]["attributes"][0]["value"]["stringValue"],
        "my-app"
    );
    // Removing it: the next post carries no header.
    let Ok(Reply::ExportHealth(h)) = e.call(Call::ExportClearCredential) else {
        panic!()
    };
    assert!(h.credential.is_none());
    e.prompt(&s, "r2", "go").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let posts = collector.until_posts(2);
    assert_eq!(posts.last().unwrap().header("authorization"), None);
    // Turning it off keeps the rest of the configuration.
    let h = configure(&mut e, false, Some(&endpoint), "metadata").unwrap();
    assert!(
        !h.enabled && h.state == "off",
        "disabled export health: enabled={}, state={}",
        h.enabled,
        h.state
    );
    assert_eq!(h.endpoint_url.as_deref(), Some(endpoint.as_str()));
    assert_eq!(settings_file(&fx)["export"]["enabled"], false);
}

#[test]
fn the_content_level_adds_redacted_texts_that_metadata_never_carries() {
    let collector = Collector::start();
    let fx = Fixture::new();
    let provider = Scripted::new(script());
    let mut e = fx.start(&provider);
    e.call(Call::ExportSetCredential {
        header: "Authorization".into(),
        value: Secret(SECRET.into()),
    })
    .unwrap();
    configure(&mut e, true, Some(&collector.endpoint()), "metadata").unwrap();
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "the person's prompt text").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let posts = collector.until_run("r1");
    let spans = spans_of(&posts);
    let run = spans.iter().find(|sp| sp["name"] == "run").unwrap();
    assert!(
        span_attr(run, "pipkin.input").is_none(),
        "metadata carries no prompt"
    );
    for post in &posts {
        let body = String::from_utf8_lossy(&post.body);
        assert!(!body.contains("prompt text") && !body.contains("reply text"));
        assert!(!body.contains("c2VjcmV0"));
    }
    // Content level: texts appear, bounded and redacted.
    configure(&mut e, true, Some(&collector.endpoint()), "content").unwrap();
    e.prompt(&s, "r2", "another prompt text").unwrap();
    assert_eq!(e.settled("r2"), RunOutcome::Completed);
    let before = posts.len();
    let posts = collector.until_run("r2");
    let spans = spans_of(&posts[before..]);
    let run = spans
        .iter()
        .find(|sp| sp["name"] == "run" && span_attr(sp, "pipkin.request") == Some("r2"))
        .unwrap();
    assert_eq!(span_attr(run, "pipkin.input"), Some("another prompt text"));
    assert_eq!(span_attr(run, "pipkin.output"), Some("the reply text"));
    let generation = spans
        .iter()
        .find(|sp| {
            sp["name"] == "generation"
                && span_attr(sp, "gen_ai.completion") == Some("the reply text")
        })
        .expect("a completion");
    let _ = generation;
    let tool = spans.iter().find(|sp| sp["name"] == "tool bash").unwrap();
    assert!(
        span_attr(tool, "pipkin.tool.arguments")
            .unwrap()
            .contains("echo")
    );
    let output = span_attr(tool, "pipkin.tool.output").unwrap();
    assert!(output.contains("token [redacted]"), "{output}");
    assert!(output.contains("and [redacted]"), "{output}");
    assert!(
        !output.contains("c2VjcmV0") && !output.contains("sk-abcdef"),
        "{output}"
    );
    for post in &posts[before..] {
        let body = String::from_utf8_lossy(&post.body);
        assert!(!body.contains("c2VjcmV0"), "the credential leaked");
    }
    assert_eq!(health(&mut e).content, "content");
}

#[test]
fn the_preview_shows_a_payload_at_either_level_without_sending() {
    let collector = Collector::start();
    let fx = Fixture::new();
    let provider = Scripted::new(script());
    let mut e = fx.start(&provider);
    // No session: a sample, at each level.
    let meta = preview(&mut e, None, "metadata");
    let body: Value = serde_json::from_str(&meta).unwrap();
    let spans = body["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array()
        .unwrap();
    assert!(spans.iter().any(|sp| sp["name"] == "generation"));
    assert!(!meta.contains("gen_ai.completion"));
    let content = preview(&mut e, None, "content");
    assert!(content.contains("gen_ai.completion") && content.contains("pipkin.tool.output"));
    let bad = e.call(Call::ExportPreview {
        session: None,
        content: "full".into(),
    });
    assert!(
        matches!(&bad, Err(e) if e.code == ErrorCode::Invalid),
        "{:?}",
        bad.map(|_| "ok")
    );
    // A session with records: its own newest records, redacted; nothing is posted (export
    // is off).
    e.call(Call::ExportSetCredential {
        header: "Authorization".into(),
        value: Secret(SECRET.into()),
    })
    .unwrap();
    let s = e.create(&fx.project, "c");
    e.prompt(&s, "r1", "secret-free prompt").unwrap();
    assert_eq!(e.settled("r1"), RunOutcome::Completed);
    let content = preview(&mut e, Some(&s), "content");
    assert!(content.contains("secret-free prompt"), "{content}");
    assert!(
        content.contains("[redacted]") && !content.contains("c2VjcmV0"),
        "{content}"
    );
    let meta = preview(&mut e, Some(&s), "metadata");
    assert!(!meta.contains("secret-free prompt"));
    assert!(meta.contains("\"pipkin.run\""));
    std::thread::sleep(Duration::from_millis(200));
    assert!(collector.posts().is_empty(), "a preview sends nothing");
    assert_eq!(
        e.call(Call::ExportPreview {
            session: Some("nope".into()),
            content: "metadata".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
}
