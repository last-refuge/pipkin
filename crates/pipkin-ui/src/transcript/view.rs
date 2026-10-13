//! `TranscriptView`: the virtualized, selectable conversation transcript.
//!
//! Layout decisions worth knowing:
//! * One `gpui::list` item per transcript item, plus item 0 which is a fixed-height header row
//!   ("Loading earlier messages…" / "Beginning of conversation"). Fixed height means history
//!   loading never shifts the viewport by itself.
//! * Each conversation keeps its own `ListState`, expansion set, selection and block cache, so
//!   switching away and back restores the scroll anchor.
//! * Code blocks **wrap** rather than scroll horizontally: wrapped text keeps hit testing and
//!   selection geometry exact, and long unbroken tokens still break at the column edge.
//! * No animation anywhere. Streaming and running states are static indicators.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, DispatchPhase, Element, ElementId, Entity,
    EventEmitter, FocusHandle, Focusable, FontWeight, GlobalElementId, InspectorElementId,
    InteractiveText, IntoElement, KeyBinding, LayoutId, ListAlignment, ListOffset, ListScrollEvent,
    ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Render, Role, SharedString, Styled, StyledText, Subscription, WeakEntity, Window, actions, div,
    list, point, prelude::*, px, svg,
};
use pipkin_core::{
    Attachment, Command, ConversationId, Delivery, ItemId, ItemKind, Note, NoticeLevel, RunState,
    ToolCall, ToolStatus, TranscriptItem,
};

use super::blocks::{BaseText, BlockText, FrameStats, SharedRegistry, build_runs};
use super::document::{DocPos, Document, Selection, Step, TOOL_INPUT, TOOL_OUTPUT, word_range};
use super::markdown::{Block, BlockKind};
use crate::model::Model;
use crate::theme::{ActiveTheme, Theme};

actions!(
    transcript,
    [
        SelectAllText,
        CopySelection,
        ExtendLeft,
        ExtendRight,
        ExtendUp,
        ExtendDown,
        ExtendToStart,
        ExtendToEnd,
        ClearSelection,
    ]
);

const CONTEXT: &str = "Transcript";
const MAX_CONVERSATION_VIEWS: usize = 24;
const OVERDRAW: f32 = 900.0;
const A11Y_TEXT_LIMIT: usize = 2000;

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-a", SelectAllText, Some(CONTEXT)),
        KeyBinding::new("ctrl-c", CopySelection, Some(CONTEXT)),
        KeyBinding::new("shift-left", ExtendLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", ExtendRight, Some(CONTEXT)),
        KeyBinding::new("shift-up", ExtendUp, Some(CONTEXT)),
        KeyBinding::new("shift-down", ExtendDown, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-home", ExtendToStart, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-end", ExtendToEnd, Some(CONTEXT)),
        KeyBinding::new("escape", ClearSelection, Some(CONTEXT)),
    ]);
}

/// Numbers for the perf scorecard.
#[derive(Clone, Debug, Default)]
pub struct TranscriptStats {
    /// Rows built in the most recent frame (bounded by viewport + overdraw).
    pub mounted_rows: usize,
    pub cached_blocks: usize,
    /// Bytes of selected text (0 when there is no selection).
    pub selection_len: usize,
    pub following: bool,
    pub frames: usize,
    pub frame_p50: Option<Duration>,
    pub frame_p95: Option<Duration>,
}

struct ConvView {
    list: ListState,
    /// Message identities for restoring the reading anchor after an engine snapshot.
    item_ids: Vec<ItemId>,
    doc: Document,
    expanded: HashSet<ItemId>,
    /// Runs of tool steps shown in full, by the id of the run's first step. A run not in here
    /// shows only its latest step.
    steps_open: HashSet<ItemId>,
    selection: Option<Selection>,
    last_used: u64,
}

impl ConvView {
    fn reset_items(&mut self, item_ids: Vec<ItemId>) {
        let following = self.list.is_following_tail();
        let top = self.list.logical_scroll_top();
        let anchored_item = top
            .item_ix
            .checked_sub(1)
            .and_then(|i| self.item_ids.get(i));
        let item_ix = anchored_item
            .and_then(|id| item_ids.iter().position(|candidate| candidate == id))
            .map(|i| i + 1)
            .unwrap_or_else(|| top.item_ix.min(item_ids.len()));
        self.list.reset(item_ids.len() + 1);
        if following {
            self.list.scroll_to_end();
        } else {
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: top.offset_in_item,
            });
        }
        self.item_ids = item_ids;
    }
}

struct Drag {
    conv: ConversationId,
    pointer: gpui::Point<Pixels>,
}

pub struct TranscriptView {
    model: Entity<Model>,
    focus: FocusHandle,
    convs: HashMap<ConversationId, ConvView>,
    current: Option<ConversationId>,
    registry: SharedRegistry,
    frame_stats: Rc<RefCell<FrameStats>>,
    list_bounds: Rc<Cell<Bounds<Pixels>>>,
    mounted: Rc<Cell<usize>>,
    drag: Option<Drag>,
    autoscroll: f32,
    autoscroll_task: Option<gpui::Task<()>>,
    copied: Option<(ItemId, u32)>,
    last_following: bool,
    tick: u64,
    resolved: Option<super::document::Resolved>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<()> for TranscriptView {}

impl Focusable for TranscriptView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[derive(Clone)]
enum Click {
    Url(String),
    Change(usize),
}

/// What the row above the first message says: whether there is more to load, a saved copy, or a
/// failure to load it.
struct HeaderState {
    has_older: bool,
    loading: bool,
    opened: bool,
    saved_copy: bool,
    older_error: Option<String>,
}

struct RowCtx {
    theme: Theme,
    conv: ConversationId,
    change_paths: Vec<String>,
    /// Retained web sources as `(citation id, URL)`, so `[s<n>]` opens its page.
    sources: Vec<(String, String)>,
    expanded: bool,
    /// Set on the latest row of a run of tool steps, which carries the fold control.
    steps: Option<StepsLine>,
}

/// Work between a prompt and its reply that folds into one line: the tool steps and the model's
/// thinking. (Errors and other notices stay visible on their own.)
fn is_activity(kind: &ItemKind) -> bool {
    match kind {
        ItemKind::Tool(_) => true,
        ItemKind::Notice {
            level: NoticeLevel::Info,
            text,
        } => super::tools::is_thinking(text),
        _ => false,
    }
}

/// Whether a reply is the start of a new one, and so carries the Pipkin marker and label: it does
/// unless the item before it is also reply text (steps and thinking in between do not continue it).
fn starts_reply(previous: Option<&ItemKind>) -> bool {
    !matches!(previous, Some(ItemKind::Assistant { .. }))
}

/// The most characters of a step's target (a path or a command) shown in its card.
const TOOL_TARGET_CHARS: usize = 60;

/// `text` cut to `max` characters, with an ellipsis when it was cut.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max).collect();
        format!("{}\u{2026}", cut.trim_end())
    }
}

/// What a collapsed work run says instead of exposing commands and file paths.
fn activity_caption(items: &[TranscriptItem], active: bool, failed: bool, summary: &str) -> String {
    if !active {
        let outcome = if failed {
            "Finished with errors."
        } else {
            "Done."
        };
        return if summary.is_empty() {
            outcome.into()
        } else {
            format!("{summary} · {outcome}")
        };
    }
    items
        .iter()
        .rev()
        .filter_map(|item| match &item.kind {
            ItemKind::Notice {
                level: NoticeLevel::Info,
                text,
            } if super::tools::is_thinking(text) => {
                let first = super::tools::thinking_first_line(text);
                (!first.is_empty()).then_some(first)
            }
            _ => None,
        })
        .next()
        .unwrap_or_else(|| {
            if items
                .iter()
                .any(|item| matches!(item.kind, ItemKind::Tool(_)))
            {
                summary.to_owned()
            } else {
                "Working…".into()
            }
        })
}

/// The fold control of a run of two or more work items.
struct StepsLine {
    count: usize,
    open: bool,
    first: ItemId,
    /// "Read 2 files, ran 3 commands".
    summary: String,
    /// Latest human-readable thinking or observed tool counts while active; outcome once finished.
    caption: String,
    /// How the run stands: still going, any step failed, or all done.
    running: bool,
    failed: bool,
}

