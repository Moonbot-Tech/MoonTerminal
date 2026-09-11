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
//! digits cannot be made narrow: below the width the blocks ON THE SCREEN need side by side, the
//! frame stops being a grid and becomes one scrollable column read in anchor order. Nothing is
//! clipped sideways and no saved anchor becomes wrong — the same arrangement is simply read top to
//! bottom.
//!
//! **The threshold is what is switched on, where it was put.** It used to be three times the widest
//! board there is — 1400-odd logical pixels, which a 1920-wide screen at 150 % never reaches — so
//! every anchor a person chose was overridden by the one-column form, and "everything is always in
//! the middle" was the whole of the feature. Now each column is as wide as the widest block anchored
//! in it, and a column nobody uses costs nothing. The side columns are made EQUAL while the panel
//! affords it, so the brand in the middle stays in the true middle; when it does not, the brand
//! moves off-center by a few pixels before the whole grid is given up.

use gpui::{
    AnyElement, App, Div, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use moon_core::config::layout::{EmptyBlock, EmptyPlaces, EmptySlot};
use rust_i18n::t;

use super::table::Look;
use crate::design;

/// Gap between two blocks stacked in one cell, and between two cells, in design units.
///
/// One value for both, because a reader cannot be told which gap they are looking at: two blocks in
/// the middle cell and two blocks in two cells must sit apart by the same amount or the stack reads
/// as a group and the pair does not.
const BLOCK_GAP: f32 = 12.0;

/// What the switched-on blocks ask of the grid's three columns, in pixels.
///
/// Resolved once per frame from the same three inputs the drawing uses — which blocks are on, where
/// each is anchored, and the theme's measurements — so the threshold and the columns it guards
/// cannot disagree about what is on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ColumnNeeds {
    /// The widest block anchored in the start column, or zero when nothing is.
    pub(crate) start: Pixels,
    /// The same for the middle column.
    pub(crate) center: Pixels,
    /// The same for the end column.
    pub(crate) end: Pixels,
    /// Both outer insets and both gaps between columns: what the grid costs before any block.
    pub(crate) fixed: Pixels,
}

/// Which of the frame's two forms a width gets, and the side columns' widths when it is the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Form {
    /// Every block in one scrollable column, read in anchor order.
    OneColumn,
    /// Three columns; the middle one takes whatever the two sides leave.
    Grid { start: Pixels, end: Pixels },
}

impl Form {
    /// The number the size probe repaints on: one column, the symmetric grid, or the asymmetric
    /// one. Two widths that give the same number draw the same frame, so a resize between them
    /// costs no repaint — and two that differ must repaint, since the side columns moved.
    pub(crate) fn mode(self) -> usize {
        match self {
            Self::OneColumn => 0,
            Self::Grid { start, end } if start == end => 1,
            Self::Grid { .. } => 2,
        }
    }
}

impl ColumnNeeds {
    /// Measure the blocks that are on, column by column.
    ///
    /// Args:
    ///     on: The blocks that are switched on, in any order.
    ///     places: Where each block is anchored.
    ///     cx: Application context supplying the theme's scaled metrics.
    pub(crate) fn of(on: &[EmptyBlock], places: EmptyPlaces, cx: &App) -> Self {
        let look = Look::of(cx);
        let gap = design::ui_px(cx, BLOCK_GAP);
        let mut needs = Self {
            start: px(0.0),
            center: px(0.0),
            end: px(0.0),
            fixed: look.edge_x * 2.0 + gap * 2.0,
        };
        for block in on {
            let width = block_width(*block, &look, cx);
            let column = match places.slot(*block).column() {
                0 => &mut needs.start,
                1 => &mut needs.center,
                _ => &mut needs.end,
            };
            *column = (*column).max(width);
        }
        needs
    }

    /// The width below which the grid is abandoned for one column: every column's own need, side
    /// by side, plus the insets and gaps.
    pub(crate) fn narrow_below(self) -> Pixels {
        self.start + self.center + self.end + self.fixed
    }

