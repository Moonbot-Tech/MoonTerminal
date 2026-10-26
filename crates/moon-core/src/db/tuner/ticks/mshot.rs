//! The MoonShot entry model: a limit order at a fixed distance below the price (above, for a
//! short), walking a corridor and filled by the first print that reaches it.
//!
//! Mechanics, from the Moonbot FAQ ("Какие есть специфические параметры у стратегии MoonShot"):
//!
//! - the order stands `MShotPrice` % away from the reference price; the price may approach it
//!   down to `MShotPriceMin` % — closer than that, the order is re-placed at `MShotPrice` again
//!   after `MShotReplaceDelay` seconds; when the price runs away, it is re-placed after
//!   `MShotRaiseWait` seconds. How far it may run is not what the FAQ's wording suggests: the
//!   far bound uses the minimum print of the RaiseWait window (maximum for a short), with
//!   retreat depth `far + min(near, far - near)` and the width clamps in [`retreat_pct`];
//! - every `MShotAdd*Delta` adds `k · delta` to BOTH bounds, the delta's sign as the report
//!   carries it: the FAQ's `-10% + (-20 · 0.05) = -11%` is a coin UP 20 % on 3 h putting the
//!   order 1 % deeper (checked on the live tape on 2026-09-20: a long on ROSE, up 7 % / 11 % on
//!   3 h / 24 h, had its real order deeper than the unmodified corridor by exactly `Σ k · δ`);
//!   `MShotAddPriceBug` adds `k · pricebug` — deeper during exchange lag — and
//!   `MShotAddDistance` scales what the FAR bound receives by `1 + distance / 100`;
//! - `MShotMinusSatoshi` keeps the order at least two price steps off the ASK;
//! - `MShotUsePrice` picks the reference: the last trade, or the book's ASK / BID;
//! - the near bound uses the current price. A Trade re-place uses records younger than 74 ms,
//!   the core's short extreme, regardless of RaiseWait; ASK/BID use the last wanted-side print.
//!   Approach is due after `max(ReplaceDelay - 2 ms, 0)`; Retreat waits strictly more than
//!   RaiseWait since the last replacement command. Timers fire between prints before the next
//!   fill check. Only one re-place may be in flight; order-loop jitter is not modelled.
//!
//! Two things the tape cannot give and the model states instead: the book (an ASK / BID
//! reference is approximated by the last buy-side / sell-side print), and the time a
//! replacement takes to reach the exchange (`latency_ms`). The latency is what makes fills
//! possible at all — a corridor that re-places instantly is never reached by a spike — and its
//! first value is a constant; calibrating it from the archive's replacement times against the
//! moments the price left the corridor is phase 3.
//!
//! The first MoonShot take placement uses [`MSHOT_TAKE_LATENCY_MS`]; re-places keep `latency_ms`.
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
//! gives no way to compute. The archived start and blind-window levels apply as archived for
//! the fact and every variant, including the two-tick minimum (`MShotMinusSatoshi`): only the
//! creation placement and the Shift entry apply that minimum to the archived level. Mapping
//! archived fallback levels through a variant's far bound missed fills in
//! 11 of 164 paired variants that filled at the archived level. The archived level captures
//! the core's own reference, and on a coarse grid the model's
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
use super::{Deal, Deltas, EntryParams, Fill, ORDER_WAIT_CAP_MS, deltas, reaches, snap_to_step};
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
    /// `MShotReplaceDelay` and `MShotUsePrice`, which change the path (`FastShotAlgo` is read
    /// at the strategy's value; order-loop jitter is not modelled).
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

/// How a family of modifiers reads the market-wide and the BTC deltas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarketSign {
    /// With the sign the report carries, for an explicitly signed modifier family.
    Signed,
    /// As a magnitude for both MoonShot and the Delta Modifiers tab: market and BTC terms
    /// always use the absolute delta (KB PARAMS.md section 13).
    #[default]
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
    /// The ceiling on the `add_pricebug` term alone, per cent; 0 for none.
    /// Matches the core's rule: `MShotAddPriceBug` contributes at most 30 %;
    /// the Delta Modifiers tab's `AddPriceBug` has no ceiling of its own.
    pub pricebug_cap: f64,
}

