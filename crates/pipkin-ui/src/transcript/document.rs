//! `TranscriptDocument`: stable addressing, a document-level selection and copy that do not
//! depend on which rows are mounted. No GPUI.
//!
//! A position is `(ItemId, block index, byte offset)`. Blocks come from `blocks()`: markdown
//! blocks for assistant messages, one paragraph for user prompts and notices, and for tool rows
//! the input/output blocks *only while expanded* (a collapsed tool row has no selectable text).
//! Selection survives streaming: positions are by item id, and offsets are clamped whenever they
//! are resolved against the current content.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use pipkin_core::{ItemId, ItemKind, TranscriptItem};
use unicode_segmentation::UnicodeSegmentation;

use super::markdown::{self, Block, BlockKind};

pub type Blocks = Arc<Vec<Block>>;

/// Language tags used for tool-row blocks so the view can style them.
pub const TOOL_INPUT: &str = "tool-input";
pub const TOOL_OUTPUT: &str = "tool-output";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocPos {
    pub item: ItemId,
    pub block: u32,
    pub offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: DocPos,
    pub head: DocPos,
}

impl Selection {
    pub fn caret(pos: DocPos) -> Self {
        Selection {
            anchor: pos,
            head: pos,
        }
    }
}

/// A selection resolved against the current items: indices instead of ids, ordered, clamped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub start: (usize, u32, usize),
    pub end: (usize, u32, usize),
}

impl Resolved {
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The selected byte range inside block `(item_ix, block)` of length `len`, if any.
    pub fn range_in(&self, item_ix: usize, block: u32, len: usize) -> Option<Range<usize>> {
        let here = (item_ix, block);
        if here < (self.start.0, self.start.1) || here > (self.end.0, self.end.1) {
            return None;
        }
        let s = if here == (self.start.0, self.start.1) {
            self.start.2
        } else {
            0
        };
        let e = if here == (self.end.0, self.end.1) {
            self.end.2
        } else {
            len
        };
        let (s, e) = (s.min(len), e.min(len));
        (s < e).then_some(s..e)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Left,
    Right,
    /// To the start of the previous / end of the next block (fallback for vertical movement).
    BlockUp,
    BlockDown,
    DocStart,
    DocEnd,
}

type Key = (ItemId, u64);

struct CacheEntry {
    blocks: Blocks,
    tick: u64,
    kind: std::mem::Discriminant<ItemKind>,
    expanded: bool,
}

struct Cache {
    map: HashMap<Key, CacheEntry>,
    /// The live key of each item, so a streaming item never accumulates stale revisions.
    latest: HashMap<ItemId, Key>,
    tick: u64,
    cap: usize,
}

pub struct Document {
    cache: RefCell<Cache>,
    parses: Cell<usize>,
}

impl Default for Document {
    fn default() -> Self {
        Self::with_capacity(2000)
    }
}

fn revision(item: &TranscriptItem, expanded: bool) -> u64 {
    let (tag, a, b) = match &item.kind {
        ItemKind::User { text, .. } => (1u64, text.len(), 0),
        ItemKind::Assistant { text, .. } => (2, text.len(), 0),
        ItemKind::Tool(t) => (3, t.input.len(), t.output.len()),
        ItemKind::Notice { text, .. } => (4, text.len(), 0),
        ItemKind::Background { text, .. } => (5, text.len(), 0),
        ItemKind::Job(job) => (6, job.command.len(), job.state as usize),
    };
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (tag, expanded, a, b).hash(&mut h);
    // Snapshots can replace text without changing its length (including table alignment).
    match &item.kind {
        ItemKind::User { text, .. }
        | ItemKind::Assistant { text, .. }
        | ItemKind::Notice { text, .. }
        | ItemKind::Background { text, .. } => text.hash(&mut h),
        ItemKind::Tool(t) => {
            t.input.hash(&mut h);
            t.output.hash(&mut h);
        }
        ItemKind::Job(job) => job.command.hash(&mut h),
    }
    h.finish()
}

fn build(item: &TranscriptItem, expanded: bool) -> Vec<Block> {
    match &item.kind {
        ItemKind::User { text, .. } => vec![Block::plain(BlockKind::Paragraph, text.clone())],
        ItemKind::Notice { text, .. } => vec![Block::plain(BlockKind::Paragraph, text.clone())],
        ItemKind::Background { text, .. } => {
            if expanded {
                vec![Block::plain(BlockKind::Paragraph, text.clone())]
            } else {
                vec![]
            }
        }
        ItemKind::Job(job) => {
            if expanded {
                vec![Block::plain(BlockKind::Paragraph, job.command.clone())]
            } else {
                vec![]
            }
        }
        ItemKind::Assistant { text, .. } => markdown::parse(text),
        ItemKind::Tool(t) => {
            let mut v = Vec::new();
            if expanded {
                if !t.input.is_empty() {
                    v.push(Block::plain(
                        BlockKind::Code {
                            lang: Some(TOOL_INPUT.into()),
                        },
                        t.input.clone(),
                    ));
                }
                if !t.output.is_empty() {
                    v.push(Block::plain(
                        BlockKind::Code {
                            lang: Some(TOOL_OUTPUT.into()),
                        },
                        t.output.clone(),
                    ));
                }
            }
            v
        }
    }
}

impl Document {
    pub fn with_capacity(cap: usize) -> Self {
        Document {
            cache: RefCell::new(Cache {
                map: HashMap::new(),
                latest: HashMap::new(),
                tick: 0,
                cap,
            }),
            parses: Cell::new(0),
        }
    }

