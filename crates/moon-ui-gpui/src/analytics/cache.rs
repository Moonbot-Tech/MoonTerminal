//! Bounded, process-lifetime snapshots for reopening Analytics and revisiting its pages.

use std::collections::VecDeque;
use std::mem::size_of;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moon_core::db::analytics::{
    AnalyticsRead, CalendarRead, CellTotals, DayCell, GroupStat, PreviousPeriodBasis, Query,
    StrategyBase, Summary, TopTrade, UndatedCloses,
};
use moon_core::db::valuation::ValuationMode;
use moon_core::db::{
    CoreNames, ProfitMetric, ProfitScope, ProfitUnit, QuoteBreakdown, ReportAxis, SideFilter,
};

/// Revalidate metadata cadence and time-sensitive valuation even when no publication moved.
const FRESH_FOR: Duration = Duration::from_secs(60);

/// Independent data publications that can change a computed page without changing its filters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CacheRevisions {
    pub(super) reports: u64,
    pub(super) valuation: u64,
    pub(super) strategies: u64,
    /// Certified process knowledge used to build report money expressions.
    pub(super) read_state: Option<u64>,
    /// Healthy derived-cache attachment epoch, including recovery transitions.
    pub(super) valuation_attachment: Option<u64>,
}

impl CacheRevisions {
    /// Reject late background completions without treating a counter rollover as a regression.
    fn no_older_than(self, previous: Self) -> bool {
        self.reports.wrapping_sub(previous.reports) < (1 << 63)
            && self.valuation.wrapping_sub(previous.valuation) < (1 << 63)
            && self.strategies.wrapping_sub(previous.strategies) < (1 << 63)
            && matches!((self.read_state, previous.read_state), (Some(now), Some(before)) if now.wrapping_sub(before) < (1 << 63))
            && matches!((self.valuation_attachment, previous.valuation_attachment), (Some(now), Some(before)) if now.wrapping_sub(before) < (1 << 63))
    }
}

/// Every query operand that changes rows, units, attribution, or displayed metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
struct QueryKey {
    axis: ReportAxis,
    previous_period_basis: PreviousPeriodBasis,
    from: i64,
    to: i64,
    cores: Vec<u64>,
    side: SideFilter,
    emulator: Option<bool>,
    strategies: Vec<(i64, Option<u64>)>,
    strategy_name_mask: String,
    metric: ProfitMetric,
    valuation: ValuationMode,
    prefer_usdt: bool,
    core_names: CoreNames,
}

impl From<&Query> for QueryKey {
    /// Preserve raw mask identity while canonicalizing fields used as semantic sets by SQL.
    fn from(query: &Query) -> Self {
        let mut cores = query.cores.clone();
        cores.sort_unstable();
        cores.dedup();
        let mut strategies = query.strategies.clone();
        strategies.sort_unstable();
        strategies.dedup();
        Self {
            axis: query.axis.clone(),
            previous_period_basis: query.previous_period_basis,
            from: query.from,
            to: query.to,
            cores,
            side: query.side,
            emulator: query.emulator,
            strategies,
            strategy_name_mask: query.strategy_name_mask.clone(),
            metric: query.metric,
            valuation: query.valuation,
            prefer_usdt: query.prefer_usdt,
            core_names: query.core_names.clone(),
        }
    }
}

/// Page-specific work that must never alias another page's payload.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Surface {
    Summary {
        include_previous: bool,
    },
    Strategies,
    Calendar {
        previous: Option<Box<QueryKey>>,
        hourly: bool,
    },
}

/// Complete semantic request identity, separate from its data-publication revisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CacheKey {
    query: QueryKey,
    surface: Surface,
    replica_hidden: Option<Vec<u64>>,
    revisions: CacheRevisions,
}

impl CacheKey {
    /// Reuse requires certified admission and money knowledge, even for explicitly stale display.
    pub(super) fn is_reusable(&self) -> bool {
        self.revisions.read_state.is_some() && self.revisions.valuation_attachment.is_some()
    }

    /// A changed admission or money policy cannot inherit even explicitly age-labelled figures.
    pub(super) fn same_read_state(&self, other: &Self) -> bool {
        self.revisions.read_state == other.revisions.read_state
            && self.revisions.valuation_attachment == other.revisions.valuation_attachment
    }

