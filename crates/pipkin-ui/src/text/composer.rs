//! `ComposerEditor`: a multiline, wrapping text editor wired to the platform input handler
//! (IME marked text, UTF-16 ranges) and drawn by a custom element.

use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardEntry, ClipboardItem, ContentMask, Context, CursorStyle,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, Hsla, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Role, ScrollWheelEvent, SharedString, Style, Task, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, fill, point, prelude::*, px,
    relative, size,
};

use super::actions::*;
use super::mentions::{self, PathMention};
use super::model::EditorModel;
use crate::theme::ActiveTheme;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerEvent {
    /// The user edited the text (typing, paste, undo, IME commit). Not emitted by `set_text`.
    Changed,
    /// Enter was pressed with no IME composition active.
    Submit,
    Escape,
    /// Single-line list navigation, or multiline prompt recall at a visual text boundary.
    Up,
    Down,
    /// Clipboard image data, handled by the workspace as a durable draft attachment.
    PasteImage(gpui::Image),
}

const DEFAULT_MAX_LINES: usize = 8;
const CARET_WIDTH: f32 = 1.5;

#[derive(Clone, PartialEq)]
struct LayoutKey {
    revision: u64,
    width: Pixels,
    font_size: Pixels,
    line_height: Pixels,
    marked: Option<Range<usize>>,
    color: Hsla,
    /// The placeholder text when it is what's being shown.
    placeholder: Option<SharedString>,
}

/// Shaped, wrapped text plus the geometry needed for hit testing and painting. All points are
/// in content coordinates (origin at the top-left of the unscrolled text).
struct TextLayout {
    key: LayoutKey,
    lines: Vec<WrappedLine>,
    line_starts: Vec<usize>,
    line_tops: Vec<Pixels>,
    height: Pixels,
    width: Pixels,
    line_height: Pixels,
}

impl TextLayout {
    fn line_ix(&self, offset: usize) -> usize {
        self.line_starts
            .partition_point(|s| *s <= offset)
            .saturating_sub(1)
            .min(self.lines.len().saturating_sub(1))
    }

    fn point_for_offset(&self, offset: usize) -> Point<Pixels> {
        if self.lines.is_empty() {
            return point(px(0.), px(0.));
        }
        let i = self.line_ix(offset);
        let local = (offset - self.line_starts[i]).min(self.lines[i].text.len());
        let p = self.lines[i]
            .position_for_index(local, self.line_height)
            .unwrap_or_default();
        point(p.x, p.y + self.line_tops[i])
    }

    fn offset_for_point(&self, p: Point<Pixels>) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let y = p.y.max(px(0.));
        let i = self
            .line_tops
            .partition_point(|t| *t <= y)
            .saturating_sub(1)
            .min(self.lines.len() - 1);
        let local = point(p.x.max(px(0.)), y - self.line_tops[i]);
        let ix = self.lines[i]
            .closest_index_for_position(local, self.line_height)
            .unwrap_or_else(|e| e);
        self.line_starts[i] + ix.min(self.lines[i].text.len())
    }

    fn selection_rects(&self, sel: Range<usize>, newline_extra: Pixels) -> Vec<Bounds<Pixels>> {
        let mut out = Vec::new();
        if sel.is_empty() {
            return out;
        }
        let lh = self.line_height;
        for (i, line) in self.lines.iter().enumerate() {
            let start = self.line_starts[i];
            let end = start + line.text.len();
            if sel.end < start || sel.start > end {
                continue;
            }
            let s = sel.start.max(start) - start;
            let e = sel.end.min(end) - start;
            let extends = sel.end > end;
            let top = self.line_tops[i];
            let ps = line.position_for_index(s, lh).unwrap_or_default();
            let pe = line.position_for_index(e, lh).unwrap_or_default();
            let row = |p: Point<Pixels>| (p.y / lh).round() as usize;
            let (rs, re) = (row(ps), row(pe));
            for r in rs..=re {
                let x0 = if r == rs { ps.x } else { px(0.) };
                let mut x1 = if r == re { pe.x } else { self.width };
                if r == re && extends {
                    x1 += newline_extra;
                }
                if x1 > x0 {
                    out.push(Bounds::from_corners(
                        point(x0, top + lh * r as f32),
                        point(x1, top + lh * (r + 1) as f32),
                    ));
                }
            }
        }
        out
    }
}

pub struct ComposerEditor {
    focus_handle: FocusHandle,
    model: EditorModel,
    placeholder: SharedString,
    label: SharedString,
    disabled: bool,
    single_line: bool,
    prompt_history_enabled: bool,
    prompt_history_browsing: bool,
    max_lines: usize,
    layout: Option<Rc<TextLayout>>,
    scroll_y: Pixels,
    scroll_x: Pixels,
    scroll_to_caret: bool,
    goal_x: Option<Pixels>,
    selecting: bool,
    last_bounds: Option<Bounds<Pixels>>,
    /// Where the caret was when the input method was last told, so it is told again when the
    /// caret moves (its candidate window follows it).
    ime_caret: Option<Bounds<Pixels>>,
    blink_visible: bool,
    blink_task: Option<Task<()>>,
    project_root: Option<PathBuf>,
    path_index: Vec<String>,
    path_task: Option<Task<()>>,
    path_error: Option<String>,
    path_epoch: u64,
    path_query: Option<PathMention>,
    path_matches: Vec<String>,
    path_selected: usize,
    path_links: Vec<PathMention>,
}

impl EventEmitter<ComposerEvent> for ComposerEditor {}

