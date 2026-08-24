mod arb;
mod archive;
mod history;
#[cfg(test)]
mod label_tests;
mod read;
mod refresh;
#[cfg(test)]
mod tests;

pub use read::{ReplayAddress, ReplayAddressError};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use moonproto::MoonTime;
use moonproto::state::{
    LastPricePoint, MarkPricePoint, OrderBookKind, SeqRingCursor, SeqRingPriceRow, SeqRingReader,
    SeqRingTimedRow, TradeHistoryRow,
};

use super::candles::{CandleSeries, ChartCandle};
use crate::feed::{MarketDirtyFlags, PricePoint, SharedMoonClient, Side, Tick};
use crate::session::CoreId;

use super::SharedMarketStore;

const ORDERBOOK_PULL_PERIOD_MS: u64 = 200;

/// Default gap between two trace lines about the same subject, from
/// `limits.market_trace_min_interval_ms`. A function rather than a constant because the value is
/// live: the order-book pull runs five times a second, so this floor is what decides whether the
/// channel is readable, and it has to be adjustable without a rebuild.
fn market_diag_floor() -> Duration {
    crate::diagnostics::market_trace_min_interval()
}

/// Level for market-source tracing, admitted by `log.market_sources` in `cfg/diagnostics.toml`.
///
/// Debug, so the default filter (info and above) excludes it. At info these lines followed cursor
/// movement — a hovering cursor re-reads sources — and produced ~2600 lines a day on their own,
/// against a Log panel ring that holds a few thousand.
pub(super) const SOURCE_TRACE_LEVEL: log::Level = log::Level::Debug;

/// Whether the market channel is on, from `channels.markets` in `cfg/diagnostics.toml`.
///
/// `MOON_MARKET_DIAG` and `MOON_RENDER_DIAG` both still enable it; that pairing is preserved in
/// `diagnostics::config::apply_env` rather than restated here.
fn market_diag_enabled() -> bool {
    crate::diagnostics::markets()
}

fn market_diag_due(key: impl Into<String>, floor: Duration) -> bool {
    if !market_diag_enabled() {
        return false;
    }
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let key = key.into();
    let now = Instant::now();
    let mut last = LAST
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("market diag lock poisoned");
    match last.get(&key).copied() {
        Some(prev) if now.duration_since(prev) < floor => false,
        _ => {
            last.insert(key, now);
            true
        }
    }
}

fn market_diag(msg: impl std::fmt::Display) {
    if market_diag_enabled() {
        log::info!("[market_diag] {msg}");
    }
}

fn bump_generation(revisions: &mut HashMap<CoreId, u64>, provider: CoreId) {
    let entry = revisions.entry(provider).or_insert(0);
    *entry = entry.wrapping_add(1);
}

fn bump_market_revisions(
    revisions: &mut HashMap<CoreId, HashMap<String, MarketRevisionCounters>>,
    provider: CoreId,
    market: &str,
    flags: MarketDirtyFlags,
) {
    let entry = revisions
        .entry(provider)
        .or_default()
        .entry(market.to_string())
        .or_default();
    if flags.contains(MarketDirtyFlags::HISTORY) {
        entry.history = entry.history.wrapping_add(1);
    }
    if flags.contains(MarketDirtyFlags::ORDERBOOK) {
        entry.book = entry.book.wrapping_add(1);
    }
    if flags.contains(MarketDirtyFlags::MARKET_META) {
        entry.meta = entry.meta.wrapping_add(1);
    }
    if flags.contains(MarketDirtyFlags::HISTORY_ARCHIVE) {
        entry.archive = entry.archive.wrapping_add(1);
    }
}

fn mix_pair(a: u64, b: u64) -> u64 {
    a.wrapping_mul(0x9e37_79b1_85eb_ca87).rotate_left(17) ^ b
}

#[derive(Default)]
struct MarketPullCursor {
    book_phase_ms: Option<u64>,
    last_book_slot: Option<u64>,
    last_book_dirty_revision: u64,
    last_book_revision: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MarketRevisionCounters {
    history: u64,
    book: u64,
    meta: u64,
    archive: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarketRevisions {
    pub provider: CoreId,
    pub generation: u64,
    pub history: u64,
    pub book: u64,
    pub meta: u64,
    /// Bumps once per merged core chart archive for this market.
    ///
    /// A consumer that keeps a cursor must FULLY re-read its window when this changes: the
    /// archive prepends rows older than the cursor, which no incremental drain can reach.
    pub archive: u64,
}

impl MarketRevisions {
    pub fn combined_signature(self) -> u64 {
        let mut sig = 0xcbf29ce4_84222325u64;
        sig = mix_pair(sig, self.provider);
        sig = mix_pair(sig, self.generation);
        sig = mix_pair(sig, self.history);
        sig = mix_pair(sig, self.book);
        sig = mix_pair(sig, self.meta);
        mix_pair(sig, self.archive)
    }
}

/// Market price snapshot for the header ticker: last price and signed percentage deltas.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketTickerReadout {
    pub last: f64,
    pub delta_1h_pct: f64,
    pub delta_24h_pct: f64,
}

/// Where a market's maximum order size came from.
///
/// The two non-absent cases behave DIFFERENTLY in front of a user and must stay distinguishable:
/// a stated cap is a fixed exchange figure, while a derived one is recomputed from the current ask
/// and therefore drifts as the price moves. A readout that cannot tell them apart either explains
/// nothing or explains the wrong thing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaxOrderSource {
    /// The exchange stated a notional cap directly.
    Stated,
    /// No notional cap was stated, so the quantity cap was converted into quote currency.
    Derived,
    /// A quantity cap EXISTS but what it takes to convert it has not arrived — on a linear market
    /// that is the ask price, which is zero until the market's first price update lands.
    ///
    /// Distinct from [`Self::Absent`] and never merged with it: this is "not known yet", while
    /// `Absent` is "the exchange says there is no cap". Telling an operator a market has no maximum
    /// order size when the truth is that its price has not loaded is the exact misstatement the
    /// two-level unknown in [`MarketLimits`] exists to prevent.
    Pending,
    /// The exchange stated no cap of either kind.
    #[default]
    Absent,
}

/// A market's maximum order size together with how it was obtained.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MaxOrder {
    /// The cap in the quote currency; `0.0` while its source is pending or absent.
    pub value: f64,
    /// Provenance of `value`, so a readout can say WHY the figure moves — or why there is none.
    pub source: MaxOrderSource,
}

