//! The "Entry/Exit" tuning axis — a what-if evaluated by REPLAYING the trade tape around each
//! closed trade rather than by an SQL mask over report fields.
//!
//! Two groups of parameters, one variant column: an **entry model** decides where the buy order
//! would have stood and when a print would have filled it (only strategy kinds whose mechanics
//! are a function of the tape have one — MoonShot today, see [`entry`]), and an **exit model**
//! decides where the sell line stood and when a print crossed it (the same for every kind, see
//! [`exit`]). A kind without an entry model takes its entry from the report row as it happened.
//!
//! Everything here is a pure function of one report row, its prints and the parameters; the
//! rows, the prints and the order traces come from the caller. Nothing opens a database — the
//! callers already run the axis' reads on a background executor, and the search of phase 2 calls
//! [`simulate`] thousands of times per second.
//!
//! The model is NOT a backtest: the core is not in the loop, and the sample is only the fills
//! that happened. A variant that puts the order DEEPER is judged honestly (the tape says whether
//! the spike reached the new level); a variant that puts it CLOSER is underestimated, because
//! spikes the real order never reached are not in the report at all. The caller writes that
//! into the column caption; the model does not hide it.
//!
//! Sources of the mechanics: the Moonbot FAQ (`data/faqru.tsv`, "Какие есть специфические
//! параметры у стратегии MoonShot" and the `PriceDown*` / `SellLevel*` / `SellShot*` answers),
//! checked against the live `strategies.sqlite` field names on 2026-09-20.

use crate::feed::types::Tick;
use crate::market::trade_replay::Coverage;

pub mod calibrate;
pub mod deals;
pub mod entry;
pub mod exit;
pub mod hook;
pub mod line;
pub mod mshot;
pub mod params;
pub mod scope;
pub mod search;
pub mod stats;
pub mod verify;

pub use deals::{DealsRead, read_deals};
pub use entry::{EntryModel, entry_model_for};
pub use exit::{ExitModel, ExitParams, archived_pre_spike_ask, archived_take, take_model_for};
pub use hook::{HookDetect, KIND_MOONHOOK, hook_take_pct, parse_hook_detect};
pub use mshot::{MshotEntry, MshotParams, UsePrice};
pub use params::{ParamGroup, ParamKind, TICK_PARAMS, TickParam};
pub use scope::{is_service_row, is_tunable};
pub use search::{PreparedDeal, SearchParams, SearchResult, suggest, variant_tally};
pub use stats::{fact_stats, stats_of};
pub use verify::{Verdict, verify};

/// Relative tolerance under which a modelled price counts as reproducing the fact: 0.05 %.
///
/// One price step on a mid-priced coin is well under this, and the tape carries `f32` prices,
/// whose 7 significant digits sit an order of magnitude below it too.
pub const PRICE_TOLERANCE: f64 = 0.0005;

/// Relative slack when a print is held against a level: the tape carries `f32` prices, so a
/// print AT the level can sit a few units in the seventh digit past it. Well under any price
/// step, so it never turns a miss into a fill.
pub const PRICE_EPS: f64 = 1e-6;

/// Whether a print at `price` reaches a level at `level` from the position's side — at or
/// below for a long buy or a short take, at or above for the mirror — with [`PRICE_EPS`].
///
/// Args:
///     price: The print.
///     level: The order.
///     from_below: `true` when the print must come DOWN to the level (a long entry, a short
///         take); `false` when it must come up.
pub fn reaches(price: f64, level: f64, from_below: bool) -> bool {
    if from_below {
        price <= level * (1.0 + PRICE_EPS)
    } else {
        price >= level * (1.0 - PRICE_EPS)
    }
}

/// A level snapped to the market's price grid: down to the step below when `down`, up to the
/// step above otherwise — the entry's placement, which the core rounds AWAY from the price. A
/// level already on the grid stays — the quotient is read with [`PRICE_EPS`] of slack, so
/// `0.3379` computed as `0.33789999` does not lose a step. A non-positive step snaps nothing.
///
/// Args:
///     level: The price to snap.
///     tick: The price step.
///     down: Whether to round toward zero (a long's buy sits below the price) or away from it
///         (a short's sits above).
pub fn snap_to_step(level: f64, tick: f64, down: bool) -> f64 {
    if tick <= 0.0 || tick.is_nan() || !level.is_finite() {
        return level;
    }
    let steps = level / tick;
    let snapped = if down {
        (steps + PRICE_EPS).floor()
    } else {
        (steps - PRICE_EPS).ceil()
    };
    snapped * tick
}

