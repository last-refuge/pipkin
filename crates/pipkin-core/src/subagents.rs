//! Subagent directory history and child-activity paging. Pure state transitions: the UI only
//! projects what is here, and every page answer is checked against the selected child and the
//! cursor that asked for it (conversation and generation are checked by `AppState`).

use std::collections::HashMap;

use crate::ids::ItemId;
use crate::model::*;

/// Finished directory rows revealed per "Show more" (and the whole unpaged history bound).
pub const SUBAGENT_HISTORY_PAGE: usize = 100;
/// An unpaged (Pi) child view previews only its newest entries.
pub const SUBAGENT_PREVIEW_WINDOW: usize = 80;
const PREVIEW_CHARS: usize = 4096;
const TOOL_PART_CHARS: usize = 2048;

/// Completed, failed and stopped tasks are history; everything else (including statuses this
/// build does not know) stays current.
pub fn subagent_finished(status: &str) -> bool {
    matches!(status, "done" | "failed" | "aborted")
}

impl SubagentState {
    pub fn finished_count(&self) -> usize {
        self.children
            .iter()
            .filter(|c| subagent_finished(&c.status))
            .count()
    }

    /// Finished rows the directory shows: all revealed pages when paged, at most one page
    /// otherwise (Pi's unchanged bound).
    pub fn history_shown(&self) -> usize {
        let limit = if self.paged {
            self.history
        } else {
            self.history.min(SUBAGENT_HISTORY_PAGE)
        };
        limit.min(self.finished_count())
    }

    pub fn can_show_more_history(&self) -> bool {
        self.paged && self.finished_count() > self.history
    }

    pub fn can_load_older(&self) -> bool {
        self.paged
            && self.selected.is_some()
            && self.viewed == self.selected
            && self.has_older
            && self.loading_older.is_none()
            && !self.items.is_empty()
    }

    pub(crate) fn toggle_history(&mut self) {
        self.history = if self.history > 0 {
            0
        } else {
            SUBAGENT_HISTORY_PAGE
        };
    }

    pub(crate) fn show_more_history(&mut self) -> bool {
        if !self.can_show_more_history() {
            return false;
        }
        self.history = (self.history / SUBAGENT_HISTORY_PAGE + 1) * SUBAGENT_HISTORY_PAGE;
        true
    }

    /// The cursor for the next older page, recorded as in flight.
    pub(crate) fn begin_older(&mut self) -> Option<(u64, ItemId)> {
        if !self.can_load_older() {
            return None;
        }
        let before = self.items.first()?.id;
        self.loading_older = Some(before);
        self.older_error = None;
        Some((self.selected?, before))
    }

    /// Selecting a task (or returning to the directory) starts its activity from nothing.
    pub(crate) fn select(&mut self, child: Option<u64>) {
        self.selected = child;
        self.clear_activity();
        self.error = None;
        self.live = false;
        self.loading = false;
    }

    pub(crate) fn clear_activity(&mut self) {
        self.items.clear();
        self.previews.clear();
        self.viewed = None;
        self.has_older = false;
        self.loading_older = None;
        self.older_error = None;
    }

    fn preview(&mut self, item: &TranscriptItem) -> SubagentPreview {
        self.preview_rev += 1;
        let (speaker, text): (&'static str, std::borrow::Cow<'_, str>) = match &item.kind {
            ItemKind::User { text, .. } => ("Task", text.into()),
            ItemKind::Assistant { text, .. } => ("Pipkin", text.into()),
            ItemKind::Tool(tool) => (
                "Tool",
                format!(
                    "{}: {}\n{}",
                    tool.name,
                    tool.input.chars().take(TOOL_PART_CHARS).collect::<String>(),
                    tool.output
                        .chars()
                        .take(TOOL_PART_CHARS)
                        .collect::<String>()
                )
                .into(),
            ),
            ItemKind::Notice { text, .. } => ("Notice", text.into()),
        };
        let clipped = text.chars().nth(PREVIEW_CHARS).is_some();
        SubagentPreview {
            native_only: item.native_only,
            id: item.id,
            rev: self.preview_rev,
            speaker,
            text: text.chars().take(PREVIEW_CHARS).collect::<String>().into(),
            clipped,
        }
    }

    fn previews_of(&mut self, range: std::ops::Range<usize>) -> Vec<SubagentPreview> {
        let items = std::mem::take(&mut self.items);
        let end = range.end.min(items.len());
        let previews = items[range.start.min(end)..end]
            .iter()
            .map(|item| self.preview(item))
            .collect();
        self.items = items;
        previews
    }

