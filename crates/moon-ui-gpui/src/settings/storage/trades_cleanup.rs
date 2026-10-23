//! Automatic startup retention for `trades.sqlite`, completed before missing tape is loaded.
//!
//! The file knows markets and stretches of time, not trades (`trade_cache::trim`), so a cleanup
//! is built the other way round: the report's rows are resolved to the markets the file spells —
//! through the live catalog like a trade window, or, for a core that is not connected, by the
//! coin's name against the markets the file already holds — and each row claims the stretches
//! its window would ask for as prints (`ReplayWindow::focus_spans`) at the margin in force.
//! The union of every claim per market is the [`KeepMap`]; everything outside it goes. Two
//! rows whose claims overlap claim the ground between them too, so only the outer edges of a
//! run of trades are ever cut.
//!
//! Only the tuner's rows claim ([`TapeOwner::is_tunable`]), each at the margin the tuner reads
//! it with: what a wider margin left behind, the prints of manual sells, funding rows,
//! container strategies and of trades the report no longer knows are removed by startup retention.
//!
//! Everything here runs on the background executor: the replica scan, the catalog lookups and
//! the trim itself all block. The one thing taken on the UI thread is the [`CleanupContext`] —
//! handles and settings, not state.

pub(crate) mod startup;

use std::collections::HashMap;
use std::sync::Arc;

use crate::Backend;
use moon_core::db::ReportAxis;
use moon_core::db::ReportStamp;
use moon_core::db::tape_owners::{TapeOwner, read_tape_owners};
use moon_core::db::tuner::ticks::{ORDER_WAIT_CAP_MS, model_window_at, order_open_at};
use moon_core::market::MarketDataSource;
use moon_core::market::trade_replay::trade_cache::{self, Inventory, KeepMap, TrimReport};
use moon_core::market::trade_replay::{
    Coverage, ReplayWindow, long_position_ms, margin_ms, worker,
};
use moon_core::symbol::Exchange;

/// Retention counts kept for the startup log and owner-resolution diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::settings) struct CleanupPreview {
    pub report: TrimReport,
    /// Report rows inside the file's time range that were let claim.
    pub claimants: usize,
    /// Of those, rows whose core is not connected and that were matched by the coin's name.
    pub by_name: usize,
    /// Of those, rows that matched no market of the file either way; their prints, if any,
    /// count as nobody's.
    pub unresolved: usize,
}

/// The tuner's deals whose claim meets tape the file holds, for the Storage tab's facts line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::settings) struct TapeCount {
    /// Deals with at least one held print inside their claim.
    pub with_tape: usize,
    /// Of those, positions longer than the long-position threshold: their claim is two ends.
    pub long: usize,
    /// Distinct markets whose held-spans read did not answer; both counts are lower bounds.
    pub incomplete_markets: usize,
}

/// How far past the file's own time range the replica is read, in seconds, before the margin's
/// own reach is added ([`Margins::reach_s`]): a core's clock offset moves a row's stamps
/// against the file's UTC ones by at most that.
const OWNER_SLACK_S: i64 = moon_core::db::MAX_OFFSET_SECS as i64;

/// The handles and settings a pass needs, taken on the UI thread.
#[derive(Clone)]
pub(super) struct CleanupContext {
    source: MarketDataSource,
    /// `core_uid` to the core's quote (`ServerConfig::market`), for spelling a coin into a
    /// market — the connected cores' and the offline ones' alike.
    quotes: HashMap<u64, String>,
    /// The session's core clocks, for lifting the report's stamps onto the file's UTC the way
    /// the trade window and the chart do. The tuner and the station lift through the replica's
    /// stored offsets instead ([`persisted_axis`]); a pass claims for both.
    axis: ReportAxis,
}

/// What a claim is sized with, read once per pass so every row of it is judged alike whatever
/// the Storage tab does meanwhile: the margin the tuner's fetch and the close-time capture ask
/// for (`[trade_replay] margin_s`, whose floor is the model's pad), and the length from which a
/// position is walked as its two ends.
#[derive(Clone, Copy, Debug)]
struct Margins {
    margin_ms: i64,
    long_position_ms: i64,
}

impl Margins {
    fn live() -> Self {
        Self {
            margin_ms: margin_ms(),
            long_position_ms: long_position_ms(),
        }
    }

    /// How far, in seconds, a row's claim can reach past its own stamps — the margin, and before
    /// the entry the entry order's life on top of it (`ORDER_WAIT_CAP_MS`), rounded up. A row
    /// that opened this much after the file's last print still claims prints inside the file, so
    /// the replica is read that much wider than the file's range. The order's life reaches only
    /// before the entry: on the other bound the reach is wider than any claim, and the few rows
    /// it adds claim nothing.
    fn reach_s(self) -> i64 {
        (self.margin_ms + ORDER_WAIT_CAP_MS).div_euclid(1_000) + 1
    }
}

