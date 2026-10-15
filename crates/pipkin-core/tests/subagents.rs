use pipkin_core::*;
mod support;

fn state() -> AppState {
    state_in_mode(Mode::Real)
}

fn state_in_mode(mode: Mode) -> AppState {
    let mut state = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "P".into(),
                path: "/tmp/p".into(),
            }],
            conversations: vec![
                (ConversationId(1), ProjectId(1), "one".into(), 1),
                (ConversationId(2), ProjectId(1), "two".into(), 2),
            ],
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

fn child() -> SubagentInfo {
    SubagentInfo {
        id: 10,
        task_id: 7,
        call_id: "call-7".into(),
        task: "Investigate".into(),
        status: "running".into(),
        profile: None,
        ..SubagentInfo::default()
    }
}

fn ready(s: &mut AppState) {
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
        EventKind::SubagentsSynced {
            enabled: true,
            children: vec![child()],
        },
    ));
}

#[test]
fn advertised_demo_directory_opens_and_accepts_only_matching_synthetic_view() {
    for enabled in [true, false] {
        let mut s = state_in_mode(Mode::Demo);
        ready(&mut s);
        s.apply_event(event(
            &s,
            EventKind::SubagentsSynced {
                enabled,
                children: vec![child()],
            },
        ));
        s.dispatch(Command::EditDraft("parent draft".into()));
        s.read_only = Some("simulated read-only session".into());
        let parent = s.current().unwrap().clone();
        let availability = s.availability();
        let out = s.dispatch(Command::SelectSubagent(Some(10)));
        assert_eq!(
            out.effects,
            vec![Effect::Backend(BackendRequest::OpenSubagent {
                conversation: parent.id,
                generation: parent.generation,
                child: 10,
            })]
        );
        assert!(s.current().unwrap().subagents.loading);
        assert!(
            s.dispatch(Command::SelectSubagent(Some(999)))
                .effects
                .is_empty()
        );
        assert_eq!(s.current().unwrap().subagents.selected, Some(10));
        let items = vec![TranscriptItem {
            native_only: false,
            id: ItemId(101),
            at: 1,
            kind: ItemKind::Assistant {
                text: "Demo · simulated agent: synthetic child result".into(),
                streaming: false,
            },
        }];
        let matching = event(
            &s,
            EventKind::SubagentView {
                child: 10,
                items: items.clone(),
                live: false,
            },
        );
        let mut stale = matching.clone();
        stale.generation += 1;
        let mut wrong_parent = matching.clone();
        wrong_parent.conversation = ConversationId(2);
        let mut wrong_child = matching.clone();
        wrong_child.kind = EventKind::SubagentView {
            child: 11,
            items: items.clone(),
            live: false,
        };
        for rejected in [stale, wrong_parent, wrong_child] {
            assert!(s.apply_event(rejected).notes.is_empty());
            assert!(s.current().unwrap().subagents.loading);
            assert!(s.current().unwrap().subagents.items.is_empty());
        }
        s.apply_event(matching);
        let c = s.current().unwrap();
        assert_eq!(c.subagents.items, items);
        assert_eq!(c.subagents.viewed, Some(10));
        assert!(!c.subagents.loading);
        assert_eq!(c.subagents.previews.len(), 1);
        assert_eq!(c.draft, parent.draft);
        assert_eq!(c.run, parent.run);
        assert_eq!(c.items, parent.items);
        assert_eq!(s.mode, Mode::Demo);
        assert_eq!(s.availability(), availability);
        assert!(
            s.dispatch(Command::SetSubagentsEnabled(!enabled))
                .effects
                .is_empty()
        );
        assert_eq!(s.current().unwrap().subagents.enabled, Some(enabled));
        s.dispatch(Command::SelectSubagent(None));
        assert_eq!(s.current().unwrap().subagents.selected, None);
        assert!(s.current().unwrap().subagents.items.is_empty());
    }
}

