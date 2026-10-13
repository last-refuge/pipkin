//! Engine diagnostics in the core (OBS-1): fetched only from a connected real engine for an
//! opened conversation, one fetch and one export in flight at a time, the answers fill the
//! section and a refusal becomes its error, and the section resets when the engine goes away.

use pipkin_core::diagnostics::*;
use pipkin_core::*;

fn state(mode: Mode) -> AppState {
    let mut state = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "P".into(),
                path: "/tmp/p".into(),
            }],
            conversations: vec![(ConversationId(1), ProjectId(1), "one".into(), 1)],
            models: vec![],
            now: 1,
        },
        Prefs::default(),
    );
    state.mode = mode;
    state.set_connection(Connection::Ready);
    state.dispatch(Command::SelectConversation(ConversationId(1)));
    state
}

fn event(s: &AppState, kind: EventKind) -> BackendEvent {
    let c = s.current().unwrap();
    BackendEvent {
        conversation: c.id,
        generation: c.generation,
        op: None,
        kind,
    }
}

fn open(s: &mut AppState) {
    let ev = event(
        s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    s.apply_event(ev);
}

fn backend(out: &Outcome) -> Vec<BackendRequest> {
    out.effects
        .iter()
        .filter_map(|e| match e {
            Effect::Backend(r) => Some(r.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn diagnostics_are_fetched_once_at_a_time_from_a_real_engine_and_fill_the_section() {
    let mut s = state(Mode::Real);
    // Not opened yet: nothing.
    assert!(backend(&s.dispatch(Command::RefreshDiagnostics)).is_empty());
    open(&mut s);
    let out = s.dispatch(Command::RefreshDiagnostics);
    let generation = s.current().unwrap().generation;
    assert_eq!(
        backend(&out),
        [BackendRequest::Diagnostics {
            conversation: ConversationId(1),
            generation,
        }]
    );
    assert!(s.current().unwrap().diagnostics.loading);
    assert!(
        backend(&s.dispatch(Command::RefreshDiagnostics)).is_empty(),
        "one in flight"
    );
    let summary = DiagnosticsSummary {
        records: 5,
        runs: 1,
        provider_requests: 2,
        tool_calls: 1,
        provider_p50_ms: Some(120),
        ..DiagnosticsSummary::default()
    };
    s.apply_event(event(
        &s,
        EventKind::DiagnosticsSynced {
            summary: summary.clone(),
            records: vec![DiagnosticRecord {
                seq: 5,
                at_ms: 1,
                kind: "run.end".into(),
                run: Some(1),
                call_id: None,
                line: "run.end · completed · 1.2 s".into(),
            }],
        },
    ));
    let d = &s.current().unwrap().diagnostics;
    assert!(d.available && !d.loading && d.error.is_none());
    assert_eq!(d.summary.as_ref(), Some(&summary));
    assert_eq!(d.records.len(), 1);
    // Export: once at a time, answered with the path.
    let out = s.dispatch(Command::ExportDiagnostics);
    assert_eq!(
        backend(&out),
        [BackendRequest::ExportDiagnostics {
            conversation: ConversationId(1),
            generation,
        }]
    );
    assert!(backend(&s.dispatch(Command::ExportDiagnostics)).is_empty());
    s.apply_event(event(
        &s,
        EventKind::DiagnosticsExported {
            path: "/data/diagnostics/s.jsonl".into(),
            records: 5,
        },
    ));
    let d = &s.current().unwrap().diagnostics;
    assert!(!d.exporting);
    assert_eq!(
        d.exported.as_deref(),
        Some("/data/diagnostics/s.jsonl (5 records)")
    );
    // A refusal lands as the section's error and clears the flags.
    s.dispatch(Command::RefreshDiagnostics);
    s.apply_event(event(
        &s,
        EventKind::DiagnosticsFailed {
            reason: "Could not read the diagnostics: gone".into(),
        },
    ));
    let d = &s.current().unwrap().diagnostics;
    assert!(!d.loading);
    assert_eq!(
        d.error.as_deref(),
        Some("Could not read the diagnostics: gone")
    );
    assert!(d.available, "an earlier answer still shows");
    // The engine going away resets the section.
    s.set_connection(Connection::Reconnecting);
    assert_eq!(
        s.current().unwrap().diagnostics,
        DiagnosticsState::default()
    );
}

#[test]
fn the_demo_and_a_disconnected_engine_fetch_nothing() {
    let mut s = state(Mode::Demo);
    open(&mut s);
    assert!(backend(&s.dispatch(Command::RefreshDiagnostics)).is_empty());
    assert!(backend(&s.dispatch(Command::ExportDiagnostics)).is_empty());
    let mut s = state(Mode::Real);
    open(&mut s);
    s.set_connection(Connection::Connecting);
    assert!(backend(&s.dispatch(Command::RefreshDiagnostics)).is_empty());
}

#[test]
fn milliseconds_are_shown_at_a_readable_precision() {
    assert_eq!(format_ms(0), "0 ms");
    assert_eq!(format_ms(999), "999 ms");
    assert_eq!(format_ms(1_250), "1.3 s");
    assert_eq!(format_ms(12_400), "12 s");
}

#[test]
fn the_export_health_is_app_wide_and_summarized_in_one_line() {
    let mut s = state(Mode::Real);
    assert!(s.export_health.is_none());
    let off = ExportHealth {
        state: "off".into(),
        content: "metadata".into(),
        ..ExportHealth::default()
    };
    assert!(!s.set_export_health(off.clone()).notes.is_empty());
    assert!(
        s.set_export_health(off.clone()).notes.is_empty(),
        "unchanged"
    );
    assert_eq!(s.export_health.as_ref().unwrap().summary(), "Off");
    let busy = ExportHealth {
        enabled: true,
        state: "backoff".into(),
        endpoint: Some("collector.example:4318".into()),
        content: "metadata".into(),
        queued: 7,
        sent: 120,
        dropped: 3,
        failures: 2,
        last_error: Some("the collector answered 503".into()),
        last_success_ms: Some(1),
        ..ExportHealth::default()
    };
    s.set_export_health(busy);
    assert_eq!(
        s.export_health.as_ref().unwrap().summary(),
        "Retrying to collector.example:4318 · 120 sent · 7 waiting · 3 dropped · the collector answered 503"
    );
}

#[test]
fn export_settings_commands_go_to_a_real_engine_one_at_a_time_and_answers_settle_them() {
    use pipkin_core::onboarding::SignInAnswer;
    let mut s = state(Mode::Real);
    let config = ExportConfig {
        enabled: true,
        endpoint: "http://127.0.0.1:4318/v1/traces".into(),
        content: "content".into(),
        service_name: "app".into(),
    };
    let out = s.dispatch(Command::ExportConfigure(config.clone()));
    assert_eq!(
        backend(&out),
        [BackendRequest::ExportConfigure(config.clone())]
    );
    assert_eq!(s.export_page.busy.as_deref(), Some("Saving…"));
    // One at a time.
    assert!(backend(&s.dispatch(Command::ExportPreview("metadata".into()))).is_empty());
    // The health answers it.
    s.set_export_health(ExportHealth {
        enabled: true,
        state: "idle".into(),
        endpoint_url: Some(config.endpoint.clone()),
        content: "content".into(),
        service_name: Some("app".into()),
        ..ExportHealth::default()
    });
    assert!(s.export_page.busy.is_none());
    // Disable keeps the configuration.
    let out = s.dispatch(Command::ExportDisable);
    assert_eq!(
        backend(&out),
        [BackendRequest::ExportConfigure(ExportConfig {
            enabled: false,
            ..config.clone()
        })]
    );
    s.export_failed("Cannot write settings".into());
    assert_eq!(
        s.export_page.notice.as_deref(),
        Some("Cannot write settings")
    );
    assert!(s.export_page.busy.is_none());
    // The credential needs both parts; the value is never printed.
    let out = s.dispatch(Command::ExportSetCredential {
        header: "Authorization".into(),
        value: SignInAnswer::new(" ".into()),
    });
    assert!(backend(&out).is_empty());
    assert!(s.export_page.notice.as_deref().unwrap().contains("needed"));
    let out = s.dispatch(Command::ExportSetCredential {
        header: " Authorization ".into(),
        value: SignInAnswer::new("Basic abc".into()),
    });
    let requests = backend(&out);
    assert!(
        matches!(&requests[..], [BackendRequest::ExportSetCredential { header, .. }] if header == "Authorization")
    );
    assert!(!format!("{requests:?}").contains("Basic abc"));
    s.set_export_health(ExportHealth::default());
    // The preview lands on the page with its level.
    let out = s.dispatch(Command::ExportPreview("content".into()));
    assert_eq!(
        backend(&out),
        [BackendRequest::ExportPreview {
            conversation: Some(ConversationId(1)),
            content: "content".into()
        }]
    );
    s.export_previewed("content".into(), "{}".into());
    assert_eq!(s.export_page.preview, Some(("content".into(), "{}".into())));
    s.dispatch(Command::ExportDismissNotice);
    assert!(s.export_page.preview.is_none() && s.export_page.notice.is_none());
    // The demo sends nothing.
    let mut d = state(Mode::Demo);
    assert!(backend(&d.dispatch(Command::ExportConfigure(config))).is_empty());
}
