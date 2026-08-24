//! How many COLUMNS a chart stack lays its slots out in, and how wide each one is.
//!
//! A stack has always been one column (or one row, turned sideways). The screen divider makes that
//! number a setting: divide into N, and either hold to N exactly or work up to it only as the
//! charts stop fitting. Everything here is pure arithmetic on sizes the caller measured, so the
//! rule is testable without a live stack — the entity work lives in `stack.rs`.

/// Upper bound on the divider. Six 1/6-width charts on a 3840-wide screen are already 640 px each,
/// and the popup's segmented control has to fit them all in its own width.
pub(in crate::chart_tabs) const MAX_COLUMNS: u8 = 6;

/// Narrowest a column may become while the divider is working UP to its number on its own, in
/// LOGICAL px.
///
/// Deliberately NOT `moon_chart::GLASS_ZONE_PX + 20`: that constant is 220 PHYSICAL px (see its own
/// doc), while everything the divider measures comes from the render probe in logical px. Deriving
/// this from it read as "the order book plus a little", and on a 150% display it silently became a
/// third stricter than intended — the arithmetic here has one unit and this is it.
///
/// The number is still chosen for the same reason: below it a column is its controls and no chart.
/// Not applied when the reader asked for an exact division — that setting is a promise about the
/// layout, and quietly giving fewer columns than the number on screen would be worse than narrow
/// ones.
pub(in crate::chart_tabs) const MIN_COLUMN_W: f32 = 240.0;

/// The same floor for a tab with the ORDER BOOK switched off, in logical px.
///
/// Most of `MIN_COLUMN_W` is the control zone the order book occupies. A tab that draws none is not
/// paying for it, and holding those columns to a width they do not need is what makes the divider
/// look broken on a narrow window: it never divides, however many charts arrive.
pub(in crate::chart_tabs) const MIN_COLUMN_W_NO_BOOK: f32 = 140.0;

/// Narrowest a ROW may become while a HORIZONTAL stack works its divider up on its own, in logical
/// px like everything else here.
///
/// A horizontal stack divides its cross extent vertically, so the floor there is a height, not a
/// width: the card header plus enough plot under it to be a chart rather than a strip.
pub(in crate::chart_tabs) const MIN_ROW_H: f32 = 90.0;

/// Default for the "minimum slot" field, in logical px along the stack axis.
///
/// Only FIT-stretch needs it: the other two modes state a slot size outright, and that size is what
/// says when the charts have stopped fitting. Stretch has no such number — its slots share whatever
/// space there is — so without one, "we have filled the screen" is undefined.
pub(in crate::chart_tabs) const DEFAULT_MIN_SLOT: u16 = 180;

/// A stack's divider SETTINGS, as stored — the three per-tab values and the one piece of live
/// state that overrides them.
///
/// Copied rather than borrowed so the size probe can carry it into a paint callback: everything
/// here is a small `Copy`, and the probe must be able to re-derive the column count from a new size
/// without reaching back into the stack.
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::chart_tabs) struct GridCfg {
    pub columns: Option<u8>,
    pub exact: Option<bool>,
    pub min_slot: Option<u16>,
    /// Comparison's broom mode, which lays the stack out as one row whatever the settings say.
    pub broom: bool,
    /// Whether the tab draws order books, which is most of what a column has to be wide enough for.
    pub orderbook: bool,
}

/// How many columns a stack with these settings lays out in at this size.
///
/// The one place that turns stored settings plus a measured size into the number the layout uses —
/// shared by the render path and by the size probe, which has to answer the same question about a
/// size that has not been rendered yet.
///
/// Args:
///     cfg: The stack's divider settings.
///     size: `(across the axis, along the axis)` in logical px, as painted.
///     count: Live charts to place.
///     horizontal: Whether the stack runs along X, which swaps what the divider cuts.
///     slot_extent: The mode's own slot size along the axis, or zero in FIT-stretch.
///
/// Returns:
///     Columns to render, at least 1.
pub(in crate::chart_tabs) fn columns_for(
    cfg: GridCfg,
    size: (f32, f32),
    count: usize,
    horizontal: bool,
    slot_extent: f32,
) -> usize {
    if cfg.broom {
        return 1;
    }
    let (width, height) = size;
    let (axis_extent, cross_extent) = match horizontal {
        true => (width, height),
        false => (height, width),
    };
    effective_columns(GridInput {
        columns: cfg.columns.unwrap_or(1),
        exact: cfg.exact.unwrap_or(false),
        count,
        axis_extent,
        cross_extent,
        min_cross: match (horizontal, cfg.orderbook) {
            (true, _) => MIN_ROW_H,
            (false, true) => MIN_COLUMN_W,
            (false, false) => MIN_COLUMN_W_NO_BOOK,
        },
        // FIT-stretch names no slot size of its own, so the minimum-slot setting is what says when
        // the charts have stopped fitting.
        slot_extent: match slot_extent > 0.0 {
            true => slot_extent,
            false => f32::from(cfg.min_slot.unwrap_or(DEFAULT_MIN_SLOT)),
        },
    })
}