    fn settle_view(&mut self, child: u64, live: bool) {
        self.viewed = Some(child);
        self.error = None;
        self.live = live;
        self.loading = false;
    }

    /// An unpaged engine's complete view (Pi, demo): replaces everything and previews only the
    /// newest entries, as before paging existed.
    pub(crate) fn apply_view(&mut self, child: u64, items: Vec<TranscriptItem>, live: bool) {
        self.items = items;
        let start = self.items.len().saturating_sub(SUBAGENT_PREVIEW_WINDOW);
        self.previews = self.previews_of(start..usize::MAX);
        self.has_older = false;
        self.loading_older = None;
        self.older_error = None;
        self.settle_view(child, live);
    }

    /// The newest page of a paged child. A live refresh replaces only the part it covers:
    /// older pages already shown are kept when the page overlaps them. When it does not (more
    /// new entries arrived than one page holds), what was shown cannot be proven contiguous,
    /// so the view restarts from this page.
    pub(crate) fn apply_newest(
        &mut self,
        child: u64,
        page: Vec<TranscriptItem>,
        live: bool,
        has_older: bool,
    ) {
        self.paged = true;
        let keep = match (self.viewed == Some(child), page.first()) {
            (true, Some(first)) => self.items.iter().position(|i| i.id >= first.id),
            _ => None,
        }
        .unwrap_or(0);
        if keep == 0 {
            // A refresh can replace the same newest page while an older read is in flight.
            // Its cursor is still valid when the oldest visible item did not change.
            let same_cursor = self.viewed == Some(child)
                && !self.items.is_empty()
                && self.items.first().map(|i| i.id) == page.first().map(|i| i.id);
            self.items = page;
            self.previews = self.previews_of(0..usize::MAX);
            self.has_older = has_older;
            if !same_cursor {
                // A genuinely replaced range invalidates the outstanding cursor.
                self.loading_older = None;
            }
        } else {
            self.items.truncate(keep);
            self.previews.truncate(keep);
            self.items.extend(page);
            let (modified, _) = stitch_tools(&mut self.items, keep);
            for i in modified {
                let item = self.items[i].clone();
                self.previews[i] = self.preview(&item);
            }
            let tail = self.previews_of(keep..usize::MAX);
            self.previews.extend(tail);
        }
        self.settle_view(child, live);
    }

    /// An older page answering the in-flight request; anything else is stale and ignored.
    /// Only entries strictly older than those shown are added (no duplicates).
    pub(crate) fn apply_older(
        &mut self,
        child: u64,
        before: ItemId,
        page: Vec<TranscriptItem>,
        has_older: bool,
    ) -> Option<usize> {
        if self.selected != Some(child) || self.loading_older != Some(before) {
            return None;
        }
        self.loading_older = None;
        self.older_error = None;
        self.has_older = has_older;
        let oldest = self.items.first().map(|i| i.id);
        let mut items: Vec<TranscriptItem> = page
            .into_iter()
            .filter(|i| oldest.is_none_or(|o| i.id < o))
            .collect();
        let n = items.len();
        items.append(&mut self.items);
        self.items = items;
        let (_, removed) = stitch_tools(&mut self.items, n);
        let mut previews = self.previews_of(0..n);
        let old = std::mem::take(&mut self.previews);
        previews.extend(old.into_iter().filter(|p| !removed.contains(&p.id)));
        self.previews = previews;
        Some(n)
    }

    pub(crate) fn older_failed(&mut self, child: u64, before: ItemId, reason: String) -> bool {
        if self.selected != Some(child) || self.loading_older != Some(before) {
            return false;
        }
        self.loading_older = None;
        self.older_error = Some(reason);
        true
    }
}

