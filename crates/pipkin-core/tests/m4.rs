//! M4: saved copies, history paging, complete tool output, extension questions, search hits and
//! launching the editor or a terminal.

use pipkin_core::*;

fn boot(conversations: usize) -> Bootstrap {
    Bootstrap {
        projects: vec![Project {
            id: ProjectId(1),
            name: "p".into(),
            path: "/work/p".into(),
        }],
        models: vec![ModelInfo {
            id: "a/m".into(),
            name: "M".into(),
            note: String::new(),
        }],
        conversations: (1..=conversations as u64)
            .map(|n| {
                (
                    ConversationId(n),
                    ProjectId(1),
                    format!("conversation {n}"),
                    100 - n as i64,
                )
            })
            .collect(),
        now: 1000,
    }
}

fn real(conversations: usize) -> AppState {
    let mut s = AppState::new(boot(conversations), Prefs::default());
    s.mode = Mode::Real;
    s.prefs.selected_project = Some(ProjectId(1));
    s
}

fn open(s: &mut AppState, id: u64, items: Vec<TranscriptItem>, has_older: bool) {
    s.dispatch(Command::SelectConversation(ConversationId(id)));
    let generation = s.current().unwrap().generation;
    s.apply_event(BackendEvent {
        conversation: ConversationId(id),
        generation,
        op: None,
        kind: EventKind::Opened {
            items,
            has_older,
            changes: vec![],
        },
    });
}

fn event(s: &AppState, kind: EventKind) -> BackendEvent {
    let c = s.current().unwrap();
    BackendEvent {
        conversation: c.id,
        generation: c.generation,
        op: c.run.op(),
        kind,
    }
}

fn apply(s: &mut AppState, kind: EventKind) -> Outcome {
    let e = event(s, kind);
    s.apply_event(e)
}

fn user(id: u64, text: &str) -> TranscriptItem {
    TranscriptItem {
        native_only: false,
        id: ItemId(id),
        at: 1,
        kind: ItemKind::User {
            text: text.into(),
            attachments: vec![],
            delivery: Delivery::Sent,
            steer: false,
        },
    }
}

fn tool(id: u64, call: &str, status: ToolStatus, output: &str, truncated: bool) -> TranscriptItem {
    TranscriptItem {
        native_only: false,
        id: ItemId(id),
        at: 1,
        kind: ItemKind::Tool(ToolCall {
            call_ref: None,
            call_id: Some(call.into()),
            name: "bash".into(),
            input: "{}".into(),
            output: output.into(),
            truncated,
            full_len: output.len() * if truncated { 10 } else { 1 },
            status,
        }),
    }
}

fn effects_of<'a>(out: &'a Outcome, f: impl Fn(&Effect) -> bool + 'a) -> usize {
    out.effects.iter().filter(|e| f(e)).count()
}

// ------------------------------------------------------------------------- saved copies

#[test]
fn selecting_a_conversation_shows_its_saved_copy_until_the_engine_answers() {
    let mut s = real(2);
    let out = s.dispatch(Command::SelectConversation(ConversationId(1)));
    assert_eq!(
        effects_of(
            &out,
            |e| matches!(e, Effect::LoadCache { conversation } if *conversation == ConversationId(1))
        ),
        1
    );
    assert_eq!(
        effects_of(&out, |e| matches!(
            e,
            Effect::Backend(BackendRequest::Open { .. })
        )),
        1
    );

    s.apply_cache(
        ConversationId(1),
        vec![user(1024, "from before")],
        true,
        555,
    );
    let c = s.current().unwrap();
    assert_eq!(
        (c.items.len(), c.cached_at, c.has_older, c.opened),
        (1, Some(555), true, false)
    );

    // The engine's own state replaces it and clears the label.
    let generation = c.generation;
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation,
        op: None,
        kind: EventKind::Opened {
            items: vec![user(1024, "live"), user(2048, "newer")],
            has_older: false,
            changes: vec![],
        },
    });
    let c = s.current().unwrap();
    assert_eq!((c.items.len(), c.cached_at, c.opened), (2, None, true));
}

