//! The one background thread that fetches trade replays, and the request protocol reaching it.
//!
//! # Why exactly one thread
//!
//! Not for throughput — for RESTRAINT. A user walking down a report opens rows faster than any of
//! these endpoints wants to be asked, and a thread per request would turn that into a burst that
//! spends the user's IP budget. One worker serialises every call, which is also the only thing
//! that makes [`super::gate::ReplayGate::pace`]'s floor a real process-wide floor rather than a
//! per-caller hope. `moon-core` has no async runtime, so this is a plain blocking thread and an
//! `mpsc` pair, exactly like the kline cache and the report valuation worker beside it.
//!
//! # Two caches, and both are load-bearing
//!
//! Bars go into the SHARED `klines.sqlite` under the real exchange key, because a one-minute bar
//! fetched here is indistinguishable from one the recorder wrote and the rest of the application
//! benefits from it. Whole OUTCOMES additionally go into a small in-memory ring owned by this
//! worker, keyed by the exact question asked. That second cache is what actually satisfies "the
//! second open of the same trade costs nothing": the SQLite cache cannot hold ticks at all, and
//! nothing else in the process remembers that a given window was already answered.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::gate::ReplayGate;
use super::venue_caps::{bybit_category, kline_route};
use super::{
    pages, rest, ReplayWindow, TradeReplayEmpty, TradeReplayFailure, TradeReplayOutcome,
    TradeReplaySeries, TradeReplaySource,
};
use crate::market::candles::ChartCandle;
use crate::market::kline_cache::{KlineCache, MergeItem};
use crate::market::source::ReplayAddress;

/// Milliseconds per one-minute bar.
const BAR_MS: i64 = 60_000;

/// Widest gap in cached bars still counted as coverage rather than a hole.
///
/// A quiet market legitimately has minutes with no trade at all, so demanding a bar per minute
/// would send every window to the network forever. Three bars is wide enough for a thin market and
/// narrow enough that a genuinely interrupted fetch is still recognised as incomplete.
const MAX_GAP_BARS: i64 = 3;

/// Ceiling on the whole job, however many pages it takes.
///
/// The HTTP client bounds each REQUEST at fifteen seconds, which says nothing about a paginated
/// job: a wide window can be many requests, and without this a single slow venue would hold the
/// one worker — and therefore every later window — for minutes. On expiry the caller is told the
/// fetch is transient, which is true and retryable.
const JOB_DEADLINE: Duration = Duration::from_secs(45);

/// How many answered windows the in-memory outcome cache remembers.
///
/// Small on purpose: this exists so a reopen costs nothing, not to be a history store. Each entry
/// holds one bounded window's rows.
const OUTCOME_CACHE_LEN: usize = 8;

/// What one answered question is remembered as.
///
/// An authoritative EMPTY is an answer too, and a valuable one: a delisted or halted market
/// answers empty every time, so refetching it on each reopen spends the host's budget to learn
/// something already known.
#[derive(Clone, Debug)]
enum Remembered {
    /// Rows to draw.
    Ready(TradeReplaySeries),
    /// The venue answered and its answer held nothing in this window.
    Empty,
}

/// What identifies one replay question, so an identical one is recognised on reopen.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OutcomeKey {
    /// Host the rows came from, which pins the venue and the market kind together.
    host: &'static str,
    /// Exchange-native market name.
    market: String,
    /// Window the rows cover.
    from_ms: i64,
    to_ms: i64,
}

/// One replay request.
pub struct TradeReplayRequest {
    /// Exchange addressing resolved from the live source before the request was queued.
    pub address: ReplayAddress,
    /// Exchange-native market name, as the core reports it.
    pub market: String,
    /// The window to cover.
    pub window: ReplayWindow,
    /// Stable discriminator for the series this produces, so two open windows never collide.
    pub identity: u64,
    /// Set by the requester when its window closes; checked between pages.
    pub cancel: Arc<AtomicBool>,
    /// Where the answer goes. A dead receiver is normal and is not an error.
    pub reply: Sender<TradeReplayOutcome>,
}

/// Handle to the process-wide replay worker.
struct Worker {
    tx: Sender<TradeReplayRequest>,
}

/// The one worker, started on the first request and never stopped.
static WORKER: OnceLock<Worker> = OnceLock::new();

/// Queue one replay request, starting the worker if this is the first.
///
/// Returns immediately. The answer arrives on the request's own reply channel, or never, if the
/// requester dropped its receiver first — which is exactly what a closed window looks like.
///
/// Args:
///     request: The window to fetch and where to answer.
pub fn request(request: TradeReplayRequest) {
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<TradeReplayRequest>();
        let spawned = std::thread::Builder::new()
            .name("trade-replay".into())
            .spawn(move || run(&rx));
        if let Err(error) = &spawned {
            // Nothing to fall back to, and the caller's own timeout is what will surface it; say
            // so once rather than failing silently.
            log::warn!("[x] trade-replay worker did not start: {error}");
        }
        Worker { tx }
    });
    // A send failure means the worker thread is gone, which only happens if it never started.
    if worker.tx.send(request).is_err() {
        log::warn!("[x] trade-replay request dropped: worker is not running");
    }
}

