//! The strategy window's "Stops" section: `StopLoss`, `StopLossDelay`, `UseStopLoss`,
//! `FastStopLoss`, `StopLossEMA` and `StopLossModifier`.
//!
//! **StopLoss** — `StopLoss` per cent from the buy (negative: a loss), adjusted by
//! `StopLossModifier` ([`stop_pct`]), armed `StopLossDelay` seconds after the buy; `UseStopLoss`
//! off zeroes it (`params::exit_params`). With `FastStopLoss` the first print through it is a
//! market exit at the print's own price. Without it — the core's default — the core watches the
//! REST ticker's BID (a short's ASK), a long's averaged over `StopLossEMA` of the ticker's
//! arrivals, and the walk reads a proxy of that: the last print on that side of the book (a
//! taker sell prints at the BID), sampled every [`TICKER_PERIOD_MS`], averaged the same way from
//! before the fill on; at `StopLossEMA` 0 the core's price series fires it too
//! ([`SERIES_TICK_MS`]). The exit is at the sample, at the proxy's price. Where the core's panic
//! sell then fills is the book's business — see [`crate::db::tuner::ticks::verify`] for how the
//! fact is judged.
//!
//! The trailing stop (`UseTrailing`) is [`trailing`]; the stop ladder (`UseSecondStop`,
//! `UseStopLoss3`) is not modelled ([`super::UnmodelledRule`]).

pub mod trailing;

use self::trailing::Trailing;
use super::delta_mods::modifier_sum;
use super::{ExitParams, Side};
use crate::db::tuner::ticks::{Deal, Exit, ExitKind, Fill, reaches};
use crate::feed::types::Tick;

/// How often the core's REST ticker brings the BID (a short's ASK) the non-fast stop watches,
/// and so how often the walk samples its proxy. The core developer (2026-09-23): the stop reads
/// the ticker, not the book and not the trades; the ticker arrives every ~2–2.3 s (Gate's spot
/// half a second slower, about once a second around each hour's turn); the stop is checked
/// every ~0.1 s but the price only moves with the ticker, so it fires on the first arrival
/// that puts it past the level. The middle of that range: over the 192 live book-watching
/// stops of 2026-09-23, 2 000–2 300 ms move the stops judged on time by ±5, the noise of a
/// phase no record keeps.
pub const TICKER_PERIOD_MS: i64 = 2_150;

/// The core's price-series tick: one point per 250 ms, of the prints the tick brought the one
/// closest to the previous point (`docs-internal/STRATEGY_FORMULAS/deltas.md` §7). A stop at
/// `StopLossEMA` 0 fires on that series as well as on the ticker's price (the core developer,
/// 2026-09-23) — a lone print in its tick is a point, a spike among prints near the last
/// point is not.
pub const SERIES_TICK_MS: i64 = 250;

/// The stop distance of a trade, per cent: `StopLoss` adjusted by `StopLossModifier · Σ`.
///
/// Normally that deepens the stop (a positive coefficient over a positive delta sum), but
/// neither sign is guaranteed: live strategies carry `StopLossModifier` down to −0.3, and a
/// delta sum can be negative, so the adjustment can also pull the stop TOWARD the entry.
///
/// An adjustment big enough to pull it THROUGH the entry answers `0.0` — no stop on this trade
/// — rather than a level. Clamping it to a hair's breadth from the entry instead would fire on
/// the first print that moves, which is not a stop but a coin flip dressed as one; and placing
/// it beyond the entry would fire on the first print, full stop. What the core does with an
/// adjustment that large is unknown: none of the 1 735 replayed trades reaches this branch, so
/// the model declines to invent an answer. A configured stop on the profit side (`StopLoss` positive —
/// live data has it) is a different thing and is left exactly as configured.
///
/// Args:
///     params: The sell parameters.
///     deal: The trade, for its deltas.
///     at_ms: When the sell was placed — the fill; see [`modifier_sum`].
pub fn stop_pct(params: &ExitParams, deal: &Deal, at_ms: i64) -> f64 {
    if params.stop_loss_pct == 0.0 || params.stop_loss_modifier == 0.0 {
        return params.stop_loss_pct;
    }
    let adjusted =
        params.stop_loss_pct - modifier_sum(params, deal, at_ms) * params.stop_loss_modifier;
    // Same side as configured, or nothing at all.
    if adjusted == 0.0 || adjusted.is_sign_negative() != params.stop_loss_pct.is_sign_negative() {
        return 0.0;
    }
    adjusted
}

