//! Layout only: cells are already parsed, flat document blocks with stable selection addresses.
use super::markdown::{Block, BlockKind, TableAlignment, TableCell};
use crate::theme::Theme;
use gpui::{prelude::*, *};

pub fn cell(meta: TableCell, theme: &Theme) -> Stateful<Div> {
    div()
        .id(("table-cell", meta.column))
        .role(if meta.row == 0 {
            Role::ColumnHeader
        } else {
            Role::Cell
        })
        .flex_1()
        .min_w_0()
        .px(px(8.0 * theme.scale))
        .py(px(6.0 * theme.scale))
        .min_h(theme.body_line_height() + px(12.0 * theme.scale))
        .when(meta.column + 1 < meta.columns, |d| {
            d.border_r_1().border_color(theme.colors.border)
        })
        .when(meta.row == 0, |d| d.font_weight(FontWeight::SEMIBOLD))
        .text_align(match meta.alignment {
            TableAlignment::Center => TextAlign::Center,
            TableAlignment::Right => TextAlign::Right,
            _ => TextAlign::Left,
        })
}

pub fn grid(
    id: ElementId,
    cells: &[Block],
    theme: &Theme,
    mut render_cell: impl FnMut(usize, &Block) -> AnyElement,
) -> AnyElement {
    let BlockKind::TableCell(first) = cells[0].kind else {
        unreachable!("table cells only")
    };
    let columns = first.columns.max(1);
    let rows = cells
        .chunks(columns)
        .enumerate()
        .map(|(row, cells)| {
            div()
                .id(("table-row", row))
                .role(Role::Row)
                .w_full()
                .flex()
                .items_stretch()
                .when(row == 0, |d| d.bg(theme.colors.bg_active))
                .when(row > 0, |d| {
                    d.border_t_1().border_color(theme.colors.border)
                })
                .children(
                    cells
                        .iter()
                        .enumerate()
                        .map(|(column, block)| render_cell(row * columns + column, block)),
                )
        })
        .collect::<Vec<_>>();
    div()
        .w_full()
        .my(px(6.0 * theme.scale))
        .pl(px(cells[0].indent as f32 * 12.0 * theme.scale))
        .when(cells[0].quote, |d| {
            d.border_l_2()
                .border_color(theme.colors.border_strong)
                .pl(px(12.0))
        })
        .child(
            div().id(id).w_full().overflow_x_scroll().child(
                div()
                    .id("table-grid")
                    .role(Role::Table)
                    .aria_label("Markdown table; scroll horizontally for additional columns")
                    .w_full()
                    .min_w(px(columns as f32 * 80.0 * theme.scale + 2.0))
                    .border_1()
                    .border_color(theme.colors.border)
                    .rounded(px(4.0))
                    .children(rows),
            ),
        )
        .into_any_element()
}

/// End of a contiguous table in the flat block model. Distinct tables never merge.
pub fn end(blocks: &[Block], start: usize) -> usize {
    let BlockKind::TableCell(first) = blocks[start].kind else {
        return start + 1;
    };
    start
        + blocks[start..]
            .iter()
            .take_while(|b| matches!(b.kind, BlockKind::TableCell(c) if c.table == first.table))
            .count()
}
