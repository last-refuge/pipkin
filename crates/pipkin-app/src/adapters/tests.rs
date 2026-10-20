//! Headless integration tests: the real `AppState` driven against the real `DemoBackend`
//! (speed 0, no GPUI). Effects are executed by the harness the way the controller would.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use pipkin_core::*;

use super::demo::{DemoBackend, DemoOptions, SETTLE_MS};
use super::fixtures::*;

const TIMEOUT: Duration = Duration::from_secs(20);

fn options(scenario: &str, hold_steer: bool) -> DemoOptions {
    DemoOptions {
        speed: 0.0,
        scenario: scenario.into(),
        hold_steer,
        ..DemoOptions::default()
    }
}

struct Harness {
    state: AppState,
    backend: DemoBackend,
    rx: async_channel::Receiver<BackendEvent>,
    log: Vec<BackendEvent>,
}

impl Harness {
    /// Starts with `conversation` selected and its history opened.
    fn new(scenario: &str, hold_steer: bool, conversation: ConversationId) -> Harness {
        let (tx, rx) = async_channel::unbounded();
        let backend = DemoBackend::new(tx, options(scenario, hold_steer));
        let state = AppState::new(backend.bootstrap(), Prefs::default());
        let mut h = Harness {
            state,
            backend,
            rx,
            log: Vec::new(),
        };
        h.dispatch(Command::SelectConversation(conversation));
        h.pump_until("history opened", |s| s.current().unwrap().opened);
        h
    }

    fn dispatch(&mut self, command: Command) {
        let outcome = self.state.dispatch(command);
        self.exec(outcome);
    }

    fn exec(&mut self, outcome: Outcome) {
        for effect in outcome.effects {
            match effect {
                Effect::SaveDashboard { .. } => {}
                Effect::Backend(request) => self.backend.request(request),
                Effect::JournalLocalCliCreation {
                    conversation,
                    generation,
                    op,
                    identity,
                } => {
                    let out = self.state.local_cli_creation_persisted(
                        conversation,
                        generation,
                        op,
                        &identity,
                        Ok(()),
                    );
                    self.exec(out);
                }
                Effect::RetireLocalCliCreation { .. } | Effect::SaveCliApprovalRecovery(_) => {}
                Effect::JournalCliApproval(request) => {
                    let out = self.state.cli_approval_persisted(&request, Ok(()));
                    self.exec(out);
                }
                // The journal commits instantly here; the controller does it asynchronously.
                Effect::JournalIntent {
                    conversation,
                    request,
                    ..
                } => {
                    let outcome = self.state.intent_persisted(conversation, &request, Ok(()));
                    self.exec(outcome);
                }
                Effect::JournalState { .. } => {}
                Effect::SaveDraft {
                    conversation, rev, ..
                } => self.state.draft_saved(conversation, rev, Ok(())),
                Effect::SavePrefs(_)
                | Effect::CompleteSetup(_)
                | Effect::SaveGoal { .. }
                | Effect::SaveConversation { .. }
                | Effect::SaveProject { .. }
                | Effect::SaveCache { .. }
                | Effect::LoadCache { .. }
                | Effect::SearchHistory { .. }
                | Effect::CopyText(_)
                | Effect::SaveText { .. }
                | Effect::Launch(_)
                | Effect::Import(_)
                | Effect::ReleaseOwnership { .. } => {}
            }
        }
    }

