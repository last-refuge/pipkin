//! Selectable text blocks: run building (markdown/syntax/selection → `TextRun`s), the
//! `BlockText` element that registers its painted layout for hit testing, and frame stats.
//!
//! The registry is repopulated every frame by the blocks that actually paint, so recycled rows
//! can never be hit-tested with stale geometry.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Element, ElementId, FontStyle, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, SharedString, StrikethroughStyle,
    TextLayout, TextRun, UnderlineStyle, Window, point, px,
};
use pipkin_core::ItemId;

pub(super) fn aligned_link_text(
    id: ElementId,
    styled: gpui::StyledText,
    alignment: TableAlignment,
    ranges: Vec<Range<usize>>,
    activate: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let layout = styled.layout().clone();
    gpui::div()
        .id(id)
        .w_full()
        .cursor_pointer()
        .on_click(move |event, window, cx| {
            let gpui::ClickEvent::Mouse(event) = event else {
                return;
            };
            if event.up.button != gpui::MouseButton::Left {
                return;
            }
            let delta = event.up.position - event.down.position;
            if delta.x.abs() > px(3.) || delta.y.abs() > px(3.) {
                return;
            }
            if let (Ok(down), Ok(up)) = (
                aligned_index(&layout, alignment, event.down.position),
                aligned_index(&layout, alignment, event.up.position),
            ) && let Some(i) = ranges
                .iter()
                .position(|r| r.contains(&down) && r.contains(&up))
            {
                activate(i, window, cx);
            }
        })
        .child(styled)
        .into_any_element()
}

use super::highlight::{self, Token};
use super::markdown::{Block, BlockKind, TableAlignment};

// GPUI paints text alignment but TextLayout's coordinate helpers remain left-origin.
// Translate using each visual row's shaped advance, including wrapped rows.
fn alignment_offset(layout: &TextLayout, alignment: TableAlignment, y: Pixels) -> Pixels {
    if matches!(alignment, TableAlignment::None | TableAlignment::Left) {
        return px(0.);
    }
    let bounds = layout.bounds();
    let height = layout.line_height();
    let mut row = (f32::from((y - bounds.top()).max(px(0.))) / f32::from(height)).floor() as usize;
    for line in layout.line_layouts() {
        let count = line.wrap_boundaries.len() + 1;
        if row >= count {
            row -= count;
            continue;
        }
        let x = |boundary: &gpui::WrapBoundary| {
            line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix]
                .position
                .x
        };
        let start = row
            .checked_sub(1)
            .map(|i| x(&line.wrap_boundaries[i]))
            .unwrap_or(px(0.));
        let end = line
            .wrap_boundaries
            .get(row)
            .map(x)
            .unwrap_or(line.unwrapped_layout.width);
        let spare = bounds.size.width - (end - start);
        return if alignment == TableAlignment::Center {
            spare / 2.
        } else {
            spare
        };
    }
    px(0.)
}

pub(super) fn aligned_index(
    layout: &TextLayout,
    alignment: TableAlignment,
    mut point: Point<Pixels>,
) -> Result<usize, usize> {
    point.x -= alignment_offset(layout, alignment, point.y);
    layout.index_for_position(point)
}

use crate::theme::Theme;

/// One painted block, as of the last frame.
#[derive(Clone)]
pub struct BlockHit {
    pub item: ItemId,
    pub item_ix: usize,
    pub block: u32,
    pub text_len: usize,
    pub layout: TextLayout,
    pub bounds: Bounds<Pixels>,
    pub visible_bounds: Bounds<Pixels>,
    alignment: TableAlignment,
}

impl BlockHit {
    pub fn position_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        let mut point = self.layout.position_for_index(index)?;
        point.x += alignment_offset(
            &self.layout,
            self.alignment,
            point.y + self.layout.line_height() / 2.,
        );
        Some(point)
    }

    pub fn index_for_position(&self, point: Point<Pixels>) -> Result<usize, usize> {
        aligned_index(&self.layout, self.alignment, point)
    }
}

#[derive(Default)]
pub struct HitRegistry {
    pub blocks: Vec<BlockHit>,
    /// Opt-in paint history: tests must inspect intermediate frames, not just settled geometry.
    #[cfg(test)]
    pub recorded_frames: Option<Vec<Vec<BlockHit>>>,
}

