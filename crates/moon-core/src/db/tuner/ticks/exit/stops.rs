//! The strategy window's "Stops" section: `StopLoss`, `StopLossDelay`, `UseStopLoss`,
//! `FastStopLoss`, `StopLossEMA` and `StopLossModifier`.
//!
//! **StopLoss** — `StopLoss` per cent from the buy (negative: a loss), adjusted by
//! `StopLossModifier` ([`stop_pct`]), armed `StopLossDelay` seconds after the buy; `UseStopLoss`
//! off zeroes it (`params::exit_params`). With `FastStopLoss`, `ModelSettings::fast_price`
//! (default: the last price) is checked as each [`SERIES_TICK_MS`] tick closes, a market exit
//! at the tick's last print
//! ([`FastStop`]). Without it — the core's default — the core watches the
//! REST ticker's BID (a short's ASK), a long's averaged over `StopLossEMA` of the ticker's
//! arrivals, and the walk reads a proxy of that: the last print on that side of the book (a
//! taker sell prints at the BID), sampled every [`TICKER_PERIOD_MS`], averaged the same way from
//! before the fill on; at `StopLossEMA` 0 the core's price series fires it too
//! ([`SERIES_TICK_MS`]). The exit is at the sample, at the proxy's price. Where the core's panic
//! sell then fills is the book's business — see [`crate::db::tuner::ticks::verify`] for how the
//! fact is judged.
//!
//! The trailing stop (`UseTrailing`) is [`trailing`]; the stop ladder (`UseSecondStop`,
//! `UseStopLoss3`) is [`ladder`], which moves the level this stop fires at.

pub mod ladder;
pub mod trailing;

use self::ladder::Ladder;
use self::trailing::Trailing;
use super::delta_mods::modifier_sum;
use super::{ExitParams, Side};
use crate::db::tuner::ticks::settings::FastPrice;
use crate::db::tuner::ticks::verify::{StopGroup, stop_group};
use crate::db::tuner::ticks::{Deal, Exit, ExitKind, Fill, reaches};
use crate::feed::types::Tick;

/// The estimator's bounded trade history, shared with settings sanitization.
pub(crate) const FAST_TRADES_MAX: usize = 16;

/// A tentative trade combines consecutive prints at one time and side.
#[derive(Clone, Copy)]
struct OpenTrade {
    t_ms: f64,
    side: crate::feed::types::Side,
    pq: f64,
    q: f64,
}

/// A bounded estimator over consecutive same-time, same-side groups of prints.
/// The open group is tentative so splitting a trade never adds EMA steps.
pub(crate) struct FastPriceState {
    rule: FastPrice,
    open: Option<OpenTrade>,
    closed: [(f64, f64); FAST_TRADES_MAX],
    len: usize,
    head: usize,
    ema: Option<f64>,
    /// Closed-trade mean quantity; `step` tentatively includes the open group.
    qbar: f64,
    count: u64,
}

impl FastPriceState {
    /// Start empty; callers sanitize the code-level rule with their model settings.
    pub(crate) fn new(rule: FastPrice) -> Self {
        Self {
            rule,
            open: None,
            closed: [(0.0, 0.0); FAST_TRADES_MAX],
            len: 0,
            head: 0,
            ema: None,
            qbar: 0.0,
            count: 0,
        }
    }

    /// Tentatively step the open trade using the cumulative mean including that trade.
    fn step(&self, price: f64, qty: f64) -> f64 {
        let FastPrice::TradeEma {
            alpha,
            qty_weighted,
        } = self.rule
        else {
            return price;
        };
        let mean = self.qbar + (qty - self.qbar) / (self.count + 1) as f64;
        let weight = if qty_weighted {
            (alpha * qty / mean).min(1.0)
        } else {
            alpha
        };
        self.ema.map_or(price, |ema| ema + weight * (price - ema))
    }