    /// Identify a Summary read, including whether its comparison has been computed.
    pub(super) fn summary(
        query: &Query,
        replica_hidden: Option<&[u64]>,
        revisions: CacheRevisions,
        include_previous: bool,
    ) -> Self {
        Self::new(
            query,
            replica_hidden,
            revisions,
            Surface::Summary { include_previous },
        )
    }

    /// Identify the compact strategy/coin base without retaining tuner drafts.
    pub(super) fn strategies(
        query: &Query,
        replica_hidden: Option<&[u64]>,
        revisions: CacheRevisions,
    ) -> Self {
        Self::new(query, replica_hidden, revisions, Surface::Strategies)
    }

    /// Identify Calendar cells and the exact optional comparison they contain.
    pub(super) fn calendar(
        query: &Query,
        previous: Option<&Query>,
        hourly: bool,
        replica_hidden: Option<&[u64]>,
        revisions: CacheRevisions,
    ) -> Self {
        Self::new(
            query,
            replica_hidden,
            revisions,
            Surface::Calendar {
                previous: previous.map(|query| Box::new(QueryKey::from(query))),
                hourly,
            },
        )
    }

    /// Copy the request before asynchronous work can outlive its originating view.
    fn new(
        query: &Query,
        replica_hidden: Option<&[u64]>,
        revisions: CacheRevisions,
        mut surface: Surface,
    ) -> Self {
        let mut query = QueryKey::from(query);
        let replica_hidden = replica_hidden.map(|hidden| {
            let mut hidden = hidden.to_vec();
            hidden.sort_unstable();
            hidden.dedup();
            // Replica discovery replaces the bootstrap inclusion list for BOTH period reads.
            query.cores.clear();
            if let Surface::Calendar {
                previous: Some(previous),
                ..
            } = &mut surface
            {
                previous.cores.clear();
            }
            hidden
        });
        Self {
            query,
            surface,
            replica_hidden,
            revisions,
        }
    }

    /// Compare only data meaning; a stale hit is explicitly classified by the caller.
    pub(super) fn same_scope(&self, other: &Self) -> bool {
        self.query == other.query
            && self.surface == other.surface
            && self.replica_hidden == other.replica_hidden
    }

    /// Conservatively budget opaque axis/name maps through their complete debug representation.
    fn estimated_bytes(&self) -> usize {
        size_of::<Self>().saturating_add(format!("{self:?}").len().saturating_mul(2))
    }
}

/// Shared immutable scalar data or an explicit, unit-safe split-money result.
#[derive(Clone)]
pub(super) enum CachedScope<T> {
    Comparable { unit: ProfitUnit, data: Arc<T> },
    Empty(Arc<T>),
    Split(Arc<QuoteBreakdown>),
}

impl<T> CachedScope<T> {
    /// Move a successful result into shared storage once, before either view or cache owns it.
    fn new(scope: ProfitScope<T>) -> Self {
        match scope {
            ProfitScope::Comparable { unit, data } => Self::Comparable {
                unit,
                data: Arc::new(data),
            },
            ProfitScope::Empty(data) => Self::Empty(Arc::new(data)),
            ProfitScope::Split(totals) => Self::Split(Arc::new(totals)),
        }
    }

    /// Account for the scalar or split allocation without cloning its rows.
    fn estimated_bytes(&self, scalar_bytes: impl FnOnce(&T) -> usize) -> usize {
        match self {
            Self::Comparable { data, .. } | Self::Empty(data) => scalar_bytes(data),
            Self::Split(totals) => quote_bytes(totals),
        }
    }
}

/// One fully successful compound snapshot; partial errors never enter the cache.
#[derive(Clone)]
pub(super) struct CachedRead<T> {
    pub(super) data: CachedScope<T>,
    pub(super) undated: Arc<UndatedCloses>,
    pub(super) cores: Option<Arc<Vec<(u64, String)>>>,
    /// Metadata has its own observation time; refreshing a page cannot renew an older core list.
    pub(super) cores_at: Option<Instant>,
}