    pub fn cached_blocks(&self) -> usize {
        self.cache.borrow().map.len()
    }

    /// Number of times content was actually parsed (not served from the cache).
    pub fn parse_count(&self) -> usize {
        self.parses.get()
    }

    pub fn has_item(&self, id: ItemId) -> bool {
        self.cache.borrow().latest.contains_key(&id)
    }

    /// Reconcile an authoritative snapshot outside render. Retain unchanged parses and
    /// refresh only the bounded working set before invalidating any measured row heights.
    /// Deleted identities leave the cache; loading history must not parse the entire history.
    pub fn refresh_cached(&self, items: &[TranscriptItem], expanded: &HashSet<ItemId>) {
        let live: HashSet<_> = items.iter().map(|item| item.id).collect();
        let wanted = {
            let mut cache = self.cache.borrow_mut();
            cache.map.retain(|(id, _), _| live.contains(id));
            cache.latest.retain(|id, _| live.contains(id));
            cache.latest.clone()
        };
        for item in items {
            let expanded = expanded.contains(&item.id);
            if let Some(key) = wanted.get(&item.id)
                && key.1 != revision(item, expanded)
            {
                self.blocks(item, expanded);
            }
        }
        // Unchanged entries keep their LRU order: receiving a snapshot is not reading
        // every cached row, particularly while the person is looking at older history.
    }

    /// Keep the previously prepared presentation until a deferred cache miss is ready.
    /// Never substitute an expanded/collapsed tool or a different kind of message.
    pub fn previous(&self, item: &TranscriptItem, expanded: bool) -> Option<Blocks> {
        let cache = self.cache.borrow();
        let entry = cache.map.get(cache.latest.get(&item.id)?)?;
        (entry.expanded == expanded && entry.kind == std::mem::discriminant(&item.kind))
            .then(|| entry.blocks.clone())
    }

    /// Render-time lookup only: never parse on a cache miss.
    pub fn cached(&self, item: &TranscriptItem, expanded: bool) -> Option<Blocks> {
        let key = (item.id, revision(item, expanded));
        let mut cache = self.cache.borrow_mut();
        cache.tick += 1;
        let tick = cache.tick;
        cache.map.get_mut(&key).map(|entry| {
            entry.tick = tick;
            entry.blocks.clone()
        })
    }

    /// Resolve a painted selection only after both endpoints have been prepared.
    pub fn resolve_cached(
        &self,
        items: &[TranscriptItem],
        expanded: &HashSet<ItemId>,
        sel: &Selection,
    ) -> Option<Resolved> {
        for pos in [sel.anchor, sel.head] {
            let item = &items[Self::index_of(items, pos.item)?];
            self.cached(item, expanded.contains(&item.id))?;
        }
        self.resolve(items, expanded, sel)
    }

