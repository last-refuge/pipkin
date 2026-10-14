use std::ops::Range;

use gpui::{Entity, Focusable, TestAppContext, VisualTestContext, px};
use pipkin_core::*;

use super::*;
use crate::theme::Theme as UiTheme;

fn project() -> Project {
    Project {
        id: ProjectId(1),
        name: "demo".into(),
        path: "/demo".into(),
    }
}

fn boot() -> Bootstrap {
    Bootstrap {
        projects: vec![project()],
        models: vec![ModelInfo {
            id: "m".into(),
            name: "M".into(),
            note: String::new(),
        }],
        conversations: vec![
            (ConversationId(1), ProjectId(1), "one".into(), 10),
            (ConversationId(2), ProjectId(1), "two".into(), 5),
        ],
        now: 100,
    }
}

fn item(i: u64) -> TranscriptItem {
    let kind = match i % 5 {
        0 => ItemKind::User {
            text: format!("prompt number {i} please look at src/lib.rs"),
            attachments: vec![],
            delivery: Delivery::Sent,
            steer: false,
        },
        1 | 2 => ItemKind::Assistant {
            text: format!(
                "Paragraph {i} with **bold** and `code`.\n\n{}\n\n```rust\nfn f{i}() {{ println!(\"hi\"); }}\n```\n\n- one\n- two",
                "A fairly long sentence that should wrap across several lines when laid out. "
                    .repeat(4)
            ),
            streaming: false,
        },
        3 => ItemKind::Tool(ToolCall {
            call_ref: None,
            call_id: None,
            name: "bash".into(),
            input: format!("cargo test --package p{i}"),
            output: "line one\nline two\nline three".repeat(3),
            truncated: false,
            full_len: 80,
            status: ToolStatus::Ok,
        }),
        _ => ItemKind::Assistant {
            text: format!("Short reply {i}."),
            streaming: false,
        },
    };
    TranscriptItem {
        native_only: false,
        id: ItemId(i),
        at: 1_700_000_000 + i as i64,
        kind,
    }
}

fn items(r: Range<u64>) -> Vec<TranscriptItem> {
    r.map(item).collect()
}

struct Harness {
    model: Entity<Model>,
    view: Entity<TranscriptView>,
}

fn open(
    model: &Entity<Model>,
    conv: u64,
    items: Vec<TranscriptItem>,
    has_older: bool,
    cx: &mut VisualTestContext,
) {
    cx.update(|_, cx| {
        model.update(cx, |m, cx| {
            let generation = m
                .state
                .conversation(ConversationId(conv))
                .unwrap()
                .generation;
            m.apply_event(
                BackendEvent {
                    conversation: ConversationId(conv),
                    generation,
                    op: None,
                    kind: EventKind::Opened {
                        items,
                        has_older,
                        changes: vec![],
                    },
                },
                cx,
            );
        });
    });
}

/// Force a real window frame (the view is the window's root).
fn frame(_h: &Harness, cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn setup(cx: &mut TestAppContext, n: u64) -> (Harness, &mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(UiTheme::new(
            pipkin_core::Theme::Dark,
            TextSize::Normal,
            false,
        ));
        init(cx);
    });
    let mut state = AppState::new(boot(), Prefs::default());
    state.dispatch(Command::SelectConversation(ConversationId(1)));
    let model = cx.new(|_| Model::new(state));
    let m2 = model.clone();
    let (view, vcx) =
        cx.add_window_view(move |window, cx| TranscriptView::new(m2.clone(), window, cx));
    let h = Harness { model, view };
    open(&h.model, 1, items(0..n), false, vcx);
    frame(&h, vcx);
    (h, vcx)
}

fn anchor(h: &Harness, conv: u64, cx: &mut VisualTestContext) -> (usize, Pixels) {
    cx.update(|_, cx| h.view.read(cx).scroll_anchor(ConversationId(conv)).unwrap())
}

fn list_of(h: &Harness, conv: u64, cx: &mut VisualTestContext) -> ListState {
    cx.update(|_, cx| h.view.read(cx).list_state(ConversationId(conv)).unwrap())
}

fn stats(h: &Harness, cx: &mut VisualTestContext) -> TranscriptStats {
    cx.update(|_, cx| h.view.read(cx).stats(cx))
}

fn append(h: &Harness, text: &str, cx: &mut VisualTestContext) {
    // Drive the model the way the controller would: submit, accept, stream a token.
    cx.update(|_, cx| {
        h.model.update(cx, |m, cx| {
            if matches!(m.state.current().unwrap().run, RunState::Idle) {
                m.dispatch(Command::EditDraft("go".into()), cx);
                m.dispatch(Command::Submit, cx);
                let c = m.state.current().unwrap();
                let ev = BackendEvent {
                    conversation: c.id,
                    generation: c.generation,
                    op: c.run.op(),
                    kind: EventKind::Accepted,
                };
                m.apply_event(ev, cx);
            }
            let c = m.state.current().unwrap();
            let ev = BackendEvent {
                conversation: c.id,
                generation: c.generation,
                op: c.run.op(),
                kind: EventKind::Token(text.into()),
            };
            m.apply_event(ev, cx);
        });
    });
}

