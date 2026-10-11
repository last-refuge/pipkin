//! Line diffs for the `edit` tool: a port of jsdiff 8 `diffLines` (its Myers variant, so ties
//! resolve the same way), `createTwoFilesPatch` with `FILE_HEADERS_ONLY`, and the reference's
//! display diff (`edit-diff.ts` `generateDiffString`).

use std::rc::Rc;

/// One run of equal, added or removed lines (`Change` in jsdiff).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub value: String,
    pub added: bool,
    pub removed: bool,
}

/// jsdiff `tokenize` for lines: each line keeps its `\n` (or `\r\n`).
fn tokenize(value: &str) -> Vec<&str> {
    let mut tokens: Vec<&str> = Vec::new();
    let mut start = 0;
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            tokens.push(&value[start..=i]);
            start = i + 1;
        }
        i += 1;
    }
    if start < value.len() {
        tokens.push(&value[start..]);
    }
    tokens
}

/// A run in the edit path, linked to the run before it. Components are shared between paths
/// and freed once no path reaches them (jsdiff relies on the GC for this).
struct Component {
    count: usize,
    added: bool,
    removed: bool,
    previous: Option<Rc<Component>>,
}

impl Drop for Component {
    /// Unlink iteratively, so dropping a long chain does not recurse.
    fn drop(&mut self) {
        let mut next = self.previous.take();
        while let Some(rc) = next {
            match Rc::try_unwrap(rc) {
                Ok(mut component) => next = component.previous.take(),
                Err(_) => break,
            }
        }
    }
}

#[derive(Clone)]
struct PathState {
    old_pos: isize,
    last: Option<Rc<Component>>,
}

/// `diffLines(oldStr, newStr)` with default options.
pub fn diff_lines(old: &str, new: &str) -> Vec<Change> {
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    let old_len = old_tokens.len() as isize;
    let new_len = new_tokens.len() as isize;

    let extract_common = |base: &mut PathState, diagonal: isize| {
        let mut old_pos = base.old_pos;
        let mut new_pos = old_pos - diagonal;
        let mut common = 0;
        while new_pos + 1 < new_len
            && old_pos + 1 < old_len
            && old_tokens[(old_pos + 1) as usize] == new_tokens[(new_pos + 1) as usize]
        {
            new_pos += 1;
            old_pos += 1;
            common += 1;
        }
        if common > 0 {
            base.last = Some(Rc::new(Component {
                count: common,
                added: false,
                removed: false,
                previous: base.last.take(),
            }));
        }
        base.old_pos = old_pos;
        new_pos
    };
    let add_to_path = |path: &PathState, added: bool, removed: bool, inc: isize| {
        let component = match &path.last {
            Some(c) if c.added == added && c.removed == removed => Component {
                count: c.count + 1,
                added,
                removed,
                previous: c.previous.clone(),
            },
            _ => Component {
                count: 1,
                added,
                removed,
                previous: path.last.clone(),
            },
        };
        PathState {
            old_pos: path.old_pos + inc,
            last: Some(Rc::new(component)),
        }
    };

    let max_edit_length = new_len + old_len;
    let offset = max_edit_length + 2;
    let mut best_path: Vec<Option<PathState>> = vec![None; (2 * offset + 1) as usize];
    let idx = |d: isize| (d + offset) as usize;
    let mut first = PathState {
        old_pos: -1,
        last: None,
    };
    let new_pos = extract_common(&mut first, 0);
    if first.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        return build_values(first.last, &new_tokens, &old_tokens);
    }
    best_path[idx(0)] = Some(first);
    let mut min_diagonal = isize::MIN;
    let mut max_diagonal = isize::MAX;
    let mut edit_length: isize = 1;
    while edit_length <= max_edit_length {
        let mut diagonal = min_diagonal.max(-edit_length);
        while diagonal <= max_diagonal.min(edit_length) {
            // No one else uses the remove path again; the add path stays for the next diagonal.
            let remove_path = best_path[idx(diagonal - 1)].take();
            let add_path = best_path[idx(diagonal + 1)].clone();
            let can_add = add_path.as_ref().is_some_and(|p| {
                let add_new_pos = p.old_pos - diagonal;
                0 <= add_new_pos && add_new_pos < new_len
            });
            let can_remove = remove_path
                .as_ref()
                .is_some_and(|p| p.old_pos + 1 < old_len);
            if !can_add && !can_remove {
                best_path[idx(diagonal)] = None;
                diagonal += 2;
                continue;
            }
            let use_add = !can_remove
                || (can_add
                    && remove_path.as_ref().map(|r| r.old_pos)
                        < add_path.as_ref().map(|a| a.old_pos));
            let chosen = if use_add { &add_path } else { &remove_path };
            let Some(path) = chosen else {
                diagonal += 2;
                continue;
            };
            let mut base = if use_add {
                add_to_path(path, true, false, 0)
            } else {
                add_to_path(path, false, true, 1)
            };
            let new_pos = extract_common(&mut base, diagonal);
            if base.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return build_values(base.last, &new_tokens, &old_tokens);
            }
            let reached_old_end = base.old_pos + 1 >= old_len;
            best_path[idx(diagonal)] = Some(base);
            if reached_old_end {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if new_pos + 1 >= new_len {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            diagonal += 2;
        }
        edit_length += 1;
    }
    // Unreachable for line diffs (the full edit length always completes); keep a safe answer.
    let mut changes = Vec::new();
    if !old.is_empty() {
        changes.push(Change {
            value: old.to_owned(),
            added: false,
            removed: true,
        });
    }
    if !new.is_empty() {
        changes.push(Change {
            value: new.to_owned(),
            added: true,
            removed: false,
        });
    }
    changes
}

