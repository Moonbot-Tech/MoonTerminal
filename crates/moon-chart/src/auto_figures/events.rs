//! Events on found lines: lines + the current price + the last closed bars → which events each line
//! shows. Pure and cheap — O(lines × min(bars, R)) with `R` = [`bars_read`] of the row's policy (at
//! most [`ROLE_FLIP_LOOKBACK`] plus the false-break window plus two bars) — so it can run on every
//! price change, unlike the detectors. The deep part of the walk (the role flip) runs only for a
//! line the last bar touched and held; every other line costs at most `window + 2` bars, as before.
//!
//! Every comparison is against the line's price at the bar's OPEN time (the moment touches are
//! stamped with), through [`super::FoundShape::price_at`]: a squeeze is judged by its level. A
//! two-line formation (`FoundShape::Pair`) is judged side by side — each side as a line of its
//! own, the two answers merged into one story (`pair_events`): leaving the formation through
//! either side is its breakout, and a bar that crosses both sides tells the story of the side its
//! close ended up beyond. A close exactly on the line has no side and neither breaks nor
//! confirms anything. "The previous close" below is the nearest earlier close that HAD a side.
//!
//! - **Approach** — the current price is within `approach_pct` of the line at `at_ms`, measured as
//!   a fraction of the LINE's price; for a formation, of its side nearer the price
//!   ([`super::FoundShape::price_near`]).
//! - **Squeeze** — the figure is a squeeze whose `pressed` flag is set.
//! - **Breakout** — the last closed bar closed on the other side of the line from the previous
//!   close.
//! - **FalseBreak** — the last closed bar closed back on a side the line held before a run of
//!   closes beyond it, and that run began at most `window` bars before the last bar. `window` is
//!   [`false_break_window`] of the row's policy; under `Remove` it is zero and a false break is
//!   never shown — exactly what removing a level at once is for. A false break is the return
//!   crossing, so the same bar does not also report a `Breakout`.
//! - **Bounce** — the last closed bar's range reached the line and it closed on the same side as
//!   the previous close, and that side is the one the line has HELD: walking back from the last
//!   bar, no run of closes on the other side longer than `window` precedes the run on this side
//!   within [`ROLE_FLIP_LOOKBACK`] bars (a run of `window` closes or fewer, with a close on this
//!   side before it, was a false break of a line held on this side).
//! - **RoleFlip** — the same touch-and-hold, but the run on this side was preceded, within
//!   [`ROLE_FLIP_LOOKBACK`] bars, by a run of MORE than `window` closes on the other side: the line
//!   held price there, price broke through, and the line now holds it from the new side (a broken
//!   resistance holding as support, or the reverse). Under `Remove` (`window = 0`) any close on the
//!   other side is a real break, as it is for that policy everywhere. A role flip is a particular
//!   bounce, so a row that marks bounces but not role flips sees such a bar as a `Bounce`.
//! - **StopHunt** — the touch-and-hold bar's wick pierced the line by MORE than `stop_hunt_pct` of
//!   the line's price (below a held support, above a held resistance): the stops behind the line
//!   were taken and the bar still closed back. A refinement of the touch-and-hold, so it always
//!   comes WITH that bar's `Bounce` or `RoleFlip`, never alone.
//! - **Confluence** — judged ACROSS rows by the caller, which alone sees the lines of every
//!   timeframe ([`super::confluence`]); never set by this per-line pass. The caller inserts it into
//!   the line's set and filters it through the same `EventMarks::marks`.
//!
//! What one bar can show together. Its close tells ONE story of the line: a crossing (`Breakout` or
//! `FalseBreak` — the previous close on the other side) or a touch-and-hold (`Bounce` or `RoleFlip`
//! — the previous close on the same side). The two cases exclude each other, and inside each
//! exactly one event is chosen, so no bar is both a bounce and a breakout, or both a bounce and a
//! role flip. `StopHunt` only adds to a touch-and-hold; `Approach`, `Squeeze` and `Confluence`
//! describe the place, not the bar, and combine with any of them.

use moon_core::config::auto_figures::{AutoEvent, BrokenPolicy, EventMarks};
use moon_core::market::candles::ChartCandle;

use super::{Found, FoundShape, PairSide, Seg, Side, bar_ok, pct_frac, side_of};

/// The events one line shows, as a bit set — no allocation per line.
///
/// A `u8` holds all eight events there are; a ninth widens it to `u16` (the set never leaves the
/// process, so its width is free to change).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct EventSet(u8);

