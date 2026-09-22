//! The MoonShot entry model: a limit order at a fixed distance below the price (above, for a
//! short), walking a corridor and filled by the first print that reaches it.
//!
//! Mechanics, from the Moonbot FAQ ("Какие есть специфические параметры у стратегии MoonShot"):
//!
//! - the order stands `MShotPrice` % away from the reference price; the price may approach it
//!   down to `MShotPriceMin` % — closer than that, the order is re-placed at `MShotPrice` again
//!   after `MShotReplaceDelay` seconds; when the price runs away so the order is farther than
//!   `MShotPrice`, it is re-placed after `MShotRaiseWait` seconds;
//! - every `MShotAdd*Delta` adds `k · delta` to BOTH bounds, the delta's sign as the report
//!   carries it: the FAQ's `-10% + (-20 · 0.05) = -11%` is a coin UP 20 % on 3 h putting the
//!   order 1 % deeper (checked on the live tape on 2026-09-20: a long on ROSE, up 7 % / 11 % on
//!   3 h / 24 h, had its real order deeper than the unmodified corridor by exactly `Σ k · δ`);
//!   `MShotAddPriceBug` adds `k · pricebug` — deeper during exchange lag — and
//!   `MShotAddDistance` scales what the FAR bound receives by `1 + distance / 100`;
//! - `MShotMinusSatoshi` keeps the order at least two price steps off the reference;
//! - `MShotUsePrice` picks the reference: the last trade, or the book's ASK / BID;
//! - `FastShotAlgo` with a non-zero `MShotRaiseWait` is the FAQ's "algorithm 2": the reference
//!   is the lowest print of the last 100 ms (highest, for a short), which is what keeps the
//!   order from bouncing back up on a single print. With a zero wait it is "algorithm 1", a
//!   price "over the last few trades" the FAQ does not size; the model reads the last print.
//!
//! Two things the tape cannot give and the model states instead: the book (an ASK / BID
//! reference is approximated by the last buy-side / sell-side print), and the time a
//! replacement takes to reach the exchange (`latency_ms`). The latency is what makes fills
//! possible at all — a corridor that re-places instantly is never reached by a spike — and its
//! first value is a constant; calibrating it from the archive's replacement times against the
//! moments the price left the corridor is phase 3.
//!
//! The core keeps its own idea of where the order is; the exchange learns about a move
//! `latency_ms` later. A print in between fills at the OLD level. That is the one interleaving
//! the model has to get right, and it is why the state carries two levels.
//!
//! What the order archive adds, when it holds the trade's entry line: where the order stood
//! when the tape begins (the last archived level at or before the first print — the tape
//! cannot tell where an order placed minutes earlier was), and the core's first moves inside
//! the model's BLIND WINDOW — the first `MShotRaiseWait` / `MShotReplaceDelay` seconds of the
//! tape, where a wait the core started before the tape began expires at a moment the tape
//! gives no way to compute. A move archived in that window is taken as archived, moment and
//! level both: the level is the core's own reference at work, and on a coarse grid the model's
//! reference off the prints lands a step away often enough to turn the fill into a miss
//! (2026-09-21, 458 deals: 4 entries lost to a modelled level, none gained by it). Seen on
//! ARX the same day: the core re-placed 0.3 s into the tape after a wait of 30 s, and the model
//! waiting its own 30 s put the order one step too deep for the spike. Past the window the
//! model is on its own, and the archive is what it is held against.

use super::verify::archived_replacements;
use super::{Deal, Deltas, Fill, reaches, snap_to_step};
use crate::feed::types::{Side, Tick};

/// Which price the order keeps its distance from (`MShotUsePrice`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UsePrice {
    /// The last trade — the model then matches the core exactly.
    #[default]
    Trade,
    /// Best ask; approximated here by the last BUY-side print (a taker buy prints at the ask).
    Ask,
    /// Best bid; approximated here by the last SELL-side print.
    Bid,
}

impl UsePrice {
    /// Parse the strategy's spelling (`Trade` / `ASK` / `BID`), case-insensitively; anything
    /// else reads as `Trade`, the default of the field.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "ASK" => Self::Ask,
            "BID" => Self::Bid,
            _ => Self::Trade,
        }
    }
}

