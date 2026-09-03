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
use super::venue_caps::{TradeRoute, bybit_category, kline_route, trade_route};
use super::{
    ReplayWindow, TradeReplayEmpty, TradeReplayFailure, TradeReplayOutcome, TradeReplaySeries,
    TradeReplaySource, pages, rest, time_slices,
};
use crate::feed::types::Tick;
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

/// Ceiling on the total number of ticks held across every remembered entry.
///
/// A single entry can carry up to [`TICK_BUDGET`] ticks, and [`OUTCOME_CACHE_LEN`] entries of
/// that size would let the ring's own memory dwarf the point ring it feeds. This bounds the ring
/// independently of its entry count: eviction runs oldest-first, exactly as the entry-count
/// eviction does, and never touches the entry that was just inserted, so one huge series is held
/// rather than immediately discarded and re-fetched.
const OUTCOME_CACHE_MAX_TICKS: usize = 2 * TICK_BUDGET;

/// Bounds MEMORY and the GPU point ring for one tick series.
///
/// Sits under the live chart's default `trades_limit` of 50 000 (`candles.rs:93`), so a tick
/// replay never asks the point ring for more than the main chart already draws.
pub(crate) const TICK_BUDGET: usize = 40_000;

/// Bounds WALL TIME on the single worker thread for one tick stage.
///
/// 60 pages at [`super::gate::ReplayGate::pace`]'s 100 ms floor plus a ~250 ms round trip is
/// about 21 s, inside [`JOB_DEADLINE`] with room for a slow venue.
const TICK_PAGE_BUDGET: usize = 60;

/// What one answered question is remembered as.
///
/// An authoritative EMPTY is an answer too, and a valuable one: a delisted or halted market
/// answers empty every time, so refetching it on each reopen spends the host's budget to learn
/// something already known.
#[derive(Clone, Debug)]
enum Remembered {
    /// Rows to draw.
    Ready {
        series: TradeReplaySeries,
        /// Whether these rows are already a SETTLED tick series, so a reopen never re-asks for
        /// ticks it already has, and a fresh entry (candles only, no tick attempt made yet) still
        /// earns one.
        ticks_settled: bool,
    },
    /// The venue answered and its answer held nothing in this window.
    Empty,
}

/// What identifies one replay question, so an identical one is recognised on reopen.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OutcomeKey {
    /// Venue the rows were fetched for.
    ///
    /// The host ALONE will not do, and the reason is not hypothetical. One host commonly answers
    /// both of a brand's markets — `api.bybit.com`, `api.gateio.ws` and `api.bitget.com` each do —
    /// while the exchange-native market name is frequently IDENTICAL across them: Bybit spot and
    /// Bybit linear are both `BTCUSDT`, and so are BitGet's two. Keyed on the host alone, a spot
    /// replay and a futures replay of the same pair over the same window are one entry, and
    /// whichever ran first serves the other its candles — or its authoritative `Empty`. The venue
    /// is what actually separates them, so it is what the key carries.
    venue: crate::venue::Venue,
    /// Host the rows came from, which is the rate-limit budget they were fetched under.
    host: &'static str,
    /// Exchange-native market name.
    market: String,
    /// Window the rows cover.
    from_ms: i64,
    to_ms: i64,
}

/// Which route and cache key a queued tick stage answers.
///
/// Carried on [`Job::Ticks`] alongside the original [`TradeReplayRequest`] rather than
/// re-derived when the stage finally runs, so a stage queued behind a long line of candle jobs
/// answers the same question `serve` decided it should — never a re-lookup that could disagree.
#[derive(Clone, Debug)]
pub(crate) struct TickStage {
    /// Which venue endpoint to ask.
    route: TradeRoute,
    /// The ring key this stage's answer replaces on success.
    key: OutcomeKey,
}

/// One unit of the worker's internal priority queue.
///
/// A candle job and its own tick upgrade are two separate units on purpose: queuing the tick
/// stage inline would make a second report-row double-click wait behind it for its OWN candles —
/// see [`next_job`], which is what keeps candle jobs strictly ahead.
pub(crate) enum Job {
    Candles(TradeReplayRequest),
    Ticks(TradeReplayRequest, TickStage),
}

