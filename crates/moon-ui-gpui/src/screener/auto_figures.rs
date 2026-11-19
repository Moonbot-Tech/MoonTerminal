//! The Screener's auto-figures column (`docs-internal/AUTO_FIGURES.md` §13.4): what one press of
//! "Calculate" computes, how its answers are kept, and how the column shows, sorts and filters them.
//!
//! # What runs, and when
//!
//! One press scans every market of the Screener's current publication with the drawn rows of
//! Main's auto-figure set, through `moon_chart::auto_figures::scan` — the detectors, parameters and
//! event rules the chart itself uses, so a coin the column flags shows the same lines when it is
//! opened on Main (LinKvo, 19.11.2026: not a standing recount of every market, a column with a
//! button and a filter). Nothing here runs on a timer or per frame, and the answers stay as they
//! are until the next press.
//!
//! # Where the bars come from
//!
//! ONLY the local 5-minute kline cache, which the background recorder keeps for every market (15
//! days): real candles from the trade stream while the terminal runs, and the core's own 5-minute
//! ring where no real candle exists, written each minute — so the last ~41 hours of every market
//! the core carries are there without holes, the ring's rows with only their range (the scan
//! gives them no close, `moon_chart::auto_figures::scan`). The cache is the ONE source: a market
//! the recorder has not reached yet (the first minute after start) is honestly short of history.
//! Never a core: a history request makes the core fetch klines from the exchange, and that is how
//! a user's API key got banned once.
//!
//! Sides and distances are measured from the market's live price as the Screener's own row holds
//! it at the press (its ask), and from the cache's last bar only where the row has none. The exchange key a market's rows are read under is the one the chart reads its own
//! cache rows under (`MarketDataSource::kline_address`, the same call as
//! `panels::chart::auto_figures`), so the two see the same rows. A market with no address is
//! "no cache" in its own cell, not a failure of the run.
//!
//! # Once per market
//!
//! Screener rows are per core, and in per-core mode several providers sit on one exchange. A market
//! is read and scanned once per (exchange key, market), and every (provider, market) that resolves
//! to it takes that one answer; the rows of the cores under a provider read it by (provider,
//! market).
//!
//! # Slices
//!
//! The work runs in slices of about [`SLICE`] on the background executor. Between two slices the
//! window takes the answers so far — that is the progress the button shows — and a new press
//! drops the run at its next slice boundary (see `ScanBook::begin`).

use std::collections::hash_map::Entry as MapEntry;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::SharedString;
use moon_chart::auto_figures::events::bars_read;
use moon_chart::auto_figures::scan::{
    BEST_ORDER, ScanAnswer, ScanFilter, ScanResult, ScanRow, event_rank, scan_market,
};
use moon_core::market::ChartCandle;
use moon_core::market::MarketDataSource;
use moon_core::market::kline_cache::KlineCache;
use moon_core::session::CoreId;
use moon_core::util::fmt;
use moon_ui::{MoonDataCell, MoonTone};

use super::table::Entry;

/// Key of the auto-figures column — stable snake case, stored in the layout's column lists.
pub(super) const AUTO_FIGURES_KEY: &str = "auto_figures";

/// How long one background slice of the scan runs before handing its answers to the window.
///
/// Long enough that a run of several hundred markets repaints the window about ten times a second
/// at most, short enough that the progress moves. A soft budget: it is checked after each market,
/// so a slice ends with the first market read past it (a cache read waits at most the cache's own
/// read timeout), and the first slice also resolves every target's address. A second press does
/// not wait for it — the new run starts at once, and the old one stops at this boundary.
pub(super) const SLICE: Duration = Duration::from_millis(100);

/// The cache kind the scan reads: 5-minute rows, the only kind recorded for every market.
const BASE_MIN: u32 = 5;

/// One market's answer as the column keeps it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum MarketScan {
    /// The terminal has no cache address for the market: no kline cache open, or no exchange key
    /// known for it. Definitive for this press.
    NoCache,
    /// The cache read did not happen (the worker was busy, full or gone) — not "no history".
    Unread,
    /// The scan ran on what the cache holds.
    Scanned(ScanAnswer),
}

/// One answer as a slice hands it over: the provider, the market, and the answer.
pub(super) type Answer = (CoreId, String, MarketScan);