/// The `MShotAdd*` modifiers — per-cent added to the corridor bounds per one per cent of the
/// matching delta at the buy.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Modifiers {
    pub add_1m: f64,
    pub add_5m: f64,
    pub add_15m: f64,
    pub add_1h: f64,
    pub add_3h: f64,
    pub add_24h: f64,
    pub add_mark: f64,
    pub add_pricebug: f64,
    pub add_btc_1h: f64,
    pub add_btc_5m: f64,
    /// `AddBTC1mDelta` of the Delta Modifiers tab; the `MShotAdd*` family has no such field
    /// and leaves it at zero.
    pub add_btc_1m: f64,
    pub add_market_1h: f64,
    /// `MShotAddDistance` — per cent by which the far bound's addition exceeds the near one's.
    pub distance_pct: f64,
}

impl Modifiers {
    /// The addition to the NEAR bound, in per cent, for these deltas — `Σ k · δ`, every delta
    /// with its own sign, so a coin that went up gets a deeper order (the module doc has the
    /// FAQ example and the live check behind the sign).
    pub fn near_addition(&self, d: &Deltas) -> f64 {
        self.add_1m * d.d1m
            + self.add_5m * d.d5m
            + self.add_15m * d.d15m
            + self.add_1h * d.d1h
            + self.add_3h * d.d3h
            + self.add_24h * d.d24h
            + self.add_mark * d.dmark
            + self.add_btc_1h * d.btc1h
            + self.add_btc_5m * d.btc5m
            + self.add_btc_1m * d.btc1m
            + self.add_market_1h * d.market1h
            + self.add_pricebug * d.pricebug
    }

    /// The addition to the FAR bound: the near one scaled by `1 + distance / 100`.
    pub fn far_addition(&self, d: &Deltas) -> f64 {
        self.near_addition(d) * (1.0 + self.distance_pct / 100.0)
    }
}

/// Smallest distance either bound may end up at after the modifiers, in per cent — the
/// expert-mode floor of `MShotPrice`, so a pumping coin cannot push the order ABOVE the price.
const BOUND_FLOOR_PCT: f64 = 0.02;

/// Default replacement latency, milliseconds — see the module doc.
pub const DEFAULT_LATENCY_MS: f64 = 100.0;

/// The seconds the FAQ's "4-second-old ASK" of `MShotSellAtLastPrice` looks back.
pub const PRE_SPIKE_LOOKBACK_MS: i64 = 4_000;

/// The window of `FastShotAlgo`'s algorithm 2: the reference is the extreme print of the last
/// 100 ms (the FAQ: "the minimum trade over 100 ms").
pub const FAST_ALGO_WINDOW_MS: i64 = 100;

/// MoonShot entry parameters, in the strategy's own units (per cent, seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct MshotParams {
    /// `MShotPrice` — the far bound, per cent below the reference (above, for a short).
    pub price_pct: f64,
    /// `MShotPriceMin` — the near bound.
    pub price_min_pct: f64,
    pub use_price: UsePrice,
    /// `MShotRaiseWait`, seconds — before re-placing after the price RAN AWAY.
    pub raise_wait_s: f64,
    /// `MShotReplaceDelay`, seconds — before re-placing after the price CAME TOO CLOSE.
    pub replace_delay_s: f64,
    /// `MShotMinusSatoshi`.
    pub minus_satoshi: bool,
    /// `FastShotAlgo` — see the module doc; only algorithm 2 (with a non-zero raise wait)
    /// changes the reference.
    pub fast_algo: bool,
    pub modifiers: Modifiers,
    /// Model parameter, not a strategy field: how long a replacement takes to reach the book.
    pub latency_ms: f64,
}

impl Default for MshotParams {
    /// A plain 1 % / 0.5 % corridor with no waits and no modifiers — the shape every real
    /// strategy overrides, kept sane so a missing field never yields a zero-width corridor.
    fn default() -> Self {
        Self {
            price_pct: 1.0,
            price_min_pct: 0.5,
            use_price: UsePrice::Trade,
            raise_wait_s: 0.0,
            replace_delay_s: 0.0,
            minus_satoshi: false,
            fast_algo: false,
            modifiers: Modifiers::default(),
            latency_ms: DEFAULT_LATENCY_MS,
        }
    }
}

impl MshotParams {
    /// The effective `(near, far)` bounds in per cent for a deal's deltas, floored and ordered.
    pub fn bounds_pct(&self, deltas: &Deltas) -> (f64, f64) {
        let near = (self.price_min_pct + self.modifiers.near_addition(deltas)).max(BOUND_FLOOR_PCT);
        let far = (self.price_pct + self.modifiers.far_addition(deltas)).max(near);
        (near, far)
    }
}

