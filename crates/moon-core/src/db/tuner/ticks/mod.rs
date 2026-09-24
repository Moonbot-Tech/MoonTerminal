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
//! параметры у стратегии MoonShot" and the `PriceDown*` / `SellLevel*` answers),
//! checked against the live `strategies.sqlite` field names on 2026-09-20.

use crate::feed::types::Tick;
use crate::market::trade_replay::{Coverage, ReplayWindow, replay_window_ms};

pub mod calibrate;
pub mod deals;
pub mod deltas;
pub mod entry;
pub mod exit;
pub mod hook;
pub mod mshot;
pub mod params;
pub mod record;
pub mod scope;
pub mod search;
pub mod settings;
pub mod stats;
pub mod unmodelled;
pub mod verify;

pub use deals::{DealsRead, read_deals};
pub use entry::{EntryModel, entry_model_for};
pub use exit::sell_order::{archived_pre_spike_ask, archived_take, take_model_for};
pub use exit::{ExitModel, ExitParams};
pub use hook::{HookDetect, KIND_MOONHOOK, hook_take_pct, parse_hook_detect};
pub use mshot::{CorridorStep, EntryMethod, MshotEntry, MshotParams, UsePrice};
pub use params::{ParamGroup, ParamKind, TICK_PARAMS, TickParam};
pub use record::{OwnLines, StopAnchor, entry_placement, fit_for_search, prepare_deal};
pub use scope::{is_service_row, is_tunable};
pub use search::{
    PreparedDeal, SearchMiss, SearchParams, SearchResult, SearchStats, suggest, variant_tally,
};
pub use settings::ModelSettings;
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

/// The report-side deltas the delta modifiers read (`MShotAdd*` on the entry corridor, `Add*`
/// on the sell and the stop).
///
/// The report stamps them once — at the buy for MoonShot, at the detect and the order's
/// placement for every other kind (FAQ :423) — while the core re-reads them live. Every field of
/// [`deltas::DeltaField`] is re-evaluated along the window where the caller could build a
/// [`deltas::DeltaTrack`] for the deal ([`Deal::deltas_at`]); the rest ([`deltas::NotComputed`])
/// stay this snapshot. All values are per cent, exactly as `orders_rep` stores them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Deltas {
    /// The last five seconds' move (`d5s`) — read by `MShotAdd5sDelta`.
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
    /// Exchange-wide 1-hour delta (`exchange1hdelta`), signed.
    pub market1h: f64,
    /// Exchange-wide 24-hour delta (`exchange24hdelta`), signed — read by `AddMarket24Delta`.
    pub market24h: f64,
    /// The hour's rise from the price an hour ago to its high (`pump1h`) — read by `AddPump1h`.
    pub pump1h: f64,
    /// The hour's fall from the price an hour ago to its low (`dump1h`) — read by `AddDump1h`.
    pub dump1h: f64,
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
    /// `buysetdatems` — the moment the core CREATED the entry order, Unix ms on the same clock
    /// as `buy_ms`. Filed by cores since 2026-09-21 and never backfilled; `None` on older rows,
    /// on a zero, and on a stamp after the fill, which no order can have.
    pub buy_set_ms: Option<i64>,
    /// `buycorridordown` / `buycorridorup` — the entry corridor the core last saved, as absolute
    /// prices under their own names: `Down` is the edge a falling price crosses, `Up` the one a
    /// rising price crosses, whatever their numeric order. `None` unless both are prices. The
    /// model does not replay it — the core saves it once, at a moment the report does not name —
    /// but its width is the corridor's own, so the `real_data` bench holds the model's
    /// `MShotPriceMin` … `2 · MShotPrice − MShotPriceMin` band against it on every run.
    pub corridor: Option<(f64, f64)>,
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
    /// The report's snapshot of the deltas; read through [`Deal::deltas_at`], never directly by a
    /// model, so the live track applies wherever the deal has one.
    pub deltas: Deltas,
    /// The deltas along the window, as the core re-evaluated them — every field of
    /// [`deltas::DeltaField`] the history could give and the report filled
    /// ([`deltas::track_for`]); filled by the caller that holds the tape, before the other model
    /// inputs (the stop anchor reads the stop through it). `None` keeps the snapshot everywhere.
    pub delta_track: Option<std::sync::Arc<deltas::DeltaTrack>>,
    /// The market's price bars — one-minute klines, five-minute ones where no minute bar lies —
    /// from [`deltas::PRICE_HISTORY_MS`] before the window through the tape's end
    /// ([`deltas::track_for`]): what a rule that looks back further than the tape reads, as
    /// SellLevel's `SellLevelTime` does. `None` without a kline cache; the rule then reads the tape
    /// alone.
    pub bars: Option<std::sync::Arc<[deltas::Bar]>>,
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
    /// The level the entry order stood at when the core created it — where a replay from the
    /// creation places the fact's order ([`record::entry_placement`]); filled with the rest of
    /// the model inputs, `None` before them and wherever the record does not prove it.
    pub entry_placed: Option<f64>,
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
    /// What the fact proves about the stop, for a variant that runs the same one
    /// ([`record::StopAnchor`]); filled with the rest of the model inputs, `None` before.
    pub stop_anchor: Option<StopAnchor>,
    /// The entry parameters the trade itself ran with. A variant that runs exactly these has
    /// nothing to model on the entry side: the fill is the report's ([`simulate`]). `None`
    /// until the model inputs are filled, and in the verdict, which exists to test the model.
    pub own_entry: Option<EntryParams>,
}