/// The answers of the latest press, by provider and market.
///
/// Lives on the Screener window for its session and is never written to disk. A press starts a
/// new run ([`Self::begin`]), which drops every earlier answer, and only the slices of THAT run land
/// ([`Self::apply`]): a late slice of a run the user restarted is recognised by its number and
/// dropped.
#[derive(Debug, Clone, Default)]
pub(super) struct ScanBook {
    /// The current run's number; zero before the first press.
    run: u64,
    /// The reference time of the current run, Unix ms — what decides which bars were closed, and
    /// what the column's header prints. `None` before the first press.
    as_of_ms: Option<i64>,
    /// (provider, market) pairs the current run answers, and how many it has so far.
    total: usize,
    done: usize,
    /// Answers by provider, then market: looked up with a borrowed market name per visible row.
    markets: HashMap<CoreId, HashMap<String, MarketScan>>,
}

impl ScanBook {
    /// Start a new run of `total` (provider, market) pairs at `as_of_ms`, dropping every earlier
    /// answer; returns the run's number, which its slices carry to [`Self::apply`].
    pub(super) fn begin(&mut self, as_of_ms: i64, total: usize) -> u64 {
        self.run = self.run.wrapping_add(1);
        self.as_of_ms = Some(as_of_ms);
        self.total = total;
        self.done = 0;
        self.markets.clear();
        self.run
    }

    /// Take one slice's answers when they belong to the current run; `false` (and nothing taken)
    /// for a slice of an earlier one.
    pub(super) fn apply(&mut self, run: u64, answers: Vec<Answer>) -> bool {
        if run != self.run || self.as_of_ms.is_none() {
            return false;
        }
        for (provider, market, scan) in answers {
            if self
                .markets
                .entry(provider)
                .or_default()
                .insert(market, scan)
                .is_none()
            {
                self.done += 1;
            }
        }
        self.done = self.done.min(self.total);
        true
    }

    /// The current run's reference time, Unix ms; `None` before the first press.
    pub(super) fn as_of_ms(&self) -> Option<i64> {
        self.as_of_ms
    }

    /// `(answered, total)` while the current run is still being worked off; `None` before the
    /// first press and once every pair is answered.
    pub(super) fn progress(&self) -> Option<(usize, usize)> {
        (self.as_of_ms.is_some() && self.done < self.total).then_some((self.done, self.total))
    }

    /// The answer for `market` under `provider`, if the current run has given one.
    pub(super) fn get(&self, provider: CoreId, market: &str) -> Option<MarketScan> {
        self.markets.get(&provider)?.get(market).copied()
    }
}

/// The auto-figures filter as the row selection applies it: the latest answers and the choice.
#[derive(Clone)]
pub(super) struct ScanQuery {
    book: Rc<ScanBook>,
    filter: ScanFilter,
}

impl ScanQuery {
    pub(super) fn new(book: Rc<ScanBook>, filter: ScanFilter) -> Self {
        Self { book, filter }
    }

    /// Whether `entry` passes the filter.
    ///
    /// `Any` passes every row, answered or not. Any other choice passes only a row whose market
    /// was scanned and whose answer matches (`ScanAnswer::matches`): a market with no cache, an
    /// unread one, one the run has not reached yet — and every row before the first press — is
    /// not a squeeze, a breakout or an approach the scan found, so it is not shown as one. The
    /// window says why an empty table is empty (`screener.af_press_calculate`).
    pub(super) fn admits(&self, entry: &Entry) -> bool {
        matches!(self.filter, ScanFilter::Any)
            || matches!(
                self.book.get(entry.row.provider, &entry.row.market),
                Some(MarketScan::Scanned(answer)) if answer.matches(&self.filter)
            )
    }

    /// Sort `order` (indices into `entries`) by the column; see [`sort_by_scan`].
    pub(super) fn sort(&self, entries: &[Entry], order: &mut [usize], desc: bool) {
        sort_by_scan(&self.book, entries, order, desc);
    }
}