impl Focusable for ComposerEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ComposerEditor {
    /// Multiline, wrapping, growing editor.
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(false, cx)
    }

    /// Single-line input (search box, palette, rename): no wrapping or newlines, scrolls
    /// horizontally, Up/Down emit `ComposerEvent::Up/Down`, Enter emits `Submit`.
    pub fn single_line(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(true, cx)
    }

    fn build(single_line: bool, cx: &mut Context<Self>) -> Self {
        ComposerEditor {
            focus_handle: cx.focus_handle(),
            model: EditorModel::new(),
            placeholder: SharedString::default(),
            label: "Message".into(),
            disabled: false,
            single_line,
            prompt_history_enabled: false,
            prompt_history_browsing: false,
            max_lines: if single_line { 1 } else { DEFAULT_MAX_LINES },
            layout: None,
            scroll_y: px(0.),
            scroll_x: px(0.),
            scroll_to_caret: false,
            goal_x: None,
            selecting: false,
            last_bounds: None,
            ime_caret: None,
            blink_visible: true,
            blink_task: None,
            project_root: None,
            path_index: Vec::new(),
            path_task: None,
            path_error: None,
            path_epoch: 0,
            path_query: None,
            path_matches: Vec::new(),
            path_selected: 0,
            path_links: Vec::new(),
        }
    }

    // ------------------------------------------------------------- public API

    /// Only the conversation composer opts in; search/setup inputs retain their usual keys.
    pub fn set_prompt_history_enabled(&mut self, enabled: bool) {
        self.prompt_history_enabled = enabled;
    }

    pub fn set_prompt_history_browsing(&mut self, browsing: bool) {
        self.prompt_history_browsing = browsing;
    }

    /// Index off the UI thread. Dropping the task and checking the root prevents
    /// an old project's results from appearing after a conversation switch.
    pub fn set_project_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.project_root == root {
            return;
        }
        self.project_root = root;
        self.path_epoch = self.path_epoch.wrapping_add(1);
        self.path_task = None;
        self.path_error = None;
        self.path_index.clear();
        self.refresh_paths();
        self.scan_paths(cx);
        cx.notify();
    }

    fn scan_paths(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.project_root.clone() else {
            return;
        };
        if self.path_task.is_some() {
            return;
        }
        self.path_epoch = self.path_epoch.wrapping_add(1);
        let epoch = self.path_epoch;
        self.path_error = None;
        self.path_index.clear();
        self.refresh_paths();
        self.path_task = Some(cx.spawn(async move |this, cx| {
            let scan_root = root.clone();
            let result = cx
                .background_spawn(async move { mentions::collect(&scan_root) })
                .await;
            this.update(cx, |this, cx| {
                if this.project_root.as_ref() == Some(&root) && this.path_epoch == epoch {
                    this.path_task = None;
                    match result {
                        Ok(paths) => this.path_index = paths,
                        Err(error) => {
                            this.path_error = Some(format!("Project paths unavailable: {error}"))
                        }
                    }
                    this.refresh_paths();
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// Rescan once when a mention query opens, not on every character or render.
    fn refresh_input_paths(&mut self, cx: &mut Context<Self>) {
        let previous = self.path_query.as_ref().map(|q| q.range.start);
        self.refresh_paths();
        let current = self.path_query.as_ref().map(|q| q.range.start);
        if current.is_some() && current != previous {
            self.scan_paths(cx);
        }
    }

    fn refresh_paths(&mut self) {
        self.path_links = mentions::tokens(self.model.text())
            .into_iter()
            .filter(|t| self.path_index.binary_search(&t.path).is_ok())
            .collect();
        self.path_query =
            if self.disabled || self.model.is_composing() || self.model.has_selection() {
                None
            } else {
                mentions::query(self.model.text(), self.model.head())
            };
        self.path_matches = self
            .path_query
            .as_ref()
            .map(|q| mentions::matching(&self.path_index, &q.path))
            .unwrap_or_default();
        self.path_selected = 0;
        self.layout = None;
    }

    fn complete_path(&mut self, i: usize, cx: &mut Context<Self>) -> bool {
        if self.disabled || self.model.is_composing() {
            return false;
        }
        let Some(query) = self.path_query.clone() else {
            return false;
        };
        let Some(path) = self.path_matches.get(i).cloned() else {
            return false;
        };
        self.model.move_to(query.range.start, false);
        self.model.move_to(query.range.end, true);
        let changed = self.model.insert(&mentions::insertion(&path));
        self.after_edit(changed, cx);
        true
    }

    fn on_complete_path(&mut self, _: &CompletePath, _: &mut Window, cx: &mut Context<Self>) {
        if !self.complete_path(self.path_selected, cx) {
            cx.propagate();
        }
    }

    pub fn text(&self) -> String {
        self.model.text().to_string()
    }

    /// Replace the text programmatically: resets undo history, puts the caret at the end and
    /// does not emit `Changed`.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = self.sanitize(text);
        self.model.set_text(&text);
        self.refresh_input_paths(cx);
        self.scroll_y = px(0.);
        self.scroll_x = px(0.);
        self.scroll_to_caret = true;
        self.goal_x = None;
        cx.notify();
    }

    /// Takes effect on the next render (call `cx.notify()` if the editor is already on screen).
    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    /// Select the whole text (e.g. when opening a rename dialog).
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.model.select_all();
        self.after_motion(false, cx);
    }

    pub fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn is_single_line(&self) -> bool {
        self.single_line
    }

    fn sanitize(&self, text: &str) -> String {
        let text = super::model::normalize_newlines(text);
        if self.single_line {
            text.replace('\n', " ")
        } else {
            text
        }
    }

    /// Accessible name announced by assistive technology.
    pub fn set_label(&mut self, label: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.label = label.into();
        cx.notify();
    }

    /// Read-only, dimmed, no caret, and the platform stops sending text input.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.refresh_paths();
            self.selecting = false;
            self.reset_blink(cx);
            cx.notify();
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Lines shown before the editor stops growing and scrolls internally (default 8).
    pub fn set_max_lines(&mut self, lines: usize, cx: &mut Context<Self>) {
        self.max_lines = lines.max(1);
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus_handle, cx);
    }

    pub fn is_composing(&self) -> bool {
        self.model.is_composing()
    }

    pub fn selection(&self) -> Range<usize> {
        self.model.selection()
    }

    pub fn model(&self) -> &EditorModel {
        &self.model
    }

    /// Height of all content at the last measured width, before clamping to `max_lines`.
    pub fn content_height(&self) -> Pixels {
        self.layout.as_ref().map(|l| l.height).unwrap_or_default()
    }

    // ----------------------------------------------------------------- layout

    fn ensure_layout(&mut self, width: Pixels, window: &mut Window, cx: &App) -> Rc<TextLayout> {
        let theme = cx.theme();
        let font_size = theme.body_size();
        let line_height = theme.body_line_height();
        let placeholder = self.model.is_empty();
        let color = if placeholder || self.disabled {
            theme.colors.text_faint
        } else {
            theme.colors.text
        };
        let marked = if placeholder {
            None
        } else {
            self.model.marked_range()
        };
        let key = LayoutKey {
            revision: self.model.revision(),
            width,
            font_size,
            line_height,
            marked: marked.clone(),
            color,
            placeholder: placeholder.then(|| self.placeholder.clone()),
        };
        if let Some(l) = self.layout.as_ref().filter(|l| l.key == key) {
            return l.clone();
        }
        let text: SharedString = if placeholder {
            self.placeholder.clone()
        } else {
            self.model.text().to_string().into()
        };
        let font = gpui::font(theme.ui_font());
        let run = TextRun {
            len: text.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match marked {
            Some(m) => [
                TextRun {
                    len: m.start,
                    ..run.clone()
                },
                TextRun {
                    len: m.end - m.start,
                    underline: Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: text.len() - m.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect::<Vec<_>>(),
            None if !placeholder && !self.path_links.is_empty() => {
                let mut runs = Vec::new();
                let mut start = 0;
                for link in &self.path_links {
                    if link.range.start > start {
                        runs.push(TextRun {
                            len: link.range.start - start,
                            ..run.clone()
                        });
                    }
                    runs.push(TextRun {
                        len: link.range.len(),
                        color: theme.colors.accent,
                        underline: Some(UnderlineStyle {
                            color: Some(theme.colors.accent),
                            thickness: px(1.0),
                            wavy: false,
                        }),
                        ..run.clone()
                    });
                    start = link.range.end;
                }
                if start < text.len() {
                    runs.push(TextRun {
                        len: text.len() - start,
                        ..run
                    });
                }
                runs
            }
            None => vec![run],
        };
        let lines: Vec<WrappedLine> = window
            .text_system()
            .shape_text(
                text,
                font_size,
                &runs,
                (!self.single_line).then_some(width),
                None,
            )
            .map(|l| l.into_iter().collect())
            .unwrap_or_default();
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut line_tops = Vec::with_capacity(lines.len());
        let (mut start, mut top) = (0usize, px(0.));
        for line in &lines {
            line_starts.push(start);
            line_tops.push(top);
            start += line.text.len() + 1;
            top += line.size(line_height).height;
        }
        let layout = Rc::new(TextLayout {
            key,
            lines,
            line_starts,
            line_tops,
            height: top.max(line_height),
            width,
            line_height,
        });
        self.layout = Some(layout.clone());
        layout
    }

    /// Called from the element's measure function.
    fn measure(&mut self, width: Pixels, window: &mut Window, cx: &mut Context<Self>) -> Pixels {
        let layout = self.ensure_layout(width, window, cx);
        let max_h = layout.line_height * self.max_lines as f32;
        layout.height.min(max_h)
    }

    fn max_scroll(&self, viewport: Pixels) -> Pixels {
        (self.content_height() - viewport).max(px(0.))
    }

    fn offset_at(&self, pos: Point<Pixels>) -> usize {
        let (Some(bounds), Some(layout)) = (self.last_bounds, self.layout.as_ref()) else {
            return 0;
        };
        if self.model.is_empty() {
            return 0;
        }
        let local = point(
            pos.x - bounds.left() + self.scroll_x,
            pos.y - bounds.top() + self.scroll_y,
        );
        self.model.snap_to_boundary(layout.offset_for_point(local))
    }

    // ---------------------------------------------------------- state changes

    fn reset_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_visible = true;
        self.blink_task = None;
        if cx.theme().reduced_motion || self.disabled {
            return;
        }
        self.blink_task = Some(cx.spawn(async move |this, cx| {
            for _ in 0..20 {
                cx.background_executor()
                    .timer(Duration::from_millis(530))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    this.blink_visible = !this.blink_visible;
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.blink_visible = true;
                cx.notify();
            })
            .ok();
        }));
    }

    fn after_edit(&mut self, changed: bool, cx: &mut Context<Self>) {
        if changed {
            self.refresh_input_paths(cx);
            self.goal_x = None;
            self.scroll_to_caret = true;
            self.reset_blink(cx);
            cx.emit(ComposerEvent::Changed);
            cx.notify();
        }
    }

    fn after_motion(&mut self, keep_goal: bool, cx: &mut Context<Self>) {
        self.refresh_input_paths(cx);
        if !keep_goal {
            self.goal_x = None;
        }
        self.scroll_to_caret = true;
        self.reset_blink(cx);
        cx.notify();
    }

    fn motion(&mut self, f: impl FnOnce(&mut EditorModel), cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        f(&mut self.model);
        self.after_motion(false, cx);
    }

    fn edit(&mut self, f: impl FnOnce(&mut EditorModel) -> bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let changed = f(&mut self.model);
        self.after_edit(changed, cx);
    }

    /// Home/End move to the start/end of the visual (wrapped) row.
    fn row_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let Some(layout) = self.layout.clone().filter(|_| !self.model.is_empty()) else {
            self.motion(|m| if end { m.end(extend) } else { m.home(extend) }, cx);
            return;
        };
        let head = self.model.head();
        let p = layout.point_for_offset(head);
        let x = if end {
            layout.width + px(10_000.)
        } else {
            px(0.)
        };
        let mut off = layout.offset_for_point(point(x, p.y + layout.line_height * 0.5));
        if end && off != self.model.line_end(head) && layout.point_for_offset(off).y > p.y {
            // The wrap boundary offset renders at the next row's start; stay on this row.
            off = self.model.prev_boundary(off);
        }
        self.model.move_to(off, extend);
        self.after_motion(false, cx);
    }

    fn vertical(&mut self, dir: f32, extend: bool, page: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if !page && !extend && !self.model.is_composing() && !self.path_matches.is_empty() {
            self.path_selected = (self.path_selected as i32 + if dir < 0.0 { -1 } else { 1 })
                .rem_euclid(self.path_matches.len() as i32)
                as usize;
            cx.notify();
            return;
        }
        if self.single_line {
            if !page && !extend {
                cx.emit(if dir < 0. {
                    ComposerEvent::Up
                } else {
                    ComposerEvent::Down
                });
            }
            return;
        }
        let recall = self.prompt_history_enabled
            && (dir < 0. || self.prompt_history_browsing)
            && !page
            && !extend
            && !self.model.is_composing()
            && !self.model.has_selection();
        if recall && self.model.text().is_empty() {
            cx.emit(if dir < 0. {
                ComposerEvent::Up
            } else {
                ComposerEvent::Down
            });
            return;
        }
        let Some(layout) = self.layout.clone() else {
            return;
        };
        let lh = layout.line_height;
        let head = self.model.head();
        let p = layout.point_for_offset(head);
        if recall && (p.y + lh * dir < px(0.) || p.y + lh * dir >= layout.height) {
            cx.emit(if dir < 0. {
                ComposerEvent::Up
            } else {
                ComposerEvent::Down
            });
            return;
        }
        let goal = *self.goal_x.get_or_insert(p.x);
        let step = if page {
            let vh = self.last_bounds.map(|b| b.size.height).unwrap_or(lh);
            (vh - lh).max(lh)
        } else {
            lh
        };
        let target_y = p.y + step * dir;
        if target_y < px(0.) {
            self.model.doc_start(extend);
        } else if target_y >= layout.height {
            self.model.doc_end(extend);
        } else {
            let off = layout.offset_for_point(point(goal, target_y + lh * 0.5));
            self.model.move_to(off, extend);
        }
        self.after_motion(true, cx);
    }

    // --------------------------------------------------------------- actions

    fn on_backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(|m| m.backspace(), cx);
    }
    fn on_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(|m| m.delete(), cx);
    }
    fn on_delete_word_back(&mut self, _: &DeleteWordBack, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(|m| m.delete_word_back(), cx);
    }
    fn on_delete_word_forward(
        &mut self,
        _: &DeleteWordForward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(|m| m.delete_word_forward(), cx);
    }
    fn on_left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.left(false), cx);
    }
    fn on_right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.right(false), cx);
    }
    fn on_up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1., false, false, cx);
    }
    fn on_down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1., false, false, cx);
    }
    fn on_word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.word_left(false), cx);
    }
    fn on_word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.word_right(false), cx);
    }
    fn on_select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.left(true), cx);
    }
    fn on_select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.right(true), cx);
    }
    fn on_select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1., true, false, cx);
    }
    fn on_select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1., true, false, cx);
    }
    fn on_select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.word_left(true), cx);
    }
    fn on_select_word_right(
        &mut self,
        _: &SelectWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion(|m| m.word_right(true), cx);
    }
    fn on_home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.row_edge(false, false, cx);
    }
    fn on_end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.row_edge(true, false, cx);
    }
    fn on_select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.row_edge(false, true, cx);
    }
    fn on_select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.row_edge(true, true, cx);
    }
    fn on_doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.doc_start(false), cx);
    }
    fn on_doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.doc_end(false), cx);
    }
    fn on_select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.doc_start(true), cx);
    }
    fn on_select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.doc_end(true), cx);
    }
    fn on_page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1., false, true, cx);
    }
    fn on_page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1., false, true, cx);
    }
    fn on_select_page_up(&mut self, _: &SelectPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1., true, true, cx);
    }
    fn on_select_page_down(&mut self, _: &SelectPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1., true, true, cx);
    }
    fn on_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(|m| m.select_all(), cx);
    }

    fn on_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.model.has_selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.model.selected_text().to_string(),
            ));
        }
    }

    fn on_cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || !self.model.has_selection() || self.model.is_composing() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.model.selected_text().to_string(),
        ));
        self.edit(|m| m.insert(""), cx);
    }

    fn on_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.model.is_composing() {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        // A screenshot may offer both a bitmap and text. Attach the bitmap rather than
        // accidentally inserting its fallback text into the message.
        if !self.single_line
            && let Some(image) = item.entries().iter().find_map(|entry| match entry {
                ClipboardEntry::Image(image) => Some(image),
                _ => None,
            })
        {
            cx.emit(ComposerEvent::PasteImage(image.clone()));
        } else if let Some(text) = item.text() {
            let text = self.sanitize(&text);
            self.edit(|m| m.insert(&text), cx);
        }
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(|m| m.undo(), cx);
    }

    fn on_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(|m| m.redo(), cx);
    }

    fn on_enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        // While composing, Enter belongs to the input method (commit); it must never submit.
        if self.disabled || self.model.is_composing() {
            return;
        }
        if !self.complete_path(self.path_selected, cx) {
            cx.emit(ComposerEvent::Submit);
        }
    }

    fn on_newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if self.model.is_composing() || self.single_line {
            return;
        }
        self.edit(|m| m.insert("\n"), cx);
    }

    fn on_escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        if self.path_query.take().is_some() {
            self.path_matches.clear();
            cx.notify();
            return;
        }
        cx.emit(ComposerEvent::Escape);
    }

    // ----------------------------------------------------------------- mouse

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        if self.disabled {
            return;
        }
        let offset = self.offset_at(event.position);
        if event.modifiers.control
            && let Some(link) = self.path_links.iter().find(|l| l.range.contains(&offset))
            && let Some(root) = &self.project_root
        {
            cx.open_url(&mentions::file_url(&root.join(&link.path)));
            return;
        }
        match event.click_count {
            0 | 1 => {
                self.model.move_to(offset, event.modifiers.shift);
                self.selecting = true;
            }
            2 => {
                self.model.select_word_at(offset);
                self.selecting = true;
            }
            _ => self.model.select_line_at(offset),
        }
        self.after_motion(false, cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if let Some(bounds) = self.last_bounds {
            let max = self.max_scroll(bounds.size.height);
            if event.position.y < bounds.top() {
                self.scroll_y =
                    (self.scroll_y - (bounds.top() - event.position.y) * 0.25).max(px(0.));
            } else if event.position.y > bounds.bottom() {
                self.scroll_y =
                    (self.scroll_y + (event.position.y - bounds.bottom()) * 0.25).min(max);
            }
        }
        let offset = self.offset_at(event.position);
        self.model.move_to(offset, true);
        self.refresh_input_paths(cx);
        self.goal_x = None;
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(bounds) = self.last_bounds else {
            return;
        };
        let max = self.max_scroll(bounds.size.height);
        if max <= px(0.) {
            return;
        }
        let lh = self
            .layout
            .as_ref()
            .map(|l| l.line_height)
            .unwrap_or(px(20.));
        let dy = event.delta.pixel_delta(lh).y;
        let next = (self.scroll_y - dy).clamp(px(0.), max);
        if next != self.scroll_y {
            self.scroll_y = next;
            cx.stop_propagation();
            cx.notify();
        }
    }

    // ----------------------------------------------------------- paint prep

    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Prepaint {
        let layout = self.ensure_layout(bounds.size.width, window, cx);
        self.last_bounds = Some(bounds);
        let vh = bounds.size.height;
        let max = self.max_scroll(vh);
        if self.scroll_to_caret {
            self.scroll_to_caret = false;
            let p = layout.point_for_offset(self.model.head());
            if p.y < self.scroll_y {
                self.scroll_y = p.y;
            } else if p.y + layout.line_height > self.scroll_y + vh {
                self.scroll_y = p.y + layout.line_height - vh;
            }
        }
        self.scroll_y = self.scroll_y.clamp(px(0.), max);
        if self.single_line {
            let caret_x = layout.point_for_offset(self.model.head()).x;
            let end_x = layout.point_for_offset(self.model.text().len()).x;
            let vw = bounds.size.width;
            let pad = px(CARET_WIDTH) + px(2.);
            if caret_x < self.scroll_x {
                self.scroll_x = caret_x;
            } else if caret_x + pad > self.scroll_x + vw {
                self.scroll_x = caret_x + pad - vw;
            }
            self.scroll_x = self.scroll_x.clamp(px(0.), (end_x + pad - vw).max(px(0.)));
        }
        let origin = bounds.origin - point(self.scroll_x, self.scroll_y);
        let theme = cx.theme();
        let selection = if self.model.is_empty() {
            Vec::new()
        } else {
            layout
                .selection_rects(self.model.selection(), theme.body_size() * 0.3)
                .into_iter()
                .map(|r| Bounds::new(origin + r.origin, r.size))
                .collect()
        };
        let caret = (!self.model.has_selection() && !self.disabled).then(|| {
            let p = if self.model.is_empty() {
                point(px(0.), px(0.))
            } else {
                layout.point_for_offset(self.model.head())
            };
            Bounds::new(origin + p, size(px(CARET_WIDTH), layout.line_height))
        });
        Prepaint {
            layout,
            selection,
            selection_color: theme.colors.text_selection,
            caret,
            caret_color: theme.colors.accent,
            origin,
        }
    }
}