/// One market's exchange-imposed trading limits, for the leverage control.
///
/// Every field carries its own "unknown", and those unknowns are DIFFERENT facts the UI states
/// differently: an absent [`MarketDataSource::market_limits`] result means no provider, snapshot or
/// market has arrived yet, while [`MaxOrderSource::Absent`] or a zero `max_leverage` here means the
/// exchange itself stated no cap (or the market is spot, which has no leverage). Collapsing the two
/// would tell an operator "this coin has no limit" when the truth is "nothing has loaded".
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketLimits {
    /// Exchange maximum order size in the quote currency, with its provenance.
    ///
    /// FLAT: it does not vary with the selected leverage and is the same figure at x1 and at x50.
    pub max_order: MaxOrder,
    /// Maximum market leverage; `0` means spot or unknown.
    pub max_leverage: i32,
}

/// The exchange maximum order size in the quote currency, from the figures a market carries.
///
/// The stated notional cap wins. Only when the exchange gave none is the QUANTITY cap converted,
/// and that conversion is not one formula but two, because a quantity does not mean the same thing
/// on every market:
///
/// - **Inverse (coin-margined) futures report quantity in CONTRACTS**, each worth a fixed amount of
///   quote currency (`contract_size` — BTCUSD is $100, other `*USD` contracts $10). The notional is
///   therefore `max_qty * contract_size` and needs no price at all. `feed/live/convert.rs` derives
///   position sizes from the same fact; multiplying a contract COUNT by a coin PRICE instead would
///   be off by roughly the contract size.
/// - **Linear markets report quantity in the base coin**, so the notional is `max_qty * ask`. This
///   is why that fallback MOVES with the price while a stated cap stands still — a distinction the
///   returned [`MaxOrderSource`] preserves so the UI can say so instead of hiding it.
///
/// An EMPTY QUOTE in the market name is what separates the two, NOT `contract_size != 1` on its
/// own: linear QUANTO futures also carry a contract size (Gate's `ASTEROID_USDT` has 10000) while
/// still reporting quantity in coins. Same guard, same reason, as `convert.rs`. That test is made
/// HERE, from the market name and its exchange, rather than by each caller: it is half of this one
/// rule — it chooses which formula applies — and a copy of it at every call site is a second place
/// to get coin-margined detection wrong.
///
/// A quantity cap whose conversion cannot be completed yet — a linear market before its first price
/// tick, where `ask` is still zero — is [`MaxOrderSource::Pending`], NOT `Absent`. Only a market
/// stating neither cap is `Absent`. Both render as a dash, and they explain themselves differently.
///
/// Takes the market's FIGURES as primitives rather than the market itself, because `moonproto`'s
/// `Market` is not re-exported at its crate root and cannot be named here. Both readers of the rule
/// — `screener_rows` and [`MarketDataSource::market_limits`] — go through this function, so the
/// Screener's `Max.Order` column and the trading toolbar can never print two different caps for one
/// coin.
///
/// Args:
///     market: Canonical market name used to distinguish linear from inverse contracts.
///     exchange: Market exchange used when resolving the market's quote token.
///     max_notional: Exchange-stated maximum order size in quote currency, when available.
///     max_qty: Exchange-stated maximum quantity, used only when no notional cap exists.
///     ask: Current best ask used to convert a linear-market quantity cap.
///     contract_size: Fixed quote-currency value of one inverse contract.
///
/// Returns:
///     A stated, derived, pending, or absent quote-currency cap with its provenance.
pub(crate) fn max_order_notional(
    market: &str,
    exchange: crate::symbol::Exchange,
    max_notional: f64,
    max_qty: f64,
    ask: f64,
    contract_size: f64,
) -> MaxOrder {
    if max_notional.is_finite() && max_notional > 0.0 {
        return MaxOrder {
            value: max_notional,
            source: MaxOrderSource::Stated,
        };
    }
    if !max_qty.is_finite() || max_qty <= 0.0 {
        return MaxOrder::default();
    }
    let inverse = crate::symbol::resolve_quote_on(market, exchange).is_empty()
        && contract_size.is_finite()
        && contract_size > 0.0
        && contract_size != 1.0;
    let value = if inverse {
        max_qty * contract_size
    } else {
        max_qty * ask
    };
    if !value.is_finite() || value <= 0.0 {
        // The cap exists; what converts it does not yet. Never report that as "no cap".
        return MaxOrder {
            value: 0.0,
            source: MaxOrderSource::Pending,
        };
    }
    MaxOrder {
        value,
        source: MaxOrderSource::Derived,
    }
}

/// Market-wide context a chart caption can state beside the coin's own numbers.
///
/// Two different subjects on purpose. The BACKGROUND deltas — the exchange's own average and BTC's
/// — answer "is this the coin or the whole market"; the funding pair answers "what does holding
/// cost, and when is it charged". Both come from one snapshot read, because a caption asking for
/// either has already paid for it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketContextReadout {
    /// Signed average movement across the exchange's markets, in percent.
    pub exchange_1h_pct: f64,
    pub exchange_24h_pct: f64,
    /// Signed BTC movement over the retained windows, in percent.
    pub btc_1h_pct: f64,
    pub btc_24h_pct: f64,
    pub btc_72h_pct: f64,
    /// Funding rate as a percentage, or `None` on a market that has none (spot).
    pub funding_pct: Option<f64>,
    /// When funding is next charged, in Unix milliseconds. `None` when the core reports no time —
    /// spot markets, and futures before the first funding message arrives.
    pub funding_at_ms: Option<i64>,
}

/// Exchange tag a coin carries, as the venue itself classifies it.
///
/// The names are the EXCHANGE's own labels, not prose, so they are printed verbatim in every locale
/// — "Seed" and "Alpha" are what the venue calls those listings and what its own interface shows.
/// Translating them would leave the caption saying something no exchange page repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoinTag {
    Monitoring,
    Fan,
    Seed,
    Launch,
    Gaming,
    New,
    Old,
    Bnb,
    Alpha,
    OiCapped,
    TradFi,
}

impl CoinTag {
    /// Tags in the order a caption prints them, with the bit each one occupies on the wire.
    ///
    /// Bit 0 is the venue's "no tag" marker and carries no tag of its own, which is why the table
    /// starts at bit 1 — a coin with only that bit set prints nothing.
    const BITS: [(CoinTag, u32); 11] = [
        (CoinTag::Monitoring, 1 << 1),
        (CoinTag::Fan, 1 << 2),
        (CoinTag::Seed, 1 << 3),
        (CoinTag::Launch, 1 << 4),
        (CoinTag::Gaming, 1 << 5),
        (CoinTag::New, 1 << 6),
        (CoinTag::Old, 1 << 7),
        (CoinTag::Bnb, 1 << 8),
        (CoinTag::Alpha, 1 << 9),
        (CoinTag::OiCapped, 1 << 10),
        (CoinTag::TradFi, 1 << 11),
    ];