/// Where one answer sorts in the column, ascending: the best events first by
/// [`event_rank`], then by the distance to the nearest line, then the markets with a line and no
/// event, then those scanned with nothing, then the ones that could not say anything CURRENT — a
/// cache that lags behind now (`ScanAnswer::cache_lag_ms`: whatever it found is of the moment the
/// cache stops), short of history, unread, without a cache — and last the markets the run has not
/// answered.
///
/// The distance is an absolute value, so its bit pattern orders like the number (an absent line
/// is `+inf`, after every real one).
fn rank(scan: Option<MarketScan>) -> (u8, usize, u64) {
    let Some(scan) = scan else {
        return (6, 0, 0);
    };
    let answer = match scan {
        MarketScan::Scanned(answer) => answer,
        MarketScan::Unread => return (4, 0, 0),
        MarketScan::NoCache => return (5, 0, 0),
    };
    let distance = answer
        .nearest
        .map_or(f64::INFINITY, |line| line.distance_pct.abs())
        .to_bits();
    if answer.cache_lag_ms.is_some() {
        return (3, 0, 0);
    }
    match answer.best {
        Some(best) => (0, event_rank(best.event), distance),
        None if answer.nearest.is_some() => (1, 0, distance),
        // After a lagging cache, which at least read bars.
        None if answer.short_history => (3, 1, 0),
        None => (2, 0, 0),
    }
}

/// Sort `order` (indices into `entries`) by the auto-figures column: [`rank`] ascending, reversed
/// for `desc`. Stable, as every other column's sort is, so equal answers keep the publication's
/// order.
pub(super) fn sort_by_scan(book: &ScanBook, entries: &[Entry], order: &mut [usize], desc: bool) {
    let mut keyed: Vec<_> = order
        .iter()
        .map(|&index| {
            let row = &entries[index].row;
            (rank(book.get(row.provider, &row.market)), index)
        })
        .collect();
    keyed.sort_by(|a, b| {
        let ord = a.0.cmp(&b.0);
        if desc { ord.reverse() } else { ord }
    });
    for (slot, (_, index)) in order.iter_mut().zip(keyed) {
        *slot = index;
    }
}

/// The column's words, translated once per table build rather than per visible row per frame.
pub(super) struct ScanLabels {
    /// Event names in [`BEST_ORDER`] order, so an event's name is at its [`event_rank`].
    pub(super) events: Vec<SharedString>,
    /// Rows scanned, no event found.
    pub(super) nothing: SharedString,
    /// Too little cache to say anything.
    pub(super) short: SharedString,
    pub(super) no_cache: SharedString,
    pub(super) unread: SharedString,
    /// A cache that lags behind now, worded with its lag in ms — called only for such a market,
    /// which a current recorder never produces, so the per-row words above stay the common case.
    pub(super) stale: Box<dyn Fn(i64) -> String>,
}

impl ScanLabels {
    /// The labels of the current language; `event_name` turns an event into its translated name.
    pub(super) fn new(
        event_name: impl Fn(moon_core::config::auto_figures::AutoEvent) -> String,
        nothing: String,
        short: String,
        no_cache: String,
        unread: String,
        stale: impl Fn(i64) -> String + 'static,
    ) -> Self {
        Self {
            events: BEST_ORDER
                .iter()
                .map(|&event| event_name(event).into())
                .collect(),
            nothing: nothing.into(),
            short: short.into(),
            no_cache: no_cache.into(),
            unread: unread.into(),
            stale: Box::new(stale),
        }
    }
}

/// The column's text for one market, without its tone: the best event and the signed distance to
/// the nearest line (`Ложный пробой +0.42%`), the "nothing" word with the distance when a line
/// was kept but showed no event, or what kept the market from an answer — a cache that lags behind
/// now says so with its lag (`кэш отстаёт 3ч`) INSTEAD of its event, which is not current. `pending` is the mark of
/// a market the run has not reached; `—` before the first press and for a market the last run did
/// not cover (it appeared after the press).
pub(super) fn cell_text(scan: Option<MarketScan>, running: bool, labels: &ScanLabels) -> String {
    let distance = |answer: &ScanAnswer| {
        answer
            .nearest
            .and_then(|line| fmt::signed_pct(line.distance_pct, 2))
            .map(|(text, _)| text)
    };
    let with_distance = |label: &str, answer: &ScanAnswer| match distance(answer) {
        Some(d) => format!("{label} {d}"),
        None => label.to_string(),
    };
    match scan {
        None if running => "…".to_string(),
        None => "—".to_string(),
        Some(MarketScan::NoCache) => labels.no_cache.to_string(),
        Some(MarketScan::Unread) => labels.unread.to_string(),
        Some(MarketScan::Scanned(ScanAnswer {
            cache_lag_ms: Some(lag),
            ..
        })) => (labels.stale)(lag),
        Some(MarketScan::Scanned(answer)) => match answer.best {
            Some(best) => {
                let name = labels
                    .events
                    .get(event_rank(best.event))
                    .map_or("", |name| name.as_ref());
                with_distance(name, &answer)
            }
            None if answer.nearest.is_some() => with_distance(&labels.nothing, &answer),
            None if answer.short_history => labels.short.to_string(),
            None => labels.nothing.to_string(),
        },
    }
}

