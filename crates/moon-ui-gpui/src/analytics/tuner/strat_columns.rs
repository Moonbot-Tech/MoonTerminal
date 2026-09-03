//! Columns of the STRATEGY list: which metrics it shows, in what order, their sort ids and
//! the visibility bits of its column selector.
//!
//! Separate from the coin table's columns (`coin_columns`) even though both draw from the same
//! [`MetricCol`] pool: the two tables sit on one screen but answer different questions, and a
//! column added to one must not appear in the other by accident.

use super::columns::{
    COL_AVG, COL_AVG_ORDER, COL_BEST, COL_BL, COL_PF, COL_PROFIT, COL_PROFIT_PCT, COL_TRADES,
    COL_WINRATE, COL_WL, COL_WORST, MetricCol,
};

/// Strategy comparison columns, in reading order: identity, then how many trades back the figure,
/// then the result, then how good it is, then the tails. Profit leads the numbers because the
/// table is sorted by it — the sort key sitting mid-row leaves the ranking without an anchor, and
/// winrate placed ahead of it reads a 100%-on-two-trades row as the winner.
pub(super) const METRIC_COLS: &[MetricCol] = &[
    COL_TRADES,
    COL_PROFIT,
    COL_PROFIT_PCT,
    COL_AVG_ORDER,
    COL_AVG,
    COL_WINRATE,
    COL_PF,
    COL_BEST,
    COL_WORST,
    // The strategy's own coin lists, at the tail: they describe its CONFIGURATION, not its
    // result, so they must not sit between the performance numbers.
    COL_BL,
    COL_WL,
];

/// Sort ids for the columns METRIC_COLS doesn't cover (name, the two identity columns, lastedit).
pub(super) const SORT_NAME: &str = "name";
pub(super) const SORT_KIND: &str = "kind";
pub(super) const SORT_CORE: &str = "core";
pub(super) const SORT_LASTEDIT: &str = "lastedit";
/// Strategy-list sort used when no valid saved choice exists.
pub(super) const STRAT_SORT_DEFAULT: (&str, bool) = (COL_PROFIT.key, true);

/// Visible-column bit layout. Existing fields keep their historical bits; the two new metrics use
/// bits above the historical last-edit bit so old persisted masks cannot silently change meaning.
pub(super) const COL_BIT_KIND: u16 = 1 << 0;
pub(super) const COL_BIT_CORE: u16 = 1 << 1;
const METRIC_BITS: &[u16] = &[
    1 << 2,
    1 << 3,
    1 << 13,
    1 << 12,
    1 << 4,
    1 << 5,
    1 << 6,
    1 << 7,
    1 << 8,
    1 << 9,
    1 << 10,
];
/// Visibility bit of `METRIC_COLS[i]`, preserving the semantic layout of saved masks.
pub(super) const fn metric_bit(i: usize) -> u16 {
    METRIC_BITS[i]
}
/// Historical visibility bit of the "last edit" column.
pub(super) const COL_BIT_LASTEDIT: u16 = 1u16 << 11;
/// Width (font-scaled px) of the "last edit" column — fits a `dd.mm.yyyy hh:mm` stamp.
pub(super) const LASTEDIT_W: f32 = 110.0;
/// How narrow that stamp may be squeezed before it stops being readable.
pub(super) const LASTEDIT_MIN_W: f32 = 78.0;
/// Width (font-scaled px) of the strategy-kind identity column. Kind is a short type name
/// ("EMA", "Drop"), so a fixed width serves it — shared by the row and the header, which
/// used to carry these numbers as separate literals.
pub(super) const KIND_W: f32 = 72.0;
/// How narrow the kind column may be squeezed once row space runs short.
pub(super) const KIND_MIN_W: f32 = 48.0;
/// FLOOR of the core column's preferred width. Unlike every other column this one is
/// content-measured per data load (`list::table::core_col_w`): its single-core label is a
/// free-form server name, which no fixed width fits — 88 truncated real names on a wide
/// window while the flexible name column held the slack. The floor is what the column takes
/// when every measured name is shorter, and when there is nothing to measure yet; it also
/// covers the multi-core aggregate label ("Núcleos: 999", the longest locale form, stays
/// under it at body size).
pub(super) const CORE_W: f32 = 88.0;
/// How narrow the core column may be squeezed once row space runs short.
pub(super) const CORE_MIN_W: f32 = 56.0;
/// Ceiling of the measured core width: one anomalously named server must not push the
/// metric columns off the row.
pub(super) const CORE_W_MAX: f32 = 240.0;
/// Full mask — every toggleable column visible (kind, core, all metric columns, lastedit).
///
/// NOT the default: the fixed columns then total more than the left column is wide, and since
/// the name is the only flexible one, it is the name that collapses — the table loses the very
/// thing every row is identified by. See [`STRAT_COLS_DEFAULT`].
pub(in crate::analytics) const STRAT_COLS_ALL: u16 = (1 << 14) - 1;
/// New fields enabled once while migrating an older per-mode mask.
pub(in crate::analytics) const STRAT_COLS_V3_ADDITIONS: u16 = metric_bit(2) | metric_bit(3);