#[test]
fn a_saved_copy_never_replaces_what_is_already_shown_and_never_takes_input() {
    let mut s = real(1);
    s.set_connection(Connection::Ready);
    open(&mut s, 1, vec![user(1024, "live")], false);
    let out = s.apply_cache(ConversationId(1), vec![user(1, "stale")], false, 5);
    assert!(out.notes.is_empty());
    assert_eq!(s.current().unwrap().cached_at, None);
    assert!(
        effects_of(
            &s.dispatch(Command::SelectConversation(ConversationId(1))),
            |e| matches!(e, Effect::LoadCache { .. })
        ) == 0
    );

    // Showing only a saved copy while connected (the open is still in flight): no sending.
    let mut s = real(1);
    s.set_connection(Connection::Ready);
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    s.apply_cache(ConversationId(1), vec![user(1024, "saved")], false, 5);
    s.dispatch(Command::EditDraft("hello".into()));
    assert!(!s.availability().submit);
}

#[test]
fn when_the_engine_cannot_be_reached_the_saved_copy_stays_with_the_reason() {
    let mut s = real(1);
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    s.apply_cache(ConversationId(1), vec![user(1024, "saved")], false, 5);
    apply(
        &mut s,
        EventKind::OpenFailed {
            message: "Not connected to the engine.".into(),
        },
    );
    let c = s.current().unwrap();
    assert_eq!(
        c.items.len(),
        1,
        "the saved messages are not replaced by an error"
    );
    assert_eq!(
        c.stale_reason.as_deref(),
        Some("Not connected to the engine.")
    );
    assert_eq!(c.cached_at, Some(5));

    // With nothing saved, the failure is shown as before.
    let mut s = real(1);
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    apply(
        &mut s,
        EventKind::OpenFailed {
            message: "gone".into(),
        },
    );
    assert!(matches!(
        &s.current().unwrap().items[0].kind,
        ItemKind::Notice {
            level: NoticeLevel::Error,
            ..
        }
    ));
}

#[test]
fn a_settled_view_asks_for_a_saved_copy_in_real_mode_only() {
    let mut s = real(1);
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    let generation = s.current().unwrap().generation;
    let opened = s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation,
        op: None,
        kind: EventKind::Opened {
            items: vec![user(1024, "hi")],
            has_older: false,
            changes: vec![],
        },
    });
    assert_eq!(
        effects_of(&opened, |e| matches!(e, Effect::SaveCache { .. })),
        1
    );
    for kind in [
        EventKind::Synced {
            items: vec![user(1024, "hi")],
        },
        EventKind::OlderPage {
            items: vec![],
            has_older: false,
        },
    ] {
        let out = apply(&mut s, kind);
        assert_eq!(
            effects_of(&out, |e| matches!(e, Effect::SaveCache { .. })),
            1
        );
    }
    // Nothing is saved for a conversation that was never opened.
    let mut fresh = real(1);
    fresh.dispatch(Command::SelectConversation(ConversationId(1)));
    let g = fresh.current().unwrap().generation;
    let out = fresh.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: g,
        op: None,
        kind: EventKind::Synced { items: vec![] },
    });
    assert_eq!(
        effects_of(&out, |e| matches!(e, Effect::SaveCache { .. })),
        0
    );

    // The demo has no engine copy to keep.
    let mut demo = real(1);
    demo.mode = Mode::Demo;
    demo.dispatch(Command::SelectConversation(ConversationId(1)));
    let g = demo.current().unwrap().generation;
    let out = demo.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: g,
        op: None,
        kind: EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    });
    assert_eq!(
        effects_of(&out, |e| matches!(e, Effect::SaveCache { .. })),
        0
    );
}

// ----------------------------------------------------------------------------- history

#[test]
fn a_failed_page_of_older_history_can_be_asked_for_again() {
    let mut s = real(1);
    s.set_connection(Connection::Ready);
    open(&mut s, 1, vec![user(5 * 1024, "recent")], true);
    let out = s.dispatch(Command::LoadOlder);
    assert_eq!(
        effects_of(&out, |e| matches!(
            e,
            Effect::Backend(BackendRequest::LoadOlder { .. })
        )),
        1
    );
    assert!(s.current().unwrap().loading_older);
    apply(
        &mut s,
        EventKind::OlderFailed {
            message: "timed out".into(),
        },
    );
    let c = s.current().unwrap();
    assert!(!c.loading_older && c.has_older);
    assert_eq!(c.older_error.as_deref(), Some("timed out"));
    assert!(s.availability().load_older);
    s.dispatch(Command::LoadOlder);
    apply(
        &mut s,
        EventKind::OlderPage {
            items: vec![user(4 * 1024, "older")],
            has_older: false,
        },
    );
    let c = s.current().unwrap();
    assert_eq!(
        (c.items.len(), c.older_error.clone(), c.has_older),
        (2, None, false)
    );
}