/// The column's cell for one market: [`cell_text`], a current event in the accent tone and
/// everything else — a lagging cache's included — muted.
pub(super) fn cell(scan: Option<MarketScan>, running: bool, labels: &ScanLabels) -> MoonDataCell {
    let tone = match scan {
        Some(MarketScan::Scanned(ScanAnswer {
            best: Some(_),
            cache_lag_ms: None,
            ..
        })) => MoonTone::Accent,
        _ => MoonTone::Muted,
    };
    MoonDataCell::text(cell_text(scan, running, labels)).tone(tone)
}

/// One (provider, market) of the publication, with a core of that provider to resolve the
/// market's cache address through, and the market's live price at the press.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ScanTarget {
    pub(super) provider: CoreId,
    pub(super) core: CoreId,
    pub(super) market: String,
    /// The publication's ask for the market, when it is a positive price: what the scan measures
    /// sides and distances from, instead of the cache's last bar.
    pub(super) price: Option<f64>,
}

/// Every (provider, market) of the publication once, in publication order. A provider's member
/// cores all list the same markets (the market half of a row is one per provider), so the first
/// core met answers for the provider.
pub(super) fn scan_targets(entries: &[Entry]) -> Vec<ScanTarget> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .filter(|entry| seen.insert((entry.row.provider, entry.row.market.as_str())))
        .map(|entry| ScanTarget {
            provider: entry.row.provider,
            core: entry.row.core,
            market: entry.row.market.clone(),
            price: Some(entry.row.ask).filter(|p| p.is_finite() && *p > 0.0),
        })
        .collect()
}

/// Where the scan reads its rows: the kline cache in the terminal, a map in the tests.
pub(super) trait ScanSource {
    /// The exchange key `market`'s cache rows are filed under, as the chart of `core` reads them;
    /// `None` when the terminal has no cache address for it.
    fn exchange(&self, core: CoreId, market: &str) -> Option<String>;

    /// The 5-minute rows of `market` with an open time in `[from_ms, to_ms]`; `None` when the read
    /// did not happen, as opposed to an empty answer.
    fn read_5m(
        &self,
        exchange: &str,
        market: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Option<Vec<ChartCandle>>;
}

/// The terminal's own source: the market source for addresses and its one kline cache for rows.
pub(super) struct CacheSource {
    source: MarketDataSource,
    cache: Option<KlineCache>,
}

impl CacheSource {
    pub(super) fn new(source: MarketDataSource) -> Self {
        let cache = source.kline_cache();
        Self { source, cache }
    }
}

impl ScanSource for CacheSource {
    fn exchange(&self, core: CoreId, market: &str) -> Option<String> {
        self.source
            .kline_address(core, market)
            .map(|(_, exchange)| exchange)
    }

    fn read_5m(
        &self,
        exchange: &str,
        market: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Option<Vec<ChartCandle>> {
        self.cache
            .as_ref()?
            .read_range(exchange, market, BASE_MIN, from_ms, to_ms)
    }
}

/// One market to read and scan, the providers that take its answer, and its live price (the first
/// target's that has one).
struct Unit {
    exchange: String,
    market: String,
    providers: Vec<CoreId>,
    price: Option<f64>,
}

/// One press's work, carried from slice to slice: the targets still to resolve, the markets to
/// scan, and the scan's reused buffers. Plain values only, so it moves to the background executor
/// and back.
pub(super) struct ScanWork {
    /// Targets whose cache address is not resolved yet — all of them until the first slice.
    pending: Vec<ScanTarget>,
    units: Vec<Unit>,
    next: usize,
    rows: Vec<ScanRow>,
    now_ms: i64,
    /// How far back from `now_ms` the cache is read, ms ([`read_span_ms`]).
    span_ms: i64,
    result: ScanResult,
}

impl ScanWork {
    /// The work of one press over `targets` with Main's scan `rows`, judged at `now_ms`.
    pub(super) fn new(targets: Vec<ScanTarget>, rows: Vec<ScanRow>, now_ms: i64) -> Self {
        let span_ms = read_span_ms(&rows);
        Self {
            pending: targets,
            units: Vec::new(),
            next: 0,
            rows,
            now_ms,
            span_ms,
            result: ScanResult::default(),
        }
    }

