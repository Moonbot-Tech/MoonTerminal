//! The MoonShot entry model: a limit order at a fixed distance below the price (above, for a
//! short), walking a corridor and filled by the first print that reaches it.
//!
//! Mechanics, from the Moonbot FAQ ("Какие есть специфические параметры у стратегии MoonShot"):
//!
//! - the order stands `MShotPrice` % away from the reference price; the price may approach it
//!   down to `MShotPriceMin` % — closer than that, the order is re-placed at `MShotPrice` again
//!   after `MShotReplaceDelay` seconds; when the price runs away, it is re-placed after
//!   `MShotRaiseWait` seconds. How far it may run is not what the FAQ's wording suggests: the
//!   corridor the core saves with the report (`BuyCorridorDown` / `BuyCorridorUp`) is a band of
//!   ORDER prices symmetric around the placement, from `MShotPriceMin` to `2 · MShotPrice −
//!   MShotPriceMin` off the reference, modifiers included — to 0.05 % on 155 of 287 MoonShot
//!   trades (2026-09-23), the rest wider by what the live deltas moved. So the order is re-placed
//!   only past `far + min(near, far − near)` ([`retreat_pct`], the core developer's rule, the
//!   same band wherever `near ≥ far / 2`), not past `far`;
//! - every `MShotAdd*Delta` adds `k · delta` to BOTH bounds, the delta's sign as the report
//!   carries it: the FAQ's `-10% + (-20 · 0.05) = -11%` is a coin UP 20 % on 3 h putting the
//!   order 1 % deeper (checked on the live tape on 2026-09-20: a long on ROSE, up 7 % / 11 % on
//!   3 h / 24 h, had its real order deeper than the unmodified corridor by exactly `Σ k · δ`);
//!   `MShotAddPriceBug` adds `k · pricebug` — deeper during exchange lag — and
//!   `MShotAddDistance` scales what the FAR bound receives by `1 + distance / 100`;
//! - `MShotMinusSatoshi` keeps the order at least two price steps off the reference;
//! - `MShotUsePrice` picks the reference: the last trade, or the book's ASK / BID;
//! - the corridor is measured from the current price, and a re-placed order is put off the
//!   lowest print of the last 100 ms (highest, for a short) — for every `MShotRaiseWait`, which
//!   is what keeps the order from being placed off a spike's rebound (the core developer,
//!   2026-09-23; the FAQ gives the window to `FastShotAlgo`'s "algorithm 2" alone). See
//!   [`Reference`]. The core re-places one order at a time — the next only once the exchange
//!   answered the last — and checks the corridor on a timer, 16 ms with `FastShotAlgo` and 80 ms
//!   without; the model decides on the prints and folds the timer into `latency_ms`. Stepping the
//!   timer, and `FastShotAlgo = NO`'s own run-away rule and 100 ms sleep, moved the live sample by
//!   −1…−4 entries of 823 (2026-09-23) while the corridor's LEVEL was still off by 0.2 % on the
//!   median — `MShotAddMarkDelta` reads a mark price with no history here — and are left out
//!   until the level can tell them apart.
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
//!
//! What the report adds since 2026-09-21: the moment the core CREATED the order
//! (`Deal::order_open_ms`), and with it the whole of the order's life — where the record proves
//! the placement (`Deal::entry_placed`, see `record::entry_placement`: the archived line's first
//! point, or the buy price for an order the archive shows never moved). When the tape reaches
//! back to the creation the model replays that whole life: the order is placed at the creation
//! where the core placed it, and from then on the corridor is the model's own, with no blind
//! window and no archived move taken as given. A VARIANT is placed off the same reference by its
//! own bounds ([`MshotEntry::placement_at_creation`]) instead of inheriting the fact's level,
//! which is what a search over `MShotPrice` asks about.
//!
//! All of the above is the corridor MODEL. A variant can instead be replayed as a SHIFT of the
//! fact ([`EntryMethod`]): the fact's order where it stood at the spike, moved by the variant's
//! far bound, with no path of its own.