#[gpui::test]
fn user_messages_align_right_and_keep_hit_testing(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    let long = format!(
        "Background job failed. Last output:\n{}\n   Compiling pipkin-ui v1.1.0 (/home/jason/coding/pipkin/crates/pipkin-ui)\nUse job_log for the full log.",
        "A long message must wrap inside the conversation column, never overflow into the navigation or inspector. ".repeat(3)
    );
    let mut previous_edges = None;
    for text in ["A short message", long.as_str(), "First line\nSecond line"] {
        let mut user = item(0);
        user.kind = ItemKind::User {
            text: text.into(),
            attachments: vec![],
            delivery: Delivery::Sent,
            steer: false,
        };
        let mut assistant = item(1);
        assistant.kind = ItemKind::Assistant {
            text: "A short reply".into(),
            streaming: false,
        };
        open(&h.model, 1, vec![user, assistant], false, cx);
        frame(&h, cx);
        cx.update(|_, cx| {
            let registry = h.view.read(cx).registry.borrow();
            let user = registry.find(ItemId(0), 0).expect("user painted");
            let assistant = registry.find(ItemId(1), 0).expect("assistant painted");
            assert!(user.bounds.left() > assistant.bounds.left() + px(40.0));
            // The bubble ends at the conversation boundary; its text is inset
            // by the bubble's horizontal padding.
            let padding = px(14.0 * cx.global::<UiTheme>().scale);
            assert!((user.bounds.right() + padding - assistant.bounds.right()).abs() < px(1.0));
            let edges = (user.bounds.left(), user.bounds.right());
            if let Some(previous) = previous_edges {
                assert_eq!(edges, previous, "message length must not change alignment");
            }
            previous_edges = Some(edges);
            if text == long {
                assert!(user.layout.wrapped_text().lines().count() > text.lines().count());
            }
            for (offset, ch) in text.char_indices() {
                let mut position = user.layout.position_for_index(offset).unwrap();
                assert!(position.x >= user.bounds.left() - px(1.0));
                assert!(position.x <= user.bounds.right() + px(1.0));
                if ch != '\n' {
                    position.x += px(0.5);
                    position.y += user.layout.line_height() / 2.0;
                    assert_eq!(registry.hit_test(position), Some((ItemId(0), 0, offset)));
                }
            }
        });
    }
}

#[gpui::test]
fn background_reports_are_collapsed_activity_with_selectable_details(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    for source in [BackgroundSource::Job, BackgroundSource::Subagent] {
        let text = "Background work finished.\nAnswer:\nChanges made and validated.\nArtifacts:\n- file: /project/src/lib.rs";
        let report = TranscriptItem {
            id: ItemId(42),
            at: 1_700_000_000,
            native_only: true,
            kind: ItemKind::Background {
                text: text.into(),
                source,
            },
        };
        assert!(
            !is_activity(&report.kind),
            "reports are not folded into tool runs"
        );
        open(&h.model, 1, vec![report], false, cx);
        frame(&h, cx);
        cx.update(|_, cx| {
            let view = h.view.read(cx);
            assert!(!view.is_expanded(ConversationId(1), ItemId(42)));
            assert!(view.registry.borrow().find(ItemId(42), 0).is_none());
        });
        cx.update(|_, cx| {
            h.view.update(cx, |view, cx| {
                view.toggle_tool(ConversationId(1), ItemId(42), cx);
            })
        });
        frame(&h, cx);
        cx.update(|_, cx| {
            let view = h.view.read(cx);
            let registry = view.registry.borrow();
            let hit = registry
                .find(ItemId(42), 0)
                .expect("expanded report paints its details");
            assert_eq!(hit.layout.text(), text);
            let mut position = hit.layout.position_for_index(0).unwrap();
            position.x += px(0.5);
            position.y += hit.layout.line_height() / 2.0;
            assert_eq!(registry.hit_test(position), Some((ItemId(42), 0, 0)));
        });
        cx.update(|_, cx| {
            h.view.update(cx, |view, cx| {
                view.toggle_tool(ConversationId(1), ItemId(42), cx);
            })
        });
        frame(&h, cx);
        cx.update(|_, cx| {
            assert!(
                h.view
                    .read(cx)
                    .registry
                    .borrow()
                    .find(ItemId(42), 0)
                    .is_none()
            )
        });
    }
}

#[gpui::test]
fn mounted_rows_are_bounded_for_ten_thousand_items(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 10_000);
    frame(&h, cx);
    let s = stats(&h, cx);
    assert!(s.mounted_rows > 0, "nothing rendered");
    assert!(s.mounted_rows < 150, "mounted {} rows", s.mounted_rows);
    assert!(s.cached_blocks < 300, "cached {}", s.cached_blocks);
    assert!(s.following);
}

#[gpui::test]
fn tail_follow_pauses_when_reading_above_and_resumes_on_jump(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 200);
    assert!(stats(&h, cx).following);
    let list = list_of(&h, 1, cx);
    list.scroll_by(px(-3000.));
    frame(&h, cx);
    assert!(!stats(&h, cx).following);
    let before = anchor(&h, 1, cx);
    for i in 0..5 {
        append(&h, &format!("token {i} "), cx);
        frame(&h, cx);
    }
    // Streaming below must not move the reading position.
    assert_eq!(anchor(&h, 1, cx), before);
    assert!(!stats(&h, cx).following);
    cx.update(|window, cx| h.view.update(cx, |v, cx| v.jump_to_latest(window, cx)));
    frame(&h, cx);
    assert!(stats(&h, cx).following);
    append(&h, "more ", cx);
    frame(&h, cx);
    assert!(stats(&h, cx).following);
}