    pub fn name(self) -> &'static str {
        match self {
            CoinTag::Monitoring => "Monitoring",
            CoinTag::Fan => "Fan",
            CoinTag::Seed => "Seed",
            CoinTag::Launch => "Launch",
            CoinTag::Gaming => "Gaming",
            CoinTag::New => "New",
            CoinTag::Old => "Old",
            CoinTag::Bnb => "BNB",
            CoinTag::Alpha => "Alpha",
            CoinTag::OiCapped => "OI-capped",
            CoinTag::TradFi => "TradFi",
        }
    }

    /// Every tag the given wire bits carry, in print order.
    pub fn from_bits(bits: u32) -> Vec<CoinTag> {
        Self::BITS
            .iter()
            .filter(|(_, bit)| bits & bit != 0)
            .map(|(tag, _)| *tag)
            .collect()
    }
}

/// A venue the core watches for arbitrage against the market being charted.
///
/// The core reports a numeric PLATFORM CODE, not a name: the codes are Moonbot's own
/// `TBotPlatform` ordinals plus arbitrage-only ones, and nothing on the wire says how to spell
/// them. Which codes exist and how each is spelled is the venue directory's answer —
/// [`crate::venue::ARB_VENUES`] — so an exchange is described in ONE place whether it is asked
/// about as a connection or as a price to compare against. This type is the code itself: the
/// deployer arithmetic the directory has no opinion on, and the fallbacks for a code it cannot
/// name.
///
/// A Hyperliquid DEPLOYER carries only an index in its arbitrage slot, and the directory has no
/// entry to give it, so [`Self::default_name`] numbers it here instead.
/// The real name usually exists elsewhere — `AuthCheck` hands over `known_dexes`, and the same
/// index reads into that list — and a live quote carries it (see [`ArbQuote::dex_name`]); the
/// numbered form is what remains when a core sent no list at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArbVenue(u8);

impl ArbVenue {
    /// Every venue this build can name, in the order the settings window lists them.
    ///
    /// The codes and the order are [`crate::venue::ARB_VENUES`]'s, not a second list beside it:
    /// this used to be one array of codes and [`Self::default_name`] a second one of the same
    /// codes, which nothing checked against each other — a venue added to one and forgotten in the
    /// other printed as a bare number in the column while the settings window offered it a colour.
    ///
    /// Deployers are deliberately absent: they exist per core, are discovered from the data, and
    /// are appended after this list wherever one actually reports a price.
    pub const KNOWN: [ArbVenue; crate::venue::ARB_VENUES.len()] = {
        let mut out = [ArbVenue(0); crate::venue::ARB_VENUES.len()];
        let mut i = 0;
        while i < out.len() {
            out[i] = ArbVenue(crate::venue::ARB_VENUES[i].0);
            i += 1;
        }
        out
    };

    /// First deployer code; everything from here to [`Self::DEPLOYER_END`] is one.
    pub const DEPLOYER_BASE: u8 = 50;
    const DEPLOYER_END: u8 = 100;

    /// How many deployer indices a read actually asks about.
    ///
    /// The protocol reserves fifty codes for them; a core watches a handful. Every candidate costs
    /// a market-lock round trip on a read (see [`super::MarketDataSource::market_arb`]), so the
    /// scan stops where the reference terminal's own column does rather than paying for forty-two
    /// venues nobody deploys.
    pub const DEPLOYERS_SCANNED: u8 = 8;

    /// The deployer at `index`, as a venue.
    pub const fn deployer(index: u8) -> Self {
        Self(Self::DEPLOYER_BASE.wrapping_add(index))
    }

    /// This deployer's index, or `None` for anything else.
    pub const fn deployer_index(self) -> Option<u8> {
        match self.is_deployer() {
            true => Some(self.0 - Self::DEPLOYER_BASE),
            false => None,
        }
    }

    /// Whether this build would ask about the venue with no core settings to go by.
    ///
    /// The FALLBACK roster, used only until `client_settings` arrives: everything this build can
    /// name, plus the deployer indices it scans. With settings in hand the core's own mask decides
    /// instead, and it can name venues this list cannot.
    pub fn is_known_or_scanned_deployer(self) -> bool {
        Self::KNOWN.contains(&self)
            || self
                .deployer_index()
                .is_some_and(|index| index < Self::DEPLOYERS_SCANNED)
    }

    pub const fn from_code(code: u8) -> Self {
        Self(code)
    }

    pub const fn code(self) -> u8 {
        self.0
    }

    pub const fn is_deployer(self) -> bool {
        self.0 >= Self::DEPLOYER_BASE && self.0 < Self::DEPLOYER_END
    }

    /// What this venue is CALLED, in the REFERENCE TERMINAL's spelling.
    ///
    /// The spelling itself lives in the venue directory — [`crate::venue::arb_alias`] — beside the
    /// brand, market kind and logo the same code already answers for. Here is only what to do when
    /// the directory has no word for it.
    ///
    /// The one name that does come over the wire is a Hyperliquid deployer's: `AuthCheck` carries
    /// `known_dexes`, and Moonbot prints those with an `HL_` prefix (`HL_hyna`, `HL_para`). That is
    /// handled where the live quote is — see `ArbVenueCfg::label_for` — and this is the fallback
    /// when no list has arrived.
    ///
    /// A code no spelling covers prints its NUMBER: it says "the core sent a platform this build
    /// has never seen" plainly, and the number is what identifies it.
    pub fn default_name(self) -> String {
        if let Some(name) = crate::venue::arb_alias(self.0) {
            return name.to_string();
        }
        if self.is_deployer() {
            return format!("HL #{}", self.0 - Self::DEPLOYER_BASE);
        }
        format!("#{}", self.0)
    }

    /// A deployer's name as the reference terminal prints it: its DEX name behind an `HL_` prefix.
    ///
    /// The prefix is the terminal's, the word after it is the core's — which is why this is one
    /// function and not a format string repeated at every call site.
    pub fn hl_name(dex_name: &str) -> String {
        format!("HL_{dex_name}")
    }
}

/// One venue's price on the charted coin, against the price of the market being charted.
#[derive(Clone, Debug, PartialEq)]
pub struct ArbQuote {
    pub venue: ArbVenue,
    /// The venue's own name, when the CORE supplies one.
    ///
    /// Hyperliquid deployers are the case this exists for: the arbitrage slot carries an index and
    /// the index alone, but `AuthCheck` hands over `known_dexes` — the deployer names the reference
    /// terminal shows as `HL_hyna`, `HL_para`. Empty for every other venue, whose name this build
    /// spells itself, and empty for a deployer whose core sent no list.
    pub dex_name: String,
    /// The other venue's price.
    pub price: f64,
    /// The CHARTED market's own price at the moment that one was recorded.
    ///
    /// Taken from the same ring entry rather than from the live ticker, so the percentage below
    /// compares two prices that existed at the same instant. A spread computed against a price that
    /// has moved since is the classic way to see arbitrage that was never there.
    pub my_price: f64,
    /// How far the other venue is from this one, in percent of this one's price.
    pub spread_pct: f64,
    /// Whether the venue is not accepting deposits or withdrawals for this coin — an arbitrage that
    /// cannot be settled. Reported by the core alongside the price.
    pub deposit_blocked: bool,
    pub withdraw_blocked: bool,
}