/// The stop's price: `pct` per cent ([`stop_pct`]) off the buy — `buy·(1 + pct/100)` for a long,
/// `buy/(1 + pct/100)` for a short, not the long's product mirrored. The core prints the level
/// into its reason (`StopLoss fixed: X`): over the report (2026-09-24) the division lands on it
/// for 11 099 short stops against 25 for the mirror, the product for 20 930 long ones against 12
/// — the adjusted distance of `StopLossModifier` included. At `−2.5 %` the two short readings
/// part by 0.06 % of the price.
///
/// A loss of 100 % or more leaves no price to stop at: 0 for a long and `f64::INFINITY` for a
/// short, levels no print reaches.
///
/// Args:
///     buy: The buy the stop counts from.
///     pct: The stop distance, negative on the losing side.
///     long: The trade's side.
pub fn stop_level(buy: f64, pct: f64, long: bool) -> f64 {
    let keep = 1.0 + pct / 100.0;
    match (long, keep <= 0.0) {
        (true, true) => 0.0,
        (true, false) => buy * keep,
        (false, true) => f64::INFINITY,
        (false, false) => buy / keep,
    }
}

/// The weight of a new ticker price in the average the non-fast stop watches, when the core
/// keeps one: `avg = (avg·(N − 1) + bid) / N`, a weight of `1/N`, for a LONG at `StopLossEMA`
/// 3, 5 or 10 only. Any other value — 7 included — and every short watch the bare price (the
/// core developer, 2026-09-23). The FAQ's "average over the last 3, 5, 10 ticks" read as an
/// EMA of `2/(N + 1)` forgot the price before a dump twice as fast, and fired early.
fn stop_average_weight(params: &ExitParams, long: bool) -> Option<f64> {
    let n = params.stop_loss_ema;
    let whole = (n - n.round()).abs() < 1e-9;
    (long && whole && matches!(n.round() as i64, 3 | 5 | 10)).then(|| 1.0 / n)
}

/// The book-watching stop's state: a ticker-price proxy — the last print on the stop's side of
/// the book — sampled every [`TICKER_PERIOD_MS`] and, for a long at `StopLossEMA` 3, 5 or 10,
/// averaged ([`stop_average_weight`]); at `StopLossEMA` 0 the core's price series as well.
///
/// The core keeps the average for every market from its start, so at the fill it is warm:
/// after a dump it still remembers the prices above, and fires later than a fresh one. The
/// walk feeds it the prints before the fill for that — they move the average and can never
/// fire it.
struct BookStop {
    long: bool,
    level: f64,
    /// The end of `StopLossDelay`: a sample before it is averaged but cannot fire.
    armed_at: i64,
    /// The fill: a sample or a series point up to it only warms the state.
    fill_ms: i64,
    /// The average's weight, `None` for the bare price.
    weight: Option<f64>,
    proxy: Option<f64>,
    avg: Option<f64>,
    next_sample: i64,
    /// The ticker's period (`ModelSettings::ticker_period_ms`), at least a millisecond.
    period_ms: i64,
    /// Up to when the fact proves this stop did not fire (`record::StopAnchor`): a sample by
    /// then is averaged but cannot fire. `i64::MIN` when the walk is not the trade's own stop.
    quiet_until: i64,
    /// The price series a stop at `StopLossEMA` 0 also fires on.
    series: Option<SeriesPoint>,
}

/// The core's price series as the stop reads it (see [`SERIES_TICK_MS`]).
struct SeriesPoint {
    /// The series' last point.
    point: Option<f64>,
    /// Of the prints the open tick brought, the one closest to `point`.
    candidate: Option<f64>,
    /// When the open tick closes; every pending print is before it.
    tick_end: i64,
    /// The tick's length (`ModelSettings::series_tick_ms`), at least a millisecond.
    tick_ms: i64,
}

