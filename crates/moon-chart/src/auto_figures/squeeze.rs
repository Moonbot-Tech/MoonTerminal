//! `Squeeze`: a horizontal level plus a sloped line from its OTHER side converging into it —
//! rising lows pressing a resistance, falling highs pressing a support. One formation, one figure.
//!
//! The level half is exactly a `Swing` level of `params.level`, under the level's own
//! `BrokenPolicy`: `Remove` drops a squeeze whose level broke, `Keep { bars }` keeps it with the
//! level's break in `Found::broken`, so the breakout of a squeeze is visible for those bars and the
//! event layer can mark it as a breakout or a false break. The sloped half is a line through pivot LOWS for a resistance (pivot HIGHS for a support) found by the
//! `Trend` line search with `trend_tolerance_pct` and `min_trend_touches`, which must:
//! - rise toward a resistance / fall toward a support (converge, not diverge);
//! - have every touch strictly on the level's original side;
//! - overlap the level in time (its last touch after the level's first touch);
//! - still hold: no valid close after its last touch on the wrong side of it (below rising lows,
//!   above falling highs) — a line price has already left presses nothing;
//! - not have passed through the level by more than `pressed_pct` at the last bar — past that the
//!   formation is over.
//!
//! `pressed` is set when the sloped line, projected to the last valid bar, is within `pressed_pct`
//! of the level — measured as a fraction of the LEVEL price — on either side of it, AND price is
//! still inside the formation: not past the level by more than its break buffer (a broken level
//! presses nothing — the 21.11.2026 audit found half the screener's squeezes on a level price
//! stood beyond), and, while the line has not yet reached the level, not past the line by more
//! than the line's break buffer. "Past" is the last bar's close side (`super::close_side`), as a
//! break reads it: a ring bar without a close is past a line only when its whole range is. A
//! squeeze kept flagged broken by `Keep { bars }` is still drawn, and is not pressed while price
//! stands beyond its level; once price is back inside within the policy — a false break — it
//! can press again.
//!
//! Each level and each sloped line takes part in at most one formation: valid pairs are taken
//! greedily, the converging ones (line still on the level's original side) before those already
//! past the level, and within each the one whose projected gap to the level is smallest first.
//!
//! Both halves use ONE tolerance unit, `level.tolerance_unit`: under `Pct` the level reads
//! `level.tolerance_pct` and the line `trend_tolerance_pct`; under `Atr` the level reads
//! `level.tolerance_atr` and the line `trend_tolerance_atr`, each times the same ATR over
//! `ATR_PERIOD` bars (computed once, over all the bars handed in) divided by the last close. No ATR
//! (too few bars) finds nothing. `pressed_pct` stays in % whatever the unit.
//!
//! The contract gives the sloped half no body-break or slope bounds of its own; it is searched with
//! [`MAX_BODY_BREAKS`] and with no slope bounds beyond the direction above.
//!
//! Cost: one pivot pass, the `Swing` levels and two `Trend` line searches (each capped at
//! `trend::MAX_LINE_PIVOTS` pivots, see that module), plus O(L·S) to pair `L` levels with `S`
//! lines.

use moon_core::config::auto_figures::{
    ATR_PERIOD, DetectorKind, DetectorParams, SqueezeParams, ToleranceUnit,
};
use moon_core::market::candles::ChartCandle;

use super::pivots::{PivotKind, find_pivots};
use super::swing::{Level, levels_from};
use super::trend::{Line, LineSpec, find_lines};
use super::{
    DetectCtx, Detector, Found, FoundShape, Side, Touch, atr, bar_ok, break_buffer, close_side,
    pct_frac, scan_break, tolerance_frac, touch_strength, touch_volume, window,
};

/// Bodies allowed to cross the sloped half between its first and last touch. One, not zero: a
/// single wide bar closing through a rising-lows line is common in a live squeeze and does not
/// undo the formation, while two already make the line a guess.
pub const MAX_BODY_BREAKS: usize = 1;

/// The `Squeeze` registry row.
pub struct Squeeze;

impl Detector for Squeeze {
    fn kind(&self) -> DetectorKind {
        DetectorKind::Squeeze
    }

    fn detect(&self, bars: &[ChartCandle], params: &DetectorParams, ctx: &DetectCtx) -> Vec<Found> {
        let DetectorParams::Squeeze(p) = params else {
            return Vec::new();
        };
        squeezes(bars, p, ctx.tf_ms, ctx.price_tick)
    }
}

/// A valid (level, line) pairing and its gap, fraction of the level price (negative = crossed).
struct Pair {
    level: usize,
    line: usize,
    gap: f64,
}