use super::settings::ModelSettings;
use super::verify::archived_replacements;
use super::{Deal, Deltas, EntryParams, Fill, deltas, reaches, snap_to_step};
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

/// How a MoonShot variant's entry is replayed — a setting of the search, not a strategy field.
///
/// Measured on 2026-09-23 against the one real counterfactual the history holds — 273 pairs of
/// cores with different `MShotPrice` filled on the same spike — predicting the second core's
/// fill from the first's tape and record: the model placed it a median 0.156 % off (99 within
/// 0.1 %) and filled 264; the shift 0.078 % off (141 within 0.1 %) and filled 250.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EntryMethod {
    /// The corridor model from the order's creation ([`MshotEntry::fill`] via `run`): the whole
    /// path the variant's order would have walked. The one that answers for `MShotRaiseWait`,
    /// `MShotReplaceDelay`, `MShotUsePrice` and `FastShotAlgo`, which change the path.
    #[default]
    Model,
    /// The fact's order at the spike, shifted by the variant's far bound
    /// ([`MshotEntry::shifted_fill`]): the path before the spike is the fact's, so only the
    /// depth parameters — `MShotPrice`, `MShotPriceMin` through the modifiers' floor,
    /// `MShotMinusSatoshi`, `MShotAdd*`, `MShotAddDistance` — move the entry; the waits and the
    /// reference do not.
    Shift,
}

impl EntryMethod {
    /// The strategy fields that change only the order's PATH — which a shift does not replay.
    const PATH_ONLY: [&'static str; 4] = [
        "MShotUsePrice",
        "MShotRaiseWait",
        "MShotReplaceDelay",
        "FastShotAlgo",
    ];

    /// Whether a replay by this method reads the strategy field `key` at all: a field it does
    /// not read moves no column, so the grid greys it out and the search leaves it alone. Every
    /// field that is not an entry field is read by both.
    pub fn reads(self, key: &str) -> bool {
        match self {
            Self::Model => true,
            Self::Shift => !Self::PATH_ONLY.contains(&key),
        }
    }
}

/// How a family of modifiers reads the market-wide deltas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarketSign {
    /// With the sign the report carries — `MShotAddMarketDelta` ("аналогично", FAQ :1289).
    #[default]
    Signed,
    /// As a magnitude — the Delta Modifiers tab's `AddMarketDelta` and `AddMarket24Delta`, "по
    /// модулю, то есть всегда положительный" (FAQ :1171, :1172).
    Magnitude,
}

/// A family of delta modifiers — `MShotAdd*` on the entry corridor, the Delta Modifiers tab's
/// `Add*` on the sell and the stop: per cent added per one per cent of the matching delta.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Modifiers {
    /// `MShotAdd5sDelta` (the exe's `MShotAdd*` list; no FAQ entry, no live strategy sets it on
    /// 2026-09-23); the Delta Modifiers tab has no such field and leaves it at zero.
    pub add_5s: f64,
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
    /// `AddMarket24Delta` of the Delta Modifiers tab (56 live strategies, 2026-09-23); the
    /// `MShotAdd*` family has no such field and leaves it at zero.
    pub add_market_24h: f64,
    /// `AddPump1h` / `AddDump1h` of the Delta Modifiers tab — FAQ :1177, :1178; no live strategy
    /// sets them (2026-09-23), read so that one that does is not silently unmodified.
    pub add_pump_1h: f64,
    pub add_dump_1h: f64,
    /// How the family reads the market-wide deltas.
    pub market_sign: MarketSign,
    /// `MShotAddDistance` — per cent by which the far bound's addition exceeds the near one's.
    pub distance_pct: f64,
}

