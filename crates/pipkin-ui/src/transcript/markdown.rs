//! Markdown → flat block model. No GPUI.
//!
//! Every leaf text container (paragraph, heading, list item, code block) becomes one `Block`
//! with plain selectable `text` and styled `spans` over that text. List markers are part of
//! the text so copy/selection naturally include them. Unterminated constructs (an open code
//! fence while streaming, unclosed emphasis) degrade gracefully: pulldown-cmark treats an open
//! fence as code to the end of input and unclosed emphasis stays literal.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAlignment {
    None,
    Left,
    Center,
    Right,
}

impl From<pulldown_cmark::Alignment> for TableAlignment {
    fn from(value: pulldown_cmark::Alignment) -> Self {
        match value {
            pulldown_cmark::Alignment::None => Self::None,
            pulldown_cmark::Alignment::Left => Self::Left,
            pulldown_cmark::Alignment::Center => Self::Center,
            pulldown_cmark::Alignment::Right => Self::Right,
        }
    }
}
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Cells remain individual document blocks in row-major order, including empty cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableCell {
    pub table: usize,
    pub row: usize,
    pub column: usize,
    pub columns: usize,
    pub alignment: TableAlignment,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: SpanStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    /// A list item; the marker ("•", "1.") is already the prefix of `text`.
    ListItem,
    Code {
        lang: Option<String>,
    },
    Rule,
    TableCell(TableCell),
}

/// Plain-text copy keeps a table's row/column structure without Markdown delimiters.
pub fn separator(previous: &BlockKind, next: &BlockKind) -> &'static str {
    match (previous, next) {
        (BlockKind::TableCell(a), BlockKind::TableCell(b)) if a.table == b.table => {
            if a.row == b.row { "\t" } else { "\n" }
        }
        (BlockKind::ListItem, BlockKind::ListItem) => "\n",
        _ => "\n\n",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    pub spans: Vec<Span>,
    /// List nesting depth (0 = not in a list).
    pub indent: u8,
    pub quote: bool,
}

impl Block {
    pub fn plain(kind: BlockKind, text: impl Into<String>) -> Self {
        Block {
            kind,
            text: text.into(),
            spans: Vec::new(),
            indent: 0,
            quote: false,
        }
    }

    pub fn is_code(&self) -> bool {
        matches!(self.kind, BlockKind::Code { .. })
    }
}

struct Builder {
    block: Block,
    marker_only: bool,
}

struct Table {
    id: usize,
    row: usize,
    column: usize,
    alignments: Vec<TableAlignment>,
}

#[derive(Default)]
struct Parse {
    out: Vec<Block>,
    cur: Option<Builder>,
    bold: u32,
    italic: u32,
    strike: u32,
    links: Vec<String>,
    /// One entry per open list: next number for ordered lists, None for bullets.
    lists: Vec<Option<u64>>,
    quote_depth: u32,
    table: Option<Table>,
    next_table: usize,
}

impl Parse {
    fn style(&self, code: bool) -> SpanStyle {
        SpanStyle {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code,
            link: self.links.last().cloned(),
        }
    }

    fn flush(&mut self) {
        if let Some(b) = self.cur.take() {
            let keep = b.block.is_code()
                || matches!(b.block.kind, BlockKind::Rule | BlockKind::TableCell(_))
                || !b.block.text.trim().is_empty();
            if keep {
                self.out.push(b.block);
            }
        }
    }

    fn begin(&mut self, kind: BlockKind) {
        self.flush();
        self.cur = Some(Builder {
            block: Block {
                kind,
                text: String::new(),
                spans: Vec::new(),
                indent: self.lists.len().min(255) as u8,
                quote: self.quote_depth > 0,
            },
            marker_only: false,
        });
    }