/// Which way the price left the corridor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Breach {
    /// The price came closer than the near bound.
    Approach,
    /// The price ran farther than the far bound.
    Retreat,
}

/// The MoonShot entry model over one parameter set.
pub struct MshotEntry<'a> {
    params: &'a MshotParams,
}

impl<'a> MshotEntry<'a> {
    pub fn new(params: &'a MshotParams) -> Self {
        Self { params }
    }

    /// Where the order would stand for a reference price: the far bound away, kept two steps
    /// off the reference when `MShotMinusSatoshi` says so, snapped to the price grid AWAY from
    /// the reference (a limit is placed on the grid, and the core rounds toward safety).
    fn place(&self, reference: f64, far_pct: f64, deal: &Deal) -> f64 {
        let distance = reference * far_pct / 100.0;
        let mut level = if deal.is_long() {
            reference - distance
        } else {
            reference + distance
        };
        if let Some(tick) = deal.tick.filter(|t| *t > 0.0) {
            if self.params.minus_satoshi {
                let keep_off = 2.0 * tick;
                level = if deal.is_long() {
                    level.min(reference - keep_off)
                } else {
                    level.max(reference + keep_off)
                };
            }
            level = snap_to_step(level, tick, deal.is_long());
        }
        level
    }

    /// Distance from the reference to the level, per cent, positive when the level is on the
    /// order's own side of the price (below for a long) and negative when the price has
    /// crossed it.
    fn distance_pct(reference: f64, level: f64, deal: &Deal) -> f64 {
        if reference <= 0.0 {
            return 0.0;
        }
        let signed = if deal.is_long() {
            reference - level
        } else {
            level - reference
        };
        signed / reference * 100.0
    }

    /// The tape replay — see the module doc for the two-level bookkeeping and for what the
    /// archived entry line contributes.
    ///
    /// Args:
    ///     deal: The report row.
    ///     ticks: The window's prints, ascending.
    ///     line: The archived points of the trade's own entry line, in the archive's order,
    ///         when the archive holds it.
    pub(super) fn run(
        &self,
        deal: &Deal,
        ticks: &[Tick],
        line: Option<&[(i64, f64)]>,
    ) -> Option<Fill> {
        if ticks.is_empty() {
            return None;
        }
        let (near_pct, far_pct) = self.params.bounds_pct(&deal.deltas);
        let raise_wait_ms = (self.params.raise_wait_s * 1000.0).max(0.0);
        let replace_delay_ms = (self.params.replace_delay_s * 1000.0).max(0.0);
        let latency_ms = self.params.latency_ms.max(0.0);

        let mut reference = Reference::new(
            self.params.use_price,
            self.params.fast_algo && raise_wait_ms > 0.0,
            deal.is_long(),
        );

        // The archive's moves, and where the order stood when the tape begins: the last
        // archived level at or before the first print, else the archive's first point (an
        // order placed inside the tape starts at its own moment), else nothing.
        let first_print_ms = ticks[0].time_ms as i64;
        let moves: Vec<(i64, f64)> = line
            .filter(|l| !l.is_empty())
            .map(archived_replacements)
            .unwrap_or_default();
        let start: Option<(i64, f64)> = moves
            .iter()
            .filter(|(t, _)| *t <= first_print_ms)
            .max_by_key(|(t, _)| *t)
            .or(moves.first())
            .copied();
        // The blind window: the core's moves archived inside it are applied as archived,
        // because the wait behind each began before the tape did. Only moves after the start
        // and before the fill count.
        let blind_until_ms = first_print_ms + raise_wait_ms.max(replace_delay_ms) as i64;
        let mut hints: Vec<(i64, f64)> = moves
            .iter()
            .filter(|(t, p)| {
                start.is_none_or(|(s, _)| *t > s)
                    && *t > first_print_ms
                    && *t < blind_until_ms
                    && *t < deal.buy_ms
                    && *p > 0.0
            })
            .copied()
            .collect();
        // The archive files a move as the old level's end and the new one's start, a few
        // milliseconds apart and not always in that order; the hints are walked in time.
        hints.sort_by_key(|(t, _)| *t);
        let mut hints = hints.into_iter().peekable();

        // Where the tape starts for the order: at the archived start, or at the first print.
        // Prints before the start only feed the reference.
        let start_ms = start.map(|(t, _)| t);
        let mut index = 0;
        if let Some(start_ms) = start_ms {
            while index < ticks.len() && (ticks[index].time_ms as i64) < start_ms {
                reference.observe(&ticks[index]);
                index += 1;
            }
        }
        // The exchange's level (what fills) and the core's (what the corridor is measured
        // against); `pending` is a move the core made that the exchange has not seen yet.
        let (mut exch_level, mut core_level) = match start {
            Some((_, price)) if price > 0.0 => (price, price),
            _ => {
                // No archive: the order is placed off the first print, which then cannot fill
                // it (it is the reference itself).
                let first = ticks.get(index)?;
                reference.observe(first);
                index += 1;
                let level = self.place(reference.price()?, far_pct, deal);
                (level, level)
            }
        };
        let mut pending: Option<(i64, f64)> = None;
        let mut breach: Option<(Breach, i64)> = None;

        for tick in &ticks[index..] {
            let t_ms = tick.time_ms as i64;
            let price = f64::from(tick.price);
            if !price.is_finite() || price <= 0.0 {
                continue;
            }
            // An archived move due by this print happened before it; the exchange learns of
            // it after the latency, like any move.
            while let Some((hint_ms, level)) = hints.next_if(|(h, _)| *h <= t_ms) {
                core_level = level;
                pending = Some((hint_ms + latency_ms as i64, level));
                breach = None;
            }
            if let Some((_, level)) = pending.filter(|(apply_at, _)| t_ms >= *apply_at) {
                exch_level = level;
                pending = None;
            }
            if reaches(price, exch_level, deal.is_long()) {
                return Some(Fill {
                    t_ms,
                    price: exch_level,
                });
            }
            reference.observe(tick);
            let Some(reference) = reference.price() else {
                continue;
            };
            let distance = Self::distance_pct(reference, core_level, deal);
            let now = if distance < near_pct {
                Some(Breach::Approach)
            } else if distance > far_pct {
                Some(Breach::Retreat)
            } else {
                None
            };
            match now {
                None => breach = None,
                Some(kind) => {
                    let since = match breach {
                        Some((seen, since)) if seen == kind => since,
                        _ => {
                            breach = Some((kind, t_ms));
                            t_ms
                        }
                    };
                    let wait_ms = match kind {
                        Breach::Approach => replace_delay_ms,
                        Breach::Retreat => raise_wait_ms,
                    };
                    if (t_ms - since) as f64 >= wait_ms {
                        core_level = self.place(reference, far_pct, deal);
                        pending = Some((t_ms + latency_ms as i64, core_level));
                        breach = None;
                    }
                }
            }
        }
        None
    }
}