/// A tool call and its result can fall on either side of a page boundary. A tool item after
/// `boundary` with the call id of one before it is that call's result: merge it into the earlier
/// item (which keeps its place) and drop it. Returns the merged indices and dropped item ids.
fn stitch_tools(items: &mut Vec<TranscriptItem>, boundary: usize) -> (Vec<usize>, Vec<ItemId>) {
    let calls: HashMap<String, usize> = items[..boundary]
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match &item.kind {
            ItemKind::Tool(t) => t.call_id.clone().map(|id| (id, i)),
            _ => None,
        })
        .collect();
    let (mut modified, mut removed) = (Vec::new(), Vec::new());
    if calls.is_empty() {
        return (modified, removed);
    }
    let mut j = boundary;
    while j < items.len() {
        let earlier = match &items[j].kind {
            ItemKind::Tool(t) => t.call_id.as_ref().and_then(|id| calls.get(id)).copied(),
            _ => None,
        };
        let Some(i) = earlier else {
            j += 1;
            continue;
        };
        let later = items.remove(j);
        items[i].native_only |= later.native_only;
        if let (ItemKind::Tool(done), ItemKind::Tool(t)) = (later.kind, &mut items[i].kind) {
            if done.status != ToolStatus::Running || !done.output.is_empty() {
                t.output = done.output;
                t.truncated = done.truncated;
                t.full_len = done.full_len;
                t.status = done.status;
            }
            if t.input.is_empty() {
                t.input = done.input;
            }
        }
        modified.push(i);
        removed.push(later.id);
    }
    (modified, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(id: u64) -> TranscriptItem {
        TranscriptItem {
            native_only: false,
            id: ItemId(id),
            at: 0,
            kind: ItemKind::Assistant {
                text: format!("entry {id}"),
                streaming: false,
            },
        }
    }

    fn tool(id: u64, call: &str, output: &str, status: ToolStatus) -> TranscriptItem {
        TranscriptItem {
            native_only: false,
            id: ItemId(id),
            at: 0,
            kind: ItemKind::Tool(ToolCall {
                call_ref: None,
                call_id: Some(call.into()),
                name: "read".into(),
                input: if output.is_empty() {
                    "{}".into()
                } else {
                    String::new()
                },
                output: output.into(),
                truncated: false,
                full_len: output.len(),
                status,
            }),
        }
    }

    fn paged() -> SubagentState {
        let mut s = SubagentState {
            paged: true,
            ..SubagentState::default()
        };
        s.select(Some(1));
        s
    }

    fn ids(s: &SubagentState) -> Vec<u64> {
        s.items.iter().map(|i| i.id.0).collect()
    }

    #[test]
    fn previews_track_items_and_every_preparation_has_a_new_revision() {
        let mut s = paged();
        s.apply_newest(1, (10..20).map(text).collect(), true, true);
        assert_eq!(s.previews.len(), 10);
        let revs: Vec<u64> = s.previews.iter().map(|p| p.rev).collect();
        // A live refresh overlapping the last five keeps the first five untouched.
        s.apply_newest(1, (15..22).map(text).collect(), true, true);
        assert_eq!(ids(&s), (10..22).collect::<Vec<_>>());
        assert_eq!(
            s.previews.iter().map(|p| p.id.0).collect::<Vec<_>>(),
            ids(&s)
        );
        assert_eq!(
            s.previews[..5].iter().map(|p| p.rev).collect::<Vec<_>>(),
            revs[..5]
        );
        assert!(s.previews[5..].iter().all(|p| p.rev > revs[9]));
    }

    #[test]
    fn native_history_provenance_survives_subagent_page_stitching_and_previews() {
        let mut state = paged();
        let mut result = tool(30, "c", "native result", ToolStatus::Ok);
        result.native_only = true;
        state.apply_newest(1, vec![result.clone()], false, true);
        state.begin_older().unwrap();
        state.apply_older(
            1,
            ItemId(30),
            vec![tool(21, "c", "", ToolStatus::Running)],
            false,
        );
        assert_eq!(state.items.len(), 1);
        assert!(state.items[0].native_only);
        assert!(state.previews[0].native_only);
        assert!(state.previews[0].text.contains("native result"));
        state.apply_newest(1, vec![result], true, false);
        assert_eq!(state.items.len(), 1);
        assert!(state.previews[0].native_only);
    }

    #[test]
    fn a_boundary_tool_is_merged_once_in_both_directions() {
        let mut s = paged();
        // Newest page starts with the orphan result of a call on the older page.
        s.apply_newest(
            1,
            vec![tool(30, "c", "result", ToolStatus::Ok), text(31)],
            false,
            true,
        );
        s.begin_older().unwrap();
        let n = s.apply_older(
            1,
            ItemId(30),
            vec![text(20), tool(21, "c", "", ToolStatus::Running)],
            false,
        );
        assert_eq!(n, Some(2));
        assert_eq!(ids(&s), vec![20, 21, 31]);
        let ItemKind::Tool(t) = &s.items[1].kind else {
            panic!()
        };
        assert_eq!((t.output.as_str(), t.status), ("result", ToolStatus::Ok));
        assert!(s.previews[1].text.contains("result"));
        assert_eq!(s.previews.len(), 3);
        // A live refresh whose page starts at the result again does not duplicate it.
        s.apply_newest(
            1,
            vec![tool(30, "c", "result", ToolStatus::Ok), text(31), text(32)],
            true,
            true,
        );
        assert_eq!(ids(&s), vec![20, 21, 31, 32]);
        assert_eq!(s.previews.len(), 4);
        assert!(!s.has_older);
    }
}