impl CleanupContext {
    pub(super) fn of(backend: &Backend) -> Self {
        Self {
            source: backend.session.market_source(),
            quotes: backend
                .config
                .servers
                .iter()
                .map(|s| (s.uid, s.market.clone()))
                .collect(),
            axis: backend.report_axis(chrono_tz::UTC),
        }
    }

    /// Remove unclaimed tape and compact only a nonempty cut with enough filesystem room.
    /// Blocking; call from the background executor.
    pub(super) fn run(&self) -> anyhow::Result<CleanupPreview> {
        let cache = trade_cache::handle()
            .ok_or_else(|| anyhow::anyhow!("trade cache worker is not running"))?;
        self.run_with(&cache, &persisted_axis()?, owners_around)
    }

    /// The axes a pass claims on: the replica's stored offsets, and the session's where they
    /// lift differently — an offline core the session holds no offset for, or a trade from
    /// before the core's offset last changed.
    fn axes(&self, persisted: &ReportAxis) -> Vec<ReportAxis> {
        let mut axes = vec![persisted.clone()];
        if !self.axis.same_offsets(persisted) {
            axes.push(self.axis.clone());
        }
        axes
    }

    /// Require complete owner metadata before building or applying any retention map.
    fn run_with(
        &self,
        cache: &trade_cache::TradeCache,
        persisted: &ReportAxis,
        owners: impl FnOnce(i64, i64, Margins) -> anyhow::Result<Vec<TapeOwner>>,
    ) -> anyhow::Result<CleanupPreview> {
        let inventory = cache
            .inventory()
            .ok_or_else(|| anyhow::anyhow!("trade cache did not answer"))??;
        let Some((from_ms, to_ms)) = inventory.range_ms else {
            return Ok(CleanupPreview::default());
        };
        let margins = Margins::live();
        let owners = owners(from_ms, to_ms, margins)?;
        let claimants: Vec<&TapeOwner> = owners.iter().filter(|o| o.is_tunable()).collect();
        let (keep, mut preview) = build_keep(
            &inventory,
            &claimants,
            &self.axes(persisted),
            &self.quotes,
            margins,
            |owner| self.live_address(owner),
        );
        let report = cache
            .trim(Arc::new(keep))
            .ok_or_else(|| anyhow::anyhow!("trade cache did not answer"))??;
        // The tiles in memory still hold what the file just lost; a held-data query would
        // go on answering from them until a restart.
        worker::forget_tiles();
        log::info!(
            "[x] trades cleanup: {} claimant row(s), {} by name, {} unresolved; {} span(s) dropped, {} cut, {} print(s) / {} bytes gone of {}",
            preview.claimants,
            preview.by_name,
            preview.unresolved,
            report.spans_dropped,
            report.spans_cut,
            report.prints_dropped,
            report.bytes_dropped,
            report.bytes_total
        );
        preview.report = report;
        Ok(preview)
    }

    /// Count the tuner's deals with tape in the file, read the way [`Self::run`] claims: the same
    /// widened owner bounds, clocks and market resolution, one held-spans read per market.
    /// Blocking; call from the background executor.
    pub(super) fn tape_count(&self) -> anyhow::Result<TapeCount> {
        let cache = trade_cache::handle()
            .ok_or_else(|| anyhow::anyhow!("trade cache worker is not running"))?;
        let inventory = cache
            .inventory()
            .ok_or_else(|| anyhow::anyhow!("trade cache did not answer"))??;
        let Some((from_ms, to_ms)) = inventory.range_ms else {
            return Ok(TapeCount::default());
        };
        let margins = Margins::live();
        let owners = owners_around(from_ms, to_ms, margins)?;
        let claimants: Vec<&TapeOwner> = owners.iter().filter(|o| o.is_tunable()).collect();
        count_with_tape(
            &inventory,
            &claimants,
            &self.axes(&persisted_axis()?),
            &self.quotes,
            margins,
            |owner| self.live_address(owner),
            |(exchange, market)| {
                Ok(cache
                    .held_spans(exchange, market, from_ms, to_ms)
                    .map(Coverage::from_spans))
            },
        )
    }

    /// Where a row's prints live by the live catalog, the way a trade window resolves it;
    /// `None` when the core is not connected or its catalog does not spell the coin.
    fn live_address(&self, owner: &TapeOwner) -> Option<(String, String)> {
        let quote = self
            .quotes
            .get(&owner.core_uid)
            .map(String::as_str)
            .unwrap_or_default();
        let address = self.source.replay_address(owner.core_uid).ok()?;
        let market = self
            .source
            .resolve_market(owner.core_uid, quote, &owner.coin)?;
        Some((address.exchange_key, market))
    }
}

