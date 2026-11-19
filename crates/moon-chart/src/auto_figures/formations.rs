//! `Formation`: chart patterns of two lines — triangles, wedges, channels, flags and pennants, and
//! double tops and bottoms — built on the pivots and the line search of `trend`.
//!
//! # Two-line formations (triangle, wedge, channel, flag)
//!
//! The sides are the `Trend` line search run twice: once through pivot HIGHS (the upper side) and
//! once through pivot LOWS (the lower side), each with the row's touch tolerance and
//! `min_touches`, at most [`MAX_BODY_BREAKS`] bodies across a side between its own touches (the
//! allowance a squeeze's sloped half gets, for the same reason: one wide bar does not undo a
//! pattern, two make the line a guess), and no
//! slope bound — a horizontal side is what an ascending or descending triangle and a range are made
//! of. The strongest [`MAX_SIDE_LINES`] lines of each side are paired, and a pair is a formation
//! when:
//!
//! - its span — first touch of either side to last touch of either — is at least `min_bars` bars,
//!   and its sides SHARE at least half of it: the stretch from the later first touch to the earlier
//!   last touch, where both sides are lines through their own touches, is that long;
//! - the upper side lies above the lower at the span's start, at its end and at the last closed
//!   bar: the lines have not crossed, so a converging pattern has not run past its apex;
//! - at most [`MAX_SPAN_BREAKS`] closes inside the span lie outside the pair by more than the
//!   tolerance — the sides' own body checks do not see the stretch where a side is extrapolated
//!   to the other side's touches;
//! - it still holds under the row's `broken` policy: a valid close after the last touch beyond
//!   EITHER side is a break (the latest episode of the two, see `scan_break`), and `Remove` drops
//!   the formation, `Keep { bars }` keeps it flagged, exactly as a trend line.
//!
//! What it is, from the width `w = upper − lower` at the start (`w0`) and the end (`w1`) of the
//! shared stretch — never on a side's extrapolation past its own touches:
//!
//! - **channel** — `|w1 / w0 − 1| ≤ parallel_width_pct / 100`: the sides are parallel, sloped or
//!   both flat. Both flat (see below) is a RANGE, and its sides are reported as two horizontal
//!   levels — the highest touch of the upper side and the lowest of the lower — which is what its
//!   breaks are judged against and what draws as the box of the zone; a line through the first
//!   and last touch would tilt by the touches' scatter and lose that box;
//! - **converging** — `w1 / w0` below that band. A side is FLAT when its price moves by at most
//!   twice the touch tolerance over the shared stretch (its touches may scatter by a tolerance either way,
//!   so a line through them may tilt by that much without being sloped). A converging pair is a
//!   **wedge** when both sides slope the same way and neither is flat, otherwise a **triangle** —
//!   symmetric (opposite slopes), ascending or descending (one side flat);
//! - **broadening** (`w1 / w0` above the band) — none of the kinds here; not reported.
//!
//! A channel or converging pair is a **flag** (a pennant when converging) when:
//!
//! - it is no longer than `flag_max_bars`;
//! - the `impulse_bars` bars ending at its first touch (that bar included, so `impulse_bars`
//!   bars in all) hold a pole of at least `impulse_pct` of the price it started from. The pole is
//!   the NET move INTO the formation: from the open of the window's first bar to the CLOSE of the
//!   bar of the first touch — a rise when that close is above the window's open, a fall when it
//!   is below. Not the widest swing inside the window: after a crash whose bar leaves a long lower
//!   wick, the bounce off the wick is a swing in the window, but the move that brought price to the
//!   pattern is the crash (LinKvo, 19.11.2026). And not the touch bar's own high or low either: in
//!   a flat market one long wick on that bar is not a move price made — measured to the wick, a
//!   flat with one shadow passed `impulse_pct` by itself and became a "bull flag". Never one bar's
//!   own range: the window needs two valid bars, so the pole always spans bars;
//! - its width at the start is at most [`FLAG_MAX_RETRACE`] of the pole's height;
//! - it does not run WITH the pole: its midline — halfway between the sides — is flat (it moves
//!   by at most twice the touch tolerance over the shared stretch, the flat-side rule below) or
//!   slopes against the pole. A flag after a rise drifts down or sideways, a pennant converges
//!   around a flat midline; a channel or wedge that keeps climbing after a rise is the trend
//!   going on, not a pause in it, and is reported as the channel or wedge it is.
//!
//! The flag reading wins when the row looks for flags; when it does not, or the pair fails a test
//! above, the pair is reported as the channel or triangle it is, if the row looks for that kind.
//!
//! Each line takes part in at most one formation, and one stretch of bars holds one: the pairs are
//! taken greedily, most touches first, then the longer span, and a pair whose span shares more than
//! half of the shorter span with one already taken is a second reading of that pattern and is
//! skipped.
//!
//! # Double tops and bottoms
//!
//! Two pivot highs (lows) within the row's double tolerance of each other, at least `min_bars` bars
//! apart, with no bar between them reaching above the second by more than that tolerance (below
//! it, for a bottom), no third pivot of the kind between them inside the band (that is a triple
//! top or a range), and a trough between them (a peak, for a bottom) at least `double_depth_pct`
//! deep, measured from the nearer extreme. The second extreme is taken newest first; the only
//! earlier extreme it can pair with is the NEAREST one in its band. Each pivot takes part in at
//! most one double, and one stretch of bars holds one double of a kind (the overlap rule above). Drawn as a pair of horizontal sides between the two extremes' times: the
//! level of the extremes (the higher top, the lower bottom — the price a close must pass to undo
//! the pattern) and the neckline through the trough (the peak). A close beyond either after the
//! second extreme is a break under the row's policy: through the neckline it completes the pattern,
//! through the level it undoes it, and both are the event layer's to tell.
//!
//! # Tolerances
//!
//! One unit for the row (`tolerance_unit`), as in `Squeeze`: under `Pct` the touches read
//! `tolerance_pct` and a double's equality `double_tolerance_pct`; under `Atr` they read
//! `tolerance_atr` and `double_tolerance_atr`, each times the same ATR over `ATR_PERIOD` bars (over
//! all the bars handed in) divided by the last close. No ATR (too few bars) finds nothing.
//! `Found::tolerance_frac` is the touch tolerance of a two-line formation and the equality
//! tolerance of a double.
//!
//! # Cost
//!
//! One pivot pass; two `trend::find_lines` searches, each capped at `trend::MAX_LINE_PIVOTS`
//! pivots (≈ 8·10⁶ touch checks at worst, see `trend`); at most `MAX_SIDE_LINES²` = 1 600 pairs, each
//! checked over its span and its tail — at most `n` bar reads, ≈ 8·10⁶ at the 5 000-bar ceiling;
//! and the doubles, each second extreme walking back bar by bar until a bar out-ranks it or the
//! window ends — at most `MAX_LINE_PIVOTS · n` = 10⁶ bar reads per kind. Run on a bar close, never
//! per frame.

