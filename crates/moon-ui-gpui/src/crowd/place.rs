//! The empty Main screen's frame: nine anchors, whatever is switched on, and one column when the
//! panel is too narrow for three.
//!
//! Every block of that screen — the brand, the line under it, the three crowd tables — used to nail
//! itself to one corner with its own `absolute().right().top()`. Placement was therefore spread
//! across four files and could not be chosen. It is one function now: the blocks arrive as
//! elements, the anchors arrive as [`EmptyPlaces`], and this decides where each one is drawn.
//!
//! **A cell is a STACK, not a seat.** Two blocks may name the same anchor, and then they are drawn
//! one under the other in [`EmptyBlock::ALL`] order. That is not a fallback for a clash — it is how
//! the shipped screen works, with the mark and its hint sharing the middle cell.
//!
//! **Narrow is a different layout, not a squeezed one.** Three columns of a table measured in
//! digits cannot be made narrow: below the width its widest board needs, the frame stops being a
//! grid and becomes one scrollable column read in anchor order. Nothing is clipped sideways and no
//! saved anchor becomes wrong — the same arrangement is simply read top to bottom.

use gpui::{
    AnyElement, App, Div, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use moon_core::config::layout::{EmptyBlock, EmptyPlaces, EmptySlot};

use super::table::Look;

/// How many columns the grid form has. Named because the narrow threshold is derived from it: the
/// grid is worth having exactly while the widest board fits in one of them.
const COLUMNS: f32 = 3.0;

/// Gap between two blocks stacked in one cell, and between two cells, in design units.
///
/// One value for both, because a reader cannot be told which gap they are looking at: two blocks in
/// the middle cell and two blocks in two cells must sit apart by the same amount or the stack reads
/// as a group and the pair does not.
const BLOCK_GAP: f32 = 12.0;

/// The width below which the grid is abandoned for one column.
///
/// Derived from the boards themselves rather than chosen: a column of figures is as wide as the
/// digits in it, so the honest question is whether the widest board still fits in a third of the
/// panel. A design-unit constant here would be somebody's font setting and nobody else's — the
/// boards grow with the Font slider and this threshold grows with them.
///
/// Args:
///     cx: Application context supplying the theme's scaled metrics.
pub(crate) fn narrow_below(cx: &App) -> Pixels {
    let look = Look::of(cx);
    let widest = f32::from(look.w_day_traders()).max(f32::from(look.w_minute()));
    grid_width(px(widest), look.edge_x, crate::design::ui_px(cx, BLOCK_GAP))
}

/// Budget all columns, both outer insets and the gaps between columns.
fn grid_width(board: Pixels, inset: Pixels, gap: Pixels) -> Pixels {
    board * COLUMNS + inset * 2.0 + gap * (COLUMNS - 1.0)
}

/// Outer Main widths needed by each full board, independently of the three-column threshold.
pub(crate) fn board_thresholds(cx: &App) -> [Pixels; 3] {
    let look = Look::of(cx);
    [
        look.w_minute(),
        look.w_day_coins() + look.rank_indent(),
        look.w_day_traders(),
    ]
    .map(|width| width + look.edge_x * 2.0)
}

/// Select compact minute, coin and trader records only when that board cannot fit the inner width.
/// Zero is the unmeasured first frame and keeps the full presentation, like the grid probe.
pub(crate) fn board_modes(width: Pixels, thresholds: [Pixels; 3]) -> usize {
    thresholds
        .into_iter()
        .enumerate()
        .fold(0, |modes, (index, threshold)| {
            modes | (usize::from(width > px(0.0) && width < threshold) << index)
        })
}

/// Draw the empty screen's blocks where this profile has put them.
///
/// Args:
///     id: Element identity for the scrolling narrow form, already scoped to the group window.
///     blocks: The blocks that are switched ON, in any order — the anchors decide what is drawn
///         where, and [`EmptyBlock::ALL`] decides what stacks above what.
///     places: Where each block goes.
///     narrow: Whether the panel is below [`narrow_below`].
///     cx: Application context supplying the theme's scaled metrics.
///
/// Returns:
///     One absolutely-placed layer, ready to be laid over the screen's background.
pub(crate) fn frame(
    id: SharedString,
    blocks: Vec<(EmptyBlock, AnyElement)>,
    places: EmptyPlaces,
    narrow: bool,
    cx: &App,
) -> AnyElement {
    let look = Look::of(cx);
    let gap = crate::design::ui_px(cx, BLOCK_GAP);
    // Taken out of the list as each anchor claims it: an element cannot be cloned, and a block
    // drawn twice would be two live tables reading one board.
    let mut pending: Vec<(EmptyBlock, Option<AnyElement>)> = blocks
        .into_iter()
        .map(|(block, element)| (block, Some(element)))
        .collect();

    if narrow {
        // One column, in anchor reading order: the arrangement still says what comes first, it is
        // simply read down the page. Each board independently chooses whether its columns fit.
        let ordered: Vec<AnyElement> = EmptySlot::ALL
            .into_iter()
            .flat_map(|slot| places.blocks_in(slot))
            .filter_map(|block| take(&mut pending, block))
            .collect();
        return div()
            .id(id)
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(gap)
            .px(look.edge_x)
            .pt(look.edge_top)
            .pb(look.edge_bottom)
            .overflow_x_hidden()
            .overflow_y_scroll()
            .children(ordered.into_iter().map(|block| {
                div()
                    .flex()
                    .justify_center()
                    .w_full()
                    .min_w_0()
                    .flex_none()
                    .child(block)
            }))
            .into_any_element();
    }

    let mut columns: Vec<Div> = Vec::new();
    for column in 0..3u8 {
        let mut cells: Vec<Div> = Vec::new();
        for (row, slot) in column_slots(column).enumerate() {
            let stacked: Vec<AnyElement> = places
                .blocks_in(slot)
                .filter_map(|block| take(&mut pending, block))
                .collect();
            cells.push(cell(row as u8, column, stacked, gap));
        }
        columns.push(column_flow(cells, gap, look.edge_top, look.edge_bottom));
    }

    div()
        .id(id)
        .absolute()
        .inset_0()
        .flex()
        .items_start()
        .gap(gap)
        .px(look.edge_x)
        .overflow_x_hidden()
        .overflow_y_scroll()
        // Direct children: GPUI measures scroll extent from immediate child bounds.
        .children(columns)
        .into_any_element()
}

/// Visit only one column's anchors so other columns cannot contribute to its vertical flow.
fn column_slots(column: u8) -> impl Iterator<Item = EmptySlot> {
    (0..3).filter_map(move |row| EmptySlot::at(row, column))
}

/// Take one block's element out of the list, or `None` when that block is switched off.
///
/// Args:
///     pending: Every block handed in, each still holding its element until an anchor claims it.
///     block: The block being placed.
fn take(pending: &mut [(EmptyBlock, Option<AnyElement>)], block: EmptyBlock) -> Option<AnyElement> {
    pending
        .iter_mut()
        .find(|(candidate, _)| *candidate == block)
        .and_then(|(_, element)| element.take())
}

/// Give each column its own viewport-height flow, growing beyond it only for its own content.
/// The parent has a definite viewport height and start alignment, so a tall left column cannot
/// stretch the center column or move its middle anchor. Overflow reaches the frame's scrollport.
fn column_flow(cells: Vec<Div>, gap: Pixels, top: Pixels, bottom: Pixels) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h(gpui::relative(1.0))
        .gap(gap)
        .pt(top)
        .pb(bottom)
        .children(cells)
}

