//! Chart caption labels: WHAT a chart prints beside its plot, WHERE, and in WHICH style.
//!
//! The chart used to print a fixed roster — the coin, the core name that qualifies it, the
//! comparison delta and the Y-scale badge — hard-coded into the text pass. That roster became DATA
//! in PR #281: a flat list of slots, one caption each, joined into rows by an `inline` flag.
//!
//! This module is that model's second shape. A caption is no longer the unit of configuration; a
//! ROW is. A row carries its own name, the band it lives in and where in that band it sits, and
//! holds up to [`CHART_LABEL_PARTS`] captions — each with its own field, colour, size, prefix and
//! PnL basis. What used to cost four slots (the open-order figures chained with `inline`) is one
//! row with four parts, and the ceiling that used to be "sixteen captions on the whole chart" is
//! now "sixteen rows", each of them a family.
//!
//! Fixed-length rather than a `Vec`, like [`super::detect_view::DetectSizeCfg`] beside it, for the
//! reason the retained text pass needs: a part's GPU run is addressed by its INDEX
//! (`row * ROW_RUN_STRIDE + part`), and an index that shifts when a neighbour appears hands a run a
//! different string and reshapes it. The FILE, however, is written as a variable-length list —
//! see `wire` — so neither ceiling is baked into a saved profile and both can be raised without
//! costing anybody their tabs.

use serde::{Deserialize, Serialize};

mod builtin;
mod fields;
mod ops;
mod presets;
mod span;
mod style;
mod vocab;
mod wire;

pub use builtin::*;
pub use fields::{ChartAction, ChartLabelField, ChartLabelGroup};
pub use ops::*;
pub use presets::LabelPreset;
pub use span::*;
pub use style::*;
pub use vocab::*;

/// Number of caption rows one chart configuration holds.
///
/// Rows past the last used one are blank and are skipped while drawing. The count also sizes the
/// terminal's per-pane text-run pool together with [`ROW_RUN_STRIDE`], so raising it costs retained
/// runs on every pane and must be done deliberately. It costs nothing in a saved file.
pub const CHART_LABEL_ROWS: usize = 16;

/// Number of captions one row holds.
///
/// Eight is what a row can print before the horizontal budget truncates it anyway: the control
/// strip is only as wide as the order book, and even a full-width row over the plot runs out of
/// room around there.
pub const CHART_LABEL_PARTS: usize = 8;

/// First run index a module's COLUMN entries occupy, past its captions, name and filter header.
///
/// A column caption — the venue roster, or the strategy-filter skip lines — prints many lines from
/// a single configured caption, so those lines cannot be addressed as parts: there are more of
/// them than a module holds, and how many depends on what the core reports rather than on anything
/// saved. They get their own range of the same per-row stride instead, which keeps one addressing
/// rule for every retained run and costs nothing while no chart prints one (the pool grows by
/// index, on demand). Two column captions in one row share this range; the first one drawn wins.
pub const ARB_PART_BASE: usize = FILTER_HEADER_PART + 1;

/// Run index of the strategy-filter column's control, separate from the module's own name.
/// Reserving it before the entries keeps the full entry limit and every prefix run disjoint.
pub const FILTER_HEADER_PART: usize = ROW_NAME_PART + 1;

/// First run index reserved for a caption's PREFIX.
///
/// A caption whose colour applies to the value alone is two runs, not one: the prefix in the
/// theme's colour and the figure in the sign's. They cannot share an index — a run holds one string
/// — and the prefix cannot borrow a neighbour's, so the whole prefix range mirrors the value range
/// above it. The pool grows by index on demand, so a chart that colours whole captions never
/// allocates any of this.
pub const PREFIX_PART_BASE: usize = ARB_PART_BASE + super::arb_view::ARB_MAX_ROWS;

/// Retained text runs reserved per row: one per part, the row's printed name, the filter header,
/// the shared column range, a prefix run mirroring each of them, and the continuation lines of
/// the one caption in the row that may wrap.
pub const ROW_RUN_STRIDE: usize = WRAP_PART_BASE + (LABEL_WRAP_LINES - 1);

/// How many lines a caption that WRAPS may take, its first line included.
///
/// Only prose wraps — see [`ChartLabelField::wraps`] — and only this far: a detect line is worth
/// two or three lines of the plot's width, and a caption that could take ten would push whatever
/// the module prints under it off the pane.
pub const LABEL_WRAP_LINES: usize = 3;