pub type SharedRegistry = Rc<RefCell<HitRegistry>>;

impl HitRegistry {
    #[cfg(test)]
    pub(super) fn record_frame(&mut self) {
        if let Some(frames) = &mut self.recorded_frames {
            frames.push(self.blocks.clone());
        }
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
    }

    pub fn find(&self, item: ItemId, block: u32) -> Option<&BlockHit> {
        self.blocks
            .iter()
            .find(|b| b.item == item && b.block == block)
    }

    /// The document offset nearest to a window position. Inside a block this is exact; between
    /// or beyond blocks it snaps to the vertically nearest block (start if the pointer is above
    /// it, end if below).
    pub fn hit_test(&self, position: Point<Pixels>) -> Option<(ItemId, u32, usize)> {
        let mut best: Option<(&BlockHit, f32, f32)> = None;
        for hit in &self.blocks {
            let b = hit.visible_bounds;
            if b.size.width <= px(0.) || b.size.height <= px(0.) {
                continue;
            }
            let dy = if position.y < b.top() {
                f32::from(b.top() - position.y)
            } else if position.y > b.bottom() {
                f32::from(position.y - b.bottom())
            } else {
                0.0
            };
            let dx = if position.x < b.left() {
                f32::from(b.left() - position.x)
            } else if position.x > b.right() {
                f32::from(position.x - b.right())
            } else {
                0.0
            };
            let better = match best {
                None => true,
                Some((_, by, bx)) => dy < by || (dy == by && dx < bx),
            };
            if better {
                best = Some((hit, dy, dx));
            }
        }
        let (hit, ..) = best?;
        let b = hit.bounds;
        let offset = if position.y < b.top() {
            0
        } else if position.y > b.bottom() {
            hit.text_len
        } else {
            match hit.index_for_position(position) {
                Ok(i) | Err(i) => i.min(hit.text_len),
            }
        };
        Some((hit.item, hit.block, offset))
    }
}

/// Element that paints a `StyledText` (or an interactive wrapper around it) and then records
/// the painted layout in the registry.
pub struct BlockText {
    inner: Option<AnyElement>,
    layout: TextLayout,
    registry: SharedRegistry,
    item: ItemId,
    item_ix: usize,
    block: u32,
    text_len: usize,
    alignment: TableAlignment,
    clip: bool,
}

impl BlockText {
    pub fn new(
        layout: TextLayout,
        inner: AnyElement,
        registry: SharedRegistry,
        item: ItemId,
        item_ix: usize,
        block: u32,
        text_len: usize,
    ) -> Self {
        BlockText {
            inner: Some(inner),
            layout,
            registry,
            item,
            item_ix,
            block,
            text_len,
            alignment: TableAlignment::None,
            clip: false,
        }
    }

    pub fn aligned(mut self, alignment: TableAlignment) -> Self {
        self.alignment = alignment;
        self.clip = true;
        self
    }
}

impl IntoElement for BlockText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for BlockText {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        (self.inner.as_mut().unwrap().request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.as_mut().unwrap().prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.as_mut().unwrap().paint(window, cx);
        self.registry.borrow_mut().blocks.push(BlockHit {
            item: self.item,
            item_ix: self.item_ix,
            block: self.block,
            text_len: self.text_len,
            layout: self.layout.clone(),
            bounds,
            visible_bounds: if self.clip {
                bounds.intersect(&window.content_mask().bounds)
            } else {
                bounds
            },
            alignment: self.alignment,
        });
    }
}

/// Fonts/colors a block starts from before spans and tokens are applied.
#[derive(Clone)]
pub struct BaseText {
    pub family: SharedString,
    pub mono: SharedString,
    pub color: Hsla,
    pub weight: FontWeight,
}

fn find_span(block: &Block, at: usize) -> Option<&super::markdown::Span> {
    let ix = block.spans.partition_point(|s| s.range.end <= at);
    block.spans.get(ix).filter(|s| s.range.start <= at)
}

fn find_range<T: Copy>(ranges: &[(Range<usize>, T)], at: usize) -> Option<T> {
    let ix = ranges.partition_point(|(r, _)| r.end <= at);
    ranges
        .get(ix)
        .filter(|(r, _)| r.start <= at)
        .map(|(_, t)| *t)
}