/// One retained-history window, in the figures a caption can print from it.
///
/// Both figures are `Option` for the same reason [`MarketContextReadout`]'s funding is: a coin that
/// has not traded in the window and a coin whose history has not arrived both produce zero, and a
/// caption that printed it would claim a quiet market rather than an unknown one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowFigures {
    /// Price movement over the window, in percent. UNSIGNED — this is the range magnitude the
    /// Screener's `Δ` columns show, not a signed change from an average.
    pub delta_pct: Option<f64>,
    /// Traded volume over the window, in the market's quote currency.
    pub volume_quote: Option<f64>,
    /// Share of that volume that was BUYING, in percent.
    ///
    /// Only the short windows have it: the buy/sell split comes from the retained trade buckets,
    /// which cover five minutes, while the longer windows are built from candles that carry no
    /// split at all.
    pub buy_share_pct: Option<f64>,
}

/// Retained-history figures for every window a caption may ask for.
///
/// Indexed by the window's position in [`crate::config::LabelWindow::ALL`], which is the ONE order
/// the two crates agree on; the readout carries no window names of its own so the config stays the
/// single place a window is spelled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketWindowsReadout {
    pub windows: [WindowFigures; crate::config::LABEL_WINDOW_COUNT],
}

/// Per-market figures a caption can print beside the price: the quote side, what the venue says
/// about the market, and what THIS core holds in it.
///
/// Two sources in one value, deliberately. The market half comes from the deduplicated PROVIDER —
/// the ask on `BTCUSDT@Binance` is the same for every core on that exchange — while the position
/// half comes from the core the pane is actually looking at, because a position is an account fact.
/// Reading them separately at the call site is what let the Screener's overlay drift from its
/// market columns; here one readout answers both.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarketFiguresReadout {
    /// Best bid and ask; `None` until the book has arrived.
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    /// Exchange mark price, `None` when the venue reports none (spot).
    pub mark: Option<f64>,
    /// Absolute chart price step.
    pub price_step: Option<f64>,
    /// 24-hour volume from the venue's own market list, in the quote currency.
    pub vol_24h: Option<f64>,
    /// Maximum leverage the market allows; `None` on spot or before it arrives.
    pub max_leverage: Option<i32>,
    /// Exchange maximum order size in the quote currency, through the shared rule.
    pub max_order: MaxOrder,
    /// The venue's own tags for this coin, in print order. Empty when none arrived.
    pub tags: Vec<CoinTag>,
    /// Open position on THIS core, in the base coin; negative while short.
    pub pos_size: Option<f64>,
    /// Liquidation price the venue reports for it.
    pub liq_price: Option<f64>,
    /// Account leverage in force on this market; `None` when unset.
    pub leverage_x: Option<i32>,
    /// Whether margin is isolated; `None` when the venue stated no position type.
    pub isolated: Option<bool>,
    /// The core's own per-coin profit counter (`b + l + s`), which MoonBot prints as `PnL`.
    ///
    /// NOT the `Session` figure beside it in MoonBot's chart header: that one is an accumulator the
    /// bot keeps for itself and never puts on the wire. Zero on part of the venues even where
    /// MoonBot shows an amount, so a reader must not take a zero here for "traded to break even".
    pub session_pnl: Option<f64>,
    /// Free balance of the coin itself, for a spot market.
    pub coin_balance: Option<f64>,
}

/// Frozen snapshot for a detection card, built exactly once when the detection occurs.
///
/// The mini-chart combines recent 5-minute candles from the provider's retained
/// `candles_5m`, the local kline cache, and the live trade-ring tail without calling the exchange
/// API. `server_info` supplies the connection identity. Missing history leaves the chart data
/// empty while exchange metadata may still be present; a missing provider, client, or client
/// snapshot returns the fully empty default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetectSnapshot {
    /// Recent 5-minute candles as `(open, high, low, close)`, ordered oldest to newest.
    ///
    /// Full OHLC lets the mini-chart draw bodies and wicks. The retained `candles_5m` fallback is
    /// range-only, so its candle orientation is synthesized. Empty when no history is available.
    pub bars: Vec<(f32, f32, f32, f32)>,
    /// Close prices for line mode, ordered oldest to newest, with up to about 24 hours of history.
    pub line: Vec<f32>,
    /// Actual 24-hour price change in percent, comparing now with a close from about 24 hours ago.
    ///
    /// This is derived from our buckets so it matches the line movement. It is not MoonProto's
    /// `coin_24h_delta`, which measures deviation from a retained average.
    pub delta_24h: f32,
    /// Actual 1-hour price change in percent, comparing now with a close from about one hour ago.
    pub delta_1h: f32,
    /// Venue the detecting core's provider is connected to, frozen with the rest of the card.
    ///
    /// The card captions this through the venue directory like every other core list, so a
    /// detection on Binance COIN-M reads as the same venue the Orders picker shows. `None` when the
    /// provider reported no identity this build can name.
    pub venue: Option<crate::venue::CoreVenue>,
    /// Short Russian-language exchange type label derived from `exchange_type_mask`.
    ///
    /// The label distinguishes spot, futures, DEX, and combined connections. Empty when the type
    /// was not reported.
    pub exchange_kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LatestPriceError {
    NoProvider,
    NoClient,
    NoSnapshot,
    NoHistoryReaders,
    NoPrice,
}

impl std::fmt::Display for LatestPriceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProvider => f.write_str("no provider"),
            Self::NoClient => f.write_str("no client"),
            Self::NoSnapshot => f.write_str("no snapshot"),
            Self::NoHistoryReaders => f.write_str("no history readers"),
            Self::NoPrice => f.write_str("no price"),
        }
    }
}