/// Pop the next unit of work: any pending [`Job::Candles`] strictly ahead of every
/// [`Job::Ticks`], oldest first within each kind.
///
/// Args:
///     queue: The worker's own pending-work deque.
///
/// Returns:
///     The next job to run, or `None` when the queue is empty.
fn next_job(queue: &mut VecDeque<Job>) -> Option<Job> {
    match queue.iter().position(|job| matches!(job, Job::Candles(_))) {
        Some(index) => queue.remove(index),
        None => queue.pop_front(),
    }
}

/// Why a tick stage produced nothing, so a caller and a test can each name the reason precisely.
///
/// Each arm is a DIFFERENT log line and a different test; see [`paginate_ticks`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TickAbandon {
    Cancelled,
    Deadline,
    Transient,
    Empty,
    UnknownSymbol,
    OverTickBudget,
    OverPageBudget,
    /// The stage's own [`TickObserver::claim`] was refused: an active refusal is already
    /// recorded for this host by some other request, and the tick stage must respect it rather
    /// than send anyway on the strength of a candle stage's claim that already cleared.
    RateLimited,
}

/// What one tick stage produced.
///
/// `Ready` carries the ticks EXACTLY as fetched, page order untouched: the global sort and the
/// window clip happen in [`serve_ticks`] after this returns, so a test can hand in DESCENDING
/// pages and observe that the SORT, not the pagination, is what fixes them.
#[derive(Debug)]
pub(crate) enum TickVerdict {
    Ready(Vec<Tick>),
    Abandoned(TickAbandon),
}

/// Records the gate calls one tick stage makes, so a test can assert exactly one claim per stage
/// and exactly one pace per fetched page, without a network or a real gate.
///
/// `claim` takes a REAL send permit rather than merely observing one: the candle stage that ran
/// immediately before this one claimed and cleared its OWN permit already, and neither a
/// cache-answered candle stage nor a memory-ring reopen ever reaches `gate.claim` at all, so a
/// tick stage that trusted that prior claim would send its up-to-60 requests blind to an active
/// refusal recorded for this host by any other request — the exact escalation-to-ban path
/// [`ReplayGate`] exists to prevent.
pub(crate) trait TickObserver {
    fn claim(&mut self, host: &str) -> Result<(), u32>;
    fn pace(&mut self, host: &str);
}

/// Bridges the pure [`TickObserver`] seam to the real [`ReplayGate`] for production use.
///
/// Holds its own `host` rather than trusting the one handed to each call: [`TickObserver`]'s
/// methods take `&str` so the pure seam stays free of a lifetime a test double has no reason to
/// carry, while [`ReplayGate::claim`] and [`ReplayGate::pace`] need the `'static` the route
/// itself already guarantees. Pinning it at construction resolves that without widening the
/// trait's parameter type.
struct GateObserver<'a> {
    gate: &'a ReplayGate,
    host: &'static str,
}

impl TickObserver for GateObserver<'_> {
    fn claim(&mut self, _host: &str) -> Result<(), u32> {
        self.gate.claim(self.host, Instant::now())
    }

    fn pace(&mut self, _host: &str) {
        self.gate.pace(self.host);
    }
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