    /// Parsed blocks for an item, cached by `(id, content revision)`.
    pub fn blocks(&self, item: &TranscriptItem, expanded: bool) -> Blocks {
        self.blocks_inner(item, expanded, true)
    }

    fn blocks_inner(&self, item: &TranscriptItem, expanded: bool, insert: bool) -> Blocks {
        let key = (item.id, revision(item, expanded));
        {
            let mut c = self.cache.borrow_mut();
            c.tick += 1;
            let tick = c.tick;
            if let Some(e) = c.map.get_mut(&key) {
                e.tick = tick;
                return e.blocks.clone();
            }
        }
        self.parses.set(self.parses.get() + 1);
        let blocks: Blocks = Arc::new(build(item, expanded));
        if insert {
            let mut c = self.cache.borrow_mut();
            if let Some(old) = c.latest.insert(item.id, key)
                && old != key
            {
                c.map.remove(&old);
            }
            let tick = c.tick;
            c.map.insert(
                key,
                CacheEntry {
                    blocks: blocks.clone(),
                    tick,
                    kind: std::mem::discriminant(&item.kind),
                    expanded,
                },
            );
            if c.map.len() > c.cap {
                // Evict the least recently used tenth.
                let mut ticks: Vec<u64> = c.map.values().map(|e| e.tick).collect();
                ticks.sort_unstable();
                let cutoff = ticks[(ticks.len() / 10).max(1) - 1];
                let dead: Vec<Key> = c
                    .map
                    .iter()
                    .filter(|(_, e)| e.tick <= cutoff)
                    .map(|(k, _)| *k)
                    .collect();
                for k in dead {
                    c.map.remove(&k);
                    if c.latest.get(&k.0) == Some(&k) {
                        c.latest.remove(&k.0);
                    }
                }
            }
        }
        blocks
    }

    /// Blocks for copy/navigation: served from the cache when present, otherwise parsed without
    /// polluting it (a 10,000-message select-all must not evict the working set).
    fn peek_blocks(&self, item: &TranscriptItem, expanded: bool) -> Blocks {
        self.blocks_inner(item, expanded, false)
    }

    pub fn index_of(items: &[TranscriptItem], id: ItemId) -> Option<usize> {
        items.iter().position(|i| i.id == id)
    }

    fn blocks_at(&self, items: &[TranscriptItem], ix: usize, expanded: &HashSet<ItemId>) -> Blocks {
        let item = &items[ix];
        self.peek_blocks(item, expanded.contains(&item.id))
    }

    /// Resolve against current content: indices, clamped to existing blocks and char boundaries.
    pub fn resolve(
        &self,
        items: &[TranscriptItem],
        expanded: &HashSet<ItemId>,
        sel: &Selection,
    ) -> Option<Resolved> {
        let a = self.resolve_pos(items, expanded, &sel.anchor)?;
        let h = self.resolve_pos(items, expanded, &sel.head)?;
        let (start, end) = if a <= h { (a, h) } else { (h, a) };
        Some(Resolved { start, end })
    }

    pub fn resolve_pos(
        &self,
        items: &[TranscriptItem],
        expanded: &HashSet<ItemId>,
        p: &DocPos,
    ) -> Option<(usize, u32, usize)> {
        let ix = Self::index_of(items, p.item)?;
        let blocks = self.blocks_at(items, ix, expanded);
        if blocks.is_empty() {
            return Some((ix, 0, 0));
        }
        let b = (p.block as usize).min(blocks.len() - 1);
        let text = &blocks[b].text;
        let mut off = p.offset.min(text.len());
        while !text.is_char_boundary(off) {
            off -= 1;
        }
        Some((ix, b as u32, off))
    }

    pub fn pos_at(items: &[TranscriptItem], (ix, block, offset): (usize, u32, usize)) -> DocPos {
        DocPos {
            item: items[ix].id,
            block,
            offset,
        }
    }