#[derive(Default)]
pub struct ChartHistoryCursor {
    trades: Option<SeqRingCursor>,
    liquidations: Option<SeqRingCursor>,
    last_prices: Option<SeqRingCursor>,
    mark_prices: Option<SeqRingCursor>,
    last_price: Option<f32>,
    trade_rows: Vec<TradeHistoryRow>,
    scan_trade_rows: Vec<TradeHistoryRow>,
    liq_rows: Vec<TradeHistoryRow>,
    last_price_rows: Vec<LastPricePoint>,
    mark_price_rows: Vec<MarkPricePoint>,
    /// Merged candle series consisting of the server base and a local tail built from trades.
    /// Uses its own trade-ring cursor so aggregation is independent of the cross display range.
    candle_series: CandleSeries,
    candle_trades: Option<SeqRingCursor>,
    candle_trade_rows: Vec<TradeHistoryRow>,
    candle_ticks: Vec<Tick>,
    server_candle_rows: Vec<moonproto::state::Candle5mRow>,
    server_candles: Vec<ChartCandle>,
    /// Throttle for non-blocking CoinCard deep-history requests that provide authoritative OHLC.
    last_deep_request: Option<Instant>,
    /// Most recently requested kind; a timeframe change bypasses the throttle.
    last_deep_kind: Option<moonproto::DeepHistoryKind>,
    /// Coin-card retry backoff in seconds, where zero starts at 30 seconds.
    ///
    /// The core fetches deep history from the exchange API and consumes request weight. A retry
    /// without progress doubles the delay up to 10 minutes, while new rows reset it. Without this
    /// backoff, a stalled core or exchange would receive a request storm from every open chart and
    /// trigger the core's API-limit auto-stop.
    deep_retry_delay_s: u32,
    /// Prefix loaded from the local kline cache using the panel's native kind.
    ///
    /// It is read from SQLite once per `(market, kind, left edge)` and survives series resets.
    /// Resets are frequent during pan and zoom, so each reset must not query the database.
    cache_rows: Vec<ChartCandle>,
    cache_kind: Option<u32>,
    /// Actual kind of the loaded rows: the panel's native kind or a fallback.
    ///
    /// Fallback order is the recorder's 5-minute rows followed by 1-minute deep-history rows.
    cache_rows_kind: u32,
    cache_from_ms: i64,
    /// Cache-only coarser layers used to extend the historical prefix as far back as possible.
    ///
    /// The 5-minute layer contains kind-5 cache rows from the recorder and possible deep-history
    /// writeback; the retained 5-minute snapshot separately feeds the main series through
    /// `snap_part`. The 1-day layer comes from backfill and cache.
    cache_rows_5m: Vec<ChartCandle>,
    cache_rows_1d: Vec<ChartCandle>,
    /// The core's OWN retained 5-minute ring, kept as a coarse fill layer for sub-5m timeframes.
    ///
    /// `snap_part` already merges this ring into the series at 5 minutes and coarser, where it can
    /// be resampled. A 1-minute series cannot resample it — but it can still DRAW it in a hole, and
    /// this is the only layer that covers a stretch during which the CORE was up and the terminal
    /// was not. Rows arrive end-stamped and range-only, so they are shifted and oriented on the way
    /// in and stored ready to use.
    ring_rows_5m: Vec<ChartCandle>,
    /// Bumped whenever the coarse layers above are reread, so the composed fill below can tell a
    /// stale cache from a stale series without comparing the row vectors themselves.
    cache_generation: u64,
    /// When the last cache read TIMED OUT, so the retry is not attempted on every frame.
    ///
    /// `None` means the last attempt completed, whatever it found.
    cache_retry_at: Option<Instant>,
    /// The series plus its coarse gap fillers, each tagged with the timeframe it is drawn at.
    ///
    /// Retained rather than rebuilt per block because the two consumers fire INDEPENDENTLY: the
    /// upload runs only when the series revision moved, while the auto-Y scan runs every frame.
    /// Deriving the fill twice is how the price scale and the drawn candles came to disagree about
    /// which coarse rows exist; one vector makes that unrepresentable.
    coarse_fill: Vec<(ChartCandle, f32)>,
    /// `(series revision, cache generation)` the retained fill was composed from.
    coarse_fill_key: Option<(u64, u64)>,
    /// Signature of the last deep rows written to the cache; write back only after a change.
    cache_written_sig: u64,
    /// Throttle for candle-to-now gap diagnostics: at most one warning per panel every 30 seconds.
    last_gap_diag: Option<Instant>,
    /// Equivalent signature for rows of the panel's native kind.
    ///
    /// These rows are produced by native backfill attempts when the core's effective kind is finer
    /// than the panel's native kind.
    cache_written_native_sig: u64,
    /// Low-cost fingerprint of loaded deep rows at the last series rebuild.
    ///
    /// It hashes only the row count and final timestamp. An in-place OHLC update at the same final
    /// timestamp therefore does not itself trigger a rebuild or cache writeback. A trade-tail or
    /// explicit reset can independently rebuild the series; writeback waits for the fingerprint to
    /// advance, normally when a new bucket arrives.
    last_deep_sig: u64,
}

impl ChartHistoryCursor {
    pub fn reset(&mut self) {
        self.trades = None;
        self.liquidations = None;
        self.last_prices = None;
        self.mark_prices = None;
        self.last_price = None;
        self.trade_rows.clear();
        self.scan_trade_rows.clear();
        self.liq_rows.clear();
        self.last_price_rows.clear();
        self.mark_price_rows.clear();
        self.candle_series.invalidate();
        self.candle_trades = None;
        self.candle_trade_rows.clear();
        self.candle_ticks.clear();
        self.server_candle_rows.clear();
        self.server_candles.clear();
        // The composed fill is derived from the series, so it cannot outlive an invalidated one.
        self.ring_rows_5m.clear();
        self.coarse_fill.clear();
        self.coarse_fill_key = None;
        // Preserve last_deep_request so request throttling survives a reset. Changing markets
        // recreates PaneRender and therefore starts with a fresh cursor.
    }
}

#[derive(Default)]
pub struct ChartHistoryBuffers {
    pub ticks: Vec<Tick>,
    /// Liquidation trades from the separate `readers.liquidations` ring.
    ///
    /// A reset returns the full visible range; otherwise only new live-edge rows are returned, as
    /// with `ticks`. The quantity sign carries the side, but the renderer assigns `side=2` and
    /// draws all liquidations with one color.
    pub liquidations: Vec<Tick>,
    pub last_points: Vec<PricePoint>,
    pub mark_points: Vec<PricePoint>,
    /// Complete visible candle series.
    ///
    /// Populated only when the series revision differs from
    /// `CandleReadParams::shipped_revision`; see `ChartHistoryRead::candles_changed`.
    pub candles: Vec<ChartCandle>,
    /// Timeframe in milliseconds for each entry in `candles`, stored as a parallel array.
    ///
    /// The historical prefix is extended with coarser timeframes, first 5-minute and then 1-day,
    /// whose candles require distinct widths. Empty means every candle uses the series timeframe.
    pub candle_tf_ms: Vec<f32>,
}