/// The level's and the sloped line's tolerances as fractions of price, in the formation's one unit
/// (`level.tolerance_unit`), from one ATR over all of `bars`, each at least two of the market's
/// `price_tick` when it is known; `None` under ATR without an ATR.
fn squeeze_tolerances(
    bars: &[ChartCandle],
    p: &SqueezeParams,
    price_tick: Option<f64>,
) -> Option<(f64, f64)> {
    let unit = p.level.tolerance_unit;
    let atr = match unit {
        ToleranceUnit::Pct => None,
        ToleranceUnit::Atr => atr(bars, ATR_PERIOD),
    };
    let level = tolerance_frac(
        unit,
        p.level.tolerance_pct,
        p.level.tolerance_atr,
        atr,
        bars,
        price_tick,
    )?;
    let line = tolerance_frac(
        unit,
        p.trend_tolerance_pct,
        p.trend_tolerance_atr,
        atr,
        bars,
        price_tick,
    )?;
    Some((level, line))
}

/// Every squeeze of `bars` under `p`; see the module doc for the procedure. `price_tick` is the
/// market's tick when known ([`DetectCtx::price_tick`]).
fn squeezes(
    bars: &[ChartCandle],
    p: &SqueezeParams,
    tf_ms: i64,
    price_tick: Option<f64>,
) -> Vec<Found> {
    let Some((level_frac, line_frac)) = squeeze_tolerances(bars, p, price_tick) else {
        return Vec::new();
    };
    let win = window(bars, p.level.lookback_bars);
    let Some(last) = win.iter().rev().find(|b| bar_ok(b)) else {
        return Vec::new();
    };
    let last_t = last.t_open_ms;
    let mut pivots = Vec::new();
    find_pivots(win, p.level.pivot_left, p.level.pivot_right, &mut pivots);
    // `levels_from` has already applied the level's policy: what is left is either unbroken or
    // kept flagged.
    let levels: Vec<Level> = levels_from(win, &pivots, &p.level, level_frac);
    if levels.is_empty() {
        return Vec::new();
    }
    let spec = |kind| LineSpec {
        kind,
        tol_frac: line_frac,
        min_touches: usize::from(p.min_trend_touches),
        max_body_breaks: MAX_BODY_BREAKS,
        min_slope_frac: 0.0,
        max_slope_frac: f64::INFINITY,
    };
    // Lows rise under a resistance (origin below), highs fall over a support (origin above).
    let lows = find_lines(win, &pivots, &spec(PivotKind::Low), tf_ms);
    let highs = find_lines(win, &pivots, &spec(PivotKind::High), tf_ms);
    let pressed_frac = pct_frac(p.pressed_pct);

    let lines_for = |origin: Side| match origin {
        Side::Below => &lows,
        Side::Above => &highs,
    };
    let mut pairs: Vec<(Side, Pair)> = Vec::new();
    for (li, level) in levels.iter().enumerate() {
        let lines = lines_for(level.origin);
        for (si, line) in lines.iter().enumerate() {
            if let Some(gap) = pair_gap(win, level, line, last_t, pressed_frac, line_frac) {
                pairs.push((
                    level.origin,
                    Pair {
                        level: li,
                        line: si,
                        gap,
                    },
                ));
            }
        }
    }
    // Converging pairs first, then the smallest gap either way (module doc): a pair already past
    // the level must not take a level from one still pressing into it.
    pairs.sort_by(|a, b| {
        (a.1.gap < 0.0)
            .cmp(&(b.1.gap < 0.0))
            .then(a.1.gap.abs().total_cmp(&b.1.gap.abs()))
            .then(a.1.level.cmp(&b.1.level))
            .then(a.1.line.cmp(&b.1.line))
    });

    let mut level_used = vec![false; levels.len()];
    let mut low_used = vec![false; lows.len()];
    let mut high_used = vec![false; highs.len()];
    let mut out = Vec::new();
    for (origin, pair) in pairs {
        let line_used = match origin {
            Side::Below => &mut low_used,
            Side::Above => &mut high_used,
        };
        if level_used[pair.level] || line_used[pair.line] {
            continue;
        }
        level_used[pair.level] = true;
        line_used[pair.line] = true;
        let level = &levels[pair.level];
        let line = &lines_for(origin)[pair.line];
        let touches = merge_touches(&level.touches, &line.touches);
        let first_idx = level.first_idx.min(line.first_idx);
        let last_idx = level.last_idx.max(line.last_idx);
        out.push(Found {
            shape: FoundShape::Squeeze {
                level: level.price,
                t0_ms: line.t0_ms,
                p0: line.p0,
                t1_ms: line.t1_ms,
                p1: line.p1,
                pressed: pair.gap <= pressed_frac
                    && price_inside(level, line, last, line_frac, pair.gap),
            },
            first_ms: touches.first().map_or(0.0, |t| t.t_ms),
            last_ms: touches.last().map_or(0.0, |t| t.t_ms),
            strength: touch_strength(touches.len(), first_idx, last_idx, win.len()),
            touch_volume: touch_volume(win, &touches),
            touches,
            broken: level.broken,
            // The band is drawn around the level, so it is the level's tolerance.
            tolerance_frac: level.tolerance_frac,
            tag: None,
            // Events judge a squeeze by its level, so the level's side.
            origin: Some(level.origin),
        });
    }
    out
}