fn build_values(
    last: Option<Rc<Component>>,
    new_tokens: &[&str],
    old_tokens: &[&str],
) -> Vec<Change> {
    let mut order = Vec::new();
    let mut next = last.as_deref();
    while let Some(c) = next {
        order.push((c.count, c.added, c.removed));
        next = c.previous.as_deref();
    }
    order.reverse();
    let mut new_pos = 0;
    let mut old_pos = 0;
    let mut changes = Vec::with_capacity(order.len());
    for (count, added, removed) in order {
        let value = if !removed {
            let v: String = new_tokens[new_pos..new_pos + count].concat();
            new_pos += count;
            if !added {
                old_pos += count;
            }
            v
        } else {
            let v: String = old_tokens[old_pos..old_pos + count].concat();
            old_pos += count;
            v
        };
        changes.push(Change {
            value,
            added,
            removed,
        });
    }
    changes
}

/// jsdiff `splitLines`: lines with their trailing newline, where present.
fn split_lines(text: &str) -> Vec<String> {
    let has_trailing = text.ends_with('\n');
    let mut result: Vec<String> = text.split('\n').map(|l| format!("{l}\n")).collect();
    if has_trailing {
        result.pop();
    } else if let Some(last) = result.last_mut() {
        last.pop();
    }
    result
}

/// `Diff.createTwoFilesPatch(path, path, old, new, undefined, undefined, {context, headerOptions:
/// FILE_HEADERS_ONLY})`.
pub fn unified_patch(path: &str, old: &str, new: &str, context: usize) -> String {
    struct Hunk {
        old_start: usize,
        old_lines: usize,
        new_start: usize,
        new_lines: usize,
        lines: Vec<String>,
    }
    let mut diff: Vec<(Vec<String>, bool, bool)> = diff_lines(old, new)
        .into_iter()
        .map(|c| (split_lines(&c.value), c.added, c.removed))
        .collect();
    diff.push((vec![], false, false));
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut old_range_start, mut new_range_start) = (0usize, 0usize);
    let mut cur_range: Vec<String> = Vec::new();
    let (mut old_line, mut new_line) = (1usize, 1usize);
    for i in 0..diff.len() {
        let (lines, added, removed) = &diff[i];
        if *added || *removed {
            if old_range_start == 0 {
                old_range_start = old_line;
                new_range_start = new_line;
                if i > 0 {
                    let prev = &diff[i - 1].0;
                    let from = prev.len().saturating_sub(context);
                    cur_range = if context > 0 {
                        prev[from..].iter().map(|l| format!(" {l}")).collect()
                    } else {
                        vec![]
                    };
                    old_range_start -= cur_range.len();
                    new_range_start -= cur_range.len();
                }
            }
            for line in lines {
                cur_range.push(format!("{}{line}", if *added { '+' } else { '-' }));
            }
            if *added {
                new_line += lines.len();
            } else {
                old_line += lines.len();
            }
        } else {
            if old_range_start != 0 {
                if lines.len() <= context * 2 && i + 2 < diff.len() {
                    for line in lines {
                        cur_range.push(format!(" {line}"));
                    }
                } else {
                    let size = lines.len().min(context);
                    for line in &lines[..size] {
                        cur_range.push(format!(" {line}"));
                    }
                    hunks.push(Hunk {
                        old_start: old_range_start,
                        old_lines: old_line - old_range_start + size,
                        new_start: new_range_start,
                        new_lines: new_line - new_range_start + size,
                        lines: std::mem::take(&mut cur_range),
                    });
                    old_range_start = 0;
                    new_range_start = 0;
                }
            }
            old_line += lines.len();
            new_line += lines.len();
        }
    }
    let mut out = vec![format!("--- {path}"), format!("+++ {path}")];
    for mut hunk in hunks {
        let mut lines = Vec::with_capacity(hunk.lines.len());
        for line in hunk.lines.drain(..) {
            match line.strip_suffix('\n') {
                Some(stripped) => lines.push(stripped.to_owned()),
                None => {
                    lines.push(line);
                    lines.push("\\ No newline at end of file".to_owned());
                }
            }
        }
        if hunk.old_lines == 0 {
            hunk.old_start = hunk.old_start.saturating_sub(1);
        }
        if hunk.new_lines == 0 {
            hunk.new_start = hunk.new_start.saturating_sub(1);
        }
        out.push(format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
        ));
        out.extend(lines);
    }
    out.join("\n") + "\n"
}