impl ChartHistoryBuffers {
    fn clear(&mut self) {
        self.ticks.clear();
        self.liquidations.clear();
        self.last_points.clear();
        self.mark_points.clear();
        self.candles.clear();
        self.candle_tf_ms.clear();
    }
}

/// Candle and trade-zone parameters for `read_chart_history_into`.
///
/// Passing `None` preserves legacy behavior: trade crosses only, without candles.
#[derive(Debug, Clone, Copy)]
pub struct CandleReadParams {
    /// Series timeframe in milliseconds.
    pub tf_ms: i64,
    /// Lower bound for displayed trades in milliseconds relative to the epoch.
    ///
    /// This defines the last-K-candles display zone. `f32::INFINITY` hides trades entirely when
    /// K is zero. It does not limit candle aggregation.
    pub trades_from_rel_ms: f32,
    /// Hard limit on the number of displayed trades.
    pub trades_limit: usize,
    /// Series revision already delivered to the renderer.
    ///
    /// When it matches the current revision, `out.candles` remains empty.
    pub shipped_revision: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ChartHistoryRead {
    pub provider: CoreId,
    pub revision: u64,
    pub combo_capacity: usize,
    pub price_line_capacity: usize,
    pub combo_left_rel_ms: Option<f32>,
    pub combo_reset: bool,
    pub price_lines_changed: bool,
    pub clipped: bool,
    pub caught_up: bool,
    pub tick_price_range: Option<(f32, f32)>,
    pub last_price: Option<f32>,
    /// Whether the candle series changed from `shipped_revision`, populating `out.candles`.
    pub candles_changed: bool,
    /// Current candle-series revision to return in the next `CandleReadParams`.
    pub candles_revision: u64,
}

/// Live timeframe-bar subscription state for one `(provider, market)`; see `candle_subs`.
struct CandleSubState {
    kind_min: u32,
    last_want: Instant,
    subscribed: bool,
}

struct MarketDataSourceInner {
    store: SharedMarketStore,
    clients: HashMap<CoreId, SharedMoonClient>,
    core_provider: HashMap<CoreId, CoreId>,
    provider_orderbook_kind: HashMap<CoreId, OrderBookKind>,
    cursors: HashMap<(CoreId, String), MarketPullCursor>,
    market_revisions: HashMap<CoreId, HashMap<String, MarketRevisionCounters>>,
    provider_generations: HashMap<CoreId, u64>,
    started_at: Instant,
    /// Global deduplication gate for coin-card requests, mapping request keys to send times.
    ///
    /// Cursors are per panel, so N windows for one coin would otherwise send N identical requests.
    /// Deep history consumes exchange request weight in the core, so the application sends at most
    /// one request per `(provider, market, kind_min)` every 30 seconds. The response enters shared
    /// retained state used by all panels.
    deep_req_gate: Mutex<HashMap<(CoreId, String, u32), Instant>>,
    /// Requested deep kinds for live candle panels, grouped by provider.
    ///
    /// Each `kind_min` maps to its most recent demand. The core holds one candle timeframe per
    /// core, according to the MoonBot developer on 2026-07-12, and each kind change refetches
    /// history from the exchange. Alternating kinds across windows can therefore trigger API
    /// limits. The effective core kind is the minimum live request because the supported kinds
    /// divide into the chain `1|5|30|60|240|1440`; coarser panels resample the finer base at the
    /// cost of depth. A demand entry remains live for 30 seconds.
    deep_kind_wants: Mutex<HashMap<CoreId, HashMap<u32, Instant>>>,
    /// Live timeframe-bar subscriptions keyed by `(provider, market)`.
    ///
    /// A subscription is global to the client and the most recent kind wins, so per-panel control
    /// repeatedly disturbed the core. Panels now only refresh demand. Entries stale for more than
    /// 60 seconds, such as closed panels or sub-minute timeframes, are unsubscribed during the next
    /// candle read for that provider.
    candle_subs: Mutex<HashMap<(CoreId, String), CandleSubState>>,
    /// Local kline cache in `klines.sqlite`; `None` until the terminal supplies its path.
    kline_cache: Option<crate::market::kline_cache::KlineCache>,
    /// Exchange identity used to share kline-cache rows across cores on the same exchange and to
    /// survive selected-provider changes. `CoreId` itself is a stable uid since schema v11, but it
    /// identifies one core rather than the exchange.
    provider_exchange: HashMap<CoreId, crate::feed::ExchangeId>,
    /// What every core is connected to, as the session identified it.
    ///
    /// Wider than `provider_exchange` beside it, which holds only elected providers: the arbitrage
    /// column has to know the venue of the core a PANE sits on, whoever serves its prices, to keep
    /// that venue out of its own column.
    core_venue: HashMap<CoreId, crate::venue::CoreVenue>,
    /// Arbitrage quotes by coin; see [`arb`] for why they are not per core.
    ///
    /// Behind its own `Mutex` inside this lock so a read that takes minutes' worth of market locks
    /// does not hold the source's write side. Never lock it while holding the source lock for
    /// anything but a copy.
    arb_book: Arc<Mutex<arb::ArbBook>>,
    /// Who may currently ask the core for a coarse-timeframe native backfill; see
    /// [`history::NativeBackfillGate`], which owns the claim state and the whole rationale.
    native_backfill: history::NativeBackfillGate,
    /// Who has already been asked for a core chart archive; see [`archive`].
    ///
    /// Shared behind an `Arc` so the chart read can take its handle in the same guard that
    /// resolves the client and then talk to MoonProto with the source lock released.
    archive: Arc<archive::ArchiveGate>,
}

/// How one market is named to the user.
///
/// Built by [`MarketDataSource::market_label`]; see it for why both fields come from one place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketLabel {
    /// Coin TOKEN as the core names it: `SOL`, OKX `BEAT`, Bybit `1kBONKPERP`, COIN-M `SOL_RP`
    /// and `SOL_0925`, Hyperliquid spot `HFUN` for the market `@156`.
    ///
    /// This is the identity to WRITE and to FILTER by — the core matches its coin lists against it
    /// by exact text and its report stores it. It is not always what a coin column should show:
    /// it carries a contract tail. Use [`Self::display_coin`] for that and [`Self::match_key`] to
    /// compare two spellings of one coin.
    pub coin: String,
    /// Coin identity as the CORE resolved it — `market_currency_canonic` — or empty when the
    /// catalog does not hold this market.
    ///
    /// THE answer to "is this the same coin on another exchange", and deliberately taken whole
    /// rather than derived: the core already folds `1000BONK`, `1kBONK` and `BONK` to one `BONK`,
    /// `1000SATS` to `SATS` and `AAVE_RP` to `AAVE` — foldings no rule over a market name can
    /// reproduce, because `1000SATS` and `1000CAT` look alike and only one of them is a
    /// multiplier. Measured across 21 live cores (2026-08-24): `market_currency` splits BONK into
    /// four groups and BTC into twelve, `canonic` into one each.
    ///
    /// NOT a replacement for [`Self::coin`], which stays the token to WRITE: the core matches its
    /// own coin lists against `market_currency` by exact text and the report stores that spelling.
    /// Two fields, two questions — use [`Self::identity`] to compare, `coin` to write.
    ///
    /// Left as the core sends it, including what it does not fold: a Bybit USDC perpetual arrives
    /// as `BONKPERP` and therefore matches only its own kind. That is a core-side gap by decision,
    /// not something to paper over here — a terminal-side rule would be a second opinion about
    /// coin identity, and the whole point is to have one.
    pub canonic: String,
    /// Quote currency, uppercase, or empty when neither the catalog nor the name carries one.
    pub quote: String,
    /// Contract tail recovered from a market NAME when the catalog could not answer, so a dated
    /// contract does not share a label with its perpetual. `None` on the catalog path, where the
    /// contract is already spelled inside [`Self::coin`].
    pub contract: Option<String>,
}

