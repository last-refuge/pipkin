//! Per-child controls (SUB-2/SUB-3) in the core: Stop and Steer go to the engine only for the
//! selected, active child (a stale number cannot reach a sibling), one request at a time,
//! only from a connected real engine; Launch needs a task; answers settle the pending request
//! and a refusal becomes the directory's error.

use pipkin_core::*;
mod support;

fn state_in_mode(mode: Mode) -> AppState {
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
    support::native_context(&mut state);
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

fn child(id: u64, status: &str) -> SubagentInfo {
    SubagentInfo {
        id,
        task_id: id,
        call_id: format!("call-{id}"),
        task: format!("Task {id}."),
        status: status.into(),
        ..SubagentInfo::default()
    }
}

fn ready(s: &mut AppState, children: Vec<SubagentInfo>) {
    s.apply_event(event(
        s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    ));
    s.apply_event(event(
        s,
        EventKind::SubagentDirectory {
            enabled: true,
            children,
            truncated: false,
        },
    ));
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

fn pending(s: &AppState) -> Option<RequestId> {
    s.current().unwrap().subagents.control_pending.clone()
}

#[test]
fn stop_reaches_the_engine_only_for_the_selected_active_child() {
    let mut s = state_in_mode(Mode::Real);
    ready(
        &mut s,
        vec![
            child(10, "running"),
            child(11, "running"),
            child(12, "done"),
        ],
    );
    // Nothing selected: nothing to stop.
    assert!(backend(&s.dispatch(Command::StopSubagent(10))).is_empty());
    s.dispatch(Command::SelectSubagent(Some(10)));
    // A sibling's number is refused even while another child is selected.
    assert!(backend(&s.dispatch(Command::StopSubagent(11))).is_empty());
    let out = s.dispatch(Command::StopSubagent(10));
    let request = pending(&s).expect("one request in flight");
    assert_eq!(
        backend(&out),
        [BackendRequest::StopSubagent {
            conversation: ConversationId(1),
            generation: s.current().unwrap().generation,
            request: request.clone(),
            child: 10,
        }]
    );
    // One at a time.
    assert!(backend(&s.dispatch(Command::StopSubagent(10))).is_empty());
    assert!(
        backend(&s.dispatch(Command::SteerSubagent {
            child: 10,
            text: "hi".into()
        }))
        .is_empty()
    );
    // An answer for another request changes nothing; the matching one settles it.
    s.apply_event(event(
        &s,
        EventKind::SubagentControlDone {
            request: RequestId("other".into()),
        },
    ));
    assert_eq!(pending(&s), Some(request.clone()));
    s.apply_event(event(&s, EventKind::SubagentControlDone { request }));
    assert_eq!(pending(&s), None);
    assert!(s.current().unwrap().subagents.error.is_none());
    // A finished child cannot be stopped; a stopping one (note on record) neither.
    s.dispatch(Command::SelectSubagent(Some(12)));
    assert!(backend(&s.dispatch(Command::StopSubagent(12))).is_empty());
    let mut stopping = child(11, "running");
    stopping.note = Some("Stopped by the person".into());
    assert!(stopping.stopping());
    s.apply_event(event(
        &s,
        EventKind::SubagentDirectory {
            enabled: true,
            children: vec![child(10, "running"), stopping, child(12, "done")],
            truncated: false,
        },
    ));
    s.dispatch(Command::SelectSubagent(Some(11)));
    assert!(backend(&s.dispatch(Command::StopSubagent(11))).is_empty());
}

#[test]
fn steer_needs_text_and_a_refusal_becomes_the_directory_error() {
    let mut s = state_in_mode(Mode::Real);
    ready(&mut s, vec![child(10, "running")]);
    s.dispatch(Command::SelectSubagent(Some(10)));
    assert!(
        backend(&s.dispatch(Command::SteerSubagent {
            child: 10,
            text: "   ".into()
        }))
        .is_empty()
    );
    let out = s.dispatch(Command::SteerSubagent {
        child: 10,
        text: "  use the other file  ".into(),
    });
    let request = pending(&s).unwrap();
    assert_eq!(
        backend(&out),
        [BackendRequest::SteerSubagent {
            conversation: ConversationId(1),
            generation: s.current().unwrap().generation,
            request: request.clone(),
            child: 10,
            text: "use the other file".into(),
        }]
    );
    s.apply_event(event(
        &s,
        EventKind::SubagentControlFailed {
            request,
            reason: "Could not steer the subagent: gone".into(),
        },
    ));
    assert_eq!(pending(&s), None);
    assert_eq!(
        s.current().unwrap().subagents.error.as_deref(),
        Some("Could not steer the subagent: gone")
    );
    // The next request clears the message.
    s.dispatch(Command::SteerSubagent {
        child: 10,
        text: "again".into(),
    });
    assert!(s.current().unwrap().subagents.error.is_none());
    assert!(pending(&s).is_some());
}

#[test]
fn launch_needs_a_task_a_real_engine_and_an_advertised_directory() {
    let mut s = state_in_mode(Mode::Real);
    // Not opened / no directory yet: nothing.
    assert!(backend(&s.dispatch(Command::LaunchSubagent("survey".into()))).is_empty());
    ready(&mut s, vec![]);
    assert!(backend(&s.dispatch(Command::LaunchSubagent("  ".into()))).is_empty());
    let out = s.dispatch(Command::LaunchSubagent(" survey the crate ".into()));
    let request = pending(&s).unwrap();
    assert_eq!(
        backend(&out),
        [BackendRequest::LaunchSubagent {
            conversation: ConversationId(1),
            generation: s.current().unwrap().generation,
            request,
            task: "survey the crate".into(),
        }]
    );
    // Disconnected: nothing is sent.
    let mut s = state_in_mode(Mode::Real);
    ready(&mut s, vec![child(10, "running")]);
    s.set_connection(Connection::Connecting);
    s.dispatch(Command::SelectSubagent(Some(10)));
    assert!(backend(&s.dispatch(Command::StopSubagent(10))).is_empty());
    assert!(backend(&s.dispatch(Command::LaunchSubagent("x".into()))).is_empty());
    // The demo never sends controls.
    let mut s = state_in_mode(Mode::Demo);
    ready(&mut s, vec![child(10, "running")]);
    s.dispatch(Command::SelectSubagent(Some(10)));
    assert!(backend(&s.dispatch(Command::StopSubagent(10))).is_empty());
    assert!(backend(&s.dispatch(Command::LaunchSubagent("x".into()))).is_empty());
}