impl EventSet {
    /// Every event in its display order, the order [`EventSet::iter`] yields them in. Confluence
    /// last: it is a property of the place, not of what the price just did there.
    const ALL: [AutoEvent; 8] = [
        AutoEvent::Approach,
        AutoEvent::Squeeze,
        AutoEvent::Breakout,
        AutoEvent::FalseBreak,
        AutoEvent::Bounce,
        AutoEvent::RoleFlip,
        AutoEvent::StopHunt,
        AutoEvent::Confluence,
    ];

    /// The event's bit, one per event.
    fn bit(event: AutoEvent) -> u8 {
        match event {
            AutoEvent::Approach => 1,
            AutoEvent::Squeeze => 1 << 1,
            AutoEvent::Breakout => 1 << 2,
            AutoEvent::FalseBreak => 1 << 3,
            AutoEvent::Bounce => 1 << 4,
            AutoEvent::RoleFlip => 1 << 5,
            AutoEvent::StopHunt => 1 << 6,
            AutoEvent::Confluence => 1 << 7,
        }
    }

    /// Whether `event` is in the set.
    pub fn contains(self, event: AutoEvent) -> bool {
        self.0 & Self::bit(event) != 0
    }

    /// Adds `event`.
    pub fn insert(&mut self, event: AutoEvent) {
        self.0 |= Self::bit(event);
    }

    /// Drops `event` — how a formation's two sides are merged into one story (`pair_events`).
    fn remove(&mut self, event: AutoEvent) {
        self.0 &= !Self::bit(event);
    }

    /// No event at all.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The events in the set, in display order.
    pub fn iter(self) -> impl Iterator<Item = AutoEvent> {
        Self::ALL.into_iter().filter(move |e| self.contains(*e))
    }
}

/// Bars within which a return counts as a false break: the bars a `Keep` policy keeps a broken
/// level for, and none for `Remove`.
pub fn false_break_window(policy: BrokenPolicy) -> u16 {
    match policy {
        BrokenPolicy::Remove => 0,
        BrokenPolicy::Keep { bars } => bars,
    }
}

/// Most bars back a role flip's break may lie: the touching bar's run of closes on the line's new
/// side must have begun within this many valid bars before it.
///
/// Thirty bars is a retest's horizon — half an hour on 1-minute bars, more than a day on 1-hour
/// ones. A line price has held from one side for longer than that has settled into its new role,
/// and a touch of it is a bounce.
pub const ROLE_FLIP_LOOKBACK: usize = 30;

/// The most of the newest valid closed bars [`line_events`] reads under `policy` — what a caller
/// keeping only the tail of a series must keep for the events to be exact (plus slack for
/// unreadable bars, which are skipped and do not count): the last bar, a run of up to
/// [`ROLE_FLIP_LOOKBACK`] bars on its side, and enough of the run before that to tell a false
/// break (`window` closes or fewer) from a side the line held.
///
/// Independent of the detector's own `DetectorParams::bars_needed`, which may be smaller (round
/// prices need one bar): the caller that holds the series fetches the larger of the two and keeps
/// this many newest bars for the events, whatever the detector read — on the chart that is
/// `moon-ui-gpui`'s `chartdx::auto_figures_sync::job`, and any other caller owes the same.
pub fn bars_read(policy: BrokenPolicy) -> usize {
    scan_len(usize::from(false_break_window(policy)))
}

/// [`bars_read`] for a false-break window of `window` bars.
fn scan_len(window: usize) -> usize {
    ROLE_FLIP_LOOKBACK.saturating_add(window).saturating_add(2)
}

/// The events of every line, one [`EventSet`] per line in input order, written into `out`.
///
/// Args:
///     lines: The kept figures.
///     bars: Closed bars of the row's timeframe, ascending; only the last [`bars_read`] valid
///         ones are read.
///     price: The current price, for `Approach`.
///     at_ms: The moment the current price belongs to, for projecting sloped lines.
///     marks: Which events the row shows, and the approach and stop-hunt distances; an unmarked
///         event is never set (an unmarked role flip reads as the bounce it is, when bounces are
///         marked — see `shown_events`).
///     policy: The row's broken policy, for the false-break window — the row's
///         `DetectorParams::broken_policy()`, which answers for every detector kind (a period
///         extreme, which has no policy of its own, answers the default). Taken as the policy
///         rather than the params so the scanner can call this without a row.
pub fn line_events(
    lines: &[Found],
    bars: &[ChartCandle],
    price: f64,
    at_ms: f64,
    marks: &EventMarks,
    policy: BrokenPolicy,
    out: &mut Vec<EventSet>,
) {
    out.clear();
    let window = usize::from(false_break_window(policy));
    let approach_frac = pct_frac(marks.approach_pct);
    let hunt_frac = pct_frac(marks.stop_hunt_pct);
    for line in lines {
        let mut set = EventSet::default();
        let shape = &line.shape;
        // A formation is approached at its nearer side; every other shape has one line.
        if let Some(at) = shape.price_near(at_ms, price) {
            if price.is_finite() && price > 0.0 && super::within(price, at, approach_frac) {
                set.insert(AutoEvent::Approach);
            }
        }
        if matches!(shape, FoundShape::Squeeze { pressed: true, .. }) {
            set.insert(AutoEvent::Squeeze);
        }
        match shape {
            FoundShape::Pair { upper, lower } => {
                pair_events(bars, upper, lower, window, hunt_frac, &mut set);
            }
            _ => bar_events(bars, |t| shape.price_at(t), window, hunt_frac, &mut set),
        }
        out.push(shown_events(set, marks));
    }
}

