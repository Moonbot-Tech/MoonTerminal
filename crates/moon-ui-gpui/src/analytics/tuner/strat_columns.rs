//! Columns of the STRATEGY list: which metrics it shows, in what order, their sort ids and
//! the visibility bits of its column selector.
//!
//! Separate from the coin table's columns (`coin_columns`) even though both draw from the same
//! [`MetricCol`] pool: the two tables sit on one screen but answer different questions, and a
//! column added to one must not appear in the other by accident.

use super::columns::{
    COL_AVG, COL_BEST, COL_BL, COL_PF, COL_PROFIT, COL_TRADES, COL_WINRATE, COL_WL, COL_WORST,
    MetricCol,
};

/// Strategy comparison columns, in reading order: identity, then how many trades back the figure,
/// then the result, then how good it is, then the tails. Profit leads the numbers because the
/// table is sorted by it — the sort key sitting mid-row leaves the ranking without an anchor, and
/// winrate placed ahead of it reads a 100%-on-two-trades row as the winner.
pub(super) const METRIC_COLS: &[MetricCol] = &[
    COL_TRADES,
    COL_PROFIT,
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

/// Visible-column bit layout: kind at bit 0, core at bit 1, then METRIC_COLS[i] at bit 2+i, and
/// lastedit just above the metrics. The strategy name column is identity and always shown (no bit).
pub(super) const COL_BIT_KIND: u16 = 1 << 0;
pub(super) const COL_BIT_CORE: u16 = 1 << 1;
/// Visibility bit of `METRIC_COLS[i]`. `const` so the default mask below can be built from
/// the same function the renderer reads, instead of hand-written bit literals that would go
/// stale the moment a column is inserted.
pub(super) const fn metric_bit(i: usize) -> u16 {
    1u16 << (2 + i)
}
/// Visibility bit of the "last edit" column (sits above the metric bits).
pub(super) const COL_BIT_LASTEDIT: u16 = 1u16 << (2 + METRIC_COLS.len() as u16);
/// Width (font-scaled px) of the "last edit" column — fits a `dd.mm.yyyy hh:mm` stamp.
pub(super) const LASTEDIT_W: f32 = 110.0;
/// How narrow that stamp may be squeezed before it stops being readable.
pub(super) const LASTEDIT_MIN_W: f32 = 78.0;
/// Full mask — every toggleable column visible (kind, core, all metric columns, lastedit).
///
/// NOT the default: the fixed columns then total more than the left column is wide, and since
/// the name is the only flexible one, it is the name that collapses — the table loses the very
/// thing every row is identified by. See [`STRAT_COLS_DEFAULT`].
pub(in crate::analytics) const STRAT_COLS_ALL: u16 = (COL_BIT_LASTEDIT << 1) - 1;

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
/// Identity (kind, core) plus the headline numbers (trades, profit, winrate, PF) — the set that
/// FITS beside a readable name. The tails (avg / best / worst) and the edit stamp stay one click
/// away in the ▦ selector rather than squeezing the name out of the row for everyone.
/// `strat_columns_default_fits` holds this honest.
pub(in crate::analytics) const STRAT_COLS_DEFAULT: u16 =
    COL_BIT_KIND | COL_BIT_CORE | metric_bit(0) | metric_bit(1) | metric_bit(3) | metric_bit(4);

/// The same, plus the strategy's coin-list counts — the default of the "By coin" axis.
///
/// The lists are what that axis is ABOUT, so they earn their width there; on the other two they
/// would only take it from the name. That is the whole reason the mask is kept per axis.
pub(in crate::analytics) const STRAT_COLS_DEFAULT_COINS: u16 =
    STRAT_COLS_DEFAULT | metric_bit(7) | metric_bit(8);

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests {
    use super::super::columns::{COL_PROFIT, COL_TRADES, COL_WINRATE, MetricCol};
    use super::{
        COL_BIT_CORE, COL_BIT_KIND, COL_BIT_LASTEDIT, LASTEDIT_MIN_W, METRIC_COLS, STRAT_COLS_ALL,
        STRAT_COLS_DEFAULT, STRAT_COLS_DEFAULT_COINS, STRAT_MIN_PANEL_W, STRAT_NAME_MIN_W,
        metric_bit,
    };

    /// What the row spends outside the metric descriptors AT ITS FLOOR: the two identity
    /// columns, the alive dot and its gap, the row padding, and the gap to the cluster.
    ///
    /// Floors, not preferred widths: every column shrinks toward its own floor before the row
    /// overflows, so "does this fit" is a question about the minimum layout.
    fn fixed_extras(mask: u16) -> f32 {
        let mut w = 8.0 * 2.0 + (6.0 + 6.0) + 8.0;
        if mask & COL_BIT_KIND != 0 {
            w += 48.0 + 8.0;
        }
        if mask & COL_BIT_CORE != 0 {
            w += 56.0 + 8.0;
        }
        if mask & COL_BIT_LASTEDIT != 0 {
            w += LASTEDIT_MIN_W + 8.0;
        }
        w
    }

    /// What the whole column cluster costs at its floor under the given mask.
    fn fixed_total(mask: u16) -> f32 {
        let metrics: f32 = METRIC_COLS
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & metric_bit(*i) != 0)
            .map(|(_, c)| c.min_w + 8.0)
            .sum();
        fixed_extras(mask) + metrics
    }

    /// Every column shrinks toward its own floor, and the name has a floor of its own, so the
    /// table fits exactly while the floors plus the name floor fit the panel. The default has
    /// to satisfy that; the full mask is allowed not to (it overflows, visibly, on purpose).
    #[test]
    fn strat_columns_default_fits() {
        // Both defaults, or the axis that adds two columns is the one nobody checked.
        for (label, mask) in [
            ("filter/time", STRAT_COLS_DEFAULT),
            ("coins", STRAT_COLS_DEFAULT_COINS),
        ] {
            let left = STRAT_MIN_PANEL_W - fixed_total(mask);
            assert!(
                left >= STRAT_NAME_MIN_W,
                "{label} default leaves the name {left}px, below its {STRAT_NAME_MIN_W}px floor"
            );
        }
        // If even the full mask fits at its floors, the reduced default is pointless.
        assert!(
            STRAT_MIN_PANEL_W - fixed_total(STRAT_COLS_ALL) < STRAT_NAME_MIN_W,
            "the full mask now fits; make it the default instead of keeping a reduced one"
        );
    }

    fn position(cols: &[MetricCol], key: &str) -> usize {
        cols.iter()
            .position(|c| c.key == key)
            .unwrap_or_else(|| panic!("column {key} missing"))
    }

    /// The tables are sorted by profit descending (SQL-side, `db::analytics`). The sort key has
    /// to lead the numbers: buried mid-row it leaves the ranking without an anchor, and winrate
    /// ahead of it presents a 100%-on-two-trades row as the top performer.
    #[test]
    fn profit_anchors_the_numeric_columns() {
        assert_eq!(METRIC_COLS[0].key, COL_TRADES.key, "trade count leads");
        assert_eq!(METRIC_COLS[1].key, COL_PROFIT.key, "profit follows it");
        assert!(
            position(METRIC_COLS, COL_PROFIT.key) < position(METRIC_COLS, COL_WINRATE.key),
            "profit must precede winrate"
        );
        // COIN_COLS inherits the ordering via `coin_columns_keep_the_strategy_order`.
    }
}