/// The reference price the corridor is measured from, as the prints go by.
///
/// The last print of the wanted side (`MShotUsePrice`), falling back to the last print of any
/// side until one of that side has been seen; under algorithm 2 of `FastShotAlgo`, the extreme
/// of the wanted side's prints inside the last [`FAST_ALGO_WINDOW_MS`].
struct Reference {
    wanted_side: Option<Side>,
    fast: bool,
    is_long: bool,
    last_any: Option<f64>,
    last_side: Option<f64>,
    /// `(t_ms, price)` of the wanted side's prints inside the fast window, oldest first.
    recent: std::collections::VecDeque<(i64, f64)>,
}

impl Reference {
    fn new(use_price: UsePrice, fast: bool, is_long: bool) -> Self {
        Self {
            wanted_side: match use_price {
                UsePrice::Trade => None,
                UsePrice::Ask => Some(Side::Buy),
                UsePrice::Bid => Some(Side::Sell),
            },
            fast,
            is_long,
            last_any: None,
            last_side: None,
            recent: std::collections::VecDeque::new(),
        }
    }

    fn observe(&mut self, tick: &Tick) {
        let price = f64::from(tick.price);
        if !price.is_finite() || price <= 0.0 {
            return;
        }
        self.last_any = Some(price);
        if self.wanted_side.is_none_or(|s| s == tick.side) {
            self.last_side = Some(price);
            if self.fast {
                let t_ms = tick.time_ms as i64;
                self.recent.push_back((t_ms, price));
                while self
                    .recent
                    .front()
                    .is_some_and(|(t, _)| t_ms - *t > FAST_ALGO_WINDOW_MS)
                {
                    self.recent.pop_front();
                }
            }
        }
    }

    fn price(&self) -> Option<f64> {
        if self.fast && !self.recent.is_empty() {
            let prices = self.recent.iter().map(|(_, p)| *p);
            return if self.is_long {
                prices.reduce(f64::min)
            } else {
                prices.reduce(f64::max)
            };
        }
        self.last_side.or(self.last_any)
    }
}
