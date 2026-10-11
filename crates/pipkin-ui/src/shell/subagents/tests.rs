use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{Entity, TestAppContext, VisualTestContext};
use pipkin_core::*;

use super::*;
use crate::model::Model;
use crate::shell::actions::ShowSubagents;

fn task(id: u64, text: &str, status: &str) -> SubagentInfo {
    SubagentInfo {
        id,
        task_id: id,
        call_id: format!("call-{id}"),
        task: text.into(),
        status: status.into(),
    }
}

fn st(children: &[SubagentInfo], selected: Option<u64>, history: usize) -> SubagentState {
    SubagentState {
        enabled: Some(true),
        children: children.to_vec(),
        selected,
        history,
        ..SubagentState::default()
    }
}

fn paged(children: &[SubagentInfo], history: usize) -> SubagentState {
    SubagentState {
        paged: true,
        ..st(children, None, history)
    }
}

#[test]
fn labels_are_readable_unicode_safe_and_do_not_claim_engine_names() {
    assert_eq!(
        task_title("You are Counter A. Count to 20 slowly.", 2),
        "Counter A"
    );
    assert_eq!(
        task_title("Review the Rust tests. Do not edit.", 3),
        "Review the Rust tests"
    );
    assert_eq!(task_title(&"🦀".repeat(57), 4), "Subagent 4");
    assert_eq!(task_title("", 5), "Subagent 5");
    let row = task_row(&task(
        6,
        "You are Counter A. Count to 20 slowly.",
        "running",
    ));
    assert_eq!(row.summary, "Count to 20 slowly.");
    assert_eq!(row.task, "You are Counter A. Count to 20 slowly.");
    assert!(
        task_row(&task(7, "Review Rust tests.", "done"))
            .summary
            .is_empty()
    );
    let row = task_row(&task(8, &"🦀".repeat(160), "waiting"));
    assert_eq!(row.summary.chars().count(), 121);
}

#[test]
fn every_engine_status_is_distinct_and_unknown_does_not_imply_success() {
    for (raw, label, active) in [
        ("pending", "Pending", true),
        ("running", "Running", true),
        ("waiting", "Waiting", true),
        ("completing", "Completing", true),
        ("done", "Completed", false),
        ("failed", "Failed", false),
        ("aborted", "Stopped", false),
        ("unexpected", "Unknown", false),
    ] {
        let status = TaskStatus::from_engine(raw);
        assert_eq!(status.label(), label);
        assert_eq!(status.active(), active);
        assert!(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/icons")
                .join(format!("{}.svg", status.icon()))
                .is_file()
        );
    }
    assert!(TaskStatus::from_engine("completing").active());
    assert!(!TaskStatus::from_engine("done").active());
    assert_eq!(TaskStatus::from_engine("anything").label(), "Unknown");
}

#[test]
fn active_tasks_survive_bounds_and_counts_include_undisplayed_work() {
    let key = Some((ConversationId(1), 2));
    let mut children = (1..=110)
        .map(|id| task(id, "Finished.", "done"))
        .collect::<Vec<_>>();
    children.push(task(111, "Running.", "running"));
    children.push(task(112, "Failed.", "failed"));
    let mut rows = SubagentRows::default();
    rows.refresh(key, Some(&st(&children, Some(110), 0)));
    assert_eq!(
        (rows.total, rows.active, rows.completed, rows.attention),
        (112, 1, 110, 1)
    );
    assert_eq!(rows.selected_row.as_ref().unwrap().id, 110);
    assert_eq!(rows.rows[0].id, 111);
    assert_eq!(rows.rows.len(), 1);
    assert!(rows.finished_rows.is_empty(), "history starts collapsed");
    assert_eq!(rows.visible_rows().count(), 1);
    children.extend((201..321).map(|id| task(id, "Current work.", "running")));
    // Unpaged (Pi): 100 current rows and at most 100 finished rows, with a bound notice.
    rows.refresh(key, Some(&st(&children, Some(110), 100)));
    assert_eq!(rows.active, 121);
    assert_eq!(rows.rows.len(), 100);
    assert_eq!(rows.finished_rows.len(), 100);
    assert_eq!(rows.finished_rows[0].id, 112);
    assert_eq!(rows.visible_rows().count(), 200);
    assert!(
        rows.entries
            .iter()
            .any(|e| matches!(e, DirEntry::Notice(n) if n.starts_with("Showing up to 100")))
    );
    assert!(
        !rows
            .entries
            .iter()
            .any(|e| matches!(e, DirEntry::ShowMore { .. }))
    );
    rows.refresh(Some((ConversationId(2), 1)), Some(&st(&[], None, 0)));
    assert!(rows.rows.is_empty());
    assert_eq!(rows.active, 0);
}