impl MarketLabel {
    /// The reading a market NAME alone supports, used when the catalog does not hold the market.
    pub fn from_name(market: &str, exchange: crate::symbol::Exchange) -> Self {
        let parts = crate::symbol::parse::split_market(market, exchange);
        Self {
            coin: parts.base.to_string(),
            // No catalog, no canonic: this path exists precisely because the market is not in it.
            // [`Self::identity`] falls back to the folded token, which is what comparing did
            // before the field existed.
            canonic: String::new(),
            quote: parts.quote.to_ascii_uppercase(),
            contract: parts.contract.map(str::to_string),
        }
    }

    /// The coin WITHOUT its contract tail, for a column headed "coin": `AAVE_RP` reads `AAVE`.
    pub fn display_coin(&self) -> &str {
        crate::symbol::strip_contract_suffix(&self.coin)
    }

    /// THE key for "are these the same coin?", folding contract and case exactly as a strategy
    /// coin list is compared. Matching raw [`Self::coin`] would fail to connect `AAVE` to the
    /// COIN-M market the core calls `AAVE_RP`.
    ///
    /// This is the key for the core's own vocabulary — coin lists, the report, the news feed, the
    /// tuner — all of which compare against `market_currency`. For "the same coin on ANOTHER
    /// exchange" use [`Self::identity`]: this one keeps `1kBONK` and `1000BONK` apart, as the two
    /// cores that spell them do.
    pub fn match_key(&self) -> String {
        crate::symbol::coin_match_key(&self.coin)
    }

    /// THE key for "is this the same coin on another exchange".
    ///
    /// The core's own [`Self::canonic`] where there is one, and the folded token otherwise — a
    /// market the catalog does not hold has no canonic, and answering "never the same coin" for it
    /// would drop a chart's whole arbitrage column the moment its market was delisted.
    ///
    /// Uppercased because the two sources are not consistent about case: `1kBONK` folds to `BONK`
    /// on one core and the name-based fallback yields whatever the market name carried.
    pub fn identity(&self) -> String {
        match self.canonic.trim().is_empty() {
            true => self.match_key(),
            false => self.canonic.trim().to_ascii_uppercase(),
        }
    }

    /// `SOL-USDT` for a table cell or a chart caption, with a dated contract keeping its expiry
    /// (`SOL-USD-0925`) because two expiries are two instruments. A perpetual carries no tail:
    /// on a futures connection every market is one, so printing it everywhere says nothing.
    pub fn pair(&self) -> String {
        let base = self.display_coin();
        let mut out = if self.quote.is_empty() {
            base.to_string()
        } else {
            format!("{base}-{}", self.quote)
        };
        if let Some(expiry) = self.expiry() {
            out.push('-');
            out.push_str(expiry);
        }
        out
    }

    /// The EXPIRY this market carries, if any: `BTC_0925` → `0925`, a name-sourced `07AUG26`.
    ///
    /// `None` for a perpetual. Moonbot marks one with an `_RP` tail, which is a contract KIND and
    /// not a date — reading it as an expiry is what made the market picker prefer a quarterly
    /// over the perpetual.
    pub fn expiry(&self) -> Option<&str> {
        let base = self.display_coin();
        self.coin
            .get(base.len()..)
            .map(|tail| tail.trim_start_matches('_'))
            .filter(|tail| !tail.is_empty())
            .or(self.contract.as_deref())
            .filter(|tail| !tail.eq_ignore_ascii_case("RP"))
    }
}

/// Pick the market that carries a coin's IDENTITY on one exchange, for a chart to open beside
/// another.
///
/// A different question from [`pick_market_for_coin`] beside it, and kept apart on purpose. That
/// one answers "which market is this token" for a report row or a news click, matching the core's
/// own spelling. This one answers "show me this coin over there", where the token is spelled by a
/// different exchange and the candidates include instruments a reader did not ask for.
///
/// Three rules, in order:
///
/// 1. **Same identity.** `1kBONK` on Bybit and `1000BONK` on Binance are one coin because both
///    cores fold them to `BONK`; `BONK3L` is not, and neither is `PEPECOIN`.
/// 2. **A dated contract only when nothing else carries the coin.** One live core lists BTC under
///    ten Bybit expiries plus the perpetual; opening eleven charts answers a question nobody asked,
///    and a spread against an expiry is basis rather than arbitrage. A coin that trades ONLY as a
///    dated contract still opens — the nearest expiry, since the list arrives in no useful order.
/// 3. **The reader's own quote currency first.** A click from a USDT chart opens `BTCUSDT`, from a
///    USDC one `BTCPERP`. Then any USD stablecoin, then whatever is left, so a coin quoted only in
///    BTC still opens rather than silently doing nothing.
///
/// Ties break on the market NAME, not on catalog order: two clicks on one venue must open the same
/// chart, and the catalog is a `HashMap` walk away from being ordered differently.
///
/// Args:
///     candidates: `(market name, label)` pairs from ONE core, as `market_labels` builds them.
///     identity: [`MarketLabel::identity`] of the chart the request came from.
///     quote: Quote currency of that chart, for rule 3. Empty asks for no preference.
///
/// Returns:
///     The market to open, or `None` when this core does not carry the coin at all.
pub fn pick_market_for_identity<'a>(
    candidates: &'a [(String, MarketLabel)],
    identity: &str,
    quote: &str,
) -> Option<&'a str> {
    let wanted = identity.trim().to_ascii_uppercase();
    if wanted.is_empty() {
        return None;
    }
    let mut matching: Vec<&'a (String, MarketLabel)> = candidates
        .iter()
        .filter(|(_, label)| label.identity() == wanted)
        .collect();
    if matching.is_empty() {
        return None;
    }
    matching.sort_by(|a, b| a.0.cmp(&b.0));
    if matching.iter().any(|(_, label)| label.expiry().is_none()) {
        matching.retain(|(_, label)| label.expiry().is_none());
    }
    let quote = quote.trim();
    let exact = matching
        .iter()
        .find(|(_, label)| !quote.is_empty() && label.quote.eq_ignore_ascii_case(quote));
    let usd = || {
        matching
            .iter()
            .find(|(_, label)| crate::symbol::is_usd_stable(&label.quote))
    };
    exact
        .or_else(usd)
        .or_else(|| matching.first())
        .map(|(name, _)| name.as_str())
}