/// One cell: whatever is anchored here, aligned to the cell's own corner of the screen.
///
/// The alignment is the anchor. A block in the end column is drawn against the right-hand edge and
/// a block in the bottom band against the bottom one, which is what makes "bottom right" mean the
/// same thing at every window size.
///
/// Args:
///     row: Which band, from the top.
///     column: Which cell, from the band's start.
///     stacked: What is anchored here, in the order it stacks.
///     gap: Space between two stacked blocks.
fn cell(row: u8, column: u8, stacked: Vec<AnyElement>, gap: Pixels) -> Div {
    // Only the middle cell grows; no cell shrinks below its own stack's intrinsic height.
    let cell = div().flex().w_full().min_w_0().flex_shrink_0();
    let cell = if row == 1 { cell.flex_grow(1.0) } else { cell };
    let cell = match column {
        0 => cell.justify_start(),
        1 => cell.justify_center(),
        _ => cell.justify_end(),
    };
    let cell = match row {
        0 => cell.items_start(),
        1 => cell.items_center(),
        _ => cell.items_end(),
    };
    if stacked.is_empty() {
        return cell;
    }
    let column_flow = div().flex().flex_col().min_w_0().gap(gap);
    let column_flow = match column {
        0 => column_flow.items_start(),
        1 => column_flow.items_center(),
        _ => column_flow.items_end(),
    };
    cell.child(column_flow.children(stacked))
}

#[cfg(test)]
mod tests;