struct Prepaint {
    layout: Rc<TextLayout>,
    selection: Vec<Bounds<Pixels>>,
    selection_color: Hsla,
    caret: Option<Bounds<Pixels>>,
    caret_color: Hsla,
    origin: Point<Pixels>,
}

struct ComposerElement {
    editor: Entity<ComposerEditor>,
}

impl IntoElement for ComposerElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ComposerElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, ()) {
        let editor = self.editor.clone();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let id = window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.unwrap_or(match available.width {
                AvailableSpace::Definite(w) => w,
                _ => px(480.),
            });
            let height = editor.update(cx, |e, cx| e.measure(width, window, cx));
            size(width, height)
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        self.editor
            .update(cx, |e, cx| e.prepare(bounds, window, cx))
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus_handle, blink_visible, disabled) = {
            let e = self.editor.read(cx);
            (e.focus_handle.clone(), e.blink_visible, e.disabled)
        };
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        let focused = focus_handle.is_focused(window);
        // An input method places its candidate window beside the caret, and only learns where
        // that is when told: tell it whenever the caret has moved.
        if focused {
            let caret = self.editor.update(cx, |e, cx| {
                let selection = e.selected_text_range(false, window, cx)?;
                e.bounds_for_range(selection.range, bounds, window, cx)
            });
            let moved = self.editor.update(cx, |e, _| {
                let moved = e.ime_caret != caret;
                e.ime_caret = caret;
                moved
            });
            if moved {
                window.invalidate_character_coordinates();
            }
        }
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for r in &prepaint.selection {
                window.paint_quad(fill(*r, prepaint.selection_color));
            }
            let layout = prepaint.layout.clone();
            for (i, line) in layout.lines.iter().enumerate() {
                let top = layout.line_tops[i];
                let origin = prepaint.origin + point(px(0.), top);
                let height = line.size(layout.line_height).height;
                if origin.y + height < bounds.top() || origin.y > bounds.bottom() {
                    continue;
                }
                line.paint(
                    origin,
                    layout.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            }
            if let Some(caret) = prepaint.caret
                && focused
                && blink_visible
                && !disabled
            {
                window.paint_quad(fill(caret, prepaint.caret_color));
            }
        });
        super::latency::painted();
    }
}