impl TranscriptView {
    pub fn new(model: Entity<Model>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&model, |this, _model, note: &Note, cx| {
            this.on_note(note, cx)
        });
        TranscriptView {
            model,
            focus: cx.focus_handle(),
            convs: HashMap::new(),
            current: None,
            registry: Rc::default(),
            frame_stats: Rc::default(),
            list_bounds: Rc::default(),
            mounted: Rc::default(),
            drag: None,
            autoscroll: 0.0,
            autoscroll_task: None,
            copied: None,
            last_following: true,
            tick: 0,
            resolved: None,
            _subscriptions: vec![sub],
        }
    }

    // ------------------------------------------------------------------ public API

    pub fn jump_to_latest(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cv) = self.current.and_then(|c| self.convs.get(&c)) {
            cv.list.set_follow_mode(gpui::FollowMode::Tail);
            cv.list.scroll_to_end();
        }
        cx.notify();
    }

    pub fn stats(&self, cx: &App) -> TranscriptStats {
        let fs = self.frame_stats.borrow();
        let cv = self.current.and_then(|c| self.convs.get(&c));
        let selection_len = self.selection_text(cx).map(|t| t.len()).unwrap_or(0);
        TranscriptStats {
            mounted_rows: self.mounted.get(),
            cached_blocks: cv.map(|c| c.doc.cached_blocks()).unwrap_or(0),
            selection_len,
            following: cv.map(|c| c.list.is_following_tail()).unwrap_or(true),
            frames: fs.len(),
            frame_p50: fs.percentile(0.5),
            frame_p95: fs.percentile(0.95),
        }
    }

    pub fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selection_text(cx).filter(|t| !t.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.current else { return };
        let model = self.model.clone();
        let Some(conv) = model.read(cx).state.conversation(id) else {
            return;
        };
        if let Some(cv) = self.convs.get_mut(&id) {
            cv.selection = cv.doc.select_all(&conv.items, &cv.expanded);
        }
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(cv) = self.current.and_then(|c| self.convs.get_mut(&c)) {
            cv.selection = None;
        }
        cx.notify();
    }

    pub fn has_selection(&self) -> bool {
        self.current
            .and_then(|c| self.convs.get(&c))
            .is_some_and(|cv| cv.selection.is_some_and(|s| s.anchor != s.head))
    }

    /// Expand/collapse a tool row programmatically (also used by clicks).
    pub fn toggle_tool(&mut self, conv: ConversationId, item: ItemId, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let ix = model
            .read(cx)
            .state
            .conversation(conv)
            .and_then(|c| Document::index_of(&c.items, item));
        if let Some(cv) = self.convs.get_mut(&conv) {
            if !cv.expanded.remove(&item) {
                cv.expanded.insert(item);
            }
            if let Some(ix) = ix {
                cv.list.remeasure_items(ix + 1..ix + 2);
            }
        }
        cx.notify();
    }

    /// Show or hide all the steps of a run of tool calls (named by its first step). The rows of the
    /// run change height, so they are measured again.
    pub fn toggle_steps(&mut self, conv: ConversationId, first: ItemId, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let range = model.read(cx).state.conversation(conv).and_then(|c| {
            let s = Document::index_of(&c.items, first)?;
            let is_tool = |i: usize| is_activity(&c.items[i].kind);
            let (_, last) = super::tools::tool_run(c.items.len(), s, is_tool)?;
            Some(s..last + 1)
        });
        if let Some(cv) = self.convs.get_mut(&conv) {
            if !cv.steps_open.remove(&first) {
                cv.steps_open.insert(first);
            }
            if let Some(r) = range {
                cv.list.remeasure_items(r.start + 1..r.end + 1);
            }
        }
        cx.notify();
    }

    pub fn is_expanded(&self, conv: ConversationId, item: ItemId) -> bool {
        self.convs
            .get(&conv)
            .is_some_and(|c| c.expanded.contains(&item))
    }

    /// Scroll position of a conversation as (list item index, pixel offset); item 0 is the header.
    pub fn scroll_anchor(&self, conv: ConversationId) -> Option<(usize, Pixels)> {
        self.convs.get(&conv).map(|c| {
            let o = c.list.logical_scroll_top();
            (o.item_ix, o.offset_in_item)
        })
    }

    pub fn list_state(&self, conv: ConversationId) -> Option<ListState> {
        self.convs.get(&conv).map(|c| c.list.clone())
    }

    pub fn selection(&self, conv: ConversationId) -> Option<Selection> {
        self.convs.get(&conv).and_then(|c| c.selection)
    }

    pub fn set_selection(
        &mut self,
        conv: ConversationId,
        sel: Option<Selection>,
        cx: &mut Context<Self>,
    ) {
        if let Some(cv) = self.convs.get_mut(&conv) {
            cv.selection = sel;
        }
        cx.notify();
    }

    pub fn selection_text(&self, cx: &App) -> Option<String> {
        let id = self.current?;
        let cv = self.convs.get(&id)?;
        let sel = cv.selection?;
        let conv = self.model.read(cx).state.conversation(id)?;
        Some(cv.doc.selected_text(&conv.items, &cv.expanded, &sel))
    }

    // --------------------------------------------------------------- list bookkeeping

    fn total_rows(&self, conv: ConversationId, cx: &App) -> usize {
        self.model
            .read(cx)
            .state
            .conversation(conv)
            .map_or(1, |c| c.items.len() + 1)
    }

    fn item_ids(&self, conv: ConversationId, cx: &App) -> Vec<ItemId> {
        self.model
            .read(cx)
            .state
            .conversation(conv)
            .map(|c| c.items.iter().map(|item| item.id).collect())
            .unwrap_or_default()
    }

    fn make_list(&mut self, conv: ConversationId, cx: &mut Context<Self>) -> ListState {
        let total = self.total_rows(conv, cx);
        let list = ListState::new(total, ListAlignment::Bottom, px(OVERDRAW));
        list.set_follow_mode(gpui::FollowMode::Tail);
        let weak: WeakEntity<Self> = cx.entity().downgrade();
        list.set_scroll_handler(move |event: &ListScrollEvent, _window, cx| {
            let following = event.is_following_tail;
            let weak = weak.clone();
            // Never update the view from inside layout: defer.
            cx.defer(move |cx| {
                weak.update(cx, |this, cx| {
                    if this.last_following != following {
                        this.last_following = following;
                        cx.notify();
                    }
                    this.maybe_load_older(cx);
                })
                .ok();
            });
        });
        list
    }

    fn ensure_conv(&mut self, conv: ConversationId, cx: &mut Context<Self>) {
        self.tick += 1;
        let total = self.total_rows(conv, cx);
        if !self.convs.contains_key(&conv) {
            if self.convs.len() >= MAX_CONVERSATION_VIEWS {
                let victim = self
                    .convs
                    .iter()
                    .filter(|(id, _)| Some(**id) != self.current)
                    .min_by_key(|(_, v)| v.last_used)
                    .map(|(id, _)| *id);
                if let Some(v) = victim {
                    self.convs.remove(&v);
                }
            }
            let list = self.make_list(conv, cx);
            let item_ids = self.item_ids(conv, cx);
            self.convs.insert(
                conv,
                ConvView {
                    list,
                    item_ids,
                    doc: Document::default(),
                    expanded: HashSet::new(),
                    steps_open: HashSet::new(),
                    selection: None,
                    last_used: self.tick,
                },
            );
        }
        let needs_reset = self.convs[&conv].list.item_count() != total;
        let item_ids = needs_reset.then(|| self.item_ids(conv, cx));
        let cv = self.convs.get_mut(&conv).unwrap();
        cv.last_used = self.tick;
        if let Some(item_ids) = item_ids {
            // A missed note must not take the reader back to the tail either.
            cv.reset_items(item_ids);
        }
    }

    fn on_note(&mut self, note: &Note, cx: &mut Context<Self>) {
        match note {
            Note::ItemsReset(c) => {
                let item_ids = self.item_ids(*c, cx);
                if let Some(cv) = self.convs.get_mut(c) {
                    cv.reset_items(item_ids);
                    cv.selection = None;
                    cv.doc = Document::default();
                }
            }
            Note::ItemsAppended(c, _) => {
                let item_ids = self.item_ids(*c, cx);
                let total = item_ids.len() + 1;
                if let Some(cv) = self.convs.get_mut(c) {
                    cv.item_ids = item_ids;
                    let old = cv.list.item_count();
                    if total > old {
                        cv.list.splice(old..old, total - old);
                        // The step before the new one may now be folded away, or carry the line.
                        if old >= 2 {
                            cv.list.remeasure_items(old - 1..old);
                        }
                    }
                }
            }
            Note::ItemChanged(c, i) => {
                if let Some(cv) = self.convs.get(c)
                    && i + 1 < cv.list.item_count()
                {
                    cv.list.remeasure_items(i + 1..i + 2);
                }
            }
            Note::ItemsPrepended(c, _) => {
                let item_ids = self.item_ids(*c, cx);
                let total = item_ids.len() + 1;
                if let Some(cv) = self.convs.get_mut(c) {
                    cv.item_ids = item_ids;
                    let top = cv.list.logical_scroll_top();
                    let old = cv.list.item_count();
                    if total > old {
                        let delta = total - old;
                        cv.list.splice(1..1, delta);
                        // Re-anchor on the same item at the same pixel offset.
                        let anchor = if top.item_ix >= 1 {
                            ListOffset {
                                item_ix: top.item_ix + delta,
                                offset_in_item: top.offset_in_item,
                            }
                        } else {
                            ListOffset {
                                item_ix: delta + 1,
                                offset_in_item: px(0.),
                            }
                        };
                        cv.list.scroll_to(anchor);
                    }
                }
            }
            _ => {}
        }
        self.apply_scroll_target(cx);
        cx.notify();
    }

    /// Scroll to the message a search hit pointed at, once the conversation shows it, and
    /// tell the state it has been shown.
    fn apply_scroll_target(&mut self, cx: &mut Context<Self>) {
        let Some((conv, item)) = self.model.read(cx).state.scroll_target else {
            return;
        };
        let Some(pos) = self
            .model
            .read(cx)
            .state
            .conversation(conv)
            .and_then(|c| c.items.iter().position(|i| i.id == item))
        else {
            return;
        };
        self.ensure_conv(conv, cx);
        if let Some(cv) = self.convs.get(&conv) {
            cv.list.set_follow_mode(gpui::FollowMode::Normal);
            // Row 0 is the header, so the message is at `pos + 1`.
            cv.list.scroll_to(ListOffset {
                item_ix: pos + 1,
                offset_in_item: px(0.),
            });
        }
        let model = self.model.clone();
        cx.defer(move |cx| {
            model.update(cx, |m, cx| m.dispatch(Command::ClearScrollTarget, cx));
        });
    }

    fn maybe_load_older(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.current else { return };
        let Some(cv) = self.convs.get(&id) else {
            return;
        };
        if cv.list.logical_scroll_top().item_ix > 2 {
            return;
        }
        let wants = self.model.read(cx).state.conversation(id).is_some_and(|c| {
            c.has_older && !c.loading_older && c.opened && c.older_error.is_none()
        });
        if wants {
            self.model
                .update(cx, |m, cx| m.dispatch(Command::LoadOlder, cx));
        }
    }

    // ------------------------------------------------------------------- selection

    fn current_selection_ctx(&self) -> Option<(ConversationId, Selection)> {
        let id = self.current?;
        Some((id, self.convs.get(&id)?.selection?))
    }

    fn pos_from_hit(hit: (ItemId, u32, usize)) -> DocPos {
        DocPos {
            item: hit.0,
            block: hit.1,
            offset: hit.2,
        }
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.current else { return };
        window.focus(&self.focus, cx);
        let hit = self.registry.borrow().hit_test(ev.position);
        let Some(hit) = hit else {
            if let Some(cv) = self.convs.get_mut(&id) {
                cv.selection = None;
            }
            cx.notify();
            return;
        };
        let pos = Self::pos_from_hit(hit);
        let model = self.model.clone();
        let Some(conv) = model.read(cx).state.conversation(id) else {
            return;
        };
        let Some(cv) = self.convs.get_mut(&id) else {
            return;
        };
        match ev.click_count {
            0 | 1 => {
                if ev.modifiers.shift {
                    if let Some(sel) = cv.selection.as_mut() {
                        sel.head = pos;
                    } else {
                        cv.selection = Some(Selection::caret(pos));
                    }
                } else {
                    cv.selection = Some(Selection::caret(pos));
                }
                self.drag = Some(Drag {
                    conv: id,
                    pointer: ev.position,
                });
            }
            n => {
                let item = conv.items.iter().find(|i| i.id == pos.item);
                let blocks = item.map(|i| cv.doc.blocks(i, cv.expanded.contains(&i.id)));
                if let Some(block) = blocks.as_ref().and_then(|b| b.get(pos.block as usize)) {
                    let range = if n == 2 {
                        word_range(&block.text, pos.offset)
                    } else {
                        0..block.text.len()
                    };
                    cv.selection = Some(Selection {
                        anchor: DocPos {
                            offset: range.start,
                            ..pos
                        },
                        head: DocPos {
                            offset: range.end,
                            ..pos
                        },
                    });
                }
                self.drag = None;
            }
        }
        cx.notify();
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            self.autoscroll = 0.0;
            return;
        }
        drag.pointer = ev.position;
        let conv = drag.conv;
        let bounds = self.list_bounds.get();
        self.autoscroll = if ev.position.y < bounds.top() {
            -(f32::from(bounds.top() - ev.position.y)).clamp(4.0, 48.0)
        } else if ev.position.y > bounds.bottom() {
            f32::from(ev.position.y - bounds.bottom()).clamp(4.0, 48.0)
        } else {
            0.0
        };
        self.extend_to_pointer(conv, ev.position, cx);
        if self.autoscroll != 0.0 {
            self.ensure_autoscroll(cx);
        }
    }

    fn extend_to_pointer(
        &mut self,
        conv: ConversationId,
        pointer: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let hit = self.registry.borrow().hit_test(pointer);
        if let (Some(hit), Some(cv)) = (hit, self.convs.get_mut(&conv))
            && let Some(sel) = cv.selection.as_mut()
        {
            sel.head = Self::pos_from_hit(hit);
        }
        cx.notify();
    }

    fn mouse_up(&mut self) {
        self.drag = None;
        self.autoscroll = 0.0;
    }

    fn ensure_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.autoscroll_task.is_some() {
            return;
        }
        self.autoscroll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let keep = this
                    .update(cx, |v, cx| {
                        let Some(drag) = v.drag.as_ref().filter(|_| v.autoscroll != 0.0) else {
                            v.autoscroll_task = None;
                            return false;
                        };
                        let (conv, pointer) = (drag.conv, drag.pointer);
                        if let Some(cv) = v.convs.get(&conv) {
                            cv.list.scroll_by(px(v.autoscroll));
                        }
                        v.extend_to_pointer(conv, pointer, cx);
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    fn extend(&mut self, step: Step, cx: &mut Context<Self>) {
        let Some((id, sel)) = self.current_selection_ctx() else {
            return;
        };
        let model = self.model.clone();
        let Some(conv) = model.read(cx).state.conversation(id) else {
            return;
        };
        let registry = self.registry.clone();
        let Some(cv) = self.convs.get_mut(&id) else {
            return;
        };
        let mut head = None;
        // Vertical movement follows visual lines when the head block is mounted.
        if matches!(step, Step::BlockUp | Step::BlockDown)
            && let Some(hit) = registry.borrow().find(sel.head.item, sel.head.block)
            && let Some(p) = hit.layout.position_for_index(sel.head.offset)
        {
            let lh = hit.layout.line_height();
            let y = if step == Step::BlockUp {
                p.y - lh
            } else {
                p.y + lh
            };
            let b = hit.bounds;
            if y >= b.top() && y < b.bottom() {
                let (Ok(i) | Err(i)) = hit.layout.index_for_position(point(p.x, y));
                head = Some(DocPos {
                    offset: i.min(hit.text_len),
                    ..sel.head
                });
            }
        }
        let head =
            head.unwrap_or_else(|| cv.doc.move_pos(&conv.items, &cv.expanded, sel.head, step));
        if let Some(s) = cv.selection.as_mut() {
            s.head = head;
        }
        cx.notify();
    }

    fn copy_block(&mut self, text: String, key: (ItemId, u32), cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(key);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |v, cx| {
                if v.copied == Some(key) {
                    v.copied = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

// ------------------------------------------------------------------------ rendering

fn icon(name: &str, size: f32, color: gpui::Hsla) -> gpui::Svg {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

fn format_time(at: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(dt) = Local.timestamp_opt(at, 0).single() else {
        return String::new();
    };
    if dt.date_naive() == Local::now().date_naive() {
        dt.format("%H:%M").to_string()
    } else {
        dt.format("%b %-d, %H:%M").to_string()
    }
}

fn clip(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn stop_mouse_down(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}

fn element_key(item: ItemId, block: u32) -> u64 {
    item.0.wrapping_mul(1_000_003).wrapping_add(block as u64)
}

impl TranscriptView {
    fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        self.mounted.set(self.mounted.get() + 1);
        let Some(conv_id) = self.current else {
            return div().into_any_element();
        };
        let theme = cx.theme().clone();
        let model = self.model.clone();
        let Some(conv) = model.read(cx).state.conversation(conv_id) else {
            return div().into_any_element();
        };
        if ix == 0 {
            let header = HeaderState {
                has_older: conv.has_older,
                loading: conv.loading_older,
                opened: conv.opened,
                saved_copy: conv.cached_at.is_some() && !conv.opened,
                older_error: conv.older_error.clone(),
            };
            return self.render_header_row(header, &theme, cx);
        }
        let item_ix = ix - 1;
        let Some(item) = conv.items.get(item_ix) else {
            return div().into_any_element();
        };
        let item = item.clone();
        let new_reply = starts_reply(item_ix.checked_sub(1).map(|p| &conv.items[p].kind));
        // Consecutive tool calls fold into one line, showing only the latest step; clicking it
        // shows them all, and they follow along live. The rows keep their places in the list
        // (so every index stays valid); the hidden ones simply have no height.
        let mut steps = None;
        if is_activity(&item.kind) {
            let is_tool = |i: usize| {
                is_activity(&conv.items[i].kind) && conv.items[i].native_only == item.native_only
            };
            if let Some((first, last)) = super::tools::tool_run(conv.items.len(), item_ix, is_tool)
                && last > first
            {
                let first_id = conv.items[first].id;
                let open = self
                    .convs
                    .get(&conv_id)
                    .is_some_and(|c| c.steps_open.contains(&first_id));
                if !open && item_ix != last {
                    return div().h(px(0.0)).into_any_element();
                }
                // The fold line sits on the latest step while folded, on the first when open.
                if item_ix == if open { first } else { last } {
                    let mut names = Vec::new();
                    let mut failed = false;
                    for step in &conv.items[first..=last] {
                        if let ItemKind::Tool(t) = &step.kind {
                            names.push(t.name.as_str());
                            failed |= t.status == ToolStatus::Failed;
                        }
                    }
                    // Only the newest stretch can still be running; a stale tool status in
                    // history must not leave a completed run saying "Working…" forever.
                    let running = conv.run.is_busy() && last + 1 == conv.items.len();
                    failed |=
                        matches!(conv.run, RunState::Failed { .. }) && last + 1 == conv.items.len();
                    let end = if running {
                        self.model.read(cx).state.now().max(conv.items[last].at)
                    } else {
                        conv.items[last].at
                    };
                    let mut summary = super::tools::run_summary(names.into_iter(), running);
                    if summary.is_empty() {
                        summary = if running { "Thinking" } else { "Thought" }.into();
                    }
                    if let Some(took) = super::tools::format_elapsed(end - conv.items[first].at) {
                        summary = format!("{summary} \u{b7} {took}");
                    }
                    let caption =
                        activity_caption(&conv.items[first..=last], running, failed, &summary);
                    steps = Some(StepsLine {
                        count: last - first + 1,
                        open,
                        first: first_id,
                        summary,
                        caption,
                        running,
                        failed,
                    });
                }
            }
        }
        let ctx = RowCtx {
            theme,
            conv: conv_id,
            change_paths: conv.changes.iter().map(|c| c.path.clone()).collect(),
            sources: conv
                .web_sources
                .iter()
                .map(|s| (s.id.clone(), s.url.clone()))
                .collect(),
            expanded: self.is_expanded(conv_id, item.id),
            steps,
        };
        let body = match &item.kind {
            ItemKind::User {
                text,
                attachments,
                delivery,
                steer,
            } => self.render_user(
                &item,
                item_ix,
                text,
                attachments,
                *delivery,
                *steer,
                &ctx,
                cx,
            ),
            ItemKind::Assistant { text, streaming } => {
                self.render_assistant(&item, item_ix, text, *streaming, new_reply, &ctx, cx)
            }
            ItemKind::Tool(t) => self.render_tool(&item, item_ix, t, &ctx, cx),
            ItemKind::Notice { text, level } => {
                self.render_notice(&item, item_ix, text, *level, &ctx, cx)
            }
        };
        let scale = ctx.theme.scale;
        // Steps and thinking are quiet, slim lines; messages keep their room.
        let (pad_top, pad_bottom) = match &item.kind {
            ItemKind::Tool(_) => (1.0, 1.0),
            ItemKind::Notice { .. } => (2.0, 2.0),
            // The header of a message ("You 11:30", "Pipkin 11:32") starts a new step in the
            // conversation, so it gets room above it; the steps between stay tight.
            ItemKind::User { .. } => (16.0, 4.0),
            ItemKind::Assistant { .. } if new_reply => (16.0, 4.0),
            _ => (3.0, 4.0),
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(760.0 * scale + 48.0))
                    .px(px(24.0))
                    .pt(px(pad_top * scale))
                    .pb(px(pad_bottom * scale))
                    .when(item.native_only, |row| {
                        row.child(
                            div()
                                .text_size(ctx.theme.small_size())
                                .text_color(ctx.theme.colors.text_muted)
                                .child("Native-only history · not in the Pi copy"),
                        )
                    })
                    .child(body),
            )
            .into_any_element()
    }

    fn render_header_row(
        &self,
        header: HeaderState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let HeaderState {
            has_older,
            loading,
            opened,
            saved_copy,
            older_error,
        } = header;
        let c = &theme.colors;
        let base = div()
            .h(px(40.0))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(theme.small_size())
            .font_family(theme.ui_font())
            .text_color(c.text_faint);
        if loading {
            base.child("Loading earlier messages…").into_any_element()
        } else if has_older && opened {
            let model = self.model.clone();
            let _ = cx;
            base.child(
                div()
                    .id("load-older")
                    .px_2()
                    .py_1()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(c.bg_hover))
                    .on_mouse_down(MouseButton::Left, stop_mouse_down)
                    .on_click(move |_, _, cx| {
                        model.update(cx, |m, cx| m.dispatch(Command::LoadOlder, cx));
                    })
                    .child(match &older_error {
                        Some(e) => format!("{e} Try again"),
                        None => "Load earlier messages".to_owned(),
                    })
                    .when(older_error.is_some(), |d| d.text_color(c.danger)),
            )
            .into_any_element()
        } else if opened {
            base.child("Beginning of conversation").into_any_element()
        } else if saved_copy {
            base.child(if has_older {
                "Earlier messages need the engine"
            } else {
                "Beginning of the saved copy"
            })
            .into_any_element()
        } else {
            base.child("Opening conversation…").into_any_element()
        }
    }

    /// The round marker beside a message: a plain dot for the person, the amber bolt for Pipkin.
    fn avatar(theme: &Theme, pipkin: bool) -> gpui::Div {
        let c = &theme.colors;
        let size = px(28.0 * theme.scale.max(1.0));
        let base = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(size)
            .rounded_full();
        if pipkin {
            // Midnight in both themes: the mascot's own colours.
            base.bg(gpui::rgb(0x080f1a))
                .border_1()
                .border_color(c.border_strong)
                .child(icon("zap", 13.0, c.accent_fill))
        } else {
            base.bg(c.text_muted)
                .child(div().size(px(8.0)).rounded_full().bg(c.bg_app))
        }
    }

    fn author_line(
        &self,
        label: &str,
        at: i64,
        status: Option<(String, gpui::Hsla)>,
        theme: &Theme,
    ) -> gpui::Div {
        let c = &theme.colors;
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .mb(px(3.0))
            .text_size(theme.small_size())
            .font_family(theme.ui_font())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.text_muted)
                    .child(label.to_string()),
            )
            .child(div().text_color(c.text_faint).child(format_time(at)));
        if let Some((text, color)) = status {
            row = row.child(div().text_color(color).child(text));
        }
        row
    }

    #[allow(clippy::too_many_arguments)]
    fn render_user(
        &self,
        item: &TranscriptItem,
        item_ix: usize,
        text: &str,
        attachments: &[Attachment],
        delivery: Delivery,
        steer: bool,
        ctx: &RowCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &ctx.theme;
        let c = &theme.colors;
        let status = match delivery {
            Delivery::Sent => None,
            Delivery::Pending => Some(("Sending…".to_string(), c.text_faint)),
            Delivery::Unknown => Some(("Outcome unknown".to_string(), c.warning)),
            Delivery::Rejected => Some(("Not sent".to_string(), c.danger)),
        };
        let label = if steer { "You (steering)" } else { "You" };
        let blocks = self.blocks_for(ctx.conv, item, ctx.expanded);
        let mut card = div().w_full();
        for (bi, block) in blocks.iter().enumerate() {
            card = card.child(self.block_element(ctx, item, item_ix, bi, block, cx));
        }
        if !attachments.is_empty() {
            let mut chips = div().flex().flex_wrap().gap(px(6.0)).mt(px(6.0));
            for a in attachments {
                let tint = if a.error.is_some() {
                    c.danger
                } else {
                    c.text_muted
                };
                let label = match &a.error {
                    Some(e) => format!("{} — {e}", a.name),
                    None => a.name.clone(),
                };
                chips = chips.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(6.0))
                        .h(px(22.0 * theme.scale))
                        .rounded(px(4.0))
                        .bg(c.bg_hover)
                        .text_size(theme.small_size())
                        .text_color(tint)
                        .child(icon("paperclip", 12.0, tint))
                        .child(label),
                );
            }
            card = card.child(chips);
        }
        let aria = format!("You: {}", clip(text, A11Y_TEXT_LIMIT));
        div()
            .id(ElementId::NamedInteger("msg".into(), item.id.0))
            .role(Role::Article)
            .aria_label(aria)
            .flex()
            .gap(px(10.0))
            .child(Self::avatar(theme, false))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.author_line(label, item.at, status, theme))
                    .child(card),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_assistant(
        &self,
        item: &TranscriptItem,
        item_ix: usize,
        text: &str,
        streaming: bool,
        show_label: bool,
        ctx: &RowCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &ctx.theme;
        let c = &theme.colors;
        let blocks = self.blocks_for(ctx.conv, item, ctx.expanded);
        let mut col = div().w_full().flex().flex_col();
        if show_label {
            let status = streaming.then(|| ("Writing…".to_string(), c.text_faint));
            col = col.child(self.author_line("Pipkin", item.at, status, theme));
        }
        for (bi, block) in blocks.iter().enumerate() {
            // A run of list items reads as one section, with breathing room around the
            // whole list rather than a repeated card around every point.
            if matches!(block.kind, BlockKind::ListItem)
                && (bi == 0 || !matches!(blocks[bi - 1].kind, BlockKind::ListItem))
            {
                col = col.child(div().h(px(6.0 * theme.scale)));
            }
            col = col.child(self.block_element(ctx, item, item_ix, bi, block, cx));
            if matches!(block.kind, BlockKind::ListItem)
                && !blocks
                    .get(bi + 1)
                    .is_some_and(|b| matches!(b.kind, BlockKind::ListItem))
            {
                col = col.child(div().h(px(6.0 * theme.scale)));
            }
        }
        if streaming {
            // Static indicator: no animation, identical under reduced motion.
            col = col.child(
                div()
                    .mt(px(2.0))
                    .w(px(7.0))
                    .h(px(14.0 * theme.scale))
                    .rounded(px(1.0))
                    .bg(c.accent),
            );
        }
        let aria = format!("Pipkin: {}", clip(text, A11Y_TEXT_LIMIT));
        // Follow-on parts of one reply line up under the first, without repeating the marker.
        let marker = if show_label {
            Self::avatar(theme, true)
        } else {
            div().flex_none().w(px(28.0 * theme.scale.max(1.0)))
        };
        div()
            .id(ElementId::NamedInteger("msg".into(), item.id.0))
            .role(Role::Article)
            .aria_label(aria)
            .flex()
            .gap(px(10.0))
            .child(marker)
            .child(div().flex_1().min_w_0().child(col))
            .into_any_element()
    }

    fn render_notice(
        &self,
        item: &TranscriptItem,
        item_ix: usize,
        text: &str,
        level: NoticeLevel,
        ctx: &RowCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &ctx.theme;
        let c = &theme.colors;
        // The model's thinking is a quiet line: its first words, with a chevron to read it all.
        if level == NoticeLevel::Info
            && let Some(rest) = text.strip_prefix("Thinking")
        {
            // A folded run is just its fold line, wherever its latest item is a thinking line.
            if let Some(line) = ctx.steps.as_ref().filter(|l| !l.open) {
                return div()
                    .id(ElementId::NamedInteger("msg".into(), item.id.0))
                    .role(Role::Article)
                    .aria_label(format!("{}. Show all {} steps", line.caption, line.count))
                    .pl(px((28.0 + 10.0) * theme.scale.max(1.0)))
                    .child(self.render_run_line(line, theme, cx))
                    .into_any_element();
            }
            let body = rest.trim();
            let first = super::tools::thinking_first_line(text);
            let hidden = matches!(text, "Thinking..." | "Thinking (not shown)");
            let can_open = !body.is_empty() && !hidden;
            let (conv, id, expanded) = (ctx.conv, item.id, ctx.expanded && can_open);
            let mut line = div()
                .id(ElementId::NamedInteger("think".into(), id.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .min_h(px(26.0 * theme.scale.max(1.0)))
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(5.0))
                .text_size(theme.small_size())
                .text_color(c.text_faint)
                .role(Role::Button)
                .aria_label(if hidden {
                    "Thinking...".to_owned()
                } else {
                    format!("Thinking: {}", clip(&first, A11Y_TEXT_LIMIT))
                })
                .aria_expanded(expanded)
                .when(can_open, |d| {
                    d.cursor_pointer()
                        .tab_stop(true)
                        .focus_visible(|s| s.border_1().border_color(c.accent))
                        .hover(|s| s.bg(c.bg_hover))
                        .on_mouse_down(MouseButton::Left, stop_mouse_down)
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_tool(conv, id, cx)))
                })
                .child(icon("clock", 14.0, c.text_faint))
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.text_muted)
                        .child(if hidden { "Thinking..." } else { "Thinking" }),
                );
            if !expanded && !hidden {
                line = line.child(div().flex_1().min_w_0().truncate().child(first));
            } else {
                line = line.child(div().flex_1());
            }
            if can_open {
                line = line.child(icon(
                    if expanded {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    13.0,
                    c.text_faint,
                ));
            }
            let mut col = div().w_full().flex().flex_col();
            // An opened run carries its fold line above its first item.
            if let Some(run) = ctx.steps.as_ref().filter(|l| l.open) {
                col = col.child(self.render_run_line(run, theme, cx));
            }
            col = col.child(line);
            if expanded {
                let blocks = self.blocks_for(ctx.conv, item, true);
                let mut full = div().w_full().pl(px(30.0)).pr(px(6.0)).pb(px(4.0));
                for (bi, block) in blocks.iter().enumerate() {
                    full = full.child(self.block_element(ctx, item, item_ix, bi, block, cx));
                }
                col = col.child(full.text_color(c.text_muted));
            }
            return div()
                .id(ElementId::NamedInteger("msg".into(), item.id.0))
                .role(Role::Article)
                .aria_label(if hidden {
                    "Thinking...".to_owned()
                } else {
                    format!("Thinking: {}", clip(body, A11Y_TEXT_LIMIT))
                })
                .pl(px((28.0 + 10.0) * theme.scale.max(1.0)))
                .child(col)
                .into_any_element();
        }
        let (name, tint, bg) = match level {
            NoticeLevel::Info => ("circle-help", c.text_muted, c.bg_hover),
            NoticeLevel::Error => ("circle-alert", c.danger, c.danger_bg),
        };
        let blocks = self.blocks_for(ctx.conv, item, ctx.expanded);
        let mut body = div().flex_1().min_w_0().text_color(tint);
        for (bi, block) in blocks.iter().enumerate() {
            body = body.child(self.block_element(ctx, item, item_ix, bi, block, cx));
        }
        let label = match level {
            NoticeLevel::Info => format!("Notice: {}", clip(text, A11Y_TEXT_LIMIT)),
            NoticeLevel::Error => format!("Error: {}", clip(text, A11Y_TEXT_LIMIT)),
        };
        div()
            .id(ElementId::NamedInteger("msg".into(), item.id.0))
            .role(Role::Article)
            .aria_label(label)
            .flex()
            .items_start()
            .gap(px(8.0))
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .bg(bg)
            .child(div().mt(px(3.0)).child(icon(name, 14.0, tint)))
            .child(body)
            .into_any_element()
    }

    /// Folded: only the latest thinking explanation (or outcome). Expanded: the technical
    /// summary and each underlying step. Clicking the line switches between the two.
    fn render_run_line(
        &self,
        line: &StepsLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let c = &theme.colors;
        let (conv, first) = (self.current.unwrap_or(ConversationId(0)), line.first);
        let (mark, color) = if line.running {
            ("loader-circle", c.accent)
        } else if line.failed {
            ("circle-alert", c.danger)
        } else {
            ("check", c.success)
        };
        let hint = if line.open {
            "Collapse progress"
        } else {
            "Show all steps"
        };
        let label = if line.open || !line.running {
            line.summary.as_str()
        } else {
            line.caption.as_str()
        };
        let accessible_label = if line.open {
            line.summary.as_str()
        } else {
            line.caption.as_str()
        };
        div()
            .id(ElementId::NamedInteger("steps".into(), first.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .min_h(px(28.0 * theme.scale.max(1.0)))
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(5.0))
            .cursor_pointer()
            .tab_stop(true)
            .hover(|s| s.bg(c.bg_hover))
            .focus_visible(|s| s.border_1().border_color(c.accent))
            .role(Role::Button)
            .aria_label(format!("{accessible_label} {hint}"))
            .aria_expanded(line.open)
            .on_mouse_down(MouseButton::Left, stop_mouse_down)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_steps(conv, first, cx)))
            .child(icon(mark, 14.0, color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme.small_size())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.text_muted)
                    .child(label.to_owned()),
            )
            .when(!line.open && !line.running, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(theme.small_size())
                        .text_color(if line.failed { c.danger } else { c.text_muted })
                        .child(if line.failed {
                            "· Finished with errors."
                        } else {
                            "· Done."
                        }),
                )
            })
            .child(icon(
                if line.open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                13.0,
                c.text_faint,
            ))
    }

    fn render_tool(
        &self,
        item: &TranscriptItem,
        item_ix: usize,
        tool: &ToolCall,
        ctx: &RowCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &ctx.theme;
        let c = &theme.colors;
        let (status_icon, status_text, status_color) = match tool.status {
            ToolStatus::Running => ("loader-circle", "Running", c.accent),
            ToolStatus::Ok => ("check", "Done", c.success),
            ToolStatus::Failed => ("circle-alert", "Failed", c.danger),
        };
        let label = super::tools::label(&tool.name, &tool.input);
        let file_ref = ctx
            .change_paths
            .iter()
            .position(|p| !p.is_empty() && tool.input.contains(p.as_str()));
        let conv = ctx.conv;
        let id = item.id;
        let expanded = ctx.expanded;
        let model = self.model.clone();
        // A folded run is just its human-readable progress line; the steps
        // themselves come back, one slim line each, when it is opened.
        if let Some(line) = ctx.steps.as_ref().filter(|l| !l.open) {
            return div()
                .id(ElementId::NamedInteger("msg".into(), id.0))
                .role(Role::Article)
                .aria_label(format!("{}. Show all {} steps", line.caption, line.count))
                .pl(px((28.0 + 10.0) * theme.scale.max(1.0)))
                .child(self.render_run_line(line, theme, cx))
                .into_any_element();
        }
        let target = shorten(&label.target, TOOL_TARGET_CHARS);
        let mut header = div()
            .id(ElementId::NamedInteger("tool".into(), id.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .min_h(px(26.0 * theme.scale.max(1.0)))
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(5.0))
            .cursor_pointer()
            .tab_stop(true)
            .hover(|s| s.bg(c.bg_hover))
            .focus_visible(|s| s.border_1().border_color(c.accent))
            .role(Role::Button)
            .aria_label(format!("Tool {}: {}", tool.name, status_text))
            .aria_expanded(expanded)
            .on_mouse_down(MouseButton::Left, stop_mouse_down)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_tool(conv, id, cx)))
            .child(icon(label.icon, 14.0, c.text_faint))
            .child(
                div()
                    .flex_none()
                    .text_size(theme.small_size())
                    .text_color(c.text_muted)
                    .child(label.verb.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme.mono_font())
                    .text_size(theme.small_size())
                    .text_color(c.text_faint)
                    .child(target.clone()),
            );
        if let Some(ix) = file_ref {
            header = header.child(
                div()
                    .id(ElementId::NamedInteger("tool-diff".into(), id.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(6.0))
                    .rounded(px(4.0))
                    .text_size(theme.small_size())
                    .text_color(c.accent)
                    .hover(|s| s.bg(c.accent_bg))
                    .role(Role::Button)
                    .aria_label("View changes for this file")
                    .on_mouse_down(MouseButton::Left, stop_mouse_down)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        model.update(cx, |m, cx| m.dispatch(Command::SelectChange(ix), cx));
                    })
                    .child(icon("file-diff", 12.0, c.accent))
                    .child("Diff"),
            );
        }
        // Success is a quiet check; a failure or a run in progress says so in words.
        header = header
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(theme.small_size())
                    .text_color(status_color)
                    .child(icon(status_icon, 14.0, status_color))
                    .when(tool.status != ToolStatus::Ok, |d| d.child(status_text)),
            )
            .child(icon(
                if expanded {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                13.0,
                c.text_faint,
            ));
        let mut col = div().w_full().flex().flex_col();
        if let Some(line) = ctx.steps.as_ref().filter(|l| l.open) {
            col = col.child(self.render_run_line(line, theme, cx));
        }
        col = col.child(header);
        if expanded {
            let blocks = self.blocks_for(conv, item, true);
            let mut body = div().w_full().pl(px(22.0)).pt(px(4.0));
            if blocks.is_empty() {
                body = body.child(
                    div()
                        .text_size(theme.small_size())
                        .text_color(c.text_faint)
                        .child(if tool.status == ToolStatus::Running {
                            "Waiting for output…"
                        } else {
                            "No output"
                        }),
                );
            }
            for (bi, block) in blocks.iter().enumerate() {
                body = body.child(self.block_element(ctx, item, item_ix, bi, block, cx));
            }
            if tool.truncated {
                body = body.child(
                    div()
                        .text_size(theme.small_size())
                        .text_color(c.warning)
                        .child(format!(
                            "Output truncated — showing first {} KB of {} KB",
                            tool.output.len().div_ceil(1024),
                            tool.full_len.div_ceil(1024)
                        )),
                );
            }
            if tool.status != ToolStatus::Running && (!tool.output.is_empty() || tool.truncated) {
                let action = |name: &'static str, label: &'static str, icon_name: &'static str| {
                    let model = self.model.clone();
                    let command = if name == "tool-copy" {
                        Command::CopyToolOutput(id)
                    } else {
                        Command::SaveToolOutput(id)
                    };
                    div()
                        .id(ElementId::NamedInteger(name.into(), id.0))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .text_size(theme.small_size())
                        .text_color(c.text_muted)
                        .hover(|s| s.bg(c.bg_hover))
                        .cursor_pointer()
                        .role(Role::Button)
                        .aria_label(label)
                        .on_mouse_down(MouseButton::Left, stop_mouse_down)
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            model.update(cx, |m, cx| m.dispatch(command.clone(), cx));
                        })
                        .child(icon(icon_name, 12.0, c.text_muted))
                        .child(label)
                };
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .pt(px(2.0))
                        .child(action(
                            "tool-copy",
                            if tool.truncated {
                                "Copy full output"
                            } else {
                                "Copy output"
                            },
                            "copy",
                        ))
                        .child(action(
                            "tool-save",
                            if tool.truncated {
                                "Save full output…"
                            } else {
                                "Save output…"
                            },
                            "file-plus",
                        )),
                );
            }
            col = col.child(body);
        }
        div()
            .id(ElementId::NamedInteger("msg".into(), id.0))
            .role(Role::Article)
            .aria_label(format!("Tool {}: {}", tool.name, status_text))
            // Under the message text, past the marker column.
            .pl(px((28.0 + 10.0) * theme.scale.max(1.0)))
            .child(col)
            .into_any_element()
    }

    fn blocks_for(
        &self,
        conv: ConversationId,
        item: &TranscriptItem,
        expanded: bool,
    ) -> super::document::Blocks {
        match self.convs.get(&conv) {
            Some(cv) => cv.doc.blocks(item, expanded),
            None => Document::default().blocks(item, expanded),
        }
    }

    fn block_element(
        &self,
        ctx: &RowCtx,
        item: &TranscriptItem,
        item_ix: usize,
        bi: usize,
        block: &Block,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &ctx.theme;
        let c = &theme.colors;
        let is_code = block.is_code();
        if matches!(block.kind, BlockKind::Rule) {
            return div()
                .my(px(8.0))
                .h(px(1.0))
                .w_full()
                .bg(c.border_strong)
                .into_any_element();
        }
        let resolved = self.resolved;
        let selection = resolved.and_then(|r| r.range_in(item_ix, bi as u32, block.text.len()));
        // Inline code that names a changed file, and links, are clickable.
        let mut file_refs: Vec<Range<usize>> = Vec::new();
        let mut clicks: Vec<(Range<usize>, Click)> = Vec::new();
        if !is_code {
            for s in &block.spans {
                if let Some(url) = &s.style.link {
                    clicks.push((s.range.clone(), Click::Url(url.clone())));
                } else if s.style.code {
                    let t = &block.text[s.range.clone()];
                    let hit = ctx
                        .change_paths
                        .iter()
                        .position(|p| !t.is_empty() && (p == t || p.ends_with(&format!("/{t}"))));
                    if let Some(ix) = hit {
                        file_refs.push(s.range.clone());
                        clicks.push((s.range.clone(), Click::Change(ix)));
                    }
                }
            }
        }
        // Citations of retained web sources (`[s12]`) open their page; a citation whose
        // source the engine no longer lists stays plain text.
        if !ctx.sources.is_empty() {
            for (range, id) in citations(&block.text) {
                if clicks
                    .iter()
                    .any(|(r, _)| r.start < range.end && range.start < r.end)
                {
                    continue;
                }
                if let Some((_, url)) = ctx.sources.iter().find(|(s, _)| *s == id) {
                    file_refs.push(range.clone());
                    clicks.push((range, Click::Url(url.clone())));
                }
            }
        }
        let (family, size, line_height, color, weight) = match &block.kind {
            BlockKind::Code { .. } => (
                theme.mono_font(),
                theme.code_size(),
                theme.code_line_height(),
                c.text,
                FontWeight::NORMAL,
            ),
            BlockKind::Heading(n) => {
                let f = match n {
                    1 => 1.45,
                    2 => 1.25,
                    3 => 1.12,
                    _ => 1.0,
                };
                (
                    theme.ui_font(),
                    px(f32::from(theme.body_size()) * f),
                    px((f32::from(theme.body_size()) * f * 1.35).round()),
                    c.text,
                    FontWeight::SEMIBOLD,
                )
            }
            _ => (
                theme.ui_font(),
                theme.body_size(),
                theme.body_line_height(),
                if block.quote { c.text_muted } else { c.text },
                FontWeight::NORMAL,
            ),
        };
        let base = BaseText {
            family: family.clone(),
            mono: theme.mono_font(),
            color,
            weight,
        };
        let runs = build_runs(block, theme, &base, selection, &file_refs);
        let key = element_key(item.id, bi as u32);
        let styled = StyledText::new(SharedString::from(block.text.clone())).with_runs(runs);
        let layout = styled.layout().clone();
        let inner: AnyElement = if clicks.is_empty() {
            styled.into_any_element()
        } else {
            let ranges: Vec<Range<usize>> = clicks.iter().map(|(r, _)| r.clone()).collect();
            let actions: Vec<Click> = clicks.into_iter().map(|(_, a)| a).collect();
            let model = self.model.clone();
            InteractiveText::new(ElementId::NamedInteger("tb".into(), key), styled)
                .on_click(ranges, move |i, window, cx| match &actions[i] {
                    Click::Url(u) => {
                        let project = model
                            .read(cx)
                            .state
                            .current_project()
                            .map(|p| p.path.as_str());
                        match super::links::target(u, project) {
                            Ok(uri) => cx.open_url(&uri),
                            Err(reason) => {
                                drop(window.prompt(
                                    gpui::PromptLevel::Warning,
                                    "Cannot open document",
                                    Some(&reason),
                                    &["OK"],
                                    cx,
                                ));
                            }
                        }
                    }
                    Click::Change(ix) => {
                        let ix = *ix;
                        model.update(cx, |m, cx| m.dispatch(Command::SelectChange(ix), cx));
                    }
                })
                .into_any_element()
        };
        let text = BlockText::new(
            layout,
            inner,
            self.registry.clone(),
            item.id,
            item_ix,
            bi as u32,
            block.text.len(),
        );
        let text_box = div()
            .w_full()
            .font_family(family)
            .text_size(size)
            .line_height(line_height)
            .text_color(color)
            .child(text);
        match &block.kind {
            BlockKind::Code { lang } => {
                let label = match lang.as_deref() {
                    Some(l) if l == TOOL_INPUT => "Input".to_string(),
                    Some(l) if l == TOOL_OUTPUT => "Output".to_string(),
                    Some(l) => l.to_string(),
                    None => "text".to_string(),
                };
                let copied = self.copied == Some((item.id, bi as u32));
                let copy_text = block.text.clone();
                let ckey = (item.id, bi as u32);
                let button = div()
                    .id(ElementId::NamedInteger("copy".into(), key))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(6.0))
                    .h(px(22.0))
                    .rounded(px(4.0))
                    .text_size(theme.small_size())
                    .font_family(theme.ui_font())
                    .text_color(if copied { c.success } else { c.text_muted })
                    .cursor_pointer()
                    .hover(|s| s.bg(c.bg_hover))
                    .role(Role::Button)
                    .aria_label(if copied { "Copied" } else { "Copy code" })
                    .on_mouse_down(MouseButton::Left, stop_mouse_down)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_block(copy_text.clone(), ckey, cx)
                    }))
                    .child(icon(
                        if copied { "check" } else { "copy" },
                        12.0,
                        if copied { c.success } else { c.text_muted },
                    ))
                    .child(if copied { "Copied" } else { "Copy" });
                div()
                    .w_full()
                    .my(px(5.0))
                    .rounded(px(6.0))
                    .bg(c.code_bg)
                    .border_1()
                    .border_color(c.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .pl(px(10.0))
                            .pr(px(4.0))
                            .pt(px(3.0))
                            .child(
                                div()
                                    .text_size(theme.small_size())
                                    .font_family(theme.mono_font())
                                    .text_color(c.text_faint)
                                    .child(label),
                            )
                            .child(button),
                    )
                    .child(div().px(px(10.0)).pb(px(8.0)).child(text_box))
                    .into_any_element()
            }
            BlockKind::ListItem => {
                let depth = block.indent.max(1) as f32 - 1.0;
                let mut d = div()
                    .w_full()
                    .pl(px((depth * 20.0 + 12.0) * theme.scale.max(1.0)))
                    .mb(px(6.0 * theme.scale));
                if block.quote {
                    d = d.border_l_2().border_color(c.border_strong).pl(px(12.0));
                }
                d.child(text_box).into_any_element()
            }
            BlockKind::Heading(_) => div()
                .w_full()
                .mt(px(16.0 * theme.scale))
                .mb(px(6.0 * theme.scale))
                .child(text_box)
                .into_any_element(),
            _ => {
                let mut d = div().w_full().mb(px(10.0 * theme.scale));
                if block.quote {
                    d = d.border_l_2().border_color(c.border_strong).pl(px(12.0));
                }
                d.child(text_box).into_any_element()
            }
        }
    }
}