#[test]
fn demo_sessions_without_an_open_advertised_service_reject_selection() {
    let mut s = state_in_mode(Mode::Demo);
    // An early directory event cannot advertise a service before the session opens.
    s.apply_event(event(
        &s,
        EventKind::SubagentsSynced {
            enabled: true,
            children: vec![child()],
        },
    ));
    assert_eq!(s.current().unwrap().subagents.enabled, None);
    for opened in [false, true] {
        s.conversations[0].opened = opened;
        // Even a directory row alone is not a capability advertisement.
        s.conversations[0].subagents.children = vec![child()];
        let before = s.current().unwrap().subagents.clone();
        for command in [
            Command::SelectSubagent(Some(10)),
            Command::SelectSubagent(None),
            Command::SetSubagentsEnabled(true),
        ] {
            let out = s.dispatch(command);
            assert!(out.effects.is_empty() && out.notes.is_empty());
            assert_eq!(s.current().unwrap().subagents, before);
        }
    }
    ready(&mut s);
    s.apply_event(event(
        &s,
        EventKind::SubagentsUnavailable("No simulated service".into()),
    ));
    let before = s.current().unwrap().subagents.clone();
    assert!(
        s.dispatch(Command::SelectSubagent(Some(10)))
            .effects
            .is_empty()
    );
    assert_eq!(s.current().unwrap().subagents, before);
    // Capability belongs to this conversation, not every demo session.
    ready(&mut s);
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    assert!(
        s.dispatch(Command::SelectSubagent(Some(10)))
            .effects
            .is_empty()
    );
    assert_eq!(s.current().unwrap().subagents.selected, None);
}

#[test]
fn unsupported_session_cannot_toggle_or_open_and_disable_is_engine_authoritative() {
    let mut s = state();
    assert!(
        s.dispatch(Command::SetSubagentsEnabled(false))
            .effects
            .is_empty()
    );
    assert!(
        s.dispatch(Command::SelectSubagent(Some(10)))
            .effects
            .is_empty()
    );
    ready(&mut s);
    let effects = s.dispatch(Command::SetSubagentsEnabled(false)).effects;
    assert!(matches!(
        effects.as_slice(),
        [Effect::Backend(BackendRequest::SetSubagentsEnabled {
            conversation: ConversationId(1),
            enabled: false,
            ..
        })]
    ));
    assert_eq!(s.current().unwrap().subagents.enabled, Some(true));
    s.apply_event(event(
        &s,
        EventKind::SubagentsSynced {
            enabled: false,
            children: vec![child()],
        },
    ));
    assert_eq!(s.current().unwrap().subagents.enabled, Some(false));
    // Existing child remains visible even though new engine calls have been disabled.
    assert!(matches!(
        s.dispatch(Command::SelectSubagent(Some(10)))
            .effects
            .as_slice(),
        [Effect::Backend(BackendRequest::OpenSubagent {
            child: 10,
            ..
        })]
    ));
    s.apply_event(event(
        &s,
        EventKind::SubagentView {
            child: 10,
            items: vec![TranscriptItem {
                native_only: false,
                id: ItemId(1),
                at: 0,
                kind: ItemKind::Assistant {
                    text: "x".repeat(5000),
                    streaming: true,
                },
            }],
            live: true,
        },
    ));
    assert_eq!(s.current().unwrap().subagents.previews[0].text.len(), 4096);
    assert!(s.current().unwrap().subagents.previews[0].clipped);
    assert_eq!(s.current().unwrap().subagents.enabled, Some(false));
    assert!(s.current().unwrap().subagents.live);
}