impl EntityInputHandler for ComposerEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (text, used) = self.model.text_for_range_utf16(range_utf16);
        *actual_range = Some(used);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.model.range_to_utf16(&self.model.selection()),
            reversed: self.model.is_reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.model
            .marked_range()
            .map(|r| self.model.range_to_utf16(&r))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.model.unmark();
        self.refresh_input_paths(cx);
        cx.emit(ComposerEvent::Changed);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        super::latency::mark();
        let new_text = self.sanitize(new_text);
        let changed = self
            .model
            .replace_text_in_range_utf16(range_utf16, &new_text);
        self.after_edit(changed, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let new_text = self.sanitize(new_text);
        self.model.replace_and_mark_text_in_range_utf16(
            range_utf16,
            &new_text,
            new_selected_range_utf16,
        );
        self.after_edit(true, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let r = self.model.range_from_utf16(&range_utf16);
        let (a, b) = if self.model.is_empty() {
            (point(px(0.), px(0.)), point(px(0.), px(0.)))
        } else {
            (
                layout.point_for_offset(r.start),
                layout.point_for_offset(r.end),
            )
        };
        let origin = element_bounds.origin - point(self.scroll_x, self.scroll_y);
        let width = if a.y == b.y {
            (b.x - a.x).max(px(1.))
        } else {
            px(1.)
        };
        Some(Bounds::new(origin + a, size(width, layout.line_height)))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        if !bounds.contains(&point) {
            return None;
        }
        let offset = self.offset_at(point);
        Some(self.model.offset_to_utf16(offset))
    }

    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        !self.disabled
    }
}

/// What a screen reader is told about the text: one run per line, and where the selection is.
///
/// A line's run holds its text and, when another line follows, the line break, so reading by
/// character crosses lines the way a person expects. Lengths are per character in UTF-8 bytes,
/// as accessibility toolkits want them.
#[derive(Debug, PartialEq)]
pub(crate) struct A11yText {
    pub runs: Vec<A11yRun>,
    /// `(run, character within the run)` of the selection's anchor and its focus (the caret).
    pub anchor: (usize, usize),
    pub focus: (usize, usize),
}

#[derive(Debug, PartialEq)]
pub(crate) struct A11yRun {
    pub text: String,
    pub character_lengths: Vec<u8>,
    pub word_starts: Vec<u8>,
}

/// Where each word begins, in characters from the start of the run (a reader steps by word
/// to these). Only positions a `u8` can hold are reported.
fn word_starts(text: &str) -> Vec<u8> {
    let mut starts = Vec::new();
    let mut in_word = false;
    for (i, ch) in text.chars().enumerate() {
        if ch.is_whitespace() {
            in_word = false;
        } else if !in_word {
            in_word = true;
            if let Ok(at) = u8::try_from(i) {
                starts.push(at);
            }
        }
    }
    starts
}

pub(crate) fn a11y_text(text: &str, anchor: usize, head: usize) -> A11yText {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut runs = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let mut run_text = (*line).to_owned();
        if i + 1 < lines.len() {
            run_text.push('\n');
        }
        let character_lengths = run_text.chars().map(|c| c.len_utf8() as u8).collect();
        let word_starts = word_starts(&run_text);
        runs.push(A11yRun {
            text: run_text,
            character_lengths,
            word_starts,
        });
    }
    // The run and character a byte offset falls on.
    let position = |offset: usize| -> (usize, usize) {
        let offset = offset.min(text.len());
        let mut start = 0;
        for (i, line) in lines.iter().enumerate() {
            let end = start + line.len();
            if offset <= end || i + 1 == lines.len() {
                let within = offset.saturating_sub(start).min(line.len());
                let mut at = within;
                while !line.is_char_boundary(at) {
                    at -= 1;
                }
                return (i, line[..at].chars().count());
            }
            start = end + 1;
        }
        (0, 0)
    };
    A11yText {
        runs,
        anchor: position(anchor),
        focus: position(head),
    }
}