    /// Copy text for the selection, across blocks and messages, including offscreen ones.
    pub fn selected_text(
        &self,
        items: &[TranscriptItem],
        expanded: &HashSet<ItemId>,
        sel: &Selection,
    ) -> String {
        let Some(r) = self.resolve(items, expanded, sel) else {
            return String::new();
        };
        if r.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        let mut prev: Option<(BlockKind, usize)> = None;
        for ix in r.start.0..=r.end.0.min(items.len().saturating_sub(1)) {
            let blocks = self.blocks_at(items, ix, expanded);
            for (bi, block) in blocks.iter().enumerate() {
                let range = r.range_in(ix, bi as u32, block.text.len());
                // Empty cells and cell-boundary endpoints still contribute delimiters.
                let table_cell = matches!(block.kind, BlockKind::TableCell(_))
                    && (ix, bi as u32) >= (r.start.0, r.start.1)
                    && (ix, bi as u32) <= (r.end.0, r.end.1);
                if range.is_none() && !table_cell {
                    continue;
                }
                if let Some((kind, pix)) = &prev {
                    out.push_str(if *pix == ix {
                        markdown::separator(kind, &block.kind)
                    } else {
                        "\n\n"
                    });
                }
                if let Some(range) = range {
                    out.push_str(&block.text[range]);
                }
                prev = Some((block.kind.clone(), ix));
            }
        }
        out
    }

    pub fn select_all(
        &self,
        items: &[TranscriptItem],
        expanded: &HashSet<ItemId>,
    ) -> Option<Selection> {
        let first = self.first_pos(items, expanded)?;
        let last = self.last_pos(items, expanded)?;
        Some(Selection {
            anchor: first,
            head: last,
        })
    }

    fn first_pos(&self, items: &[TranscriptItem], expanded: &HashSet<ItemId>) -> Option<DocPos> {
        (0..items.len()).find_map(|ix| {
            let blocks = self.blocks_at(items, ix, expanded);
            blocks
                .iter()
                .position(|b| !b.text.is_empty() || matches!(b.kind, BlockKind::TableCell(_)))
                .map(|bi| DocPos {
                    item: items[ix].id,
                    block: bi as u32,
                    offset: 0,
                })
        })
    }

    fn last_pos(&self, items: &[TranscriptItem], expanded: &HashSet<ItemId>) -> Option<DocPos> {
        (0..items.len()).rev().find_map(|ix| {
            let blocks = self.blocks_at(items, ix, expanded);
            blocks
                .iter()
                .rposition(|b| !b.text.is_empty() || matches!(b.kind, BlockKind::TableCell(_)))
                .map(|bi| DocPos {
                    item: items[ix].id,
                    block: bi as u32,
                    offset: blocks[bi].text.len(),
                })
        })
    }

    fn next_block(
        &self,
        items: &[TranscriptItem],
        expanded: &HashSet<ItemId>,
        ix: usize,
        b: usize,
    ) -> Option<(usize, usize)> {
        let blocks = self.blocks_at(items, ix, expanded);
        if let Some(n) = (b + 1..blocks.len()).find(|n| !blocks[*n].text.is_empty()) {
            return Some((ix, n));
        }
        (ix + 1..items.len()).find_map(|j| {
            self.blocks_at(items, j, expanded)
                .iter()
                .position(|x| !x.text.is_empty())
                .map(|n| (j, n))
        })
    }

    fn prev_block(
        &self,
        items: &[TranscriptItem],
        expanded: &HashSet<ItemId>,
        ix: usize,
        b: usize,
    ) -> Option<(usize, usize)> {
        let blocks = self.blocks_at(items, ix, expanded);
        if let Some(n) = (0..b.min(blocks.len()))
            .rev()
            .find(|n| !blocks[*n].text.is_empty())
        {
            return Some((ix, n));
        }
        (0..ix).rev().find_map(|j| {
            self.blocks_at(items, j, expanded)
                .iter()
                .rposition(|x| !x.text.is_empty())
                .map(|n| (j, n))
        })
    }