/// Worker loop: one request at a time, forever.
///
/// Args:
///     rx: Queue of pending requests.
fn run(rx: &Receiver<TradeReplayRequest>) {
    let agent = rest::agent();
    let gate = ReplayGate::new();
    let cache: Mutex<VecDeque<(OutcomeKey, Remembered)>> = Mutex::new(VecDeque::new());
    while let Ok(request) = rx.recv() {
        // A window that closed while its request sat in the queue costs nothing at all: this is
        // the cheapest of the three cancellation guards and the only one that prevents the work.
        if request.cancel.load(Ordering::Relaxed) {
            continue;
        }
        let outcome = serve(&agent, &gate, &cache, &request);
        // The receiver is gone whenever the window closed mid-fetch. Normal, not an error.
        let _ = request.reply.send(outcome);
    }
}

/// Answer one request: memory cache, then SQLite cache, then the network.
///
/// The order is fixed and each step earns its place. The memory cache answers a reopen with no
/// work at all. The SQLite cache answers without a request, which matters most precisely when the
/// gate is refusing — a user in backoff still sees the real chart rather than a countdown. Only
/// then is a permit taken.
///
/// Args:
///     agent: Shared HTTP client.
///     gate: Per-host pacing and backoff.
///     cache: In-memory outcome ring.
///     request: The request being served.
///
/// Returns:
///     The outcome to send back.
fn serve(
    agent: &ureq::Agent,
    gate: &ReplayGate,
    cache: &Mutex<VecDeque<(OutcomeKey, Remembered)>>,
    request: &TradeReplayRequest,
) -> TradeReplayOutcome {
    let venue = request.address.venue;
    let Some(route) = kline_route(venue) else {
        return TradeReplayOutcome::Empty(TradeReplayEmpty::NoEndpoint { brand: venue.brand });
    };
    let key = OutcomeKey {
        host: route.host(),
        market: request.market.clone(),
        from_ms: request.window.from_ms,
        to_ms: request.window.to_ms,
    };
    match remember_lookup(cache, &key, request.identity) {
        Some(Remembered::Ready(series)) => return TradeReplayOutcome::Ready(series),
        Some(Remembered::Empty) => {
            return TradeReplayOutcome::Empty(TradeReplayEmpty::NoDataInWindow)
        }
        None => {}
    }

    // The SQLite cache is read first and unconditionally: it costs no request and is not gated.
    if let Some(rows) = read_cached_bars(request.address.cache.as_ref(), request) {
        let series = compose(request, venue, rows);
        remember_store(cache, key, Remembered::Ready(series.clone()));
        return TradeReplayOutcome::Ready(series);
    }

    if let Err(retry_in_s) = gate.claim(route.host(), Instant::now()) {
        return TradeReplayOutcome::Failed(TradeReplayFailure::RateLimited { retry_in_s });
    }
    let category = bybit_category(venue, &request.market);
    let deadline = Instant::now() + JOB_DEADLINE;
    let mut rows: Vec<ChartCandle> = Vec::new();
    // Whether every page of the window was actually fetched. A cancelled run keeps its rows — they
    // were paid for — but must NOT be remembered as this window's answer.
    let mut complete = true;
    for (from_ms, to_ms) in pages(request.window, BAR_MS, route.max_rows()) {
        if request.cancel.load(Ordering::Relaxed) {
            // The window is gone, or a Retry superseded this request. Whatever was fetched is
            // still worth merging into the shared cache, so fall through rather than discarding a
            // page already paid for.
            complete = false;
            break;
        }
        if Instant::now() >= deadline {
            return TradeReplayOutcome::Failed(TradeReplayFailure::Transient {
                diagnostic: format!("trade replay exceeded {}s", JOB_DEADLINE.as_secs()),
            });
        }
        gate.pace(route.host());
        match rest::fetch_klines(
            agent,
            route,
            &request.market,
            category,
            from_ms,
            to_ms,
            route.max_rows(),
        ) {
            Ok(page) => rows.extend(page),
            Err(rest::FetchError::UnknownSymbol) => {
                // The venue ANSWERED; it simply does not list this symbol. Holding the host's
                // claim here would make one bad market throttle every other market on that host,
                // and five of them would push it to the backoff ceiling for nothing.
                gate.clear(route.host());
                return TradeReplayOutcome::Failed(TradeReplayFailure::UnknownSymbol);
            }
            Err(rest::FetchError::Transient(diagnostic)) => {
                return TradeReplayOutcome::Failed(TradeReplayFailure::Transient { diagnostic })
            }
        }
    }
    // The venue answered, so its refusal history is stale whatever the rows say.
    gate.clear(route.host());
    write_cached_bars(request.address.cache.as_ref(), request, &rows);
    if rows.is_empty() {
        // Only a COMPLETE run may be remembered, empty or not: a cancelled one proves nothing
        // about the window it never finished reading.
        if complete {
            remember_store(cache, key, Remembered::Empty);
        }
        return TradeReplayOutcome::Empty(TradeReplayEmpty::NoDataInWindow);
    }
    let series = compose(request, venue, rows);
    // Only a COMPLETE run may be remembered. Pages are issued left to right, so a cancelled run
    // holds the window's left-hand prefix — typically missing exactly the bars around the exit —
    // and the in-memory ring, unlike the SQLite path, has no coverage re-check to catch that on
    // read. Storing it would serve a silently truncated chart as `Ready` for the life of the
    // entry. The SQLite merge above is unaffected: `cache_covers` re-checks it on every read.
    if complete {
        remember_store(cache, key, Remembered::Ready(series.clone()));
    }
    TradeReplayOutcome::Ready(series)
}

