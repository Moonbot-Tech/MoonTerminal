//! Connection, market freshness and identity status types.

/// Connection status for a core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnStatus {
    Connecting,
    /// Intermediate connection or initialization stage, carrying badge text.
    Stage(String),
    Ready,
    Failed(String),
    Disconnected,
}

/// Market-data domains that can wake a visible chart.
///
/// The payload is intentionally small: data rows stay in MoonProto/MarketStore,
/// while the terminal keeps causal per-market revisions and pulls only visible
/// chart targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketDirtyFlags(u8);

impl MarketDirtyFlags {
    pub const HISTORY: Self = Self(1 << 0);
    pub const ORDERBOOK: Self = Self(1 << 1);
    pub const MARKET_META: Self = Self(1 << 2);
    /// The core's chart archive was merged into this market's retained rings, PREPENDING rows
    /// older than everything the chart has read so far.
    ///
    /// Distinct from [`Self::HISTORY`] because the two demand different work. `HISTORY` says
    /// "new rows at the live edge", which a chart drains through its cursor; this one says
    /// "rows appeared BEHIND the cursor", which no cursor drain can ever reach. Only a full
    /// history reset picks them up, so it drives its own revision counter.
    pub const HISTORY_ARCHIVE: Self = Self(1 << 3);
    /// Every domain that a periodic sample may re-read.
    ///
    /// Deliberately WITHOUT [`Self::HISTORY_ARCHIVE`]: `ALL` is also the force-sample flag set
    /// whenever the wanted-market set changes, and folding the archive bit in would order a
    /// full chart reset on every chart open, with no archive behind it.
    pub const ALL: Self = Self(Self::HISTORY.0 | Self::ORDERBOOK.0 | Self::MARKET_META.0);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOr for MarketDirtyFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketDirty {
    pub market: String,
    pub flags: MarketDirtyFlags,
}

impl MarketDirty {
    pub fn new(market: impl Into<String>, flags: MarketDirtyFlags) -> Self {
        Self {
            market: market.into(),
            flags,
        }
    }
}

/// Message from a backend to the UI.
///
/// Account messages such as Status, Orders, Detects, and Strategies carry ready UI state for one
/// core. Market ticks, order books, and price lines do not travel through this channel: the feed
/// thread publishes them to MoonProto/MarketStore and sends only a lightweight
/// [`MarketDataChanged`] wake-up for consumer-side pulling.
/// One core's measured clock offset, as the UI is allowed to see it.
///
/// `offset_secs` is `None` for a core nothing has ever been measured on, and that is deliberately
/// NOT the same fact as `Some(0)`: a diagnosis surface must be able to say "never measured" rather
/// than claim the core runs on UTC. Every field beside it exists so the surface can say WHY it
/// believes the number — how many samples, how long ago, and from which source — because an
/// unexplained four-hour correction on a trade list is indistinguishable from a bug.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoreTimeOffsetStatus {
    /// Seconds east of UTC on the core's own clock, or `None` when nothing was ever adopted.
    pub offset_secs: Option<i32>,
    /// True-UTC instant of the LATEST observation carrying this offset, in milliseconds — which is
    /// not always the one that adopted it, and the field is named for what it holds. A reconnect
    /// builds a fresh estimator that re-measures the unchanged value, so this advances while the
    /// durable `core_time_offset.observed_at` deliberately stays at the adoption instant. The
    /// surface reading it says «Замерено» / "Observed"
    /// for exactly that reason: a fresh instant here means the measurement is still live, not that
    /// the offset moved.
    pub observed_at_utc: i64,
    /// Samples standing behind the adopted value.
    pub samples: u32,
    /// Which measurement produced it.
    pub source: crate::session::core_time_offset::OffsetSource,
}

/// Why a feed reported its published exchange identity as stale ([`crate::feed::FeedMsg::IdentityStale`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityStaleCause {
    /// A different MoonBot process now answers on the connection (`LifecycleEvent::ServerRestart`).
    ServerRestart,
    /// A market-list refresh added a large share of the core's market universe at once, which a
    /// listing never does and an exchange switch on a live core does.
    MarketTurnover {
        /// Markets the refresh added.
        added: usize,
        /// Markets the client retains after the refresh, the added ones included.
        total: usize,
    },
}