impl SeriesPoint {
    fn new(first_ms: i64, tick_ms: i64) -> Self {
        Self {
            point: None,
            candidate: None,
            tick_end: next_series_tick(first_ms, tick_ms),
            tick_ms,
        }
    }

    /// Read a print into the open tick.
    fn see(&mut self, price: f64) {
        self.candidate = match (self.point, self.candidate) {
            (Some(point), Some(held)) if (held - point).abs() <= (price - point).abs() => {
                Some(held)
            }
            // Before the first point any print will do; the latest stands.
            _ => Some(price),
        };
    }
}

/// The end of the series tick a print at `t_ms` falls in: the ticks run on the clock's
/// `tick_ms` boundaries ([`SERIES_TICK_MS`] by default), and a print ON a boundary opens the next
/// tick.
fn next_series_tick(t_ms: i64, tick_ms: i64) -> i64 {
    let tick_ms = tick_ms.max(1);
    (t_ms.div_euclid(tick_ms) + 1) * tick_ms
}

impl BookStop {
    /// Args:
    ///     long: The trade's side.
    ///     level: The stop level.
    ///     armed_at: The end of `StopLossDelay`.
    ///     fill_ms: The fill.
    ///     first_ms: The first print the walk will feed, before the fill when the tape reaches
    ///         back: the ticker's clock is run back to it so the average is warm at the fill.
    ///     params: The sell parameters, for `StopLossEMA`.
    ///     quiet_until: See the field.
    fn new(
        long: bool,
        level: f64,
        armed_at: i64,
        fill_ms: i64,
        first_ms: i64,
        params: &ExitParams,
        quiet_until: i64,
    ) -> Self {
        // One period past the fill, and back from there in whole periods: the ticker's phase
        // is on no record, so the fill anchors it, however far back the tape reaches.
        let period_ms = params.model.ticker_period_ms.max(1);
        let anchor = fill_ms + period_ms;
        let back = (anchor - first_ms).max(0) / period_ms;
        Self {
            long,
            level,
            armed_at,
            fill_ms,
            weight: stop_average_weight(params, long),
            proxy: None,
            avg: None,
            next_sample: anchor - back * period_ms,
            period_ms,
            quiet_until,
            series: (params.stop_loss_ema.abs() < 1e-9)
                .then(|| SeriesPoint::new(first_ms, params.model.series_tick_ms)),
        }
    }

    fn may_fire(&self, at: i64) -> bool {
        at > self.fill_ms && at >= self.armed_at && at > self.quiet_until
    }

    /// Everything due before the print at `until` — the ticker's arrivals strictly before it,
    /// the series ticks closing at or before it — and the earliest that put the stop past its
    /// level: the stop, at that moment and that price.
    fn before(&mut self, until: i64) -> Option<Exit> {
        let by_ticker = self.samples_before(until);
        let by_series = self.series_before(until);
        match (by_ticker, by_series) {
            (Some(t), Some(s)) => Some(if s.t_ms < t.t_ms { s } else { t }),
            (t, s) => t.or(s),
        }
    }

    /// The ticker's arrivals strictly before `until` — the prints before it are all the proxy
    /// has seen — and the first whose price (averaged, when the core averages) is past the
    /// level.
    fn samples_before(&mut self, until: i64) -> Option<Exit> {
        while self.next_sample < until {
            let at = self.next_sample;
            self.next_sample += self.period_ms;
            let Some(bid) = self.proxy else {
                continue;
            };
            let avg = match (self.weight, self.avg) {
                (Some(w), Some(avg)) => w * bid + (1.0 - w) * avg,
                _ => bid,
            };
            self.avg = Some(avg);
            if self.may_fire(at) && reaches(avg, self.level, self.long) {
                return Some(stop_exit(at, bid));
            }
        }
        None
    }

    /// The series tick the pending prints fall in, when it closes by `until`: its point, and
    /// the stop when that point is strictly past the level. The ticks after it up to `until`
    /// brought no print and add no point.
    fn series_before(&mut self, until: i64) -> Option<Exit> {
        let series = self.series.as_mut()?;
        if series.tick_end > until {
            return None;
        }
        let at = series.tick_end;
        series.tick_end = next_series_tick(until, series.tick_ms);
        let point = series.candidate.take()?;
        series.point = Some(point);
        let past = if self.long {
            point < self.level
        } else {
            point > self.level
        };
        (past && self.may_fire(at)).then(|| stop_exit(at, point))
    }