fn sync_snapshot(h: &Harness, items: Vec<TranscriptItem>, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        h.model.update(cx, |m, cx| {
            let c = m.state.current().unwrap();
            let event = BackendEvent {
                conversation: c.id,
                generation: c.generation,
                op: None,
                kind: EventKind::Synced { items },
            };
            m.apply_event(event, cx);
        });
    });
    frame(h, cx);
}

#[gpui::test]
fn engine_snapshots_preserve_reading_position_and_resume_at_the_bottom(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 200);
    list_of(&h, 1, cx).scroll_by(px(-3000.));
    frame(&h, cx);
    let before = anchor(&h, 1, cx);
    assert!(!stats(&h, cx).following);
    for n in 201..206 {
        sync_snapshot(&h, items(0..n), cx);
        assert_eq!(anchor(&h, 1, cx), before);
        assert!(!stats(&h, cx).following);
    }
    // A same-length snapshot can also grow the last message.
    let mut snapshot = items(0..205);
    snapshot.last_mut().unwrap().kind = ItemKind::Assistant {
        text: "New output below the reader.\n\n".repeat(100),
        streaming: true,
    };
    sync_snapshot(&h, snapshot, cx);
    assert_eq!(anchor(&h, 1, cx), before);
    assert!(!stats(&h, cx).following);

    list_of(&h, 1, cx).scroll_by(px(1_000_000.));
    frame(&h, cx);
    assert!(stats(&h, cx).following);
    sync_snapshot(&h, items(0..206), cx);
    assert!(stats(&h, cx).following);
}

#[gpui::test]
fn snapshot_with_earlier_history_keeps_the_same_message_visible(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    open(&h.model, 1, items(100..300), false, cx);
    frame(&h, cx);
    list_of(&h, 1, cx).scroll_by(px(-3000.));
    frame(&h, cx);
    let (ix, offset) = anchor(&h, 1, cx);
    assert!(ix > 1);
    sync_snapshot(&h, items(50..301), cx);
    assert_eq!(anchor(&h, 1, cx), (ix + 50, offset));
    assert!(!stats(&h, cx).following);
}

#[gpui::test]
fn missed_append_note_does_not_resume_following(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 200);
    list_of(&h, 1, cx).scroll_by(px(-3000.));
    frame(&h, cx);
    let before = anchor(&h, 1, cx);
    cx.update(|_, cx| {
        h.model.update(cx, |m, cx| {
            // Model notification without a structural note exercises ensure_conv's fallback.
            m.state
                .conversations
                .iter_mut()
                .find(|c| c.id == ConversationId(1))
                .unwrap()
                .items
                .push(item(200));
            cx.notify();
        });
    });
    frame(&h, cx);
    assert_eq!(anchor(&h, 1, cx), before);
    assert!(!stats(&h, cx).following);
}

#[gpui::test]
fn prepending_history_keeps_the_viewport_anchored(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    open(&h.model, 1, items(100..300), true, cx);
    frame(&h, cx);
    let list = list_of(&h, 1, cx);
    list.scroll_by(px(-900.));
    frame(&h, cx);
    let (ix, off) = anchor(&h, 1, cx);
    assert!(ix > 3, "scrolled too little: {ix}");
    let generation = cx.update(|_, cx| {
        h.model
            .read(cx)
            .state
            .conversation(ConversationId(1))
            .unwrap()
            .generation
    });
    cx.update(|_, cx| {
        h.model.update(cx, |m, cx| {
            m.apply_event(
                BackendEvent {
                    conversation: ConversationId(1),
                    generation,
                    op: None,
                    kind: EventKind::OlderPage {
                        items: items(50..100),
                        has_older: false,
                    },
                },
                cx,
            )
        })
    });
    frame(&h, cx);
    assert_eq!(anchor(&h, 1, cx), (ix + 50, off));
}

#[gpui::test]
fn each_conversation_restores_its_own_scroll_position(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 300);
    list_of(&h, 1, cx).scroll_by(px(-1200.));
    frame(&h, cx);
    let a1 = anchor(&h, 1, cx);
    cx.update(|_, cx| {
        h.model.update(cx, |m, cx| {
            m.dispatch(Command::SelectConversation(ConversationId(2)), cx)
        })
    });
    open(&h.model, 2, items(0..50), false, cx);
    frame(&h, cx);
    assert!(
        stats(&h, cx).following,
        "fresh conversation starts at the tail"
    );
    cx.update(|_, cx| {
        h.model.update(cx, |m, cx| {
            m.dispatch(Command::SelectConversation(ConversationId(1)), cx)
        })
    });
    frame(&h, cx);
    assert_eq!(anchor(&h, 1, cx), a1);
    assert!(!stats(&h, cx).following);
}

