//! Rendering half of the screen divider: one grid cell, one grid row.
//!
//! Split from `stack.rs` because the arithmetic already lives beside it in `grid.rs` and the file
//! it came from is at the size the repo's own rule caps. What stays in `render_chart_stack` is the
//! dispatch — which of the three modes is asking — and everything a row needs is here.

use gpui::*;

use moon_ui::{h_flex, v_flex};

use super::super::grid;
use crate::panels::ChartPanel;

/// Wrap one slot of a grid row: a fixed FRACTION of the row across the stack's axis.
///
/// A fraction rather than a measured width, so the columns add up to the row exactly and the last
/// row stays lined up with the ones above even when it is short.
///
/// The tile inside is built with the axis FLIPPED (`!horizontal`): a tile stretches along the axis
/// it is told is its own, and inside a cell it has to fill the cell in both directions — across,
/// because the cell is its share of the row, and along, because the row states the height.
pub(in crate::chart_tabs) fn grid_cell(child: AnyElement, columns: usize, horizontal: bool) -> Div {
    let share = relative(1.0 / columns.max(1) as f32);
    let cell = div().flex().overflow_hidden();
    match horizontal {
        // A horizontal stack divides its rows vertically: each cell is a share of the column.
        true => cell.flex_col().w_full().h(share),
        false => cell.h_full().w(share),
    }
    .child(child)
}

/// Build one row of the grid: every live slot of that row, each in its own cell.
#[allow(clippy::too_many_arguments)]
pub(in crate::chart_tabs) fn grid_row<S, P, T>(
    s: &S,
    entity: &Entity<S>,
    row: usize,
    columns: usize,
    count: usize,
    horizontal: bool,
    border: Rgba,
    panel_at: &P,
    tile: &T,
) -> Div
where
    S: Render + 'static,
    P: Fn(&S, usize) -> Option<Entity<ChartPanel>> + Clone + 'static,
    T: Fn(
            &S,
            usize,
            Entity<ChartPanel>,
            Option<f32>,
            bool,
            Option<f32>,
            bool,
            Rgba,
            Entity<S>,
        ) -> AnyElement
        + Clone
        + 'static,
{
    let mut cells: Vec<Div> = Vec::with_capacity(columns);
    for ix in grid::row_slots(row, columns, count) {
        let child = match panel_at(s, ix) {
            // A retained empty COMPRESS slot keeps its cell: that is what holds the neighbours
            // still, which is the whole reason the slot was retained.
            None => div().size_full().into_any_element(),
            Some(panel) => tile(
                s,
                ix,
                panel,
                None,
                true,
                None,
                !horizontal,
                border,
                entity.clone(),
            ),
        };
        cells.push(grid_cell(child, columns, horizontal));
    }
    let row = match horizontal {
        true => v_flex().h_full(),
        false => h_flex().w_full().items_stretch(),
    };
    row.children(cells)
}