    /// Move a position. Positions that no longer resolve are returned unchanged.
    pub fn move_pos(
        &self,
        items: &[TranscriptItem],
        expanded: &HashSet<ItemId>,
        pos: DocPos,
        step: Step,
    ) -> DocPos {
        let Some((ix, b, off)) = self.resolve_pos(items, expanded, &pos) else {
            return pos;
        };
        let blocks = self.blocks_at(items, ix, expanded);
        let b = b as usize;
        let text = blocks.get(b).map(|x| x.text.as_str()).unwrap_or("");
        let at = |ix: usize, b: usize, off: usize| DocPos {
            item: items[ix].id,
            block: b as u32,
            offset: off,
        };
        let len_of = |ix: usize, b: usize| self.blocks_at(items, ix, expanded)[b].text.len();
        match step {
            Step::Right => {
                if off < text.len() {
                    let next = text
                        .grapheme_indices(true)
                        .map(|(i, g)| i + g.len())
                        .find(|e| *e > off)
                        .unwrap_or(text.len());
                    at(ix, b, next)
                } else if let Some((nx, nb)) = self.next_block(items, expanded, ix, b) {
                    at(nx, nb, 0)
                } else {
                    at(ix, b, off)
                }
            }
            Step::Left => {
                if off > 0 {
                    let prev = text
                        .grapheme_indices(true)
                        .map(|(i, _)| i)
                        .rev()
                        .find(|i| *i < off)
                        .unwrap_or(0);
                    at(ix, b, prev)
                } else if let Some((px, pb)) = self.prev_block(items, expanded, ix, b) {
                    at(px, pb, len_of(px, pb))
                } else {
                    at(ix, b, 0)
                }
            }
            Step::BlockUp => match self.prev_block(items, expanded, ix, b) {
                Some((px, pb)) => at(px, pb, 0),
                None => at(ix, b, 0),
            },
            Step::BlockDown => match self.next_block(items, expanded, ix, b) {
                Some((nx, nb)) => at(nx, nb, len_of(nx, nb)),
                None => at(ix, b, text.len()),
            },
            Step::DocStart => self.first_pos(items, expanded).unwrap_or(pos),
            Step::DocEnd => self.last_pos(items, expanded).unwrap_or(pos),
        }
    }
}

/// The word (or whitespace/punctuation run) around `offset`.
pub fn word_range(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    let mut last = 0..0;
    for (i, w) in text.split_word_bound_indices() {
        let r = i..i + w.len();
        if offset < r.end {
            return r;
        }
        last = r;
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipkin_core::{Delivery, ToolCall, ToolStatus};

    fn user(id: u64, text: &str) -> TranscriptItem {
        TranscriptItem {
            native_only: false,
            id: ItemId(id),
            at: 0,
            kind: ItemKind::User {
                text: text.into(),
                attachments: vec![],
                delivery: Delivery::Sent,
                steer: false,
            },
        }
    }

    fn asst(id: u64, text: &str) -> TranscriptItem {
        TranscriptItem {
            native_only: false,
            id: ItemId(id),
            at: 0,
            kind: ItemKind::Assistant {
                text: text.into(),
                streaming: false,
            },
        }
    }

    fn tool(id: u64, input: &str, output: &str) -> TranscriptItem {
        TranscriptItem {
            native_only: false,
            id: ItemId(id),
            at: 0,
            kind: ItemKind::Tool(ToolCall {
                call_ref: None,
                call_id: None,
                name: "bash".into(),
                input: input.into(),
                output: output.into(),
                truncated: false,
                full_len: output.len(),
                status: ToolStatus::Ok,
            }),
        }
    }

    fn pos(item: u64, block: u32, offset: usize) -> DocPos {
        DocPos {
            item: ItemId(item),
            block,
            offset,
        }
    }

    #[test]
    fn snapshots_refresh_only_cached_items_and_prune_removed_identities() {
        let doc = Document::with_capacity(4);
        let ex = HashSet::new();
        let mut items: Vec<_> = (0..10_000)
            .map(|id| asst(id, markdown::TABLE_EXAMPLE))
            .collect();
        doc.refresh_cached(&items, &ex);
        assert_eq!(doc.parse_count(), 0, "cold history stays lazy");
        let unchanged = doc.blocks(&items[9998], false);
        doc.blocks(&items[9999], false);
        items[9999] = asst(9999, "| A | B |\n|---|---|\n| new | text |");
        doc.refresh_cached(&items, &ex);
        assert_eq!(
            doc.parse_count(),
            3,
            "only the changed cached item reparses"
        );
        assert_eq!(doc.cached_blocks(), 2);
        assert!(Arc::ptr_eq(
            &unchanged,
            &doc.cached(&items[9998], false).unwrap()
        ));
        assert_eq!(doc.cached(&items[9999], false).unwrap()[2].text, "new");
        items.truncate(9999);
        doc.refresh_cached(&items, &ex);
        assert_eq!(doc.cached_blocks(), 1);
        assert!(!doc.has_item(ItemId(9999)));
        assert_eq!(doc.parse_count(), 3);
    }

    #[test]
    fn unchanged_snapshots_do_not_promote_unread_rows_over_the_readers_working_set() {
        let doc = Document::with_capacity(3);
        let items: Vec<_> = (0..4).map(|id| asst(id, "text")).collect();
        for item in &items[..3] {
            doc.blocks(item, false);
        }
        doc.blocks(&items[0], false); // The person is reading an older message.
        doc.refresh_cached(&items, &HashSet::new());
        doc.blocks(&items[3], false);
        assert!(doc.cached(&items[0], false).is_some());
        assert!(!doc.has_item(ItemId(1)));
    }

    #[test]
    fn previous_presentation_is_scoped_to_item_kind_and_expansion() {
        let doc = Document::default();
        let first = asst(1, "before");
        let previous = doc.blocks(&first, false);
        let next = asst(1, "after");
        assert!(doc.cached(&next, false).is_none());
        assert!(Arc::ptr_eq(&previous, &doc.previous(&next, false).unwrap()));
        assert!(doc.previous(&user(1, "after"), false).is_none());
        assert!(doc.previous(&asst(2, "after"), false).is_none());
        let tool = tool(3, "input", "output");
        doc.blocks(&tool, true);
        assert!(doc.previous(&tool, false).is_none());
        doc.refresh_cached(&[], &HashSet::new());
        assert!(doc.previous(&next, false).is_none());
    }

    #[test]
    fn table_copy_keeps_empty_cells_rows_and_unicode_in_both_directions() {
        let items = vec![asst(1, "| | B |\n|---|---|\n| é | |")];
        let doc = Document::default();
        let ex = HashSet::new();
        let all = doc.select_all(&items, &ex).unwrap();
        assert_eq!(all.anchor, pos(1, 0, 0));
        assert_eq!(all.head, pos(1, 3, 0));
        assert_eq!(doc.selected_text(&items, &ex, &all), "\tB\né\t");
        assert_eq!(
            doc.selected_text(
                &items,
                &ex,
                &Selection {
                    anchor: all.head,
                    head: all.anchor
                }
            ),
            "\tB\né\t"
        );
        let boundary = Selection {
            anchor: pos(1, 1, 1),
            head: pos(1, 2, 0),
        };
        assert_eq!(doc.selected_text(&items, &ex, &boundary), "\n");
        let partial = Selection {
            anchor: pos(1, 2, 1),
            head: pos(1, 2, 2),
        };
        assert_eq!(doc.selected_text(&items, &ex, &partial), "é");
        assert_eq!(
            doc.cached_blocks(),
            0,
            "offscreen copy does not evict the working set"
        );
    }

    #[test]
    fn tables_copy_with_surrounding_prose_and_across_messages() {
        let items = vec![
            asst(1, "Intro\n\n| A | B |\n|---|---|\n| c | d |\n\nAfter"),
            user(2, "Thanks"),
        ];
        let doc = Document::default();
        let ex = HashSet::new();
        let all = doc.select_all(&items, &ex).unwrap();
        assert_eq!(
            doc.selected_text(&items, &ex, &all),
            "Intro\n\nA\tB\nc\td\n\nAfter\n\nThanks"
        );
    }

    #[test]
    fn streaming_table_replaces_cached_revision_and_render_lookups_never_parse() {
        let doc = Document::with_capacity(2);
        let source = markdown::TABLE_EXAMPLE;
        for end in 0..=source.len() {
            let item = asst(1, &source[..end]);
            let before = doc.parse_count();
            assert!(doc.cached(&item, false).is_none());
            assert_eq!(doc.parse_count(), before);
            let parsed = doc.blocks(&item, false);
            assert!(Arc::ptr_eq(&parsed, &doc.cached(&item, false).unwrap()));
            assert_eq!(doc.cached_blocks(), 1);
        }
        let old = asst(1, "| A | B |\n|:--|--:|\n| c | d |");
        let new = asst(1, "| A | B |\n|--:|:--|\n| e | f |");
        doc.blocks(&old, false);
        assert!(
            doc.cached(&new, false).is_none(),
            "same-length replacement must not reuse a table"
        );
        let blocks = doc.blocks(&new, false);
        assert_eq!(blocks[2].text, "e");
        assert!(matches!(
            blocks[0].kind,
            BlockKind::TableCell(markdown::TableCell {
                alignment: markdown::TableAlignment::Right,
                ..
            })
        ));
        assert_eq!(doc.cached_blocks(), 1);
    }

    #[test]
    fn copies_across_messages_code_and_offscreen_rows() {
        let items = vec![
            user(1, "fix the test"),
            asst(2, "Sure.\n\n```rust\nfn a() {}\n```\n\nDone."),
            user(3, "thanks"),
        ];
        let doc = Document::default();
        let ex = HashSet::new();
        let sel = Selection {
            anchor: pos(1, 0, 4),
            head: pos(3, 0, 3),
        };
        let text = doc.selected_text(&items, &ex, &sel);
        assert_eq!(
            text,
            "the test\n\nSure.\n\nfn a() {}\n\nDone.\n\nthx".replace("thx", "tha")
        );
        // Reversed selection gives the same text.
        let rev = Selection {
            anchor: sel.head,
            head: sel.anchor,
        };
        assert_eq!(doc.selected_text(&items, &ex, &rev), text);
    }

    #[test]
    fn list_items_join_with_single_newline() {
        let items = vec![asst(1, "- a\n- b\n- c")];
        let doc = Document::default();
        let sel = doc.select_all(&items, &HashSet::new()).unwrap();
        assert_eq!(
            doc.selected_text(&items, &HashSet::new(), &sel),
            "• a\n• b\n• c"
        );
    }

    #[test]
    fn collapsed_tool_rows_contribute_nothing_expanded_rows_do() {
        let items = vec![user(1, "go"), tool(2, "cargo test", "ok"), user(3, "end")];
        let doc = Document::default();
        let sel = Selection {
            anchor: pos(1, 0, 0),
            head: pos(3, 0, 3),
        };
        assert_eq!(
            doc.selected_text(&items, &HashSet::new(), &sel),
            "go\n\nend"
        );
        let ex: HashSet<_> = [ItemId(2)].into_iter().collect();
        assert_eq!(
            doc.selected_text(&items, &ex, &sel),
            "go\n\ncargo test\n\nok\n\nend"
        );
    }

    #[test]
    fn selection_survives_streaming_growth_and_clamps_on_shrink() {
        let mut items = vec![user(1, "go"), asst(2, "hello wor")];
        let doc = Document::default();
        let ex = HashSet::new();
        let sel = Selection {
            anchor: pos(1, 0, 0),
            head: pos(2, 0, 9),
        };
        assert!(doc.selected_text(&items, &ex, &sel).ends_with("hello wor"));
        if let ItemKind::Assistant { text, .. } = &mut items[1].kind {
            text.push_str("ld, more\n\nnew para");
        }
        // The head offset is unchanged; the extended text is simply not selected yet.
        assert!(doc.selected_text(&items, &ex, &sel).ends_with("hello wor"));
        // Content that shrinks (block reshaped) clamps instead of panicking.
        if let ItemKind::Assistant { text, .. } = &mut items[1].kind {
            *text = "hi".into();
        }
        assert!(doc.selected_text(&items, &ex, &sel).ends_with("hi"));
        // Unknown item: no selection, no panic.
        let gone = Selection {
            anchor: pos(99, 0, 0),
            head: pos(99, 0, 1),
        };
        assert_eq!(doc.selected_text(&items, &ex, &gone), "");
    }

    #[test]
    fn clamps_to_char_boundaries() {
        let items = vec![user(1, "aé👨‍👩‍👧‍👦z")];
        let doc = Document::default();
        let r = doc
            .resolve_pos(&items, &HashSet::new(), &pos(1, 0, 2))
            .unwrap();
        assert_eq!(r.2, 1);
    }

    #[test]
    fn grapheme_movement_does_not_split_emoji_or_combining_marks() {
        let items = vec![user(1, "e\u{301}👨‍👩‍👧‍👦x"), user(2, "next")];
        let doc = Document::default();
        let ex = HashSet::new();
        let p = doc.move_pos(&items, &ex, pos(1, 0, 0), Step::Right);
        assert_eq!(p.offset, "e\u{301}".len());
        let p = doc.move_pos(&items, &ex, p, Step::Right);
        assert_eq!(p.offset, "e\u{301}👨‍👩‍👧‍👦".len());
        let p = doc.move_pos(&items, &ex, p, Step::Left);
        assert_eq!(p.offset, "e\u{301}".len());
        // Crossing into the next message and back.
        let end = pos(1, 0, "e\u{301}👨‍👩‍👧‍👦x".len());
        assert_eq!(doc.move_pos(&items, &ex, end, Step::Right), pos(2, 0, 0));
        assert_eq!(doc.move_pos(&items, &ex, pos(2, 0, 0), Step::Left), end);
        assert_eq!(
            doc.move_pos(&items, &ex, pos(2, 0, 2), Step::DocStart),
            pos(1, 0, 0)
        );
        assert_eq!(
            doc.move_pos(&items, &ex, pos(1, 0, 0), Step::DocEnd),
            pos(2, 0, 4)
        );
    }

    #[test]
    fn word_ranges() {
        assert_eq!(word_range("hello world", 7), 6..11);
        assert_eq!(word_range("hello world", 5), 5..6);
        assert_eq!(word_range("hello", 5), 0..5);
        assert_eq!(word_range("", 0), 0..0);
    }

    #[test]
    fn cache_reuses_completed_items_and_reparses_only_the_tail() {
        let mut items: Vec<_> = (0..50)
            .map(|i| asst(i, &format!("message **{i}**")))
            .collect();
        let doc = Document::default();
        for it in &items {
            doc.blocks(it, false);
        }
        assert_eq!(doc.parse_count(), 50);
        for _ in 0..5 {
            for it in &items {
                doc.blocks(it, false);
            }
        }
        assert_eq!(doc.parse_count(), 50);
        // Streaming: only the last item changes, only it is reparsed, and no stale revisions linger.
        for n in 0..10 {
            if let ItemKind::Assistant { text, .. } = &mut items[49].kind {
                text.push_str(&format!(" tok{n}"));
            }
            for it in &items {
                doc.blocks(it, false);
            }
        }
        assert_eq!(doc.parse_count(), 60);
        assert_eq!(doc.cached_blocks(), 50);
    }

    #[test]
    fn cache_is_bounded() {
        let doc = Document::with_capacity(100);
        for i in 0..1000 {
            doc.blocks(&asst(i, &format!("m{i}")), false);
        }
        assert!(doc.cached_blocks() <= 100, "{}", doc.cached_blocks());
    }

    #[test]
    fn select_all_over_large_history_does_not_pollute_cache() {
        let items: Vec<_> = (0..10_000).map(|i| asst(i, &format!("para {i}"))).collect();
        let doc = Document::default();
        let ex = HashSet::new();
        let sel = doc.select_all(&items, &ex).unwrap();
        let text = doc.selected_text(&items, &ex, &sel);
        assert!(text.starts_with("para 0\n\npara 1") && text.ends_with("para 9999"));
        assert_eq!(doc.cached_blocks(), 0);
    }
}