#[test]
fn a_tool_call_and_its_result_split_by_a_page_boundary_become_one_item() {
    let mut s = real(1);
    s.set_connection(Connection::Ready);
    // What is shown starts with the orphaned result of a call that is on the previous page.
    open(
        &mut s,
        1,
        vec![
            tool(7 * 1024, "c1", ToolStatus::Ok, "result text", false),
            user(8 * 1024, "after"),
        ],
        true,
    );
    s.dispatch(Command::LoadOlder);
    apply(
        &mut s,
        EventKind::OlderPage {
            items: vec![
                user(5 * 1024, "before"),
                tool(6 * 1024, "c1", ToolStatus::Running, "", false),
            ],
            has_older: false,
        },
    );
    let items = &s.current().unwrap().items;
    assert_eq!(items.len(), 3, "{items:?}");
    let ItemKind::Tool(t) = &items[1].kind else {
        panic!("{:?}", items[1])
    };
    assert_eq!(
        (t.status, t.output.as_str()),
        (ToolStatus::Ok, "result text")
    );
    assert!(matches!(&items[2].kind, ItemKind::User { text, .. } if text == "after"));
}

// ------------------------------------------------------------------------- tool output

#[test]
fn a_complete_preview_is_copied_at_once_and_a_cut_one_is_fetched() {
    let mut s = real(1);
    s.set_connection(Connection::Ready);
    open(
        &mut s,
        1,
        vec![
            tool(1024, "whole", ToolStatus::Ok, "all of it", false),
            tool(2048, "cut", ToolStatus::Ok, "the first part", true),
        ],
        false,
    );
    let out = s.dispatch(Command::CopyToolOutput(ItemId(1024)));
    assert_eq!(out.effects, [Effect::CopyText("all of it".into())]);

    let out = s.dispatch(Command::CopyToolOutput(ItemId(2048)));
    assert!(matches!(
        &out.effects[..],
        [Effect::Backend(BackendRequest::FetchToolOutput { call_id, .. })] if call_id == "cut"
    ));
    assert_eq!(
        s.current().unwrap().pending_output,
        Some(("cut".into(), OutputUse::Copy))
    );
    let out = apply(
        &mut s,
        EventKind::ToolOutputFull {
            call_id: "cut".into(),
            text: "the first part and everything after it".into(),
        },
    );
    assert_eq!(
        out.effects,
        [Effect::CopyText(
            "the first part and everything after it".into()
        )]
    );
    assert_eq!(s.current().unwrap().pending_output, None);
    // The same answer arriving again does nothing.
    let out = apply(
        &mut s,
        EventKind::ToolOutputFull {
            call_id: "cut".into(),
            text: "again".into(),
        },
    );
    assert!(out.effects.is_empty());
}

#[test]
fn saving_names_the_file_after_the_tool_and_a_failed_fetch_says_so() {
    let mut s = real(1);
    s.set_connection(Connection::Ready);
    open(
        &mut s,
        1,
        vec![tool(1024, "cut", ToolStatus::Ok, "part", true)],
        false,
    );
    s.dispatch(Command::SaveToolOutput(ItemId(1024)));
    let out = apply(
        &mut s,
        EventKind::ToolOutputFull {
            call_id: "cut".into(),
            text: "full".into(),
        },
    );
    assert_eq!(
        out.effects,
        [Effect::SaveText {
            suggested_name: "bash-output.txt".into(),
            text: "full".into()
        }]
    );

    s.dispatch(Command::CopyToolOutput(ItemId(1024)));
    // An answer about another call is not this one's.
    let out = apply(
        &mut s,
        EventKind::ToolOutputUnavailable {
            call_id: "other".into(),
            reason: "x".into(),
        },
    );
    assert!(out.effects.is_empty() && s.notice.is_none());
    apply(
        &mut s,
        EventKind::ToolOutputUnavailable {
            call_id: "cut".into(),
            reason: "the engine no longer has it".into(),
        },
    );
    assert!(
        s.notice
            .as_deref()
            .unwrap()
            .contains("the engine no longer has it")
    );
    assert_eq!(s.current().unwrap().pending_output, None);

    // A tool the engine never named has only its preview to copy.
    let mut item = tool(2048, "x", ToolStatus::Ok, "preview", true);
    if let ItemKind::Tool(t) = &mut item.kind {
        t.call_id = None;
    }
    let mut s = real(1);
    open(&mut s, 1, vec![item], false);
    let out = s.dispatch(Command::CopyToolOutput(ItemId(2048)));
    assert_eq!(out.effects, [Effect::CopyText("preview".into())]);
}