#[test]
fn child_identity_generation_and_parent_are_guarded_and_draft_is_unchanged() {
    let mut s = state();
    ready(&mut s);
    s.dispatch(Command::EditDraft("parent draft".into()));
    let old = event(
        &s,
        EventKind::SubagentView {
            child: 10,
            items: vec![],
            live: true,
        },
    );
    s.dispatch(Command::SelectSubagent(Some(10)));
    s.apply_event(event(
        &s,
        EventKind::SubagentView {
            child: 11,
            items: vec![],
            live: true,
        },
    ));
    assert!(s.current().unwrap().subagents.loading);
    s.apply_event(event(
        &s,
        EventKind::SubagentView {
            child: 10,
            items: vec![],
            live: true,
        },
    ));
    assert!(s.current().unwrap().subagents.live);
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    s.apply_event(old);
    assert!(
        s.conversation(ConversationId(2))
            .unwrap()
            .subagents
            .items
            .is_empty()
    );
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    assert_eq!(s.current().unwrap().draft.text, "parent draft");
    // The previous attachment may not overwrite the newly attached child.
    let mut stale = event(
        &s,
        EventKind::SubagentViewFailed {
            child: 10,
            reason: "old".into(),
        },
    );
    stale.generation -= 1;
    s.apply_event(stale);
    assert!(s.current().unwrap().subagents.error.is_none());
    assert!(
        s.dispatch(Command::SelectSubagent(Some(999)))
            .effects
            .is_empty()
    );
    assert_eq!(s.current().unwrap().subagents.selected, Some(10));
}

// ----------------------------------------------------------------- native paging

fn entry(id: u64) -> TranscriptItem {
    TranscriptItem {
        native_only: false,
        id: ItemId(id),
        at: 0,
        kind: ItemKind::Assistant {
            text: format!("child entry {id}"),
            streaming: false,
        },
    }
}

fn rows(n: u64, status: &str) -> Vec<SubagentInfo> {
    (1..=n)
        .map(|id| SubagentInfo {
            id,
            task_id: id,
            call_id: format!("call-{id}"),
            task: format!("Task {id}"),
            status: status.into(),
            profile: None,
            ..SubagentInfo::default()
        })
        .collect()
}

/// An opened real session whose engine pages (native), with child 10 selected and its newest
/// page (entries `range`) shown.
fn paged_child(range: std::ops::Range<u64>, has_older: bool) -> AppState {
    let mut s = state();
    s.apply_event(event(
        &s,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    ));
    s.apply_event(event(
        &s,
        EventKind::SubagentDirectory {
            enabled: true,
            children: vec![child()],
            truncated: false,
        },
    ));
    s.dispatch(Command::SelectSubagent(Some(10)));
    s.apply_event(event(
        &s,
        EventKind::SubagentPage {
            child: 10,
            items: range.map(entry).collect(),
            live: true,
            has_older,
        },
    ));
    s
}

fn shown(s: &AppState) -> Vec<u64> {
    s.current()
        .unwrap()
        .subagents
        .items
        .iter()
        .map(|i| i.id.0)
        .collect()
}

fn load_older(s: &mut AppState) -> ItemId {
    let out = s.dispatch(Command::LoadOlderSubagent);
    match out.effects.as_slice() {
        [
            Effect::Backend(BackendRequest::LoadOlderSubagent {
                child: 10, before, ..
            }),
        ] => *before,
        other => panic!("expected one older-page request, got {other:?}"),
    }
}

#[test]
fn older_child_pages_prepend_once_and_stale_or_foreign_pages_are_rejected() {
    let mut s = paged_child(4800..5000, true);
    assert_eq!(s.current().unwrap().subagents.previews.len(), 200);
    let before = load_older(&mut s);
    assert_eq!(before, ItemId(4800));
    // Single flight: a second request waits for the first answer.
    assert!(s.dispatch(Command::LoadOlderSubagent).effects.is_empty());
    let page = |child, before, range: std::ops::Range<u64>| EventKind::SubagentOlderPage {
        child,
        before,
        items: range.map(entry).collect(),
        has_older: true,
    };
    let mut stale = event(&s, page(10, before, 4600..4800));
    stale.generation += 1;
    let wrong_child = event(&s, page(11, before, 4600..4800));
    let wrong_cursor = event(&s, page(10, ItemId(4700), 4600..4700));
    for rejected in [stale, wrong_child, wrong_cursor] {
        assert!(s.apply_event(rejected).notes.is_empty());
        assert_eq!(shown(&s).len(), 200);
    }
    // Overlapping entries (already shown) are not duplicated.
    s.apply_event(event(&s, page(10, before, 4600..4805)));
    assert_eq!(shown(&s), (4600..5000).collect::<Vec<_>>());
    let sub = &s.current().unwrap().subagents;
    assert_eq!(sub.previews.len(), 400);
    assert!(sub.loading_older.is_none() && sub.has_older);
    // The same answer again is not in flight any more.
    assert!(
        s.apply_event(event(&s, page(10, before, 4400..4600)))
            .notes
            .is_empty()
    );
    // Every entry is reachable, page by page, until the engine says there is nothing older.
    while s.current().unwrap().subagents.has_older {
        let before = load_older(&mut s);
        let start = before.0.saturating_sub(200);
        s.apply_event(event(
            &s,
            EventKind::SubagentOlderPage {
                child: 10,
                before,
                items: (start..before.0).map(entry).collect(),
                has_older: start > 0,
            },
        ));
    }
    assert_eq!(shown(&s), (0..5000).collect::<Vec<_>>());
    assert!(s.dispatch(Command::LoadOlderSubagent).effects.is_empty());
}