#[gpui::test]
fn expanding_a_tool_does_not_move_the_anchor(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 300);
    list_of(&h, 1, cx).scroll_by(px(-1500.));
    frame(&h, cx);
    let (ix, off) = anchor(&h, 1, cx);
    // First tool row at or below the top item.
    let tool = (ix..300).find(|i| i % 5 == 3 && *i >= ix).unwrap() as u64;
    cx.update(|_, cx| {
        h.view.update(cx, |v, cx| {
            v.toggle_tool(ConversationId(1), ItemId(tool), cx)
        })
    });
    frame(&h, cx);
    assert!(cx.update(|_, cx| h.view.read(cx).is_expanded(ConversationId(1), ItemId(tool))));
    assert_eq!(anchor(&h, 1, cx), (ix, off));
    cx.update(|_, cx| {
        h.view.update(cx, |v, cx| {
            v.toggle_tool(ConversationId(1), ItemId(tool), cx)
        })
    });
    frame(&h, cx);
    assert_eq!(anchor(&h, 1, cx), (ix, off));
}

#[gpui::test]
fn selection_copies_across_rows_including_offscreen(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 10_000);
    let conv = ConversationId(1);
    let sel = Selection {
        anchor: DocPos {
            item: ItemId(10),
            block: 0,
            offset: 7,
        },
        head: DocPos {
            item: ItemId(12),
            block: 1,
            offset: 4,
        },
    };
    cx.update(|_, cx| {
        h.view
            .update(cx, |v, cx| v.set_selection(conv, Some(sel), cx))
    });
    frame(&h, cx);
    let text = cx
        .update(|_, cx| h.view.read(cx).selection_text(cx))
        .unwrap();
    assert!(
        text.starts_with("number 10 please look at src/lib.rs"),
        "{text}"
    );
    assert!(text.contains("Paragraph 11 with bold and code."));
    assert!(text.contains("fn f11()"));
    assert!(text.ends_with("A fa"), "{text:?}");
    assert!(stats(&h, cx).selection_len > 0);
    // Streaming appends do not corrupt the selection.
    let before = text.clone();
    append(&h, "streamed ", cx);
    frame(&h, cx);
    assert_eq!(
        cx.update(|_, cx| h.view.read(cx).selection_text(cx))
            .unwrap(),
        before
    );
}