// --------------------------------------------------------------------- extension questions

fn question(id: &str, kind: UiRequestKind) -> UiRequest {
    UiRequest {
        id: id.into(),
        kind,
        title: format!("Question {id}"),
        message: None,
        items: vec![UiRequestItem {
            value: "a".into(),
            label: "A".into(),
            description: None,
        }],
        placeholder: None,
        default_value: None,
        deadline: None,
        form: None,
        source: None,
    }
}

#[test]
fn an_answer_goes_to_the_engine_once_and_the_engine_decides_when_the_question_is_over() {
    let mut s = real(1);
    s.set_connection(Connection::Ready);
    open(&mut s, 1, vec![], false);
    apply(
        &mut s,
        EventKind::UiState {
            requests: vec![
                question("q1", UiRequestKind::Select),
                question("q2", UiRequestKind::Confirm),
            ],
            status: vec![("build".into(), "compiling".into())],
            notices: vec![UiNotice {
                id: "n1".into(),
                level: UiNoticeLevel::Info,
                message: "hello".into(),
            }],
        },
    );
    let c = s.current().unwrap();
    assert_eq!(
        (c.ui_requests.len(), c.ui_status.len(), c.ui_notices.len()),
        (2, 1, 1)
    );

    let out = s.dispatch(Command::AnswerUiRequest {
        id: "q1".into(),
        answer: UiAnswer::Choice("a".into()),
    });
    assert!(matches!(
        &out.effects[..],
        [Effect::Backend(BackendRequest::UiRespond { id, answer: UiAnswer::Choice(v), .. })] if id == "q1" && v == "a"
    ));
    // Pressing the button again while the answer is on its way sends nothing more.
    let again = s.dispatch(Command::AnswerUiRequest {
        id: "q1".into(),
        answer: UiAnswer::Choice("a".into()),
    });
    assert!(again.effects.is_empty());
    // Nor does an answer to a question that is not open.
    let none = s.dispatch(Command::AnswerUiRequest {
        id: "zz".into(),
        answer: UiAnswer::Confirm(true),
    });
    assert!(none.effects.is_empty());
    assert_eq!(
        s.current().unwrap().ui_requests.len(),
        2,
        "still open until the engine says"
    );

    // The engine took it: the question leaves its list and nothing is left answering.
    apply(
        &mut s,
        EventKind::UiState {
            requests: vec![question("q2", UiRequestKind::Confirm)],
            status: vec![],
            notices: vec![],
        },
    );
    let c = s.current().unwrap();
    assert_eq!((c.ui_requests.len(), c.ui_answering.len()), (1, 0));
}

#[test]
fn a_refused_answer_leaves_the_question_open_with_the_reason() {
    let mut s = real(1);
    s.set_connection(Connection::Ready);
    open(&mut s, 1, vec![], false);
    apply(
        &mut s,
        EventKind::UiState {
            requests: vec![question("q1", UiRequestKind::Input)],
            status: vec![],
            notices: vec![],
        },
    );
    s.dispatch(Command::AnswerUiRequest {
        id: "q1".into(),
        answer: UiAnswer::Text("x".into()),
    });
    apply(
        &mut s,
        EventKind::UiRespondRefused {
            id: "q1".into(),
            reason: "That question is no longer open.".into(),
        },
    );
    let c = s.current().unwrap();
    assert!(c.ui_answering.is_empty());
    assert_eq!(
        c.ui_error.as_deref(),
        Some("That question is no longer open.")
    );
    assert_eq!(c.ui_requests.len(), 1);
    // It can be answered again, and the old error goes.
    let out = s.dispatch(Command::AnswerUiRequest {
        id: "q1".into(),
        answer: UiAnswer::Text("y".into()),
    });
    assert_eq!(out.effects.len(), 1);
    assert!(s.current().unwrap().ui_error.is_none());

    // Declining names the question; dismissing clears notices and the error.
    let out = s.dispatch(Command::CancelUiRequest("q1".into()));
    assert!(
        matches!(&out.effects[..], [Effect::Backend(BackendRequest::UiCancel { id, .. })] if id == "q1")
    );
    assert!(
        s.dispatch(Command::CancelUiRequest("nope".into()))
            .effects
            .is_empty()
    );
    apply(
        &mut s,
        EventKind::UiRespondRefused {
            id: "q1".into(),
            reason: "r".into(),
        },
    );
    s.dispatch(Command::DismissUiNotices);
    assert!(s.current().unwrap().ui_error.is_none());
}