/// The replica's stored core offsets — the axis the tuner's deal read lifts its windows on.
/// Fails closed: a pass that cannot tell which tape the tuner asks for removes none of it.
fn persisted_axis() -> anyhow::Result<ReportAxis> {
    let conn = moon_core::db::open_reader().map_err(|e| anyhow::anyhow!("{e}"))?;
    ReportAxis::load(&conn, chrono_tz::UTC).map_err(|e| anyhow::anyhow!("{e}"))
}

/// The replica's rows that can claim prints of a file spanning `from_ms..=to_ms`: read wider by
/// the clock slack and the margin's reach.
fn owners_around(from_ms: i64, to_ms: i64, margins: Margins) -> anyhow::Result<Vec<TapeOwner>> {
    let slack_s = OWNER_SLACK_S + margins.reach_s();
    read_tape_owners(
        from_ms.div_euclid(1_000) - slack_s,
        to_ms.div_euclid(1_000) + slack_s,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Each row's markets, resolved once per distinct `(core, coin)`: the catalog's answer, or the
/// coin's name against the file's markets.
struct Resolver<'a> {
    by_name: NameIndex,
    quotes: &'a HashMap<u64, String>,
    addresses: Addresses,
}

impl<'a> Resolver<'a> {
    fn new(inventory: &Inventory, quotes: &'a HashMap<u64, String>) -> Self {
        Self {
            by_name: NameIndex::of(inventory),
            quotes,
            addresses: HashMap::new(),
        }
    }

    /// The row's markets and whether the name stood in for the catalog.
    fn resolve(
        &mut self,
        owner: &TapeOwner,
        live: &mut impl FnMut(&TapeOwner) -> Option<(String, String)>,
    ) -> &(Vec<(String, String)>, bool) {
        let (by_name, quotes) = (&self.by_name, self.quotes);
        self.addresses
            .entry((owner.core_uid, owner.coin.clone()))
            .or_insert_with(|| match live(owner) {
                Some(key) => (vec![key], false),
                None => {
                    let quote = quotes
                        .get(&owner.core_uid)
                        .map(String::as_str)
                        .unwrap_or_default();
                    (by_name.matches(&owner.coin, quote), true)
                }
            })
    }
}

/// One row's distinct windows across `axes`: two axes that lift it alike claim it once.
fn claim_windows(axes: &[ReportAxis], owner: &TapeOwner, margins: Margins) -> Vec<ReplayWindow> {
    let mut windows: Vec<ReplayWindow> = Vec::new();
    for axis in axes {
        if let Some(window) = claim_window(stamps(axis, owner), margins)
            && !windows.contains(&window)
        {
            windows.push(window);
        }
    }
    windows
}

/// The tuner's own window for one claim (`model_window_at`): from the entry order's creation
/// where the row carries it, split by the pass's own threshold, not one read a moment later.
fn claim_window(stamp: ClaimStamps, margins: Margins) -> Option<ReplayWindow> {
    model_window_at(
        order_open_at(stamp.buy_ms, stamp.buy_set_ms),
        stamp.buy_ms,
        stamp.close_ms,
        margins.margin_ms,
        margins.long_position_ms,
    )
}

/// The union of every claimant's stretches, per market of the file.
///
/// Args:
///     inventory: The file's markets, for the name fallback.
///     claimants: The rows let claim.
///     axes: The core clocks a claim is lifted on, each claiming its own window.
///     quotes: Each core's quote, for spelling a coin by name.
///     margins: The margins in force.
///     live: The catalog's answer for a row, or `None` — see [`CleanupContext::live_address`].
fn build_keep(
    inventory: &Inventory,
    claimants: &[&TapeOwner],
    axes: &[ReportAxis],
    quotes: &HashMap<u64, String>,
    margins: Margins,
    mut live: impl FnMut(&TapeOwner) -> Option<(String, String)>,
) -> (KeepMap, CleanupPreview) {
    let mut resolver = Resolver::new(inventory, quotes);
    let mut keep = KeepMap::new();
    let mut preview = CleanupPreview {
        claimants: claimants.len(),
        ..Default::default()
    };
    for owner in claimants {
        let (keys, by_name_only) = resolver.resolve(owner, &mut live);
        if keys.is_empty() {
            preview.unresolved += 1;
            continue;
        }
        if *by_name_only {
            preview.by_name += 1;
        }
        // The lead the tuner fetched for the order's life is claimed with the trade.
        for window in claim_windows(axes, owner, margins) {
            let focus = window.focus_spans();
            for key in keys.iter() {
                let coverage = keep.entry(key.clone()).or_insert_with(Coverage::none);
                for &span in focus.spans() {
                    coverage.add(span);
                }
            }
        }
    }
    (keep, preview)
}

/// The deals of `claimants` whose claim meets a held print, counted once however many markets
/// or clocks match, and of them the long ones.
///
/// Args:
///     inventory: The file's markets, for the name fallback.
///     claimants: The rows let claim.
///     axes: The core clocks a claim is lifted on, each claiming its own window.
///     quotes: Each core's quote, for spelling a coin by name.
///     margins: The margins in force.
///     live: The catalog's answer for a row, or `None`.
///     held: What the file holds of one market; `None` when its read timed out, read once.
fn count_with_tape(
    inventory: &Inventory,
    claimants: &[&TapeOwner],
    axes: &[ReportAxis],
    quotes: &HashMap<u64, String>,
    margins: Margins,
    mut live: impl FnMut(&TapeOwner) -> Option<(String, String)>,
    mut held: impl FnMut(&(String, String)) -> anyhow::Result<Option<Coverage>>,
) -> anyhow::Result<TapeCount> {
    let mut resolver = Resolver::new(inventory, quotes);
    let mut stored: HashMap<(String, String), Coverage> = HashMap::new();
    let mut out = TapeCount::default();
    for owner in claimants {
        let keys = resolver.resolve(owner, &mut live).0.clone();
        let (mut with_tape, mut long) = (false, false);
        for window in claim_windows(axes, owner, margins) {
            let focus = window.focus_spans();
            for key in &keys {
                if !stored.contains_key(key) {
                    let coverage = held(key)?.unwrap_or_else(|| {
                        out.incomplete_markets += 1;
                        Coverage::none()
                    });
                    stored.insert(key.clone(), coverage);
                }
                if !focus.clip(&stored[key]).is_empty() {
                    with_tape = true;
                    long |= window.close_ms - window.open_ms > window.long_position_ms;
                }
            }
        }
        if with_tape {
            out.with_tape += 1;
            out.long += usize::from(long);
        }
    }
    Ok(out)
}

/// Per distinct `(core, coin)`: the markets it claims on, and whether the name had to stand in
/// for the catalog.
type Addresses = HashMap<(u64, String), (Vec<(String, String)>, bool)>;

/// One claim's stamps on true UTC: the entry, the exit, and the entry order's creation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClaimStamps {
    buy_ms: i64,
    close_ms: i64,
    buy_set_ms: Option<i64>,
}