impl<T> CachedRead<T> {
    /// Retain independently classified failures for the original completion handler to publish.
    pub(super) fn from_analytics(
        read: AnalyticsRead<ProfitScope<T>>,
    ) -> Result<Self, Box<AnalyticsRead<ProfitScope<T>>>> {
        Self::from_analytics_at(read, Instant::now())
    }

    /// Record a supplied observation time only when this compound read actually fetched cores.
    fn from_analytics_at(
        read: AnalyticsRead<ProfitScope<T>>,
        observed_at: Instant,
    ) -> Result<Self, Box<AnalyticsRead<ProfitScope<T>>>> {
        if super::refresh::CatchUpOutcome::of_scope(&read.data).is_transient() {
            return Err(Box::new(read));
        }
        match read {
            AnalyticsRead {
                data: Ok(data),
                undated: Ok(undated),
                cores,
            } if cores.as_ref().is_none_or(Result::is_ok) => Ok(Self {
                data: CachedScope::new(data),
                undated: Arc::new(undated),
                cores_at: cores.as_ref().map(|_| observed_at),
                cores: cores.and_then(Result::ok).map(Arc::new),
            }),
            read => Err(Box::new(read)),
        }
    }

    /// Budget compound metadata as well as its primary page payload.
    fn estimated_bytes(&self, scalar_bytes: impl FnOnce(&T) -> usize) -> usize {
        size_of::<Self>()
            .saturating_add(self.data.estimated_bytes(scalar_bytes))
            .saturating_add(quote_bytes(&self.undated.totals))
            .saturating_add(
                self.cores
                    .as_ref()
                    .map_or(0, |cores| core_names_bytes(cores)),
            )
    }
}

/// Calendar retains its displayed cell allocation separately from comparison totals.
#[derive(Clone)]
pub(super) struct CachedCalendar {
    pub(super) data: CachedScope<Vec<DayCell>>,
    pub(super) previous: Option<CellTotals>,
    pub(super) undated: Arc<UndatedCloses>,
    pub(super) cores: Option<Arc<Vec<(u64, String)>>>,
    /// Core-selector cadence is independent of when the Calendar cells were recomputed.
    pub(super) cores_at: Option<Instant>,
}

impl CachedCalendar {
    /// Keep Calendar failures available to its existing typed publication path.
    pub(super) fn from_calendar(read: CalendarRead) -> Result<Self, Box<CalendarRead>> {
        Self::from_calendar_at(read, Instant::now())
    }

    /// Keep deterministic metadata provenance separate from the cache entry publication clock.
    fn from_calendar_at(
        read: CalendarRead,
        observed_at: Instant,
    ) -> Result<Self, Box<CalendarRead>> {
        if super::refresh::CatchUpOutcome::of_scope(&read.period).is_transient() {
            return Err(Box::new(read));
        }
        match read {
            CalendarRead {
                period: Ok(data),
                undated: Ok(undated),
                cores,
            } if cores.as_ref().is_none_or(Result::is_ok) => Ok(Self {
                previous: data.data().and_then(|period| period.previous),
                data: match data {
                    ProfitScope::Comparable { unit, data } => CachedScope::Comparable {
                        unit,
                        data: Arc::new(data.current),
                    },
                    ProfitScope::Empty(data) => CachedScope::Empty(Arc::new(data.current)),
                    ProfitScope::Split(totals) => CachedScope::Split(Arc::new(totals)),
                },
                undated: Arc::new(undated),
                cores_at: cores.as_ref().map(|_| observed_at),
                cores: cores.and_then(Result::ok).map(Arc::new),
            }),
            read => Err(Box::new(read)),
        }
    }
}

/// Typed page snapshots whose clones only increment shared ownership counts.
#[derive(Clone)]
pub(super) enum CachedResult {
    Summary(Arc<CachedRead<Summary>>),
    Strategies(Arc<CachedRead<StrategyBase>>),
    Calendar(Arc<CachedCalendar>),
}

impl CachedResult {
    /// Refuse accidentally associating a Calendar or Strategies payload with a Summary key.
    fn matches_surface(&self, surface: &Surface) -> bool {
        matches!(
            (self, surface),
            (Self::Summary(_), Surface::Summary { .. })
                | (Self::Strategies(_), Surface::Strategies)
                | (Self::Calendar(_), Surface::Calendar { .. })
        )
    }

