//! The jobs projection: engine-reported jobs, the person's view choices, log tails fetched
//! for the expanded job under a pending flag, Stop through the engine only from a connected
//! real engine, and an attention count for jobs that finish out of view.

use pipkin_core::*;

fn state() -> AppState {
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
    state.mode = Mode::Real;
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

fn open(state: &mut AppState) {
    let ev = event(
        state,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    state.apply_event(ev);
}

fn job(id: u64, state: JobState, log_bytes: u64) -> Job {
    Job {
        id,
        name: Some(format!("job{id}")),
        command: format!("cmd {id}"),
        cwd: "/tmp/p".into(),
        origin: JobOrigin::Model,
        state,
        exit_code: None,
        reason: None,
        call_id: Some("call-1".into()),
        task: None,
        wake: false,
        delivery: "pending".into(),
        log_bytes,
        log_truncated: false,
        created_at_ms: 0,
        started_at_ms: Some(0),
        ended_at_ms: None,
    }
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
fn syncs_replace_the_list_and_count_jobs_that_finish_out_of_view() {
    let mut state = state();
    assert!(!state.current().unwrap().jobs.available);
    // Before the conversation is open, syncs are ignored.
    let ev = event(
        &state,
        EventKind::JobsSynced {
            jobs: JobsSync {
                jobs: vec![job(1, JobState::Running, 0)],
                active: 1,
                truncated: false,
            },
        },
    );
    state.apply_event(ev);
    assert!(!state.current().unwrap().jobs.available);
    open(&mut state);
    let ev = event(
        &state,
        EventKind::JobsSynced {
            jobs: JobsSync {
                jobs: vec![job(1, JobState::Running, 0)],
                active: 1,
                truncated: false,
            },
        },
    );
    state.apply_event(ev);
    let jobs = &state.current().unwrap().jobs;
    assert!(jobs.available);
    assert_eq!(jobs.active, 1);
    assert_eq!(jobs.unseen, 0);
    let ev = event(
        &state,
        EventKind::JobsSynced {
            jobs: JobsSync {
                jobs: vec![job(1, JobState::Succeeded, 10)],
                active: 0,
                truncated: false,
            },
        },
    );
    state.apply_event(ev);
    assert_eq!(state.current().unwrap().jobs.unseen, 1);
    state.dispatch(Command::MarkJobsSeen);
    assert_eq!(state.current().unwrap().jobs.unseen, 0);
    // Jobs that were already finished when first seen do not count.
    let ev = event(
        &state,
        EventKind::JobsSynced {
            jobs: JobsSync {
                jobs: vec![job(2, JobState::Failed, 0), job(1, JobState::Succeeded, 10)],
                active: 0,
                truncated: true,
            },
        },
    );
    state.apply_event(ev);
    let jobs = &state.current().unwrap().jobs;
    assert_eq!(jobs.unseen, 0);
    assert!(jobs.truncated);
    assert_eq!(jobs.finished(), 2);
    state.dispatch(Command::ToggleJobHistory);
    assert!(state.current().unwrap().jobs.history_open);
}

#[test]
fn expanding_a_job_fetches_its_log_tail_once_and_again_when_it_grows() {
    let mut state = state();
    open(&mut state);
    let ev = event(
        &state,
        EventKind::JobsSynced {
            jobs: JobsSync {
                jobs: vec![job(1, JobState::Running, 10_000)],
                active: 1,
                truncated: false,
            },
        },
    );
    state.apply_event(ev);
    let out = state.dispatch(Command::ToggleJob(1));
    assert_eq!(
        backend(&out),
        [BackendRequest::JobLog {
            conversation: ConversationId(1),
            generation: state.current().unwrap().generation,
            id: 1,
            offset: 10_000 - JOB_LOG_TAIL_BYTES,
            limit: JOB_LOG_TAIL_BYTES,
        }]
    );
    assert_eq!(state.current().unwrap().jobs.pending_log, Some(1));
    // A refresh while one is pending sends nothing more.
    assert!(state.dispatch(Command::RefreshJobLog(1)).effects.is_empty());
    let ev = event(
        &state,
        EventKind::JobLogPage {
            tail: JobLogTail {
                id: 1,
                offset: 10_000 - JOB_LOG_TAIL_BYTES,
                text: "tail text".into(),
                next_offset: 10_000,
                total_bytes: 10_000,
                truncated: false,
                done: false,
            },
        },
    );
    state.apply_event(ev);
    let jobs = &state.current().unwrap().jobs;
    assert_eq!(jobs.pending_log, None);
    assert_eq!(jobs.log.as_ref().unwrap().text, "tail text");
    // The log grew: the sync asks for the newest tail again, exactly once.
    let ev = event(
        &state,
        EventKind::JobsSynced {
            jobs: JobsSync {
                jobs: vec![job(1, JobState::Running, 12_000)],
                active: 1,
                truncated: false,
            },
        },
    );
    let out = state.apply_event(ev);
    assert_eq!(backend(&out).len(), 1);
    assert_eq!(state.current().unwrap().jobs.pending_log, Some(1));
    let ev = event(
        &state,
        EventKind::JobsSynced {
            jobs: JobsSync {
                jobs: vec![job(1, JobState::Running, 13_000)],
                active: 1,
                truncated: false,
            },
        },
    );
    let out = state.apply_event(ev);
    assert!(
        backend(&out).is_empty(),
        "coalesced while a page is pending"
    );
    // An explicit refresh after the page arrives works; collapsing drops the log.
    let ev = event(
        &state,
        EventKind::JobLogPage {
            tail: JobLogTail {
                id: 1,
                offset: 0,
                text: "newer".into(),
                next_offset: 13_000,
                total_bytes: 13_000,
                truncated: false,
                done: false,
            },
        },
    );
    state.apply_event(ev);
    assert_eq!(backend(&state.dispatch(Command::RefreshJobLog(1))).len(), 1);
    state.dispatch(Command::ToggleJob(1));
    let jobs = &state.current().unwrap().jobs;
    assert_eq!(jobs.expanded, None);
    assert!(jobs.log.is_none() && jobs.pending_log.is_none());
    // A job that disappears from the list collapses too.
    state.dispatch(Command::ToggleJob(1));
    let ev = event(
        &state,
        EventKind::JobsSynced {
            jobs: JobsSync {
                jobs: vec![],
                active: 0,
                truncated: false,
            },
        },
    );
    state.apply_event(ev);
    assert_eq!(state.current().unwrap().jobs.expanded, None);
}

#[test]
fn stop_goes_to_the_engine_only_for_active_jobs_on_a_real_engine() {
    let mut state = state();
    open(&mut state);
    let ev = event(
        &state,
        EventKind::JobsSynced {
            jobs: JobsSync {
                jobs: vec![job(1, JobState::Running, 0), job(2, JobState::Succeeded, 0)],
                active: 1,
                truncated: false,
            },
        },
    );
    state.apply_event(ev);
    assert!(state.dispatch(Command::CancelJob(2)).effects.is_empty());
    assert!(state.dispatch(Command::CancelJob(9)).effects.is_empty());
    assert_eq!(
        backend(&state.dispatch(Command::CancelJob(1))),
        [BackendRequest::CancelJob {
            conversation: ConversationId(1),
            generation: state.current().unwrap().generation,
            id: 1
        }]
    );
    let ev = event(
        &state,
        EventKind::JobRefused {
            reason: "Could not stop the job: gone".into(),
        },
    );
    state.apply_event(ev);
    assert_eq!(
        state.current().unwrap().jobs.error.as_deref(),
        Some("Could not stop the job: gone")
    );
    state.dispatch(Command::DismissJobError);
    assert!(state.current().unwrap().jobs.error.is_none());
    state.mode = Mode::Demo;
    assert!(state.dispatch(Command::CancelJob(1)).effects.is_empty());
    assert!(
        state.dispatch(Command::ToggleJob(1)).effects.is_empty(),
        "no log fetch in the demo"
    );
    assert_eq!(state.current().unwrap().jobs.expanded, Some(1));
}
