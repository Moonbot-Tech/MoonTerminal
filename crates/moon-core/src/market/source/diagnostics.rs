//! Market tracing, revision counters and pull cursors.

use super::*;

pub(super) const ORDERBOOK_PULL_PERIOD_MS: u64 = 200;

/// Default gap between two trace lines about the same subject, from
/// `limits.market_trace_min_interval_ms`. A function rather than a constant because the value is
/// live: the order-book pull runs five times a second, so this floor is what decides whether the
/// channel is readable, and it has to be adjustable without a rebuild.
pub(crate) fn market_diag_floor() -> Duration {
    crate::diagnostics::market_trace_min_interval()
}

/// Level for market-source tracing, admitted by `log.market_sources` in `cfg/diagnostics.toml`.
///
/// Debug, so the default filter (info and above) excludes it. At info these lines followed cursor
/// movement — a hovering cursor re-reads sources — and produced ~2600 lines a day on their own,
/// against a Log panel ring that holds a few thousand.
pub(crate) const SOURCE_TRACE_LEVEL: log::Level = log::Level::Debug;

/// Whether the market channel is on, from `channels.markets` in `cfg/diagnostics.toml`.
///
/// `MOON_MARKET_DIAG` and `MOON_RENDER_DIAG` both still enable it; that pairing is preserved in
/// `diagnostics::config::apply_env` rather than restated here.
pub(super) fn market_diag_enabled() -> bool {
    crate::diagnostics::markets()
}

pub(crate) fn market_diag_due(key: impl Into<String>, floor: Duration) -> bool {
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

pub(crate) fn market_diag(msg: impl std::fmt::Display) {
    if market_diag_enabled() {
        log::info!("[market_diag] {msg}");
    }
}

pub(super) fn bump_generation(revisions: &mut HashMap<CoreId, u64>, provider: CoreId) {
    let entry = revisions.entry(provider).or_insert(0);
    *entry = entry.wrapping_add(1);
}

pub(super) fn bump_market_revisions(
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

pub(super) fn mix_pair(a: u64, b: u64) -> u64 {
    a.wrapping_mul(0x9e37_79b1_85eb_ca87).rotate_left(17) ^ b
}

#[derive(Default)]
pub(super) struct MarketPullCursor {
    pub(super) book_phase_ms: Option<u64>,
    pub(super) last_book_slot: Option<u64>,
    pub(super) last_book_dirty_revision: u64,
    pub(super) last_book_revision: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct MarketRevisionCounters {
    pub(super) history: u64,
    pub(super) book: u64,
    pub(super) meta: u64,
    pub(super) archive: u64,
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