/// The bar events of a two-line formation: each side judged as a line of its own by
/// [`bar_events`], then merged so the bar still tells ONE story.
///
/// Which side's crossing counts is decided by where the last close lies:
///
/// - **beyond a side** (above the upper, below the lower) — only THAT side's crossing is the bar's
///   story. A bar that ran through a narrow formation from above its top to below its bottom
///   crossed both lines: judged alone, the upper side calls it a return (a `FalseBreak`, when the
///   close before was a short run above it) and the lower side a `Breakout`. The close is outside
///   the formation, below it, so the formation broke down, and the bar shows the lower side's
///   `Breakout` alone; the upper side's crossing is dropped.
/// - **inside** — the close is back between the sides, and the crossing of whichever side the
///   previous close lay beyond is the story (a `FalseBreak` within the window, a `Breakout` back
///   in after a long stay outside). The previous close lay beyond one side at most, so at most one
///   side reports a crossing; should both (a close exactly on one side shifts which close counts as
///   "previous" for it), the upper side's is kept.
///
/// A crossing wins over a touch: a bar that closed through one side after its range brushed the
/// other broke the formation, and that is all it shows — never a `Bounce`, `RoleFlip` or
/// `StopHunt` with it. Without a crossing, the touch-and-hold events of both sides are kept (a bar
/// spanning a narrow formation can hold at both), a `RoleFlip` on one side absorbing a plain
/// `Bounce` on the other.
fn pair_events(
    bars: &[ChartCandle],
    upper: &Seg,
    lower: &Seg,
    window: usize,
    hunt_frac: f64,
    set: &mut EventSet,
) {
    let mut up = EventSet::default();
    let mut down = EventSet::default();
    bar_events(bars, |t| upper.price_at(t), window, hunt_frac, &mut up);
    bar_events(bars, |t| lower.price_at(t), window, hunt_frac, &mut down);
    let crossed =
        |e: EventSet| e.contains(AutoEvent::Breakout) || e.contains(AutoEvent::FalseBreak);
    let drop_crossing = |e: &mut EventSet| {
        e.remove(AutoEvent::Breakout);
        e.remove(AutoEvent::FalseBreak);
    };
    match last_close_beyond(bars, upper, lower) {
        Some(PairSide::Upper) => drop_crossing(&mut down),
        Some(PairSide::Lower) => drop_crossing(&mut up),
        None if crossed(up) => drop_crossing(&mut down),
        None => {}
    }
    let mut sides = EventSet(up.0 | down.0);
    if crossed(sides) {
        sides.remove(AutoEvent::Bounce);
        sides.remove(AutoEvent::RoleFlip);
        sides.remove(AutoEvent::StopHunt);
    } else if sides.contains(AutoEvent::RoleFlip) {
        sides.remove(AutoEvent::Bounce);
    }
    for e in sides.iter() {
        set.insert(e);
    }
}

/// Which side of the pair the last valid close lies beyond — above `upper` or below `lower` at the
/// bar's open time — or `None` when it is inside, on a side, or a side has no price there.
fn last_close_beyond(bars: &[ChartCandle], upper: &Seg, lower: &Seg) -> Option<PairSide> {
    let last = bars.iter().rev().find(|b| bar_ok(b))?;
    let close = f64::from(last.close);
    if upper.price_at(last.t_open_ms).is_some_and(|u| close > u) {
        Some(PairSide::Upper)
    } else if lower.price_at(last.t_open_ms).is_some_and(|l| close < l) {
        Some(PairSide::Lower)
    } else {
        None
    }
}