// ----------------------------------------------------------------------------- search

fn hit(conversation: u64, item: u64) -> SearchHit {
    SearchHit {
        conversation: ConversationId(conversation),
        item: ItemId(item),
        snippet: "a \u{2}needle\u{3} here".into(),
        at: 1,
    }
}

#[test]
fn typing_in_the_search_box_searches_saved_history_and_stale_answers_are_dropped() {
    let mut s = real(2);
    let out = s.dispatch(Command::SetSearch("ne".into()));
    assert!(matches!(
        out.effects.iter().find(|e| matches!(e, Effect::SearchHistory { .. })),
        Some(Effect::SearchHistory { query }) if query == "ne"
    ));
    // One character is too little to search for, and clears what was found.
    s.apply_search_results(SearchResults {
        query: "ne".into(),
        hits: vec![hit(1, 1)],
        ..Default::default()
    });
    assert_eq!(s.history_search.hits.len(), 1);
    let out = s.dispatch(Command::SetSearch("n".into()));
    assert_eq!(
        effects_of(&out, |e| matches!(e, Effect::SearchHistory { .. })),
        0
    );
    assert!(s.history_search.hits.is_empty());

    s.dispatch(Command::SetSearch("needle".into()));
    // An answer for text that is no longer in the box is ignored.
    s.apply_search_results(SearchResults {
        query: "need".into(),
        hits: vec![hit(1, 1)],
        ..Default::default()
    });
    assert!(s.history_search.hits.is_empty());
    s.apply_search_results(SearchResults {
        query: "needle".into(),
        hits: vec![hit(2, 3)],
        conversations_searched: 4,
        messages_searched: 120,
        truncated: false,
    });
    assert_eq!(
        (
            s.history_search.hits.len(),
            s.history_search.conversations_searched
        ),
        (1, 4)
    );

    // The demo only filters titles.
    let mut demo = real(1);
    demo.mode = Mode::Demo;
    assert_eq!(
        effects_of(
            &demo.dispatch(Command::SetSearch("needle".into())),
            |e| matches!(e, Effect::SearchHistory { .. })
        ),
        0
    );
}

#[test]
fn opening_a_hit_shows_its_message_and_offers_the_way_back() {
    let mut s = real(2);
    s.set_connection(Connection::Ready);
    open(&mut s, 1, vec![user(1024, "where I was")], false);
    s.dispatch(Command::SetSearch("needle".into()));
    s.apply_search_results(SearchResults {
        query: "needle".into(),
        hits: vec![hit(2, 4096)],
        ..Default::default()
    });

    let out = s.dispatch(Command::OpenSearchHit(0));
    assert_eq!(s.selected, Some(ConversationId(2)));
    assert_eq!(s.search_return, Some(ConversationId(1)));
    assert_eq!(s.scroll_target, Some((ConversationId(2), ItemId(4096))));
    assert!(
        s.search.is_empty() && s.history_search.hits.is_empty(),
        "the search is done"
    );
    assert_eq!(
        effects_of(
            &out,
            |e| matches!(e, Effect::Backend(BackendRequest::Open { conversation, .. }) if *conversation == ConversationId(2))
        ),
        1
    );

    // The conversation opens with the message loaded: nothing more to fetch.
    let g = s.current().unwrap().generation;
    let out = s.apply_event(BackendEvent {
        conversation: ConversationId(2),
        generation: g,
        op: None,
        kind: EventKind::Opened {
            items: vec![user(4096, "the needle"), user(5120, "next")],
            has_older: true,
            changes: vec![],
        },
    });
    assert_eq!(
        effects_of(&out, |e| matches!(
            e,
            Effect::Backend(BackendRequest::LoadOlder { .. })
        )),
        0
    );
    assert_eq!(
        s.scroll_target,
        Some((ConversationId(2), ItemId(4096))),
        "kept until the view has scrolled"
    );
    s.dispatch(Command::ClearScrollTarget);
    assert_eq!(s.scroll_target, None);

    s.dispatch(Command::ReturnFromSearch);
    assert_eq!(s.selected, Some(ConversationId(1)));
    assert_eq!(s.search_return, None);
    assert!(s.dispatch(Command::ReturnFromSearch).effects.is_empty());
}