/// Pick the market a COIN belongs to, from candidates already labelled by the core's catalog.
///
/// The question "which market is `1kRATS`?" cannot be answered from market names: the market is
/// spelled `1000RATSUSDT` and only the catalog knows the core folds it to `1kRATS`. Comparing a
/// name reading against a coin the core wrote — a report row, a coin list — silently finds
/// nothing and leaves the caller inventing a market that does not exist.
///
/// Exact token first, then the folded [`MarketLabel::match_key`] so a bare `AAVE` still reaches
/// the COIN-M market the core calls `AAVE_RP`. Within each pass an undated contract wins: a coin
/// names an instrument family, not an expiry.
pub fn pick_market_for_coin<'a>(
    candidates: &'a [(String, MarketLabel)],
    coin: &str,
) -> Option<&'a str> {
    let wanted_key = crate::symbol::coin_match_key(coin);
    let pick = |matches: &dyn Fn(&MarketLabel) -> bool| -> Option<&'a str> {
        let mut dated = None;
        for (name, label) in candidates {
            if !matches(label) {
                continue;
            }
            if label.expiry().is_none() {
                return Some(name.as_str());
            }
            dated.get_or_insert(name.as_str());
        }
        dated
    };
    pick(&|label: &MarketLabel| label.coin.eq_ignore_ascii_case(coin))
        .or_else(|| pick(&|label: &MarketLabel| label.match_key() == wanted_key))
}

impl MarketDataSourceInner {
    /// The naming family of a market-data provider, for callers already holding the lock.
    ///
    /// Exists so a caller that needs the provider's client AND its naming family reads both under
    /// one guard: taken separately, a provider election between the two would spell markets for
    /// one exchange and price them against another's catalog.
    fn exchange_of_provider(&self, provider: CoreId) -> crate::symbol::Exchange {
        self.provider_exchange
            .get(&provider)
            .map(|id| crate::symbol::Exchange::from_code(id.code))
            .unwrap_or_default()
    }
}

/// UI-agnostic market read-model bridge.
///
/// Feed threads publish only `SharedMoonClient` slots and lightweight wakes.
/// Consumers call this source when they are about to render: it pulls retained
/// MoonProto snapshot rows through per-consumer cursors into the shared
/// `MarketStore`, then exposes a read-only view by consumer core/market.
#[derive(Clone)]
pub struct MarketDataSource {
    inner: Arc<RwLock<MarketDataSourceInner>>,
}

fn moon_time_from_rel_ms(epoch_ms: f64, rel_ms: f32) -> MoonTime {
    MoonTime::from_unix_millis((epoch_ms + rel_ms as f64).round() as i64)
}

/// Drain a last-price or mark-price line through their shared control flow.
///
/// The branches differ only in cursor, buffer, output, and converter. A reset or first call places
/// the cursor at now; subsequent calls drain new rows and accumulate `clipped` and `caught_up` in
/// `read`. After a change, the visible range is copied and converted to points. Call only when the
/// reader exists.
#[allow(clippy::too_many_arguments)]
fn drain_price_line<R: SeqRingTimedRow>(
    reader: &SeqRingReader<R>,
    from_time: MoonTime,
    to_time: MoonTime,
    force_reset: bool,
    cursor_slot: &mut Option<SeqRingCursor>,
    rows: &mut Vec<R>,
    out: &mut Vec<PricePoint>,
    read: &mut ChartHistoryRead,
    convert: impl Fn(&[R], &mut Vec<PricePoint>),
) {
    read.price_line_capacity = read.price_line_capacity.max(reader.capacity());
    let reset = force_reset || cursor_slot.is_none();
    let mut changed = reset;
    if reset {
        *cursor_slot = Some(reader.cursor_from_now());
    } else if let Some(cur) = cursor_slot.as_mut() {
        let meta = reader.drain_new_bounded(cur, reader.capacity(), rows);
        read.clipped |= meta.clipped;
        read.caught_up &= meta.caught_up;
        changed = meta.copied > 0 || meta.clipped;
    }
    if changed {
        reader.copy_time_range(from_time, to_time, reader.capacity(), rows);
        convert(rows, out);
        read.price_lines_changed = true;
    }
}

fn rows_to_ticks(rows: &[TradeHistoryRow], out: &mut Vec<Tick>) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().map(|r| Tick {
        time_ms: r.unix_millis() as f64,
        price: r.price,
        qty: r.quantity(),
        side: if r.is_buy() { Side::Buy } else { Side::Sell },
    }));
}

/// Convert last-price or mark-price line rows into chart points.
///
/// Both row types expose time through `SeqRingTimedRow` and price as the degenerate `(p, p)` range
/// through `SeqRingPriceRow`; these rows never return `None`.
fn price_rows_to_points<R: SeqRingTimedRow + SeqRingPriceRow>(
    rows: &[R],
    out: &mut Vec<PricePoint>,
) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().filter_map(|p| {
        let (price, _) = p.seq_ring_price_range()?;
        Some(PricePoint {
            time_ms: p.seq_ring_time_ms() as f64,
            price,
        })
    }));
}

fn trade_price_range(rows: &[TradeHistoryRow]) -> Option<(f32, f32)> {
    if rows.is_empty() {
        return None;
    }
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for r in rows {
        lo = lo.min(r.price);
        hi = hi.max(r.price);
    }
    Some((lo, hi))
}

fn cadence_phase_ms(provider: CoreId, market: &str, period_ms: u64) -> u64 {
    let mut sig = 0xcbf29ce484222325u64;
    sig ^= provider;
    sig = sig.wrapping_mul(0x100000001b3);
    for b in market.bytes() {
        sig ^= b as u64;
        sig = sig.wrapping_mul(0x100000001b3);
    }
    sig % period_ms.max(1)
}

fn cadence_slot(elapsed_ms: u64, phase_ms: u64, period_ms: u64) -> Option<u64> {
    if elapsed_ms < phase_ms {
        None
    } else {
        Some((elapsed_ms - phase_ms) / period_ms.max(1))
    }
}