/// First run slot of the continuation lines a wrapped caption draws.
///
/// A retained text run is addressed by `row * ROW_RUN_STRIDE + part`, so a second line of the same
/// caption needs a part of its own or it would overwrite the first. Continuation `k` (counted from
/// one) takes `WRAP_PART_BASE + k - 1`.
///
/// Per ROW rather than per caption, because a retained run is some three kilobytes and the pool is
/// kept dense to its highest index: a slot per caption would reserve sixteen of them per row, on
/// every pane, to serve the one caption that is prose. The cost of that choice is the rule the
/// drawing pass enforces — only the FIRST prose caption of a module wraps, and a second one is cut
/// as it was before.
pub const WRAP_PART_BASE: usize = PREFIX_PART_BASE * 2;

/// Run index — and part index — of a row's printed NAME.
///
/// Past every caption, so switching the name on renumbers none of them. Declared here rather than
/// in the drawing pass because it is the other half of [`ROW_RUN_STRIDE`]: two crates agreeing on
/// the stride but not on which index it reserves is a silent overlap.
pub const ROW_NAME_PART: usize = CHART_LABEL_PARTS;

/// Largest gap a module may ask for, in the chart's own logical pixels.
///
/// A gap past this is not spacing any more, it is a second band — and a hand-edited file asking for
/// one would push everything after it off the pane.
pub const LABEL_GAP_MAX: u8 = 64;

/// Longest row name kept; anything longer is cut on write.
///
/// A name is an identifier in a list and, when the row prints it, a caption over candles. Both stop
/// being readable well before this, and an unbounded string in a per-tab config is a file-size
/// question nobody wants to answer later.
pub const LABEL_ROW_NAME_MAX: usize = 48;

/// Smallest and largest font multiplier a part may carry.
///
/// The multiplier scales the chart's own label size, which already follows the Settings font
/// slider, so these bounds are relative to whatever the user picked there.
pub const LABEL_SIZE_MULT_MIN: f32 = 0.5;
pub const LABEL_SIZE_MULT_MAX: f32 = 3.0;

/// Multiplier a caption draws at while nothing overrides it — every caption, of every field.
///
/// One number rather than a per-field ladder: a chart is read from across a desk, and the size the
/// captions were legible at is the same for the coin, the badge and the position figures alike. The
/// hierarchy the ladder used to state — the coin a step over the core, the badge a step under the
/// comparison delta — is still available per caption, on the popup's own size strip, which is where
/// a reader who wants one puts it.
///
/// It is a DEFAULT, not a value: a part that overrides nothing writes nothing to the file, so a
/// profile follows this number when it moves rather than freezing the one it was created under.
pub const LABEL_SIZE_MULT_DEFAULT: f32 = 1.5;

/// Remaining time under which a candle countdown is clocked SECOND by second rather than by the
/// minute: the last hour, plus one minute of slack so the step changes before the display does.
///
/// The hour is where the caption's own format changes: past it the figure is hours and minutes,
/// which moves once a minute, so a second-by-second clock there would re-format the caption sixty
/// times for one printed change. Inside it the caption prints seconds and needs every one of them.
///
/// The minute of slack is what makes the switch land BEFORE the format needs it: a coarse clock
/// cannot express a sub-minute remainder at all, so arriving late would print `1ч 00м` where the
/// caption should already be reading `59м 50с`.
const COUNTDOWN_SECOND_STEP_BELOW_MS: i64 = 3_600_000 + 60_000;

/// Every label one chart draws.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartLabelsCfg {
    pub rows: [ChartLabelRow; CHART_LABEL_ROWS],
}

/// Move the used items of a fixed-length list to the front, blanking the tail.
///
/// The same repair at both levels — captions inside a row, rows inside a configuration — because
/// both read "the leading N are the used ones" everywhere: the popup's list, the draw order and the
/// retained-run pool.
fn compact<T: Default>(items: &mut [T], is_used: impl Fn(&T) -> bool) {
    let mut write = 0;
    for read in 0..items.len() {
        if is_used(&items[read]) {
            items.swap(write, read);
            write += 1;
        }
    }
    for item in &mut items[write..] {
        *item = T::default();
    }
}

/// Remove one item, closing the gap and blanking the freed tail slot.
fn remove_at<T: Default>(items: &mut [T], ix: usize) {
    if ix >= items.len() {
        return;
    }
    items[ix..].rotate_left(1);
    if let Some(last) = items.last_mut() {
        *last = T::default();
    }
}

/// Swap one item with its neighbour, refusing at the ends of the USED run rather than wrapping.
fn move_at<T>(items: &mut [T], used: usize, ix: usize, up: bool) -> bool {
    if ix >= used {
        return false;
    }
    // `wrapping_sub` turns "up from the first" into an index past the end, which the same bound
    // rejects — one check instead of a nested pair.
    let other = if up { ix.wrapping_sub(1) } else { ix + 1 };
    if other >= used {
        return false;
    }
    items.swap(ix, other);
    true
}

#[cfg(test)]
mod tests;