/// Tokens for a code block's language; tool blocks and unknown languages are plain.
pub fn tokens_for(block: &Block) -> Vec<(Range<usize>, Token)> {
    match &block.kind {
        BlockKind::Code { lang: Some(l) } if !l.starts_with("tool-") => {
            highlight::highlight(l, &block.text)
        }
        _ => Vec::new(),
    }
}

/// Build `TextRun`s covering exactly `block.text`.
pub fn build_runs(
    block: &Block,
    theme: &Theme,
    base: &BaseText,
    selection: Option<Range<usize>>,
    file_refs: &[Range<usize>],
) -> Vec<TextRun> {
    let len = block.text.len();
    let tokens = tokens_for(block);
    let mut cuts = BTreeSet::from([0usize, len]);
    for s in &block.spans {
        cuts.insert(s.range.start.min(len));
        cuts.insert(s.range.end.min(len));
    }
    for (r, _) in &tokens {
        cuts.insert(r.start);
        cuts.insert(r.end);
    }
    for r in file_refs {
        cuts.insert(r.start.min(len));
        cuts.insert(r.end.min(len));
    }
    if let Some(s) = &selection {
        cuts.insert(s.start.min(len));
        cuts.insert(s.end.min(len));
    }
    let cuts: Vec<usize> = cuts.into_iter().collect();
    let c = &theme.colors;
    let mut runs = Vec::with_capacity(cuts.len());
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == b {
            continue;
        }
        let mut family = base.family.clone();
        let mut weight = base.weight;
        let mut style = FontStyle::Normal;
        let mut color = base.color;
        let mut background = None;
        let mut underline = None;
        let mut strike = None;
        if let Some(span) = find_span(block, a) {
            let st = &span.style;
            if st.bold {
                weight = FontWeight::SEMIBOLD;
            }
            if st.italic {
                style = FontStyle::Italic;
                if family == base.family {
                    family = crate::assets::ITALIC_FONT.into();
                }
            }
            if st.code {
                family = base.mono.clone();
                background = Some(c.code_bg);
            }
            if st.strike {
                strike = Some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(color),
                });
            }
            if st.link.is_some() {
                color = c.accent;
                underline = Some(UnderlineStyle {
                    color: Some(c.accent),
                    thickness: px(1.),
                    wavy: false,
                });
            }
        }
        if file_refs.iter().any(|r| r.start <= a && a < r.end) {
            color = c.accent;
            underline = Some(UnderlineStyle {
                color: Some(c.accent),
                thickness: px(1.),
                wavy: false,
            });
        }
        if let Some(tok) = find_range(&tokens, a) {
            color = match tok {
                Token::Keyword => c.syn_keyword,
                Token::String => c.syn_string,
                Token::Comment => c.syn_comment,
                Token::Number => c.syn_number,
                Token::Type => c.syn_type,
                Token::Function => c.syn_function,
                Token::Added => c.diff_add_text,
                Token::Removed => c.diff_remove_text,
                Token::Hunk => c.accent,
            };
        }
        if let Some(sel) = &selection
            && sel.start <= a
            && a < sel.end
        {
            background = Some(c.text_selection);
        }
        let mut font = gpui::font(family);
        font.weight = weight;
        font.style = style;
        runs.push(TextRun {
            len: b - a,
            font,
            color,
            background_color: background,
            underline,
            strikethrough: strike,
        });
    }
    runs
}

/// Ring buffer of per-frame transcript paint costs.
#[derive(Default)]
pub struct FrameStats {
    samples: VecDeque<Duration>,
}

const FRAME_SAMPLES: usize = 600;

impl FrameStats {
    pub fn record(&mut self, d: Duration) {
        if self.samples.len() == FRAME_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(d);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn percentile(&self, p: f32) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        let mut v: Vec<_> = self.samples.iter().copied().collect();
        v.sort_unstable();
        let ix = ((v.len() - 1) as f32 * p).round() as usize;
        Some(v[ix])
    }
}

pub fn clamp_point(p: Point<Pixels>, b: Bounds<Pixels>) -> Point<Pixels> {
    point(
        p.x.clamp(b.left(), b.right()),
        p.y.clamp(b.top(), b.bottom()),
    )
}
