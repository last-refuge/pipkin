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