impl Modifiers {
    /// The addition to the NEAR bound, in per cent: coin deltas keep their sign, while market
    /// and BTC deltas follow [`MarketSign`] (magnitudes by default, KB PARAMS.md section 13).
    pub fn near_addition(&self, d: &Deltas) -> f64 {
        let wide = |delta: f64| match self.market_sign {
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
            // Shared addition takes the mark as given: the sell sum orients it first,
            // while the corridor sum passes the raw delta.
            + self.add_mark * d.dmark
            + self.add_btc_1h * wide(d.btc1h)
            + self.add_btc_5m * wide(d.btc5m)
            + self.add_btc_1m * wide(d.btc1m)
            + self.add_market_1h * wide(d.market1h)
            + self.add_market_24h * wide(d.market24h)
            + self.add_pump_1h * d.pump1h
            + self.add_dump_1h * d.dump1h
            + self.pricebug_term(d.pricebug)
    }

    /// The `add_pricebug` term, under its own ceiling when the family has one.
    fn pricebug_term(&self, pricebug: f64) -> f64 {
        let term = self.add_pricebug * pricebug;
        if self.pricebug_cap > 0.0 {
            term.min(self.pricebug_cap)
        } else {
            term
        }
    }
}

/// The ceiling `MShotPriceBug`'s term stands under in the corridor family, per cent.
pub const MSHOT_PRICEBUG_CAP_PCT: f64 = 30.0;

/// Smallest near distance, per cent. Source:
/// https://moonbot.pro/documentation/advanced-strategy-parameters/moonshot-strategy-and-its-parameters
const NEAR_FLOOR_PCT: f64 = 0.002;

/// Smallest gap between near and far bounds, per cent (KB PARAMS.md section 13).
const WIDTH_FLOOR_PCT: f64 = 0.001;

/// Default replacement latency, milliseconds — see the module doc.
pub const DEFAULT_LATENCY_MS: f64 = 100.0;

/// First take placement latency only; re-places keep `latency_ms`.
/// Calibration on 2026-10-15: of 4899 long take closes, the first print after buy + L matched
/// the sell within 0.05 % for 2371 at 100 ms, 2715 at 160, 3256 at 200, 3543 at 220,
/// 3371 at 240, 2539 at 300 and 2083 at 400; both largest cores peaked at 220 ms.
/// On the bench, MoonShot take exits improved from 2114 to 3249 and stops stayed at 1994,
/// while using 220 ms for every exit move worsened MoonStrike/MoonHook takes.
pub const MSHOT_TAKE_LATENCY_MS: i64 = 220;

/// The latency a deal's entry re-places reach the exchange with: its core's measured round trip
/// ([`Deal::round_trip_ms`]), or the model settings' one latency for a core not measured. The
/// round trip alone, nothing added: on the user's data (2026-09-27, 568 MoonShot entries) it
/// judged 483 entries ✓ against 481 for the one 100 ms latency and 481 for 100 ms on top of it.
pub fn entry_latency_ms(model: ModelSettings, deal: &Deal) -> f64 {
    deal.round_trip_ms
        .filter(|rt| rt.is_finite())
        .unwrap_or(model.latency_ms)
        .max(0.0)
}

/// How far past the fact's fill a shifted order ([`MshotEntry::shifted_fill`]) may still be
/// reached by the same spike.
pub const SHIFT_WINDOW_MS: i64 = 2_000;

/// The seconds the FAQ's "4-second-old ASK" of `MShotSellAtLastPrice` looks back.
pub const PRE_SPIKE_LOOKBACK_MS: i64 = 4_000;

/// Trade re-places use records younger than 74 ms, the core's short extreme.
pub const FAST_ALGO_WINDOW_MS: i64 = 74;

/// MoonShot entry parameters, in the strategy's own units (per cent, seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct MshotParams {
    /// `MShotPrice` — the far bound, per cent below the reference (above, for a short).
    pub price_pct: f64,
    /// `MShotPriceMin` — the near bound.
    pub price_min_pct: f64,
    pub use_price: UsePrice,
    /// `MShotRaiseWait`, seconds — strict wait since the last command before a Retreat.
    pub raise_wait_s: f64,
    /// `MShotReplaceDelay`, seconds — before re-placing after the price CAME TOO CLOSE.
    pub replace_delay_s: f64,
    /// `AutoCancelBuy`, seconds from creation; zero disables expiry.
    pub auto_cancel_buy_s: f64,
    /// `MShotMinusSatoshi`.
    pub minus_satoshi: bool,
    /// `FastShotAlgo` — the core's order-loop period (16 ms on, 80 ms off). The model
    /// evaluates due commands without that loop's jitter (see the module doc).
    pub fast_algo: bool,
    pub modifiers: Modifiers,
    /// `MaxModifier` — the one ceiling the Delta Modifiers tab shares with the corridor: the sum
    /// of the `MShotAdd*` terms is capped by it from above when it is above zero, with its sign
    /// kept (no magnitude, unlike the sell's sum), matching the core's rule.
    pub max_modifier: f64,
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
            auto_cancel_buy_s: 0.0,
            minus_satoshi: false,
            fast_algo: false,
            modifiers: Modifiers::default(),
            max_modifier: 0.0,
            model: ModelSettings::default(),
        }
    }
}