/// The events of `set` a row with `marks` shows.
///
/// Every event passes through `marks.marks` — except a `RoleFlip` the row does not mark: it shows
/// as a `Bounce` when the row marks that. A role flip IS a bounce (a touch held on the line's side),
/// only one with a known history behind it, and [`bar_events`] picks the more specific name; a row
/// that switched role flips off without switching bounces off asked for the plain story, not for
/// silence on that bar.
fn shown_events(set: EventSet, marks: &EventMarks) -> EventSet {
    let mut shown = EventSet::default();
    for e in set.iter() {
        if marks.marks(e) {
            shown.insert(e);
        } else if e == AutoEvent::RoleFlip && marks.marks(AutoEvent::Bounce) {
            shown.insert(AutoEvent::Bounce);
        }
    }
    shown
}

/// `Breakout`, `FalseBreak`, `Bounce`, `RoleFlip` and `StopHunt` from the last closed bars, into
/// `set` (see the module doc). `hunt_frac` is the stop-hunt pierce as a fraction of the line's
/// price; zero never reports a stop hunt.
fn bar_events(
    bars: &[ChartCandle],
    line_at: impl Fn(f64) -> Option<f64>,
    window: usize,
    hunt_frac: f64,
    set: &mut EventSet,
) {
    // The last valid bars, newest first, each with its line price and its side (`None` on the
    // line or where the line has no price). Bounded by `scan_len`: nothing past it can change any
    // answer.
    let mut recent = bars
        .iter()
        .rev()
        .filter(|b| bar_ok(b))
        .take(scan_len(window))
        .map(|b| {
            let line = line_at(b.t_open_ms);
            let side = line.and_then(|l| side_of(f64::from(b.close), l));
            (b, line, side)
        });
    let Some((last, last_line, Some(last_side))) = recent.next() else {
        return;
    };
    // `prev_side` is the previous close. The run of closes NOT on `last_side` before the last bar
    // must, for a false break, hold a close beyond, start within the window, and be preceded by a
    // close on `last_side` — the side the line held before the break. `k` counts the valid bars
    // walked past the last one, so the touch-and-hold walk below continues from where this stops.
    let mut prev_side: Option<Side> = None;
    let mut run_start: Option<usize> = None;
    let mut origin_seen = false;
    let mut k = 0usize;
    for (_, _, side) in recent.by_ref() {
        k += 1;
        let Some(side) = side else {
            continue;
        };
        prev_side.get_or_insert(side);
        if side == last_side {
            origin_seen = true;
            break;
        }
        run_start = Some(k);
        if k > window {
            // The run already began more than `window` bars back: no false break can follow.
            break;
        }
    }
    let false_break = origin_seen && run_start.is_some_and(|k| k <= window);
    if false_break {
        set.insert(AutoEvent::FalseBreak);
    } else if prev_side == Some(last_side.opposite()) {
        set.insert(AutoEvent::Breakout);
    }

    // The touch-and-hold case: the previous close on the same side, the last bar's range on the
    // line.
    if prev_side != Some(last_side) {
        return;
    }
    let Some(line) = last_line else {
        return;
    };
    let (low, high) = (f64::from(last.low), f64::from(last.high));
    if !(low <= line && line <= high) {
        return;
    }
    let sides = recent.map(|(_, _, side)| side);
    set.insert(if side_flipped(sides, k, last_side, window) {
        AutoEvent::RoleFlip
    } else {
        AutoEvent::Bounce
    });
    let pierced = match last_side {
        Side::Above => low < line * (1.0 - hunt_frac),
        Side::Below => high > line * (1.0 + hunt_frac),
    };
    if hunt_frac > 0.0 && pierced {
        set.insert(AutoEvent::StopHunt);
    }
}

/// Whether the line held price on the side OPPOSITE `held` before the current run on `held`.
///
/// Continues the newest-first walk after valid bar `k` (the previous close, on `held`): the first
/// close on the other side must come within [`ROLE_FLIP_LOOKBACK`] bars of the last one, and the
/// run of closes there must be longer than `window`. A shorter run with a close on `held` before
/// it was a false break of a line held on `held`'s side; a run cut off by the end of the bars read
/// at `window` closes or fewer is no evidence of a flip either. Bars with no side are skipped
/// without ending a run.
fn side_flipped(
    sides: impl Iterator<Item = Option<Side>>,
    mut k: usize,
    held: Side,
    window: usize,
) -> bool {
    let mut beyond = 0usize;
    for side in sides {
        k += 1;
        let Some(side) = side else {
            continue;
        };
        if side == held {
            // Back on `held` after a short run beyond (a false break), or still on `held` past the
            // lookback (a settled role): no flip either way.
            if beyond > 0 || k >= ROLE_FLIP_LOOKBACK {
                return false;
            }
            continue;
        }
        if beyond == 0 && k > ROLE_FLIP_LOOKBACK {
            return false;
        }
        beyond += 1;
        if beyond > window {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests;