#[gpui::test]
fn mouse_drag_selects_across_rows_and_select_all_copies_everything(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 40);
    frame(&h, cx);
    let hits: Vec<_> = cx.update(|_, cx| h.view.read(cx).registry.borrow().blocks.clone());
    assert!(
        hits.len() > 3,
        "expected several painted blocks, got {}",
        hits.len()
    );
    let visible: Vec<_> = hits
        .iter()
        .filter(|h| h.bounds.top() >= px(0.) && h.bounds.bottom() <= px(1000.))
        .collect();
    assert!(
        visible.len() > 3,
        "expected visible blocks, got {}",
        visible.len()
    );
    let a = visible.first().unwrap().bounds;
    let b = visible.last().unwrap().bounds;
    cx.simulate_mouse_down(a.center(), gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.simulate_mouse_move(b.center(), gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.simulate_mouse_up(b.center(), gpui::MouseButton::Left, gpui::Modifiers::none());
    let text = cx
        .update(|_, cx| h.view.read(cx).selection_text(cx))
        .unwrap_or_default();
    assert!(
        text.contains("\n\n"),
        "drag should span multiple blocks: {text:?}"
    );

    cx.update(|_, cx| h.view.update(cx, |v, cx| v.select_all(cx)));
    let all = cx
        .update(|_, cx| h.view.read(cx).selection_text(cx))
        .unwrap();
    assert!(all.starts_with("prompt number 0") && all.contains("Short reply 39."));
    cx.update(|_, cx| h.view.update(cx, |v, cx| v.clear_selection(cx)));
    assert!(!cx.update(|_, cx| h.view.read(cx).has_selection()));
}

#[gpui::test]
fn load_older_is_requested_once_near_the_top(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    open(&h.model, 1, items(100..160), true, cx);
    frame(&h, cx);
    list_of(&h, 1, cx).scroll_to(ListOffset {
        item_ix: 0,
        offset_in_item: px(0.),
    });
    frame(&h, cx);
    cx.run_until_parked();
    let loading = cx.update(|_, cx| {
        h.model
            .read(cx)
            .state
            .conversation(ConversationId(1))
            .unwrap()
            .loading_older
    });
    assert!(loading, "reaching the top should request the previous page");
}

#[gpui::test]
fn empty_and_unselected_states_render(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    frame(&h, cx);
    assert_eq!(stats(&h, cx).mounted_rows, 0);
}

#[gpui::test]
fn keyboard_select_all_copy_extend_and_escape(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 30);
    let conv = ConversationId(1);
    let handle = cx.update(|_, cx| h.view.read(cx).focus_handle(cx));
    cx.update(|window, cx| window.focus(&handle, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-a");
    assert!(cx.update(|_, cx| h.view.read(cx).has_selection()));
    cx.simulate_keystrokes("ctrl-c");
    let copied = cx.read_from_clipboard().and_then(|c| c.text()).unwrap();
    assert!(copied.starts_with("prompt number 0") && copied.contains("Short reply 29."));
    cx.simulate_keystrokes("escape");
    assert!(!cx.update(|_, cx| h.view.read(cx).has_selection()));
    // shift-right extends an existing caret by whole graphemes.
    let sel = Selection::caret(DocPos {
        item: ItemId(0),
        block: 0,
        offset: 0,
    });
    cx.update(|_, cx| {
        h.view
            .update(cx, |v, cx| v.set_selection(conv, Some(sel), cx))
    });
    cx.simulate_keystrokes("shift-right shift-right shift-right");
    assert_eq!(
        cx.update(|_, cx| h.view.read(cx).selection_text(cx))
            .unwrap(),
        "pro"
    );
    cx.simulate_keystrokes("ctrl-shift-end");
    let tail = cx
        .update(|_, cx| h.view.read(cx).selection_text(cx))
        .unwrap();
    assert!(tail.ends_with("Short reply 29."));
}

#[gpui::test]
fn stress_content_renders_without_panicking(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    let long = "x".repeat(5000);
    let nasty = vec![
        TranscriptItem {
            native_only: false,
            id: ItemId(1),
            at: 0,
            kind: ItemKind::User {
                text: "مرحبا بالعالم שלום עולם 👨‍👩‍👧‍👦 e\u{301}\u{301}".into(),
                attachments: vec![Attachment {
                    path: "/missing".into(),
                    name: "missing.txt".into(),
                    size: None,
                    error: Some("File not found".into()),
                }],
                delivery: Delivery::Unknown,
                steer: false,
            },
        },
        TranscriptItem {
            native_only: false,
            id: ItemId(2),
            at: 0,
            kind: ItemKind::Assistant {
                text: format!(
                    "**unclosed and `tick\n\n```rust\nfn open() {{\n\n- a\n    - b\n  - c\n\n/very/long/{long}/path.rs {long}"
                ),
                streaming: true,
            },
        },
        TranscriptItem {
            native_only: false,
            id: ItemId(3),
            at: 0,
            kind: ItemKind::Tool(ToolCall {
                call_ref: None,
                call_id: None,
                name: "bash".into(),
                input: long.clone(),
                output: long,
                truncated: true,
                full_len: 2_000_000,
                status: ToolStatus::Failed,
            }),
        },
        TranscriptItem {
            native_only: false,
            id: ItemId(4),
            at: 0,
            kind: ItemKind::Assistant {
                text: String::new(),
                streaming: true,
            },
        },
        TranscriptItem {
            native_only: false,
            id: ItemId(5),
            at: 0,
            kind: ItemKind::Notice {
                text: "Provider unavailable".into(),
                level: NoticeLevel::Error,
            },
        },
    ];
    open(&h.model, 1, nasty, false, cx);
    cx.update(|_, cx| {
        h.view
            .update(cx, |v, cx| v.toggle_tool(ConversationId(1), ItemId(3), cx))
    });
    cx.update(|_, cx| h.view.update(cx, |v, cx| v.select_all(cx)));
    frame(&h, cx);
    assert!(stats(&h, cx).mounted_rows >= 1);
    let all = cx
        .update(|_, cx| h.view.read(cx).selection_text(cx))
        .unwrap();
    assert!(all.contains("שלום") && all.contains("fn open()"));
}

/// Resident memory of this process, in kilobytes.
fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.split_whitespace().next()?.parse().ok())
        })
        .unwrap_or(0)
}

/// Walk the whole of a 10,000-message conversation from top to bottom, again and again, with
/// the model streaming into it while it is being read: what is built per frame, what is cached
/// and the memory of the process all stay bounded, so a long session does not grow.
#[gpui::test]
fn repeated_traversal_of_ten_thousand_items_with_live_updates_stays_bounded(
    cx: &mut TestAppContext,
) {
    let (h, cx) = setup(cx, 10_000);
    frame(&h, cx);
    let list = list_of(&h, 1, cx);
    let mut max_mounted = 0;
    let mut max_cached = 0;
    let mut rss: Vec<u64> = Vec::new();
    for pass in 0..6 {
        for (step, ix) in (0..10_000usize).step_by(89).enumerate() {
            list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
            frame(&h, cx);
            // Live output arrives while the person reads somewhere else.
            if step % 20 == 0 {
                append(&h, "streaming ", cx);
                frame(&h, cx);
            }
            let s = stats(&h, cx);
            assert!(
                s.mounted_rows > 0,
                "pass {pass}, item {ix}: nothing rendered"
            );
            max_mounted = max_mounted.max(s.mounted_rows);
            max_cached = max_cached.max(s.cached_blocks);
        }
        rss.push(rss_kb());
    }
    eprintln!(
        "traversal: max mounted rows {max_mounted}, max cached blocks {max_cached}, RSS per pass (KB) {rss:?}"
    );
    assert!(max_mounted < 150, "mounted {max_mounted} rows at once");
    assert!(max_cached <= 2000, "cached {max_cached} blocks");
    // The first passes fill the caches; after that nothing may keep growing.
    let growth = rss[5].saturating_sub(rss[1]);
    assert!(
        growth < 40 * 1024,
        "memory grew {growth} KB over four more traversals: {rss:?}"
    );
}