#[test]
fn finished_tasks_leave_current_work_without_discarding_selected_detail() {
    let key = Some((ConversationId(1), 2));
    let mut children = vec![
        task(1, "Count slowly.", "running"),
        task(2, "Old result.", "done"),
    ];
    let mut rows = SubagentRows::default();
    rows.refresh(key, Some(&st(&children, Some(1), 0)));
    assert_eq!(rows.tab_label(), "Subagents 1");
    assert_eq!(
        rows.visible_rows().map(|r| r.id).collect::<Vec<_>>(),
        vec![1]
    );
    children[0].status = "done".into();
    rows.refresh(key, Some(&st(&children, Some(1), 0)));
    assert!(rows.visible_rows().next().is_none());
    assert_eq!(rows.tab_label(), "Subagents");
    assert_eq!(rows.selected_row.as_ref().unwrap().id, 1);
    assert_eq!(rows.selected_row.as_ref().unwrap().status, TaskStatus::Done);
    rows.refresh(key, Some(&st(&children, Some(1), 100)));
    assert_eq!(
        rows.visible_rows().map(|r| r.id).collect::<Vec<_>>(),
        vec![2, 1]
    );
    assert_eq!(
        children.len(),
        2,
        "presentation must not delete engine history"
    );
}

#[test]
fn stopped_failed_and_unknown_are_not_mistaken_for_running_work() {
    let children = vec![
        task(1, "Unknown.", "new-status"),
        task(2, "Stopped.", "aborted"),
        task(3, "Failed.", "failed"),
    ];
    let mut rows = SubagentRows::default();
    rows.refresh(Some((ConversationId(1), 1)), Some(&st(&children, None, 0)));
    assert_eq!(
        rows.visible_rows().map(|r| r.id).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(rows.finished_count(), 2);
    assert_eq!(rows.attention, 2);
    assert_eq!(rows.active, 0);
}

#[test]
fn loading_live_snapshot_and_disconnection_are_not_conflated() {
    assert_eq!(
        view_state(true, false, false, false, false),
        "Loading activity…"
    );
    assert_eq!(view_state(false, true, false, true, true), "Live activity");
    assert_eq!(view_state(false, false, false, true, true), "Snapshot");
    assert_eq!(
        view_state(false, false, true, false, false),
        "Activity unavailable"
    );
    assert_eq!(
        view_state(false, true, true, true, true),
        "Disconnected · retained preview"
    );
}

#[test]
fn paged_history_reveals_pages_in_stable_order_without_duplicates() {
    let key = Some((ConversationId(1), 1));
    let children: Vec<_> = (1..=1000)
        .map(|id| task(id, "Work.", if id % 10 == 0 { "running" } else { "done" }))
        .collect();
    let mut rows = SubagentRows::default();
    let mut seen = Vec::new();
    for history in [0, 100, 200, 900] {
        rows.refresh(key, Some(&paged(&children, history)));
        let ids: Vec<u64> = rows.visible_rows().map(|r| r.id).collect();
        assert_eq!(ids.len(), 100 + history, "all current work plus {history}");
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        // Earlier pages are a prefix of later ones: order is stable.
        assert!(ids.starts_with(&seen));
        seen = ids;
        let more = rows
            .entries
            .iter()
            .any(|e| matches!(e, DirEntry::ShowMore { .. }));
        assert_eq!(more, history > 0 && history < 900);
        assert_eq!(rows.directory.item_count(), rows.entries.len());
    }
    // Newest finished first, every task reachable.
    assert_eq!(seen[100], 999);
    assert_eq!(*seen.last().unwrap(), 1);
    assert_eq!(seen.iter().collect::<HashSet<_>>().len(), 1000);
    // A truncated engine directory says so instead of implying it is complete.
    rows.refresh(
        key,
        Some(&SubagentState {
            truncated: true,
            ..paged(&children, 900)
        }),
    );
    assert!(
        rows.entries
            .iter()
            .any(|e| matches!(e, DirEntry::Notice(n) if n.contains("size limit")))
    );
}

#[test]
fn list_sync_keeps_unchanged_rows_and_anchors_prepends() {
    let list = ListState::new(4, ListAlignment::Top, px(100.));
    let old = [(1, 1), (2, 1), (3, 1)];
    // Header row at the top while three rows are prepended: the first read row stays on top.
    let new = [(7, 1), (8, 1), (9, 1), (1, 1), (2, 1), (3, 1)];
    sync_list(&list, &old, &new, 1);
    assert_eq!(list.item_count(), 7);
    assert_eq!(list.logical_scroll_top().item_ix, 4);
    // Reading row 2 (index 5) while the tail changes: the position is untouched.
    list.scroll_to(ListOffset {
        item_ix: 5,
        offset_in_item: px(3.),
    });
    let tail = [(7, 1), (8, 1), (9, 1), (1, 1), (2, 1), (3, 2), (4, 1)];
    sync_list(&list, &new, &tail, 1);
    assert_eq!(list.item_count(), 8);
    let top = list.logical_scroll_top();
    assert_eq!((top.item_ix, top.offset_in_item), (5, px(3.)));
}

// ------------------------------------------------------------------- rendered (GPUI) checks

struct Ui {
    root: Entity<Workspace>,
}

fn boot() -> AppState {
    let mut state = AppState::new(
        Bootstrap {
            projects: vec![Project {
                id: ProjectId(1),
                name: "test".into(),
                path: "/synthetic-project".into(),
            }],
            models: vec![],
            conversations: vec![(ConversationId(1), ProjectId(1), "one".into(), 1)],
            now: 0,
        },
        Prefs::default(),
    );
    state.mode = Mode::Real;
    state.set_connection(Connection::Ready);
    state.dispatch(Command::SelectConversation(ConversationId(1)));
    state
}

fn open(cx: &mut TestAppContext) -> (Ui, &mut VisualTestContext) {
    cx.update(crate::text::init);
    let window = cx.add_window(|window, cx| {
        let model = cx.new(|_| Model::new(boot()));
        Workspace::new(model, PathBuf::new(), window, cx)
    });
    let root = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    let ui = Ui { root };
    event(
        &ui,
        cx,
        EventKind::Opened {
            items: vec![],
            has_older: false,
            changes: vec![],
        },
    );
    cx.update(|window, cx| window.dispatch_action(Box::new(ShowSubagents), cx));
    frame(cx);
    (ui, cx)
}

fn event(ui: &Ui, cx: &mut VisualTestContext, kind: EventKind) {
    ui.root.update(cx, |w, cx| {
        let model = w.model.clone();
        model.update(cx, |m, cx| {
            let c = m.state.current().unwrap();
            let ev = BackendEvent {
                conversation: c.id,
                generation: c.generation,
                op: None,
                kind,
            };
            m.apply_event(ev, cx);
        });
    });
    cx.run_until_parked();
}

fn dispatch(ui: &Ui, cx: &mut VisualTestContext, command: Command) {
    ui.root.update(cx, |w, cx| w.dispatch(command, cx));
    cx.run_until_parked();
}

fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn subagents<R>(ui: &Ui, cx: &mut VisualTestContext, f: impl FnOnce(&SubagentState) -> R) -> R {
    ui.root.read_with(cx, |w, cx| {
        f(&w.model.read(cx).state.current().unwrap().subagents)
    })
}

fn entry(id: u64) -> TranscriptItem {
    TranscriptItem {
        id: ItemId(id),
        at: 0,
        kind: ItemKind::Assistant {
            text: format!("Child entry {id}. ") + &"Long wrapped words. ".repeat((id % 7) as usize),
            streaming: false,
        },
    }
}

#[gpui::test]
fn a_thousand_tasks_mount_a_bounded_window_and_every_task_is_reachable(cx: &mut TestAppContext) {
    let (ui, cx) = open(cx);
    let children: Vec<_> = (1..=1000)
        .map(|id| {
            task(
                id,
                &format!("Task number {id}. Inspect module {id}."),
                if id % 10 == 0 { "running" } else { "done" },
            )
        })
        .collect();
    event(
        &ui,
        cx,
        EventKind::SubagentDirectory {
            enabled: true,
            children,
            truncated: false,
        },
    );
    frame(cx);
    let mounted = ui.root.read_with(cx, |w, _| w.subagent_rows.mounted.get());
    assert!(mounted > 0, "the directory rendered nothing");
    assert!(mounted < 60, "mounted {mounted} directory rows");
    // History: open it, then page until every finished task is listed.
    dispatch(&ui, cx, Command::ToggleSubagentHistory);
    let mut pages = 1;
    while subagents(&ui, cx, |s| s.can_show_more_history()) {
        dispatch(&ui, cx, Command::ShowMoreSubagentHistory);
        pages += 1;
    }
    assert_eq!(pages, 9);
    let (count, unique) = ui.root.read_with(cx, |w, _| {
        let ids: Vec<u64> = w.subagent_rows.visible_rows().map(|r| r.id).collect();
        (ids.len(), ids.iter().collect::<HashSet<_>>().len())
    });
    assert_eq!((count, unique), (1000, 1000));
    // Scroll to the very end: the oldest finished task is mounted, still a bounded window.
    let list = ui
        .root
        .read_with(cx, |w, _| w.subagent_rows.directory.clone());
    list.scroll_to(ListOffset {
        item_ix: list.item_count() - 1,
        offset_in_item: px(0.),
    });
    frame(cx);
    let (mounted, tasks) = ui.root.read_with(cx, |w, _| {
        (
            w.subagent_rows.mounted.get(),
            w.subagent_rows.mounted_tasks.borrow().clone(),
        )
    });
    assert!(tasks.contains(&1), "oldest task not reachable: {tasks:?}");
    assert!(mounted < 60, "mounted {mounted} rows at the end");
    // A selected finished task stays readable however far down it was.
    dispatch(&ui, cx, Command::SelectSubagent(Some(1)));
    let selected = ui.root.read_with(cx, |w, _| {
        w.subagent_rows.selected_row.as_ref().map(|r| r.id)
    });
    assert_eq!(selected, Some(1));
}

#[gpui::test]
fn five_thousand_child_entries_page_in_above_the_reading_position(cx: &mut TestAppContext) {
    let (ui, cx) = open(cx);
    event(
        &ui,
        cx,
        EventKind::SubagentDirectory {
            enabled: true,
            children: vec![task(10, "Long task.", "running")],
            truncated: false,
        },
    );
    dispatch(&ui, cx, Command::SelectSubagent(Some(10)));
    event(
        &ui,
        cx,
        EventKind::SubagentPage {
            child: 10,
            items: (4800..5000).map(entry).collect(),
            live: true,
            has_older: true,
        },
    );
    frame(cx);
    let list = ui
        .root
        .read_with(cx, |w, _| w.subagent_rows.activity.clone());
    assert_eq!(list.item_count(), 201);
    let mut max_mounted = 0;
    let mut pages = 0;
    while subagents(&ui, cx, |s| s.can_load_older()) {
        // Read from the header (where the control is) or from somewhere in the middle.
        let reading = if pages % 2 == 0 { 0 } else { 3 };
        list.scroll_to(ListOffset {
            item_ix: reading,
            offset_in_item: px(0.),
        });
        frame(cx);
        let anchored = ui.root.read_with(cx, |w, cx| {
            let s = &w.model.read(cx).state.current().unwrap().subagents;
            s.previews[reading.max(1) - 1].id
        });
        dispatch(&ui, cx, Command::LoadOlderSubagent);
        let before = subagents(&ui, cx, |s| s.loading_older).expect("a page is in flight");
        let start = before.0.saturating_sub(200);
        event(
            &ui,
            cx,
            EventKind::SubagentOlderPage {
                child: 10,
                before,
                items: (start..before.0).map(entry).collect(),
                has_older: start > 0,
            },
        );
        frame(cx);
        pages += 1;
        // The entry that was being read is still the one at the top.
        let top = list.logical_scroll_top().item_ix;
        let at_top = ui.root.read_with(cx, |w, cx| {
            let s = &w.model.read(cx).state.current().unwrap().subagents;
            s.previews[top - 1].id
        });
        assert_eq!(at_top, anchored, "page {pages} moved the reading position");
        max_mounted = max_mounted.max(ui.root.read_with(cx, |w, _| w.subagent_rows.mounted.get()));
    }
    assert_eq!(pages, 24);
    assert_eq!(list.item_count(), 5001);
    let ids = subagents(&ui, cx, |s| {
        s.previews.iter().map(|p| p.id.0).collect::<Vec<_>>()
    });
    assert_eq!(ids, (0..5000).collect::<Vec<_>>());
    assert!(max_mounted > 0 && max_mounted < 80, "mounted {max_mounted}");
    // A live refresh of the newest page keeps everything paged in.
    event(
        &ui,
        cx,
        EventKind::SubagentPage {
            child: 10,
            items: (4810..5010).map(entry).collect(),
            live: true,
            has_older: true,
        },
    );
    assert_eq!(list.item_count(), 5011);
    // Returning to the directory and back starts the task over from its newest page.
    dispatch(&ui, cx, Command::SelectSubagent(None));
    dispatch(&ui, cx, Command::SelectSubagent(Some(10)));
    assert_eq!(list.item_count(), 1);
}