/// Whether the last bar `last` closed inside the formation of `level` and `line`: not past the
/// level, on the side away from its origin, by more than the level's break buffer; and, while the
/// line has not reached the level (`gap > 0`), not past the line, on the level's origin side, by
/// more than the line's break buffer (`line_frac` is the line's touch tolerance), the line read at
/// the bar's open time. Past the apex (`gap <= 0`) only the level counts: the line has met it.
/// "Past" is [`close_side`] against the buffered line, so a bar without a close is past only when
/// its whole range is — the rule every break follows.
fn price_inside(level: &Level, line: &Line, last: &ChartCandle, line_frac: f64, gap: f64) -> bool {
    let past = |at: f64, buffer: f64, side: Side| {
        let shifted = match side {
            Side::Above => at * (1.0 + buffer),
            Side::Below => at * (1.0 - buffer),
        };
        close_side(last, shifted) == Some(side)
    };
    let beyond_level = level.origin.opposite();
    if past(
        level.price,
        break_buffer(level.tolerance_frac),
        beyond_level,
    ) {
        return false;
    }
    if gap <= 0.0 {
        return true;
    }
    // Past rising lows is below them, past falling highs above them: the level's origin side.
    line.price_at(last.t_open_ms)
        .is_some_and(|at| !past(at, break_buffer(line_frac), level.origin))
}

/// The touches of both halves in time order, each pivot once.
///
/// A pivot can belong to both: a level groups pivot highs AND lows within its band, so a low just
/// under a resistance can be both a level touch and a touch of the rising lows line. Counted twice,
/// it would inflate the formation's touch count and its strength.
fn merge_touches(level: &[Touch], line: &[Touch]) -> Vec<Touch> {
    let mut touches = Vec::with_capacity(level.len() + line.len());
    touches.extend_from_slice(level);
    touches.extend_from_slice(line);
    touches.sort_by(|a, b| a.t_ms.total_cmp(&b.t_ms).then(a.price.total_cmp(&b.price)));
    touches.dedup_by(|a, b| a.t_ms == b.t_ms && a.price == b.price);
    touches
}

/// The gap between `line` projected to `last_t` and `level`, as a fraction of the level price —
/// positive while the line is still on the level's original side — or `None` when the two do not
/// form a squeeze (see the module doc).
///
/// Two ways a formation is OVER and is refused here, both seen live on the bench: a valid close
/// after the line's last touch on the wrong side of the line by more than the break buffer of the
/// line's tolerance `line_frac` (price left the pressing line, so it presses nothing —
/// `scan_break` on the line, from its last touch), and a line projected past the level by more
/// than `pressed_frac` (the two crossed a while ago; what remains is two unrelated lines). A gap
/// in `[-pressed_frac, 0]` is a squeeze at its apex and still counts as pressed.
fn pair_gap(
    win: &[ChartCandle],
    level: &Level,
    line: &Line,
    last_t: f64,
    pressed_frac: f64,
    line_frac: f64,
) -> Option<f64> {
    if level.price <= 0.0 || line.t1_ms <= level.touches.first()?.t_ms {
        return None;
    }
    // Price sits on the line's side opposite the level: above rising lows, below falling highs.
    let line_origin = level.origin.opposite();
    let buffer = break_buffer(line_frac);
    if scan_break(win, line.last_idx, line_origin, buffer, |t| {
        line.price_at(t)
    })
    .is_some()
    {
        return None;
    }
    let converging = match level.origin {
        Side::Below => line.p1 > line.p0,
        Side::Above => line.p1 < line.p0,
    };
    let on_side = line.touches.iter().all(|t| match level.origin {
        Side::Below => t.price < level.price,
        Side::Above => t.price > level.price,
    });
    if !converging || !on_side {
        return None;
    }
    let projected = line.price_at(last_t)?;
    let gap = match level.origin {
        Side::Below => (level.price - projected) / level.price,
        Side::Above => (projected - level.price) / level.price,
    };
    (gap.is_finite() && gap >= -pressed_frac).then_some(gap)
}

#[cfg(test)]
mod tests;