impl Modifiers {
    /// The addition to the NEAR bound, in per cent, for these deltas — `Σ k · δ`, every delta
    /// with its own sign, so a coin that went up gets a deeper order (the module doc has the
    /// FAQ example and the live check behind the sign), the market-wide ones as the family reads
    /// them ([`MarketSign`]).
    pub fn near_addition(&self, d: &Deltas) -> f64 {
        let market = |delta: f64| match self.market_sign {
            MarketSign::Signed => delta,
            MarketSign::Magnitude => delta.abs(),
        };
        self.add_5s * d.d5s
            + self.add_1m * d.d1m
            + self.add_5m * d.d5m
            + self.add_15m * d.d15m
            + self.add_1h * d.d1h
            + self.add_3h * d.d3h
            + self.add_24h * d.d24h
            + self.add_mark * d.dmark
            + self.add_btc_1h * d.btc1h
            + self.add_btc_5m * d.btc5m
            + self.add_btc_1m * d.btc1m
            + self.add_market_1h * market(d.market1h)
            + self.add_market_24h * market(d.market24h)
            + self.add_pump_1h * d.pump1h
            + self.add_dump_1h * d.dump1h
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

/// How far past the fact's fill a shifted order ([`MshotEntry::shifted_fill`]) may still be
/// reached by the same spike.
pub const SHIFT_WINDOW_MS: i64 = 2_000;

/// The seconds the FAQ's "4-second-old ASK" of `MShotSellAtLastPrice` looks back.
pub const PRE_SPIKE_LOOKBACK_MS: i64 = 4_000;

/// The window a re-placed order's price is read off: the extreme print of the last 100 ms. The
/// FAQ gives it to `FastShotAlgo`'s algorithm 2 ("the minimum trade over 100 ms"); the core
/// developer (2026-09-23) to every re-place, whatever `MShotRaiseWait` — the trades of the last
/// ~75–150 ms.
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
    /// `FastShotAlgo` — the core's corridor timer (16 ms on, 80 ms off) and, off, a run-away
    /// read off the last print with a 100 ms sleep after each re-place (the core developer,
    /// 2026-09-23). Off on 224 of the 823 live MoonShot entries of that day; the model reads the
    /// same corridor either way (see the module doc).
    pub fast_algo: bool,
    pub modifiers: Modifiers,
    /// The model's own settings, not strategy fields: the replacement latency, the replay
    /// method, the windows.
    pub model: ModelSettings,
}

impl MshotParams {
    /// Whether two parameter sets are the same strategy — every strategy field equal, whatever
    /// the model's settings say.
    pub fn same_strategy(&self, other: &Self) -> bool {
        *self
            == Self {
                model: self.model,
                ..other.clone()
            }
    }
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
            model: ModelSettings::default(),
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