/// A level rounded to the NEAREST step of the price grid — the sell line's placement: the
/// archived Exit lines round both ways (2026-09-21, 458 deals: a floor lost 5 exits the
/// nearest step keeps, ARX's four levels agree with both). A non-positive step rounds nothing.
///
/// Args:
///     level: The price to round.
///     tick: The price step.
pub fn round_to_step(level: f64, tick: f64) -> f64 {
    if tick <= 0.0 || tick.is_nan() || !level.is_finite() {
        return level;
    }
    (level / tick).round() * tick
}

/// The report-side deltas the MoonShot modifiers read, as of the BUY of the trade.
///
/// The report stamps them once, at the buy; the model treats them as constant over the window,
/// which is a stated assumption — on a 5-minute window a 1-hour delta barely moves, a 1-minute
/// delta can. All values are per cent, exactly as `orders_rep` stores them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Deltas {
    /// The last five seconds' move (`d5s`) — the spike itself; shown, not a modifier input.
    pub d5s: f64,
    pub d1m: f64,
    pub d5m: f64,
    pub d15m: f64,
    pub d1h: f64,
    pub d3h: f64,
    pub d24h: f64,
    /// Mark-price delta (`dmark`).
    pub dmark: f64,
    /// Price-bug measure (`pricebug`), a positive lag figure the exchange showed at the buy.
    pub pricebug: f64,
    /// BTC 1-hour delta (`btc1hdelta`).
    pub btc1h: f64,
    /// BTC 5-minute delta (`btc5mdelta`).
    pub btc5m: f64,
    /// BTC 1-minute delta (`dbtc1m`) — read by the sell side's `AddBTC1mDelta`; MoonShot's
    /// corridor family has no term for it.
    pub btc1m: f64,
    /// Exchange-wide 1-hour delta (`exchange1hdelta`).
    pub market1h: f64,
}

/// One closed trade as the model needs it — the report row narrowed to the fields the tape
/// replay reads, plus what the caller knows about the market.
#[derive(Clone, Debug, PartialEq)]
pub struct Deal {
    /// `reportuid` — the key of the order-trace archive and of the coverage map.
    pub report_uid: i64,
    pub core_uid: u64,
    /// The core's name as the report row carries it (`core_name`) — the table's core column;
    /// the uid is the key, the name is what the user knows the core by.
    pub core_name: String,
    pub strategy_id: i64,
    /// Strategy kind as the strategy list names it (`"MoonShot"`, `"Spread"`, …); selects the
    /// entry model through [`entry_model_for`].
    pub kind: String,
    /// The coin as the report spells it (`coin`, e.g. `BEN`); the market (`BEN_USDT`) is the
    /// caller's to resolve through `symbol::` for the tape lookup.
    pub coin: String,
    /// `buydatems` — the fill of the entry, Unix ms. Rows without a millisecond stamp are not
    /// deals for this axis; the caller drops them and counts them.
    pub buy_ms: i64,
    /// `closedatems` — the fill of the exit, Unix ms.
    pub close_ms: i64,
    pub buy_price: f64,
    pub sell_price: f64,
    /// `spentbtc` — what the entry cost, in the scan's own money unit (the row's quote, or
    /// USDT where the scan's projection converts a valued scope); the money KPI of a variant
    /// is `profit_pct * spent` so the columns stay in the units of the "Fact" column.
    pub spent: f64,
    pub is_short: bool,
    /// `sellreason` as the core wrote it (`"Auto Price Down"`, `"Sell Price"`, …).
    pub sell_reason: String,
    /// The row's result in the scope's active metric (`pnl` of the unified source) — what the
    /// "Fact" column over the same subset sums.
    pub fact_pnl: f64,
    /// The row's result as MONEY IN USDT whatever the metric — `profitbtc` of the USDT-valued
    /// source (`read_deals`), fees as the core wrote them, converted like the Report's USDT
    /// column; `None` when the scope's money cannot be valued in USDT (a non-USDT quote
    /// without valuation coverage, a mixed scope), and the table shows a dash. The table's
    /// profit column alone reads it — the KPI stays in the scope's own unit through
    /// `fact_pnl` and `spent`.
    pub profit: Option<f64>,
    pub deltas: Deltas,
    /// Price step of the market, when the caller could resolve it (the live catalog, or
    /// [`infer_tick`] over the window). `None` disables the step-bound rules and rounds nothing.
    pub tick: Option<f64>,
    /// The book's ASK the core read `MShotSellAtLastPrice` off — "the 4-second-old ASK, before
    /// the spike" — when the caller could recover it: the archived Exit line's first point is
    /// the take as placed, and dividing out the trade's own `MShotSellPriceAdjust` gives the
    /// ask back. `None` leaves the model to its own reading of the tape (the last print at
    /// least [`mshot::PRE_SPIKE_LOOKBACK_MS`] before the fill), which sits below the ask on a
    /// dump by 0.1–0.5 % (B2/CELR 2026-09-20, GSTOCKBSC 2026-09-21) and shifts every level
    /// the sell line then steps down from.
    pub pre_spike_ask: Option<f64>,
    /// The take as the core placed it — the archived Exit line's first point — when the
    /// archive holds it. The one take rule the model has is MoonShot's (`SellPrice` lifted by
    /// `MShotSellAtLastPrice`); every other kind places its take by a rule of its own
    /// (MoonHook's buffer, Spread's level), and for those the archive is where the line
    /// starts. FLOCK 2026-09-21 (HookN0, short): the model's `SellPrice` take at −1.0 %, the
    /// core's at −2.2 %, every PriceDown step then a different level.
    pub archived_take: Option<f64>,
    /// The detect depth of a MoonHook trade, per cent, as the core wrote it into the report's
    /// `comment` — the base of that kind's take rule ([`hook::hook_take_pct`]). `None` for
    /// every other kind, and for a hook row whose comment the scan could not read.
    pub hook_depth_pct: Option<f64>,
    /// The take the core actually placed on a hook trade, per cent from the buy, as the same
    /// comment states it. Never an input of the model, and nothing asserts it: the ignored
    /// `tests::real_data` harness prints it beside the formula's own number so a developer can
    /// see the two drift apart on real trades. It could not stand in for the formula anyway —
    /// a variant asks about a level the core never used, and this number answers only for the
    /// one it did.
    pub hook_stated_take_pct: Option<f64>,
    /// How much later than `PriceDownDelay` the deal's CORE makes the next PriceDown step after
    /// one that moved the order, milliseconds — its replace round trip, calibrated by the caller
    /// off the core's own archived lines ([`calibrate`]); 0 runs the steps on the plain schedule.
    pub step_lag_ms: f64,
}