fn tool(i: u64) -> TranscriptItem {
    TranscriptItem {
        native_only: false,
        id: ItemId(i),
        at: 1_700_000_000 + i as i64,
        kind: ItemKind::Tool(ToolCall {
            call_ref: None,
            call_id: None,
            name: "read".into(),
            input: format!("{{\"path\":\"src/f{i}.rs\"}}"),
            output: String::new(),
            truncated: false,
            full_len: 0,
            status: ToolStatus::Ok,
        }),
    }
}

/// How far the list can scroll: content taller than the window, which is zero while it all fits.
fn scroll_extent(h: &Harness, cx: &mut VisualTestContext) -> f32 {
    frame(h, cx);
    f32::from(list_of(h, 1, cx).max_offset_for_scrollbar().y)
}

fn notice(i: u64, text: &str) -> TranscriptItem {
    TranscriptItem {
        native_only: false,
        id: ItemId(i),
        at: 1_700_000_000 + i as i64,
        kind: ItemKind::Notice {
            text: text.into(),
            level: NoticeLevel::Info,
        },
    }
}

fn toggle(h: &Harness, first: u64, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        h.view.update(cx, |v, cx| {
            v.toggle_steps(ConversationId(1), ItemId(first), cx)
        })
    });
}

#[gpui::test]
fn runs_of_tool_steps_fold_to_the_latest_and_open_to_show_all(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    // A prompt, a thinking note and 30 steps, a reply, then another thinking note, 30 more
    // steps and a last reply: two runs, kept apart by the reply between them.
    let mut all = vec![item(0), notice(1, "Thinking\nfirst")];
    all.extend((100..130).map(tool));
    all.push(item(6));
    all.push(notice(2, "Thinking\nsecond"));
    all.extend((200..230).map(tool));
    all.push(item(11));
    open(&h.model, 1, all, false, cx);

    // Folded, 60 steps take the room of two cards, so everything fits in the window.
    let folded = scroll_extent(&h, cx);
    assert!(
        folded < 1.0,
        "folded steps should fit the window, scrolls {folded}"
    );

    // Opening the first run shows its 30 steps; the second stays folded.
    toggle(&h, 1, cx);
    let one_open = scroll_extent(&h, cx);
    assert!(
        one_open > 100.0,
        "30 steps should overflow the window: {one_open}"
    );

    // Opening the second as well shows 60.
    toggle(&h, 2, cx);
    let both_open = scroll_extent(&h, cx);
    assert!(
        both_open > one_open + 500.0,
        "the second run opens independently: {one_open} -> {both_open}"
    );

    // Folding both again returns to the start.
    toggle(&h, 1, cx);
    toggle(&h, 2, cx);
    assert!(scroll_extent(&h, cx) < 1.0);
}

/// Start a run (submit, journal, accept), then send one engine event the way the controller would.
fn run_event(h: &Harness, kind: EventKind, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        h.model.update(cx, |m, cx| {
            if matches!(m.state.current().unwrap().run, RunState::Idle) {
                m.dispatch(Command::EditDraft("go".into()), cx);
                m.dispatch(Command::Submit, cx);
                // The journal commit the controller would make before anything is sent.
                let (conv, request) = {
                    let c = m.state.current().unwrap();
                    (c.id, c.pending_intent.as_ref().unwrap().request.clone())
                };
                m.mutate(|s| s.intent_persisted(conv, &request, Ok(())), cx);
                let c = m.state.current().unwrap();
                let accepted = BackendEvent {
                    conversation: c.id,
                    generation: c.generation,
                    op: c.run.op(),
                    kind: EventKind::Accepted,
                };
                m.apply_event(accepted, cx);
            }
            let c = m.state.current().unwrap();
            let ev = BackendEvent {
                conversation: c.id,
                generation: c.generation,
                op: c.run.op(),
                kind,
            };
            m.apply_event(ev, cx);
        });
    });
}

#[gpui::test]
fn steps_arriving_live_stay_folded_to_one_card_and_show_all_when_opened(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    for call in 0..40u32 {
        run_event(
            &h,
            EventKind::ToolStarted {
                call,
                name: "read".into(),
                input: format!("{{\"path\":\"src/f{call}.rs\"}}"),
                call_id: None,
            },
            cx,
        );
        run_event(&h, EventKind::ToolFinished { call, ok: true }, cx);
        frame(&h, cx);
    }
    // Forty steps, folded: still the room of a card or two.
    let folded = scroll_extent(&h, cx);
    assert!(folded < 1.0, "live steps should fold, scrolls {folded}");

    // Open the run (named by its first step) and the forty show.
    let first = cx.update(|_, cx| {
        let c = h.model.read(cx).state.current().unwrap();
        c.items
            .iter()
            .find(|i| matches!(i.kind, ItemKind::Tool(_)))
            .map(|i| i.id)
            .unwrap()
    });
    toggle(&h, first.0, cx);
    let open = scroll_extent(&h, cx);
    assert!(
        open > 100.0,
        "opened steps should overflow the window: {open}"
    );

    // A step that arrives while it is open is shown too (following along).
    run_event(
        &h,
        EventKind::ToolStarted {
            call: 99,
            name: "read".into(),
            input: "{\"path\":\"src/new.rs\"}".into(),
            call_id: None,
        },
        cx,
    );
    let longer = scroll_extent(&h, cx);
    assert!(
        longer > open + 20.0,
        "a new step adds a row: {open} -> {longer}"
    );
}