    /// How these parameters want a variant's entry replayed.
    pub fn method(&self) -> EntryMethod {
        self.params.model.entry_method
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

    /// The level the order stood at when the core created it, under these parameters.
    ///
    /// The fact's own level is the record's ([`Deal::entry_placed`]). A VARIANT's is the level
    /// its own far bound places off the same reference: the fact's level stands the fact's far
    /// bound off it, so the reference is read back from the level and this variant's bound
    /// applied instead, through the same placement rule ([`Self::place`]).
    ///
    /// Whose parameters are the fact's is [`Deal::own_entry`]. A variant is always replayed on a
    /// prepared deal (`record::prepare_deal` fills it), and the one caller without it — the
    /// verdict — replays the trade's own parameters, so a deal without it is placed at the
    /// fact's level as it stands.
    ///
    /// Args:
    ///     deal: The report row, with its model inputs.
    ///     created_ms: The order's creation, where both far bounds are read.
    ///     far_pct: These parameters' far bound for the deal at the creation.
    ///
    /// Returns:
    ///     The level, or `None` when the record proves no placement or no reference can be read
    ///     back from it.
    fn placement_at_creation(&self, deal: &Deal, created_ms: i64, far_pct: f64) -> Option<f64> {
        let fact_level = deal.entry_placed.filter(|l| l.is_finite() && *l > 0.0)?;
        let Some(EntryParams::MoonShot(own)) = deal.own_entry.as_ref() else {
            return Some(fact_level);
        };
        let (_, fact_far_pct) = own.bounds_pct(&deal.deltas_at(created_ms));
        if fact_far_pct == far_pct {
            return Some(fact_level);
        }
        Self::reference_of(fact_level, fact_far_pct, deal).map(|r| self.place(r, far_pct, deal))
    }

    /// The reference a placed level stood off, read back from the level and the far bound it was
    /// placed with.
    ///
    /// The level is the placement snapped AWAY from the reference (`place`): before the snap it
    /// lay within one step of it toward the price, half a step on average, and the reference is
    /// read back from there — off the snapped level itself it would sit a half step too far and
    /// every variant with it. (`MShotMinusSatoshi` binds only on a corridor narrower than two
    /// steps and is not undone.)
    fn reference_of(level: f64, far_pct: f64, deal: &Deal) -> Option<f64> {
        let half_step = deal.tick.filter(|t| *t > 0.0).map_or(0.0, |t| t / 2.0);
        let reference = if deal.is_long() {
            (level + half_step) / (1.0 - far_pct / 100.0)
        } else {
            (level - half_step) / (1.0 + far_pct / 100.0)
        };
        (reference.is_finite() && reference > 0.0).then_some(reference)
    }

    /// Where the fact's order stood when the spike came: the archive's last move of the entry line
    /// at or before the buy, and its moment; else the buy price, standing since the order's
    /// creation (the core files a line only when the order moved) or, without a stamp, since the
    /// run-up before the buy.
    pub(super) fn fact_anchor(deal: &Deal, line: Option<&[(i64, f64)]>) -> (i64, f64) {
        line.map(archived_replacements)
            .and_then(|moves| {
                moves
                    .into_iter()
                    .filter(|&(t, p)| t <= deal.buy_ms && p > 0.0)
                    .max_by_key(|&(t, _)| t)
            })
            .unwrap_or_else(|| {
                let since = deal
                    .order_open_ms()
                    .unwrap_or(deal.buy_ms - super::RUN_UP_MS);
                (since, deal.buy_price)
            })
    }

    /// The level these parameters would have held where the fact's order stood at `level`: the
    /// same reference, this variant's far bound — both far bounds read off the deltas as they
    /// stood when the fact's order was placed there, the moment its far bound was computed (the
    /// report's snapshot where the deal has no live track). The fact's own far bound gives the
    /// level back.
    ///
    /// Args:
    ///     deal: The trade.
    ///     own: The trade's own entry parameters.
    ///     (placed_ms, level): When and where the fact's order was placed ([`Self::fact_anchor`]).
    fn anchored_level(
        &self,
        deal: &Deal,
        own: &MshotParams,
        (placed_ms, level): (i64, f64),
    ) -> Option<f64> {
        let deltas = deal.deltas_at(placed_ms);
        let (_, fact_far) = own.bounds_pct(&deltas);
        let (_, far) = self.params.bounds_pct(&deltas);
        if fact_far == far {
            return Some(level);
        }
        Self::reference_of(level, fact_far, deal).map(|r| self.place(r, far, deal))
    }

    /// The entry as a SHIFT of the fact: the variant's order at [`Self::anchored_level`] from the
    /// moment the fact's order last moved — standing where the fact's stood, one far bound deeper
    /// or shallower, since the same moment — filled by the first print that reaches it by
    /// the shift window past the buy (`ModelSettings::shift_window_ms`, [`SHIFT_WINDOW_MS`] by
    /// default): the same spike. Nothing about the corridor is modelled:
    /// the order is taken to stand still through the spike, as the fact's did.
    pub(super) fn shifted_fill(
        &self,
        deal: &Deal,
        ticks: &[Tick],
        own: &MshotParams,
        line: Option<&[(i64, f64)]>,
    ) -> Option<Fill> {
        let (since, fact_level) = Self::fact_anchor(deal, line);
        let level = self.anchored_level(deal, own, (since, fact_level))?;
        ticks
            .iter()
            .map(|t| (t.time_ms as i64, f64::from(t.price)))
            .filter(|&(t, p)| t >= since && p > 0.0)
            .take_while(|&(t, _)| t <= deal.buy_ms + self.params.model.shift_window_ms)
            .find(|&(_, p)| reaches(p, level, deal.is_long()))
            .map(|(t_ms, _)| Fill { t_ms, price: level })
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
        self.run_traced(deal, ticks, line, None)
    }

    /// [`Self::run`], and every move the core would have made — `(t_ms, level)` of each
    /// placement and re-place, as the core decided it — for holding the order's path against the
    /// archived entry line.
    pub fn trace(
        &self,
        deal: &Deal,
        ticks: &[Tick],
        line: Option<&[(i64, f64)]>,
    ) -> (Option<Fill>, Vec<(i64, f64)>) {
        let mut moves = Vec::new();
        let fill = self.run_traced(deal, ticks, line, Some(&mut moves));
        (fill, moves)
    }

    /// Args (beyond [`Self::run`]'s):
    ///     moves: Where to record every move, when asked.
    fn run_traced(
        &self,
        deal: &Deal,
        ticks: &[Tick],
        line: Option<&[(i64, f64)]>,
        mut moves: Option<&mut Vec<(i64, f64)>>,
    ) -> Option<Fill> {
        let mut note = |t_ms: i64, level: f64| {
            if let Some(moves) = moves.as_deref_mut() {
                moves.push((t_ms, level));
            }
        };
        if ticks.is_empty() {
            return None;
        }
        let mut bounds = LiveBounds::new(self.params, deal);
        let raise_wait_ms = (self.params.raise_wait_s * 1000.0).max(0.0);
        let replace_delay_ms = (self.params.replace_delay_s * 1000.0).max(0.0);
        let latency_ms = self.params.model.latency_ms.max(0.0);

        let mut reference = Reference::new(
            self.params.use_price,
            deal.is_long(),
            self.params.model.replace_window_ms,
        );

        let first_print_ms = ticks[0].time_ms as i64;
        let mut index = 0;
        let mut hints: Vec<(i64, f64)> = Vec::new();
        // The whole life of the order, when the tape reaches back to its creation.
        let created = match deal
            .order_open_ms()
            .filter(|&created_ms| first_print_ms <= created_ms)
        {
            Some(created_ms) => {
                let (_, far_pct) = bounds.at(created_ms);
                self.placement_at_creation(deal, created_ms, far_pct)
                    .map(|level| (created_ms, level))
            }
            None => None,
        };
        // The exchange's level (what fills; `None` until the order reaches the book) and the
        // core's (what the corridor is measured against); `pending` is a move the core made that
        // the exchange has not seen yet.
        let (mut exch_level, mut core_level, mut pending) = match created {
            Some((created_ms, level)) => {
                // Prints before the creation only feed the reference, and the placement reaches
                // the book a latency after it, like any move.
                while index < ticks.len() && (ticks[index].time_ms as i64) < created_ms {
                    reference.observe(&ticks[index]);
                    index += 1;
                }
                note(created_ms, level);
                (None, level, Some((created_ms + latency_ms as i64, level)))
            }
            None => {
                // The archive's moves, and where the order stood when the tape begins: the last
                // archived level at or before the first print, else the archive's first point
                // (an order placed inside the tape starts at its own moment), else nothing.
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
                // The blind window: the core's moves archived inside it are applied as
                // archived, because the wait behind each began before the tape did. Only moves
                // after the start and before the fill count.
                let blind_until_ms = first_print_ms + raise_wait_ms.max(replace_delay_ms) as i64;
                hints = moves
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
                // The archive files a move as the old level's end and the new one's start, a
                // few milliseconds apart and not always in that order; the hints are walked in
                // time.
                hints.sort_by_key(|(t, _)| *t);
                // Where the tape starts for the order: at the archived start, or at the first
                // print. Prints before the start only feed the reference.
                if let Some((start_ms, _)) = start {
                    while index < ticks.len() && (ticks[index].time_ms as i64) < start_ms {
                        reference.observe(&ticks[index]);
                        index += 1;
                    }
                }
                let level = match start {
                    Some((_, price)) if price > 0.0 => price,
                    _ => {
                        // No archive: the order is placed off the first print, which then
                        // cannot fill it (it is the reference itself).
                        let first = ticks.get(index)?;
                        reference.observe(first);
                        index += 1;
                        let (_, far_pct) = bounds.at(first.time_ms as i64);
                        let level =
                            self.place(reference.placement(first.time_ms as i64)?, far_pct, deal);
                        note(first.time_ms as i64, level);
                        level
                    }
                };
                (Some(level), level, None)
            }
        };
        let mut hints = hints.into_iter().peekable();
        // The breach the corridor is waiting out: which way, and since when.
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
                note(hint_ms, level);
            }
            if let Some((_, level)) = pending.filter(|(apply_at, _)| t_ms >= *apply_at) {
                exch_level = Some(level);
                pending = None;
            }
            if let Some(level) = exch_level.filter(|&level| reaches(price, level, deal.is_long())) {
                return Some(Fill { t_ms, price: level });
            }
            reference.observe(tick);
            let Some(check) = reference.check() else {
                continue;
            };
            let (near_pct, far_pct) = bounds.at(t_ms);
            let retreat_pct = retreat_pct(near_pct, far_pct);
            let distance = Self::distance_pct(check, core_level, deal);
            let now = if distance < near_pct {
                Some(Breach::Approach)
            } else if distance > retreat_pct {
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
                    // One re-place at a time: the core sends the next only once the exchange
                    // has answered the last — the archive files every re-place as a request and
                    // its answer, and none overlaps the one before (2026-09-23). A spike's prints
                    // inside the round trip move nothing; without the rule the model chased them
                    // print by print, 676 re-places the archive never shows on 143 orders
                    // replayed from their creation, against 389 with it.
                    let in_flight = pending.is_some();
                    if !in_flight && (t_ms - since) as f64 >= wait_ms {
                        // The new level comes off the window's extreme, not off the print that
                        // decided the move (see `Reference`).
                        let placement = reference.placement(t_ms).unwrap_or(check);
                        core_level = self.place(placement, far_pct, deal);
                        pending = Some((t_ms + latency_ms as i64, core_level));
                        breach = None;
                        note(t_ms, core_level);
                    }
                }
            }
        }
        None
    }
}