impl MshotParams {
    /// The effective `(near, far)` bounds in per cent for a deal's deltas, floored and ordered.
    pub fn bounds_pct(&self, deltas: &Deltas) -> (f64, f64) {
        let near_add = self.addition(deltas);
        let far_add = near_add * (1.0 + self.modifiers.distance_pct / 100.0);
        let near = (self.price_min_pct + near_add).max(NEAR_FLOOR_PCT);
        let far = (self.price_pct + far_add).max(near + WIDTH_FLOOR_PCT);
        (near, far)
    }

    /// The `MShotAdd*` sum for these deltas, capped from above by `MaxModifier` when it is set —
    /// the near bound's addition; the far one's is it scaled by `1 + MShotAddDistance / 100`.
    fn addition(&self, deltas: &Deltas) -> f64 {
        let sum = self.modifiers.near_addition(deltas);
        if self.max_modifier > 0.0 {
            sum.min(self.max_modifier)
        } else {
            sum
        }
    }

    /// Whether the near bound sits below the far one — `MShotPriceMin < MShotPrice`, the
    /// corridor the fields describe: the price moves above the order between the two (FAQ: 10 %
    /// and 7 %, the price between +7 and +10 %). A pair at or past that is a corridor no field
    /// describes; `bounds_pct` still enforces the minimum width, but such a variant's raw
    /// fields describe no ordered corridor; callers exclude such a variant (search eligibility).
    pub fn is_ordered(&self) -> bool {
        self.price_min_pct < self.price_pct
    }

    /// Whether this corridor stands at least as far from the price as `own` at every one of
    /// `deltas` — both bounds, under the same deltas. The corridor is what the delta modifiers
    /// make of the base fields, so a variant may move its distance between `MShotPrice` and the
    /// `MShotAdd*` fields, but never end up nearer the price than the trade did: an order nearer
    /// the price fills on spikes the real one never reached, and those trades are not in the
    /// sample the variant is judged on.
    ///
    /// Args:
    ///     own: The corridor the trade ran with.
    ///     deltas: The deltas its entry order lived through ([`Deal::entry_deltas`]).
    ///     near_too: Whether the near bound is held as well as the far one — not under
    ///         [`EntryMethod::Shift`], which places the order at the far bound and reads nothing
    ///         of the near one.
    pub fn never_closer_than(&self, own: &MshotParams, deltas: &[Deltas], near_too: bool) -> bool {
        deltas.iter().all(|d| {
            let (near, far) = self.bounds_pct(d);
            let (own_near, own_far) = own.bounds_pct(d);
            far >= own_far - CORRIDOR_EPS_PCT && (!near_too || near >= own_near - CORRIDOR_EPS_PCT)
        })
    }
}