#[test]
fn a_hit_older_than_what_is_loaded_pages_back_until_it_appears_or_gives_up() {
    let mut s = real(2);
    s.set_connection(Connection::Ready);
    s.dispatch(Command::SetSearch("needle".into()));
    s.apply_search_results(SearchResults {
        query: "needle".into(),
        hits: vec![hit(1, 1024)],
        ..Default::default()
    });
    s.dispatch(Command::OpenSearchHit(0));
    let g = s.current().unwrap().generation;
    let out = s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: g,
        op: None,
        kind: EventKind::Opened {
            items: vec![user(9 * 1024, "recent")],
            has_older: true,
            changes: vec![],
        },
    });
    assert_eq!(
        effects_of(&out, |e| matches!(
            e,
            Effect::Backend(BackendRequest::LoadOlder { .. })
        )),
        1
    );
    // The first page does not have it either: ask again.
    let out = apply(
        &mut s,
        EventKind::OlderPage {
            items: vec![user(8 * 1024, "older")],
            has_older: true,
        },
    );
    assert_eq!(
        effects_of(&out, |e| matches!(
            e,
            Effect::Backend(BackendRequest::LoadOlder { .. })
        )),
        1
    );
    // The next page has it: stop asking.
    let out = apply(
        &mut s,
        EventKind::OlderPage {
            items: vec![user(1024, "the needle")],
            has_older: true,
        },
    );
    assert_eq!(
        effects_of(&out, |e| matches!(
            e,
            Effect::Backend(BackendRequest::LoadOlder { .. })
        )),
        0
    );
    assert!(s.scroll_target.is_some());

    // A message that is not in the history at all ends the search with a notice.
    let mut s = real(1);
    s.set_connection(Connection::Ready);
    s.dispatch(Command::SetSearch("needle".into()));
    s.apply_search_results(SearchResults {
        query: "needle".into(),
        hits: vec![hit(1, 777)],
        ..Default::default()
    });
    s.dispatch(Command::OpenSearchHit(0));
    let g = s.current().unwrap().generation;
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: g,
        op: None,
        kind: EventKind::Opened {
            items: vec![user(9 * 1024, "recent")],
            has_older: false,
            changes: vec![],
        },
    });
    assert_eq!(s.scroll_target, None);
    assert!(
        s.notice
            .as_deref()
            .unwrap()
            .contains("no longer in this conversation")
    );
}

#[test]
fn a_hit_in_a_conversation_that_is_gone_does_nothing() {
    let mut s = real(1);
    s.dispatch(Command::SetSearch("needle".into()));
    s.apply_search_results(SearchResults {
        query: "needle".into(),
        hits: vec![hit(99, 1)],
        ..Default::default()
    });
    assert!(s.dispatch(Command::OpenSearchHit(0)).effects.is_empty());
    assert!(s.dispatch(Command::OpenSearchHit(5)).effects.is_empty());
    assert_eq!(s.scroll_target, None);
}

// --------------------------------------------------------------------------- launching

fn change(path: &str) -> FileChange {
    FileChange {
        path: path.into(),
        added: 1,
        removed: 0,
        hunks: vec![],
    }
}

#[test]
fn a_changed_file_opens_in_the_editor_by_its_full_path_and_a_terminal_opens_in_the_project() {
    let mut s = real(1);
    s.set_connection(Connection::Ready);
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    let g = s.current().unwrap().generation;
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: g,
        op: None,
        kind: EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![change("src/main.rs"), change("docs/readme.md")],
        },
    });
    assert!(s.availability().open_in_editor && s.availability().open_terminal);
    let out = s.dispatch(Command::OpenInEditor(1));
    assert_eq!(
        out.effects,
        [Effect::Launch(Launch::Editor {
            path: "/work/p/docs/readme.md".into(),
            root: "/work/p".into()
        })]
    );
    assert!(s.dispatch(Command::OpenInEditor(7)).effects.is_empty());
    let out = s.dispatch(Command::OpenTerminal);
    assert_eq!(
        out.effects,
        [Effect::Launch(Launch::Terminal {
            cwd: "/work/p".into()
        })]
    );

    // Nothing launches from the simulated agent.
    s.mode = Mode::Demo;
    assert!(!s.availability().open_in_editor && !s.availability().open_terminal);
    assert!(s.dispatch(Command::OpenTerminal).effects.is_empty());
}