use moon_core::config::auto_figures::{
    ATR_PERIOD, DetectorKind, DetectorParams, FormationParams, ToleranceUnit,
};
use moon_core::market::candles::ChartCandle;

use super::pivots::{Pivot, PivotKind, find_pivots};
use super::squeeze::MAX_BODY_BREAKS;
use super::trend::{Line, LineSpec, MAX_LINE_PIVOTS, find_lines};
use super::{
    Broken, DetectCtx, Detector, Found, FoundShape, LineTag, Seg, Side, Touch, atr, bar_ok,
    close_of, open_of, pct_frac, positive, scan_break, survives, tolerance_frac, touch_strength,
    touch_volume, window, within,
};

/// Closes allowed outside the pair, by more than the touch tolerance, between the formation's first
/// and last touch.
pub const MAX_SPAN_BREAKS: usize = 1;

/// Most lines of each side that are paired: the strongest ones, in the order the line search ranks
/// them (most touches first). Bounds the pairing at `MAX_SIDE_LINES²` pairs whatever the
/// parameters.
pub const MAX_SIDE_LINES: usize = 40;

/// Widest a flag may be at its start, as a share of its pole's height: a flag is a pause in a move,
/// and a consolidation wider than half the move before it is a range of its own.
pub const FLAG_MAX_RETRACE: f64 = 0.5;