/// Worker loop: an internal priority queue, forever.
///
/// One request produces up to two jobs, run at different priorities rather than back to back —
/// see [`next_job`] for why an inline tick stage would break the first outcome's own promise.
/// Every iteration blocks on [`Receiver::recv`] only when the queue is empty; otherwise every
/// already-queued request is drained non-blockingly first, so a burst of report-row clicks is
/// batched into the queue before priority is applied rather than served one at a time.
///
/// Args:
///     rx: Queue of pending requests.
fn run(rx: &Receiver<TradeReplayRequest>) {
    let agent = rest::agent();
    let gate = ReplayGate::new();
    let cache: Mutex<VecDeque<(OutcomeKey, Remembered)>> = Mutex::new(VecDeque::new());
    let mut queue: VecDeque<Job> = VecDeque::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(request) => queue.push_back(Job::Candles(request)),
                // Every sender lives inside `WORKER`, which is never dropped, so this is
                // unreachable in practice; exiting is the honest answer if it ever happens.
                Err(_) => return,
            }
        }
        while let Ok(request) = rx.try_recv() {
            queue.push_back(Job::Candles(request));
        }
        let Some(job) = next_job(&mut queue) else {
            continue;
        };
        match job {
            Job::Candles(request) => {
                // A window that closed while its request sat in the queue costs nothing at all:
                // this is the cheapest of the three cancellation guards and the only one that
                // prevents the work.
                if request.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                let served = serve(&agent, &gate, &cache, &request);
                // The receiver is gone whenever the window closed mid-fetch. Normal, not an
                // error — and exactly the signal that a queued tick stage would now answer no
                // one, so it is never queued on a failed send.
                let sent = request.reply.send(served.outcome).is_ok();
                if sent {
                    if let Some(stage) = served.tick_stage {
                        queue.push_back(Job::Ticks(request, stage));
                    }
                }
            }
            Job::Ticks(request, stage) => {
                if request.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                if let Some(series) = serve_ticks(&agent, &gate, &request, &stage) {
                    remember_store(
                        &cache,
                        stage.key,
                        Remembered::Ready {
                            series: series.clone(),
                            ticks_settled: true,
                        },
                    );
                    // Normal, not an error, for the same reason as the candle send above.
                    let _ = request.reply.send(TradeReplayOutcome::Ready(series));
                }
            }
        }
    }
}

