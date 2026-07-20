//! Column descriptors of the tuner's comparison tables (strategy list + per-coin panel):
//! the metric columns, their sort ids and visibility bits, the coin-panel geometry, and the
//! shared header/body cell renderers. Kept apart from the rendering so a column cannot exist
//! in the heading and not in the body — both read the SAME descriptor.

use gpui::*;
use moon_ui::MoonPalette;
use rust_i18n::t;

use super::super::summary::{fmt_signed, sign_color};
use crate::design::moon;
use moon_core::db::analytics::GroupStat;

/// One NUMERIC column of the comparison tables.
///
/// The heading and the body cell come from the SAME descriptor, so such a column cannot exist in
/// one and not the other, and its value cannot end up rendered under a neighbour's heading — a
/// misalignment no layout would reveal, since the table renders cleanly either way and only the
/// titles are wrong.
///
/// The two leading identity columns (kind, core) stay hand-paired: they are left-aligned, carry
/// their own tones and caption size, and modelling that here would add three optional fields for
/// two columns that no reordering touches.
#[derive(Clone, Copy)]
pub(super) struct MetricCol {
    /// i18n key of the heading; doubles as the stable sort id for this column.
    pub(super) key: &'static str,
    /// Column width, in font-scaled px.
    pub(super) w: f32,
    /// Cell text for one aggregate.
    pub(super) text: fn(&GroupStat) -> String,
    /// Value whose sign colours the cell; `None` renders neutral.
    pub(super) signed: Option<fn(&GroupStat) -> f64>,
    /// Value used when the list is sorted by this column.
    pub(super) sort: fn(&GroupStat) -> f64,
}

const COL_TRADES: MetricCol = MetricCol {
    key: "analytics.kpi.trades",
    w: 56.0,
    text: |g| g.n.to_string(),
    signed: None,
    sort: |g| g.n as f64,
};
const COL_PROFIT: MetricCol = MetricCol {
    key: "analytics.col.profit",
    w: 84.0,
    text: |g| fmt_signed(g.profit),
    signed: Some(|g| g.profit),
    sort: |g| g.profit,
};
const COL_AVG: MetricCol = MetricCol {
    key: "analytics.kpi.avg_short",
    w: 70.0,
    text: |g| fmt_signed(g.avg()),
    signed: Some(|g| g.avg()),
    sort: |g| g.avg(),
};
const COL_WINRATE: MetricCol = MetricCol {
    key: "analytics.kpi.winrate",
    w: 56.0,
    text: |g| format!("{:.1}%", g.winrate()),
    signed: None,
    sort: |g| g.winrate(),
};
const COL_PF: MetricCol = MetricCol {
    key: "analytics.col.pf",
    w: 52.0,
    text: |g| format!("{:.2}", g.pf),
    signed: None,
    sort: |g| g.pf,
};
const COL_BEST: MetricCol = MetricCol {
    key: "analytics.col.best",
    w: 70.0,
    text: |g| fmt_signed(g.best),
    signed: Some(|g| g.best),
    sort: |g| g.best,
};
const COL_WORST: MetricCol = MetricCol {
    key: "analytics.col.worst",
    w: 70.0,
    text: |g| fmt_signed(g.worst),
    signed: Some(|g| g.worst),
    sort: |g| g.worst,
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
/// Visibility bit of `METRIC_COLS[i]`.
pub(super) fn metric_bit(i: usize) -> u16 {
    1u16 << (2 + i)
}
/// Visibility bit of the "last edit" column (sits above the metric bits).
pub(super) const COL_BIT_LASTEDIT: u16 = 1u16 << (2 + METRIC_COLS.len() as u16);
/// Width (font-scaled px) of the "last edit" column — fits a `dd.mm.yyyy hh:mm` stamp.
pub(super) const LASTEDIT_W: f32 = 110.0;
/// Full mask — every toggleable column visible (kind, core, all metric columns, lastedit).
pub(in crate::analytics) const STRAT_COLS_ALL: u16 = (COL_BIT_LASTEDIT << 1) - 1;

/// Per-coin columns: the same order, minus the per-trade average and the best case. Both tables
/// are visible at once in "Coins" mode, so a differing order would cost the reader the position
/// cue they just learned on the left.
pub(super) const COIN_COLS: &[MetricCol] =
    &[COL_TRADES, COL_PROFIT, COL_WINRATE, COL_PF, COL_WORST];

/// Geometry of the coins panel. The coin-name column is whatever [`COIN_COLS`] leaves over, so
/// these are the terms a width test has to read rather than re-state.
pub(super) const COIN_PANEL_W: f32 = 460.0;
pub(super) const COIN_ROW_PAD_X: f32 = 8.0;
pub(super) const COIN_ROW_GAP: f32 = 8.0;

/// Heading cell of a [`MetricCol`], right-aligned over its numbers.
///
/// `scale` is the caller's hoisted [`crate::design::font_scale`]: resolving it per cell costs two
/// by-value theme-token clones each, and this table renders every row (it is not virtualized).
pub(super) fn head_cell(col: &MetricCol, scale: f32) -> impl IntoElement {
    div()
        .w(px(col.w * scale))
        .flex_none()
        .text_right()
        .child(t!(col.key).to_string())
}

/// Body cell of a [`MetricCol`]: text and sign colour both derived from the same descriptor, so
/// a reordered column carries its colouring with it.
///
/// `scale` is the caller's hoisted font scale shared by every cell in the rendered table.
pub(super) fn metric_cell(
    col: &MetricCol,
    g: &GroupStat,
    p: MoonPalette,
    scale: f32,
) -> impl IntoElement {
    let color = match col.signed {
        Some(value) => sign_color(p, value(g)),
        None => p.text_soft,
    };
    num_cell(scale, col.w, (col.text)(g), color)
}

/// Render one right-aligned numeric cell using a caller-hoisted font `scale`.
fn num_cell(scale: f32, w: f32, text: String, color: u32) -> impl IntoElement {
    div()
        .w(px(w * scale))
        .flex_none()
        .text_right()
        .text_color(moon(color))
        .child(text)
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests {
    use super::{
        COIN_COLS, COIN_PANEL_W, COIN_ROW_GAP, COIN_ROW_PAD_X, COL_PROFIT, COL_TRADES, COL_WINRATE,
        METRIC_COLS, MetricCol,
    };

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

    /// The coin panel is a fixed-width box and its name column is the residual, so widening a
    /// shared descriptor for the roomier strategy table silently eats the coin name here.
    #[test]
    fn coin_columns_leave_room_for_the_name() {
        let numbers: f32 = COIN_COLS.iter().map(|c| c.w).sum();
        let name_w =
            COIN_PANEL_W - COIN_ROW_PAD_X * 2.0 - COIN_ROW_GAP * COIN_COLS.len() as f32 - numbers;
        assert!(
            name_w >= 70.0,
            "coin name column squeezed to {name_w}px by the shared descriptors"
        );
    }
}