impl Deal {
    /// Whether the position is long; the mirror of every level rule keys off this.
    pub fn is_long(&self) -> bool {
        !self.is_short
    }
}

/// Where and when the modelled entry order filled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    /// Time of the print that reached the level, Unix ms.
    pub t_ms: i64,
    /// The level itself — a limit fills at its own price, never at the print's.
    pub price: f64,
}

/// How a modelled position closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitKind {
    /// A print reached the take-profit level.
    Take,
    /// A print crossed the moving sell line (PriceDown / SellLevel / SellShot).
    Line,
    /// The stop-loss level was crossed: a market exit at the print.
    Stop,
    /// Nothing closed the position before the tape ran out. Not a trade: excluded from the KPI
    /// and counted in the caption. Under the strategy's own parameters this is the model
    /// failing to reproduce an exit the core made, and the verdict says so.
    OpenAtWindowEnd,
}

/// Where and when the modelled position closed, and by which rule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Exit {
    pub t_ms: i64,
    pub price: f64,
    pub kind: ExitKind,
}

/// One trade's modelled outcome under one parameter set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outcome {
    /// `None` — the entry never filled on this tape; there is no trade and the KPI does not
    /// count it (the caption's "by N of M" is where that shows).
    pub fill: Option<Fill>,
    pub exit: Option<Exit>,
    /// Signed result in per cent of the fill price, sign already flipped for a short. `None`
    /// without a fill, with an [`ExitKind::OpenAtWindowEnd`] exit, or when either price is not
    /// a price (see [`profit_pct`]) — none of those is a trade.
    pub profit_pct: Option<f64>,
}

impl Outcome {
    /// Money result in the row's quote currency, or `None` when the outcome is not a trade.
    pub fn profit_money(&self, deal: &Deal) -> Option<f64> {
        self.profit_pct.map(|pct| pct / 100.0 * deal.spent)
    }

    /// Whether the outcome is a closed trade the KPI may count.
    pub fn is_trade(&self) -> bool {
        self.profit_pct.is_some()
    }
}

/// The entry-side parameters of one variant: the strategy kind's own model, or the fact.
#[derive(Clone, Debug, PartialEq)]
pub enum EntryParams {
    /// Take the entry as the report has it — for kinds without a model, and for a variant
    /// whose "Entry" group is not being varied.
    Fact,
    MoonShot(MshotParams),
}

/// The tape must reach this far back before the buy for the corridor to have a run-up. The
/// replay worker walks exactly this much as part of the trade for a model's request
/// (`trade_replay::MODEL_PAD_MS`), so the two are one number.
pub const RUN_UP_MS: i64 = crate::market::trade_replay::MODEL_PAD_MS;

/// The tape must reach this far past the close for the exit to have a tail: a line the fact
/// crossed at the close is reproduced by a print at or before it, but a near miss a few seconds
/// later must be judged on its price — [`verify`] counts a model still open when the tape ends
/// as a miss of the exit group, and a tape cut at the close would turn every such near miss
/// into one.
pub const TAIL_MS: i64 = crate::market::trade_replay::MODEL_PAD_MS;