/// The `Formation` registry row.
pub struct Formation;

impl Detector for Formation {
    fn kind(&self) -> DetectorKind {
        DetectorKind::Formation
    }

    fn detect(&self, bars: &[ChartCandle], params: &DetectorParams, ctx: &DetectCtx) -> Vec<Found> {
        let DetectorParams::Formation(p) = params else {
            return Vec::new();
        };
        formations(bars, p, ctx.tf_ms)
    }
}

/// Every formation of `bars` under `p`; see the module doc for the procedure.
fn formations(bars: &[ChartCandle], p: &FormationParams, tf_ms: i64) -> Vec<Found> {
    let two_line = p.triangle || p.wedge || p.channel || p.flag;
    if !two_line && !p.double {
        return Vec::new();
    }
    let atr = match p.tolerance_unit {
        ToleranceUnit::Pct => None,
        ToleranceUnit::Atr => atr(bars, ATR_PERIOD),
    };
    let tol = |pct: f32, share: f32| tolerance_frac(p.tolerance_unit, pct, share, atr, bars);
    let (Some(touch_tol), Some(double_tol)) = (
        tol(p.tolerance_pct, p.tolerance_atr),
        tol(p.double_tolerance_pct, p.double_tolerance_atr),
    ) else {
        return Vec::new();
    };
    let win = window(bars, p.lookback_bars);
    let Some(last_t) = win.iter().rev().find(|b| bar_ok(b)).map(|b| b.t_open_ms) else {
        return Vec::new();
    };
    let mut pivots = Vec::new();
    find_pivots(win, p.pivot_left, p.pivot_right, &mut pivots);
    let mut out = Vec::new();
    if two_line {
        two_line_formations(win, &pivots, p, touch_tol, tf_ms, last_t, &mut out);
    }
    if p.double {
        for kind in [PivotKind::High, PivotKind::Low] {
            doubles(win, &pivots, kind, p, double_tol, &mut out);
        }
    }
    out
}

/// A pair of sides that passed every test, before the greedy pick.
struct Candidate {
    upper: usize,
    lower: usize,
    tag: LineTag,
    touches: usize,
    first_idx: usize,
    last_idx: usize,
    broken: Option<Broken>,
    /// A range's `(upper, lower)` levels, when the pair is a channel of two flat sides.
    range: Option<(f64, f64)>,
}