/// What one candle job resolves to: the outcome to send, and whether it earned a tick upgrade.
struct Served {
    outcome: TradeReplayOutcome,
    /// `Some` only when the CANDLE outcome above was `Ready`, so `run` may queue it onto the
    /// BACK of the deque; see [`tick_stage_for`] for the four conditions that gate it.
    tick_stage: Option<TickStage>,
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
///     The outcome to send back, and the tick stage to queue behind it, if any.
fn serve(
    agent: &ureq::Agent,
    gate: &ReplayGate,
    cache: &Mutex<VecDeque<(OutcomeKey, Remembered)>>,
    request: &TradeReplayRequest,
) -> Served {
    let venue = request.address.venue;
    let Some(route) = kline_route(venue) else {
        return Served {
            outcome: TradeReplayOutcome::Empty(TradeReplayEmpty::NoEndpoint { brand: venue.brand }),
            tick_stage: None,
        };
    };
    let key = OutcomeKey {
        venue,
        host: route.host(),
        market: request.market.clone(),
        from_ms: request.window.from_ms,
        to_ms: request.window.to_ms,
    };
    match remember_lookup(cache, &key, request.identity) {
        Some(Remembered::Ready {
            series,
            ticks_settled,
        }) => {
            let tick_stage = tick_stage_for(venue, request.window, &key, ticks_settled);
            return Served {
                outcome: TradeReplayOutcome::Ready(series),
                tick_stage,
            };
        }
        Some(Remembered::Empty) => {
            return Served {
                outcome: TradeReplayOutcome::Empty(TradeReplayEmpty::NoDataInWindow),
                tick_stage: None,
            };
        }
        None => {}
    }

    // The SQLite cache is read first and unconditionally: it costs no request and is not gated.
    if let Some(rows) = read_cached_bars(request.address.cache.as_ref(), request) {
        let series = compose(request, venue, rows);
        remember_store(
            cache,
            key.clone(),
            Remembered::Ready {
                series: series.clone(),
                ticks_settled: false,
            },
        );
        let tick_stage = tick_stage_for(venue, request.window, &key, false);
        return Served {
            outcome: TradeReplayOutcome::Ready(series),
            tick_stage,
        };
    }

    if let Err(retry_in_s) = gate.claim(route.host(), Instant::now()) {
        return Served {
            outcome: TradeReplayOutcome::Failed(TradeReplayFailure::RateLimited { retry_in_s }),
            tick_stage: None,
        };
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
            return Served {
                outcome: TradeReplayOutcome::Failed(TradeReplayFailure::Transient {
                    diagnostic: format!("trade replay exceeded {}s", JOB_DEADLINE.as_secs()),
                }),
                tick_stage: None,
            };
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
                return Served {
                    outcome: TradeReplayOutcome::Failed(TradeReplayFailure::UnknownSymbol),
                    tick_stage: None,
                };
            }
            Err(rest::FetchError::Transient(diagnostic)) => {
                return Served {
                    outcome: TradeReplayOutcome::Failed(TradeReplayFailure::Transient {
                        diagnostic,
                    }),
                    tick_stage: None,
                };
            }
        }
    }
    // The venue answered, so its refusal history is stale whatever the rows say.
    gate.clear(route.host());
    // A window's right edge is routinely in the FUTURE: `replay_window` pads the trade's close by
    // at least `TRAIL_FLOOR_MS`, and nothing clamps that to now. So replaying a trade that closed
    // minutes ago asks every venue for the minute currently forming, and most of them send it.
    // That bar is still changing, and the rows below are merged into the kline cache the LIVE
    // recorder shares, so keeping one files a half-built minute as settled history.
    //
    // Dropped HERE rather than in the parsers, for three reasons. Two venues send no closed-flag
    // at all, so no per-venue filter could cover them. A parser is pure by design, and reading a
    // clock inside one is what would stop the recorded fixtures from being a complete test of it.
    // And the bar's own open time answers the question for every venue at once.
    //
    // The vendor flags the parsers DO read stay: a vendor is authoritative about its own bar in a
    // way a clock comparison is not, and the two disagree only where the vendor is right.
    //
    // `now_unix_ms_i64` answers 0 when the clock precedes the epoch. Zero is not a plausible now,
    // and taking it as one would put every real bar in the future and drop the lot, so a clock
    // that cannot be read leaves the rows exactly as they arrive — today's behaviour.
    let now_ms = crate::util::time::now_unix_ms_i64();
    let before_drop = rows.len();
    if now_ms > 0 {
        let closed_before_ms = (now_ms - BAR_MS) as f64;
        rows.retain(|candle| candle.t_open_ms <= closed_before_ms);
    }
    // A dropped bar makes this run INCOMPLETE, which is exactly what that flag already means: the
    // window has not been fully answered yet. Without this, a window whose only bar is the forming
    // one empties out and is remembered as an authoritative "this market did not trade".
    complete = complete && rows.len() == before_drop;
    write_cached_bars(
        request.address.cache.as_ref(),
        request,
        rows_for_cache(TradeReplaySource::Klines1m, &rows),
    );
    if rows.is_empty() {
        // Only a COMPLETE run may be remembered, empty or not: a cancelled one proves nothing
        // about the window it never finished reading.
        if complete {
            remember_store(cache, key, Remembered::Empty);
        }
        return Served {
            outcome: TradeReplayOutcome::Empty(TradeReplayEmpty::NoDataInWindow),
            tick_stage: None,
        };
    }
    let series = compose(request, venue, rows);
    // Only a COMPLETE run may be remembered. Pages are issued left to right, so a cancelled run
    // holds the window's left-hand prefix — typically missing exactly the bars around the exit —
    // and the in-memory ring, unlike the SQLite path, has no coverage re-check to catch that on
    // read. Storing it would serve a silently truncated chart as `Ready` for the life of the
    // entry. The SQLite merge above is unaffected: `cache_covers` re-checks it on every read.
    let tick_stage = match complete {
        true => tick_stage_for(venue, request.window, &key, false),
        false => None,
    };
    if complete {
        remember_store(
            cache,
            key,
            Remembered::Ready {
                series: series.clone(),
                ticks_settled: false,
            },
        );
    }
    Served {
        outcome: TradeReplayOutcome::Ready(series),
        tick_stage,
    }
}