/// How far below the fact's bound, in per cent, a variant's bound may sit and still count as
/// not nearer: the same corridor arrived at through a different sum of modifiers differs only
/// by float rounding.
const CORRIDOR_EPS_PCT: f64 = 1e-9;

/// One placement of a modelled order and the corridor around it, until the next placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CorridorStep {
    /// When the core placed or re-placed the order, Unix ms.
    pub t_ms: i64,
    /// The order's level.
    pub level: f64,
    /// The corridor as the core saves it — a band of ORDER prices off the reference the level
    /// stood from: `R·(1 − near)` … `R·(1 − retreat)` for a long, mirrored for a short, with
    /// `retreat` = `2 · far − near` where `near ≥ far / 2` ([`retreat_pct`]). The same band the
    /// report's `BuyCorridorDown` / `BuyCorridorUp` hold for the fact, in the near-edge,
    /// retreat-edge order.
    pub band: (f64, f64),
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

    /// First command time: Approach waits from detection; Retreat strictly from the last command.
    fn due_ms(&self, kind: Breach, since: i64, last_replace_ms: i64) -> i64 {
        match kind {
            Breach::Approach => {
                since.saturating_add((wait_ms(self.params.replace_delay_s) - 2).max(0))
            }
            Breach::Retreat => last_replace_ms
                .saturating_add(wait_ms(self.params.raise_wait_s))
                .saturating_add(1)
                .max(since),
        }
    }

    /// Expire from known creation using the parsed timer. Corridor re-places are not
    /// manual moves and do not disable or restart the timer. Source:
    /// https://moonbot.pro/documentation/general-parameters-for-all-strategies/buy-conditions-tab
    fn expiry_ms(&self, deal: &Deal) -> Option<i64> {
        let seconds = self.params.auto_cancel_buy_s;
        if seconds > 0.0 {
            deal.order_open_ms()
                .map(|created_ms| created_ms.saturating_add((seconds * 1000.0) as i64))
        } else {
            None
        }
    }

    /// Place a far bound off the reference, enforcing two steps from `ask` when requested,
    /// then snap away from the reference. Source:
    /// https://moonbot.pro/documentation/advanced-strategy-parameters/moonshot-strategy-and-its-parameters
    fn place(&self, reference: f64, far_pct: f64, deal: &Deal, ask: Option<f64>) -> f64 {
        let distance = reference * far_pct / 100.0;
        let mut level = if deal.is_long() {
            reference - distance
        } else {
            reference + distance
        };
        if let Some(tick) = deal.tick.filter(|t| *t > 0.0) {
            if self.params.minus_satoshi {
                let keep_off = 2.0 * tick;
                // The ASK is never below the trade/BID reference; a lower taker-buy proxy is stale.
                let ask = ask.unwrap_or(reference).max(reference);
                level = if deal.is_long() {
                    level.min(ask - keep_off)
                } else {
                    level.max(ask + keep_off)
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
    /// applied instead, through the same placement rule ([`Self::place`]). Reuse of the fact's
    /// level also requires the same two-step minimum flag.
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
    ///     ask: Last taker-buy print before creation, the tape's ASK proxy.
    ///
    /// Returns:
    ///     The level, or `None` when the record proves no placement or no reference can be read
    ///     back from it.
    fn placement_at_creation(
        &self,
        deal: &Deal,
        created_ms: i64,
        far_pct: f64,
        ask: Option<f64>,
    ) -> Option<f64> {
        let fact_level = deal.entry_placed.filter(|l| l.is_finite() && *l > 0.0)?;
        let Some(EntryParams::MoonShot(own)) = deal.own_entry.as_ref() else {
            return Some(fact_level);
        };
        let (_, fact_far_pct) = own.bounds_pct(&deal.deltas_at(created_ms));
        if fact_far_pct == far_pct && own.minus_satoshi == self.params.minus_satoshi {
            return Some(fact_level);
        }
        Self::reference_of(fact_level, fact_far_pct, deal)
            .map(|r| self.place(r, far_pct, deal, ask))
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
    /// level back when its two-step minimum flag is also unchanged.
    ///
    /// Args:
    ///     deal: The trade.
    ///     own: The trade's own entry parameters.
    ///     (placed_ms, level): When and where the fact's order was placed ([`Self::fact_anchor`]).
    ///     ticks: Ascending prints providing the last ASK proxy at or before placement.
    fn anchored_level(
        &self,
        deal: &Deal,
        own: &MshotParams,
        (placed_ms, level): (i64, f64),
        ticks: &[Tick],
    ) -> Option<f64> {
        let deltas = deal.deltas_at(placed_ms);
        let (_, fact_far) = own.bounds_pct(&deltas);
        let (_, far) = self.params.bounds_pct(&deltas);
        if fact_far == far && own.minus_satoshi == self.params.minus_satoshi {
            return Some(level);
        }
        let end = ticks.partition_point(|t| t.time_ms as i64 <= placed_ms);
        let ask = ticks[..end]
            .iter()
            .rev()
            .find(|t| t.side == Side::Buy && t.price.is_finite() && t.price > 0.0)
            .map(|t| f64::from(t.price));
        Self::reference_of(level, fact_far, deal).map(|r| self.place(r, far, deal, ask))
    }

    /// The entry as a SHIFT of the fact: the variant's order at [`Self::anchored_level`] from the
    /// moment the fact's order last moved — standing where the fact's stood, one far bound deeper
    /// or shallower, since the same moment — filled by the first print that reaches it by
    /// the shift window past the buy (`ModelSettings::shift_window_ms`, [`SHIFT_WINDOW_MS`] by
    /// default): the same spike. Nothing about the corridor is modelled:
    /// the order is taken to stand still through the spike, as the fact's did. An enabled
    /// `AutoCancelBuy` with known creation ends the fill window at expiry.
    pub(super) fn shifted_fill(
        &self,
        deal: &Deal,
        ticks: &[Tick],
        own: &MshotParams,
        line: Option<&[(i64, f64)]>,
    ) -> Option<Fill> {
        let (since, fact_level) = Self::fact_anchor(deal, line);
        let level = self.anchored_level(deal, own, (since, fact_level), ticks)?;
        let window_end = deal.buy_ms + self.params.model.shift_window_ms;
        let until = self
            .expiry_ms(deal)
            .map_or(window_end, |expiry| window_end.min(expiry));
        ticks
            .iter()
            .map(|t| (t.time_ms as i64, f64::from(t.price)))
            .filter(|&(t, p)| t >= since && p > 0.0)
            .take_while(|&(t, _)| t <= until)
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
    ///         when the archive holds it. Fallback levels apply as archived.
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

    /// [`Self::trace`], each move with the corridor it stood in — what the tuner's trade pane
    /// draws as a variant's corridor. A move whose reference cannot be read back from its level
    /// is left out.
    pub fn corridor(
        &self,
        deal: &Deal,
        ticks: &[Tick],
        line: Option<&[(i64, f64)]>,
    ) -> (Option<Fill>, Vec<CorridorStep>) {
        let (fill, moves) = self.trace(deal, ticks, line);
        let mut bounds = LiveBounds::new(self.params, deal);
        let sign = if deal.is_long() { -1.0 } else { 1.0 };
        let steps = moves
            .into_iter()
            .filter_map(|(t_ms, level)| {
                let (near_pct, far_pct) = bounds.at(t_ms);
                let reference = Self::reference_of(level, far_pct, deal)?;
                let edge = |pct: f64| reference * (1.0 + sign * pct / 100.0);
                Some(CorridorStep {
                    t_ms,
                    level,
                    band: (edge(near_pct), edge(retreat_pct(near_pct, far_pct))),
                })
            })
            .collect();
        (fill, steps)
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
        let raise_wait_ms = wait_ms(self.params.raise_wait_s);
        let replace_delay_ms = wait_ms(self.params.replace_delay_s);
        let latency_ms = entry_latency_ms(self.params.model, deal);
        let expiry_ms = self.expiry_ms(deal);

        let mut reference = Reference::new(
            self.params.use_price,
            deal.is_long(),
            self.params.model.replace_window_ms,
            raise_wait_ms,
        );

        let first_print_ms = ticks[0].time_ms as i64;
        let mut last_replace_ms = first_print_ms;
        let mut index = 0;
        let mut hints: Vec<(i64, f64)> = Vec::new();
        // The whole life of the order, when the tape reaches back to its creation.
        let created = match deal
            .order_open_ms()
            .filter(|&created_ms| first_print_ms <= created_ms)
        {
            Some(created_ms) => {
                // Creation needs the ASK proxy from the prints preceding it.
                while index < ticks.len() && (ticks[index].time_ms as i64) < created_ms {
                    reference.observe(&ticks[index]);
                    index += 1;
                }
                let (_, far_pct) = bounds.at(created_ms);
                self.placement_at_creation(deal, created_ms, far_pct, reference.ask())
                    .map(|level| (created_ms, level))
            }
            None => None,
        };
        // The exchange's level (what fills; `None` until the order reaches the book) and the
        // core's (what the corridor is measured against); `pending` is a move the core made that
        // the exchange has not seen yet.
        let (mut exch_level, mut core_level, mut pending) = match created {
            Some((created_ms, level)) => {
                // The placement reaches the book a latency after creation, like any move.
                note(created_ms, level);
                last_replace_ms = created_ms;
                (
                    None,
                    level,
                    Some((created_ms.saturating_add(latency_ms as i64), level)),
                )
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
                // The blind window keeps the core's move times because each wait began before
                // the tape did. Only moves after the start and before the fill count; their
                // levels apply as archived for the fact and every variant, including the
                // two-tick minimum (MShotMinusSatoshi). Only the creation placement and the
                // Shift entry apply that minimum to the archived level.
                let blind_until_ms =
                    first_print_ms.saturating_add(raise_wait_ms.max(replace_delay_ms));
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
                    last_replace_ms = start_ms;
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
                        let level = self.place(
                            reference.placement(first.time_ms as i64)?,
                            far_pct,
                            deal,
                            reference.ask(),
                        );
                        note(first.time_ms as i64, level);
                        last_replace_ms = first.time_ms as i64;
                        level
                    }
                };
                (Some(level), level, None)
            }
        };
        let mut hints = hints.into_iter().peekable();
        // The open condition and its detection time; only Approach waits from detection.
        let mut breach: Option<(Breach, i64)> = None;
        let mut previous_ms = ticks
            .get(index.saturating_sub(1))
            .map_or(first_print_ms, |tick| tick.time_ms as i64);

        for tick in &ticks[index..] {
            let t_ms = tick.time_ms as i64;
            if expiry_ms.is_some_and(|expiry| t_ms > expiry) {
                return None;
            }
            let price = f64::from(tick.price);
            if !price.is_finite() || price <= 0.0 {
                continue;
            }
            // An archived move due by this print happened before it; the exchange learns of
            // it after the latency, like any move.
            while let Some((hint_ms, level)) = hints.next_if(|(h, _)| *h <= t_ms) {
                core_level = level;
                pending = Some((hint_ms.saturating_add(latency_ms as i64), level));
                breach = None;
                note(hint_ms, level);
                last_replace_ms = hint_ms;
            }
            // A retained low/high can age out without a print and open the retreat edge.
            if breach.is_none() {
                let opened = reference.retreat_open_ms(
                    previous_ms.max(last_replace_ms),
                    t_ms,
                    |extreme, at| {
                        let (near, far) = bounds.at(at);
                        Self::distance_pct(extreme, core_level, deal) > retreat_pct(near, far)
                    },
                );
                if let Some(opened) = opened {
                    breach = Some((Breach::Retreat, opened));
                }
            }
            previous_ms = t_ms;
            // Matches the core's rule: a due command fires before the next print can
            // cancel its breach or fill the order. The exchange still sees the old level
            // until the command's latency has elapsed.
            if let Some((kind, since)) = breach {
                let due = self.due_ms(kind, since, last_replace_ms);
                let due = pending.map_or(due, |(apply_at, _)| due.max(apply_at));
                if due < t_ms {
                    if let Some((_, level)) = pending.take() {
                        exch_level = Some(level);
                    }
                    let (_, far_pct) = bounds.at(due);
                    if let Some(placement) = reference.placement(due) {
                        core_level = self.place(placement, far_pct, deal, reference.ask());
                        pending = Some((due.saturating_add(latency_ms as i64), core_level));
                        breach = None;
                        note(due, core_level);
                        last_replace_ms = due;
                    }
                }
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
            } else if reference
                .window_extreme(t_ms)
                .is_some_and(|extreme| Self::distance_pct(extreme, core_level, deal) > retreat_pct)
            {
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
                    let due = self.due_ms(kind, since, last_replace_ms);
                    // Matches the core's rule: send the next command only after the
                    // exchange has answered the last, preserving the old fill level meanwhile.
                    let in_flight = pending.is_some();
                    if !in_flight && t_ms >= due {
                        // The new level comes off the window's extreme, not off the print that
                        // decided the move (see `Reference`).
                        let placement = reference.placement(t_ms).unwrap_or(check);
                        core_level = self.place(placement, far_pct, deal, reference.ask());
                        pending = Some((t_ms.saturating_add(latency_ms as i64), core_level));
                        breach = None;
                        note(t_ms, core_level);
                        last_replace_ms = t_ms;
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

/// Matches the core's rule: add min(near, far - near), floored at 0.001 percent and
/// capped by the room remaining below 99 percent, to the far depth.
fn retreat_pct(near_pct: f64, far_pct: f64) -> f64 {
    let width = near_pct.min(far_pct - near_pct).max(0.001);
    far_pct + width.min(99.0 - far_pct)
}

/// Bound externally supplied waits before converting them to the command clock.
fn wait_ms(seconds: f64) -> i64 {
    (seconds * 1000.0).clamp(0.0, ORDER_WAIT_CAP_MS as f64) as i64
}

/// Prices read by the corridor: current wanted-side price for Approach, the RaiseWait
/// window's minimum (maximum for a short) for Retreat, and records younger than 74 ms,
/// the core's short extreme, for Trade placement. ASK/BID placement uses the last side print.
struct Reference {
    wanted_side: Option<Side>,
    is_long: bool,
    /// The placement window (`ModelSettings::replace_window_ms`).
    window_ms: i64,
    /// The independent far-bound window (`MShotRaiseWait`).
    raise_window_ms: i64,
    last_any: Option<f64>,
    last_side: Option<f64>,
    /// Last taker-buy print, the ASK proxy regardless of the selected reference side.
    last_buy: Option<f64>,
    /// `(t_ms, price)` of the wanted side's prints inside the window, oldest first.
    recent: std::collections::VecDeque<(i64, f64)>,
    /// Far-bound candidates, oldest first with the best extreme at the front.
    raise_recent: std::collections::VecDeque<(i64, f64)>,
}

impl Reference {
    /// Build independent placement and Retreat windows for the selected price side.
    fn new(use_price: UsePrice, is_long: bool, window_ms: i64, raise_window_ms: i64) -> Self {
        Self {
            wanted_side: match use_price {
                UsePrice::Trade => None,
                UsePrice::Ask => Some(Side::Buy),
                UsePrice::Bid => Some(Side::Sell),
            },
            is_long,
            window_ms,
            raise_window_ms,
            last_any: None,
            last_side: None,
            last_buy: None,
            recent: std::collections::VecDeque::new(),
            raise_recent: std::collections::VecDeque::new(),
        }
    }

    /// Record a valid print and prune each window against its own age limit.
    fn observe(&mut self, tick: &Tick) {
        let price = f64::from(tick.price);
        if !price.is_finite() || price <= 0.0 {
            return;
        }
        self.last_any = Some(price);
        if tick.side == Side::Buy {
            self.last_buy = Some(price);
        }
        if self.wanted_side.is_none_or(|s| s == tick.side) {
            self.last_side = Some(price);
            let t_ms = tick.time_ms as i64;
            self.recent.push_back((t_ms, price));
            while self
                .recent
                .front()
                .is_some_and(|(t, _)| t_ms - *t >= self.window_ms)
            {
                self.recent.pop_front();
            }
            self.prune_raise(t_ms);
            while self.raise_recent.back().is_some_and(|(_, old)| {
                if self.is_long {
                    *old >= price
                } else {
                    *old <= price
                }
            }) {
                self.raise_recent.pop_back();
            }
            self.raise_recent.push_back((t_ms, price));
        }
    }

    /// The tape's ASK proxy, independent of `MShotUsePrice`.
    fn ask(&self) -> Option<f64> {
        self.last_buy
    }

    /// What the corridor is measured from. The core takes the lower of the last print and the
    /// best bid (a short: the higher of it and the ask); the tape has no book, and the last
    /// taker sell standing in for the bid lost 4 entries net on the live sample (2026-09-23) —
    /// the print alone is what the tape can say.
    fn check(&self) -> Option<f64> {
        self.last_side.or(self.last_any)
    }

    /// Far-bound reference at a decision time; zero RaiseWait uses the current price.
    fn window_extreme(&mut self, now_ms: i64) -> Option<f64> {
        if self.raise_window_ms == 0 {
            return self.check();
        }
        self.prune_raise(now_ms);
        self.raise_recent
            .front()
            .map(|(_, price)| *price)
            .or_else(|| self.check())
    }

    /// Discard expired extremes; each retained print is removed only once.
    fn prune_raise(&mut self, now_ms: i64) {
        while self
            .raise_recent
            .front()
            .is_some_and(|(at, _)| now_ms.saturating_sub(*at) > self.raise_window_ms)
        {
            self.raise_recent.pop_front();
        }
    }

    /// First expiry strictly between prints whose successor extreme opens Retreat.
    fn retreat_open_ms(
        &self,
        after: i64,
        before: i64,
        mut beyond: impl FnMut(f64, i64) -> bool,
    ) -> Option<i64> {
        if self.raise_window_ms == 0 {
            return None;
        }
        for (i, (at, _)) in self.raise_recent.iter().enumerate() {
            let expires = at.saturating_add(self.raise_window_ms).saturating_add(1);
            if expires >= before {
                break;
            }
            if expires <= after {
                continue;
            }
            let next = self
                .raise_recent
                .get(i + 1)
                .map(|(_, price)| *price)
                .or_else(|| self.check());
            if next.is_some_and(|price| beyond(price, expires)) {
                return Some(expires);
            }
        }
        None
    }

    /// Trade placement uses the short extreme at the command time, falling back to check
    /// when the window is empty; ASK/BID use their last print without a short window.
    fn placement(&self, now_ms: i64) -> Option<f64> {
        if self.wanted_side.is_some() {
            return self.check();
        }
        let prices = self
            .recent
            .iter()
            .filter(|(t, _)| now_ms - *t < self.window_ms)
            .map(|(_, p)| *p);
        let extreme = if self.is_long {
            prices.reduce(f64::min)
        } else {
            prices.reduce(f64::max)
        };
        extreme.or_else(|| self.check())
    }
}

#[cfg(test)]
mod tests;