impl Render for TranscriptView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = theme.colors.clone();
        self.mounted.set(0);

        let selected = self.model.read(cx).state.selected;
        if selected != self.current {
            self.current = selected;
            self.drag = None;
            self.autoscroll = 0.0;
            self.last_following = true;
        }
        let mut empty = true;
        let mut run_label = "Idle";
        let read_only = {
            let state = &self.model.read(cx).state;
            match self.current {
                Some(id) => state.read_only_reason(id),
                None => state.read_only.clone(),
            }
        };
        let no_model = {
            let model = self.model.read(cx);
            model.state.mode == pipkin_core::Mode::Real
                && model.state.connection.is_ready()
                && model.state.models.is_empty()
        };
        self.resolved = None;
        if let Some(id) = self.current {
            self.ensure_conv(id, cx);
            let model = self.model.clone();
            if let Some(conv) = model.read(cx).state.conversation(id) {
                empty = conv.items.is_empty() && !conv.has_older;
                run_label = conv.run.label();
                if let Some(cv) = self.convs.get(&id) {
                    self.resolved = cv
                        .selection
                        .and_then(|s| cv.doc.resolve(&conv.items, &cv.expanded, &s));
                }
            }
            self.maybe_load_older_deferred(cx);
        }

        let mut root = div()
            .id("transcript")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .role(Role::Log)
            .aria_label("Conversation transcript")
            .relative()
            .size_full()
            .on_action(cx.listener(|this, _: &SelectAllText, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &ExtendLeft, _, cx| this.extend(Step::Left, cx)))
            .on_action(cx.listener(|this, _: &ExtendRight, _, cx| this.extend(Step::Right, cx)))
            .on_action(cx.listener(|this, _: &ExtendUp, _, cx| this.extend(Step::BlockUp, cx)))
            .on_action(cx.listener(|this, _: &ExtendDown, _, cx| this.extend(Step::BlockDown, cx)))
            .on_action(
                cx.listener(|this, _: &ExtendToStart, _, cx| this.extend(Step::DocStart, cx)),
            )
            .on_action(cx.listener(|this, _: &ExtendToEnd, _, cx| this.extend(Step::DocEnd, cx)))
            .on_action(cx.listener(|this, _: &ClearSelection, _, cx| this.clear_selection(cx)))
            .cursor(gpui::CursorStyle::IBeam);

        let following = self
            .current
            .and_then(|id| self.convs.get(&id))
            .map(|cv| cv.list.is_following_tail())
            .unwrap_or(true);

        match (self.current, empty) {
            (Some(id), false) if self.convs.contains_key(&id) => {
                let list_state = self.convs[&id].list.clone();
                let rows = list(
                    list_state,
                    cx.processor(|this, ix: usize, _window, cx| this.render_row(ix, cx)),
                )
                .size_full();
                root = root.child(div().size_full().child(FrameProbe {
                    child: Some(rows.into_any_element()),
                    view: cx.entity().downgrade(),
                    registry: self.registry.clone(),
                    stats: self.frame_stats.clone(),
                    bounds_out: self.list_bounds.clone(),
                    started: None,
                }));
            }
            _ => {
                let welcome = self.current.is_some() && read_only.is_none();
                let (title, hint): (&str, String) = if self.current.is_none() {
                    (
                        "No conversation selected",
                        "Choose a conversation or start a new one.".into(),
                    )
                } else {
                    match &read_only {
                        Some(reason) => ("No messages in this session", reason.clone()),
                        None if no_model => (
                            "No model is ready.",
                            "Use the account or model controls below to get started.".into(),
                        ),
                        None => ("Ready when you are.", "Connect. Prompt. Build.".into()),
                    }
                };
                root = root.child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .font_family(theme.ui_font())
                        .when(welcome, |d| {
                            // The approved mascot, at its own proportions (420 x 436).
                            d.child(
                                gpui::img(crate::assets::MASCOT)
                                    .w(px(126.0 * theme.scale.max(1.0)))
                                    .h(px(131.0 * theme.scale.max(1.0)))
                                    .object_fit(gpui::ObjectFit::Contain)
                                    .mb(px(8.0)),
                            )
                        })
                        .child(
                            div()
                                .text_size(if welcome {
                                    px(18.0 * theme.scale)
                                } else {
                                    theme.ui_size()
                                })
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if welcome { c.text } else { c.text_muted })
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(theme.small_size())
                                .text_color(c.text_faint)
                                .child(hint),
                        ),
                );
            }
        }

        if !following && !empty {
            root = root.child(
                div().absolute().bottom(px(14.0)).right(px(22.0)).child(
                    div()
                        .id("jump-to-latest")
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(30.0 * theme.scale.max(1.0)))
                        .px(px(12.0))
                        .rounded(px(15.0))
                        .bg(c.bg_elevated)
                        .border_1()
                        .border_color(c.border_strong)
                        .shadow_md()
                        .text_size(theme.small_size())
                        .font_family(theme.ui_font())
                        .text_color(c.text)
                        .cursor_pointer()
                        .hover(|s| s.bg(c.bg_active))
                        .role(Role::Button)
                        .aria_label("Jump to latest message")
                        .on_mouse_down(MouseButton::Left, stop_mouse_down)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.jump_to_latest(window, cx)),
                        )
                        .child(icon("arrow-down", 14.0, c.text))
                        .child("Jump to latest"),
                ),
            );
        }

        // Live run status for assistive technology: a status node, visually empty.
        root = root.child(
            div()
                .id("run-status")
                .role(Role::Status)
                .aria_label(format!("Run status: {run_label}"))
                .absolute()
                .size(px(1.0)),
        );
        root
    }
}

