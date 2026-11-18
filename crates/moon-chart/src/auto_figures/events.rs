//! Events on found lines: lines + the current price + the last closed bars → which events each line
//! shows. Pure and cheap — O(lines × min(bars, W)) with `W = false-break window + 2` — so it can
//! run on every price change, unlike the detectors.
//!
//! Every comparison is against the line's price at the bar's OPEN time (the moment touches are
//! stamped with), through [`super::FoundShape::price_at`]: a squeeze is judged by its level. A
//! close exactly on the line has no side and neither breaks nor confirms anything.
//!
//! - **Approach** — the current price is within `approach_pct` of the line at `at_ms`, measured as
//!   a fraction of the LINE's price.
//! - **Squeeze** — the figure is a squeeze whose `pressed` flag is set.
//! - **Breakout** — the last closed bar closed on the other side of the line from the nearest
//!   earlier close that had a side.
//! - **FalseBreak** — the last closed bar closed back on a side the line held before a run of
//!   closes beyond it, and that run began at most `window` bars before the last bar. `window` is
//!   [`false_break_window`] of the row's policy; under `Remove` it is zero and a false break is
//!   never shown — exactly what removing a level at once is for. A false break is the return
//!   crossing, so the same bar does not also report a `Breakout`.
//! - **Bounce** — the last closed bar's range reached the line and it closed on the same side as
//!   the nearest earlier close that had a side.

use moon_core::config::auto_figures::{AutoEvent, BrokenPolicy, EventMarks};
use moon_core::market::candles::ChartCandle;

use super::{Found, Side, bar_ok, pct_frac, side_of};

/// The events one line shows, as a bit set — no allocation per line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct EventSet(u8);

impl EventSet {
    /// Every event in its display order, the order [`EventSet::iter`] yields them in.
    const ALL: [AutoEvent; 5] = [
        AutoEvent::Approach,
        AutoEvent::Squeeze,
        AutoEvent::Breakout,
        AutoEvent::FalseBreak,
        AutoEvent::Bounce,
    ];

    fn bit(event: AutoEvent) -> u8 {
        match event {
            AutoEvent::Approach => 1,
            AutoEvent::Squeeze => 1 << 1,
            AutoEvent::Breakout => 1 << 2,
            AutoEvent::FalseBreak => 1 << 3,
            AutoEvent::Bounce => 1 << 4,
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

/// The events of every line, one [`EventSet`] per line in input order, written into `out`.
///
/// Args:
///     lines: The kept figures.
///     bars: Closed bars of the row's timeframe, ascending; only the last few are read.
///     price: The current price, for `Approach`.
///     at_ms: The moment the current price belongs to, for projecting sloped lines.
///     marks: Which events the row shows; an unmarked event is never set.
///     policy: The row's broken policy, for the false-break window.
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
    for line in lines {
        let mut set = EventSet::default();
        let shape = &line.shape;
        if let Some(at) = shape.price_at(at_ms) {
            if price.is_finite() && price > 0.0 && super::within(price, at, approach_frac) {
                set.insert(AutoEvent::Approach);
            }
        }
        if matches!(shape, super::FoundShape::Squeeze { pressed: true, .. }) {
            set.insert(AutoEvent::Squeeze);
        }
        bar_events(bars, |t| shape.price_at(t), window, &mut set);
        let mut shown = EventSet::default();
        for e in set.iter().filter(|e| marks.marks(*e)) {
            shown.insert(e);
        }
        out.push(shown);
    }
}

/// `Breakout`, `FalseBreak` and `Bounce` from the last closed bars, into `set`.
fn bar_events(
    bars: &[ChartCandle],
    line_at: impl Fn(f64) -> Option<f64>,
    window: usize,
    set: &mut EventSet,
) {
    // The last valid bars, newest first, each with its side (`None` on the line or where the line
    // has no price). Bounded: nothing past `window + 2` bars can change any of the three answers.
    let mut recent = bars
        .iter()
        .rev()
        .filter(|b| bar_ok(b))
        .take(window.saturating_add(2).max(2))
        .map(|b| {
            let line = line_at(b.t_open_ms);
            let side = line.and_then(|l| side_of(f64::from(b.close), l));
            (b, line, side)
        });
    let Some((last, last_line, Some(last_side))) = recent.next() else {
        return;
    };
    // One pass over the earlier bars, newest first. `prev_side` is the nearest earlier close that
    // had a side. The run of closes NOT on `last_side` before the last bar must, for a false
    // break, hold a close beyond, start within the window, and be preceded by a close on
    // `last_side` — the side the line held before the break.
    let mut prev_side: Option<Side> = None;
    let mut run_start: Option<usize> = None;
    let mut origin_seen = false;
    for (k, (_, _, side)) in recent.enumerate() {
        let Some(side) = side else {
            continue;
        };
        prev_side.get_or_insert(side);
        if side == last_side {
            origin_seen = true;
            break;
        }
        run_start = Some(k + 1);
    }
    let false_break = origin_seen && run_start.is_some_and(|k| k <= window);
    if false_break {
        set.insert(AutoEvent::FalseBreak);
    } else if prev_side == Some(last_side.opposite()) {
        set.insert(AutoEvent::Breakout);
    }

    if prev_side == Some(last_side) {
        if let Some(line) = last_line {
            if f64::from(last.low) <= line && line <= f64::from(last.high) {
                set.insert(AutoEvent::Bounce);
            }
        }
    }
}

#[cfg(test)]
mod tests;