/// The part of a deal's window the model cannot do without: the run-up before the buy through
/// the tail after the close, clipped to what the window asks for at all (a long position asks
/// only around its two ends; a model's window is built with a margin of at least the pads, see
/// `trade_replay::model_margin_ms`). The rest of the
/// window — the trail beyond the tail, the lead beyond the run-up — is served as far as the
/// tape goes: a venue's page budget runs out on the trail of a pumped coin long before the
/// margin, and a variant that outlives the tape is marked open at the window's end.
///
/// Args:
///     deal: The deal, for its buy and close stamps.
///     spans: The window's focus spans, as asked from the worker.
///
/// Returns:
///     The spans the held coverage must include for the deal to count as covered.
pub fn required_spans(deal: &Deal, spans: &Coverage) -> Coverage {
    Coverage::one((
        deal.buy_ms.saturating_sub(RUN_UP_MS),
        deal.close_ms.saturating_add(TAIL_MS),
    ))
    .clip(spans)
}

/// Run one trade through the entry and the exit model.
///
/// `ticks` is the tape the caller fetched for the deal's window, ascending by time; it must
/// reach back before `deal.buy_ms` for the entry model to have a run-up, and past
/// `deal.close_ms` for the exit to have a tail ([`required_spans`] is the caller's gate). An
/// empty tape yields no fill.
///
/// Args:
///     deal: The report row and what the caller knows about its market.
///     ticks: Prints of the window, ascending.
///     entry: Entry model parameters, or the fact.
///     exit: Sell-line parameters.
///     entry_line: The archived points of the real entry line, when the order archive holds
///         it. The model then starts its order where the archive says it stood when the tape
///         begins, rather than at the window's first print — the one thing about the order's
///         history the tape cannot tell (see [`mshot`] for what else the line gives).
pub fn simulate(
    deal: &Deal,
    ticks: &[Tick],
    entry: &EntryParams,
    exit: &ExitParams,
    entry_line: Option<&[(i64, f64)]>,
) -> Outcome {
    let fill = match entry {
        EntryParams::Fact => Some(Fill {
            t_ms: deal.buy_ms,
            price: deal.buy_price,
        }),
        EntryParams::MoonShot(params) => MshotEntry::new(params).fill(deal, ticks, entry_line),
    };
    let Some(fill) = fill else {
        return Outcome {
            fill: None,
            exit: None,
            profit_pct: None,
        };
    };
    let exit_result = ExitModel::new(exit).exit(deal, ticks, fill);
    let profit_pct = match exit_result.kind {
        ExitKind::OpenAtWindowEnd => None,
        _ => profit_pct(deal, fill.price, exit_result.price),
    };
    Outcome {
        fill: Some(fill),
        exit: Some(exit_result),
        profit_pct,
    }
}

/// Signed result of a position in per cent of its entry, long or short. `None` for prices
/// that are not prices (non-positive or non-finite) — a degenerate placement is not a
/// break-even trade, it is no trade.
pub fn profit_pct(deal: &Deal, fill: f64, exit: f64) -> Option<f64> {
    if !fill.is_finite() || fill <= 0.0 || !exit.is_finite() || exit <= 0.0 {
        return None;
    }
    let raw = (exit - fill) / fill * 100.0;
    Some(if deal.is_short { -raw } else { raw })
}

/// The market's price step, read off the tape: the smallest positive distance between two
/// consecutive prints of different price. Over a window of hundreds of prints the two prices
/// one step apart are all but certain to appear; a window too thin to show it answers `None`,
/// and the caller then runs without a step rather than with a guessed one.
///
/// The result is snapped to a power of ten times 1, 2 or 5 — the grids exchanges actually use
/// — so `f32` noise on the prints does not become a step of `0.0099999`. The cut between two
/// grid steps is their geometric mean, the nearest step in ratio rather than in difference.
pub fn infer_tick(ticks: &[Tick]) -> Option<f64> {
    let mut best: Option<f64> = None;
    for pair in ticks.windows(2) {
        let d = (f64::from(pair[1].price) - f64::from(pair[0].price)).abs();
        if d > 0.0 && best.is_none_or(|b| d < b) {
            best = Some(d);
        }
    }
    let raw = best?;
    if !raw.is_finite() || raw <= 0.0 {
        return None;
    }
    let exp = raw.log10().floor();
    let base = 10f64.powf(exp);
    let mantissa = raw / base;
    let snapped = if mantissa < 2f64.sqrt() {
        1.0
    } else if mantissa < 10f64.sqrt() {
        2.0
    } else if mantissa < 50f64.sqrt() {
        5.0
    } else {
        10.0
    };
    Some(snapped * base)
}

#[cfg(test)]
mod tests;