    /// Read a print: into the series, and into the proxy when it hit the stop's side — a taker
    /// sell prints at the BID, a long's stop side; a short's stop watches the ASK, where a
    /// taker buy prints.
    fn see(&mut self, tick: &Tick) {
        let price = f64::from(tick.price);
        if let Some(series) = self.series.as_mut() {
            series.see(price);
        }
        let stop_side = if self.long {
            crate::feed::types::Side::Sell
        } else {
            crate::feed::types::Side::Buy
        };
        if tick.side == stop_side {
            self.proxy = Some(price);
        }
    }
}

/// What fires the trade's stop.
enum Trigger {
    /// No stop: `StopLoss` 0, `UseStopLoss` off, or an adjustment that pulled it through the
    /// entry ([`stop_pct`]).
    Off,
    /// `FastStopLoss`: the first print through the level, a market order the core fires on the
    /// print.
    Fast {
        long: bool,
        level: f64,
        /// The end of `StopLossDelay`.
        from: i64,
        /// Up to when the fact proves the stop did not fire; `i64::MIN` when it proves nothing.
        quiet_until: i64,
    },
    /// The book-watching stop on its ticker proxy.
    Book(BookStop),
}

/// The stop as the walk runs it: the fast one on the prints, the book-watching one on its
/// ticker proxy, or the fact's own when the walk replays the trade's own stop — and the trailing
/// stop beside it.
pub(super) struct Stops {
    trigger: Trigger,
    /// `UseTrailing`'s line, when the strategy switched it on.
    trailing: Option<Trailing>,
    /// When and at what price the fact's own stop fired, when the walk runs the trade's own.
    fired: Option<(i64, f64)>,
}

impl Stops {
    /// The trade's stop under `params`, the book-watching one warmed on the prints before the
    /// fill.
    pub(super) fn new(
        deal: &Deal,
        ticks: &[Tick],
        fill: Fill,
        params: &ExitParams,
        side: Side,
    ) -> Self {
        // The ADJUSTED distance decides both whether there is a stop and where it stands —
        // reading the raw `stop_loss_pct` for the first and the adjusted one for the second would
        // arm a stop the adjustment had cancelled, at the fill price itself, where the next print
        // fires it. [`stop_level`] puts the distance on the trade's side, so only the distance is
        // adjusted here; the level is NOT snapped to the price grid, unlike every level that
        // reaches the exchange —
        // a stop is the core's own trigger for a market sell, and nothing about it is ever placed.
        let stop = stop_pct(params, deal, fill.t_ms);
        let stop_on = stop != 0.0;
        let level = stop_level(fill.price, stop, side.long);
        let stop_from = fill.t_ms + (params.stop_loss_delay_s.max(0.0) * 1000.0) as i64;
        // What the fact proves about the stop when this walk runs the trade's own
        // (`record::StopAnchor`): it fired when the core's did, at the price the core sold at, and
        // not a moment before — nor before the close, on a trade it never stopped. The book the
        // stop watches is not on the tape; the fact is the book's own answer.
        let anchor = deal.stop_anchor.filter(|a| a.holds(deal, fill, params));
        let quiet_until = anchor.map_or(i64::MIN, |a| a.quiet_until_ms);
        let fired = anchor.and_then(|a| a.fired);
        // The ticker's clock of both proxies is run back to the tape's first print, so the stop's
        // and the trailing's arrivals fall on the same moments.
        let first_ms = ticks
            .first()
            .map_or(fill.t_ms, |t| (t.time_ms as i64).min(fill.t_ms));
        let before_fill = || {
            ticks
                .iter()
                .take_while(|t| (t.time_ms as i64) <= fill.t_ms)
                .filter(|t| t.price.is_finite() && t.price > 0.0)
        };
        let trailing = Trailing::new(
            side.long,
            fill.price,
            stop_from,
            fill.t_ms,
            first_ms,
            params,
            quiet_until,
        )
        .map(|mut trailing| {
            // The spread the prints before the fill leave; they never step the peak.
            for tick in before_fill() {
                let _warm_only = trailing.before(tick.time_ms as i64);
                trailing.see(tick);
            }
            trailing
        });
        let trigger = if !stop_on {
            Trigger::Off
        } else if params.fast_stop_loss {
            Trigger::Fast {
                long: side.long,
                level,
                from: stop_from,
                quiet_until,
            }
        } else {
            // The non-fast stop's ticker proxy: the last print on the stop's side of the book,
            // sampled on the ticker's clock, averaged as the core averages (see `BookStop`) —
            // warmed on the prints before the fill, which can never fire it.
            let mut book = BookStop::new(
                side.long,
                level,
                stop_from,
                fill.t_ms,
                first_ms,
                params,
                quiet_until,
            );
            for tick in before_fill() {
                let _warm_only = book.before(tick.time_ms as i64);
                book.see(tick);
            }
            Trigger::Book(book)
        };
        Self {
            trigger,
            trailing,
            fired,
        }
    }