/// The triangles, wedges, channels and flags of `win`, appended to `out`.
fn two_line_formations(
    win: &[ChartCandle],
    pivots: &[Pivot],
    p: &FormationParams,
    tol: f64,
    tf_ms: i64,
    last_t: f64,
    out: &mut Vec<Found>,
) {
    let spec = |kind| LineSpec {
        kind,
        tol_frac: tol,
        min_touches: usize::from(p.min_touches),
        max_body_breaks: MAX_BODY_BREAKS,
        min_slope_frac: 0.0,
        max_slope_frac: f64::INFINITY,
    };
    let mut highs = find_lines(win, pivots, &spec(PivotKind::High), tf_ms);
    let mut lows = find_lines(win, pivots, &spec(PivotKind::Low), tf_ms);
    highs.truncate(MAX_SIDE_LINES);
    lows.truncate(MAX_SIDE_LINES);

    let mut candidates = Vec::new();
    for (ui, upper) in highs.iter().enumerate() {
        for (li, lower) in lows.iter().enumerate() {
            if let Some(c) = classify(win, upper, lower, p, tol, last_t) {
                candidates.push(Candidate {
                    upper: ui,
                    lower: li,
                    ..c
                });
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.touches
            .cmp(&a.touches)
            .then((b.last_idx - b.first_idx).cmp(&(a.last_idx - a.first_idx)))
            .then(a.upper.cmp(&b.upper))
            .then(a.lower.cmp(&b.lower))
    });

    let mut upper_used = vec![false; highs.len()];
    let mut lower_used = vec![false; lows.len()];
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for c in candidates {
        if upper_used[c.upper] || lower_used[c.lower] {
            continue;
        }
        if spans
            .iter()
            .any(|&s| mostly_overlaps(s, (c.first_idx, c.last_idx)))
        {
            continue;
        }
        upper_used[c.upper] = true;
        lower_used[c.lower] = true;
        spans.push((c.first_idx, c.last_idx));
        let (upper, lower) = (&highs[c.upper], &lows[c.lower]);
        let touches = merge_touches(&upper.touches, &lower.touches);
        let (upper_seg, lower_seg) = match c.range {
            Some((hi, lo)) => (flat(upper, hi), flat(lower, lo)),
            None => (seg(upper), seg(lower)),
        };
        out.push(Found {
            shape: FoundShape::Pair {
                upper: upper_seg,
                lower: lower_seg,
            },
            first_ms: upper.t0_ms.min(lower.t0_ms),
            last_ms: upper.t1_ms.max(lower.t1_ms),
            strength: touch_strength(touches.len(), c.first_idx, c.last_idx, win.len()),
            touch_volume: touch_volume(win, &touches),
            touches,
            broken: c.broken,
            tolerance_frac: tol,
            tag: Some(c.tag),
        });
    }
}

/// Whether two bar spans `(first, last)` share more than half of the shorter one — one stretch of
/// the chart, which holds one two-line formation: two pairs of lines over the same bars are two
/// readings of one pattern, and drawing both only crosses the chart with four lines.
fn mostly_overlaps(a: (usize, usize), b: (usize, usize)) -> bool {
    let shared = a.1.min(b.1).saturating_sub(a.0.max(b.0));
    let shorter = (a.1 - a.0).min(b.1 - b.0);
    2 * shared > shorter
}

/// A range side: horizontal at `price`, between the side's first and last touch.
fn flat(line: &Line, price: f64) -> Seg {
    Seg {
        t0_ms: line.t0_ms,
        p0: price,
        t1_ms: line.t1_ms,
        p1: price,
    }
}

/// A side as the shape's segment, through its first and last touch.
fn seg(line: &Line) -> Seg {
    Seg {
        t0_ms: line.t0_ms,
        p0: line.p0,
        t1_ms: line.t1_ms,
        p1: line.p1,
    }
}

/// Whether `upper` and `lower` form a formation the row looks for, and which; `None` when they do
/// not (see the module doc). The returned candidate's `upper` / `lower` indices are the caller's
/// to fill.
fn classify(
    win: &[ChartCandle],
    upper: &Line,
    lower: &Line,
    p: &FormationParams,
    tol: f64,
    last_t: f64,
) -> Option<Candidate> {
    let first_idx = upper.first_idx.min(lower.first_idx);
    let last_idx = upper.last_idx.max(lower.last_idx);
    let span_bars = last_idx - first_idx;
    if span_bars < usize::try_from(p.min_bars).unwrap_or(usize::MAX) {
        return None;
    }
    // The span, and the stretch where both sides exist between their own touches.
    let (t_s, t_e) = (upper.t0_ms.min(lower.t0_ms), upper.t1_ms.max(lower.t1_ms));
    let (t_a, t_b) = (upper.t0_ms.max(lower.t0_ms), upper.t1_ms.min(lower.t1_ms));
    // The sides must share at least half the span: two lines one mostly after the other are two
    // unrelated swings, and comparing their width would read one side's extrapolation.
    if !(t_b > t_a && 2.0 * (t_b - t_a) >= t_e - t_s) {
        return None;
    }
    // In order over the whole span and at the last bar: not crossed, not past an apex.
    let ordered = |t: f64| Some(upper.price_at(t)? > lower.price_at(t)?);
    if !(ordered(t_s)? && ordered(t_e)? && ordered(last_t)?) {
        return None;
    }
    // The width, and each side's move, measured where both sides are their own touches' lines.
    let (us, ue) = (upper.price_at(t_a)?, upper.price_at(t_b)?);
    let (ls, le) = (lower.price_at(t_a)?, lower.price_at(t_b)?);
    let (w0, w1) = (us - ls, ue - le);
    if !(w0 > 0.0 && w1 > 0.0) {
        return None;
    }
    if span_breaks(win, first_idx, last_idx, upper, lower, tol) > MAX_SPAN_BREAKS {
        return None;
    }
    let ratio = w1 / w0;
    let band = pct_frac(p.parallel_width_pct);
    // A side is flat when it moves by at most twice the touch tolerance over the shared stretch.
    let dir = |from: f64, to: f64| {
        let d = (to - from) / from;
        if d.abs() <= 2.0 * tol {
            0
        } else {
            d.signum() as i8
        }
    };
    let (du, dl) = (dir(us, ue), dir(ls, le));
    let geometric = if (ratio - 1.0).abs() <= band {
        LineTag::Channel
    } else if ratio < 1.0 {
        if du != 0 && du == dl {
            LineTag::Wedge
        } else {
            LineTag::Triangle
        }
    } else {
        // Broadening: none of the kinds here.
        return None;
    };
    // The midline's own direction, by the same flat rule as a side's.
    let midline = dir((us + ls) / 2.0, (ue + le) / 2.0);
    let is_flag = p.flag
        && span_bars <= usize::try_from(p.flag_max_bars).unwrap_or(usize::MAX)
        && pole(win, first_idx, p.impulse_bars).is_some_and(|pole| {
            pole.frac >= pct_frac(p.impulse_pct)
                && w0 <= FLAG_MAX_RETRACE * pole.height
                && midline != pole.dir
        });
    let wanted = |tag: LineTag| match tag {
        LineTag::Channel => p.channel,
        LineTag::Wedge => p.wedge,
        LineTag::Triangle => p.triangle,
        _ => false,
    };
    let tag = if is_flag {
        LineTag::Flag
    } else if wanted(geometric) {
        geometric
    } else {
        return None;
    };
    // A channel of two flat sides is a range, drawn and judged as two levels (module doc).
    let range = (tag == LineTag::Channel && du == 0 && dl == 0)
        .then(|| range_levels(upper, lower))
        .flatten();
    let broken = match range {
        Some((hi, lo)) => latest(
            scan_break(win, last_idx, Side::Below, |_| Some(hi)),
            scan_break(win, last_idx, Side::Above, |_| Some(lo)),
        ),
        None => latest_break(win, last_idx, upper, lower),
    };
    if !survives(broken, p.broken) {
        return None;
    }
    Some(Candidate {
        upper: 0,
        lower: 0,
        tag,
        touches: upper.touches.len() + lower.touches.len(),
        first_idx,
        last_idx,
        broken,
        range,
    })
}

/// A range's two levels: the highest touch of the upper side and the lowest of the lower, so the
/// box holds every swing that made it and a close beyond either is a real exit. `None` when a side
/// has no positive touch (never, for a side the line search returned).
fn range_levels(upper: &Line, lower: &Line) -> Option<(f64, f64)> {
    let hi = upper
        .touches
        .iter()
        .map(|t| t.price)
        .fold(f64::NAN, f64::max);
    let lo = lower
        .touches
        .iter()
        .map(|t| t.price)
        .fold(f64::NAN, f64::min);
    let (hi, lo) = (positive(hi)?, positive(lo)?);
    (hi > lo).then_some((hi, lo))
}

/// Valid closes in `win[first..=last]` outside the pair by more than `tol` of the side's price —
/// above the upper side or below the lower; a bar whose close is unknown has none. Counting stops
/// one past [`MAX_SPAN_BREAKS`], which already rejects the pair.
fn span_breaks(
    win: &[ChartCandle],
    first: usize,
    last: usize,
    upper: &Line,
    lower: &Line,
    tol: f64,
) -> usize {
    let Some(span) = win.get(first..=last) else {
        return usize::MAX;
    };
    span.iter()
        .filter(|b| bar_ok(b))
        .filter_map(|b| close_of(b).map(|close| (b, close)))
        .filter(|(b, close)| {
            let close = *close;
            let above = upper
                .price_at(b.t_open_ms)
                .is_some_and(|u| close > u * (1.0 + tol));
            let below = lower
                .price_at(b.t_open_ms)
                .is_some_and(|l| close < l * (1.0 - tol));
            above || below
        })
        .take(MAX_SPAN_BREAKS + 1)
        .count()
}

/// The latest break episode after `from_idx` of either side: a close above the upper side or below
/// the lower one. When both sides were broken, the episode that began later is the formation's —
/// price that left through the top and then fell through the bottom has left through the bottom.
fn latest_break(
    win: &[ChartCandle],
    from_idx: usize,
    upper: &Line,
    lower: &Line,
) -> Option<Broken> {
    let up = scan_break(win, from_idx, Side::Below, |t| upper.price_at(t));
    let down = scan_break(win, from_idx, Side::Above, |t| lower.price_at(t));
    latest(up, down)
}

/// The later-starting of two break episodes, the first on a tie.
fn latest(a: Option<Broken>, b: Option<Broken>) -> Option<Broken> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.at_ms > a.at_ms { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// A flag's pole: the move into the formation ([`pole`]).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pole {
    /// The move as a fraction of the price it started from.
    frac: f64,
    /// The move in price.
    height: f64,
    /// `1` for a rise, `-1` for a fall — compared with a midline's direction (`classify`'s `dir`).
    dir: i8,
}

/// The pole in the `bars` bars ending at `win[start]` (that bar included, `bars` bars in all): the
/// NET move from the open of the first valid bar of that range to the CLOSE of the last valid one —
/// the bar of the formation's first touch.
///
/// Above the range's open is a rise, below it a fall, level is no move (a zero pole). The close
/// rather than the touch bar's high or low: a wick is where price was rejected, not where the move
/// went, and in a flat market one long shadow on the touch bar measured to its extreme made a pole
/// out of nothing. The open of the range rather than its lowest low (highest high): a low inside
/// the window is a swing price passed through, and taking it as the base turned the bounce off a
/// crash's wick into a rise, and the falling channel after the crash into a "flag" against it.
///
/// Only KNOWN prices make a pole (a ring bar's open and close are unknown, see the module doc of
/// `super`): the base is the open of the first bar that has one, the end the close of the last bar
/// that has one — normally the touch bar itself; when its close is unknown, the nearest earlier
/// known close, the furthest the move is known to have gone.
///
/// `None` when no two distinct bars of the range give a known open and a later known close
/// (`bars < 2` included): a single bar's own range is not a move across bars, and the bar of the
/// first touch is part of the pattern. Without a pole the pair is reported as the channel or
/// triangle it is, never as a flag.
fn pole(win: &[ChartCandle], start: usize, bars: u32) -> Option<Pole> {
    let bars = usize::try_from(bars).unwrap_or(usize::MAX);
    let from = (start + 1).saturating_sub(bars);
    let range = win.get(from..=start)?;
    let first = range
        .iter()
        .position(|b| bar_ok(b) && open_of(b).is_some())?;
    let last = range
        .iter()
        .rposition(|b| bar_ok(b) && close_of(b).is_some())?;
    if last <= first {
        return None;
    }
    let base = open_of(&range[first])?;
    let close = close_of(&range[last])?;
    let (height, dir) = if close >= base {
        (close - base, 1)
    } else {
        (base - close, -1)
    };
    let frac = height / base;
    // `open_of` and `close_of` give positive finite prices, so `frac` is finite and not negative.
    Some(Pole { frac, height, dir })
}

/// The touches of both sides in time order, each pivot once (a bar can be a pivot high on one side
/// and a pivot low on the other).
fn merge_touches(a: &[Touch], b: &[Touch]) -> Vec<Touch> {
    let mut touches = Vec::with_capacity(a.len() + b.len());
    touches.extend_from_slice(a);
    touches.extend_from_slice(b);
    touches.sort_by(|x, y| x.t_ms.total_cmp(&y.t_ms).then(x.price.total_cmp(&y.price)));
    touches.dedup_by(|x, y| x.t_ms == y.t_ms && x.price == y.price);
    touches
}

/// The double tops (`kind = High`) or bottoms (`Low`) of `win`, appended to `out`; see the module
/// doc for the rule.
fn doubles(
    win: &[ChartCandle],
    pivots: &[Pivot],
    kind: PivotKind,
    p: &FormationParams,
    tol: f64,
    out: &mut Vec<Found>,
) {
    let piv: Vec<&Pivot> = pivots
        .iter()
        .filter(|q| q.kind == kind && q.price > 0.0)
        .collect();
    let piv = &piv[piv.len().saturating_sub(MAX_LINE_PIVOTS)..];
    let spec = DoubleSpec {
        top: kind == PivotKind::High,
        tol,
        min_bars: usize::try_from(p.min_bars).unwrap_or(usize::MAX),
        depth: pct_frac(p.double_depth_pct),
    };
    let top = spec.top;
    let mut used = vec![false; piv.len()];
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for j in (0..piv.len()).rev() {
        if used[j] {
            continue;
        }
        let second = piv[j];
        let Some(m) = first_extreme(win, piv, j, &used, &spec) else {
            continue;
        };
        let (first, between_idx, between) = (piv[m.first], m.between_idx, m.between);
        // One stretch, one double of a kind — the newest, which is taken first.
        if spans
            .iter()
            .any(|&s| mostly_overlaps(s, (first.idx, second.idx)))
        {
            continue;
        }
        used[j] = true;
        used[m.first] = true;
        spans.push((first.idx, second.idx));
        let level = if top {
            first.price.max(second.price)
        } else {
            first.price.min(second.price)
        };
        let (t0, t1) = (first.t_ms, second.t_ms);
        let level_seg = Seg {
            t0_ms: t0,
            p0: level,
            t1_ms: t1,
            p1: level,
        };
        let neck_seg = Seg {
            t0_ms: t0,
            p0: between,
            t1_ms: t1,
            p1: between,
        };
        let (upper, lower) = if top {
            (level_seg, neck_seg)
        } else {
            (neck_seg, level_seg)
        };
        let up = scan_break(win, second.idx, Side::Below, |_| Some(upper.p0));
        let down = scan_break(win, second.idx, Side::Above, |_| Some(lower.p0));
        let broken = latest(up, down);
        if !survives(broken, p.broken) {
            continue;
        }
        let touches = vec![
            Touch {
                t_ms: first.t_ms,
                price: first.price,
            },
            Touch {
                t_ms: win[between_idx].t_open_ms,
                price: between,
            },
            Touch {
                t_ms: second.t_ms,
                price: second.price,
            },
        ];
        out.push(Found {
            shape: FoundShape::Pair { upper, lower },
            first_ms: t0,
            last_ms: t1,
            strength: touch_strength(touches.len(), first.idx, second.idx, win.len()),
            touch_volume: touch_volume(win, &touches),
            touches,
            broken,
            tolerance_frac: tol,
            tag: Some(if top {
                LineTag::DoubleTop
            } else {
                LineTag::DoubleBottom
            }),
        });
    }
}

/// What a double is looked for with, from the row's parameters.
#[derive(Clone, Copy)]
struct DoubleSpec {
    /// Tops (pivot highs) rather than bottoms.
    top: bool,
    /// The equality tolerance, fraction of price.
    tol: f64,
    /// Fewest bars between the two extremes.
    min_bars: usize,
    /// Shallowest trough (peak), fraction of the nearer extreme's price.
    depth: f64,
}

/// The earlier extreme of a double, and the trough (peak) between its two extremes.
struct DoubleMatch {
    /// Index of the first extreme in the pivot list.
    first: usize,
    /// Bar index and price of the trough (peak) between.
    between_idx: usize,
    between: f64,
}

/// The nearest earlier unused extreme that makes a double with `piv[j]`, with the trough (peak)
/// between them.
///
/// Walks back bar by bar from the second extreme, keeping the lowest low and the highest high of
/// the bars strictly between, and stops for good at the first bar — or the first earlier extreme —
/// that out-ranks the pair's level by more than the tolerance: no earlier extreme can pair across
/// it.
fn first_extreme(
    win: &[ChartCandle],
    piv: &[&Pivot],
    j: usize,
    used: &[bool],
    spec: &DoubleSpec,
) -> Option<DoubleMatch> {
    let DoubleSpec {
        top,
        tol,
        min_bars,
        depth,
    } = *spec;
    let second = piv[j];
    // The trough (lowest low) and the ceiling (highest high) between, for a top; mirrored for a
    // bottom: the peak (highest high) and the floor (lowest low).
    let mut trough: Option<(usize, f64)> = None;
    let mut ceiling = f64::NEG_INFINITY;
    let mut floor = f64::INFINITY;
    let mut peak: Option<(usize, f64)> = None;
    let mut cursor = second.idx;
    for i in (0..j).rev() {
        let first = piv[i];
        // Extend the "between" range down to just after `first`.
        while cursor > first.idx + 1 {
            cursor -= 1;
            let b = &win[cursor];
            if !bar_ok(b) {
                continue;
            }
            let (hi, lo) = (f64::from(b.high), f64::from(b.low));
            ceiling = ceiling.max(hi);
            floor = floor.min(lo);
            if trough.is_none_or(|(_, t)| lo < t) {
                trough = Some((cursor, lo));
            }
            if peak.is_none_or(|(_, t)| hi > t) {
                peak = Some((cursor, hi));
            }
        }
        // An earlier extreme, or a bar between, past the second extreme's band: the series made a
        // higher top (a lower bottom) there, and nothing earlier pairs across it.
        let out_ranked = if top {
            ceiling > second.price * (1.0 + tol) || first.price > second.price * (1.0 + tol)
        } else {
            floor < second.price * (1.0 - tol) || first.price < second.price * (1.0 - tol)
        };
        if out_ranked {
            return None;
        }
        // A lesser swing inside the pattern: walk on past it.
        if !within(first.price, second.price, tol) {
            continue;
        }
        // The nearest earlier extreme in the band is the ONLY candidate. One further back would
        // have this one between as a third touch of the level — a triple top or a range, not a
        // double — so a nearest one that does not qualify (already paired, too close, too
        // shallow a trough) ends the search.
        if used[i] || second.idx - first.idx < min_bars {
            return None;
        }
        let between = if top { trough } else { peak };
        let (at, price) = between.and_then(|(at, p)| positive(p).map(|p| (at, p)))?;
        let near = if top {
            first.price.min(second.price)
        } else {
            first.price.max(second.price)
        };
        let deep = if top {
            (near - price) / near
        } else {
            (price - near) / near
        };
        return (deep >= depth).then_some(DoubleMatch {
            first: i,
            between_idx: at,
            between: price,
        });
    }
    None
}

#[cfg(test)]
mod tests;
