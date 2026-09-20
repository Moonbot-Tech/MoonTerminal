//! Columns of the DEAL table of the "Entry/Exit" axis: what each shows, its width, and the
//! sort it offers. Hand-laid like the coin table, because the two trailing cells (tape, model)
//! are marks, not metrics, and no shared descriptor draws a mark.

/// One column of the deal table.
pub(in crate::analytics::tuner) struct DealCol {
    /// Sort key and element-id suffix.
    pub(in crate::analytics::tuner) key: &'static str,
    /// Locale key of the heading.
    pub(in crate::analytics::tuner) label: &'static str,
    /// Preferred width, font-scaled px; the coin column is the flexible remainder.
    pub(in crate::analytics::tuner) w: f32,
    /// How narrow the column may be squeezed.
    pub(in crate::analytics::tuner) min_w: f32,
    /// Numbers right-align, marks and words centre or lead.
    pub(in crate::analytics::tuner) align: Align,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::analytics::tuner) enum Align {
    Left,
    Right,
    Center,
}

pub(in crate::analytics::tuner) const COL_COIN: &str = "coin";
pub(in crate::analytics::tuner) const COL_TIME: &str = "time";
pub(in crate::analytics::tuner) const COL_BUY: &str = "buy";
pub(in crate::analytics::tuner) const COL_SELL: &str = "sell";
pub(in crate::analytics::tuner) const COL_RESULT: &str = "result";
pub(in crate::analytics::tuner) const COL_DURATION: &str = "duration";
pub(in crate::analytics::tuner) const COL_D5S: &str = "d5s";
pub(in crate::analytics::tuner) const COL_D1M: &str = "d1m";
pub(in crate::analytics::tuner) const COL_D1H: &str = "d1h";
pub(in crate::analytics::tuner) const COL_DMARK: &str = "dmark";
pub(in crate::analytics::tuner) const COL_PRICEBUG: &str = "pricebug";
pub(in crate::analytics::tuner) const COL_REASON: &str = "reason";
pub(in crate::analytics::tuner) const COL_TAPE: &str = "tape";
pub(in crate::analytics::tuner) const COL_MODEL: &str = "model";

const fn col(key: &'static str, label: &'static str, w: f32, min_w: f32, align: Align) -> DealCol {
    DealCol {
        key,
        label,
        w,
        min_w,
        align,
    }
}

/// The columns after the coin, in reading order: when, the two prices and what came of them,
/// the market at the buy, why it closed, and the two marks of this axis.
pub(in crate::analytics::tuner) const DEAL_COLS: &[DealCol] = &[
    col(
        COL_TIME,
        "analytics.ticks.col.time",
        64.0,
        56.0,
        Align::Right,
    ),
    col(COL_BUY, "analytics.ticks.col.buy", 72.0, 56.0, Align::Right),
    col(
        COL_SELL,
        "analytics.ticks.col.sell",
        72.0,
        56.0,
        Align::Right,
    ),
    col(
        COL_RESULT,
        "analytics.ticks.col.result",
        58.0,
        48.0,
        Align::Right,
    ),
    col(
        COL_DURATION,
        "analytics.ticks.col.duration",
        52.0,
        44.0,
        Align::Right,
    ),
    col(COL_D5S, "analytics.ticks.col.d5s", 46.0, 40.0, Align::Right),
    col(COL_D1M, "analytics.ticks.col.d1m", 46.0, 40.0, Align::Right),
    col(COL_D1H, "analytics.ticks.col.d1h", 46.0, 40.0, Align::Right),
    col(
        COL_DMARK,
        "analytics.ticks.col.dmark",
        46.0,
        40.0,
        Align::Right,
    ),
    col(
        COL_PRICEBUG,
        "analytics.ticks.col.pricebug",
        46.0,
        40.0,
        Align::Right,
    ),
    col(
        COL_REASON,
        "analytics.ticks.col.reason",
        96.0,
        60.0,
        Align::Left,
    ),
    col(
        COL_TAPE,
        "analytics.ticks.col.tape",
        40.0,
        36.0,
        Align::Center,
    ),
    col(
        COL_MODEL,
        "analytics.ticks.col.model",
        44.0,
        40.0,
        Align::Center,
    ),
];

/// Width the coin column never drops below (font-scaled px).
pub(in crate::analytics::tuner) const DEAL_COIN_MIN_W: f32 = 64.0;
pub(in crate::analytics::tuner) const DEAL_ROW_PAD_X: f32 = 8.0;
pub(in crate::analytics::tuner) const DEAL_ROW_GAP: f32 = 6.0;