/// `generateDiffString`: display diff with padded line numbers and `context` lines of context,
/// plus the first changed line in the new file.
pub fn display_diff(old: &str, new: &str, context: usize) -> (String, Option<usize>) {
    let parts = diff_lines(old, new);
    let mut output: Vec<String> = Vec::new();
    let max_line = old.split('\n').count().max(new.split('\n').count());
    let width = max_line.to_string().len();
    let pad = |n: usize| format!("{n:>width$}");
    let blank = " ".repeat(width);
    let (mut old_line, mut new_line) = (1usize, 1usize);
    let mut last_was_change = false;
    let mut first_changed = None;
    for (i, part) in parts.iter().enumerate() {
        let mut raw: Vec<&str> = part.value.split('\n').collect();
        if raw.last() == Some(&"") {
            raw.pop();
        }
        if part.added || part.removed {
            first_changed.get_or_insert(new_line);
            for line in raw {
                if part.added {
                    output.push(format!("+{} {line}", pad(new_line)));
                    new_line += 1;
                } else {
                    output.push(format!("-{} {line}", pad(old_line)));
                    old_line += 1;
                }
            }
            last_was_change = true;
        } else {
            let next_is_change = parts
                .get(i + 1)
                .is_some_and(|next| next.added || next.removed);
            let context_line = |output: &mut Vec<String>,
                                line: &str,
                                old_line: &mut usize,
                                new_line: &mut usize| {
                output.push(format!(" {} {line}", pad(*old_line)));
                *old_line += 1;
                *new_line += 1;
            };
            if last_was_change && next_is_change {
                if raw.len() <= context * 2 {
                    for line in &raw {
                        context_line(&mut output, line, &mut old_line, &mut new_line);
                    }
                } else {
                    let skipped = raw.len() - 2 * context;
                    for line in &raw[..context] {
                        context_line(&mut output, line, &mut old_line, &mut new_line);
                    }
                    output.push(format!(" {blank} ..."));
                    old_line += skipped;
                    new_line += skipped;
                    for line in &raw[raw.len() - context..] {
                        context_line(&mut output, line, &mut old_line, &mut new_line);
                    }
                }
            } else if last_was_change {
                let shown = raw.len().min(context);
                for line in &raw[..shown] {
                    context_line(&mut output, line, &mut old_line, &mut new_line);
                }
                let skipped = raw.len() - shown;
                if skipped > 0 {
                    output.push(format!(" {blank} ..."));
                    old_line += skipped;
                    new_line += skipped;
                }
            } else if next_is_change {
                let skipped = raw.len().saturating_sub(context);
                if skipped > 0 {
                    output.push(format!(" {blank} ..."));
                    old_line += skipped;
                    new_line += skipped;
                }
                for line in &raw[skipped..] {
                    context_line(&mut output, line, &mut old_line, &mut new_line);
                }
            } else {
                old_line += raw.len();
                new_line += raw.len();
            }
            last_was_change = false;
        }
    }
    (output.join("\n"), first_changed)
}