#[test]
fn a_long_target_is_cut_to_the_first_characters_with_an_ellipsis() {
    assert_eq!(shorten("cargo check", 60), "cargo check");
    let long = "git clone --depth 1 https://github.com/jasona/pipkin \"$tmp/pipkin\" && ls -la";
    let cut = shorten(long, 20);
    assert_eq!(cut, "git clone --depth 1\u{2026}");
    assert!(cut.chars().count() <= 21);
    assert_eq!(
        shorten("héllo wörld", 5),
        "héllo\u{2026}",
        "counts characters, not bytes"
    );
}

#[test]
fn a_reply_after_steps_or_thinking_is_labelled_but_one_after_a_reply_is_not() {
    let reply = ItemKind::Assistant {
        text: "x".into(),
        streaming: false,
    };
    let user = ItemKind::User {
        text: "q".into(),
        attachments: vec![],
        delivery: Delivery::Sent,
        steer: false,
    };
    let step = tool(1).kind;
    let thinking = notice(2, "Thinking\nhm").kind;
    assert!(starts_reply(None), "the first item");
    assert!(starts_reply(Some(&user)));
    assert!(starts_reply(Some(&step)), "steps do not continue a reply");
    assert!(starts_reply(Some(&thinking)));
    assert!(!starts_reply(Some(&reply)), "a second part of one reply");
}

#[test]
fn collapsed_work_shows_latest_explanation_then_outcome_not_tool_details() {
    let items = vec![
        notice(1, "Thinking\n**Inspecting llm-docs file**"),
        tool(2),
        notice(3, "Thinking\n**Preparing two commits**"),
        tool(4),
    ];
    let summary = "Read 1 file, ran 2 commands · 9s";
    assert_eq!(
        activity_caption(&items, true, false, summary),
        "Preparing two commits"
    );
    assert_eq!(
        activity_caption(&items, false, false, summary),
        "Read 1 file, ran 2 commands · 9s · Done."
    );
    assert_eq!(
        activity_caption(&items, false, true, summary),
        "Read 1 file, ran 2 commands · 9s · Finished with errors."
    );
    assert_eq!(activity_caption(&items, false, false, ""), "Done.");
    assert_eq!(
        activity_caption(&items[1..2], true, false, summary),
        "Read 1 file, ran 2 commands · 9s"
    );
    assert_eq!(
        activity_caption(&[notice(5, "Thinking...")], true, false, summary),
        "Working…"
    );
    assert_eq!(
        activity_caption(&[notice(5, "Thinking..."), tool(6)], true, false, summary),
        summary,
    );
    assert_eq!(
        activity_caption(
            &[tool(6), notice(7, "Thinking..."), tool(8)],
            true,
            false,
            summary
        ),
        summary,
    );
}

#[test]
fn recent_activity_uses_observed_tools_and_resets_at_the_latest_prompt() {
    let mut active = tool(3);
    if let ItemKind::Tool(t) = &mut active.kind {
        t.status = ToolStatus::Running;
        t.name = "edit".into();
    }
    let items = vec![tool(1), item(0), tool(2), notice(4, "Thinking..."), active];
    assert_eq!(
        super::super::tools::recent_activity(&items).as_deref(),
        Some("Read 1 file, editing 1 file"),
    );
    assert_eq!(super::super::tools::recent_activity(&items[..2]), None);
    assert_eq!(
        super::super::tools::recent_activity(&[item(0), notice(4, "Thinking..."), tool(2)])
            .as_deref(),
        Some("Read 1 file"),
    );
    assert_eq!(
        super::super::tools::recent_activity(&[item(0), notice(4, "Thinking\n**Checking build**")])
            .as_deref(),
        Some("Checking build"),
    );
    let mut failed = tool(8);
    if let ItemKind::Tool(t) = &mut failed.kind {
        t.status = ToolStatus::Failed;
    }
    assert_eq!(
        super::super::tools::recent_activity(&[item(0), failed]).as_deref(),
        Some("1 tool failed"),
    );
    let many: Vec<_> = (0..257).map(tool).collect();
    assert_eq!(
        super::super::tools::recent_activity(&many).as_deref(),
        Some("Recent: Read 256 files"),
    );
}

#[gpui::test]
fn observed_activity_updates_as_tool_calls_start_and_finish(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    let activity = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let items = &h.model.read(cx).state.current().unwrap().items;
            super::super::tools::recent_activity(items)
        })
    };
    run_event(
        &h,
        EventKind::ToolStarted {
            call: 1,
            name: "read".into(),
            input: "{}".into(),
            call_id: None,
        },
        cx,
    );
    assert_eq!(activity(cx).as_deref(), Some("Reading 1 file"));
    run_event(&h, EventKind::ToolFinished { call: 1, ok: true }, cx);
    assert_eq!(activity(cx).as_deref(), Some("Read 1 file"));
    run_event(
        &h,
        EventKind::ToolStarted {
            call: 2,
            name: "edit".into(),
            input: "{}".into(),
            call_id: None,
        },
        cx,
    );
    assert_eq!(activity(cx).as_deref(), Some("Read 1 file, editing 1 file"));
    run_event(&h, EventKind::ToolFinished { call: 2, ok: true }, cx);
    assert_eq!(activity(cx).as_deref(), Some("Read 1 file, edited 1 file"));
}