    /// Whether every target has its answer.
    pub(super) fn finished(&self) -> bool {
        self.pending.is_empty() && self.next >= self.units.len()
    }

    /// Run one slice: resolve the addresses on the first one, then read and scan markets until
    /// `budget` is spent (at least one market per slice, so a slice always moves the run).
    ///
    /// Counters: `autofigures_scan_markets` per market read and scanned, `autofigures_scan_us`
    /// once per slice.
    pub(super) fn step(&mut self, source: &dyn ScanSource, budget: Duration) -> Vec<Answer> {
        let timer = crate::diag::timer();
        let started = Instant::now();
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            self.plan(source, &mut out);
        }
        let Self {
            units,
            next,
            rows,
            now_ms,
            span_ms,
            result,
            ..
        } = self;
        while let Some(unit) = units.get(*next) {
            *next += 1;
            let read = source.read_5m(
                &unit.exchange,
                &unit.market,
                now_ms.saturating_sub(*span_ms),
                *now_ms,
            );
            let scan = match read {
                None => MarketScan::Unread,
                Some(bars) => {
                    scan_market(rows, &bars, *now_ms as f64, unit.price, result);
                    crate::diag::bump(&crate::diag::AUTOFIGURES_SCAN_MARKETS);
                    MarketScan::Scanned(result.answer())
                }
            };
            out.extend(
                unit.providers
                    .iter()
                    .map(|&provider| (provider, unit.market.clone(), scan)),
            );
            if started.elapsed() >= budget {
                break;
            }
        }
        crate::diag::record_us(&crate::diag::AUTOFIGURES_SCAN_US, timer);
        out
    }

    /// Resolve every pending target's cache address: a target without one is answered
    /// [`MarketScan::NoCache`] at once, the rest are grouped into one unit per (exchange key,
    /// market).
    fn plan(&mut self, source: &dyn ScanSource, out: &mut Vec<Answer>) {
        let mut at: HashMap<(String, String), usize> = HashMap::new();
        for target in std::mem::take(&mut self.pending) {
            let Some(exchange) = source.exchange(target.core, &target.market) else {
                out.push((target.provider, target.market, MarketScan::NoCache));
                continue;
            };
            match at.entry((exchange, target.market)) {
                MapEntry::Occupied(slot) => {
                    let unit = &mut self.units[*slot.get()];
                    unit.providers.push(target.provider);
                    unit.price = unit.price.or(target.price);
                }
                MapEntry::Vacant(slot) => {
                    let (exchange, market) = slot.key().clone();
                    slot.insert(self.units.len());
                    self.units.push(Unit {
                        exchange,
                        market,
                        providers: vec![target.provider],
                        price: target.price,
                    });
                }
            }
        }
    }
}

/// How far back the cache is read for `rows`, ms: the widest row's bars — the detector's
/// `bars_needed` or the events' `bars_read`, whichever is more — plus two of its bars, one for the
/// partial first bucket the resample drops and one for the bucket still forming.
///
/// Not a fixed 15 days: a set of M5 and H1 rows reads a few days, not the whole cache. A row
/// deeper than the cache keeps simply gets what is there and is marked short by the scan.
pub(super) fn read_span_ms(rows: &[ScanRow]) -> i64 {
    rows.iter()
        .map(|row| {
            let events = u32::try_from(bars_read(row.params.broken_policy())).unwrap_or(u32::MAX);
            let bars = row.params.bars_needed(row.tf).max(events);
            (i64::from(bars) + 2).saturating_mul(row.tf.ms())
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