impl Deal {
    /// Whether the position is long; the mirror of every level rule keys off this.
    pub fn is_long(&self) -> bool {
        !self.is_short
    }

    /// When the entry order's life began, for a model that replays it whole ([`order_open_at`]).
    pub fn order_open_ms(&self) -> Option<i64> {
        order_open_at(self.buy_ms, self.buy_set_ms)
    }

    /// The deltas the entry order lived through, one per refresh step of the live track
    /// ([`deltas::STEP_MS`]) from the order's creation ([`Deal::order_open_ms`]) to the fill,
    /// the fill's own included — the core re-places the corridor when a delta moves, so these
    /// are every corridor it held as the model reads them: where the track does not reach, the
    /// report's snapshot, as `deltas_at` gives it to the replay itself. A deal without a track
    /// has the snapshot alone, and
    /// one whose order waited past [`ORDER_WAIT_CAP_MS`] the fill's alone: its early life is
    /// neither fetched nor replayed, so nothing of it is held against a variant either.
    pub fn entry_deltas(&self) -> Vec<Deltas> {
        if self.delta_track.is_none() {
            return vec![self.deltas];
        }
        let to = self.buy_ms;
        let from = self.order_open_ms().unwrap_or(to);
        let mut out: Vec<Deltas> = (from..to)
            .step_by(deltas::STEP_MS as usize)
            .map(|t| self.deltas_at(t))
            .collect();
        out.push(self.deltas_at(to));
        out
    }

    /// The deltas as the core held them at a moment: every field the live track answers for
    /// there, where the deal has one, the report's snapshot for everything else.
    pub fn deltas_at(&self, t_ms: i64) -> Deltas {
        match &self.delta_track {
            Some(track) => track.apply(t_ms, &self.deltas),
            None => self.deltas,
        }
    }
}

/// When an entry order's life began, for a model that replays it whole: its creation stamp,
/// unless the order waited longer than [`ORDER_WAIT_CAP_MS`] — then its early life is not
/// fetched and the model starts where the tape does. One rule for every kind of the tuner, the
/// ones without an entry model too: their tape is fetched and kept for a model to come.
///
/// Args:
///     buy_ms: The fill of the entry.
///     buy_set_ms: The order's creation, on the same clock (`buysetdatems`).
pub fn order_open_at(buy_ms: i64, buy_set_ms: Option<i64>) -> Option<i64> {
    buy_set_ms.filter(|&set| set <= buy_ms && buy_ms - set <= ORDER_WAIT_CAP_MS)
}

/// The longest wait of an entry order the tape is fetched for, from its creation to its fill.
/// MoonShot orders on this machine's reports (2026-09-23, 289 with a creation stamp) waited a
/// median 114 s, 280 s at the 90th percentile and hours at the 99th; the cap keeps the few that
/// wait for hours from asking the venue for hours of prints, and they replay as before.
pub const ORDER_WAIT_CAP_MS: i64 = 10 * 60_000;

/// The replay window of a deal as the model needs it ([`model_window_at`] on the deal's own
/// stamps).
pub fn model_window(deal: &Deal, margin_ms: i64, long_position_ms: i64) -> Option<ReplayWindow> {
    model_window_at(
        deal.order_open_ms(),
        deal.buy_ms,
        deal.close_ms,
        margin_ms,
        long_position_ms,
    )
}