#[gpui::test]
fn thinking_and_steps_between_a_prompt_and_its_reply_fold_into_one_line(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    // A prompt, then thirty rounds of "Thinking" and a step, then the reply.
    let mut all = vec![item(0)];
    for n in 0..30u64 {
        all.push(notice(
            1000 + n,
            &format!("Thinking\n**round {n}**\nsome thoughts"),
        ));
        all.push(tool(2000 + n));
    }
    all.push(item(1));
    open(&h.model, 1, all, false, cx);

    // Sixty alternating rows fold to one line, so everything fits in the window.
    let folded = scroll_extent(&h, cx);
    assert!(
        folded < 1.0,
        "interleaved work should fold to one line, scrolls {folded}"
    );

    // Opened by the run's first item, all sixty show.
    toggle(&h, 1000, cx);
    let open = scroll_extent(&h, cx);
    assert!(open > 100.0, "an opened run shows every line: {open}");

    toggle(&h, 1000, cx);
    assert!(scroll_extent(&h, cx) < 1.0);
}

#[gpui::test]
fn an_error_notice_is_never_folded_into_a_run(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    let mut all = vec![item(0)];
    all.extend((0..30).map(|n| tool(100 + n)));
    // An error between two runs keeps them apart and stays visible itself.
    all.push(TranscriptItem {
        native_only: false,
        id: ItemId(500),
        at: 1_700_000_500,
        kind: ItemKind::Notice {
            text: "The engine failed".into(),
            level: NoticeLevel::Error,
        },
    });
    all.extend((0..30).map(|n| tool(600 + n)));
    open(&h.model, 1, all, false, cx);
    toggle(&h, 100, cx);
    toggle(&h, 600, cx);
    let both_open = scroll_extent(&h, cx);
    // Folding only the first run leaves the second (60 steps' worth) open.
    toggle(&h, 100, cx);
    let one_open = scroll_extent(&h, cx);
    assert!(both_open > one_open + 500.0, "{both_open} vs {one_open}");
}

#[gpui::test]
fn jobs_are_rows_in_the_transcript_that_follow_the_engine(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 0);
    let call = TranscriptItem {
        id: ItemId(42),
        at: 1_700_000_000,
        native_only: true,
        kind: ItemKind::Tool(ToolCall {
            call_ref: None,
            call_id: Some("c-1".into()),
            name: "job_start".into(),
            input: "{\"command\":\"cargo test --workspace\"}".into(),
            output: String::new(),
            truncated: false,
            full_len: 0,
            status: ToolStatus::Ok,
        }),
    };
    open(&h.model, 1, vec![call], false, cx);
    let job = Job {
        id: 3,
        name: Some("Full workspace tests".into()),
        command: "cargo test --workspace".into(),
        cwd: "/home/you/project".into(),
        origin: JobOrigin::Model,
        state: JobState::Running,
        exit_code: None,
        reason: None,
        call_id: Some("c-1".into()),
        task: None,
        wake: true,
        delivery: "pending".into(),
        log_bytes: 0,
        log_truncated: false,
        created_at_ms: 1_700_000_000_000,
        started_at_ms: Some(1_700_000_000_000),
        ended_at_ms: None,
    };
    cx.update(|_, cx| {
        h.model.update(cx, |m, cx| {
            let c = m.state.current().unwrap();
            let ev = BackendEvent {
                conversation: c.id,
                generation: c.generation,
                op: None,
                kind: EventKind::JobsSynced {
                    jobs: JobsSync {
                        jobs: vec![job.clone()],
                        active: 1,
                        truncated: false,
                    },
                },
            };
            m.apply_event(ev, cx);
        })
    });
    frame(&h, cx);
    cx.update(|_, cx| {
        let items = &h.model.read(cx).state.current().unwrap().items;
        assert!(
            matches!(&items[0].kind, ItemKind::Job(j) if j.id == 3),
            "the job_start call became the job's row"
        );
        assert!(
            !is_activity(&items[0].kind),
            "a job is not folded into the steps"
        );
        let view = h.view.read(cx);
        assert!(!view.is_expanded(ConversationId(1), ItemId(42)));
        assert!(view.registry.borrow().find(ItemId(42), 0).is_none());
    });
    // Opening the row makes it the job the core keeps a log tail for, and its command
    // becomes selectable text.
    cx.update(|_, cx| {
        h.view.update(cx, |view, cx| {
            view.toggle_job(ConversationId(1), ItemId(42), 3, cx);
        })
    });
    frame(&h, cx);
    cx.update(|_, cx| {
        let view = h.view.read(cx);
        assert!(view.is_expanded(ConversationId(1), ItemId(42)));
        assert!(view.registry.borrow().find(ItemId(42), 0).is_some());
        let c = h.model.read(cx).state.current().unwrap();
        assert_eq!(c.jobs.expanded, Some(3));
    });
}