/// The corridor's `(near, far)` bounds as the core held them while the order lived: the core
/// moves the corridor when a delta moves ("Дельта меняется — ордер переставляется", FAQ :1289),
/// so the bounds are re-read off the deal's live deltas ([`Deal::deltas_at`]) once per refresh
/// step of the track ([`deltas::STEP_MS`]) — the deltas hold still inside one — and once for a
/// deal without a track, whose deltas are the snapshot throughout.
struct LiveBounds<'a> {
    params: &'a MshotParams,
    deal: &'a Deal,
    /// The step the bounds were last read for.
    step: Option<i64>,
    bounds: (f64, f64),
}

impl<'a> LiveBounds<'a> {
    fn new(params: &'a MshotParams, deal: &'a Deal) -> Self {
        Self {
            params,
            deal,
            step: None,
            bounds: (0.0, 0.0),
        }
    }

    fn at(&mut self, t_ms: i64) -> (f64, f64) {
        let step = match self.deal.delta_track {
            Some(_) => t_ms.div_euclid(deltas::STEP_MS),
            None => 0,
        };
        if self.step != Some(step) {
            self.bounds = self.params.bounds_pct(&self.deal.deltas_at(t_ms));
            self.step = Some(step);
        }
        self.bounds
    }
}

/// How far off the reference a run-away price leaves the order before the core re-places it:
/// `MShotPrice + min(MShotPriceMin, MShotPrice − MShotPriceMin)` (the core developer,
/// 2026-09-23). Where `near ≥ far / 2` — every one of the 292 corridors the core saved by
/// 2026-09-23 — that is `2 · far − near`, the band's far edge the saved corridor shows; a
/// variant with a narrower `near` re-places at `far + near`. Where the bounds meet after the
/// modifiers (`bounds_pct` lifts far to near) the band has no width and every move re-places.
fn retreat_pct(near_pct: f64, far_pct: f64) -> f64 {
    far_pct + near_pct.min(far_pct - near_pct)
}