    fn push_text(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        if self.cur.is_none() {
            self.begin(BlockKind::Paragraph);
        }
        let style = self.style(code);
        let b = self.cur.as_mut().unwrap();
        b.marker_only = false;
        let start = b.block.text.len();
        b.block.text.push_str(text);
        let end = b.block.text.len();
        if style != SpanStyle::default() && !b.block.is_code() {
            match b.block.spans.last_mut() {
                Some(last) if last.range.end == start && last.style == style => {
                    last.range.end = end
                }
                _ => b.block.spans.push(Span {
                    range: start..end,
                    style,
                }),
            }
        }
    }
}

pub fn parse(source: &str) -> Vec<Block> {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let mut p = Parse::default();
    for event in Parser::new_ext(source, opts) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    // A loose list item already opened its block with only a marker.
                    let reuse = matches!(&p.cur, Some(b) if b.marker_only);
                    if !reuse {
                        p.begin(BlockKind::Paragraph);
                    }
                }
                Tag::Heading { level, .. } => p.begin(BlockKind::Heading(level as u8)),
                Tag::BlockQuote(_) => {
                    p.flush();
                    p.quote_depth += 1;
                }
                Tag::CodeBlock(kind) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => {
                            let l = l.split_whitespace().next().unwrap_or("").to_string();
                            (!l.is_empty()).then_some(l)
                        }
                        CodeBlockKind::Indented => None,
                    };
                    p.begin(BlockKind::Code { lang });
                }
                Tag::List(start) => {
                    p.flush();
                    p.lists.push(start);
                }
                Tag::Item => {
                    p.begin(BlockKind::ListItem);
                    let marker = match p.lists.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}. ");
                            *n += 1;
                            m
                        }
                        _ => "• ".to_string(),
                    };
                    let b = p.cur.as_mut().unwrap();
                    b.block.text.push_str(&marker);
                    b.marker_only = true;
                }
                Tag::Table(alignments) => {
                    p.flush();
                    p.table = Some(Table {
                        id: p.next_table,
                        row: 0,
                        column: 0,
                        alignments: alignments.into_iter().map(Into::into).collect(),
                    });
                    p.next_table += 1;
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some(t) = &mut p.table {
                        t.column = 0;
                    }
                }
                Tag::TableCell => {
                    if let Some(t) = &mut p.table {
                        let cell = TableCell {
                            table: t.id,
                            row: t.row,
                            column: t.column,
                            columns: t.alignments.len(),
                            alignment: t
                                .alignments
                                .get(t.column)
                                .copied()
                                .unwrap_or(TableAlignment::None),
                        };
                        t.column += 1;
                        p.begin(BlockKind::TableCell(cell));
                    }
                }
                Tag::Emphasis => p.italic += 1,
                Tag::Strong => p.bold += 1,
                Tag::Strikethrough => p.strike += 1,
                Tag::Link { dest_url, .. } => p.links.push(dest_url.to_string()),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item => {
                    // Keep a loose item's block open only until its first paragraph ends.
                    p.flush();
                }
                TagEnd::CodeBlock => {
                    if let Some(b) = p.cur.as_mut() {
                        while b.block.text.ends_with('\n') {
                            b.block.text.pop();
                        }
                    }
                    p.flush();
                }
                TagEnd::BlockQuote(_) => {
                    p.flush();
                    p.quote_depth = p.quote_depth.saturating_sub(1);
                }
                TagEnd::List(_) => {
                    p.flush();
                    p.lists.pop();
                }
                TagEnd::TableCell => p.flush(),
                TagEnd::TableHead | TagEnd::TableRow => {
                    p.flush();
                    if let Some(t) = &mut p.table {
                        t.row += 1;
                    }
                }
                TagEnd::Table => {
                    p.flush();
                    p.table = None;
                }
                TagEnd::Emphasis => p.italic = p.italic.saturating_sub(1),
                TagEnd::Strong => p.bold = p.bold.saturating_sub(1),
                TagEnd::Strikethrough => p.strike = p.strike.saturating_sub(1),
                TagEnd::Link => {
                    p.links.pop();
                }
                _ => {}
            },
            Event::Text(t) => p.push_text(&t, false),
            Event::Code(t) => p.push_text(&t, true),
            Event::Html(t) | Event::InlineHtml(t) => p.push_text(&t, false),
            Event::SoftBreak => {
                let in_code = matches!(&p.cur, Some(b) if b.block.is_code());
                p.push_text(if in_code { "\n" } else { " " }, false)
            }
            Event::HardBreak => p.push_text("\n", false),
            Event::Rule => {
                p.flush();
                p.out.push(Block::plain(BlockKind::Rule, ""));
            }
            Event::TaskListMarker(done) => p.push_text(if done { "☑ " } else { "☐ " }, false),
            _ => {}
        }
    }
    p.flush();
    p.out
}

