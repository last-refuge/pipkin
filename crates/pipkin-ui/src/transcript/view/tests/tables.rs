use super::*;
use crate::transcript::markdown::{TABLE_EXAMPLE, parse};
use unicode_segmentation::UnicodeSegmentation;

fn table_window(cx: &mut TestAppContext, width: f32) -> (Harness, VisualTestContext) {
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
    let m = model.clone();
    let window = cx.open_window(gpui::size(px(width), px(4000.)), |window, cx| {
        TranscriptView::new(m, window, cx)
    });
    let view = window.root(cx).unwrap();
    let visual = VisualTestContext::from_window(window.into(), cx);
    (Harness { model, view }, visual)
}

#[gpui::test]
fn tables_align_wrap_hit_test_and_activate_links_at_narrow_and_wide_widths(
    cx: &mut TestAppContext,
) {
    let source = format!(
        "{TABLE_EXAMPLE}\n| {} | é 👨‍👩‍👧‍👦 | [docs](https://example.com) and `code` |",
        "Long cell content must wrap without moving its neighbours. ".repeat(4)
    );
    let blocks = parse(&source);
    for width in [340., 900.] {
        let (h, mut cx) = table_window(cx, width);
        let mut message = item(1);
        message.kind = ItemKind::Assistant {
            text: source.clone(),
            streaming: false,
        };
        open(&h.model, 1, vec![message], false, &mut cx);
        frame(&h, &mut cx);
        let link = cx.update(|_, cx| {
            let view = h.view.read(cx);
            let registry = view.registry.borrow();
            assert_eq!(registry.blocks.len(), blocks.len());
            let first = registry.find(ItemId(1), 0).unwrap().bounds;
            let second = registry.find(ItemId(1), 1).unwrap().bounds;
            assert!((first.size.width - second.size.width).abs() < px(1.));
            assert!(first.right() <= second.left() + px(1.));
            for (bi, block) in blocks.iter().enumerate() {
                let hit = registry.find(ItemId(1), bi as u32).unwrap();
                let column = registry.find(ItemId(1), (bi % 3) as u32).unwrap();
                let row = registry.find(ItemId(1), (bi / 3 * 3) as u32).unwrap();
                assert!((hit.bounds.left() - column.bounds.left()).abs() < px(1.));
                assert_eq!(hit.bounds.top(), row.bounds.top());
                assert_eq!(hit.bounds.bottom(), row.bounds.bottom());
                for (offset, grapheme) in block
                    .text
                    .grapheme_indices(true)
                    .filter(|(_, g)| !g.trim().is_empty())
                {
                    let mut p = hit.position_for_index(offset).unwrap();
                    assert!(
                        p.x >= hit.bounds.left() && p.x <= hit.bounds.right(),
                        "{width}, {bi}, {grapheme}"
                    );
                    p.x += px(0.5);
                    p.y += hit.layout.line_height() / 2.;
                    assert_eq!(
                        registry.hit_test(p),
                        Some((ItemId(1), bi as u32, offset)),
                        "{width}, {bi}, {grapheme}"
                    );
                }
            }
            let tall = registry.find(ItemId(1), 15).unwrap();
            assert!(tall.layout.wrapped_text().lines().count() > 1);
            let short = registry.find(ItemId(1), 16).unwrap();
            assert!(short.bounds.size.height > short.layout.bounds().size.height + px(12.));
            assert_eq!(
                registry.hit_test(point(
                    short.bounds.center().x,
                    short.bounds.bottom() - px(2.)
                )),
                Some((ItemId(1), 16, blocks[16].text.len()))
            );
            let right = registry.find(ItemId(1), 17).unwrap();
            let p = right.position_for_index(0).unwrap();
            point(p.x + px(0.5), p.y + right.layout.line_height() / 2.)
        });
        cx.simulate_click(link, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com"));
        let (start, end) = cx.update(|_, cx| {
            let registry = h.view.read(cx).registry.borrow();
            let a = registry.find(ItemId(1), 1).unwrap();
            let b = registry.find(ItemId(1), 2).unwrap();
            let mut start = a.position_for_index(0).unwrap();
            let mut end = b.position_for_index(blocks[2].text.len()).unwrap();
            start.x += px(0.5);
            start.y += a.layout.line_height() / 2.;
            end.y += b.layout.line_height() / 2.;
            assert!(
                start.y >= px(0.) && end.y >= px(0.),
                "drag endpoints must be visible: {start:?}, {end:?}"
            );
            (start, end)
        });
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::none());
        cx.simulate_mouse_move(end, gpui::MouseButton::Left, gpui::Modifiers::none());
        cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::none());
        frame(&h, &mut cx);
        cx.update(|_, cx| {
            assert_eq!(
                h.view.read(cx).selection_text(cx).as_deref(),
                Some("Subagent\tBackground job")
            )
        });
        cx.update(|_, cx| h.view.update(cx, |v, cx| v.select_all(cx)));
        frame(&h, &mut cx);
        cx.update(|_, cx| {
            let copy = h.view.read(cx).selection_text(cx).unwrap();
            assert!(copy.starts_with("\tSubagent\tBackground job\nWhat it is\t"));
            assert!(copy.ends_with("\té 👨‍👩‍👧‍👦\tdocs and code"));
        });
    }
}