/// The row's stamps on the file's clock: through the core's measured offset, the way the trade
/// window, the close-time capture and the tuner's deal read (`db::tuner::ticks::deals`) stamp
/// their requests. The order's creation is a millisecond stamp on the entry's clock and moves
/// with it.
fn stamps(axis: &ReportAxis, owner: &TapeOwner) -> ClaimStamps {
    let (buy_ms, close_ms) = axis.stamp_pair_to_utc_ms(owner.buy, owner.close, owner.core_uid);
    let raw_buy_ms = match owner.buy {
        ReportStamp::Millis(buy) => Some(buy),
        ReportStamp::Seconds(_) => None,
    };
    ClaimStamps {
        buy_ms,
        close_ms,
        buy_set_ms: owner
            .buy_set_ms
            .zip(raw_buy_ms)
            .map(|(set, raw)| set + (buy_ms - raw)),
    }
}

/// The file's markets by their upper-cased name, for the name fallback.
struct NameIndex {
    markets: HashMap<String, Vec<(String, String)>>,
}

impl NameIndex {
    fn of(inventory: &Inventory) -> Self {
        let mut markets: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for key in &inventory.keys {
            markets
                .entry(key.1.to_ascii_uppercase())
                .or_default()
                .push(key.clone());
        }
        Self { markets }
    }

    /// Every market of the file one of the coin's spellings names, on any exchange: the coin
    /// as stored (old rows hold the full market), then each family's own shape. Wider than the
    /// catalog would answer — a spelling shared by two exchanges claims on both — and that is
    /// the safe direction: a claim too wide keeps prints, never loses them.
    fn matches(&self, coin: &str, quote: &str) -> Vec<(String, String)> {
        // The setting is a bare quote (`USDT`) on most cores; `resolve_quote_on` reads a bare
        // quote as a market with no base and answers nothing, so the setting itself stands
        // in then, upper-cased like every spelling here.
        let quote = match moon_core::symbol::resolve_quote_on(quote, Exchange::Unknown) {
            resolved if resolved.is_empty() => quote.to_ascii_uppercase(),
            resolved => resolved,
        };
        let mut out: Vec<(String, String)> = Vec::new();
        let candidates = std::iter::once(coin.to_string()).chain(
            moon_core::symbol::parse::market_names_for(coin, &quote, Exchange::Unknown),
        );
        for candidate in candidates {
            if let Some(keys) = self.markets.get(&candidate.to_ascii_uppercase()) {
                for key in keys {
                    if !out.contains(key) {
                        out.push(key.clone());
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