/// Decide whether a just-served CANDLE outcome earns a queued tick upgrade.
///
/// `Some` only when every one of four conditions holds: a trade route exists for this venue
/// ([`trade_route`]), the ring entry backing this key does not already carry a SETTLED tick
/// series, and the window is inside the route's own documented retention
/// ([`inside_retention`]) — evaluated here, BEFORE any request is spent, so an out-of-retention
/// window costs zero. The fourth condition, "the outcome was `Ready{Klines1m}`", is enforced by
/// every call site: this is reached only from a branch that just composed or looked up a candle
/// `Ready` outcome.
///
/// A clock that cannot be read (`now_unix_ms_i64` answering `0`) is treated as INSIDE retention
/// rather than refused, the same permissive default [`serve`] already applies to the closed-bar
/// drop above: nothing here can prove the window is too old, so nothing here refuses it.
///
/// Args:
///     venue: Venue the candles came from.
///     window: The window the candles cover.
///     key: The ring key this stage would replace on success.
///     ticks_settled: Whether the ring entry for this key already carries a settled tick series.
///
/// Returns:
///     The stage to queue, or `None`.
fn tick_stage_for(
    venue: crate::venue::Venue,
    window: ReplayWindow,
    key: &OutcomeKey,
    ticks_settled: bool,
) -> Option<TickStage> {
    if ticks_settled {
        return None;
    }
    let route = trade_route(venue)?;
    let now_ms = crate::util::time::now_unix_ms_i64();
    if now_ms > 0 && !inside_retention(route, window, now_ms) {
        return None;
    }
    Some(TickStage {
        route,
        key: key.clone(),
    })
}

/// Whether a window is within a trade route's own documented retention.
///
/// Free, and evaluated BEFORE any request is spent — see [`tick_stage_for`], the only caller.
///
/// Args:
///     route: The trade route in question.
///     window: The window to check.
///     now_ms: Current Unix time in milliseconds.
///
/// Returns:
///     `true` when the route documents no retention limit, or when the window's left edge falls
///     inside the one it does document.
pub(crate) fn inside_retention(route: TradeRoute, window: ReplayWindow, now_ms: i64) -> bool {
    route
        .retention_ms()
        .is_none_or(|r| window.from_ms >= now_ms - r)
}

