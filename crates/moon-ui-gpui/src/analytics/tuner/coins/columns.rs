//! Columns and geometry of the COIN table: which metrics it shows, its default order, and the
//! widths its hand-laid rows spend outside the shared descriptors.
//!
//! Separate from the strategy list's columns (`strat_columns`): both draw from the same
//! [`MetricCol`] pool, but the coin table is narrower, carries its own tick columns, and its
//! width guard has to read ITS numbers, not the roomier table's.

use super::super::columns::{COL_PF, COL_PROFIT, COL_TRADES, COL_WINRATE, COL_WORST, MetricCol};

/// Per-coin columns: the same order, minus the per-trade average and the best case. Both tables
/// are visible at once in "Coins" mode, so a differing order would cost the reader the position
/// cue they just learned on the left.
pub(in crate::analytics::tuner) const COIN_COLS: &[MetricCol] =
    &[COL_TRADES, COL_PROFIT, COL_WINRATE, COL_PF, COL_WORST];

/// Default sort of the coin table: trade count, descending.
///
/// It has to be a column of the SELECTED strategies' own numbers. The rows arrive ordered by
/// the profit of ALL strategies, which is a figure the table does not display — ranking by it
/// would scatter the selected strategy's coins through thousands of rows it never traded, and
/// the row cap would then drop the ones that matter.
pub(in crate::analytics::tuner) const COIN_DEFAULT_SORT: &str = COL_TRADES.key;

/// Geometry of the coins panel. The coin-name column is whatever [`COIN_COLS`] leaves over, so
/// these are the terms a width test has to read rather than re-state.
///
/// The panel is elastic now (it shares the left column with the strategy list), so this is the
/// narrowest width the table is DESIGNED to stay readable at — the widths below still have to
/// leave the coin name room there. Only the width test reads it.
///
/// Where the number comes from: the analytics window opens at 1240 px wide (`open_analytics`),
/// the right column is a fixed 470 and the paddings around it 28, so the coin table gets ~742.
/// 560 is that with a wide margin, which leaves the guard some teeth: a shared descriptor may
/// grow ~30 px before it starts eating the coin name.
///
/// Below the window's OWN minimum (860 px) this mode is cramped — the numeric columns alone
/// outgrow the left half there. That predates this layout (the previous fixed 460-px coin panel
/// plus the 470-px right column already overflowed an 860-px window) and is not what this
/// guards.
#[cfg(test)]
const COIN_MIN_PANEL_W: f32 = 560.0;
/// Width the coin name is never allowed to drop below (font-scaled px).
///
/// The floor has to sit on the FLEX ITEM: without it the name column shrinks to zero and its
/// text paints outside its own box, over the column beside it — it does not simply disappear.
pub(in crate::analytics::tuner) const COIN_NAME_MIN_W: f32 = 80.0;
pub(in crate::analytics::tuner) const COIN_ROW_PAD_X: f32 = 8.0;
pub(in crate::analytics::tuner) const COIN_ROW_GAP: f32 = 8.0;
/// The coin table's two TRAILING tick columns (blacklist, whitelist), which sit outside
/// [`COIN_COLS`] and are therefore invisible to the descriptors. Declared here so the width
/// test below cannot drift away from what the rows actually render.
pub(in crate::analytics::tuner) const COIN_TICK_W: f32 = 34.0;
/// What the two ticks spend together.
#[cfg(test)]
const COIN_TICKS_W: f32 = COIN_TICK_W * 2.0;

#[cfg(test)]
mod tests {
    use super::super::super::strat_columns::METRIC_COLS;
    use super::{
        COIN_COLS, COIN_MIN_PANEL_W, COIN_NAME_MIN_W, COIN_ROW_GAP, COIN_ROW_PAD_X, COIN_TICKS_W,
    };

    /// Both tables sit on the same screen in "Coins" mode, so a metric present in both has to
    /// keep the same relative position — otherwise the position cue learned on one misreads the
    /// other.
    #[test]
    fn coin_columns_keep_the_strategy_order() {
        let shared: Vec<&str> = METRIC_COLS
            .iter()
            .map(|c| c.key)
            .filter(|k| COIN_COLS.iter().any(|c| c.key == *k))
            .collect();
        let coin_order: Vec<&str> = COIN_COLS.iter().map(|c| c.key).collect();
        assert_eq!(shared, coin_order);
    }

    /// The coin table's name column is the residual of its width, so widening a shared
    /// descriptor for the roomier strategy table silently eats the coin name here. Measured at
    /// the column FLOORS — every column shrinks toward its own before the row overflows — at
    /// the narrowest layout the panel is expected to work at, counting the two trailing tick
    /// columns the descriptors know nothing about.
    #[test]
    fn coin_columns_leave_room_for_the_name() {
        let numbers: f32 = COIN_COLS.iter().map(|c| c.min_w).sum();
        // Two gaps beyond the metric columns: one before each trailing tick column.
        let gaps = COIN_ROW_GAP * (COIN_COLS.len() + 2) as f32;
        let name_w = COIN_MIN_PANEL_W - COIN_ROW_PAD_X * 2.0 - gaps - COIN_TICKS_W - numbers;
        assert!(
            name_w >= COIN_NAME_MIN_W,
            "coin name column squeezed to {name_w}px by the shared descriptors"
        );
    }
}