// ------------------------------------------------------------- conversations known offline

#[test]
fn a_conversation_known_only_from_its_saved_copy_is_readable_until_the_engine_lists_it() {
    let mut s = AppState::new(
        Bootstrap {
            projects: vec![],
            models: vec![],
            conversations: vec![],
            now: 1000,
        },
        Prefs::default(),
    );
    s.mode = Mode::Real;
    s.restore_cached_conversation(ConversationId(7), "/work/app", "saved one".into(), 50);
    s.restore_cached_conversation(ConversationId(8), "/work/app", "saved two".into(), 60);
    // A path that is not absolute is not a project.
    s.restore_cached_conversation(ConversationId(9), "relative", "x".into(), 1);
    assert_eq!(s.conversations.len(), 2);
    assert!(s.conversations.iter().all(|c| c.cached_only));
    assert_eq!(s.projects.len(), 1);
    assert_eq!(s.projects[0].path, "/work/app");

    // It can be selected and read offline.
    s.set_connection(Connection::Offline("no server".into()));
    s.prefs.selected_project = Some(s.projects[0].id);
    let out = s.dispatch(Command::SelectConversation(ConversationId(7)));
    assert_eq!(
        effects_of(&out, |e| matches!(e, Effect::LoadCache { .. })),
        1
    );
    s.apply_cache(ConversationId(7), vec![user(1024, "kept")], false, 40);
    assert_eq!(s.current().unwrap().items.len(), 1);
    assert!(!s.availability().submit && !s.availability().new_conversation);

    // The engine arrives and lists only one of them: that one is now the engine's; the other
    // is gone, and a selection of it is cleared.
    s.set_connection(Connection::Ready);
    s.apply_catalog(Bootstrap {
        projects: vec![Project {
            id: project_id_for_path("/work/app"),
            name: "app".into(),
            path: "/work/app".into(),
        }],
        models: vec![],
        conversations: vec![(
            ConversationId(8),
            project_id_for_path("/work/app"),
            "saved two (renamed)".into(),
            70,
        )],
        now: 2000,
    });
    assert_eq!(s.conversations.len(), 1);
    assert!(!s.conversations[0].cached_only);
    assert_eq!(
        s.selected, None,
        "the selected conversation no longer exists"
    );

    // One the engine lists keeps whatever it was showing.
    let mut s = real(1);
    s.restore_cached_conversation(ConversationId(1), "/work/p", "x".into(), 1);
    s.apply_catalog(boot(1));
    assert!(!s.conversation(ConversationId(1)).unwrap().cached_only);
}

#[test]
fn a_saved_copy_that_arrives_after_the_engines_refusal_replaces_the_notice_and_keeps_the_reason() {
    let mut s = real(1);
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    // The refusal is quicker than reading the saved copy.
    apply(
        &mut s,
        EventKind::OpenFailed {
            message: "Not connected to the engine.".into(),
        },
    );
    assert!(matches!(
        &s.current().unwrap().items[0].kind,
        ItemKind::Notice { .. }
    ));
    s.apply_cache(ConversationId(1), vec![user(1024, "saved words")], false, 9);
    let c = s.current().unwrap();
    assert_eq!(c.items.len(), 1);
    assert!(matches!(&c.items[0].kind, ItemKind::User { text, .. } if text == "saved words"));
    assert_eq!(c.cached_at, Some(9));
    assert_eq!(
        c.stale_reason.as_deref(),
        Some("Not connected to the engine.")
    );

    // Anything the conversation has of its own is never replaced.
    let mut s = real(1);
    s.dispatch(Command::SelectConversation(ConversationId(1)));
    let g = s.current().unwrap().generation;
    s.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: g,
        op: None,
        kind: EventKind::OpenFailed {
            message: "x".into(),
        },
    });
    s.conversations[0].items.push(user(2048, "more"));
    s.apply_cache(ConversationId(1), vec![user(1024, "saved words")], false, 9);
    assert_eq!(s.current().unwrap().cached_at, None);
}