/// Run one queued tick stage to completion.
///
/// Args:
///     agent: Shared HTTP client.
///     gate: Per-host pacing.
///     request: The original request this stage upgrades.
///     stage: Which route and cache key this stage answers.
///
/// Returns:
///     The tick series to send as the SECOND outcome, or `None` when it could not be produced —
///     in which case the candle picture already sent stands and nothing more is sent.
fn serve_ticks(
    agent: &ureq::Agent,
    gate: &ReplayGate,
    request: &TradeReplayRequest,
    stage: &TickStage,
) -> Option<TradeReplaySeries> {
    let route = stage.route;
    let deadline = Instant::now() + JOB_DEADLINE;
    let slices = time_slices(request.window, route.max_query_ms());
    let mut observer = GateObserver {
        gate,
        host: route.host(),
    };
    let verdict = paginate_ticks(
        route,
        &slices,
        TICK_BUDGET,
        TICK_PAGE_BUDGET,
        || request.cancel.load(Ordering::Relaxed),
        || Instant::now() >= deadline,
        &mut observer,
        |from_ms, to_ms, cursor| {
            rest::fetch_trades(agent, route, &request.market, from_ms, to_ms, cursor)
        },
    );
    let mut ticks = match verdict {
        TickVerdict::Ready(ticks) => {
            // The venue answered every slice, so its refusal history is stale whatever the rows
            // say — exactly the candle stage's own `gate.clear` above. A `Transient` page never
            // reaches here: it abandons through the branch below, and the refusal it recorded
            // must stay.
            gate.clear(route.host());
            ticks
        }
        TickVerdict::Abandoned(reason) => {
            // RELEASE THE PERMIT THIS STAGE TOOK, unless the venue is the reason we stopped.
            //
            // `TickObserver::claim` records a real attempt on the host's shared claim map, and
            // only an explicit `clear` erases it. So an abandonment that is OUR OWN doing — the
            // user closed the window, the job deadline expired, either budget was crossed, the
            // market was simply quiet, or the venue answered that it does not list this symbol —
            // would otherwise leave that attempt standing and put the host into 30-600 s of
            // backoff. The next request to the SAME host is then refused, and because one host
            // serves several venues that request belongs to an unrelated trade, and is usually a
            // CANDLE stage that would have worked. The candle path one function up makes exactly
            // this distinction already: it clears unconditionally after its own loop, its own
            // cancellation break included, and clears on `UnknownSymbol` for the stated reason
            // that "one bad market would throttle every other market on that host".
            //
            // TWO reasons keep the record, and both are the venue's own word rather than ours:
            // `Transient` is a refusal or failure it just gave us, and `RateLimited` means our
            // claim was REFUSED — we recorded nothing, so clearing would erase somebody else's
            // legitimate backoff.
            match reason {
                TickAbandon::Transient | TickAbandon::RateLimited => {}
                TickAbandon::Cancelled
                | TickAbandon::Deadline
                | TickAbandon::Empty
                | TickAbandon::UnknownSymbol
                | TickAbandon::OverTickBudget
                | TickAbandon::OverPageBudget => gate.clear(route.host()),
            }
            log::info!(
                "[x] trade-replay tick stage abandoned on {}: {reason:?}",
                route.host()
            );
            return None;
        }
    };
    // The complete tick vector, stably sorted ascending and clipped to the window, BEFORE
    // anything else: per-page sorting is NOT enough for the two BACKWARD-paginating venues
    // (Bitget, OKX), whose concatenated pages walk backwards across every page boundary, and
    // `aggregate_trades`'s own trailing arm drops a trade older than the whole series rather than
    // reordering it — a tick chart missing whole minutes, with no error anywhere.
    ticks.sort_by(|a, b| {
        a.time_ms
            .partial_cmp(&b.time_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ticks.retain(|t| {
        t.time_ms.is_finite()
            && (t.time_ms as i64) >= request.window.from_ms
            && (t.time_ms as i64) <= request.window.to_ms
    });
    if ticks.is_empty() {
        log::info!(
            "[x] trade-replay tick stage abandoned on {}: no ticks left inside the window after clipping",
            route.host()
        );
        return None;
    }
    Some(compose_ticks(request, request.address.venue, ticks))
}

/// Walk every slice of one tick stage, paginating each with the venue's own cursor, until the
/// stage completes, is abandoned, or either budget is spent.
///
/// `fetch` is the injected seam — no network, no clock, no gate inside this function, which is
/// what makes it testable with a fake fetcher and a fake [`TickObserver`]. Cancellation and the
/// deadline are checked before every page, ahead of the page budget, ahead of the tick budget;
/// an empty result is recognised only once every slice has been walked with nothing collected.
///
/// Args:
///     route: Which venue endpoint this stage answers.
///     slices: The window's own [`time_slices`] output; one venue-side pagination loop runs per
///         slice.
///     tick_budget: Ceiling on the total ticks collected across every slice.
///     page_budget: Ceiling on the total pages fetched across every slice.
///     cancelled: Answers whether the requester's window has closed.
///     expired: Answers whether this stage's own deadline has passed.
///     observer: Records the `claim`/`pace` calls this stage makes.
///     fetch: Fetches one page for a given slice and cursor.
///
/// Returns:
///     The collected ticks in the vendor's own per-page order (unsorted across pages), or the
///     reason nothing was collected.
pub(crate) fn paginate_ticks<F, O>(
    route: TradeRoute,
    slices: &[(i64, i64)],
    tick_budget: usize,
    page_budget: usize,
    cancelled: impl Fn() -> bool,
    expired: impl Fn() -> bool,
    observer: &mut O,
    mut fetch: F,
) -> TickVerdict
where
    F: FnMut(i64, i64, Option<rest::TradeCursor>) -> Result<rest::TradePage, rest::FetchError>,
    O: TickObserver,
{
    if slices.is_empty() {
        return TickVerdict::Abandoned(TickAbandon::Empty);
    }
    if observer.claim(route.host()).is_err() {
        return TickVerdict::Abandoned(TickAbandon::RateLimited);
    }
    let mut ticks: Vec<Tick> = Vec::new();
    let mut pages_fetched = 0usize;
    for &(slice_from, slice_to) in slices {
        let mut cursor: Option<rest::TradeCursor> = None;
        loop {
            if cancelled() {
                return TickVerdict::Abandoned(TickAbandon::Cancelled);
            }
            if expired() {
                return TickVerdict::Abandoned(TickAbandon::Deadline);
            }
            if pages_fetched >= page_budget {
                return TickVerdict::Abandoned(TickAbandon::OverPageBudget);
            }
            observer.pace(route.host());
            let page = match fetch(slice_from, slice_to, cursor) {
                Ok(page) => page,
                Err(rest::FetchError::UnknownSymbol) => {
                    return TickVerdict::Abandoned(TickAbandon::UnknownSymbol);
                }
                Err(rest::FetchError::Transient(_)) => {
                    return TickVerdict::Abandoned(TickAbandon::Transient);
                }
            };
            pages_fetched += 1;
            ticks.extend(page.ticks);
            if ticks.len() > tick_budget {
                return TickVerdict::Abandoned(TickAbandon::OverTickBudget);
            }
            match page.next {
                Some(next_cursor) => cursor = Some(next_cursor),
                None => break,
            }
        }
    }
    match ticks.is_empty() {
        true => TickVerdict::Abandoned(TickAbandon::Empty),
        false => TickVerdict::Ready(ticks),
    }
}

/// Build the frozen TICK series one tick stage answers with.
///
/// `ticks` must already be globally sorted ascending and clipped to `request.window` — this
/// function does neither; [`serve_ticks`] does both before calling it.
///
/// Args:
///     request: The request being served.
///     venue: Venue the ticks came from.
///     ticks: Trade points, ascending, already clipped to the window.
///
/// Returns:
///     The series to hand the chart.
fn compose_ticks(
    request: &TradeReplayRequest,
    venue: crate::venue::Venue,
    ticks: Vec<Tick>,
) -> TradeReplaySeries {
    let mut candles = Vec::new();
    crate::market::candles::aggregate_trades(&ticks, BAR_MS, &mut candles);
    TradeReplaySeries {
        source: TradeReplaySource::Ticks,
        venue,
        window: request.window,
        tf_ms: BAR_MS,
        candles,
        ticks,
        identity: request.identity,
    }
}

/// Answer the rows one cache write may actually carry, keyed on the series it came from.
///
/// The SQLite isolation seam (acceptance criterion 7): a tick series' own aggregated candles must
/// NEVER reach [`write_cached_bars`], because that table is the SHARED kline cache the live
/// recorder writes too, and a candle aggregated from ticks is not a genuine exchange bar. [`serve`]
/// routes its cache write through this and nothing else, so the invariant is enforced at the one
/// call site rather than merely honoured by omission.
///
/// Args:
///     source: Which kind of series `rows` was built for.
///     rows: The candidate rows.
///
/// Returns:
///     `rows` unchanged for [`TradeReplaySource::Klines1m`]; an empty slice for
///     [`TradeReplaySource::Ticks`].
pub(crate) fn rows_for_cache(source: TradeReplaySource, rows: &[ChartCandle]) -> &[ChartCandle] {
    match source {
        TradeReplaySource::Klines1m => rows,
        TradeReplaySource::Ticks => &[],
    }
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
        Remembered::Ready {
            mut series,
            ticks_settled,
        } => {
            // The identity belongs to the WINDOW that asked, not to the cached rows: two windows
            // on the same trade must not share a chart revision, or the second would be told
            // nothing changed and would draw nothing.
            series.identity = identity;
            Remembered::Ready {
                series,
                ticks_settled,
            }
        }
        Remembered::Empty => Remembered::Empty,
    })
}

/// Remember one answered window, evicting the oldest when full.
///
/// Two independent ceilings, both enforced oldest-first: [`OUTCOME_CACHE_LEN`] bounds the number
/// of entries, [`OUTCOME_CACHE_MAX_TICKS`] bounds their combined tick count. Neither ever evicts
/// the entry this call just inserted, so a single series alone can outrun the tick ceiling
/// without being immediately discarded.
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
    while cache.len() > 1 && total_ticks(&cache) > OUTCOME_CACHE_MAX_TICKS {
        cache.pop_front();
    }
}

/// Sum the ticks carried by every remembered entry.
///
/// Args:
///     cache: The ring.
///
/// Returns:
///     Combined tick count across every entry.
fn total_ticks(cache: &VecDeque<(OutcomeKey, Remembered)>) -> usize {
    cache
        .iter()
        .map(|(_, answer)| match answer {
            Remembered::Ready { series, .. } => series.ticks.len(),
            Remembered::Empty => 0,
        })
        .sum()
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