#[test]
fn live_refreshes_replace_only_the_newest_page_and_a_gap_restarts_the_view() {
    let mut s = paged_child(300..500, true);
    let before = load_older(&mut s);
    s.apply_event(event(
        &s,
        EventKind::SubagentOlderPage {
            child: 10,
            before,
            items: (100..300).map(entry).collect(),
            has_older: true,
        },
    ));
    // A live refresh: the newest page moved on by 20 entries; older pages are kept.
    s.apply_event(event(
        &s,
        EventKind::SubagentPage {
            child: 10,
            items: (320..520).map(entry).collect(),
            live: true,
            has_older: true,
        },
    ));
    assert_eq!(shown(&s), (100..520).collect::<Vec<_>>());
    let sub = &s.current().unwrap().subagents;
    assert_eq!(
        sub.previews.iter().map(|p| p.id).collect::<Vec<_>>(),
        sub.items.iter().map(|i| i.id).collect::<Vec<_>>()
    );
    // More new entries than one page: no proof of continuity, so the view restarts there.
    s.apply_event(event(
        &s,
        EventKind::SubagentPage {
            child: 10,
            items: (900..1100).map(entry).collect(),
            live: true,
            has_older: true,
        },
    ));
    assert_eq!(shown(&s), (900..1100).collect::<Vec<_>>());
    assert!(s.current().unwrap().subagents.has_older);
}

#[test]
fn selection_change_resets_paging_and_late_answers_for_the_old_selection_are_dropped() {
    let mut s = paged_child(300..500, true);
    let children = vec![child(), SubagentInfo { id: 11, ..child() }];
    s.apply_event(event(
        &s,
        EventKind::SubagentDirectory {
            enabled: true,
            children,
            truncated: false,
        },
    ));
    let before = load_older(&mut s);
    s.dispatch(Command::SelectSubagent(Some(11)));
    let sub = &s.current().unwrap().subagents;
    assert!(sub.items.is_empty() && !sub.has_older && sub.loading_older.is_none());
    for late in [
        EventKind::SubagentOlderPage {
            child: 10,
            before,
            items: (100..300).map(entry).collect(),
            has_older: false,
        },
        EventKind::SubagentPage {
            child: 10,
            items: (0..5).map(entry).collect(),
            live: true,
            has_older: false,
        },
        EventKind::SubagentOlderFailed {
            child: 10,
            before,
            reason: "late".into(),
        },
    ] {
        assert!(s.apply_event(event(&s, late)).notes.is_empty());
    }
    assert!(shown(&s).is_empty());
    // Returning to child 10 starts from its newest page; the old cursor is not in flight.
    s.dispatch(Command::SelectSubagent(Some(10)));
    assert!(
        s.apply_event(event(
            &s,
            EventKind::SubagentOlderPage {
                child: 10,
                before,
                items: (100..300).map(entry).collect(),
                has_older: false,
            },
        ))
        .notes
        .is_empty()
    );
    assert!(shown(&s).is_empty());
}