    /// Feed a valid print, committing the previous trade only when its group ends.
    pub(crate) fn see(&mut self, tick: &Tick) {
        let price = f64::from(tick.price);
        let qty = f64::from(tick.qty);
        if !price.is_finite() || price <= 0.0 || !qty.is_finite() || qty <= 0.0 {
            return;
        }
        if let Some(OpenTrade { t_ms, side, pq, q }) = self.open {
            if t_ms == tick.time_ms && side == tick.side {
                self.open = Some(OpenTrade {
                    t_ms,
                    side,
                    pq: pq + price * qty,
                    q: q + qty,
                });
                return;
            }
            self.ema = Some(self.step(pq / q, q));
            self.count += 1;
            self.qbar += (q - self.qbar) / self.count as f64;
            self.closed[self.head] = (pq / q, q);
            self.head = (self.head + 1) % FAST_TRADES_MAX;
            self.len = (self.len + 1).min(FAST_TRADES_MAX);
        }
        self.open = Some(OpenTrade {
            t_ms: tick.time_ms,
            side: tick.side,
            pq: price * qty,
            q: qty,
        });
    }

    /// Evaluate closed trades plus the current open trade, without committing it.
    pub(crate) fn value(&self) -> Option<f64> {
        let OpenTrade { pq, q, .. } = self.open?;
        match self.rule {
            FastPrice::LastOfTick => None,
            FastPrice::TradeEma { .. } => Some(self.step(pq / q, q)),
            FastPrice::VwapLastTrades(n) => {
                let (mut sum_pq, mut sum_q) = (pq, q);
                for back in 0..self.len.min(usize::from(n.saturating_sub(1))) {
                    let (p, q) =
                        self.closed[(self.head + FAST_TRADES_MAX - 1 - back) % FAST_TRADES_MAX];
                    sum_pq += p * q;
                    sum_q += q;
                }
                Some(sum_pq / sum_q)
            }
        }
    }
}

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
/// point is not. The fast stop compares `ModelSettings::fast_price` at the same tick end,
/// defaulting to its last price ([`FastStop`]).
pub const SERIES_TICK_MS: i64 = 250;

/// Which rule of the walk fired its stop — what the diagnostic report counts per segment, so a
/// moment miss reads as the ticker proxy's or the series' without the trade in hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StopTrigger {
    /// The book-watching stop's ticker proxy ([`TICKER_PERIOD_MS`]).
    Ticker,
    /// The core's 250 ms price series, at `StopLossEMA` 0 ([`SERIES_TICK_MS`]).
    Series,
    /// `FastStopLoss`: `ModelSettings::fast_price`, defaulting to the tick's last price.
    FastTick,
    /// `UseTrailing`'s line.
    Trailing,
    /// The fact's own stop (`record::StopAnchor`) — never in a verdict, which runs unanchored.
    Fact,
}

/// The stop distance of a trade, per cent: `StopLoss` adjusted by `StopLossModifier · Σ`.
///
/// The sum is never negative (the core takes its magnitude, [`modifier_sum`]), so a positive
/// coefficient deepens the stop; but live strategies carry `StopLossModifier` down to −0.3, and
/// that pulls the stop TOWARD the entry.
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

/// Which book event is due; their print-boundary rules differ even at the same timestamp.
enum BookEvent {
    Ticker,
    Series,
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

/// The fast stop compares `ModelSettings::fast_price` at each [`SERIES_TICK_MS`] tick end,
/// defaulting to the last price. Its market exit price remains the tick's last print.
///
/// Core logs print `WeightedPrice`/`WeightedPricePrev` only on fast-stop fires; there the
/// weighted reading, rather than BID/LastPrice, crosses the logged level. A VWAP of the last
/// few trades approximates that reading without reproducing it exactly.
///
/// Checking every print can fire before the core; the tick's last price better approximates
/// its timing. The delta series' closest-to-previous-point selection can instead fire late.
struct FastStop {
    price_rule: FastPrice,
    state: FastPriceState,
    long: bool,
    level: f64,
    /// The end of `StopLossDelay`.
    from: i64,
    /// Up to when the fact proves the stop did not fire; `i64::MIN` when it proves nothing.
    quiet_until: i64,
    /// The last price the open tick brought.
    last: Option<f64>,
    /// When the open tick closes; every pending print is before it.
    tick_end: i64,
    /// The tick's length (`ModelSettings::series_tick_ms`), at least a millisecond.
    tick_ms: i64,
}

impl FastStop {
    /// When the open tick closes, if it holds a print and closes by `until`.
    fn due(&self, until: i64) -> Option<i64> {
        (self.last.is_some() && self.tick_end <= until).then_some(self.tick_end)
    }

