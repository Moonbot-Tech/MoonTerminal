//! `SessionManager` runs one backend thread, or core, for each active configured server whose
//! group is active.
//!
//! Core data is split across two planes:
//! - the account plane (status, orders, detects, and strategies) is specific to each core and
//!   stored in `CoreStore` by `CoreId`;
//! - the market plane (price ticks and order books) is stored in `MarketStore` (see
//!   `crate::market`). In `Dedup` mode, cores on one exchange share a provider core; in `PerCore`
//!   mode, every core is its own provider.
//!
//! The coordinator (see `coordinator.rs`) learns each core's exchange from `Identity`, selects
//! providers according to the active mode, and assigns market roles with the `SetMarket` command.

pub mod coordinator;
pub mod order_lines;
pub mod store;

mod commands;
mod lifecycle;

/// Re-exported from `feed` (its producer): `CoreData::sys` is this type, so the
/// session facade keeps `session::CoreSysStatus` valid for the "Core status" panel.
pub use crate::feed::CoreSysStatus;
pub use store::{BalanceState, CoreId, CoreStore};

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use moonproto::state::OrderBookKind;

use crate::config::ServerConfig;
use crate::feed::{ConnStatus, ExchangeId, FeedHandle, FeedWakeTx};
use crate::market::{MarketDataMode, MarketDataSource, SharedMarketStore};

pub struct CoreSession {
    pub id: CoreId,
    pub name: String,
    pub group: String,
    /// Signature of the connection-relevant key, feed, and synthetic fields used to start this
    /// feed thread. `reconcile` restarts the core only when this changes; changes to its name,
    /// group, market, or color do not require a restart.
    conn_sig: u64,
    handle: FeedHandle,
}

/// Return a process-stable hash of the server fields that require a feed-thread restart.
///
/// The name, group, market, color, link, and size fields are excluded because they can be updated
/// in place without reconnecting.
fn conn_sig(server: &ServerConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    server.key.expose().hash(&mut h);
    let f = server.feed;
    [
        f.orders,
        f.detects,
        f.reports,
        f.balance,
        f.strategies,
        f.log,
        f.alerts,
        f.arb,
    ]
    .hash(&mut h);
    server.synthetic.hash(&mut h);
    h.finish()
}

/// Connection summary for a status bar: ready and total counts plus non-ready core details for the
/// tooltip.
pub struct ConnSummary {
    pub ready: usize,
    pub total: usize,
    /// Non-ready cores as `(id, name, status)` for canonically ordered status tooltips.
    pub down: Vec<(CoreId, String, ConnStatus)>,
}

/// License summary for the cores in one window group.
#[derive(Clone, Debug, Default)]
pub struct LicenseSummary {
    pub total: usize,
    pub known: usize,
    pub paid: usize,
    pub free: usize,
    pub moon_credits: i64,
    pub moon_credits_hold: i64,
    pub moon_credits_auction: i64,
}

pub struct SessionManager {
    sessions: Vec<CoreSession>,
    /// Config ranks for active and inactive cores, keeping reactivated sessions in place.
    /// Refreshed by every path that can insert a session.
    config_order: Vec<CoreId>,
    feed_wake: Option<FeedWakeTx>,
    /// Account plane: per-core status, orders, detects, and strategies. External callers have
    /// read-only access through [`SessionManager::store`]; only the manager mutates it.
    store: CoreStore,
    /// Market plane: a shared buffer outside the GPUI entity. The live feed only wakes consumers;
    /// `MarketDataSource` pulls MoonProto snapshots into the buffer.
    market: SharedMarketStore,
    /// Pull/read-model bridge shared by UI listener and native chart frames.
    market_source: MarketDataSource,
    /// Market-data source mode; its current default is `Dedup`.
    mode: MarketDataMode,
    /// Core-to-exchange mapping from `Identity`. A core cannot become a provider without it.
    core_key: HashMap<CoreId, ExchangeId>,
    /// Core-to-account-base mapping from `CoreBase`, such as "USDT" or "BTC". The UI uses it for
    /// group-USD-to-base order-size conversion. It remains empty until the core is identified.
    core_base: HashMap<CoreId, String>,
    /// Core-to-market-provider mapping: one provider per exchange in deduplicated mode, or the
    /// core itself in per-core mode.
    core_provider: HashMap<CoreId, CoreId>,
    /// Exchange-to-selected-provider mapping for retention and failover in deduplicated mode.
    providers: HashMap<ExchangeId, CoreId>,
    /// Provider-to-served-markets mapping: the union of open charts and lingering markets.
    wanted: HashMap<CoreId, HashSet<String>>,
    /// Deadline for releasing each `(provider, market)` after its last chart closes.
    pending_drop: HashMap<(CoreId, String), Instant>,
    /// Last `(provider, markets, orderbook_markets)` role sent to each core, used to suppress
    /// duplicate commands. `orderbook_markets` is the subset of `markets` that needs an order book.
    last_cmd: HashMap<CoreId, (bool, Vec<String>, Vec<String>)>,
}

#[derive(Clone, Debug, Default)]
pub struct DrainStats {
    /// At least one feed message was applied to session state.
    pub any: bool,
    /// Retained market data changed. Visible charts pull this from `gpu_canvas.frame()`;
    /// this flag must not trigger account/order overlay sync.
    pub market_data: bool,
    /// Account/order overlays changed. These still live in `CoreStore` and need a narrow
    /// sync into visible chart userdata.
    pub order_lines_data: bool,
    /// Slow GPUI chrome/account state changed and the Backend entity should be notified.
    pub ui_state: bool,
}

fn orderbook_kind_for_exchange(ex: ExchangeId) -> OrderBookKind {
    match ex.code {
        // Spot exchanges.
        3 | 5 | 7 | 8 | 10 | 12 => OrderBookKind::Spot,
        // Futures/quarterly derivatives.
        2 | 4 | 6 | 9 | 11 | 13 => OrderBookKind::Futures,
        _ => OrderBookKind::Futures,
    }
}