/// Everything the divider needs to know about the stack it is dividing.
#[derive(Clone, Copy, Debug)]
pub(in crate::chart_tabs) struct GridInput {
    /// The divider itself: how many parts the reader asked the screen to be cut into.
    pub columns: u8,
    /// Whether that number is exact. False works up to it as the charts stop fitting.
    pub exact: bool,
    /// Live charts to place.
    pub count: usize,
    /// Size of the stack along its own axis — height for a vertical stack, width for a horizontal
    /// one. Zero means "not measured yet"; the first frame of a stack has no bounds to report.
    pub axis_extent: f32,
    /// Size of the stack ACROSS its axis, which is what the columns divide.
    pub cross_extent: f32,
    /// Smallest a division of the cross extent may be: a column WIDTH in a vertical stack, a row
    /// HEIGHT in a horizontal one. The caller picks which, because only it knows the orientation.
    pub min_cross: f32,
    /// Size one slot wants along the axis: the mode's own slot size, or the minimum-slot setting in
    /// FIT-stretch. Zero or less leaves the divider with nothing to measure fullness by.
    pub slot_extent: f32,
}

/// How many columns to lay the stack out in, from 1 up to the divider.
///
/// With `exact`, the answer is the divider, unconditionally — the reader asked for a layout, not
/// for advice. Otherwise it is the SMALLEST number of columns that fits the charts in the space
/// there is: one column while they still fit down the screen, two once they do not, and so on up to
/// the divider. Both answers are then held to whole columns that can still show a chart, except
/// that an exact division is never narrowed (see [`MIN_COLUMN_W`]).
///
/// Args:
///     input: The measured stack; see [`GridInput`].
///
/// Returns:
///     Columns to render, at least 1 and never above the divider.
pub(in crate::chart_tabs) fn effective_columns(input: GridInput) -> usize {
    let divider = usize::from(input.columns.clamp(1, MAX_COLUMNS));
    if divider == 1 || input.count == 0 {
        return 1;
    }
    let wanted = match input.exact {
        true => divider,
        false => grown_columns(input, divider),
    };
    match input.exact {
        // An exact division is what it says: the reader chose the number and can see the result.
        true => wanted,
        // A number reached on its own is capped by what a column can still usefully be.
        false => wanted
            .min(fitting_columns(input.cross_extent, input.min_cross))
            .max(1),
    }
}

/// The smallest column count that gets every chart into the space along the axis.
///
/// Unmeasured (`axis_extent` or `slot_extent` at zero) stays at one column: the first frame has no
/// bounds yet, and one column is what the stack did before the divider existed — the measured frame
/// that follows corrects it.
fn grown_columns(input: GridInput, divider: usize) -> usize {
    // `is_finite` and not just a sign test: a NaN extent passes every comparison, and NaN would
    // then survive `max(1.0)` and hand out the whole divider — the opposite of the safe answer.
    if !input.axis_extent.is_finite()
        || !input.slot_extent.is_finite()
        || input.axis_extent <= 0.0
        || input.slot_extent <= 0.0
    {
        return 1;
    }
    let rows_that_fit = (input.axis_extent / input.slot_extent).floor().max(1.0);
    // `ceil(count / rows)` — the columns needed for that many rows to hold every chart.
    let needed = (input.count as f32 / rows_that_fit).ceil();
    (needed as usize).clamp(1, divider)
}

/// How many divisions the cross extent can hold before one stops being big enough for a chart.
fn fitting_columns(cross_extent: f32, min_cross: f32) -> usize {
    // Unmeasured reads as ONE, the same answer the axis guard gives for the same state: the two
    // halves of "not measured yet" must not disagree, or an unmeasured stack would be refused rows
    // by one of them and handed the whole divider by the other.
    if !cross_extent.is_finite() || cross_extent <= 0.0 {
        return 1;
    }
    if !min_cross.is_finite() || min_cross <= 0.0 {
        return usize::MAX;
    }
    ((cross_extent / min_cross).floor() as usize).max(1)
}

/// Which slots belong to row `row` of a `columns`-wide grid, as a half-open range.
///
/// Row-major, which is what "left to right, then the next line down" means. The last row is short
/// whenever the count does not divide evenly, and it stays short: the grid keeps its columns lined
/// up rather than stretching two charts across three columns' worth of space.
pub(super) fn row_slots(row: usize, columns: usize, count: usize) -> std::ops::Range<usize> {
    let columns = columns.max(1);
    let start = (row * columns).min(count);
    let end = (start + columns).min(count);
    start..end
}

/// How many rows a `columns`-wide grid needs for `count` charts.
pub(super) fn row_count(columns: usize, count: usize) -> usize {
    let columns = columns.max(1);
    count.div_ceil(columns)
}

/// Turn a range of ROWS into the range of slots they hold.
///
/// The scrolling stack reports which of its items are on screen, and with a grid an item is a row.
/// Handing those row numbers on as slot numbers is what would light up the wrong charts' own pass —
/// and leave the ones actually on screen dark.
pub(super) fn slots_of_rows(
    rows: std::ops::Range<usize>,
    columns: usize,
    count: usize,
) -> std::ops::Range<usize> {
    let columns = columns.max(1);
    let start = (rows.start * columns).min(count);
    let end = (rows.end * columns).min(count);
    start..end
}

pub(in crate::chart_tabs) mod render;

#[cfg(test)]
mod tests;