#[test]
fn a_failed_older_page_is_reported_and_can_be_asked_for_again() {
    let mut s = paged_child(300..500, true);
    let before = load_older(&mut s);
    s.apply_event(event(
        &s,
        EventKind::SubagentOlderFailed {
            child: 10,
            before,
            reason: "Could not read".into(),
        },
    ));
    let sub = &s.current().unwrap().subagents;
    assert_eq!(sub.older_error.as_deref(), Some("Could not read"));
    assert_eq!(shown(&s), (300..500).collect::<Vec<_>>());
    assert_eq!(load_older(&mut s), before);
    assert!(s.current().unwrap().subagents.older_error.is_none());
}

#[test]
fn finished_history_pages_only_for_paged_engines_and_collapses_on_scope_change() {
    let mut s = state();
    ready(&mut s);
    // Pi (unpaged): one page of history at most; no "show more", no older activity.
    s.apply_event(event(
        &s,
        EventKind::SubagentsSynced {
            enabled: true,
            children: rows(250, "done"),
        },
    ));
    s.dispatch(Command::ToggleSubagentHistory);
    let sub = &s.current().unwrap().subagents;
    assert_eq!(
        (sub.history_shown(), sub.can_show_more_history()),
        (100, false)
    );
    assert!(
        s.dispatch(Command::ShowMoreSubagentHistory)
            .notes
            .is_empty()
    );
    s.dispatch(Command::SelectSubagent(Some(1)));
    s.apply_event(event(
        &s,
        EventKind::SubagentView {
            child: 1,
            items: (0..300).map(entry).collect(),
            live: true,
        },
    ));
    let sub = &s.current().unwrap().subagents;
    assert_eq!(
        sub.previews.len(),
        80,
        "The legacy engine keeps its bounded preview window"
    );
    assert_eq!(sub.previews[0].id, ItemId(220));
    assert!(!sub.can_load_older());
    assert!(s.dispatch(Command::LoadOlderSubagent).effects.is_empty());
    s.dispatch(Command::SelectSubagent(None));

    // Native (paged): pages of 100 until every finished task is revealed.
    s.apply_event(event(
        &s,
        EventKind::SubagentDirectory {
            enabled: true,
            children: rows(250, "done"),
            truncated: false,
        },
    ));
    assert_eq!(s.current().unwrap().subagents.history_shown(), 100);
    s.dispatch(Command::ShowMoreSubagentHistory);
    assert_eq!(s.current().unwrap().subagents.history_shown(), 200);
    s.dispatch(Command::ShowMoreSubagentHistory);
    assert_eq!(s.current().unwrap().subagents.history_shown(), 250);
    assert!(
        s.dispatch(Command::ShowMoreSubagentHistory)
            .notes
            .is_empty()
    );
    // Routine directory updates and selection keep the revealed pages.
    s.dispatch(Command::SelectSubagent(Some(7)));
    s.apply_event(event(
        &s,
        EventKind::SubagentDirectory {
            enabled: true,
            children: rows(251, "done"),
            truncated: true,
        },
    ));
    let sub = &s.current().unwrap().subagents;
    assert_eq!((sub.history_shown(), sub.truncated), (251, true));
    // Entering another conversation and coming back collapses history again.
    s.dispatch(Command::SelectConversation(ConversationId(2)));
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    assert_eq!(s.current().unwrap().subagents.history, 0);
}

#[test]
fn an_identical_live_page_does_not_cancel_an_older_page_in_flight() {
    let mut s = paged_child(100..200, true);
    let before = load_older(&mut s);
    s.apply_event(event(
        &s,
        EventKind::SubagentPage {
            child: 10,
            items: (100..200).map(entry).collect(),
            live: true,
            has_older: true,
        },
    ));
    assert_eq!(s.current().unwrap().subagents.loading_older, Some(before));
    s.apply_event(event(
        &s,
        EventKind::SubagentOlderPage {
            child: 10,
            before,
            items: (0..100).map(entry).collect(),
            has_older: false,
        },
    ));
    let sub = &s.current().unwrap().subagents;
    assert_eq!(sub.items.len(), 200);
    assert_eq!(sub.items[0].id, ItemId(0));
    assert!(sub.loading_older.is_none());
    assert!(!sub.has_older);
}