impl TranscriptView {
    /// Like `maybe_load_older`, but safe to call from render.
    fn maybe_load_older_deferred(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.current else { return };
        let Some(cv) = self.convs.get(&id) else {
            return;
        };
        if cv.list.logical_scroll_top().item_ix > 2 {
            return;
        }
        let wants = self.model.read(cx).state.conversation(id).is_some_and(|c| {
            c.has_older && !c.loading_older && c.opened && c.older_error.is_none()
        });
        if wants {
            let weak = cx.entity().downgrade();
            cx.defer(move |cx| {
                weak.update(cx, |this, cx| this.maybe_load_older(cx)).ok();
            });
        }
    }
}

/// Wraps the list: records paint cost, clears the hit registry each frame and routes mouse
/// events (which must keep flowing while the pointer is outside the list during a drag).
struct FrameProbe {
    child: Option<AnyElement>,
    view: WeakEntity<TranscriptView>,
    registry: SharedRegistry,
    stats: Rc<RefCell<FrameStats>>,
    bounds_out: Rc<Cell<Bounds<Pixels>>>,
    started: Option<Instant>,
}

impl IntoElement for FrameProbe {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for FrameProbe {
    type RequestLayoutState = ();
    type PrepaintState = Duration;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.as_mut().unwrap().request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Duration {
        let start = Instant::now();
        self.started = Some(start);
        self.registry.borrow_mut().clear();
        self.bounds_out.set(bounds);
        self.child.as_mut().unwrap().prepaint(window, cx);
        start.elapsed()
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut (),
        prepaint_cost: &mut Duration,
        window: &mut Window,
        cx: &mut App,
    ) {
        let start = Instant::now();
        // Registered before the children paint, so child handlers (copy buttons, tool headers)
        // run first in the bubble phase and can stop propagation.
        let view = self.view.clone();
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || ev.button != MouseButton::Left
                || !bounds.contains(&ev.position)
            {
                return;
            }
            view.update(cx, |v, cx| v.mouse_down(ev, window, cx)).ok();
        });
        let view = self.view.clone();
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble {
                view.update(cx, |v, cx| v.mouse_move(ev, cx)).ok();
            }
        });
        let view = self.view.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble {
                view.update(cx, |v, _| v.mouse_up()).ok();
            }
        });
        self.child.as_mut().unwrap().paint(window, cx);
        self.stats
            .borrow_mut()
            .record(*prepaint_cost + start.elapsed());
    }
}

/// The `[s<n>]` citations in `text`, as `(byte range, "s<n>")`, in order.
pub fn citations(text: &str) -> Vec<(Range<usize>, String)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b's' && bytes[i + 2].is_ascii_digit() {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b']' && j - (i + 2) <= 9 {
                out.push((i..j + 1, text[i + 1..j].to_owned()));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod citation_tests {
    use super::citations;

    #[test]
    fn citations_are_found_exactly_and_never_inside_other_brackets() {
        let found = citations("See [s1] and [s12], not [sx] or [s] or [s123456789012]; [s7].");
        let ids: Vec<&str> = found.iter().map(|(_, id)| id.as_str()).collect();
        assert_eq!(ids, ["s1", "s12", "s7"]);
        assert_eq!(&"See [s1] and [s12]"[found[0].0.clone()], "[s1]");
        assert_eq!(&"See [s1] and [s12]"[found[1].0.clone()], "[s12]");
        assert!(citations("no citations [S1] [s 2]").is_empty());
    }
}

#[cfg(test)]
mod tests;