/// Build the frozen series one request answers with.
///
/// Args:
///     request: The request being served.
///     venue: Venue the rows came from.
///     rows: Bars in ascending open time.
///
/// Returns:
///     The series to hand the chart.
fn compose(
    request: &TradeReplayRequest,
    venue: crate::venue::Venue,
    rows: Vec<ChartCandle>,
) -> TradeReplaySeries {
    TradeReplaySeries {
        source: TradeReplaySource::Klines1m,
        venue,
        window: request.window,
        tf_ms: BAR_MS,
        candles: rows,
        ticks: Vec::new(),
        identity: request.identity,
    }
}

/// Look one window up in the in-memory outcome ring.
///
/// Args:
///     cache: The ring.
///     key: The question being asked.
///     identity: Discriminator the caller expects on the series it gets back.
///
/// Returns:
///     A ready series, or `None`.
fn remember_lookup(
    cache: &Mutex<VecDeque<(OutcomeKey, Remembered)>>,
    key: &OutcomeKey,
    identity: u64,
) -> Option<Remembered> {
    let cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let hit = cache.iter().find(|(k, _)| k == key)?;
    Some(match hit.1.clone() {
        Remembered::Ready(mut series) => {
            // The identity belongs to the WINDOW that asked, not to the cached rows: two windows
            // on the same trade must not share a chart revision, or the second would be told
            // nothing changed and would draw nothing.
            series.identity = identity;
            Remembered::Ready(series)
        }
        Remembered::Empty => Remembered::Empty,
    })
}

/// Remember one answered window, evicting the oldest when full.
///
/// Args:
///     cache: The ring.
///     key: The question that was answered.
///     answer: What the venue said.
fn remember_store(
    cache: &Mutex<VecDeque<(OutcomeKey, Remembered)>>,
    key: OutcomeKey,
    answer: Remembered,
) {
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|(k, _)| *k != key);
    cache.push_back((key, answer));
    while cache.len() > OUTCOME_CACHE_LEN {
        cache.pop_front();
    }
}

/// Read the window's bars from the shared kline cache, when it covers the window.
///
/// Args:
///     cache: The open cache, if the terminal supplied one.
///     request: The request being served.
///
/// Returns:
///     Bars covering the whole window, or `None` to fall through to the network.
fn read_cached_bars(
    cache: Option<&KlineCache>,
    request: &TradeReplayRequest,
) -> Option<Vec<ChartCandle>> {
    let cache = cache?;
    // `read_range` answers `None` for a TIMEOUT and `Some(vec![])` for an authoritative empty, and
    // the two must never be conflated: folding a timeout into "the cache holds nothing" would send
    // a window to the network that the cache could have answered. One retry, then fall through.
    let rows = match cache.read_range(
        &request.address.exchange_key,
        &request.market,
        1,
        request.window.from_ms,
        request.window.to_ms,
    ) {
        Some(rows) => rows,
        None => {
            std::thread::sleep(Duration::from_millis(300));
            cache.read_range(
                &request.address.exchange_key,
                &request.market,
                1,
                request.window.from_ms,
                request.window.to_ms,
            )?
        }
    };
    match super::cache_covers(&rows, request.window, BAR_MS, MAX_GAP_BARS) {
        true => Some(rows),
        false => None,
    }
}

/// Merge freshly fetched bars into the shared kline cache.
///
/// Written under the REAL exchange key rather than a private one: these are genuine exchange
/// one-minute bars, indistinguishable from the recorder's, so every core on that venue benefits
/// and the second open of this trade costs no request even after a restart.
///
/// Args:
///     cache: The open cache, if the terminal supplied one.
///     request: The request being served.
///     rows: Bars to store; an empty set writes nothing.
fn write_cached_bars(
    cache: Option<&KlineCache>,
    request: &TradeReplayRequest,
    rows: &[ChartCandle],
) {
    let (Some(cache), false) = (cache, rows.is_empty()) else {
        return;
    };
    cache.merge_batch(vec![MergeItem {
        exchange: request.address.exchange_key.clone(),
        market: request.market.clone(),
        kind_min: 1,
        rows: rows.to_vec(),
    }]);
}