    /// The form a panel of this width gets.
    ///
    /// A width of zero is the frame BEFORE the first measurement, and is deliberately the grid: the
    /// grid is what almost every panel gets, and opening on the column form for one frame would
    /// flash. The grid is symmetric while the panel affords both sides at the wider side's width —
    /// that is what keeps the middle column in the middle — and asymmetric between that and the
    /// collapse, because a brand a few pixels off-center is a smaller loss than the whole grid.
    ///
    /// Args:
    ///     width: The panel's measured width, or zero before the first measurement.
    pub(crate) fn form(self, width: Pixels) -> Form {
        if width > px(0.0) && width < self.narrow_below() {
            return Form::OneColumn;
        }
        let side = self.start.max(self.end);
        if width == px(0.0) || side * 2.0 + self.center + self.fixed <= width {
            Form::Grid {
                start: side,
                end: side,
            }
        } else {
            Form::Grid {
                start: self.start,
                end: self.end,
            }
        }
    }
}

/// The width one block asks of its column.
///
/// The boards are measured by their columns, the same way [`board_thresholds`] measures them. The
/// brand is its glow frame. The line under the brand is its own block — switched and anchored on
/// its own, with or without the mark — and it WRAPS, so what it asks is the narrowest column it can
/// still fold into: its longest word. Charging it the full width it may spread to would collapse
/// the whole grid for a sentence that would have folded onto one more line.
fn block_width(block: EmptyBlock, look: &Look, cx: &App) -> Pixels {
    match block {
        EmptyBlock::Logo => design::logo_glow_frame_w(cx, design::EMPTY_STACK_LOGO_W),
        EmptyBlock::Hint => hint_min_width(cx),
        EmptyBlock::Minute => look.w_minute(),
        EmptyBlock::Traders => look.w_day_traders(),
        EmptyBlock::Coins => look.w_day_coins() + look.rank_indent(),
    }
}

/// The widest word of the hint at the size it is drawn: the width below which it cannot wrap
/// any further. Measured through the face and size the screen draws it in — `t_body` in the mono
/// family, which the hint sets nowhere and inherits from the shell root — through the glyph cache,
/// so it is not a constant somebody tuned for one locale.
fn hint_min_width(cx: &App) -> Pixels {
    let hint = t!("chart.empty.hint");
    px(hint
        .split_whitespace()
        .map(|word| design::mono_body_text_width(cx, word, 400.0))
        .fold(0.0, f32::max))
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
///     form: The grid with its side widths, or the one-column form, from [`ColumnNeeds::form`].
///     cx: Application context supplying the theme's scaled metrics.
///
/// Returns:
///     One absolutely-placed layer, ready to be laid over the screen's background.
pub(crate) fn frame(
    id: SharedString,
    blocks: Vec<(EmptyBlock, AnyElement)>,
    places: EmptyPlaces,
    form: Form,
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

    let Form::Grid { start, end } = form else {
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
    };

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
        let width = match column {
            0 => Some(start),
            1 => None,
            _ => Some(end),
        };
        columns.push(column_flow(
            cells,
            width,
            gap,
            look.edge_top,
            look.edge_bottom,
        ));
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
///
/// A side column is as wide as [`Form::Grid`] says and no wider — three equal thirds would hand a
/// board a third of the panel whatever the board measures, which is what made the old threshold
/// three boards wide. The middle column has no width of its own and takes what the sides leave.
///
/// Args:
///     cells: The column's three anchors, top to bottom.
///     width: The side column's width, or `None` for the middle one.
///     gap: Space between two cells.
///     top: Inset above the first cell.
///     bottom: Inset under the last.
fn column_flow(
    cells: Vec<Div>,
    width: Option<Pixels>,
    gap: Pixels,
    top: Pixels,
    bottom: Pixels,
) -> Div {
    let column = div().flex().flex_col().min_w_0();
    let column = match width {
        Some(width) => column.flex_none().w(width),
        None => column.flex_1(),
    };
    column
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
