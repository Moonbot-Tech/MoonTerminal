//! Market read-model types, shared state and source submodules.

mod arb;
mod archive;
pub(crate) use archive::ARCHIVE_WAIT;
mod history;
#[cfg(test)]
mod label_tests;
mod price_fit;
mod read;
mod refresh;
mod replay;
#[cfg(test)]
mod tests;
mod volume;

pub use read::{ReplayAddress, ReplayAddressError};
pub(crate) use replay::CoreReplayTicks;
pub use volume::{
    LiqSpanReadout, PriceProfileRow, ProfileWindow, SIDE_BUCKET_MS, SideSlot, SideVolumeBucket,
    VolumeAt, VolumeSpan, VolumeSpanReadout, merge_side_slots, replay_sides, side_slots_of_ticks,
};

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

mod diagnostics;
mod dto;
mod history_state;
mod identity;
mod limits;
mod ring_helpers;

pub use diagnostics::MarketRevisions;
use diagnostics::{
    MarketPullCursor, MarketRevisionCounters, ORDERBOOK_PULL_PERIOD_MS, bump_generation,
    bump_market_revisions, market_diag_enabled,
};
pub(super) use diagnostics::{SOURCE_TRACE_LEVEL, market_diag, market_diag_due, market_diag_floor};
pub(crate) use dto::CandleEmit;
pub use dto::{
    ArbQuote, ArbVenue, CoinTag, DetectSnapshot, LatestPriceError, MarketContextReadout,
    MarketFiguresReadout, MarketWindowsReadout, WindowFigures,
};
pub use history_state::{
    CandleReadParams, ChartHistoryBuffers, ChartHistoryCursor, ChartHistoryRead, PriceLineUpdate,
};
pub use identity::{
    MarketLabel, funding_from_wire, pick_market_for_coin, pick_market_for_identity,
};
pub use limits::{
    MarketLimits, MarketQuantityUnit, MarketTickerReadout, MaxOrder, MaxOrderSource, OrderSizeRules,
};
pub(crate) use limits::{
    market_quantity_unit, max_order_notional, min_order_floor, position_cap, session_base_rate,
    session_to_usdt, session_usdt,
};
pub use ring_helpers::MarketDataSource;
use ring_helpers::{
    cadence_phase_ms, cadence_slot, drain_price_line, moon_time_from_rel_ms, price_rows_to_points,
    rows_to_ticks, trade_price_range,
};

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
    /// Traded amounts by `(provider, span, market)`; see [`volume`] for what it costs to fill.
    ///
    /// Beside the arbitrage book and held the same way — behind its own `Arc<Mutex<_>>` so a read
    /// takes the handle out of the source lock and then talks to MoonProto with that lock released.
    /// Locking it while holding the source's write side would invert `remove_client`'s order.
    volume_book: Arc<Mutex<volume::VolumeBook>>,
    /// Who may currently ask the core for a coarse-timeframe native backfill; see
    /// [`history::NativeBackfillGate`], which owns the claim state and the whole rationale.
    native_backfill: history::NativeBackfillGate,
    /// Who has already been asked for a core chart archive; see [`archive`].
    ///
    /// Shared behind an `Arc` so the chart read can take its handle in the same guard that
    /// resolves the client and then talk to MoonProto with the source lock released.
    archive: Arc<archive::ArchiveGate>,
}