/// The prices the corridor reads, as the prints go by (the core developer, 2026-09-23).
///
/// - [`Self::check`], what the corridor is measured from — whether the price came too close or
///   ran away: the last print of the wanted side (`MShotUsePrice`), falling back to the last
///   print of any side until one of that side has been seen. The core reads the current price
///   for both, and a run-away holds for `MShotRaiseWait` exactly when the price stayed away that
///   long — the timer in [`MshotEntry::run`]; neither looks at a window.
/// - [`Self::placement`], what a re-placed order is put off: the extreme of the wanted side's
///   prints inside the last `window_ms` ([`FAST_ALGO_WINDOW_MS`] by default) — the lowest for a
///   long — whatever `MShotRaiseWait` is. The core takes the minimum of the trades of the last
///   ~75–150 ms, so a spike's own low prints place the order below it rather than off the
///   rebound.
struct Reference {
    wanted_side: Option<Side>,
    is_long: bool,
    /// The placement window (`ModelSettings::replace_window_ms`).
    window_ms: i64,
    last_any: Option<f64>,
    last_side: Option<f64>,
    /// `(t_ms, price)` of the wanted side's prints inside the window, oldest first.
    recent: std::collections::VecDeque<(i64, f64)>,
}

impl Reference {
    fn new(use_price: UsePrice, is_long: bool, window_ms: i64) -> Self {
        Self {
            wanted_side: match use_price {
                UsePrice::Trade => None,
                UsePrice::Ask => Some(Side::Buy),
                UsePrice::Bid => Some(Side::Sell),
            },
            is_long,
            window_ms,
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
            let t_ms = tick.time_ms as i64;
            self.recent.push_back((t_ms, price));
            while self
                .recent
                .front()
                .is_some_and(|(t, _)| t_ms - *t > self.window_ms)
            {
                self.recent.pop_front();
            }
        }
    }

    /// What the corridor is measured from. The core takes the lower of the last print and the
    /// best bid (a short: the higher of it and the ask); the tape has no book, and the last
    /// taker sell standing in for the bid lost 4 entries net on the live sample (2026-09-23) —
    /// the print alone is what the tape can say.
    fn check(&self) -> Option<f64> {
        self.last_side.or(self.last_any)
    }

    /// What a re-placed order is put off, deciding at `now_ms`: the extreme of the wanted side's
    /// prints of the last `window_ms` before it, else the check price. The window is
    /// pruned only when that side prints, so it is read against the decision's own moment: a
    /// burst an ASK / BID side printed seconds ago is not "the last 100 ms". Its fallback is still
    /// that side's last print, however old — the tape has no book, and the corridor is measured
    /// off that same print; the placement only stays consistent with it.
    fn placement(&self, now_ms: i64) -> Option<f64> {
        let prices = self
            .recent
            .iter()
            .filter(|(t, _)| now_ms - *t <= self.window_ms)
            .map(|(_, p)| *p);
        let extreme = if self.is_long {
            prices.reduce(f64::min)
        } else {
            prices.reduce(f64::max)
        };
        extreme.or_else(|| self.check())
    }
}