    /// Compare the selected estimate at tick end, exiting at the tick's last print.
    /// The fact's quiet span is read against that same tick end.
    fn before(&mut self, until: i64) -> Option<Exit> {
        if self.tick_end > until {
            return None;
        }
        let at = self.tick_end;
        let price = self.last.take()?;
        let compared = match self.price_rule {
            FastPrice::LastOfTick => price,
            // No trade seen yet: use the tick's last price.
            _ => self.state.value().unwrap_or(price),
        };
        (at >= self.from && at > self.quiet_until && reaches(compared, self.level, self.long))
            .then(|| stop_exit(at, price))
    }

    /// Read a print into the open tick, opening the one it falls in.
    fn see(&mut self, tick: &Tick, t_ms: i64, price: f64) {
        self.tick_end = next_series_tick(t_ms, self.tick_ms);
        self.last = Some(price);
        if self.price_rule != FastPrice::LastOfTick {
            self.state.see(tick);
        }
    }
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
    /// level: the stop, at that moment and that price, and which of the two fired it.
    fn before(&mut self, until: i64) -> Option<(Exit, StopTrigger)> {
        self.before_with(until, |_| None)
    }

    /// Evaluate events in order, optionally updating the level immediately before each one.
    fn before_with(
        &mut self,
        until: i64,
        mut level_before: impl FnMut(i64) -> Option<f64>,
    ) -> Option<(Exit, StopTrigger)> {
        while let Some((at, event)) = self.next_event(until) {
            if let Some(level) = level_before(at) {
                self.level = level;
            }
            if let Some(exit) = self.evaluate_event(event) {
                return Some(exit);
            }
        }
        None
    }

    /// The earliest pending ticker arrival strictly before `until` or populated series tick
    /// closing by it. Empty series ticks carry no price and need no evaluation.
    fn next_event(&self, until: i64) -> Option<(i64, BookEvent)> {
        let ticker = (self.next_sample < until).then_some((self.next_sample, BookEvent::Ticker));
        let series = self.series.as_ref().and_then(|series| {
            (series.candidate.is_some() && series.tick_end <= until)
                .then_some((series.tick_end, BookEvent::Series))
        });
        match (ticker, series) {
            (Some(t), Some(s)) => Some(if s.0 < t.0 { s } else { t }),
            (t, s) => t.or(s),
        }
    }

    /// Evaluate one pending event after the ladder has reached its moment; ticker wins a tie.
    fn evaluate_event(&mut self, event: BookEvent) -> Option<(Exit, StopTrigger)> {
        match event {
            BookEvent::Ticker => self.sample().map(|exit| (exit, StopTrigger::Ticker)),
            BookEvent::Series => self.series_sample().map(|exit| (exit, StopTrigger::Series)),
        }
    }

    /// Consume one ticker arrival, updating its average even when the stop cannot fire.
    fn sample(&mut self) -> Option<Exit> {
        let at = self.next_sample;
        self.next_sample += self.period_ms;
        let bid = self.proxy?;
        let avg = match (self.weight, self.avg) {
            (Some(w), Some(avg)) => w * bid + (1.0 - w) * avg,
            _ => bid,
        };
        self.avg = Some(avg);
        (self.may_fire(at) && reaches(avg, self.level, self.long)).then(|| stop_exit(at, bid))
    }

    /// Consume the populated series tick and fire only when its point is strictly past the level.
    fn series_sample(&mut self) -> Option<Exit> {
        let series = self.series.as_mut()?;
        let at = series.tick_end;
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
            series.tick_end = next_series_tick(tick.time_ms as i64, series.tick_ms);
            series.see(price);
        }
        if tick.side == stop_side(self.long) {
            self.proxy = Some(price);
        }
    }
}