#[gpui::test]
fn wide_tables_scroll_horizontally_and_hidden_cells_are_not_hit_targets(cx: &mut TestAppContext) {
    let (h, mut cx) = table_window(cx, 340.);
    let source = format!(
        "{}\n{}\n{}",
        (0..8)
            .map(|n| format!("h{n}"))
            .collect::<Vec<_>>()
            .join(" | "),
        ["---"; 8].join(" | "),
        (0..8)
            .map(|n| format!("v{n}"))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let mut message = item(1);
    message.kind = ItemKind::Assistant {
        text: source,
        streaming: false,
    };
    open(&h.model, 1, vec![message], false, &mut cx);
    frame(&h, &mut cx);
    let before = anchor(&h, 1, &mut cx);
    let position = cx.update(|_, cx| {
        let registry = h.view.read(cx).registry.borrow();
        let first = registry.find(ItemId(1), 0).unwrap();
        let last = registry.find(ItemId(1), 7).unwrap();
        assert!(first.visible_bounds.size.width > px(0.));
        assert_eq!(last.visible_bounds.size.width, px(0.));
        assert_ne!(
            registry.hit_test(last.bounds.center()).map(|h| h.1),
            Some(7)
        );
        first.visible_bounds.center()
    });
    cx.simulate_event(gpui::ScrollWheelEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(point(px(-800.), px(0.))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    frame(&h, &mut cx);
    assert_eq!(anchor(&h, 1, &mut cx), before);
    cx.update(|_, cx| {
        let registry = h.view.read(cx).registry.borrow();
        assert_eq!(
            registry
                .find(ItemId(1), 0)
                .unwrap()
                .visible_bounds
                .size
                .width,
            px(0.)
        );
        let last = registry.find(ItemId(1), 7).unwrap();
        assert!(last.visible_bounds.size.width > px(0.));
        assert_eq!(
            registry.hit_test(last.visible_bounds.center()).map(|h| h.1),
            Some(7)
        );
    });
    cx.update(|_, cx| h.view.update(cx, |v, cx| v.select_all(cx)));
    cx.update(|_, cx| {
        assert_eq!(
            h.view.read(cx).selection_text(cx).as_deref(),
            Some("h0\th1\th2\th3\th4\th5\th6\th7\nv0\tv1\tv2\tv3\tv4\tv5\tv6\tv7")
        )
    });
}

#[gpui::test]
fn snapshot_stream_never_paints_a_blank_table_between_revisions(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 100);
    let header = "| Feature | Subagent | Background job |\n|:---|:---:|---:|\n";
    let rest = "| Type | AI worker | Command/process |\n| Input | Objective and tools | Command to execute |\n| Behavior | Reasons and uses tools | Runs until completion |\n| Output | Findings and summary | Logs and exit status |\n| Example | **Review this code** | `cargo test` |\n| Context | Own conversation context | Process environment |";
    let snapshot = |text: String| {
        let mut entries = items(0..100);
        let mut tail = item(100);
        tail.kind = ItemKind::Assistant {
            text,
            streaming: true,
        };
        entries.push(tail);
        entries
    };
    sync_snapshot(&h, snapshot(header.into()), cx);
    let mut previous_top = cx.update(|_, cx| {
        h.view
            .read(cx)
            .registry
            .borrow()
            .find(ItemId(100), 0)
            .unwrap()
            .bounds
            .top()
    });
    let initial_widths = cx.update(|_, cx| {
        let mut registry = h.view.read(cx).registry.borrow_mut();
        registry.recorded_frames = Some(Vec::new());
        (0..3)
            .map(|bi| registry.find(ItemId(100), bi).unwrap().bounds.size.width)
            .collect::<Vec<_>>()
    });
    for end in 1..=rest.len() {
        sync_snapshot(&h, snapshot(format!("{header}{}", &rest[..end])), cx);
        cx.update(|_, cx| {
            let mut registry = h.view.read(cx).registry.borrow_mut();
            let frames = registry.recorded_frames.as_mut().unwrap();
            assert!(!frames.is_empty(), "no paint at prefix {end}");
            for (frame, hits) in frames.drain(..).enumerate() {
                for bi in 0..3 {
                    let header = hits.iter().find(|hit| hit.item == ItemId(100) && hit.block == bi)
                        .unwrap_or_else(|| panic!("header disappeared at prefix {end}, frame {frame}; {} registered blocks", hits.len()));
                    assert_eq!(header.bounds.size.width, initial_widths[bi as usize], "column width changed at prefix {end}");
                    if bi == 0 {
                        assert!((header.bounds.top() - previous_top).abs() <= cx.global::<UiTheme>().body_line_height() * 3., "single-character update jumped at prefix {end}: {previous_top:?} -> {:?}", header.bounds.top());
                        previous_top = header.bounds.top();
                    }
                }
            }
        });
        assert!(stats(&h, cx).following);
    }
}

#[gpui::test]
fn snapshot_stream_keeps_in_table_reading_position_and_selection(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 100);
    let mut entries = items(0..100);
    let mut message = item(100);
    let mut source = TABLE_EXAMPLE.to_owned();
    source.push_str(&"\n| settled | row | content |".repeat(30));
    message.kind = ItemKind::Assistant {
        text: source.clone(),
        streaming: true,
    };
    entries.push(message);
    sync_snapshot(&h, entries.clone(), cx);
    let list = list_of(&h, 1, cx);
    list.set_follow_mode(gpui::FollowMode::Normal);
    list.scroll_to(ListOffset {
        item_ix: 101,
        offset_in_item: px(40.),
    });
    frame(&h, cx);
    let before = anchor(&h, 1, cx);
    let top = cx.update(|_, cx| {
        h.view.update(cx, |v, _| {
            v.convs.get_mut(&ConversationId(1)).unwrap().selection = Some(Selection {
                anchor: DocPos {
                    item: ItemId(100),
                    block: 1,
                    offset: 0,
                },
                head: DocPos {
                    item: ItemId(100),
                    block: 2,
                    offset: "Background job".len(),
                },
            });
            let mut registry = v.registry.borrow_mut();
            registry.recorded_frames = Some(Vec::new());
            registry.find(ItemId(100), 0).unwrap().bounds.top()
        })
    });
    for n in 0..12 {
        source.push_str("\n| more | streamed | content |");
        entries.last_mut().unwrap().kind = ItemKind::Assistant {
            text: source.clone(),
            streaming: n < 11,
        };
        sync_snapshot(&h, entries.clone(), cx);
        assert_eq!(anchor(&h, 1, cx), before);
        assert!(!stats(&h, cx).following);
        cx.update(|_, cx| {
            let view = h.view.read(cx);
            assert_eq!(
                view.selection_text(cx).as_deref(),
                Some("Subagent\tBackground job")
            );
            for hits in view
                .registry
                .borrow_mut()
                .recorded_frames
                .as_mut()
                .unwrap()
                .drain(..)
            {
                let header = hits
                    .iter()
                    .find(|h| h.item == ItemId(100) && h.block == 0)
                    .expect("table stays painted");
                assert_eq!(
                    header.bounds.top(),
                    top,
                    "reading position moved within a snapshot frame"
                );
            }
        });
    }
    entries.pop();
    sync_snapshot(&h, entries, cx);
    cx.update(|_, cx| {
        let cv = &h.view.read(cx).convs[&ConversationId(1)];
        assert!(cv.selection.is_none());
        assert!(!cv.doc.has_item(ItemId(100)));
    });
}

#[gpui::test]
fn deferred_table_cache_miss_keeps_old_cells_until_ready_but_not_across_generations(
    cx: &mut TestAppContext,
) {
    let (h, cx) = setup(cx, 0);
    let mut message = item(1);
    message.kind = ItemKind::Assistant {
        text: TABLE_EXAMPLE.into(),
        streaming: true,
    };
    open(&h.model, 1, vec![message], false, cx);
    frame(&h, cx);
    cx.update(|_, cx| {
        h.view.read(cx).registry.borrow_mut().recorded_frames = Some(Vec::new());
        h.model.update(cx, |m, cx| {
            // A missed structural/content note exercises the render-time deferred path.
            if let ItemKind::Assistant { text, .. } = &mut m.state.conversations[0].items[0].kind {
                text.push_str("\n| extra | column | data |");
            }
            cx.notify();
        });
    });
    frame(&h, cx);
    cx.update(|_, cx| {
        let view = h.view.read(cx);
        let mut registry = view.registry.borrow_mut();
        let frames = registry.recorded_frames.as_mut().unwrap();
        assert!(
            frames.len() >= 2,
            "both deferred and refreshed paints were observed"
        );
        for hits in frames.drain(..) {
            assert!(hits.iter().any(|h| h.item == ItemId(1) && h.block == 0));
        }
        assert_eq!(
            registry
                .blocks
                .iter()
                .filter(|h| h.item == ItemId(1))
                .count(),
            18
        );
        drop(registry);
        h.view.update(cx, |v, cx| v.select_all(cx));
    });
    cx.update(|_, cx| {
        h.view
            .read(cx)
            .registry
            .borrow_mut()
            .recorded_frames
            .as_mut()
            .unwrap()
            .clear();
        h.model.update(cx, |m, cx| {
            let c = &mut m.state.conversations[0];
            c.generation += 1;
            let mut replacement = item(1);
            replacement.kind = ItemKind::Assistant {
                text: "New attachment content".into(),
                streaming: false,
            };
            let event = BackendEvent {
                conversation: c.id,
                generation: c.generation,
                op: None,
                kind: EventKind::Synced {
                    items: vec![replacement],
                },
            };
            m.apply_event(event, cx);
        });
    });
    frame(&h, cx);
    cx.update(|_, cx| {
        let view = h.view.read(cx);
        assert!(view.convs[&ConversationId(1)].selection.is_none());
        for hits in view
            .registry
            .borrow_mut()
            .recorded_frames
            .as_mut()
            .unwrap()
            .drain(..)
        {
            assert!(
                hits.iter()
                    .filter(|h| h.item == ItemId(1))
                    .all(|h| h.layout.text() == "New attachment content")
            );
        }
    });
}

#[gpui::test]
fn streamed_table_changes_from_prose_to_cells_and_keeps_tail_and_selection(
    cx: &mut TestAppContext,
) {
    let (h, cx) = setup(cx, 0);
    append(&h, "| A | B |\n", cx);
    frame(&h, cx);
    let id = cx.update(|_, cx| {
        h.model
            .read(cx)
            .state
            .current()
            .unwrap()
            .items
            .last()
            .unwrap()
            .id
    });
    cx.update(|_, cx| {
        assert_eq!(
            h.view
                .read(cx)
                .registry
                .borrow()
                .blocks
                .iter()
                .filter(|b| b.item == id)
                .count(),
            1
        )
    });
    append(&h, "|---|---:|\n| é | partial", cx);
    frame(&h, cx);
    cx.update(|_, cx| {
        assert_eq!(
            h.view
                .read(cx)
                .registry
                .borrow()
                .blocks
                .iter()
                .filter(|b| b.item == id)
                .count(),
            4
        );
        h.view.update(cx, |v, _| {
            v.convs.get_mut(&ConversationId(1)).unwrap().selection = Some(Selection {
                anchor: DocPos {
                    item: id,
                    block: 2,
                    offset: 0,
                },
                head: DocPos {
                    item: id,
                    block: 3,
                    offset: 7,
                },
            })
        });
    });
    append(&h, " result |\n| next | row |", cx);
    frame(&h, cx);
    assert!(stats(&h, cx).following);
    cx.update(|_, cx| {
        let view = h.view.read(cx);
        assert_eq!(view.selection_text(cx).as_deref(), Some("é\tpartial"));
        assert_eq!(
            view.registry
                .borrow()
                .blocks
                .iter()
                .filter(|b| b.item == id)
                .count(),
            6
        );
        assert_eq!(view.convs[&ConversationId(1)].doc.cached_blocks(), 1);
    });
    let parses = cx.update(|_, cx| h.view.read(cx).convs[&ConversationId(1)].doc.parse_count());
    frame(&h, cx);
    cx.update(|_, cx| {
        assert_eq!(
            h.view.read(cx).convs[&ConversationId(1)].doc.parse_count(),
            parses
        )
    });
}

#[gpui::test]
fn streaming_table_does_not_pull_reader_away_from_an_earlier_anchor(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, 100);
    list_of(&h, 1, cx).scroll_to(ListOffset {
        item_ix: 25,
        offset_in_item: px(8.),
    });
    frame(&h, cx);
    let before = anchor(&h, 1, cx);
    append(&h, TABLE_EXAMPLE, cx);
    frame(&h, cx);
    assert_eq!(anchor(&h, 1, cx), before);
    assert!(!stats(&h, cx).following);
    append(&h, "\n| another | streamed | row |", cx);
    frame(&h, cx);
    assert_eq!(anchor(&h, 1, cx), before);
    assert!(stats(&h, cx).mounted_rows < 50);
}
