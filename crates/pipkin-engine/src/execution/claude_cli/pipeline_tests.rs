//! Fake executable -> real preflight/policy/supervisor -> bounded display projection.
//! No native tools/provider, installed CLI, account or real inference is involved.
use super::{
    events::{self, Event},
    policy, preflight, process,
};
use serde_json::json;
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
const SESSION: &str = "11111111-1111-4111-8111-111111111111";

struct Fixture {
    _dir: tempfile::TempDir,
    request: preflight::Request,
}
impl Fixture {
    fn new(session: &str, hang: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fixture-cli");
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let help = policy::REQUIRED_FLAGS
            .iter()
            .map(|f| format!("{f} fixture\n"))
            .collect::<String>();
        let records = [
            json!({"type":"system","subtype":"init","session_id":session,"tools":[]}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"message_start","message":{"id":"msg-fixture","role":"assistant","content":[]}}}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Synthetic fixture answer"}}}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_stop","index":0}}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"message_stop"}}),
            json!({"type":"assistant","session_id":session,"message":{"id":"msg-fixture","role":"assistant","content":[{"type":"text","text":"Synthetic fixture answer"}]}}),
            json!({"type":"result","session_id":session,"subtype":"success","is_error":false,"result":"Synthetic fixture answer"}),
        ];
        let wire = records.iter().map(|r| format!("{r}\n")).collect::<String>();
        let auth = r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"fixture@example.invalid","orgId":"fixture-org"}"#;
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) printf '%s\\n' '2.1.296 (Claude Code)'; exit ;;\n--help) printf '%s' '{help}'; exit ;;\nesac\nfor arg in \"$@\"; do\n if [ \"$arg\" = '--print' ]; then\n  cat > received-prompt\n  printf '%s' '{wire}'\n  {}\n  exit\n fi\ndone\nprintf '%s\\n' '{auth}'\n",
            if hang { "sleep 30" } else { ":" }
        );
        std::fs::write(&exe, script).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let environment = [
            ("HOME".into(), dir.path().as_os_str().to_owned()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
        .into_iter()
        .collect();
        let candidate = preflight::Candidate::inspect(&exe, &project, &environment).unwrap();
        Self {
            _dir: dir,
            request: preflight::Request {
                candidate,
                trust_executable: true,
                trust_project: true,
                environment,
                model: policy::ModelAlias::Sonnet,
            },
        }
    }
}

async fn run(
    fixture: Fixture,
    queue_capacity: usize,
    timeout: Option<Duration>,
) -> (
    process::Report,
    Result<events::Summary, events::Error>,
    Vec<Event>,
) {
    let Fixture { _dir, request } = fixture;
    let cwd = request.candidate.project().to_owned();
    let prepared = preflight::check(request, CancellationToken::new())
        .await
        .unwrap();
    let binding = prepared.binding(SESSION).unwrap();
    let prompt = "'; touch must-not-exist; # --dangerously-skip-permissions";
    let mut spec = prepared
        .launch_spec(&binding, policy::TurnMode::Fresh, prompt)
        .unwrap();
    if let Some(timeout) = timeout {
        spec.timeout = timeout;
    }
    let decoder = Arc::new(Mutex::new(events::Decoder::new(SESSION, &[]).unwrap()));
    let observed = decoder.clone();
    let (tx, mut rx) = tokio::sync::mpsc::channel(queue_capacity);
    let final_tx = tx.clone();
    let report = process::run_observed(
        spec,
        CancellationToken::new(),
        None,
        Some(Box::new(move |bytes| {
            observed
                .try_lock()
                .map_err(|_| "projection busy")?
                .feed(bytes, |event| tx.try_send(event))
                .map_err(|_| "projection rejected")
        })),
    )
    .await
    .unwrap();
    let decoder = match Arc::try_unwrap(decoder) {
        Ok(decoder) => decoder.into_inner().ok().unwrap(),
        Err(_) => panic!("observer retained after report"),
    };
    let summary = decoder.finish(|event| final_tx.try_send(event));
    drop(final_tx);
    let mut events = vec![];
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    assert_eq!(
        std::fs::read(cwd.join("received-prompt")).unwrap(),
        format!("{prompt}\n").as_bytes()
    );
    assert!(!cwd.join("must-not-exist").exists());
    (report, summary, events)
}

#[tokio::test]
async fn fake_cli_flows_through_real_preflight_policy_supervision_and_projection_once() {
    let (report, summary, events) = run(Fixture::new(SESSION, false), 64, None).await;
    assert!(report.clean_exit());
    assert_eq!(summary.unwrap().outcome, Some(events::Outcome::Success));
    assert!(report.stream.unwrap().result_success);
    assert_eq!(
        events
            .iter()
            .filter_map(|e| match e {
                Event::BlockDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        "Synthetic fixture answer"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::BlockSnapshot { .. }))
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Terminal { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn wrong_session_or_sink_pressure_prevents_success_even_with_valid_terminal_output() {
    for (session, capacity) in [("22222222-2222-4222-8222-222222222222", 64), (SESSION, 1)] {
        let (report, summary, _) = run(Fixture::new(session, false), capacity, None).await;
        assert_eq!(report.stop, Some(process::Stop::ObserverFailure));
        assert!(!report.clean_exit());
        assert!(report.root_reaped && report.pipes_drained);
        assert!(summary.is_err());
    }
}

#[tokio::test]
async fn terminal_projection_is_provisional_until_process_supervision_settles() {
    let (report, summary, events) = run(
        Fixture::new(SESSION, true),
        64,
        Some(Duration::from_millis(150)),
    )
    .await;
    assert_eq!(summary.unwrap().outcome, Some(events::Outcome::Success));
    assert!(events.iter().any(|e| matches!(e, Event::Terminal { .. })));
    assert_eq!(report.stop, Some(process::Stop::TimedOut));
    assert!(!report.clean_exit());
    assert!(report.root_reaped && report.pipes_drained);
}