/// The aggressor side observing a long's bid or a short's ask.
pub(crate) fn stop_side(long: bool) -> crate::feed::types::Side {
    if long {
        crate::feed::types::Side::Sell
    } else {
        crate::feed::types::Side::Buy
    }
}

/// What fires the trade's stop.
enum Trigger {
    /// No stop: `StopLoss` 0, `UseStopLoss` off, or an adjustment that pulled it through the
    /// entry ([`stop_pct`]).
    Off,
    /// Compare `ModelSettings::fast_price` (default: last of tick) at tick end.
    /// Boxing once per walk keeps the inline trade ring from expanding every trigger variant.
    Fast(Box<FastStop>),
    /// The book-watching stop on its ticker proxy.
    Book(BookStop),
}

/// The stop as the walk runs it: the fast one on `ModelSettings::fast_price` at each tick end,
/// defaulting to the last print; the
/// book-watching one on its ticker proxy, or the fact's own when the walk replays the trade's own stop — and the trailing
/// stop beside it.
pub(super) struct Stops {
    trigger: Trigger,
    /// The second and third stops, when the strategy switched one on and there is a stop to move.
    ladder: Option<Ladder>,
    /// `UseTrailing`'s line, when the strategy switched it on.
    trailing: Option<Trailing>,
    /// When and at what price the fact's own stop fired, when the walk runs the trade's own.
    fired: Option<(i64, f64)>,
    /// Which rule fired the stop this walk closed on; `None` until one did.
    fired_rule: Option<StopTrigger>,
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
        // fires it. `level_off_buy` puts the distance on the trade's side, so only the distance is
        // adjusted here; the level is NOT snapped to the price grid, unlike every level that
        // reaches the exchange —
        // a stop is the core's own trigger for a market sell, and nothing about it is ever placed.
        let stop = stop_pct(params, deal, fill.t_ms);
        let stop_on = stop != 0.0;
        let level = side.off_buy(fill.price, stop);
        let stop_from = fill.t_ms + (params.stop_loss_delay_s.max(0.0) * 1000.0) as i64;
        // What the fact proves about the stop when this walk runs the trade's own
        // (`record::StopAnchor`): it fired when the core's did, at the price the core sold at, and
        // not a moment before — nor before the close, on a trade it never stopped. The book the
        // stop watches is not on the tape; the fact is the book's own answer. The stop and the
        // trailing each lean on the fact only under their own settings, and the fact's firing
        // goes to the walk only when the group its reason names holds.
        let stop_holds = deal
            .stop_anchor
            .is_some_and(|a| a.holds(deal, fill, params));
        let trailing_holds = deal
            .stop_anchor
            .is_some_and(|a| a.holds_trailing(fill, params));
        let quiet_of = |holds: bool| {
            deal.stop_anchor
                .filter(|_| holds)
                .map_or(i64::MIN, |a| a.quiet_until_ms)
        };
        let quiet_until = quiet_of(stop_holds);
        let trailing_quiet_until = quiet_of(trailing_holds);
        let group_holds = match stop_group(&deal.sell_reason) {
            Some(StopGroup::Stop) => stop_holds,
            Some(StopGroup::Trailing) => trailing_holds,
            Some(StopGroup::Either) => stop_holds && trailing_holds,
            None => false,
        };
        let fired = deal
            .stop_anchor
            .filter(|_| group_holds)
            .and_then(|a| a.fired);
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
            trailing_quiet_until,
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
            let price_rule = params.model.fast_price;
            let mut state = FastPriceState::new(price_rule);
            if price_rule != FastPrice::LastOfTick {
                for tick in before_fill() {
                    state.see(tick);
                }
            }
            Trigger::Fast(Box::new(FastStop {
                price_rule,
                state,
                long: side.long,
                level,
                from: stop_from,
                quiet_until,
                last: None,
                tick_end: i64::MIN,
                tick_ms: params.model.series_tick_ms.max(1),
            }))
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
        // The ladder moves a stop there is: none without one (`UseStopLoss` off, the fields hang
        // on it — param_deps.toml — or an adjustment that cancelled it).
        let ladder = stop_on
            .then(|| {
                Ladder::new(
                    side.long,
                    fill.price,
                    fill.t_ms,
                    super::sell_order::armed_at(fill, params),
                    stop_from,
                    first_ms,
                    params.model.ticker_period_ms,
                    &[params.second_stop, params.third_stop],
                )
            })
            .flatten()
            .map(|mut ladder| {
                for tick in before_fill() {
                    ladder.see(tick);
                }
                ladder
            });
        Self {
            trigger,
            ladder,
            trailing,
            fired,
            fired_rule: None,
        }
    }

    /// Where the stop stands now — the ladder's last step, else the first stop's level; `None`
    /// without a stop.
    pub(super) fn level(&self) -> Option<f64> {
        match &self.trigger {
            Trigger::Off => None,
            Trigger::Fast(fast) => Some(fast.level),
            Trigger::Book(book) => Some(book.level),
        }
    }

    /// Which rule fired the stop the walk closed on; `None` when no stop fired.
    pub(super) fn fired_trigger(&self) -> Option<StopTrigger> {
        self.fired_rule
    }

    /// Up to when the fact proves this stop quiet (`record::StopAnchor`): `i64::MIN` when the walk
    /// is not the trade's own stop, or there is none.
    pub(super) fn quiet_until(&self) -> i64 {
        match &self.trigger {
            Trigger::Off => i64::MIN,
            Trigger::Fast(fast) => fast.quiet_until,
            Trigger::Book(book) => book.quiet_until,
        }
    }

    /// Whether trailing, a pending rung, or a weighted fast estimate needs missing prints.
    pub(super) fn follows_price(&self) -> bool {
        self.trailing.is_some()
            || self.ladder.as_ref().is_some_and(Ladder::pending)
            || (self.fired.is_none()
                && matches!(&self.trigger, Trigger::Fast(fast) if fast.price_rule != FastPrice::LastOfTick))
    }

    /// The ladder's steps strictly before `until`, moving the stop's level when one is taken.
    /// Book events advance this clock through their own moment before checking the price, so a
    /// rung applies from its qualifying cycle without changing earlier decisions.
    fn climb(&mut self, until: i64) {
        let Some(level) = self.ladder.as_mut().and_then(|ladder| ladder.before(until)) else {
            return;
        };
        match &mut self.trigger {
            Trigger::Off => {}
            Trigger::Fast(fast) => fast.level = level,
            Trigger::Book(book) => book.level = level,
        }
    }

    /// Merge ticker and series events with ladder cycles, stopping at the first firing event.
    /// The ladder includes a cycle at the event's own time, then reaches `until` only if quiet.
    fn book_before(&mut self, until: i64) -> Option<(Exit, StopTrigger)> {
        let ladder = &mut self.ladder;
        let exit = match &mut self.trigger {
            Trigger::Book(book) => book.before_with(until, |at| {
                ladder.as_mut().and_then(|ladder| ladder.before(at + 1))
            }),
            Trigger::Off | Trigger::Fast(_) => None,
        };
        if exit.is_none() {
            self.climb(until);
        }
        exit
    }

    /// The fact's own stop, when it fired by the print at `t_ms`.
    /// The ladder is climbed up to that moment — not past it — so the level the walk then
    /// reports is the one the fact's stop fired at.
    pub(super) fn fired_by(&mut self, t_ms: i64) -> Option<Exit> {
        let (at, sold) = self.fired.filter(|(at, _)| t_ms >= *at)?;
        self.climb(at + 1);
        self.fired_rule = Some(StopTrigger::Fact);
        Some(stop_exit(at, sold))
    }

    /// The stop on the print at `t_ms`, `price`: the book stop's and the trailing's ticker
    /// arrivals due BEFORE it — every sample reads the proxy the earlier prints left, every series
    /// tick closing by it the points they left, and the earliest past its level fires at its own
    /// moment, the stop first on the same moment (the core checks it first) — or the fast stop's
    /// tick closing by it, comparing the selected fast-price estimate.
    pub(super) fn on_print(&mut self, tick: &Tick, t_ms: i64, price: f64) -> Option<Exit> {
        // The fast stop's tick is judged against the level as it stood when the tick closed,
        // before the ladder's steps up to this print move it — and a tick that fires leaves the
        // ladder where it stood then.
        let fast_due = match &self.trigger {
            Trigger::Fast(fast) => fast.due(t_ms),
            Trigger::Off | Trigger::Book(_) => None,
        };
        let by_fast = fast_due.and_then(|closes_at| {
            self.climb(closes_at);
            match &mut self.trigger {
                Trigger::Fast(fast) => fast.before(t_ms),
                Trigger::Off | Trigger::Book(_) => None,
            }
        });
        if by_fast.is_none() && !matches!(self.trigger, Trigger::Book(_)) {
            self.climb(t_ms);
        }
        let by_book = match &mut self.trigger {
            Trigger::Book(_) => self.book_before(t_ms),
            Trigger::Fast(_) => by_fast.map(|exit| (exit, StopTrigger::FastTick)),
            Trigger::Off => None,
        };
        let by_trailing = self
            .trailing
            .as_mut()
            .and_then(|trailing| trailing.before(t_ms))
            .map(|exit| (exit, StopTrigger::Trailing));
        if let Some((exit, by)) = earliest(by_book, by_trailing) {
            self.fired_rule = Some(by);
            return Some(exit);
        }
        match &mut self.trigger {
            Trigger::Book(book) => book.see(tick),
            Trigger::Fast(fast) => fast.see(tick, t_ms, price),
            Trigger::Off => {}
        }
        if let Some(trailing) = self.trailing.as_mut() {
            trailing.see(tick);
        }
        if let Some(ladder) = self.ladder.as_mut() {
            ladder.see(tick);
        }
        None
    }

    /// The stop past the tape's last print at `tail`: the fact's own — the tape went quiet, the
    /// core did not — or the ticker's arrivals up to the tape's end, the one AT the last print
    /// included, reading the proxies the last prints left; the walk only ever reaches the samples
    /// before a print.
    pub(super) fn after_tape(&mut self, tail: i64) -> Option<Exit> {
        // The fact's own stop, with the ladder climbed to its moment — often past the tape's end,
        // on the last bid the prints left.
        if let Some((at, sold)) = self.fired {
            self.climb(at + 1);
            self.fired_rule = Some(StopTrigger::Fact);
            return Some(stop_exit(at, sold));
        }
        // The tape's last tick closes on its last print — nothing the tape holds follows it — and
        // is judged against the ladder as it stood at the tick's end.
        let fast_closes_at = match &self.trigger {
            Trigger::Fast(fast) => fast.due(i64::MAX),
            Trigger::Off | Trigger::Book(_) => None,
        };
        if !matches!(self.trigger, Trigger::Book(_)) {
            self.climb(fast_closes_at.map_or(tail, |at| at.max(tail)) + 1);
        }
        let by_book = match &mut self.trigger {
            Trigger::Book(_) => self.book_before(tail + 1),
            Trigger::Fast(fast) => fast
                .before(i64::MAX)
                .map(|exit| (exit, StopTrigger::FastTick)),
            Trigger::Off => None,
        };
        let by_trailing = self
            .trailing
            .as_mut()
            .and_then(|trailing| trailing.before(tail + 1))
            .map(|exit| (exit, StopTrigger::Trailing));
        let (exit, by) = earliest(by_book, by_trailing)?;
        self.fired_rule = Some(by);
        Some(exit)
    }
}

/// The earlier of the stop's and the trailing's exits, each with the rule that fired it; the
/// stop on the same moment.
fn earliest(
    stop: Option<(Exit, StopTrigger)>,
    trailing: Option<(Exit, StopTrigger)>,
) -> Option<(Exit, StopTrigger)> {
    match (stop, trailing) {
        (Some(stop), Some(trailing)) => Some(if trailing.0.t_ms < stop.0.t_ms {
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