    /// Estimate retained allocations, including vector capacity and variable-length text.
    fn estimated_bytes(&self) -> usize {
        match self {
            Self::Summary(read) => read.estimated_bytes(summary_bytes),
            Self::Strategies(read) => read.estimated_bytes(|data| {
                size_of::<StrategyBase>()
                    + groups_bytes(&data.strategies)
                    + groups_bytes(&data.coins)
            }),
            Self::Calendar(read) => {
                size_of::<CachedCalendar>()
                    + read
                        .data
                        .estimated_bytes(|data| size_of::<Vec<DayCell>>() + vec_bytes(data))
                    + quote_bytes(&read.undated.totals)
                    + read
                        .cores
                        .as_ref()
                        .map_or(0, |cores| core_names_bytes(cores))
            }
        }
    }
}

/// Lookup outcome that never presents a revision mismatch as fresh data.
pub(super) struct CacheHit {
    pub(super) result: CachedResult,
    pub(super) fresh: bool,
    /// Original successful read time; visiting the cache never advances it.
    pub(super) as_of: i64,
}

/// One LRU entry and its charged memory estimate.
struct Entry {
    key: CacheKey,
    result: CachedResult,
    bytes: usize,
    inserted_at: Instant,
    as_of: i64,
}

/// Small aggregate-result cache; neither raw trade populations nor mutable drafts belong here.
pub(super) struct AnalyticsCache {
    entries: VecDeque<Entry>,
    max_entries: usize,
    max_bytes: usize,
    bytes: usize,
}

impl Default for AnalyticsCache {
    /// Bound retained results independently of how many scopes a user visits.
    fn default() -> Self {
        Self::new(12, 32 * 1024 * 1024)
    }
}

impl AnalyticsCache {
    /// Construct explicit limits for tests and the process-owned cache.
    fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_entries,
            max_bytes,
            bytes: 0,
        }
    }

    /// Find fresh data or an opted-in stale snapshot with unchanged admission and money policy.
    pub(super) fn lookup(&mut self, key: &CacheKey, allow_stale: bool) -> Option<CacheHit> {
        self.lookup_at(key, allow_stale, Instant::now())
    }

    /// Inspect freshness with the same generation and monotonic time rules used by real lookups.
    pub(super) fn has_fresh(&mut self, key: &CacheKey) -> bool {
        self.lookup(key, false).is_some()
    }

    /// Use an injectable monotonic clock so the expiration boundary needs no sleeping test.
    fn lookup_at(&mut self, key: &CacheKey, allow_stale: bool, now: Instant) -> Option<CacheHit> {
        if !key.is_reusable() {
            return None;
        }
        let index = self
            .entries
            .iter()
            .position(|entry| {
                entry.key == *key
                    && (allow_stale || now.saturating_duration_since(entry.inserted_at) < FRESH_FOR)
            })
            .or_else(|| {
                allow_stale
                    .then(|| {
                        self.entries.iter().rposition(|entry| {
                            entry.key.same_scope(key) && entry.key.same_read_state(key)
                        })
                    })
                    .flatten()
            })?;
        let entry = self.entries.remove(index)?;
        let hit = CacheHit {
            fresh: entry.key.revisions == key.revisions
                && now.saturating_duration_since(entry.inserted_at) < FRESH_FOR,
            result: entry.result.clone(),
            as_of: entry.as_of,
        };
        self.entries.push_back(entry);
        Some(hit)
    }

    /// Insert one complete snapshot, evicting least recently used results under both limits.
    pub(super) fn insert(&mut self, key: CacheKey, result: CachedResult) -> bool {
        self.insert_at(
            key,
            result,
            Instant::now(),
            moon_core::util::now_unix_ms_i64() / 1_000,
        )
    }

    /// Record publication clocks only on a successful read, never when serving an older snapshot.
    fn insert_at(
        &mut self,
        key: CacheKey,
        result: CachedResult,
        inserted_at: Instant,
        as_of: i64,
    ) -> bool {
        if !key.is_reusable() || !result.matches_surface(&key.surface) || self.max_entries == 0 {
            return false;
        }
        if self.entries.iter().any(|entry| {
            entry.key.same_scope(&key) && !key.revisions.no_older_than(entry.key.revisions)
        }) {
            return false;
        }
        let bytes = size_of::<Entry>()
            .saturating_add(key.estimated_bytes())
            .saturating_add(result.estimated_bytes())
            .saturating_add(256);
        if bytes > self.max_bytes {
            return false;
        }
        self.remove_scope(&key);
        while self.entries.len() >= self.max_entries
            || self.bytes.saturating_add(bytes) > self.max_bytes
        {
            let Some(oldest) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(oldest.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries.push_back(Entry {
            key,
            result,
            bytes,
            inserted_at,
            as_of,
        });
        true
    }

    /// Forget every revision of an explicitly refreshed scope without clearing unrelated pages.
    pub(super) fn remove_scope(&mut self, key: &CacheKey) {
        self.entries.retain(|entry| {
            if entry.key.same_scope(key) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
                false
            } else {
                true
            }
        });
    }
}