/// The window's size is remembered, within sense, and never written twice for one value.
#[test]
fn the_window_size_is_stored_clamped_and_only_when_it_changes() {
    let mut s = real(0);
    assert_eq!(s.prefs.window_size, None);
    let saves = |out: &Outcome| {
        out.effects
            .iter()
            .filter(|e| matches!(e, Effect::SavePrefs(_)))
            .count()
    };
    let first = s.dispatch(Command::SetWindowSize(1280.4, 800.0));
    assert_eq!(s.prefs.window_size, Some((1280.0, 800.0)));
    assert_eq!(saves(&first), 1);
    let again = s.dispatch(Command::SetWindowSize(1280.0, 800.0));
    assert_eq!(saves(&again), 0, "the same size is not written again");
    s.dispatch(Command::SetWindowSize(10.0, 99_999.0));
    assert_eq!(
        s.prefs.window_size,
        Some((480.0, 8192.0)),
        "kept within sane bounds"
    );
    let bad = s.dispatch(Command::SetWindowSize(f32::NAN, 600.0));
    assert_eq!(saves(&bad), 0);
    assert_eq!(
        s.prefs.window_size,
        Some((480.0, 8192.0)),
        "nonsense is ignored"
    );
}

#[test]
fn inspector_width_can_exceed_the_old_fixed_limit() {
    let mut s = real(0);
    s.dispatch(Command::SetInspectorWidth(1200.0));
    assert_eq!(s.prefs.inspector_width, 1200.0);
    s.dispatch(Command::SetInspectorWidth(99_999.0));
    assert_eq!(s.prefs.inspector_width, 8192.0);
}

/// A real conversation is named for the last prompt sent in it, not for its session id.
#[test]
fn a_real_conversation_is_named_for_its_last_prompt() {
    let mut s = real(1);
    assert!(
        s.conversation(ConversationId(1))
            .unwrap()
            .title
            .starts_with("conversation")
    );
    open(
        &mut s,
        1,
        vec![
            user(1, "first question about the build"),
            user(
                2,
                "Look at https://github.com/jasona/pipkin and create a plan for updating the website content accordingly.",
            ),
        ],
        false,
    );
    let title = s.conversation(ConversationId(1)).unwrap().title.clone();
    assert!(
        title.starts_with("Look at https://github.com/jasona/pipkin"),
        "{title}"
    );
    assert!(
        title.ends_with('\u{2026}') && title.chars().count() <= TITLE_CHARS + 1,
        "{title}"
    );

    // Sending another prompt renames it again.
    s.dispatch(Command::EditDraft("  Make it   shorter\nplease  ".into()));
    let out = s.dispatch(Command::Submit);
    // In real mode the prompt joins the transcript once its journal entry is durable.
    for e in &out.effects {
        if let Effect::JournalIntent {
            conversation,
            request,
            ..
        } = e
        {
            s.intent_persisted(*conversation, request, Ok(()));
        }
    }
    assert_eq!(
        s.conversation(ConversationId(1)).unwrap().title,
        "Make it shorter"
    );
}

#[test]
fn naming_a_conversation_uses_the_first_line_and_never_a_steer_or_the_demo() {
    assert_eq!(prompt_title("  hello   world \nsecond line"), "hello world");
    assert_eq!(
        prompt_title("\n\n  only on line three"),
        "only on line three"
    );
    assert_eq!(prompt_title(""), "");
    let long = "word ".repeat(30);
    let t = prompt_title(&long);
    assert!(
        t.ends_with('\u{2026}') && t.chars().count() <= TITLE_CHARS + 1,
        "{t}"
    );
    assert_eq!(
        prompt_title("héllo wörld"),
        "héllo wörld",
        "characters, not bytes"
    );

    // A steer is not the prompt the conversation is about.
    let mut s = real(1);
    let mut steer = user(2, "also, please hurry");
    if let ItemKind::User { steer, .. } = &mut steer.kind {
        *steer = true;
    }
    open(
        &mut s,
        1,
        vec![user(1, "refactor the parser"), steer],
        false,
    );
    assert_eq!(
        s.conversation(ConversationId(1)).unwrap().title,
        "refactor the parser"
    );

    // The demo keeps its own titles.
    let mut d = AppState::new(boot(1), Prefs::default());
    d.dispatch(Command::SelectConversation(ConversationId(1)));
    d.apply_event(BackendEvent {
        conversation: ConversationId(1),
        generation: d.current().unwrap().generation,
        op: None,
        kind: EventKind::Opened {
            items: vec![user(1, "something else")],
            has_older: false,
            changes: vec![],
        },
    });
    assert_eq!(
        d.conversation(ConversationId(1)).unwrap().title,
        "conversation 1"
    );
}