/// The replay window of a trade as the model needs it: from the entry order's creation
/// ([`order_open_at`]) through the close, one stretch. Where that stretch would be walked as its
/// two ends — the order's life plus the position outrun `long_position_ms` — the window opens at
/// the fill as before: an entry end around the creation would leave the fill itself between
/// the ends, where nothing is fetched. The tape cleanup claims by this same rule
/// (`trades_cleanup`), so what the tuner fetched is what it keeps.
///
/// Args:
///     order_open_ms: The order's creation where a replay may start there ([`order_open_at`]).
///     buy_ms: The fill of the entry.
///     close_ms: The close.
///     margin_ms: The margin setting (`trade_replay::margin_ms`).
///     long_position_ms: The threshold the window is split by — the caller's, so every stage
///         of one row splits it the same way.
///
/// Returns:
///     The window, or `None` when the stamps describe none.
pub fn model_window_at(
    order_open_ms: Option<i64>,
    buy_ms: i64,
    close_ms: i64,
    margin_ms: i64,
    long_position_ms: i64,
) -> Option<ReplayWindow> {
    let with_threshold = |window: ReplayWindow| ReplayWindow {
        long_position_ms,
        ..window
    };
    let from_creation = order_open_ms
        .and_then(|open| replay_window_ms(open, close_ms, margin_ms))
        .map(with_threshold)
        .filter(|w| w.close_ms - w.open_ms <= w.long_position_ms);
    from_creation.or_else(|| replay_window_ms(buy_ms, close_ms, margin_ms).map(with_threshold))
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
    /// A print crossed the moving sell line (PriceDown / SellLevel / PumpMove).
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
///
/// The MoonShot variant is the large one (its parameters carry the model's settings); left
/// unboxed on purpose — one lives per deal (`Deal::own_entry`) and one per scored point, never
/// in a collection large enough for the size to matter, and a box would put an allocation on
/// every point of the search.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum EntryParams {
    /// Take the entry as the report has it — for kinds without a model, and for a variant
    /// whose "Entry" group is not being varied.
    Fact,
    MoonShot(MshotParams),
}

/// The tape must reach this far back before the window's open — the entry order's creation, or
/// the buy — for the corridor to have a run-up. The replay worker walks exactly this much as
/// part of the trade for a model's request (`trade_replay::MODEL_PAD_MS`), so the two are one
/// number.
pub const RUN_UP_MS: i64 = crate::market::trade_replay::MODEL_PAD_MS;

/// The tape must reach this far past the close for the exit to have a tail: a line the fact
/// crossed at the close is reproduced by a print at or before it, but a near miss a few seconds
/// later must be judged on its price — [`verify`] counts a model still open when the tape ends
/// as a miss of the exit group, and a tape cut at the close would turn every such near miss
/// into one.
pub const TAIL_MS: i64 = crate::market::trade_replay::MODEL_PAD_MS;

/// The part of a deal's window the model cannot do without: the run-up before the window's open
/// — the entry order's creation where [`model_window`] opened there, else the buy — through the
/// tail after the close, clipped to what the window asks for at all — a long position asks only
/// around its two ends, and of each end the model is owed the pads, not the margin. The margin
/// on both sides of a long position's end is the window's context: owed in full, a wider setting
/// turned a trade the model already had into a missing one, and one past the venue's retention
/// could never be covered again (2026-09-23). Read off the window rather than the deal: the
/// worker walks the pads around the window's own open as part of the trade, and a requirement
/// reaching past them would name prints nobody was sure to fetch. The rest of the window — the
/// trail beyond the tail, the lead beyond the run-up — is served as far as the tape goes: a
/// venue's page budget runs out on the trail of a pumped coin long before the margin, and a
/// variant that outlives the tape is marked open at the window's end.
///
/// Args:
///     window: The deal's window, as asked from the worker.
///
/// Returns:
///     The spans the held coverage must include for the deal to count as covered.
pub fn required_spans(window: &ReplayWindow) -> Coverage {
    let pads = ReplayWindow {
        margin_ms: window.margin_ms.min(RUN_UP_MS.max(TAIL_MS)),
        ..*window
    };
    Coverage::one((
        window.open_ms.saturating_sub(RUN_UP_MS),
        window.close_ms.saturating_add(TAIL_MS),
    ))
    .clip(&pads.focus_spans())
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
    let fact_fill = Fill {
        t_ms: deal.buy_ms,
        price: deal.buy_price,
    };
    let fill = match entry {
        EntryParams::Fact => Some(fact_fill),
        // The trade's own entry settings filled where the report says, whichever way and with
        // whatever latency a variant would be replayed — both are the model's, not the
        // strategy's; the models are for the entries the core never ran. (The verdict replays
        // the own settings through the model on purpose, and does it with `own_entry` cleared.)
        EntryParams::MoonShot(params)
            if matches!(
                deal.own_entry.as_ref(),
                Some(EntryParams::MoonShot(own)) if own.same_strategy(params)
            ) =>
        {
            Some(fact_fill)
        }
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