    /// The fact's own stop, when it fired by the print at `t_ms`.
    pub(super) fn fired_by(&self, t_ms: i64) -> Option<Exit> {
        self.fired
            .filter(|(at, _)| t_ms >= *at)
            .map(|(at, sold)| stop_exit(at, sold))
    }

    /// The stop on the print at `t_ms`, `price`: the book stop's and the trailing's ticker
    /// arrivals due BEFORE it — every sample reads the proxy the earlier prints left, every series
    /// tick closing by it the points they left, and the earliest past its level fires at its own
    /// moment, the stop first on the same moment (the core checks it first) — or the fast stop, a
    /// market order the core fires on the print.
    pub(super) fn on_print(&mut self, tick: &Tick, t_ms: i64, price: f64) -> Option<Exit> {
        let by_book = match &mut self.trigger {
            Trigger::Book(book) => book.before(t_ms),
            Trigger::Off | Trigger::Fast { .. } => None,
        };
        let by_trailing = self
            .trailing
            .as_mut()
            .and_then(|trailing| trailing.before(t_ms));
        if let Some(exit) = earliest(by_book, by_trailing) {
            return Some(exit);
        }
        if let Trigger::Book(book) = &mut self.trigger {
            book.see(tick);
        }
        if let Some(trailing) = self.trailing.as_mut() {
            trailing.see(tick);
        }
        match &self.trigger {
            Trigger::Fast {
                long,
                level,
                from,
                quiet_until,
            } => (t_ms >= *from && t_ms > *quiet_until && reaches(price, *level, *long))
                .then(|| stop_exit(t_ms, price)),
            Trigger::Off | Trigger::Book(_) => None,
        }
    }

    /// The stop past the tape's last print at `tail`: the fact's own — the tape went quiet, the
    /// core did not — or the ticker's arrivals up to the tape's end, the one AT the last print
    /// included, reading the proxies the last prints left; the walk only ever reaches the samples
    /// before a print.
    pub(super) fn after_tape(&mut self, tail: i64) -> Option<Exit> {
        if let Some((at, sold)) = self.fired {
            return Some(stop_exit(at, sold));
        }
        let by_book = match &mut self.trigger {
            Trigger::Book(book) => book.before(tail + 1),
            Trigger::Off | Trigger::Fast { .. } => None,
        };
        let by_trailing = self
            .trailing
            .as_mut()
            .and_then(|trailing| trailing.before(tail + 1));
        earliest(by_book, by_trailing)
    }
}

/// The earlier of the stop's and the trailing's exits; the stop on the same moment.
fn earliest(stop: Option<Exit>, trailing: Option<Exit>) -> Option<Exit> {
    match (stop, trailing) {
        (Some(stop), Some(trailing)) => Some(if trailing.t_ms < stop.t_ms {
            trailing
        } else {
            stop
        }),
        (stop, trailing) => stop.or(trailing),
    }
}

fn stop_exit(t_ms: i64, price: f64) -> Exit {
    Exit {
        t_ms,
        price,
        kind: ExitKind::Stop,
    }
}

#[cfg(test)]
mod tests;