#[cfg(test)]
pub(crate) const TABLE_EXAMPLE: &str = "| | Subagent | Background job |\n|---|:---:|---:|\n| **What it is** | An AI worker with its own context. | A command/process. |\n| **What you give it** | An objective and tools. | A command to execute. |\n| **What it does** | Reasons and uses tools. | Runs the command. |\n| **Result** | A report. | Logs and an exit status. |";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_table_preserves_empty_header_coordinates_and_styles() {
        let blocks = parse(TABLE_EXAMPLE);
        assert_eq!(blocks.len(), 15);
        for (i, block) in blocks.iter().enumerate() {
            assert_eq!(
                block.kind,
                BlockKind::TableCell(TableCell {
                    table: 0,
                    row: i / 3,
                    column: i % 3,
                    columns: 3,
                    alignment: [
                        TableAlignment::None,
                        TableAlignment::Center,
                        TableAlignment::Right
                    ][i % 3],
                })
            );
        }
        assert_eq!(blocks[0].text, "");
        assert_eq!(blocks[2].text, "Background job");
        assert_eq!(blocks[3].text, "What it is");
        assert!(blocks[3].spans[0].style.bold);
    }

    #[test]
    fn table_cells_keep_inline_styles_escaped_pipes_and_missing_cells() {
        let blocks = parse(
            "| L | C | R |\n|:---|:---:|---:|\n| *é* | `a\\|b` | [link](https://example.com) |\n| ~~gone~~ |\\| |\n| short |\n",
        );
        assert_eq!(blocks.len(), 12);
        assert_eq!(blocks[3].text, "é");
        assert!(blocks[3].spans[0].style.italic);
        assert_eq!(blocks[4].text, "a|b");
        assert!(blocks[4].spans[0].style.code);
        assert_eq!(
            blocks[5].spans[0].style.link.as_deref(),
            Some("https://example.com")
        );
        assert!(blocks[6].spans[0].style.strike);
        assert_eq!(blocks[7].text, "|");
        assert_eq!(blocks[8].text, "");
        assert_eq!(blocks[10].text, "");
        assert_eq!(blocks[11].text, "");
    }

    #[test]
    fn every_streaming_prefix_is_safe_and_recognized_rows_are_rectangular() {
        let source = format!("{TABLE_EXAMPLE}\n| Unicode | 👨‍👩‍👧‍👦 é | `a\\|b` |\n\nAfter");
        for end in (0..=source.len()).filter(|end| source.is_char_boundary(*end)) {
            let blocks = parse(&source[..end]);
            let cells: Vec<_> = blocks
                .iter()
                .filter_map(|b| match b.kind {
                    BlockKind::TableCell(c) => Some(c),
                    _ => None,
                })
                .collect();
            assert_eq!(cells.len() % 3, 0, "prefix {end}");
            for (i, c) in cells.iter().enumerate() {
                assert_eq!((c.row, c.column), (i / 3, i % 3));
            }
            for b in blocks {
                for span in b.spans {
                    assert!(b.text.get(span.range).is_some());
                }
            }
        }
        assert!(
            !parse("| A | B |\n")
                .iter()
                .any(|b| matches!(b.kind, BlockKind::TableCell(_)))
        );
        assert!(matches!(
            parse("| A | B |\n|---|---|")[0].kind,
            BlockKind::TableCell(_)
        ));
    }

    #[test]
    fn distinct_tables_quotes_and_literal_pipes() {
        let blocks = parse(
            "before\n\n> | A | B |\n> |---|---|\n> | c | d |\n\nafter\n\n| E | F |\n|---|---|\n\n```text\n| G | H |\n|---|---|\n```\n\nliteral | pipes",
        );
        assert!(blocks[1].quote);
        assert!(matches!(
            blocks[1].kind,
            BlockKind::TableCell(TableCell { table: 0, .. })
        ));
        assert!(matches!(
            blocks[6].kind,
            BlockKind::TableCell(TableCell { table: 1, .. })
        ));
        assert!(blocks[8].is_code());
        assert_eq!(blocks[9].text, "literal | pipes");
    }

    #[test]
    fn paragraph_with_inline_styles() {
        let b = parse("Hello **bold** and *it* and `code` and [link](http://x.y).");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].text, "Hello bold and it and code and link.");
        let styles: Vec<_> = b[0]
            .spans
            .iter()
            .map(|s| (&b[0].text[s.range.clone()], s.style.clone()))
            .collect();
        assert_eq!(styles[0].0, "bold");
        assert!(styles[0].1.bold);
        assert!(styles[1].1.italic);
        assert!(styles[2].1.code);
        assert_eq!(styles[3].1.link.as_deref(), Some("http://x.y"));
    }

    #[test]
    fn lists_nested_and_ordered() {
        let b = parse("1. one\n2. two\n   - nested\n- bullet\n");
        let texts: Vec<_> = b.iter().map(|b| (b.text.as_str(), b.indent)).collect();
        assert_eq!(
            texts,
            vec![
                ("1. one", 1),
                ("2. two", 1),
                ("• nested", 2),
                ("• bullet", 1)
            ]
        );
    }

    #[test]
    fn loose_list_items_keep_one_block() {
        let b = parse("- a\n\n- b\n");
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].text, "• a");
    }

    #[test]
    fn fenced_code_with_language() {
        let b = parse("text\n\n```rust\nfn main() {}\n```\n");
        assert_eq!(b.len(), 2);
        assert_eq!(
            b[1].kind,
            BlockKind::Code {
                lang: Some("rust".into())
            }
        );
        assert_eq!(b[1].text, "fn main() {}");
    }

    #[test]
    fn unterminated_fence_is_code() {
        let b = parse("intro\n\n```py\nprint('hi')\nx = 1");
        let last = b.last().unwrap();
        assert!(last.is_code());
        assert_eq!(last.text, "print('hi')\nx = 1");
    }

    #[test]
    fn unclosed_emphasis_does_not_panic_and_stays_literal() {
        let b = parse("this is **not closed and _neither");
        assert_eq!(b.len(), 1);
        assert!(b[0].text.contains("**not closed"));
    }

    #[test]
    fn quote_flag_and_headings() {
        let b = parse("# Title\n\n> quoted\n");
        assert_eq!(b[0].kind, BlockKind::Heading(1));
        assert!(b[1].quote);
    }

    #[test]
    fn empty_and_whitespace_sources_produce_no_blocks() {
        assert!(parse("").is_empty());
        assert!(parse("   \n\n").is_empty());
    }

    #[test]
    fn hard_break_and_unicode() {
        let b = parse("مرحبا 👨‍👩‍👧‍👦 e\u{301}  \nsecond");
        assert_eq!(b.len(), 1);
        assert!(b[0].text.contains('\n'));
    }

    #[test]
    fn malformed_nested_lists_dont_panic() {
        let b = parse("- a\n    - b\n  - c\n\t- d\n* \n1) x\n```\n");
        assert!(!b.is_empty());
    }
}