    fn apply_next(&mut self, what: &str) -> BackendEvent {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Ok(event) = self.rx.try_recv() {
                self.log.push(event.clone());
                let outcome = self.state.apply_event(event.clone());
                self.exec(outcome);
                return event;
            }
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn pump_until(&mut self, what: &str, done: impl Fn(&AppState) -> bool) {
        while !done(&self.state) {
            self.apply_next(what);
        }
    }

    fn submit(&mut self, text: &str) {
        self.dispatch(Command::EditDraft(text.into()));
        self.dispatch(Command::Submit);
    }

    fn run_to_idle(&mut self) {
        self.pump_until("run to settle", |s| {
            matches!(s.current().unwrap().run, RunState::Idle)
        });
    }

    fn conv(&self) -> &ConversationState {
        self.state.current().unwrap()
    }

    fn assistant_text(&self) -> String {
        self.conv()
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                ItemKind::Assistant { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn tools(&self) -> Vec<&ToolCall> {
        self.conv()
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                ItemKind::Tool(t) => Some(t),
                _ => None,
            })
            .collect()
    }
}

/// Send requests straight to a backend and collect events until `stop` matches.
fn raw_events(
    backend: &DemoBackend,
    rx: &async_channel::Receiver<BackendEvent>,
    requests: Vec<BackendRequest>,
    stop: impl Fn(&BackendEvent) -> bool,
) -> Vec<BackendEvent> {
    for r in requests {
        backend.request(r);
    }
    let deadline = Instant::now() + TIMEOUT;
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(e) => {
                let done = stop(&e);
                out.push(e);
                if done {
                    return out;
                }
            }
            Err(_) => {
                assert!(
                    Instant::now() < deadline,
                    "timed out; got {} events",
                    out.len()
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn submit_request(conv: u64, generation: u64, op: u64, text: &str) -> BackendRequest {
    BackendRequest::Submit {
        conversation: ConversationId(conv),
        generation,
        op: OperationId(op),
        request: RequestId(format!("test-{op}")),
        text: text.into(),
        attachments: vec![],
        model: Some("pi-sonnet".into()),
    }
}

fn is_terminal(e: &BackendEvent) -> bool {
    matches!(
        e.kind,
        EventKind::Completed | EventKind::Failed { .. } | EventKind::Rejected { .. }
    )
}

// ------------------------------------------------------------------ bootstrap and history

#[test]
fn bootstrap_has_seeded_projects_models_and_conversations() {
    let (tx, _rx) = async_channel::unbounded();
    let backend = DemoBackend::new(tx, options("normal", false));
    let boot = backend.bootstrap();
    assert_eq!(boot.projects.len(), 2);
    assert!(boot.conversations.len() >= 10);
    assert!(boot.models.len() >= 3);
    assert!(
        boot.conversations.iter().any(|c| c.2.chars().count() > 100),
        "a very long title"
    );
    assert!(
        boot.conversations.iter().any(|c| !c.2.is_ascii()),
        "a Unicode title"
    );
    assert_eq!(boot.now, DEFAULT_BASE_TIME);
}

#[test]
fn opening_every_conversation_returns_at_most_one_page() {
    let (tx, rx) = async_channel::unbounded();
    let backend = DemoBackend::new(tx, options("normal", false));
    for spec in SPECS {
        let e = raw_events(
            &backend,
            &rx,
            vec![BackendRequest::Open {
                conversation: ConversationId(spec.id),
                generation: 1,
            }],
            |_| true,
        );
        let EventKind::Opened {
            items, has_older, ..
        } = &e[0].kind
        else {
            panic!("expected Opened")
        };
        assert!(items.len() <= PAGE, "conversation {}", spec.id);
        assert_eq!(
            *has_older,
            spec.kind == Kind::Large,
            "conversation {}",
            spec.id
        );
        assert_eq!(
            (e[0].conversation, e[0].generation, e[0].op),
            (ConversationId(spec.id), 1, None)
        );
    }
}

#[test]
fn large_history_pages_through_all_ten_thousand_items_without_duplicates() {
    let mut h = Harness::new("normal", false, LARGE_CONVERSATION);
    assert_eq!(h.conv().items.len(), 200);
    assert!(h.conv().has_older);
    let mut previous_len = 200;
    let mut pages = 0;
    while h.conv().has_older {
        h.dispatch(Command::LoadOlder);
        assert!(h.conv().loading_older);
        h.pump_until("older page", |s| !s.current().unwrap().loading_older);
        let len = h.conv().items.len();
        assert!(len - previous_len <= 200 && len > previous_len);
        previous_len = len;
        pages += 1;
    }
    assert_eq!(h.conv().items.len(), 10_000);
    assert_eq!(pages, 49);
    let ids: HashSet<_> = h.conv().items.iter().map(|i| i.id).collect();
    assert_eq!(ids.len(), 10_000, "duplicate item ids");
    assert!(
        h.conv().items.windows(2).all(|w| w[0].id < w[1].id),
        "ids are ordered"
    );
    // Mixed content.
    let kinds: HashSet<_> = h
        .conv()
        .items
        .iter()
        .map(|i| std::mem::discriminant(&i.kind))
        .collect();
    assert!(kinds.len() >= 4);
    assert!(
        h.tools().iter().any(|t| t.truncated),
        "some tool rows carry big output"
    );
}

#[test]
fn large_diff_and_huge_tool_output_conversations() {
    let h = Harness::new("normal", false, LARGE_DIFF_CONVERSATION);
    let changed: u32 = h.conv().changes.iter().map(|c| c.added + c.removed).sum();
    assert!(changed >= 2000, "{changed} changed lines");
    assert!(h.conv().changes.len() >= 3);

    let h = Harness::new("normal", false, BIG_OUTPUT_CONVERSATION);
    let big = h
        .tools()
        .into_iter()
        .find(|t| t.full_len > 1_000_000)
        .expect("1 MB+ tool");
    assert!(big.truncated && big.output.len() <= TOOL_OUTPUT_PREVIEW_BYTES);
}

#[test]
fn empty_and_stress_conversations_open() {
    let h = Harness::new("normal", false, EMPTY_CONVERSATION);
    assert!(h.conv().items.is_empty() && !h.conv().has_older);
    let h = Harness::new("normal", false, STRESS_CONVERSATION);
    assert!(h.conv().items.len() >= 12);
    assert!(h.conv().items.iter().any(|i| matches!(&i.kind,
        ItemKind::User { attachments, .. } if attachments.iter().any(|a| a.error.is_some()))));
}

// ------------------------------------------------------------------------ scenarios

#[test]
fn normal_scenario_tells_the_whole_story() {
    let mut h = Harness::new("normal", false, EMPTY_CONVERSATION);
    h.submit("please fix my billing bug");
    assert!(matches!(h.conv().run, RunState::Submitting { .. }));
    h.run_to_idle();

    assert_eq!(h.conv().changes.len(), 3);
    for change in &h.conv().changes {
        assert!(change.added > 0 && change.hunks.iter().all(|k| k.header.starts_with("@@ -")));
    }
    let tools = h.tools();
    assert!(tools.iter().filter(|t| t.name == "read_file").count() >= 3);
    assert!(tools.iter().any(|t| t.name == "edit"));
    let bashes: Vec<_> = tools.iter().filter(|t| t.name == "bash").collect();
    assert_eq!(bashes.len(), 2);
    assert_eq!(bashes[0].status, ToolStatus::Failed);
    assert!(bashes[0].output.contains("FAILED"));
    let last = tools.last().unwrap();
    assert_eq!((last.name.as_str(), last.status), ("bash", ToolStatus::Ok));
    assert!(last.output.contains("4 passed"));

    let text = h.assistant_text();
    assert!(text.contains("Scripted demo response") && text.contains("not read or acted on"));
    assert!(
        text.contains("please fix my billing bug"),
        "echoes the prompt as inert text"
    );
    assert!(text.contains("## Summary"));
    assert!(matches!(
        &h.conv().items[0].kind,
        ItemKind::User {
            delivery: Delivery::Sent,
            ..
        }
    ));
    assert!(h.conv().items.iter().all(|i| !matches!(
        &i.kind,
        ItemKind::Assistant {
            streaming: true,
            ..
        }
    )));
    // The stream was chunked.
    let tokens = h
        .log
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Token(_)))
        .count();
    assert!(tokens > 20);
}

#[test]
fn persist_fail_behaves_as_normal() {
    let mut h = Harness::new("persist-fail", false, EMPTY_CONVERSATION);
    h.submit("anything");
    h.run_to_idle();
    assert_eq!(h.conv().changes.len(), 3);
}

#[test]
fn followup_steer_is_acknowledged_by_the_remaining_script() {
    let mut h = Harness::new("followup", true, EMPTY_CONVERSATION);
    h.submit("start the long task");
    h.pump_until("checkpoint 1", |s| {
        s.current().is_some() && text_of(s).contains("Steer me now")
    });
    h.dispatch(Command::EditDraft("use plan B instead".into()));
    h.dispatch(Command::Steer);
    h.pump_until("steer accepted", |s| {
        matches!(
            s.current()
                .unwrap()
                .items
                .iter()
                .rev()
                .find_map(|i| match &i.kind {
                    ItemKind::User {
                        steer: true,
                        delivery,
                        ..
                    } => Some(*delivery),
                    _ => None,
                }),
            Some(Delivery::Sent)
        )
    });
    assert!(h.log.iter().any(|e| e.kind == EventKind::SteerAccepted));
    h.pump_until("course change", |s| text_of(s).contains("(simulated)"));
    assert!(text_of(&h.state).contains("use plan B instead"));
    assert!(!text_of(&h.state).contains("No steering received"));

    // The script holds at each steer point (hold_steer); steer through the remaining two.
    for marker in ["Checkpoint 2", "Checkpoint 3"] {
        h.pump_until(marker, |s| text_of(s).contains(marker));
        h.dispatch(Command::EditDraft("keep going".into()));
        h.dispatch(Command::Steer);
    }
    h.run_to_idle();
    assert!(text_of(&h.state).contains("Still honouring your steer"));
    assert_eq!(h.conv().changes.len(), 3);
}

fn text_of(s: &AppState) -> String {
    s.current()
        .unwrap()
        .items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Assistant { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn followup_queued_prompts_run_in_order_after_completion() {
    let mut h = Harness::new("followup", false, EMPTY_CONVERSATION);
    h.submit("first prompt");
    for text in ["second prompt", "third prompt"] {
        h.dispatch(Command::EditDraft(text.into()));
        assert!(h.state.availability().queue);
        h.dispatch(Command::QueueFollowUp);
    }
    assert_eq!(h.conv().queue.len(), 2);
    h.run_to_idle();
    assert!(h.conv().queue.is_empty());
    let texts: Vec<_> = h
        .backend
        .submissions()
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert_eq!(texts, ["first prompt", "second prompt", "third prompt"]);
    let users: Vec<_> = h
        .conv()
        .items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::User { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["first prompt", "second prompt", "third prompt"]);
    // Each run finished before the next began: three Completed events, in sequence.
    let completed = h
        .log
        .iter()
        .filter(|e| e.kind == EventKind::Completed)
        .count();
    assert_eq!(completed, 3);
}

#[test]
fn cancel_stays_stopping_until_the_backend_confirms() {
    let mut h = Harness::new("followup", true, EMPTY_CONVERSATION);
    h.submit("a task I will cancel");
    h.pump_until("checkpoint 1", |s| text_of(s).contains("Steer me now"));
    h.dispatch(Command::EditDraft("queued while running".into()));
    h.dispatch(Command::QueueFollowUp);

    h.dispatch(Command::Cancel);
    assert!(matches!(h.conv().run, RunState::Stopping { .. }));
    loop {
        let event = h.apply_next("cancel confirmation");
        if event.kind == EventKind::Cancelled {
            break;
        }
        assert!(
            matches!(h.conv().run, RunState::Stopping { .. }),
            "left Stopping early on {event:?}"
        );
    }
    assert!(matches!(h.conv().run, RunState::Idle));
    assert_eq!(
        h.conv().queue.len(),
        1,
        "queued follow-ups stay queued after a user cancel"
    );
    assert!(
        h.conv()
            .items
            .iter()
            .any(|i| matches!(&i.kind, ItemKind::Notice { text, .. } if text == "Run stopped."))
    );
    assert_eq!(h.backend.submit_count(), 1, "nothing was resubmitted");
}

#[test]
fn cancel_settles_after_a_delay_not_instantly() {
    let (tx, rx) = async_channel::unbounded();
    let backend = DemoBackend::new(
        tx,
        DemoOptions {
            speed: 1.0,
            ..DemoOptions::default()
        },
    );
    let events = raw_events(&backend, &rx, vec![submit_request(8, 1, 5, "go")], |e| {
        matches!(e.kind, EventKind::Token(_))
    });
    assert!(matches!(events.last().unwrap().kind, EventKind::Token(_)));
    let started = Instant::now();
    let after = raw_events(
        &backend,
        &rx,
        vec![BackendRequest::Cancel {
            conversation: ConversationId(8),
            generation: 1,
            op: OperationId(5),
        }],
        |e| e.kind == EventKind::Cancelled,
    );
    assert!(
        started.elapsed() >= Duration::from_millis(SETTLE_MS - 50),
        "{:?}",
        started.elapsed()
    );
    let last = after.last().unwrap();
    assert_eq!(
        (last.conversation, last.generation, last.op),
        (ConversationId(8), 1, Some(OperationId(5)))
    );
}

#[test]
fn failure_rejects_then_retry_succeeds_then_tool_failure() {
    let mut h = Harness::new("failure", false, EMPTY_CONVERSATION);
    h.submit("this prompt must not be lost");
    h.pump_until("rejection", |s| {
        matches!(s.current().unwrap().run, RunState::Failed { .. })
    });
    assert!(matches!(&h.conv().run, RunState::Failed { message } if message.contains("529")));
    assert_eq!(
        h.conv().draft.text,
        "this prompt must not be lost",
        "draft retained"
    );
    assert!(h.state.availability().retry);
    assert!(h.conv().items.iter().any(|i| matches!(
        &i.kind,
        ItemKind::User {
            delivery: Delivery::Rejected,
            ..
        }
    )));

    h.dispatch(Command::Retry);
    h.run_to_idle();
    assert_eq!(h.conv().changes.len(), 3, "retry ran the full story");
    assert_eq!(h.backend.submit_count(), 2);
    let texts: Vec<_> = h
        .backend
        .submissions()
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert_eq!(texts[0], texts[1]);

    // Next submission exercises the tool-failure variant.
    h.dispatch(Command::EditDraft("now break a tool".into()));
    h.dispatch(Command::Submit);
    h.pump_until("tool failure", |s| {
        matches!(s.current().unwrap().run, RunState::Failed { .. })
    });
    let failed = h
        .tools()
        .into_iter()
        .rev()
        .find(|t| t.status == ToolStatus::Failed)
        .unwrap();
    assert!(failed.output.contains("linking with `cc` failed"));
    assert!(
        failed.output.lines().count() > 3,
        "expandable multi-line output"
    );
    assert!(h.conv().items.iter().any(|i| matches!(
        &i.kind,
        ItemKind::Notice {
            level: NoticeLevel::Error,
            ..
        }
    )));
    h.dispatch(Command::DismissFailure);
    assert!(matches!(h.conv().run, RunState::Idle));
}

#[test]
fn unknown_outcome_resolves_via_check_status_without_resending() {
    let mut h = Harness::new("unknown", false, EMPTY_CONVERSATION);
    h.submit("did you get this?");
    h.pump_until("ack lost", |s| {
        matches!(s.current().unwrap().run, RunState::OutcomeUnknown { .. })
    });
    assert!(h.state.availability().check_status);
    assert!(!h.state.availability().submit);
    assert!(matches!(
        &h.conv().items[0].kind,
        ItemKind::User {
            delivery: Delivery::Unknown,
            ..
        }
    ));
    // Nothing else happens on its own: the backend waits for the status check.
    std::thread::sleep(Duration::from_millis(50));
    assert!(h.rx.try_recv().is_err());
    assert_eq!(h.backend.submit_count(), 1);

    h.dispatch(Command::CheckStatus);
    h.run_to_idle();
    assert!(
        h.log
            .iter()
            .any(|e| e.kind == EventKind::StatusResolved { accepted: true })
    );
    assert_eq!(
        h.backend.submit_count(),
        1,
        "exactly one Submit reached the backend"
    );
    assert_eq!(h.conv().changes.len(), 3, "the run continued as normal");
    assert!(matches!(
        &h.conv().items[0].kind,
        ItemKind::User {
            delivery: Delivery::Sent,
            ..
        }
    ));
}

#[test]
fn a_repeated_submit_is_counted_and_not_treated_as_the_original() {
    let (tx, rx) = async_channel::unbounded();
    let backend = DemoBackend::new(tx, options("unknown", false));
    let events = raw_events(
        &backend,
        &rx,
        vec![
            submit_request(8, 1, 1, "same text"),
            submit_request(8, 1, 2, "same text"),
        ],
        |e| e.kind == EventKind::AckLost && e.op == Some(OperationId(2)),
    );
    assert_eq!(backend.submit_count(), 2);
    assert!(
        events
            .iter()
            .any(|e| e.kind == EventKind::AckLost && e.op == Some(OperationId(1)))
    );
    // Resolving the original does not resolve (or resume) the repeat.
    let resolved = raw_events(
        &backend,
        &rx,
        vec![BackendRequest::CheckStatus {
            conversation: ConversationId(8),
            generation: 1,
            op: OperationId(1),
            request: None,
        }],
        |e| matches!(e.kind, EventKind::StatusResolved { .. }),
    );
    assert_eq!(resolved.last().unwrap().op, Some(OperationId(1)));
    std::thread::sleep(Duration::from_millis(30));
    while let Ok(e) = rx.try_recv() {
        assert_eq!(e.op, Some(OperationId(1)), "op 2 stays blocked: {e:?}");
    }
    // An operation the backend never saw is not accepted.
    let unseen = raw_events(
        &backend,
        &rx,
        vec![BackendRequest::CheckStatus {
            conversation: ConversationId(8),
            generation: 1,
            op: OperationId(99),
            request: None,
        }],
        |e| matches!(e.kind, EventKind::StatusResolved { .. }),
    );
    assert_eq!(
        unseen.last().unwrap().kind,
        EventKind::StatusResolved { accepted: false }
    );
}

#[test]
fn recovered_unresolved_request_is_never_resent_until_the_user_retries() {
    let mut h = Harness::new("normal", false, EMPTY_CONVERSATION);
    // A previous run journaled this prompt but died before it reached a terminal state.
    let out = h.state.restore_unresolved(
        EMPTY_CONVERSATION,
        RequestId("previous-run-1".into()),
        "half-sent prompt".into(),
        vec![],
    );
    h.exec(out);
    assert!(matches!(h.conv().run, RunState::OutcomeUnknown { .. }));
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(h.backend.submit_count(), 0, "recovery never resends");
    assert!(h.rx.try_recv().is_err());

    // The backend never saw it, so checking status proves non-acceptance...
    h.dispatch(Command::CheckStatus);
    h.pump_until("status resolved", |s| {
        matches!(s.current().unwrap().run, RunState::Failed { .. })
    });
    assert_eq!(h.backend.submit_count(), 0);

    // ...and only an explicit retry submits, exactly once, with the saved text.
    assert!(h.state.availability().retry);
    h.dispatch(Command::Retry);
    h.run_to_idle();
    assert_eq!(h.backend.submit_count(), 1);
    assert_eq!(h.backend.submissions()[0].text, "half-sent prompt");
}

#[test]
fn stressed_scenario_streams_awkward_content() {
    let mut h = Harness::new("stressed", false, EMPTY_CONVERSATION);
    h.submit("stress me");
    h.run_to_idle();
    let text = h.assistant_text();
    assert!(text.contains('\u{200d}') && text.contains("\u{645}\u{631}\u{62d}\u{628}\u{627}"));
    assert!(
        text.contains("```rust\nfn main()"),
        "unterminated fence is delivered intact"
    );
    assert!(text.contains(&"supercalifragilisticexpialidocious".repeat(12)));
    assert!(text.contains("`$(rm -rf ~)`"));
}

#[test]
fn large_scenario_emits_huge_diff_and_bounded_megabyte_output() {
    let mut h = Harness::new("large", false, EMPTY_CONVERSATION);
    h.submit("make it big");
    h.run_to_idle();
    let changed: u32 = h.conv().changes.iter().map(|c| c.added + c.removed).sum();
    assert!(changed >= 2000, "{changed}");
    let tool = &h.tools()[0].clone();
    assert!(tool.full_len > 1_000_000, "{}", tool.full_len);
    assert!(tool.truncated && tool.output.len() <= TOOL_OUTPUT_PREVIEW_BYTES);
    assert_eq!(tool.status, ToolStatus::Ok);
}

// -------------------------------------------------------------------------- invariants

#[test]
fn every_event_carries_the_conversation_generation_and_op_of_its_request() {
    let (tx, rx) = async_channel::unbounded();
    let backend = DemoBackend::new(tx, options("normal", false));
    let events = raw_events(
        &backend,
        &rx,
        vec![
            BackendRequest::Open {
                conversation: ConversationId(9),
                generation: 3,
            },
            submit_request(8, 7, 42, "hello"),
        ],
        is_terminal,
    );
    let (opened, run): (Vec<_>, Vec<_>) = events
        .iter()
        .partition(|e| matches!(e.kind, EventKind::Opened { .. }));
    assert_eq!(opened.len(), 1);
    assert_eq!(
        (opened[0].conversation, opened[0].generation, opened[0].op),
        (ConversationId(9), 3, None)
    );
    assert!(run.len() > 20);
    assert!(run.iter().all(|e| e.conversation == ConversationId(8)
        && e.generation == 7
        && e.op == Some(OperationId(42))));
}

#[test]
fn stale_generation_events_never_reach_another_attachment() {
    // A late event from a previous attachment is dropped by the core's guard.
    let mut h = Harness::new("normal", false, EMPTY_CONVERSATION);
    h.submit("go");
    let stale_generation = h.conv().generation + 1;
    let op = h.conv().run.op();
    h.state.apply_event(BackendEvent {
        conversation: EMPTY_CONVERSATION,
        generation: stale_generation,
        op,
        kind: EventKind::Token("stale".into()),
    });
    h.run_to_idle();
    assert!(!h.assistant_text().contains("stale"));
}

#[test]
fn same_seed_produces_identical_event_sequences() {
    fn transcript(seed: u64, scenario: &str) -> String {
        let (tx, rx) = async_channel::unbounded();
        let backend = DemoBackend::new(
            tx,
            DemoOptions {
                seed,
                ..options(scenario, false)
            },
        );
        let mut all = format!("{:?}", backend.bootstrap().conversations);
        let requests = vec![
            BackendRequest::Open {
                conversation: ConversationId(1),
                generation: 1,
            },
            BackendRequest::Open {
                conversation: ConversationId(3),
                generation: 1,
            },
            BackendRequest::Open {
                conversation: ConversationId(4),
                generation: 1,
            },
            BackendRequest::LoadOlder {
                conversation: ConversationId(4),
                generation: 1,
                before: Some(item_id(ConversationId(4), 9_800)),
            },
            submit_request(8, 1, 1, "determinism"),
        ];
        let events = raw_events(&backend, &rx, requests, is_terminal);
        for e in events {
            all.push_str(&format!("{e:?}\n"));
        }
        all
    }
    for scenario in ["normal", "stressed", "failure", "large"] {
        let a = transcript(11, scenario);
        let b = transcript(11, scenario);
        assert_eq!(a, b, "scenario {scenario} is not deterministic");
        assert!(a.len() > 1000);
    }
    assert_ne!(transcript(11, "normal"), transcript(12, "normal"));
}