/// Width the strategy name is never allowed to drop below (font-scaled px).
///
/// The name is the row's identity and the only column that can flex, so without a floor it is
/// what absorbs every column added to its right — silently, down to nothing.
pub(in crate::analytics) const STRAT_NAME_MIN_W: f32 = 120.0;

/// Narrowest width the strategy list is designed to stay usable at (font-scaled px).
///
/// The analytics window opens 1240 px wide, the right column is a fixed 470 and the paddings
/// around it 28, so this table gets ~740. Guarded at that, because unlike the coin table it
/// is the wider of the two and has no slack to give away.
#[cfg(test)]
const STRAT_MIN_PANEL_W: f32 = 740.0;

/// Columns shown when the user has not chosen for themselves.
///
/// Identity (kind, core) plus the headline numbers (trades, profit, Profit %, Avg order, winrate,
/// PF) — the set that fits beside a readable name. The tails (avg / best / worst) and the edit
/// stamp stay one click away in the column selector rather than squeezing the name out of the row.
/// `strat_columns_default_fits` holds this honest.
pub(in crate::analytics) const STRAT_COLS_DEFAULT: u16 = COL_BIT_KIND
    | COL_BIT_CORE
    | metric_bit(0)
    | metric_bit(1)
    | metric_bit(2)
    | metric_bit(3)
    | metric_bit(5)
    | metric_bit(6);

/// The same, plus the strategy's coin-list counts — the default of the "By coin" axis.
///
/// The lists are what that axis is ABOUT, so they earn their width there; on the other two they
/// would only take it from the name. That is the whole reason the mask is kept per axis.
pub(in crate::analytics) const STRAT_COLS_DEFAULT_COINS: u16 =
    STRAT_COLS_DEFAULT | metric_bit(9) | metric_bit(10);

/// Restore the current per-mode mask schema or migrate older masks by enabling new metrics once.
///
/// Args:
///     current: Saved mask in the schema containing Avg order and Profit %.
///     previous: Older per-mode mask whose existing bits retain their meaning.
///     legacy_single: Oldest one-mask preference used only when per-mode state is absent.
///
/// Returns:
///     Current per-mode masks that respect current hides or enable new columns once.
pub(in crate::analytics) fn restore_strat_columns(
    current: Option<moon_core::config::layout::StratColsByMode>,
    previous: Option<moon_core::config::layout::StratColsByMode>,
    legacy_single: Option<u16>,
) -> moon_core::config::layout::StratColsByMode {
    if let Some(current) = current {
        return current;
    }
    let mut restored = previous.unwrap_or_else(|| {
        let mut by_mode = moon_core::config::layout::StratColsByMode::default();
        for mode in super::STRAT_MODES {
            *mode.cols_slot(&mut by_mode) = legacy_single.unwrap_or(mode.default_cols());
        }
        by_mode
    });
    for mode in super::STRAT_MODES {
        *mode.cols_slot(&mut restored) |= STRAT_COLS_V3_ADDITIONS;
    }
    restored
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