/// Access process-owned snapshots across Analytics window lifetimes.
pub(super) fn with_cache<T>(access: impl FnOnce(&mut AnalyticsCache) -> T) -> T {
    static CACHE: OnceLock<Mutex<AnalyticsCache>> = OnceLock::new();
    // Entries are immutable snapshots; recovering a poisoned cache cannot alter their contents.
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(AnalyticsCache::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    access(&mut cache)
}

/// Retained vector storage, including any deliberately reserved spare capacity.
fn vec_bytes<T>(values: &Vec<T>) -> usize {
    values.capacity().saturating_mul(size_of::<T>())
}

/// Retained core-selector strings and row storage.
fn core_names_bytes(cores: &Vec<(u64, String)>) -> usize {
    vec_bytes(cores) + cores.iter().map(|(_, name)| name.capacity()).sum::<usize>()
}

/// Group vectors carry multiple independent names and keys per row.
fn groups_bytes(groups: &Vec<GroupStat>) -> usize {
    vec_bytes(groups)
        + groups
            .iter()
            .map(|group| {
                group.key.capacity()
                    + group.name.capacity()
                    + group.kind.capacity()
                    + group.core.capacity()
                    + group.lastedit.capacity()
            })
            .sum::<usize>()
}

/// Top-trade metadata is variable-length even though the number of displayed rows is small.
fn top_bytes(rows: &Vec<TopTrade>) -> usize {
    vec_bytes(rows)
        + rows
            .iter()
            .map(|row| {
                row.coin.capacity()
                    + row.strategy.capacity()
                    + row.strategy_id.capacity()
                    + row.core_name.capacity()
            })
            .sum::<usize>()
}

/// Charge every heap-owned collection in a Summary, including per-core time series.
fn summary_bytes(data: &Summary) -> usize {
    size_of::<Summary>()
        + vec_bytes(&data.days)
        + top_bytes(&data.best)
        + top_bytes(&data.worst)
        + groups_bytes(&data.strategies)
        + groups_bytes(&data.coins)
        + core_names_bytes(&data.cores)
        + vec_bytes(&data.core_days)
        + data
            .core_days
            .iter()
            .map(|core| {
                core.name.capacity()
                    + vec_bytes(&core.per_bucket)
                    + vec_bytes(&core.per_bucket_trades)
            })
            .sum::<usize>()
}

/// Split/undated totals retain several quote vectors and the optional source-group box.
fn quote_bytes(totals: &QuoteBreakdown) -> usize {
    size_of::<QuoteBreakdown>()
        + vec_bytes(&totals.totals)
        + vec_bytes(&totals.traded_volume.totals)
        + vec_bytes(&totals.entry_spend.totals)
        + size_of::<moon_core::db::SourceGroups>()
        + vec_bytes(&totals.source_groups.profit)
        + vec_bytes(&totals.source_groups.coverage)
        + vec_bytes(&totals.source_groups.volume)
        + vec_bytes(&totals.source_groups.spend)
}

#[cfg(test)]
mod tests;