impl Render for ComposerEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let a11y = a11y_text(self.model.text(), self.model.anchor(), self.model.head());
        let mut root = gpui::div()
            .id("composer-editor")
            .key_context("Composer")
            // Explicit tracked handles carry their own tab metadata; div.tab_index
            // only configures handles created by GPUI for that div.
            .track_focus(
                &self
                    .focus_handle
                    .clone()
                    .tab_index(0)
                    .tab_stop(!self.disabled),
            )
            .role(Role::MultilineTextInput)
            .aria_label(self.label.clone())
            .aria_value(SharedString::from(self.model.text().to_string()))
            // The text as runs, with the caret and selection, so a screen reader can read and
            // follow what is typed.
            .a11y_synthetic_children(move |builder| {
                let mut ids = Vec::with_capacity(a11y.runs.len());
                for (i, run) in a11y.runs.iter().enumerate() {
                    let mut node = gpui::accesskit::Node::new(Role::TextRun);
                    node.set_value(run.text.clone());
                    node.set_character_lengths(run.character_lengths.clone());
                    node.set_word_starts(run.word_starts.clone());
                    let id = builder.synthetic_node_id(i);
                    builder.push_child(id, node);
                    ids.push(id);
                }
                let at = |(run, character_index): (usize, usize)| gpui::accesskit::TextPosition {
                    node: ids[run.min(ids.len() - 1)],
                    character_index,
                };
                builder
                    .parent_node()
                    .set_text_selection(gpui::accesskit::TextSelection {
                        anchor: at(a11y.anchor),
                        focus: at(a11y.focus),
                    });
            })
            .cursor(CursorStyle::IBeam)
            .w_full()
            .on_action(cx.listener(Self::on_backspace))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_delete_word_back))
            .on_action(cx.listener(Self::on_delete_word_forward))
            .on_action(cx.listener(Self::on_left))
            .on_action(cx.listener(Self::on_right))
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_word_left))
            .on_action(cx.listener(Self::on_word_right))
            .on_action(cx.listener(Self::on_select_left))
            .on_action(cx.listener(Self::on_select_right))
            .on_action(cx.listener(Self::on_select_up))
            .on_action(cx.listener(Self::on_select_down))
            .on_action(cx.listener(Self::on_select_word_left))
            .on_action(cx.listener(Self::on_select_word_right))
            .on_action(cx.listener(Self::on_home))
            .on_action(cx.listener(Self::on_end))
            .on_action(cx.listener(Self::on_select_home))
            .on_action(cx.listener(Self::on_select_end))
            .on_action(cx.listener(Self::on_doc_start))
            .on_action(cx.listener(Self::on_doc_end))
            .on_action(cx.listener(Self::on_select_doc_start))
            .on_action(cx.listener(Self::on_select_doc_end))
            .on_action(cx.listener(Self::on_page_up))
            .on_action(cx.listener(Self::on_page_down))
            .on_action(cx.listener(Self::on_select_page_up))
            .on_action(cx.listener(Self::on_select_page_down))
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_cut))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_enter))
            .on_action(cx.listener(Self::on_newline))
            .on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(Self::on_complete_path))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll));
        if self.model.is_empty() && !self.placeholder.is_empty() {
            root = root.aria_placeholder(self.placeholder.clone());
        }
        let matches = self.path_matches.clone();
        let selected = self.path_selected;
        let theme = cx.theme().clone();
        let popup = if self.path_query.is_some()
            && self.project_root.is_some()
            && self.focus_handle.is_focused(_window)
        {
            self.last_bounds.map(|bounds| {
                let mut menu = gpui::div()
                    .id("path-completions")
                    .max_h((_window.viewport_size().height - px(16.0)).max(px(0.0)))
                    .overflow_y_scroll()
                    .role(Role::ListBox)
                    .aria_label("Project paths")
                    .w(px(420.0).min(_window.viewport_size().width - px(16.0)))
                    .bg(theme.colors.bg_elevated)
                    .border_1()
                    .border_color(theme.colors.border_strong)
                    .rounded(px(8.0))
                    .shadow_lg()
                    .p(px(6.0))
                    .flex()
                    .flex_col()
                    .occlude();
                if matches.is_empty() {
                    menu = menu.child(
                        gpui::div()
                            .px(px(10.0))
                            .py(px(6.0))
                            .text_color(theme.colors.text_muted)
                            .child(self.path_error.clone().unwrap_or_else(|| {
                                if self.path_task.is_some() {
                                    "Scanning project paths…".into()
                                } else {
                                    "No matching project paths".into()
                                }
                            })),
                    );
                }
                for (i, path) in matches.into_iter().enumerate() {
                    let editor = cx.entity();
                    menu = menu.child(
                        gpui::div()
                            .id(("path", i))
                            .when(i == 0, |r| r.debug_selector(|| "first-project-path".into()))
                            .role(Role::ListBoxOption)
                            .aria_label(path.clone())
                            .aria_selected(i == selected)
                            .px(px(10.0))
                            .py(px(6.0))
                            .truncate()
                            .text_color(theme.colors.text)
                            .when(i == selected, |r| r.bg(theme.colors.bg_selected))
                            .hover(|r| r.bg(theme.colors.bg_hover))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                cx.stop_propagation();
                                editor.update(cx, |e, cx| {
                                    e.complete_path(i, cx);
                                    window.focus(&e.focus_handle, cx);
                                });
                            })
                            .child(path),
                    );
                }
                menu = menu.child(
                    gpui::div()
                        .px(px(10.0))
                        .py(px(4.0))
                        .text_size(theme.small_size())
                        .text_color(theme.colors.text_muted)
                        .child("Tab to complete · Ctrl-click a linked path to open"),
                );
                gpui::deferred(
                    gpui::anchored()
                        .anchor(gpui::Anchor::BottomLeft)
                        .position(point(bounds.origin.x, bounds.origin.y - px(8.0)))
                        .snap_to_window_with_margin(px(8.0))
                        .child(menu),
                )
                .with_priority(4)
            })
        } else {
            None
        };
        root.child(ComposerElement {
            editor: cx.entity(),
        })
        .children(popup)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::text;
    use crate::theme::Theme;

    struct Host {
        editor: Entity<ComposerEditor>,
        width: Pixels,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div().w(self.width).child(self.editor.clone())
        }
    }

    struct Harness {
        editor: Entity<ComposerEditor>,
        events: Rc<RefCell<Vec<ComposerEvent>>>,
    }

    fn harness(
        cx: &mut TestAppContext,
        single_line: bool,
        width: f32,
    ) -> (Harness, &mut VisualTestContext) {
        cx.update(|cx| {
            cx.set_global(Theme::new(
                pipkin_core::Theme::Dark,
                pipkin_core::TextSize::Normal,
                true,
            ));
            text::init(cx);
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        let window = cx.add_window(move |window, cx| {
            let editor = cx.new(|cx| {
                if single_line {
                    ComposerEditor::single_line(window, cx)
                } else {
                    ComposerEditor::new(window, cx)
                }
            });
            cx.subscribe(&editor, move |_, _, ev: &ComposerEvent, _| {
                sink.borrow_mut().push(ev.clone());
            })
            .detach();
            Host {
                editor,
                width: px(width),
            }
        });
        let editor = window
            .root(cx)
            .unwrap()
            .read_with(cx, |h, _| h.editor.clone());
        let vcx = VisualTestContext::from_window(window.into(), cx).into_mut();
        let handle = editor.read_with(vcx, |e, _| e.focus_handle.clone());
        vcx.update(|window, cx| window.focus(&handle, cx));
        vcx.run_until_parked();
        (Harness { editor, events }, vcx)
    }

    fn text_of(h: &Harness, cx: &mut VisualTestContext) -> String {
        h.editor.read_with(cx, |e, _| e.text())
    }

    struct PathProject(PathBuf);

    impl PathProject {
        fn new(paths: &[&str]) -> Self {
            let root = std::env::temp_dir().join(format!(
                "pipkin-mention-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            for path in paths {
                let file = root.join(path);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file, "synthetic").unwrap();
            }
            Self(root)
        }
    }

    impl Drop for PathProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[gpui::test]
    fn prompt_history_arrows_only_fire_at_multiline_boundaries(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 500.);
        h.editor.update(cx, |e, cx| {
            e.set_prompt_history_enabled(true);
            e.set_text("first\nsecond", cx);
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("up");
        assert!(!h.events.borrow().contains(&ComposerEvent::Up));
        cx.simulate_keystrokes("up");
        assert_eq!(h.events.borrow().as_slice(), &[ComposerEvent::Up]);
        h.editor
            .update(cx, |e, _| e.set_prompt_history_browsing(true));
        h.events.borrow_mut().clear();
        cx.simulate_keystrokes("down");
        assert!(h.events.borrow().is_empty());
        cx.simulate_keystrokes("down");
        assert_eq!(h.events.borrow().as_slice(), &[ComposerEvent::Down]);
        h.events.borrow_mut().clear();
        cx.simulate_keystrokes("shift-up");
        cx.simulate_keystrokes("up");
        assert!(
            h.events.borrow().is_empty(),
            "selection must not be replaced by a prompt"
        );
    }

    #[gpui::test]
    fn down_in_a_fresh_draft_is_cursor_movement_not_history(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 500.);
        h.editor.update(cx, |e, cx| {
            e.set_prompt_history_enabled(true);
            e.set_text("fresh draft", cx);
            e.model.doc_start(false);
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("down");
        assert!(h.events.borrow().is_empty());
        assert_eq!(
            h.editor.read_with(cx, |e, _| e.model.head()),
            "fresh draft".len()
        );
    }

    #[gpui::test]
    fn prompt_recall_is_opt_in_and_never_runs_during_ime(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 500.);
        cx.simulate_keystrokes("up down");
        assert!(h.events.borrow().is_empty());
        h.editor.update(cx, |e, _| {
            e.set_prompt_history_enabled(true);
            e.set_prompt_history_browsing(true);
        });
        cx.simulate_keystrokes("up down");
        assert_eq!(
            h.events.borrow().as_slice(),
            &[ComposerEvent::Up, ComposerEvent::Down]
        );
        cx.update(|window, cx| {
            h.editor.update(cx, |e, cx| {
                e.replace_and_mark_text_in_range(None, "候補", None, window, cx);
            })
        });
        cx.run_until_parked();
        h.events.borrow_mut().clear();
        cx.simulate_keystrokes("up down");
        assert!(!h.events.borrow().contains(&ComposerEvent::Up));
        assert!(!h.events.borrow().contains(&ComposerEvent::Down));
    }

    #[gpui::test]
    fn path_completion_inserts_links_and_undo_restores_the_query(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 500.);
        let project = PathProject::new(&["src/lib.rs", "src/main.rs"]);
        h.editor
            .update(cx, |e, cx| e.set_project_root(Some(project.0.clone()), cx));
        cx.run_until_parked();
        cx.simulate_input("look at @src/m");
        cx.run_until_parked();
        assert_eq!(
            h.editor.read_with(cx, |e, _| e.path_matches.clone()),
            ["src/main.rs"]
        );
        cx.simulate_keystrokes("tab");
        assert_eq!(text_of(&h, cx), "look at @src/main.rs ");
        assert_eq!(h.editor.read_with(cx, |e, _| e.path_links.len()), 1);
        assert!(!h.events.borrow().contains(&ComposerEvent::Submit));
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text_of(&h, cx), "look at @src/m");
        cx.simulate_keystrokes("ctrl-shift-z");
        assert_eq!(text_of(&h, cx), "look at @src/main.rs ");
        h.editor.update(cx, |e, cx| e.set_text("@src/", cx));
        cx.run_until_parked();
        // Discovery also includes the src/ directory, ahead of its files.
        cx.simulate_keystrokes("down down tab");
        assert_eq!(text_of(&h, cx), "@src/main.rs ");
    }

    #[gpui::test]
    fn path_completion_handles_spaces_and_is_disabled_during_ime(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 500.);
        let project = PathProject::new(&["my file.rs"]);
        h.editor
            .update(cx, |e, cx| e.set_project_root(Some(project.0.clone()), cx));
        cx.run_until_parked();
        cx.simulate_input("@my");
        cx.run_until_parked();
        h.editor.update(cx, |e, cx| {
            assert!(e.complete_path(0, cx));
        });
        assert_eq!(text_of(&h, cx), "@\"my file.rs\" ");
        h.editor.update_in(cx, |e, w, cx| {
            e.set_text("@", cx);
            e.replace_and_mark_text_in_range(None, "日", None, w, cx);
            assert!(e.path_matches.is_empty());
            assert!(!e.complete_path(0, cx));
        });
    }

    #[gpui::test]
    fn new_queries_refresh_deleted_new_unicode_paths_and_project_switches(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 500.);
        let a = PathProject::new(&["old.rs"]);
        let b = PathProject::new(&["other.rs"]);
        h.editor
            .update(cx, |e, cx| e.set_project_root(Some(a.0.clone()), cx));
        cx.run_until_parked();
        std::fs::remove_file(a.0.join("old.rs")).unwrap();
        std::fs::write(a.0.join("日 é.rs"), "synthetic").unwrap();
        cx.simulate_input("@");
        cx.run_until_parked();
        assert_eq!(
            h.editor.read_with(cx, |e, _| e.path_matches.clone()),
            ["日 é.rs"]
        );
        cx.simulate_keystrokes("tab");
        assert_eq!(text_of(&h, cx), "@\"日 é.rs\" ");
        assert_eq!(h.editor.read_with(cx, |e, _| e.path_links.len()), 1);
        h.editor.update(cx, |e, cx| {
            e.set_project_root(Some(b.0.clone()), cx);
            assert!(e.path_links.is_empty());
            assert!(e.path_matches.is_empty());
            e.set_project_root(Some(a.0.clone()), cx);
            e.set_project_root(Some(b.0.clone()), cx);
            e.set_text("@", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            h.editor.read_with(cx, |e, _| e.path_matches.clone()),
            ["other.rs"]
        );
        h.editor.update(cx, |e, cx| {
            e.set_project_root(Some(a.0.join("missing")), cx)
        });
        cx.run_until_parked();
        assert!(h.editor.read_with(cx, |e, _| e.path_error.is_some()));
        assert!(h.editor.read_with(cx, |e, _| e.path_matches.is_empty()));
        assert!(!h.editor.update(cx, |e, cx| e.complete_path(0, cx)));
    }

    #[gpui::test]
    fn clicking_completion_then_control_clicking_unicode_link_opens_only_that_project(
        cx: &mut TestAppContext,
    ) {
        let (h, cx) = harness(cx, false, 500.);
        let project = PathProject::new(&["日 é.rs"]);
        h.editor
            .update(cx, |e, cx| e.set_project_root(Some(project.0.clone()), cx));
        cx.run_until_parked();
        cx.simulate_input("@日");
        cx.run_until_parked();
        let option = cx
            .debug_bounds("first-project-path")
            .expect("rendered completion option");
        cx.simulate_mouse_down(option.center(), MouseButton::Left, gpui::Modifiers::none());
        cx.simulate_mouse_up(option.center(), MouseButton::Left, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(text_of(&h, cx), "@\"日 é.rs\" ");
        assert!(!h.events.borrow().contains(&ComposerEvent::Submit));
        let link_point = h.editor.read_with(cx, |e, _| {
            e.last_bounds.unwrap().origin
                + e.layout.as_ref().unwrap().point_for_offset(2)
                + point(px(2.), px(2.))
        });
        cx.simulate_mouse_down(
            link_point,
            MouseButton::Left,
            gpui::Modifiers {
                control: true,
                ..gpui::Modifiers::none()
            },
        );
        assert_eq!(
            cx.opened_url(),
            Some(mentions::file_url(&project.0.join("日 é.rs")))
        );
    }

    #[gpui::test]
    fn unmarking_ime_text_reopens_completion_without_submitting(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 500.);
        let project = PathProject::new(&["日本.rs"]);
        h.editor
            .update(cx, |e, cx| e.set_project_root(Some(project.0.clone()), cx));
        cx.run_until_parked();
        h.editor.update_in(cx, |e, w, cx| {
            e.set_text("@", cx);
            e.replace_and_mark_text_in_range(None, "日本", None, w, cx);
            assert!(e.path_matches.is_empty());
            assert!(!e.complete_path(0, cx));
            e.unmark_text(w, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            h.editor.read_with(cx, |e, _| e.path_matches.clone()),
            ["日本.rs"]
        );
        cx.simulate_keystrokes("tab");
        assert_eq!(text_of(&h, cx), "@日本.rs ");
        assert!(!h.events.borrow().contains(&ComposerEvent::Submit));
    }

    #[gpui::test]
    fn typing_selection_and_clipboard(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        cx.simulate_input("hello world");
        assert_eq!(text_of(&h, cx), "hello world");
        cx.simulate_keystrokes("ctrl-a ctrl-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("hello world")
        );
        cx.simulate_keystrokes("ctrl-end shift-left shift-left shift-left");
        assert_eq!(
            h.editor
                .read_with(cx, |e, _| e.model().selected_text().to_string()),
            "rld"
        );
        cx.simulate_keystrokes("ctrl-x");
        assert_eq!(text_of(&h, cx), "hello wo");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("rld")
        );
        cx.simulate_keystrokes("ctrl-v ctrl-v");
        assert_eq!(text_of(&h, cx), "hello worldrld");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(
            text_of(&h, cx),
            "hello worldrld".strip_suffix("rld").unwrap()
        );
        cx.simulate_keystrokes("ctrl-shift-z");
        assert_eq!(text_of(&h, cx), "hello worldrld");
        cx.simulate_keystrokes("ctrl-backspace");
        assert_eq!(text_of(&h, cx), "hello ");
        let changed = h
            .events
            .borrow()
            .iter()
            .filter(|e| **e == ComposerEvent::Changed)
            .count();
        assert!(changed >= 8, "edits emit Changed, got {changed}");
    }

    #[gpui::test]
    fn pasting_an_image_emits_an_attachment_event_without_inserting_text(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, vec![137, 80, 78, 71]);
        cx.write_to_clipboard(ClipboardItem {
            entries: vec![image.clone().into(), "fallback text".to_string().into()],
        });
        cx.simulate_keystrokes("ctrl-v");
        assert_eq!(text_of(&h, cx), "");
        assert!(
            h.events
                .borrow()
                .contains(&ComposerEvent::PasteImage(image))
        );
    }

    #[gpui::test]
    fn pasted_crlf_is_normalized(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        cx.write_to_clipboard(ClipboardItem::new_string("a\r\nb\rc".into()));
        cx.simulate_keystrokes("ctrl-v");
        assert_eq!(text_of(&h, cx), "a\nb\nc");
    }

    #[gpui::test]
    fn enter_shift_enter_and_marked_text(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        cx.simulate_input("hi");
        cx.simulate_keystrokes("enter");
        assert_eq!(h.events.borrow().last(), Some(&ComposerEvent::Submit));
        h.events.borrow_mut().clear();
        cx.simulate_keystrokes("shift-enter");
        assert_eq!(text_of(&h, cx), "hi\n");
        assert!(!h.events.borrow().contains(&ComposerEvent::Submit));
        h.events.borrow_mut().clear();

        // IME composition: Enter must not submit or insert anything.
        h.editor.update_in(cx, |e, w, cx| {
            e.replace_and_mark_text_in_range(None, "にほ", Some(2..2), w, cx)
        });
        assert!(h.editor.read_with(cx, |e, _| e.is_composing()));
        h.events.borrow_mut().clear();
        cx.simulate_keystrokes("enter");
        assert!(!h.events.borrow().contains(&ComposerEvent::Submit));
        assert_eq!(text_of(&h, cx), "hi\nにほ");
        cx.simulate_keystrokes("shift-enter");
        assert_eq!(text_of(&h, cx), "hi\nにほ");

        // Commit, then Enter submits again.
        h.editor
            .update_in(cx, |e, w, cx| e.replace_text_in_range(None, "日本", w, cx));
        assert!(!h.editor.read_with(cx, |e, _| e.is_composing()));
        assert_eq!(text_of(&h, cx), "hi\n日本");
        h.events.borrow_mut().clear();
        cx.simulate_keystrokes("enter");
        assert_eq!(h.events.borrow().as_slice(), &[ComposerEvent::Submit]);
    }

    #[gpui::test]
    fn ime_commit_cancel_and_undo(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        cx.simulate_input("ab");
        // Cancel leaves text untouched.
        h.editor.update_in(cx, |e, w, cx| {
            e.replace_and_mark_text_in_range(None, "x", None, w, cx);
            e.replace_and_mark_text_in_range(None, "", None, w, cx);
        });
        assert_eq!(text_of(&h, cx), "ab");
        assert!(!h.editor.read_with(cx, |e, _| e.is_composing()));
        // Commit is one undo step on top of the typed run.
        h.editor.update_in(cx, |e, w, cx| {
            e.replace_and_mark_text_in_range(None, "ほ", Some(1..1), w, cx);
            e.replace_and_mark_text_in_range(None, "ほん", Some(2..2), w, cx);
            e.replace_text_in_range(None, "本", w, cx);
        });
        assert_eq!(text_of(&h, cx), "ab本");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text_of(&h, cx), "ab");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text_of(&h, cx), "");
        // Platform queries report UTF-16 ranges.
        h.editor.update_in(cx, |e, w, cx| {
            e.replace_text_in_range(None, "a😀b", w, cx);
            let sel = e.selected_text_range(false, w, cx).unwrap();
            assert_eq!(sel.range, 4..4);
            let mut used = None;
            assert_eq!(
                e.text_for_range(1..3, &mut used, w, cx).as_deref(),
                Some("😀")
            );
            assert_eq!(used, Some(1..3));
        });
    }

    #[gpui::test]
    fn set_text_resets_history_and_is_silent(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        cx.simulate_input("typed");
        h.events.borrow_mut().clear();
        h.editor
            .update(cx, |e, cx| e.set_text("restored draft", cx));
        assert_eq!(text_of(&h, cx), "restored draft");
        assert!(
            h.events.borrow().is_empty(),
            "set_text must not emit Changed"
        );
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text_of(&h, cx), "restored draft", "history was reset");
        cx.simulate_input("!");
        assert_eq!(text_of(&h, cx), "restored draft!");
        assert_eq!(h.events.borrow().as_slice(), &[ComposerEvent::Changed]);
    }

    #[gpui::test]
    fn disabled_is_read_only(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        cx.simulate_input("keep");
        h.editor.update(cx, |e, cx| e.set_disabled(true, cx));
        cx.simulate_input("nope");
        cx.simulate_keystrokes("backspace enter");
        assert_eq!(text_of(&h, cx), "keep");
        assert!(!h.events.borrow().contains(&ComposerEvent::Submit));
        let accepts = h
            .editor
            .update_in(cx, |e, w, cx| e.accepts_text_input(w, cx));
        assert!(!accepts);
        h.editor.update(cx, |e, cx| e.set_disabled(false, cx));
        cx.simulate_input("!");
        assert_eq!(text_of(&h, cx), "keep!");
    }

    #[gpui::test]
    fn escape_emits_event(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        cx.simulate_keystrokes("escape");
        assert_eq!(h.events.borrow().as_slice(), &[ComposerEvent::Escape]);
    }

    #[gpui::test]
    fn single_line_mode(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, true, 200.);
        cx.write_to_clipboard(ClipboardItem::new_string("one\ntwo".into()));
        cx.simulate_keystrokes("ctrl-v");
        assert_eq!(text_of(&h, cx), "one two");
        cx.simulate_keystrokes("shift-enter");
        assert_eq!(text_of(&h, cx), "one two");
        h.events.borrow_mut().clear();
        cx.simulate_keystrokes("up down enter");
        assert_eq!(
            h.events.borrow().as_slice(),
            &[
                ComposerEvent::Up,
                ComposerEvent::Down,
                ComposerEvent::Submit
            ]
        );
    }

    #[gpui::test]
    fn grows_then_scrolls_at_max_lines(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        let lh = cx.update(|_, cx| cx.theme().body_line_height());
        let height = |h: &Harness, cx: &mut VisualTestContext| {
            h.editor
                .read_with(cx, |e, _| e.last_bounds.map(|b| b.size.height))
        };
        h.editor.update(cx, |e, cx| e.set_text("one", cx));
        cx.run_until_parked();
        assert_eq!(height(&h, cx), Some(lh));
        h.editor.update(cx, |e, cx| e.set_text("1\n2\n3", cx));
        cx.run_until_parked();
        assert_eq!(height(&h, cx), Some(lh * 3.));
        let many = (0..30)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        h.editor.update(cx, |e, cx| e.set_text(&many, cx));
        cx.run_until_parked();
        assert_eq!(height(&h, cx), Some(lh * DEFAULT_MAX_LINES as f32));
        let (content, scroll) = h
            .editor
            .read_with(cx, |e, _| (e.content_height(), e.scroll_y));
        assert_eq!(content, lh * 30.);
        // Caret at the end is kept visible.
        assert_eq!(scroll, lh * 30. - lh * DEFAULT_MAX_LINES as f32);
        // Moving to the top scrolls back.
        cx.simulate_keystrokes("ctrl-home");
        assert_eq!(h.editor.read_with(cx, |e, _| e.scroll_y), px(0.));
    }

    #[gpui::test]
    fn up_down_across_wrapped_lines_keeps_goal_column(cx: &mut TestAppContext) {
        // Narrow width forces wrapping of the long first paragraph.
        let (h, cx) = harness(cx, false, 90.);
        let long = "alpha beta gamma delta epsilon zeta eta theta";
        h.editor
            .update(cx, |e, cx| e.set_text(&format!("{long}\nx\n{long}"), cx));
        cx.run_until_parked();
        let rows = |h: &Harness, cx: &mut VisualTestContext| {
            h.editor.read_with(cx, |e, _| {
                e.layout.as_ref().map(|l| l.height / l.line_height)
            })
        };
        let total_rows = rows(&h, cx).unwrap();
        assert!(
            total_rows > 3.0,
            "text wrapped into several rows ({total_rows})"
        );

        cx.simulate_keystrokes("ctrl-end");
        let end = h.editor.read_with(cx, |e, _| e.model().head());
        let y_end = h.editor.read_with(cx, |e, _| {
            e.layout
                .as_ref()
                .unwrap()
                .point_for_offset(e.model().head())
                .y
        });
        cx.simulate_keystrokes("up");
        let (head, y) = h.editor.read_with(cx, |e, _| {
            let l = e.layout.as_ref().unwrap();
            (e.model().head(), l.point_for_offset(e.model().head()).y)
        });
        assert!(head < end, "up moved the caret back");
        assert_eq!(
            y,
            y_end
                - h.editor
                    .read_with(cx, |e, _| e.layout.as_ref().unwrap().line_height)
        );
        // Walk to the top visual row, then one more Up goes to document start.
        for _ in 0..20 {
            cx.simulate_keystrokes("up");
        }
        assert_eq!(h.editor.read_with(cx, |e, _| e.model().head()), 0);
        // Down through the short middle line and back: the goal column is remembered.
        cx.simulate_keystrokes("end");
        let first_row_end = h.editor.read_with(cx, |e, _| e.model().head());
        assert!(
            first_row_end > 0 && first_row_end < long.len(),
            "End stops at the end of the wrapped visual row"
        );
        for _ in 0..30 {
            cx.simulate_keystrokes("down");
        }
        assert_eq!(
            h.editor.read_with(cx, |e, _| e.model().head()),
            long.len() * 2 + 3,
            "down past the last row goes to the end"
        );
    }

    #[gpui::test]
    fn vertical_motion_remembers_goal_column(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 400.);
        h.editor
            .update(cx, |e, cx| e.set_text("abcdefgh\nxy\nabcdefgh", cx));
        cx.run_until_parked();
        h.editor.update(cx, |e, _| e.model.set_selection(6, 6));
        cx.simulate_keystrokes("down");
        assert_eq!(
            h.editor.read_with(cx, |e, _| e.model().head()),
            9 + 2,
            "clamped to end of short line"
        );
        cx.simulate_keystrokes("down");
        assert_eq!(
            h.editor.read_with(cx, |e, _| e.model().head()),
            12 + 6,
            "goal column restored"
        );
        cx.simulate_keystrokes("up up");
        assert_eq!(h.editor.read_with(cx, |e, _| e.model().head()), 6);
        // A horizontal move resets the goal column.
        cx.simulate_keystrokes("right down down");
        assert_eq!(h.editor.read_with(cx, |e, _| e.model().head()), 12 + 7);
    }

    #[gpui::test]
    fn shift_arrows_extend_across_lines(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 400.);
        h.editor
            .update(cx, |e, cx| e.set_text("one\ntwo\nthree", cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-home shift-down shift-down shift-end");
        assert_eq!(
            h.editor
                .read_with(cx, |e, _| e.model().selected_text().to_string()),
            "one\ntwo\nthree"
        );
        cx.simulate_keystrokes("shift-up");
        assert!(
            h.editor
                .read_with(cx, |e, _| e.model().selected_text().len())
                < 13
        );
    }

    #[gpui::test]
    fn bounds_for_range_tracks_caret(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        cx.simulate_input("ab\ncd");
        cx.run_until_parked();
        let (first, second) = h.editor.update_in(cx, |e, w, cx| {
            let eb = e.last_bounds.unwrap();
            (
                e.bounds_for_range(0..1, eb, w, cx).unwrap(),
                e.bounds_for_range(3..4, eb, w, cx).unwrap(),
            )
        });
        assert!(
            second.origin.y > first.origin.y,
            "second line is below the first"
        );
        assert_eq!(second.origin.x, first.origin.x);
        let idx = h.editor.update_in(cx, |e, w, cx| {
            let mid = point(first.origin.x + px(1.), first.origin.y + px(2.));
            e.character_index_for_point(mid, w, cx)
        });
        assert_eq!(idx, Some(0));
    }

    #[gpui::test]
    fn the_input_method_is_told_where_the_caret_is_whenever_it_moves(cx: &mut TestAppContext) {
        let (h, cx) = harness(cx, false, 300.);
        cx.simulate_input("ab\ncd");
        cx.run_until_parked();
        let at_end = h.editor.read_with(cx, |e, _| e.ime_caret);
        assert!(
            at_end.is_some(),
            "the caret's place was handed to the input method"
        );
        cx.simulate_keystrokes("ctrl-home");
        cx.run_until_parked();
        let at_start = h.editor.read_with(cx, |e, _| e.ime_caret);
        assert_ne!(at_start, at_end, "it follows the caret to the start");
        assert!(at_start.unwrap().origin.y < at_end.unwrap().origin.y);
        // Typing moves it along the line.
        cx.simulate_input("xyz");
        cx.run_until_parked();
        let after_typing = h.editor.read_with(cx, |e, _| e.ime_caret);
        assert!(after_typing.unwrap().origin.x > at_start.unwrap().origin.x);
    }

    #[test]
    fn the_text_a_reader_sees_has_a_run_per_line_and_the_caret_in_it() {
        let t = a11y_text("hello world\nsecond line", 0, 5);
        assert_eq!(t.runs.len(), 2);
        assert_eq!(
            t.runs[0].text, "hello world\n",
            "the line break belongs to the line it ends"
        );
        assert_eq!(t.runs[1].text, "second line");
        assert_eq!(t.runs[0].character_lengths.len(), 12);
        assert_eq!(t.runs[0].word_starts, [0, 6]);
        assert_eq!(t.runs[1].word_starts, [0, 7]);
        assert_eq!((t.anchor, t.focus), ((0, 0), (0, 5)));
        // The caret at the end of the first line is before its break; at the start of the next
        // line it is on the second run.
        assert_eq!(a11y_text("ab\ncd", 2, 2).focus, (0, 2));
        assert_eq!(a11y_text("ab\ncd", 3, 3).focus, (1, 0));
        assert_eq!(a11y_text("ab\ncd", 5, 5).focus, (1, 2));
    }

    #[test]
    fn multibyte_text_is_counted_in_characters_with_their_byte_lengths() {
        let text = "h\u{e9}llo \u{4f60}\u{597d} \u{1f680}";
        let t = a11y_text(text, text.len(), text.len());
        assert_eq!(t.runs.len(), 1);
        let lengths = &t.runs[0].character_lengths;
        assert_eq!(
            lengths.iter().map(|l| *l as usize).sum::<usize>(),
            text.len()
        );
        assert_eq!(lengths.len(), text.chars().count());
        assert_eq!(
            t.focus,
            (0, text.chars().count()),
            "the end of the text, in characters"
        );
        // A caret inside the first multibyte letter's neighbour.
        assert_eq!(
            a11y_text(text, 3, 3).focus,
            (0, 2),
            "after h and the two-byte e-acute"
        );
    }

    #[test]
    fn empty_text_still_has_one_empty_run_and_the_caret_at_its_start() {
        let t = a11y_text("", 0, 0);
        assert_eq!(
            t,
            A11yText {
                runs: vec![A11yRun {
                    text: String::new(),
                    character_lengths: vec![],
                    word_starts: vec![]
                }],
                anchor: (0, 0),
                focus: (0, 0),
            }
        );
        // Offsets past the end or inside a character never panic.
        let _ = a11y_text("\u{1f680}", 99, 2);
        assert_eq!(
            a11y_text("a\n\nb", 3, 3).focus,
            (2, 0),
            "a blank line is a run of its own"
        );
    }

    #[test]
    fn the_selection_runs_from_its_anchor_to_the_caret_in_either_direction() {
        let t = a11y_text("one two three", 4, 7);
        assert_eq!((t.anchor, t.focus), ((0, 4), (0, 7)));
        let t = a11y_text("one two three", 7, 4);
        assert_eq!((t.anchor, t.focus), ((0, 7), (0, 4)));
    }
}
