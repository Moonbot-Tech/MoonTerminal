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
pub(in crate::analytics::tuner) const COL_RESULT: &str = "result";
pub(in crate::analytics::tuner) const COL_PROFIT: &str = "profit";
pub(in crate::analytics::tuner) const COL_DURATION: &str = "duration";
pub(in crate::analytics::tuner) const COL_HELD: &str = "held";
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

/// The columns after the coin, in reading order: when, what came of it (per cent and money),
/// how long it was held, how much tape the terminal holds around it, why it closed, and the
/// two marks of this axis. The prices and the market deltas were dropped on 2026-09-20: this
/// table is the axis's SAMPLE — which trades have their tape and how the model does on them —
/// and every other figure is one double-click away in the trade window.
pub(in crate::analytics::tuner) const DEAL_COLS: &[DealCol] = &[
    col(
        COL_TIME,
        "analytics.ticks.col.time",
        64.0,
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
        COL_PROFIT,
        "analytics.ticks.col.profit",
        72.0,
        56.0,
        Align::Right,
    ),
    col(
        COL_DURATION,
        "analytics.ticks.col.duration",
        52.0,
        44.0,
        Align::Right,
    ),
    col(
        COL_HELD,
        "analytics.ticks.col.held",
        76.0,
        60.0,
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
