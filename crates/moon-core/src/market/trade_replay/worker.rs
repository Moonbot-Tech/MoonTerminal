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
//! # Four caches, and each is load-bearing
//!
//! Bars go into the SHARED `klines.sqlite` under the real exchange key, because a one-minute bar
//! fetched here is indistinguishable from one the recorder wrote and the rest of the application
//! benefits from it. Whole OUTCOMES additionally go into a small in-memory ring owned by this
//! worker, keyed by the exact question asked. That second cache is what satisfies "the second
//! open of the SAME trade costs nothing": the kline cache cannot hold ticks at all, and nothing
//! else in the process remembers that a given window was already answered. The third is the tick
//! TILE store ([`super::tick_tiles`]), keyed by exchange and market and answering by COVERAGE
//! the way the bars are: it is what makes a NEIGHBOURING trade on the same market cost only the
//! stretch of its focus no earlier window fetched — and nothing at all when there is none. The
//! fourth is that store's disk, `trades.sqlite` ([`super::trade_cache`]): the tick stage hydrates
//! the tiles from it before deciding what to fetch and writes every harvest through, so the
//! same question survives a restart — behind a switch in the Storage tab.
//!
//! # The degrade ladder
//!
//! A tick stage never throws away what it already paid for. Cancellation is the one thing that
//! discards everything collected so far, because the window itself is gone; every other stop —
//! the page or tick budget, the job deadline, a venue's own refusal — instead SERVES what was
//! already fetched and names the reason in [`TradeReplaySeries::tick_status`], rather than
//! abandoning the whole stage and falling back to bars with no explanation. Only a harvest that
//! ends up genuinely empty reaches the candles-only outcome, and even then the bar layer drawn is
//! never blank: [`TickStage::candles`] carries the exchange's own one-minute klines forward from
//! the candle stage that ran first, so the window always has SOMETHING to show while the reasoned
//! caption explains what is missing and why.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::gate::ReplayGate;
use super::tick_tiles::{TickTileStore, TileKey, TileSource, residual_plan};
use super::venue_caps::{TradeRoute, bybit_category, kline_route, trade_route};
use super::{
    Coverage, ReplayWindow, TickPlan, TickStatus, TradeReplayEmpty, TradeReplayFailure,
    TradeReplayOutcome, TradeReplaySeries, TradeReplaySource, fit_ticks, pages, rest, tick_plan,
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

/// Normal stage deadline; trade-only tick tiles use [`TRADE_DEADLINE`] instead.
///
/// The HTTP client bounds each REQUEST at fifteen seconds, which says nothing about a paginated
/// job: a wide window can be many requests, and without this a single slow venue would hold the
/// one worker — and therefore every later window — for minutes. On expiry the caller is told the
/// fetch is transient, which is true and retryable.
const JOB_DEADLINE: Duration = Duration::from_secs(45);

/// Hard deadline for the position itself, so dense trades get priority without blocking forever.
const TRADE_DEADLINE: Duration = Duration::from_secs(180);

/// How many answered windows the in-memory outcome cache remembers.
///
/// Small on purpose: this exists so a reopen costs nothing, not to be a history store. Each entry
/// holds one bounded window's rows.
const OUTCOME_CACHE_LEN: usize = 8;

/// Ceiling on the total number of ticks AND per-second volume slots held across every remembered
/// entry.
///
/// A single entry can carry up to [`TICK_BUDGET`] ticks plus its `side_slots` — one per second the
/// run traded, unbounded by the tick budget since they are summed before thinning — and
/// [`OUTCOME_CACHE_LEN`] entries of that size would let the ring's own memory dwarf the point ring
/// it feeds. This bounds the ring independently of its entry count: eviction runs oldest-first,
/// exactly as the entry-count eviction does, and never touches the entry that was just inserted,
/// so one huge series is held rather than immediately discarded and re-fetched. Sized for the two
/// trade windows that can be open at once to both stay remembered, slots included.
const OUTCOME_CACHE_MAX_TICKS: usize = 4 * TICK_BUDGET;

/// Bounds the COMPOSED series and the outcome ring for one tick series — never the in-flight
/// fetch, which is bounded instead by [`TRADE_PAGE_BUDGET`] times a route's own page size. A
/// budget crossed while paginating STOPS the walk and serves what is already held rather than
/// discarding it (see the module header's degrade ladder), so this constant ceilings what gets
/// drawn and remembered, not what a stage may fetch before giving up.
///
/// Sits under the live chart's default `trades_limit` of 50 000 (`candles.rs:93`), so a tick
/// replay never asks the point ring for more than the main chart already draws.
pub(crate) const TICK_BUDGET: usize = 40_000;

/// Bounds WALL TIME on the single worker thread for one tick stage.
///
/// 60 pages at [`super::gate::ReplayGate::pace`]'s 100 ms floor plus a ~250 ms round trip is an
/// ORDER-OF-MAGNITUDE bound of a few tens of seconds, inside [`JOB_DEADLINE`] with room for a slow
/// venue. Not a precise figure: [`tick_plan`] now tiles the window into many small slices rather
/// than the one-or-two wide ones this constant was first sized against, and a quiet-market tile
/// still costs one round trip apiece, so the true page count for a given window depends on how
/// many tiles it takes as much as on how much data each holds.
const TICK_PAGE_BUDGET: usize = 60;

/// Hard page allowance for trade-only tiles; context never receives this extension.
const TRADE_PAGE_BUDGET: usize = 240;

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
    /// Prints asked for around the position ([`ReplayWindow::margin_ms`]): the bar window above
    /// does not depend on it, so without this a settled answer fetched under one margin would be
    /// served unchanged after the Storage tab moved it.
    margin_ms: i64,
}

/// Which route, cache key and bar layer a queued tick stage answers.
///
/// Carried on [`Job::Ticks`] alongside the original [`TradeReplayRequest`] rather than re-derived
/// when the stage finally runs, so a stage queued behind a long line of candle jobs answers the
/// same question `serve` decided it should — never a re-lookup that could disagree. `candles` is
/// the exchange klines `serve` already composed for this window: carrying them here removes the
/// dependence on the outcome ring not having evicted this key's entry between the two jobs, and is
/// what lets a tick outcome keep the bar layer whole even where its own points, per
/// [`TradeReplaySeries::partial`], cover only part of the window.
#[derive(Clone, Debug)]
pub(crate) struct TickStage {
    /// Previously published cache answer; every retry must retain at least this tick span.
    baseline: Option<TradeReplaySeries>,
    /// Which venue endpoint to ask, or `None` on a venue with no public trade route — then
    /// the stage asks nothing and serves what the tile store and its disk already hold for the
    /// focus (a capture from the core's archive, filed when the trade closed), or prints
    /// [`TickStatus::NoRoute`] as before when they hold nothing.
    route: Option<TradeRoute>,
    /// The ring key this stage's answer replaces on success.
    key: OutcomeKey,
    /// The exchange klines to carry forward as the bar layer of the eventual tick series.
    candles: Vec<ChartCandle>,
}

/// One unit of the worker's internal priority queue.
///
/// A candle job and its own tick upgrade are two separate units on purpose: queuing the tick
/// stage inline would make a second report-row double-click wait behind it for its OWN candles —
/// see [`next_job`], which is what keeps candle jobs strictly ahead.
pub(crate) enum Job {
    Candles(TradeReplayRequest),
    Ticks(TradeReplayRequest, TickStage),
    Native(TradeReplayRequest, NativeWait),
    /// Copy the stretches of a just-closed trade out of the core's retained archive into the
    /// tile store and its disk — see [`CaptureRequest`] and [`capture_spans`]. The flag names
    /// the settle pass, the one that runs after the trail has printed and schedules nothing.
    Capture(CaptureRequest, Coverage, bool),
}

/// A trade that just closed on a connected core, whose prints the core's own retained archive
/// still holds — the moment they are cheapest to keep.
///
/// The archive is a bounded ring per market: a busy market keeps minutes, a quiet one hours.
/// Opened later, the same trade would find the ring already moved on and page the venue. So the
/// close itself is the trigger: what the trade's own window would ask for as ticks
/// ([`ReplayWindow::focus_spans`] — the position with its margins, or on a long position only
/// the two ends) is copied up to the exit at once, and the margin after the exit
/// ([`Self::margin_ms`], the focus's trail, which has not happened yet at close time) is copied
/// once it has, by a timed second pass. A terminal closed between the two loses only the trail,
/// which the next window fetches from the venue as a residual.
///
/// Filed with [`TileSource::Core`] — provenance only: the core reports the same wire quantity
/// the venue's route does, and the band values every tile through the market's own terms.
pub struct CaptureRequest {
    /// Exchange addressing of the core that closed the trade.
    pub address: ReplayAddress,
    /// Exchange-native market name.
    pub market: String,
    /// The trade's entry, true-UTC milliseconds.
    pub open_ms: i64,
    /// The trade's exit, true-UTC milliseconds.
    pub close_ms: i64,
    /// Prints to copy around the trade, per end — [`super::margin_ms`] at close time; see
    /// [`ReplayWindow::margin_ms`].
    pub margin_ms: i64,
}

/// How long after the exit the settle pass waits past the margin: a few seconds for the core's
/// own feed to catch up to wall time.
const CAPTURE_SETTLE_SLACK: Duration = Duration::from_secs(5);

/// What reaches the worker's one inbound channel.
enum Inbound {
    Replay(TradeReplayRequest),
    Capture(CaptureRequest),
}

/// One bounded native follow-up independent of public tick-route eligibility.
pub(crate) struct NativeWait {
    fallback: TradeReplayOutcome,
    next: Instant,
    expires: Instant,
}

impl NativeWait {
    /// Keep the original terminal outcome so a timeout does not leave a loading caption behind.
    fn new(fallback: TradeReplayOutcome, now: Instant) -> Self {
        Self {
            fallback,
            next: now + Duration::from_millis(500),
            expires: now + Duration::from_secs(30),
        }
    }

    /// Finish on usable native data or expiry; otherwise retain the original fallback and retry.
    fn advance(
        &mut self,
        native: Option<TradeReplaySeries>,
        now: Instant,
    ) -> Option<TradeReplayOutcome> {
        let required = match &self.fallback {
            TradeReplayOutcome::Ready(series) if series.source.is_ticks() => series.covered.clone(),
            _ => Coverage::none(),
        };
        if let Some(series) = native.filter(|series| preserves_coverage(&series.covered, &required))
        {
            return Some(TradeReplayOutcome::Ready(attach_context(
                series,
                &self.fallback,
            )));
        }
        if now >= self.expires {
            return Some(self.fallback.clone());
        }
        self.next = now + Duration::from_millis(500);
        None
    }
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
        None => match queue.iter().position(|job| matches!(job, Job::Native(..))) {
            Some(index) => queue.remove(index),
            None => queue.pop_front(),
        },
    }
}

/// Why a tick stage stopped, logged for partial harvests as well as empty abandonments.
///
/// `Cancelled` throws away whatever was collected because the window itself closed. Every other
/// stop serves a non-empty harvest instead of abandoning it; see [`paginate_ticks`]. Budget
/// and deadline stops with paid-for rows log their reason and covered span once before returning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TickAbandon {
    Cancelled,
    Deadline,
    Transient,
    Empty,
    UnknownSymbol,
    OverPageBudget,
    /// A non-focus tile would exceed the retained tick allowance.
    OverTickBudget,
    /// The stage's own [`TickObserver::claim`] was refused: an active refusal is already
    /// recorded for this host by some other request, and the tick stage must respect it rather
    /// than send anyway on the strength of a candle stage's claim that already cleared.
    RateLimited,
}

/// What one tick stage's walk produced when it did produce something.
#[derive(Debug)]
pub(crate) struct TickHarvest {
    /// Ticks collected, in the vendor's own per-page order within each slice — the global sort
    /// and the clip to [`Self::covered`] both happen in [`serve_ticks`] after this returns, so a
    /// test can hand in DESCENDING pages and observe that the SORT, not the pagination, is what
    /// fixes them.
    pub ticks: Vec<Tick>,
    /// The stretches [`Self::ticks`] is guaranteed exhaustive over — [`serve_ticks`] clips to
    /// these rather than to the request window, since a walk cut short still holds a complete
    /// answer for the slices it actually finished. One stretch per contiguous group of completed
    /// tiles: a long position's plan walks the entry's and the exit's neighbourhoods, and a
    /// residual plan's completed tiles may be separated by stretches the store already held —
    /// those are bridged by [`serve_ticks`] over the store, never here.
    pub covered: Coverage,
    /// Whether every slice of the plan was walked to completion.
    pub complete: bool,
    /// Whether the walk stopped because the venue itself refused (`Transient`/`UnknownSymbol`),
    /// as opposed to our own budget or the caller cancelling — see [`serve_ticks`]'s gate-clear.
    pub venue_refused: bool,
}

/// What one tick stage's walk produced.
///
/// `Ready` carries the harvest exactly as walked; `Abandoned` carries the reason nothing usable
/// resulted. See [`TickHarvest`] and [`TickAbandon`].
#[derive(Debug)]
pub(crate) enum TickVerdict {
    Ready(TickHarvest),
    Abandoned(TickAbandon),
}

/// Records the gate calls one tick stage makes, so a test can assert exactly one claim per stage
/// and exactly one pace per fetched page, without a network or a real gate.
///
/// `claim` takes a REAL send permit rather than merely observing one: the candle stage that ran
/// immediately before this one claimed and cleared its OWN permit already, and neither a
/// cache-answered candle stage nor a memory-ring reopen ever reaches `gate.claim` at all, so a
/// tick stage that trusted that prior claim would send its bounded requests blind to an active
/// refusal recorded for this host by any other request — the exact escalation-to-ban path
/// [`ReplayGate`] exists to prevent.
pub(crate) trait TickObserver {
    fn claim(&mut self, host: &str) -> Result<(), u32>;
    fn pace(&mut self, host: &str);
    /// Publish the completed stretches so far without claiming unfetched time between tiles.
    fn progress(&mut self, _ticks: &[Tick], _covered: &Coverage) {}
}

/// Callback that publishes a tick snapshot to one replay window.
type TickProgress<'a> = dyn FnMut(&[Tick], &Coverage) + 'a;

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
    progress: &'a mut TickProgress<'a>,
}

impl TickObserver for GateObserver<'_> {
    fn claim(&mut self, _host: &str) -> Result<(), u32> {
        self.gate.claim(self.host, Instant::now())
    }

    fn pace(&mut self, _host: &str) {
        self.gate.pace(self.host);
    }

    /// Forward progress to this request's own reply channel.
    fn progress(&mut self, ticks: &[Tick], covered: &Coverage) {
        (self.progress)(ticks, covered);
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
    /// How the venue's prints are valued for the band — see [`super::venue_caps::TickValue`].
    /// Decided by the requester from the core's market terms; the worker only applies it.
    pub tick_value: super::venue_caps::TickValue,
    /// Whether the tick stage may run at all. `false` asks for the bars alone: no exchange
    /// trade pages and no core archive read — the reader's switch for a slow venue. A tick
    /// series a previous request already fetched and remembered is still served: it costs
    /// nothing, and the switch is about not paying, not about not seeing.
    pub ticks: bool,
    /// Set by the requester when its window closes; checked between pages.
    pub cancel: Arc<AtomicBool>,
    /// Where the answer goes. A dead receiver is normal and is not an error.
    pub reply: Sender<TradeReplayOutcome>,
}

/// Handle to the process-wide replay worker.
struct Worker {
    tx: Sender<Inbound>,
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
    send(Inbound::Replay(request));
}

/// Queue one capture of a just-closed trade's prints from its core's archive.
///
/// Returns immediately; nothing answers. The capture is silent when the core holds nothing for
/// the market, and logs one line when it filed something.
///
/// Args:
///     request: The trade and where its core is.
pub fn capture(request: CaptureRequest) {
    send(Inbound::Capture(request));
}

fn send(inbound: Inbound) {
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Inbound>();
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
    if worker.tx.send(inbound).is_err() {
        log::warn!("[x] trade-replay request dropped: worker is not running");
    }
}

/// Where an inbound message lands in the queue.
fn enqueue(queue: &mut VecDeque<Job>, inbound: Inbound) {
    match inbound {
        Inbound::Replay(request) => queue.push_back(Job::Candles(request)),
        Inbound::Capture(request) => {
            // Everything up to the exit has already printed; the trail is the settle pass's.
            let spans = capture_spans(&request, false);
            // One line per close announced, so a close announced twice is visible as two.
            log::info!(
                "[x] trade-replay capture queued {} open={} close={}",
                request.market,
                request.open_ms,
                request.close_ms
            );
            queue.push_back(Job::Capture(request, spans, false));
        }
    }
}

/// Worker loop: an internal priority queue, forever.
///
/// Candle, tick, bounded native follow-up and capture jobs share one queue: candles first,
/// then native probes, then ticks and captures in arrival order — see [`next_job`] for why an
/// inline tick stage would break the first outcome's own promise. A capture is an in-process
/// copy out of a core's ring, milliseconds, so it never holds a tick stage up for long; the
/// settle pass of each capture is timed (`settle_waits`) and enters the queue when due. Idle
/// waits end at the next native probe or settle deadline; otherwise every already-queued
/// request is drained non-blockingly first, so a burst of report-row clicks is batched into
/// the queue before priority is applied rather than served one at a time.
///
/// Args:
///     rx: Queue of pending requests.
fn run(rx: &Receiver<Inbound>) {
    let agent = rest::agent();
    let gate = ReplayGate::new();
    let cache: Mutex<VecDeque<(OutcomeKey, Remembered)>> = Mutex::new(VecDeque::new());
    let tiles: Mutex<TickTileStore> = Mutex::new(TickTileStore::default());
    let mut queue: VecDeque<Job> = VecDeque::new();
    let mut native_waits: Vec<(TradeReplayRequest, NativeWait)> = Vec::new();
    // Settle passes of captures, each due once the focus's trail has printed.
    let mut settle_waits: Vec<(CaptureRequest, Coverage, Instant)> = Vec::new();
    loop {
        let now = Instant::now();
        let mut index = 0;
        while index < native_waits.len() {
            if native_waits[index].1.next <= now {
                let (request, wait) = native_waits.remove(index);
                queue.push_back(Job::Native(request, wait));
            } else {
                index += 1;
            }
        }
        let mut index = 0;
        while index < settle_waits.len() {
            if settle_waits[index].2 <= now {
                let (request, span, _) = settle_waits.remove(index);
                queue.push_back(Job::Capture(request, span, true));
            } else {
                index += 1;
            }
        }
        if queue.is_empty() {
            let next_due = native_waits
                .iter()
                .map(|(_, wait)| wait.next)
                .chain(settle_waits.iter().map(|(_, _, due)| *due))
                .min();
            let received = match next_due {
                Some(next) => rx.recv_timeout(next.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(inbound) => enqueue(&mut queue, inbound),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                // Every sender lives inside `WORKER`, which is never dropped, so this is
                // unreachable in practice; exiting is the honest answer if it ever happens.
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        while let Ok(inbound) = rx.try_recv() {
            enqueue(&mut queue, inbound);
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
                let mut served = serve_with_core(&agent, &gate, &cache, &request);
                // No native wait either with the stage off: the wait is the core-archive half
                // of the same stage, and it would poll the archive for a window that asked for
                // the bars alone.
                let native_wait = match request.ticks {
                    true => prepare_native_wait(&mut served, Instant::now()),
                    false => None,
                };
                // The receiver is gone whenever the window closed mid-fetch. Normal, not an
                // error — and exactly the signal that a queued tick stage would now answer no
                // one, so it is never queued on a failed send.
                let sent = request.reply.send(served.outcome).is_ok();
                if sent {
                    if let Some(stage) = served.tick_stage {
                        queue.push_back(Job::Ticks(request, stage));
                    } else if let Some(wait) = native_wait {
                        native_waits.push((request, wait));
                    }
                }
            }
            Job::Capture(request, spans, settle_pass) => {
                for &span in spans.spans() {
                    capture_from_core(&request, span, &tiles);
                }
                // The settle pass is the last word and schedules nothing.
                if settle_pass {
                    continue;
                }
                if let Some((settle, due_ms)) = settle_plan(&request) {
                    // The trail has not printed yet: come back once it has, for the whole of
                    // what the window asks for rather than the trail alone — the gap rule files
                    // only what the first pass missed, so a feed that lagged behind the exit at
                    // close time (the archive not yet reaching it, and the first pass refused)
                    // is caught up here at no extra cost. Measured from the trail's own end, not
                    // from now — a capture queued late settles at once.
                    let now_ms = crate::util::time::now_unix_ms_i64();
                    let wait_ms = due_ms.saturating_sub(now_ms).max(0);
                    settle_waits.push((
                        request,
                        settle,
                        Instant::now() + Duration::from_millis(wait_ms as u64),
                    ));
                }
            }
            Job::Native(request, mut wait) => {
                if request.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                let outcome = wait.advance(read_core(&request), Instant::now());
                if request.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                match outcome {
                    Some(outcome) => {
                        let _ = request.reply.send(outcome);
                    }
                    None => native_waits.push((request, wait)),
                }
            }
            Job::Ticks(request, stage) => {
                if request.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                match serve_ticks(&agent, &gate, &request, &stage, &tiles) {
                    Ok((series, retry_on_reopen)) => {
                        let series = retain_baseline(series, stage.baseline.as_ref());
                        // A PARTIAL harvest is still a COMPLETE run of the stage: the walk is
                        // done deciding what it can serve, so a reopen must not re-ask for ticks
                        // it already answered, whether or not `series.partial` is set — UNLESS
                        // the venue's own account of why it stopped was `Transient`: serving the
                        // partial rows is right (throwing away paid-for data is what this goal
                        // removes), but calling that answer SETTLED is not, because the venue
                        // called its own refusal transient. An exact-key reopen must retry it
                        // rather than pin the same partial answer until unrelated cache eviction.
                        // The same holds when the walk was abandoned and the tile store served
                        // the part of the focus it held: what it did not hold is still owed.
                        remember_store(
                            &cache,
                            stage.key,
                            Remembered::Ready {
                                series: series.clone(),
                                ticks_settled: !retry_on_reopen,
                            },
                        );
                        // Normal, not an error, for the same reason as the candle send above.
                        let mut served = Served {
                            outcome: TradeReplayOutcome::Ready(series),
                            tick_stage: None,
                        };
                        let wait = prepare_native_wait(&mut served, Instant::now());
                        if request.reply.send(served.outcome).is_ok() {
                            if let Some(wait) = wait {
                                native_waits.push((request, wait));
                            }
                        }
                    }
                    Err(Some(status)) => {
                        let mut series = stage.baseline.clone().unwrap_or_else(|| {
                            compose(&request, request.address.venue, stage.candles)
                        });
                        series.tick_status = if series.source.is_ticks() {
                            TickStatus::Served
                        } else {
                            status
                        };
                        // `NoTrades` is authoritative — the venue answered and held nothing — and
                        // is remembered settled exactly like a `Ready` harvest. `Failed` is not:
                        // the fetch itself did not produce an answer, so a reopen must retry it.
                        if status == TickStatus::NoTrades {
                            remember_store(
                                &cache,
                                stage.key,
                                Remembered::Ready {
                                    series: series.clone(),
                                    ticks_settled: true,
                                },
                            );
                        }
                        let mut served = Served {
                            outcome: TradeReplayOutcome::Ready(series),
                            tick_stage: None,
                        };
                        let wait = prepare_native_wait(&mut served, Instant::now());
                        if request.reply.send(served.outcome).is_ok() {
                            if let Some(wait) = wait {
                                native_waits.push((request, wait));
                            }
                        }
                    }
                    // The window closed; there is no one left to send a second outcome to.
                    Err(None) => {}
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

/// Arm native observation whenever no public tick job can complete this candle/failed answer.
fn prepare_native_wait(served: &mut Served, now: Instant) -> Option<NativeWait> {
    if served.tick_stage.is_some()
        || matches!(&served.outcome, TradeReplayOutcome::Ready(series)
            if series.source == TradeReplaySource::CoreTicks || (series.source.is_ticks()
                && preserves_coverage(&series.covered, &series.window.focus_spans())))
    {
        return None;
    }
    let wait = NativeWait::new(served.outcome.clone(), now);
    if let TradeReplayOutcome::Ready(series) = &mut served.outcome {
        series.tick_status = if series.source.is_ticks() {
            TickStatus::Streaming
        } else {
            TickStatus::AwaitingCore
        };
    }
    Some(wait)
}

/// Preserve native ticks even when candle context fails, and carry an honest caption state.
fn attach_context(
    mut native: TradeReplaySeries,
    context: &TradeReplayOutcome,
) -> TradeReplaySeries {
    match context {
        TradeReplayOutcome::Ready(series) if !series.candles.is_empty() => {
            native.candles = series.candles.clone();
        }
        _ => native.tick_status = TickStatus::ContextUnavailable,
    }
    native
}

/// Prefer the core archive before paying for public history, rechecking after candles arrive.
fn serve_with_core(
    agent: &ureq::Agent,
    gate: &ReplayGate,
    cache: &Mutex<VecDeque<(OutcomeKey, Remembered)>>,
    request: &TradeReplayRequest,
) -> Served {
    if !request.ticks {
        return bars_only(serve(agent, gate, cache, request));
    }
    core_first(|| read_core(request), || serve(agent, gate, cache, request))
}

/// What a request with the tick stage switched off is answered with: what `serve` has, and no
/// stage.
///
/// The cache inside `serve` has already remembered the answer as NOT settled when a stage would
/// have run, so a later request with the stage on re-decides it rather than inheriting this one.
/// A remembered tick series is served as it is — see `TradeReplayRequest::ticks` — but with the
/// stage that would have continued it gone, its status can no longer say `Streaming`: nothing
/// will finish it, so it is what it is, served and possibly partial. The bars alone say the stage
/// is off.
fn bars_only(mut served: Served) -> Served {
    let had_stage = served.tick_stage.take().is_some();
    if had_stage {
        if let TradeReplayOutcome::Ready(series) = &mut served.outcome {
            series.tick_status = match series.source.is_ticks() {
                true => TickStatus::Served,
                false => TickStatus::Disabled,
            };
        }
    }
    served
}

/// Keep wide candle context and replace only the narrow tick stage with core data.
///
/// The callback seam verifies request avoidance without making a live exchange request. A
/// second read observes an archive that completed while the candle fallback was running.
fn core_first(
    mut read_core: impl FnMut() -> Option<TradeReplaySeries>,
    fetch_candles: impl FnOnce() -> Served,
) -> Served {
    let initial = read_core();
    let mut served = fetch_candles();
    // A previously cached exchange tick answer already avoids a tick request and may cover
    // more context than the core. Never replace it with a narrower local answer.
    if matches!(&served.outcome, TradeReplayOutcome::Ready(series) if series.source.is_ticks()) {
        return served;
    }
    let Some(series) = initial.or_else(&mut read_core) else {
        return served;
    };
    let series = attach_context(series, &served.outcome);
    served.outcome = TradeReplayOutcome::Ready(series);
    served.tick_stage = None;
    served
}

/// Freeze core-owned points into the same bounded representation the REST tick stage produces.
fn read_core(request: &TradeReplayRequest) -> Option<TradeReplaySeries> {
    if request.cancel.load(Ordering::Relaxed) {
        return None;
    }
    let native = request.address.history.replay_core_ticks(
        &request.address,
        &request.market,
        request.window.tick_window(),
    )?;
    // The core's prints are valued through the SAME terms as the venue's route: the ring's
    // quantity is the wire quantity — contracts on a contract market, exactly what the route
    // reports — so one window reads the same whichever source answered it. (The live chart's
    // own band values `price × qty` unconverted; that is its inconsistency to keep, not this
    // window's.)
    let side_slots = crate::market::source::side_slots_of_ticks(&native.ticks, request.tick_value);
    let (ticks, bucket_ms) = fit_ticks(native.ticks, TICK_BUDGET);
    if ticks.is_empty() {
        return None;
    }
    let mut series = compose_ticks(
        request,
        request.address.venue,
        ticks,
        bucket_ms,
        side_slots,
        native.covered != (request.window.from_ms, request.window.to_ms),
        Coverage::one(native.covered),
        Vec::new(),
    );
    series.source = TradeReplaySource::CoreTicks;
    Some(series)
}

/// Cooperatively replace a REST walk when a requested core archive arrives between pages.
struct CoreUpgradeProbe {
    next: Cell<Instant>,
    ready: RefCell<Option<TradeReplaySeries>>,
}

impl CoreUpgradeProbe {
    /// Space expensive retained-ring scans while an exchange page is in flight.
    fn new(now: Instant) -> Self {
        Self {
            next: Cell::new(now + Duration::from_millis(500)),
            ready: RefCell::new(None),
        }
    }

    /// Stop the walk on cancellation or replacement; never sleep or delay a network page.
    fn stop(
        &self,
        cancelled: bool,
        now: Instant,
        published: &Coverage,
        read: impl FnOnce() -> Option<TradeReplaySeries>,
    ) -> bool {
        if cancelled || self.ready.borrow().is_some() {
            return true;
        }
        if now < self.next.get() {
            return false;
        }
        self.next.set(now + Duration::from_millis(500));
        *self.ready.borrow_mut() =
            read().filter(|series| preserves_coverage(&series.covered, published));
        self.ready.borrow().is_some()
    }
}

/// Replacement may improve resolution/source but cannot remove any published tick interval.
///
/// A candidate with no coverage at all never preserves anything, not even nothing: it walked no
/// ticks and cannot stand in for a series that did.
fn preserves_coverage(candidate: &Coverage, published: &Coverage) -> bool {
    !candidate.is_empty() && candidate.covers(published)
}

/// A retry can update the cache only if it retains all previously available tick coverage.
fn retain_baseline(
    candidate: TradeReplaySeries,
    baseline: Option<&TradeReplaySeries>,
) -> TradeReplaySeries {
    match baseline {
        Some(previous) if !preserves_coverage(&candidate.covered, &previous.covered) => {
            previous.clone()
        }
        _ => candidate,
    }
}

/// Answer one request: memory cache, then SQLite cache, then the network.
///
/// The order is fixed and each step earns its place. The memory cache answers a reopen with no
/// work at all. The SQLite cache answers without a request, which matters most precisely when the
/// gate is refusing — a user in backoff still sees the real chart rather than a countdown. Only
/// then is a permit taken.
///
/// Each of the three points that produces a fresh candle answer (a non-settled ring hit, a
/// SQLite hit, a completed network fetch) also decides the tick stage for it and stamps the
/// outgoing series' [`TradeReplaySeries::tick_status`] to match, via [`stage_and_stamp`].
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
        margin_ms: request.window.margin_ms,
    };
    match remember_lookup(cache, &key, request.identity) {
        Some(Remembered::Ready {
            series,
            ticks_settled: true,
        }) => {
            // Sent exactly as stored: its own fields already carry the final answer, so no
            // stage is re-decided and none is queued.
            return Served {
                outcome: TradeReplayOutcome::Ready(series),
                tick_stage: None,
            };
        }
        Some(Remembered::Ready {
            mut series,
            ticks_settled: false,
        }) => {
            let tick_stage = stage_and_stamp(venue, request.window, &key, &mut series);
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
        let mut series = compose(request, venue, rows);
        let tick_stage = stage_and_stamp(venue, request.window, &key, &mut series);
        // Settled exactly when NO stage was queued: `stage_and_stamp` already stamped a TERMINAL
        // status (`NoRoute`/`OutOfRetention`) in that case, and both are stable facts a reopen
        // would only re-derive identically — a queued stage, by contrast, is still `Pending` and
        // must be re-decided (or answered) on the next open.
        remember_store(
            cache,
            key.clone(),
            Remembered::Ready {
                series: series.clone(),
                ticks_settled: tick_stage.is_none(),
            },
        );
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
    // Whether every page of the window was actually fetched. Two independent things can make this
    // false, and only `cancelled` below may still be true when this is — see the tick-stage
    // decision after the forming-bar drop for why the two must not be read as one fact. A
    // cancelled run keeps its rows — they were paid for — but must NOT be remembered as this
    // window's answer.
    let mut complete = true;
    // Whether the WINDOW ITSELF closed mid-fetch, as opposed to `complete` going false for the
    // forming-bar drop below: only this one discards the tick upgrade outright.
    let mut cancelled = false;
    for (from_ms, to_ms) in pages(request.window, BAR_MS, route.max_rows()) {
        if request.cancel.load(Ordering::Relaxed) {
            // The window is gone, or a Retry superseded this request. Whatever was fetched is
            // still worth merging into the shared cache, so fall through rather than discarding a
            // page already paid for.
            complete = false;
            cancelled = true;
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
    // A window's right edge is routinely in the FUTURE: `replay_window_ms` pads the trade's close by
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
    let mut series = compose(request, venue, rows);
    // Only a COMPLETE run may be remembered. Pages are issued left to right, so a cancelled run
    // holds the window's left-hand prefix — typically missing exactly the bars around the exit —
    // and the in-memory ring, unlike the SQLite path, has no coverage re-check to catch that on
    // read. Storing it would serve a silently truncated chart as `Ready` for the life of the
    // entry. The SQLite merge above is unaffected: `cache_covers` re-checks it on every read.
    //
    // The tick stage is gated on `cancelled` alone, NOT on `complete`: a forming-bar drop leaves
    // `complete` false too, but the window is fine and its ticks are fetched independently of the
    // bar layer — queuing the stage is the whole point of this feature, and skipping it here is
    // exactly what used to leave a freshly closed trade stuck on "tics ещё грузятся" forever.
    // CANCELLED is the one reason to skip it outright: the window itself is gone.
    let tick_stage = if cancelled {
        // No stage is queued, so the status must be TERMINAL: `Pending` (compose()'s default)
        // asserts a stage is in flight, and none is. `Failed` reads honestly — whatever a retry
        // would have answered, it never ran.
        series.tick_status = TickStatus::Failed;
        None
    } else {
        stage_and_stamp(venue, request.window, &key, &mut series)
    };
    if complete {
        // Settled exactly when no stage was queued — see the SQLite-hit branch above for why a
        // terminal `stage_and_stamp` result never needs re-deciding, while a queued stage's
        // `Pending` must be.
        remember_store(
            cache,
            key,
            Remembered::Ready {
                series: series.clone(),
                ticks_settled: tick_stage.is_none(),
            },
        );
    }
    Served {
        outcome: TradeReplayOutcome::Ready(series),
        tick_stage,
    }
}

/// Decide the tick stage for a just-built candle series, and stamp its own `tick_status` in place
/// to match — `Pending` when a stage is queued, or the reason it is not, via [`tick_stage_for`].
///
/// One helper for the three sites in [`serve`] that each produce a fresh candle answer: the
/// ring-hit-but-not-settled branch, the SQLite-cache-hit branch, and the completed-network-fetch
/// branch — the last of these calls it only on its NON-CANCELLED path; the cancelled sub-branch
/// skips it entirely and stamps [`TickStatus::Failed`] directly, since there is no fresh route
/// decision to make for a window that is already gone. `series.tick_status` already reads
/// `Pending` from [`compose`], so this only overwrites it when a stage is NOT queued.
///
/// Args:
///     venue: Venue the candles came from.
///     window: The window the candles cover.
///     key: The ring key this stage would replace on success.
///     series: The just-built series; its `tick_status` is overwritten in place when no stage is
///         queued for it.
///
/// Returns:
///     The stage to queue, or `None`.
fn stage_and_stamp(
    venue: crate::venue::Venue,
    window: ReplayWindow,
    key: &OutcomeKey,
    series: &mut TradeReplaySeries,
) -> Option<TickStage> {
    match tick_stage_for(venue, window, key, &series.candles) {
        Ok(mut stage) => {
            if series.source.is_ticks() {
                stage.baseline = Some(series.clone());
                series.tick_status = TickStatus::Streaming;
            }
            Some(stage)
        }
        Err(status) => {
            series.tick_status = status;
            None
        }
    }
}

/// Decide whether a just-built CANDLE series earns a queued tick upgrade, or the reason it does
/// not.
///
/// The "already settled" short-circuit this used to take as a parameter no longer lives here: it
/// is checked once, in [`serve`]'s ring-hit branch, before this is ever called — a settled entry
/// is sent exactly as stored, with no stage queued and nothing here re-decided.
///
/// A clock that cannot be read (`now_unix_ms_i64` answering `0`) is treated as INSIDE retention
/// rather than refused, the same permissive default [`serve`] already applies to the closed-bar
/// drop above: nothing here can prove the window is too old, so nothing here refuses it.
///
/// Args:
///     venue: Venue the candles came from.
///     window: The window the candles cover.
///     key: The ring key this stage would replace on success.
///     candles: The exchange klines just composed, carried forward as the eventual tick series'
///         bar layer — see [`TickStage::candles`].
///
/// Returns:
///     The stage to queue, or the reason it is not queued.
fn tick_stage_for(
    venue: crate::venue::Venue,
    window: ReplayWindow,
    key: &OutcomeKey,
    candles: &[ChartCandle],
) -> Result<TickStage, TickStatus> {
    // No public route is not a refusal any more: the stage still runs, against the tile store
    // and its disk alone, and prints `NoRoute` itself when they hold nothing for the focus.
    let route = trade_route(venue);
    let now_ms = crate::util::time::now_unix_ms_i64();
    if let Some(route) = route {
        if now_ms > 0 && !inside_retention(route, window, now_ms) {
            // `inside_retention` is false here only when the route documents a retention: it
            // is unconditionally true otherwise, so this default is never actually reached —
            // see its own doc comment.
            let retention_ms = route.retention_ms().unwrap_or(0);
            return Err(TickStatus::OutOfRetention { retention_ms });
        }
    }
    Ok(TickStage {
        baseline: None,
        route,
        key: key.clone(),
        candles: candles.to_vec(),
    })
}

/// Whether a window is within a trade route's own documented retention.
///
/// Judges the FOCUS's own right edge — the trade's EXIT ([`ReplayWindow::focus`]) — never the
/// window's padded `from_ms` (D2-3), and never the focus's left edge either: the lead context is
/// optional padding, but the trade itself is not, and [`tick_plan`]'s own retention clip already
/// asks only that the exit be inside retention, clipping everything older. Judging the entry
/// instead is a STRICTLY STRONGER check that runs first, in [`tick_stage_for`], and made
/// `tick_plan`'s whole retention-clipping recovery path unreachable for exactly the windows it was
/// written to rescue: a Binance futures trade held ~10 h and closed 40 h ago (retention 48 h) was
/// refused outright although its exit's ticks were comfortably inside retention.
///
/// Free, and evaluated BEFORE any request is spent — see [`tick_stage_for`], the only caller.
///
/// Args:
///     route: The trade route in question.
///     window: The window to check.
///     now_ms: Current Unix time in milliseconds.
///
/// Returns:
///     `true` when the route documents no retention limit, or when the focus's own right edge
///     falls inside the one it does document.
pub(crate) fn inside_retention(route: TradeRoute, window: ReplayWindow, now_ms: i64) -> bool {
    route
        .retention_ms()
        .is_none_or(|r| window.focus().1 >= now_ms - r)
}

/// Run one queued tick stage to completion.
///
/// The tile store answers first: the stage's plan is reduced to what no earlier window already
/// fetched ([`residual_plan`]), and only that remainder is walked — a window whose focus the
/// store holds whole sends nothing, claims no permit and paces nothing. Whatever the walk brings
/// back is filed into the store before the series is composed, so the composition is ONE path,
/// always from the tiles, whether the prints came from this walk, from an earlier one, or both.
/// Trade-only tiles use a bounded extended allowance before optional focus margins are fetched.
///
/// Args:
///     agent: Shared HTTP client.
///     gate: Per-host pacing.
///     request: The original request this stage upgrades.
///     stage: Which route, cache key and bar layer this stage answers.
///     tiles: The worker's tile store.
///
/// Returns:
///     `Ok((series, retry_on_reopen))` with the tick series to send as the SECOND outcome —
///     `retry_on_reopen` is carried out here so the caller can decide whether this answer is safe
///     to remember settled (see [`run`]'s `Job::Ticks` arm): it is set when the venue itself
///     refused mid-walk ([`TickHarvest::venue_refused`]), and when the walk was abandoned but the
///     store still had part of the focus to serve — the missing part is still owed.
///     `Err(Some(status))` when the stage ended for a reason the window must print instead — the
///     caller composes that second outcome from [`TickStage::candles`] carrying `status`.
///     `Err(None)` only for a cancelled window, where the requester is already gone and nothing
///     more is sent.
fn serve_ticks(
    agent: &ureq::Agent,
    gate: &ReplayGate,
    request: &TradeReplayRequest,
    stage: &TickStage,
    tiles: &Mutex<TickTileStore>,
) -> Result<(TradeReplaySeries, bool), Option<TickStatus>> {
    // The archive may have arrived while other candle jobs had priority in the worker queue.
    // Answered straight from the ring, not filed: what the ring holds of a closed trade was
    // filed when the trade closed (`capture_from_core`), and a thinned answer is not a tile.
    let baseline_coverage = stage
        .baseline
        .as_ref()
        .map(|series| series.covered.clone())
        .unwrap_or_default();
    if let Some(mut series) =
        read_core(request).filter(|series| preserves_coverage(&series.covered, &baseline_coverage))
    {
        series.candles = stage.candles.clone();
        return Ok((series, false));
    }
    let key: TileKey = (request.address.exchange_key.clone(), request.market.clone());
    let focus = request.window.focus_spans();
    let persisted = super::trade_cache::handle();
    let Some(route) = stage.route else {
        hydrate(tiles, persisted.as_ref(), &key, request, &focus);
        // No venue to ask: the focus is served from what the tiles hold inside it — a capture
        // from the core's archive — or the window prints that there is no route, as before.
        let (covered, runs) = {
            let store = lock_tiles(tiles);
            let covered = held_coverage(&store, &key, &focus, Coverage::none());
            if covered.is_empty() {
                return Err(Some(TickStatus::NoRoute));
            }
            let runs = read_coverage(&store, &key, &covered);
            (covered, runs)
        };
        let (ticks, side_slots) = flatten_runs(runs, request.tick_value);
        if ticks.is_empty() {
            return Err(Some(TickStatus::NoRoute));
        }
        let partial = !covered.contains((request.window.from_ms, request.window.to_ms));
        let (ticks, bucket_ms) = fit_ticks(ticks, TICK_BUDGET);
        return Ok((
            compose_ticks(
                request,
                request.address.venue,
                ticks,
                bucket_ms,
                side_slots,
                partial,
                covered,
                stage.candles.clone(),
            ),
            // Not settled: a later capture (the settle pass, a neighbouring trade) may widen
            // what the tiles hold, and a reopen should see it.
            true,
        ));
    };
    let deadline = Instant::now() + JOB_DEADLINE;
    // Re-derived rather than trusted from `tick_stage_for`'s own permissive pass: that check ran
    // BEFORE this stage was even queued, and a clock that could not be read then still cannot
    // prove the window is too old now, so the same `now_ms > 0` guard applies here.
    let now_ms = crate::util::time::now_unix_ms_i64();
    let earliest_ms = match now_ms > 0 {
        true => route.retention_ms().map(|r| now_ms - r),
        false => None,
    };
    let trade_deadline = deadline + (TRADE_DEADLINE - JOB_DEADLINE);
    let plan = tick_plan(request.window, route, earliest_ms);
    if plan.slices.is_empty() {
        // The FOCUS itself — the trade, not its optional context — lies entirely before
        // `earliest_ms`: nothing worth fetching remains, so this is reported as retention rather
        // than as an empty venue answer.
        return Err(Some(TickStatus::OutOfRetention {
            retention_ms: route.retention_ms().unwrap_or(0),
        }));
    }
    // After the retention refusal, which is free: a window too old for the route pays no read.
    hydrate(tiles, persisted.as_ref(), &key, request, &focus);
    let residual = residual_plan(&plan, &lock_tiles(tiles), &key);
    // The one line that tells a neighbouring window apart from a reopen: the focus is the
    // window's own, the spans are what the store made of it. In milliseconds, not slices — a
    // held tile in the middle of a slice splits it into two residual spans, so a slice count
    // would read as "nothing held" exactly when something was.
    let span_ms = |slices: &[(i64, i64)]| -> i64 {
        slices
            .iter()
            .map(|(from_ms, to_ms)| to_ms - from_ms + 1)
            .sum()
    };
    let plan_ms = span_ms(&plan.slices);
    let residual_ms = span_ms(&residual.slices);
    log::info!(
        "[x] trade-replay tick stage {} focus={focus}: {} of {} ms held, {} ms in {} spans to fetch",
        request.market,
        plan_ms - residual_ms,
        plan_ms,
        residual_ms,
        residual.slices.len()
    );
    // Whether a reopen must re-walk: the venue refused mid-walk, or the walk was abandoned and
    // what follows is served from the store alone.
    let mut retry_on_reopen = false;
    // The reason the walk stopped without a harvest, when it did — what to print if the store
    // cannot stand in for it either.
    let mut abandoned: Option<TickAbandon> = None;
    let mut complete = true;
    // What this walk brought back, kept OUT of the store until the answer is composed: the
    // stretches it is exhaustive over — the seeds the served coverage grows from — and its
    // prints, clipped to them. An empty walk over a completed residual has stretches and no
    // prints.
    let mut harvest_coverage: Option<Coverage> = None;
    let mut harvest_ticks: Vec<Tick> = Vec::new();
    if !residual.slices.is_empty() {
        let mut last_progress = None;
        let published_coverage = RefCell::new(baseline_coverage);
        let progress_key = key.clone();
        let mut publish_progress = |ticks: &[Tick], covered: &Coverage| {
            let now = Instant::now();
            if request.cancel.load(Ordering::Relaxed)
                || last_progress
                    .is_some_and(|last| now.duration_since(last) < Duration::from_millis(500))
            {
                return;
            }
            // The walk's own stretches, widened over the tiles that abut them: the second
            // window's stream shows what the first already fetched from the first snapshot on,
            // not only the remainder this walk is filling in.
            let (run, mut runs) = {
                let store = lock_tiles(tiles);
                let run = held_coverage(&store, &progress_key, &focus, covered.clone());
                let held = read_coverage(&store, &progress_key, &run);
                (run, held)
            };
            if !preserves_coverage(&run, &published_coverage.borrow()) {
                return;
            }
            runs.push((
                TileSource::Venue,
                ticks
                    .iter()
                    .copied()
                    .filter(|tick| {
                        let time_ms = tick.time_ms as i64;
                        covered.contains_ms(time_ms) && run.contains_ms(time_ms)
                    })
                    .collect(),
            ));
            let (points, side_slots) = flatten_runs(runs, request.tick_value);
            if points.is_empty() {
                return;
            }
            let (points, bucket_ms) = fit_ticks(points, TICK_BUDGET);
            let mut series = compose_ticks(
                request,
                request.address.venue,
                points,
                bucket_ms,
                side_slots,
                true,
                run.clone(),
                stage.candles.clone(),
            );
            series.tick_status = TickStatus::Streaming;
            if request
                .reply
                .send(TradeReplayOutcome::Ready(series))
                .is_ok()
            {
                last_progress = Some(now);
                *published_coverage.borrow_mut() = run;
            }
        };
        let mut observer = GateObserver {
            gate,
            host: route.host(),
            progress: &mut publish_progress,
        };
        let upgrade = CoreUpgradeProbe::new(Instant::now());
        let verdict = paginate_ticks(
            route,
            &residual,
            TICK_BUDGET,
            TICK_PAGE_BUDGET,
            || {
                upgrade.stop(
                    request.cancel.load(Ordering::Relaxed),
                    Instant::now(),
                    &published_coverage.borrow(),
                    || read_core(request),
                )
            },
            |trade| Instant::now() >= if trade { trade_deadline } else { deadline },
            &mut observer,
            |from_ms, to_ms, cursor| {
                rest::fetch_trades(agent, route, &request.market, from_ms, to_ms, cursor)
            },
        );
        if let Some(mut series) = upgrade.ready.into_inner() {
            // The paginator stopped on our replacement, not a host refusal. Its paid-for rows are
            // superseded by the core span, and this attempt must not leave the exchange in backoff.
            gate.clear(route.host());
            if request.cancel.load(Ordering::Relaxed) {
                return Err(None);
            }
            series.candles = stage.candles.clone();
            return Ok((series, false));
        }
        match verdict {
            TickVerdict::Ready(harvest) => {
                let TickHarvest {
                    mut ticks,
                    covered,
                    complete: walked_whole,
                    venue_refused,
                } = harvest;
                // The venue answered without refusing anywhere along the walk, so its refusal
                // history is stale — exactly the candle stage's own `gate.clear` above. A refusal
                // it gave us mid-walk (`venue_refused`) must stand, or the next request to this
                // host sends blind into a burst it just declined.
                if !venue_refused {
                    gate.clear(route.host());
                }
                // Clipped to what the walk actually finished (`covered`), not to the request
                // window: a walk cut short still holds a complete answer for the slices it
                // actually walked, and clipping to the wider window would let a stray
                // page-overshoot outside `covered` back in. Each stretch `paginate_ticks`
                // reports is exhaustive over its own completed slices; what the store held
                // between two of them is bridged below, over the store.
                ticks.retain(|t| t.time_ms.is_finite() && covered.contains_ms(t.time_ms as i64));
                harvest_coverage = Some(covered);
                harvest_ticks = ticks;
                retry_on_reopen = venue_refused;
                complete = walked_whole;
            }
            TickVerdict::Abandoned(reason) => {
                // RELEASE THE PERMIT THIS STAGE TOOK, unless the venue is the reason we stopped.
                //
                // `TickObserver::claim` records a real attempt on the host's shared claim map,
                // and only an explicit `clear` erases it. So an abandonment that is OUR OWN
                // doing — the user closed the window, the job deadline expired, either budget
                // was crossed, the market was simply quiet, or the venue answered that it does
                // not list this symbol — would otherwise leave that attempt standing and put the
                // host into 30-600 s of backoff. The next request to the SAME host is then
                // refused, and because one host serves several venues that request belongs to
                // an unrelated trade, and is usually a CANDLE stage that would have worked. The
                // candle path one function up makes exactly this distinction already: it clears
                // unconditionally after its own loop, its own cancellation break included, and
                // clears on `UnknownSymbol` for the stated reason that "one bad market would
                // throttle every other market on that host".
                //
                // TWO reasons keep the record, and both are the venue's own word rather than
                // ours: `Transient` is a refusal or failure it just gave us, and `RateLimited`
                // means our claim was REFUSED — we recorded nothing, so clearing would erase
                // somebody else's legitimate backoff.
                match reason {
                    TickAbandon::Transient | TickAbandon::RateLimited => {}
                    TickAbandon::Cancelled
                    | TickAbandon::Deadline
                    | TickAbandon::Empty
                    | TickAbandon::UnknownSymbol
                    | TickAbandon::OverPageBudget
                    | TickAbandon::OverTickBudget => gate.clear(route.host()),
                }
                log::info!(
                    "[x] trade-replay tick stage abandoned on {}: {reason:?}",
                    route.host()
                );
                match reason {
                    TickAbandon::Cancelled => return Err(None),
                    // `Empty` is reached only when EVERY slice of the residual was walked to
                    // completion and none held a print: an authoritative answer for each of
                    // them, filed below as empty tiles so a later window inherits it instead
                    // of asking the venue again. The residual's own slices, coalesced where
                    // they abut — never a hull over them: a long position's two neighbourhoods
                    // have unwalked hours between them, and a stretch the store already held
                    // between two residual slices is bridged over the store below.
                    TickAbandon::Empty => {
                        harvest_coverage =
                            Some(Coverage::from_spans(residual.slices.iter().copied()));
                    }
                    _ => {
                        abandoned = Some(reason);
                        retry_on_reopen = true;
                        complete = false;
                    }
                }
            }
        }
    }
    // Composed from what THIS round holds — the store's tiles plus this walk's own harvest —
    // and composed BEFORE the harvest is filed: filing evicts, and an eviction, whichever key it
    // lands on, must never reach into the answer being composed. The coverage is the walk's own
    // stretches grown over the tiles abutting them, plus every run the store holds inside the
    // focus, and all of it clipped to the focus, so a neighbouring window's wider harvest never
    // widens this window's points beyond what its own plan asked.
    let (covered, mut runs) = {
        let store = lock_tiles(tiles);
        let covered = held_coverage(
            &store,
            &key,
            &focus,
            harvest_coverage.clone().unwrap_or_default(),
        );
        if covered.is_empty() {
            // Nothing held around the trade and nothing fetched: only an abandoned walk gets
            // here, and its reason is what the window prints.
            return Err(Some(TickStatus::Failed));
        }
        let runs = read_coverage(&store, &key, &covered);
        (covered, runs)
    };
    // The store held nothing inside the harvest's own stretches — that is what made them
    // residual — so the two sets are disjoint and their union double-counts no print.
    runs.push((
        TileSource::Venue,
        harvest_ticks
            .iter()
            .copied()
            .filter(|tick| covered.contains_ms(tick.time_ms as i64))
            .collect(),
    ));
    let (ticks, side_slots) = flatten_runs(runs, request.tick_value);
    if let Some(harvest) = harvest_coverage {
        // One insert per stretch, each with its own prints: the disk files only the parts it
        // does not hold, by the same gap rule as the memory, so a print never lands twice — and
        // the unwalked ground between two stretches is filed by neither.
        for &(from_ms, to_ms) in harvest.spans() {
            let inside: Vec<Tick> = harvest_ticks
                .iter()
                .copied()
                .filter(|tick| {
                    let time_ms = tick.time_ms as i64;
                    time_ms >= from_ms && time_ms <= to_ms
                })
                .collect();
            if let Some(cache) = &persisted {
                cache.insert(
                    &request.address.exchange_key,
                    &request.market,
                    from_ms,
                    to_ms,
                    inside.clone(),
                    TileSource::Venue,
                );
            }
            lock_tiles(tiles).insert(key.clone(), from_ms, to_ms, inside, TileSource::Venue);
        }
    }
    if ticks.is_empty() {
        // The covered run holds no print. With a walk abandoned this round that is not an
        // answer — the store's part is empty and the venue's part never arrived, so a retry is
        // honest. Otherwise it is authoritative: every stretch of the run was asked and answered
        // empty, and no retry can change it.
        return Err(Some(match abandoned {
            Some(_) => TickStatus::Failed,
            None => TickStatus::NoTrades,
        }));
    }
    // `partial` must reflect what `covered` actually spans, not merely whether the walk finished.
    // `tick_plan`'s own `earliest_ms` clip can make the PLAN narrower than `request.window` before
    // the walk even starts, so a retention-clipped plan that completes still leaves the served
    // ticks short of the requested window on one or both edges.
    let partial = !complete || !covered.contains((request.window.from_ms, request.window.to_ms));
    let (ticks, bucket_ms) = fit_ticks(ticks, TICK_BUDGET);
    Ok((
        compose_ticks(
            request,
            request.address.venue,
            ticks,
            bucket_ms,
            side_slots,
            partial,
            covered,
            stage.candles.clone(),
        ),
        retry_on_reopen,
    ))
}

/// What the store proves exhaustive inside `focus`, seeded by a walk's own `harvest`: every
/// harvest stretch widened over the tiles abutting it, plus every run of tiles the store holds
/// inside the focus, all clipped to the focus.
///
/// The walk's stretches are not in the store yet — the answer is composed before they are
/// filed — so they seed the extension rather than being found by it. Two stretches a held tile
/// sits between coalesce through that tile; two with unwalked ground between them stay two.
///
/// Args:
///     store: The worker's tiles.
///     key: Exchange key and market.
///     focus: The window's own focus spans, the outer bound of what is served.
///     harvest: The walk's stretches, or none when nothing was walked.
///
/// Returns:
///     The served coverage; empty when neither the store nor the walk holds any of the focus.
fn held_coverage(
    store: &TickTileStore,
    key: &TileKey,
    focus: &Coverage,
    harvest: Coverage,
) -> Coverage {
    let mut runs = Coverage::none();
    for &span in harvest.spans() {
        runs.add(store.extend_over(key, span));
    }
    for &span in focus.spans() {
        for run in store.coverage_runs(key, span) {
            runs.add(run);
        }
    }
    runs.clip(focus)
}

/// Every held run of prints inside `covered`, one entry per tile with its source, in ascending
/// order across the stretches.
fn read_coverage(
    store: &TickTileStore,
    key: &TileKey,
    covered: &Coverage,
) -> Vec<(TileSource, Vec<Tick>)> {
    let mut runs = Vec::new();
    for &(from_ms, to_ms) in covered.spans() {
        runs.extend(store.read_by_source(key, from_ms, to_ms));
    }
    runs
}

/// One ascending run of prints for the chart, and the band's per-second slots summed over every
/// tile through ONE valuation — the market's own terms (`tick_value`): a core's ring reports the
/// same wire quantity as the venue's route (contracts on a contract market), so a tile's source
/// is provenance, never a different unit. Summing per tile and merging keeps a tile's slots
/// whole when the run is later thinned for drawing.
///
/// Args:
///     runs: Tile runs in any order, each with its source.
///     value: How this market's prints are valued.
///
/// Returns:
///     Every print ascending by time, and the merged slots.
fn flatten_runs(
    runs: Vec<(TileSource, Vec<Tick>)>,
    value: super::venue_caps::TickValue,
) -> (Vec<Tick>, Vec<crate::market::source::SideSlot>) {
    let mut ticks = Vec::new();
    let mut slots = Vec::new();
    for (_, run) in runs {
        slots.extend(crate::market::source::side_slots_of_ticks(&run, value));
        ticks.extend(run);
    }
    ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    (ticks, crate::market::source::merge_side_slots(slots))
}

/// Hydrate the tile store from the disk around one focus.
///
/// The disk is the tile store's memory across a restart: whatever it holds around the focus is
/// inserted FIRST, so what the stage decides is decided against everything ever fetched or
/// captured for this market, not only what this session saw. A read that times out hydrates
/// nothing and costs at worst a fetch the disk could have spared.
fn hydrate(
    tiles: &Mutex<TickTileStore>,
    persisted: Option<&super::trade_cache::TradeCache>,
    key: &TileKey,
    request: &TradeReplayRequest,
    focus: &Coverage,
) {
    let Some(cache) = persisted else {
        return;
    };
    let mut store = lock_tiles(tiles);
    for &(from_ms, to_ms) in focus.spans() {
        let spans = cache
            .read(
                &request.address.exchange_key,
                &request.market,
                from_ms,
                to_ms,
            )
            .unwrap_or_default();
        for span in spans {
            store.insert(
                key.clone(),
                span.from_ms,
                span.to_ms,
                span.ticks,
                span.source,
            );
        }
    }
}

/// What a close-time capture copies: the stretches the trade's own window asks for as ticks
/// ([`ReplayWindow::focus_spans`]), so a long position files only its two neighbourhoods — the
/// hours between them, which no window serves, stay out of the store's tick ceiling and off the
/// disk. Before the settle pass the trail has not printed yet, so the stretches end at the exit.
///
/// Args:
///     request: The trade and its core.
///     settle: Whether this is the settle pass, which includes the trail after the exit.
///
/// Returns:
///     The stretches to copy, ascending; one or two.
fn capture_spans(request: &CaptureRequest, settle: bool) -> Coverage {
    let margin = request.margin_ms.max(0);
    let window = ReplayWindow {
        from_ms: request.open_ms.saturating_sub(margin),
        to_ms: request.close_ms.saturating_add(margin),
        open_ms: request.open_ms,
        close_ms: request.close_ms,
        margin_ms: margin,
        over_budget: false,
    };
    let spans = window.focus_spans();
    match settle {
        true => spans,
        false => spans.clip(&Coverage::one((window.from_ms, request.close_ms))),
    }
}

/// What the settle pass of a capture copies and when it is due, or `None` when there is nothing
/// to settle: the settle spans reach past the exit only when the margin gives the trade a trail,
/// and a margin of zero does not — scheduling a pass that copied the same stretch again would
/// schedule itself forever.
///
/// Args:
///     request: The trade and its core.
///
/// Returns:
///     The settle spans and the true-UTC millisecond they are due at (the trail's end plus
///     [`CAPTURE_SETTLE_SLACK`]).
fn settle_plan(request: &CaptureRequest) -> Option<(Coverage, i64)> {
    let settle = capture_spans(request, true);
    let trail_end = settle.hull().map(|hull| hull.1)?;
    if trail_end <= request.close_ms {
        return None;
    }
    let due_ms = trail_end.saturating_add(CAPTURE_SETTLE_SLACK.as_millis() as i64);
    Some((settle, due_ms))
}

/// Copy `span` of one market out of the closing core's retained archive into the tile store and
/// its disk, as a [`TileSource::Core`] tile over what the archive actually held.
///
/// Silent when the archive holds nothing there — a market the core does not follow, or a ring
/// that has already moved past the span; the next window pages the venue as it always did.
///
/// Args:
///     request: The trade and its core.
///     span: The stretch to copy, inclusive, true-UTC milliseconds.
///     tiles: The worker's tile store.
fn capture_from_core(request: &CaptureRequest, span: (i64, i64), tiles: &Mutex<TickTileStore>) {
    if span.0 > span.1 {
        return;
    }
    // Overlap, not bracket: the ring is contiguous, so whatever it holds inside the span is
    // exhaustive whatever its edges, and the copy is clipped to that — a ring lagging behind
    // the span's end at close time files up to its last print, and the settle pass files the
    // rest.
    let Some(native) = request.address.history.capture_core_span(
        &request.address,
        &request.market,
        span.0,
        span.1,
    ) else {
        log::debug!(
            "[x] trade-replay capture {} span={}..{}: the core archive holds nothing there",
            request.market,
            span.0,
            span.1
        );
        return;
    };
    let (from_ms, to_ms) = native.covered;
    if from_ms > to_ms {
        return;
    }
    log::info!(
        "[x] trade-replay capture {} span={}..{}: {} prints from the core archive, covered={}..{}",
        request.market,
        span.0,
        span.1,
        native.ticks.len(),
        from_ms,
        to_ms
    );
    let key: TileKey = (request.address.exchange_key.clone(), request.market.clone());
    if let Some(cache) = super::trade_cache::handle() {
        cache.insert(
            &request.address.exchange_key,
            &request.market,
            from_ms,
            to_ms,
            native.ticks.clone(),
            TileSource::Core,
        );
    }
    lock_tiles(tiles).insert(key, from_ms, to_ms, native.ticks, TileSource::Core);
}

/// The worker's tile store, poison-tolerant like the outcome ring: nothing inside a tile can be
/// half-written by a panic, so a poisoned lock still holds a consistent store.
fn lock_tiles(tiles: &Mutex<TickTileStore>) -> std::sync::MutexGuard<'_, TickTileStore> {
    tiles
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Walk every tile of one tick stage's [`TickPlan`], paginating each with the venue's own cursor,
/// until the plan is exhausted or a stop condition is reached — and, unlike a candle job, a stop
/// never discards what was already collected except when the window itself closed.
///
/// `fetch` is the injected seam — no network, no clock, no gate inside this function, which is
/// what makes it testable with a fake fetcher and a fake [`TickObserver`].
///
/// The loop rule, in order of precedence (D2-2, replacing this function's earlier design in
/// full):
/// - [`cancelled`] stops EVERYTHING, always, and discards whatever was collected — the window is
///   gone and there is no one left to serve it to.
/// - The normal deadline and page budget cannot interrupt the leading trade-only tiles. These
///   use the bounded trade deadline and [`TRADE_PAGE_BUDGET`] allowance instead. After the trade
///   completes, normal limits apply immediately, before any optional margin is requested.
///   Hard stops retain the truthful partial harvest even if the entire trade could not fit.
/// - A venue's own answer — `Transient`/`UnknownSymbol` — also stops the walk rather than the
///   whole stage, and marks the harvest [`TickHarvest::venue_refused`], so [`serve_ticks`] knows
///   not to clear a refusal the venue just gave it.
/// - The FOCUS tiles (the leading [`TickPlan::focus_len`] entries) are never truncated by the
///   tick budget: it is checked only around a NON-focus tile, before it starts and again once it
///   finishes, so a tile is walked whole or not at all — never cut mid-body.
/// - Every page is clipped to the SLICE it was fetched for before it is counted toward any budget
///   (D2-1): Binance's forward pager and OKX's backward one both routinely return a page that
///   overshoots its own slice edge, and [`Tick`] carries no exchange trade id, so an unclipped
///   overlap between two adjacent slices is undetectable once concatenated, not merely unnoticed.
///   The aggregate clip to [`TickHarvest::covered`] in [`serve_ticks`] is the OUTER bound and does
///   not replace this inner one.
/// - The verdict is one question: is the harvest empty? A non-empty one is always `Ready`,
///   whatever stopped the walk; only an empty one reaches [`TickVerdict::Abandoned`], carrying
///   whichever reason actually stopped it.
///
/// Args:
///     route: Which venue endpoint this stage answers.
///     plan: The window's own [`tick_plan`] output — tiles in fetch-priority order, with the
///         first [`TickPlan::focus_len`] of them being the trade's own focus.
///     tick_budget: Ceiling on the total ticks collected before a non-focus tile is skipped.
///     page_budget: Normal page ceiling; trade tiles get at least [`TRADE_PAGE_BUDGET`].
///     cancelled: Answers whether the requester's window has closed.
///     expired: Answers whether the deadline has passed; `true` selects the hard trade deadline.
///     observer: Records the `claim`/`pace` calls this stage makes.
///     fetch: Fetches one page for a given slice and cursor.
///
/// Returns:
///     The harvest, or the reason nothing was collected.
pub(crate) fn paginate_ticks<F, O>(
    route: TradeRoute,
    plan: &TickPlan,
    tick_budget: usize,
    page_budget: usize,
    cancelled: impl Fn() -> bool,
    expired: impl Fn(bool) -> bool,
    observer: &mut O,
    mut fetch: F,
) -> TickVerdict
where
    F: FnMut(i64, i64, Option<rest::TradeCursor>) -> Result<rest::TradePage, rest::FetchError>,
    O: TickObserver,
{
    if plan.slices.is_empty() {
        return TickVerdict::Abandoned(TickAbandon::Empty);
    }
    if observer.claim(route.host()).is_err() {
        return TickVerdict::Abandoned(TickAbandon::RateLimited);
    }
    let mut ticks: Vec<Tick> = Vec::new();
    let mut pages_fetched = 0usize;
    // Completed tiles, coalesced where they abut: one stretch while the walk stays contiguous,
    // two once it crosses to a long position's other neighbourhood.
    let mut covered = Coverage::none();
    let mut complete = true;
    let mut venue_refused = false;
    let mut stop_reason: Option<TickAbandon> = None;
    // The tile still being walked when a `break 'walk` fired mid-body — its own bounds, where its
    // rows begin in `ticks`, and the cursor most recently used for it — so its paid-for rows can
    // extend `covered` afterward rather than being reclaimed by the clip in `serve_ticks` (F2).
    // `None` whenever every stop happened BETWEEN tiles (or the walk was cancelled outright, which
    // returns before this is ever read).
    let mut interrupted: Option<(i64, i64, usize, Option<rest::TradeCursor>)> = None;

    'walk: for (index, &(slice_from, slice_to)) in plan.slices.iter().enumerate() {
        let is_focus = index < plan.focus_len;
        let is_trade = index < plan.trade_len;
        // The tick budget never truncates a focus slice — checked only around a NON-focus one, so
        // a slice is whole or absent rather than cut mid-body. See the after-check below for the
        // other half of this rule.
        if !is_focus && ticks.len() >= tick_budget {
            complete = false;
            stop_reason = Some(TickAbandon::OverTickBudget);
            break;
        }
        let start_len = ticks.len();
        let mut cursor: Option<rest::TradeCursor> = None;
        loop {
            if cancelled() {
                // The window is gone; nothing collected so far is worth keeping.
                return TickVerdict::Abandoned(TickAbandon::Cancelled);
            }
            if expired(is_trade) {
                complete = false;
                stop_reason = Some(TickAbandon::Deadline);
                interrupted = Some((slice_from, slice_to, start_len, cursor));
                break 'walk;
            }
            let page_limit = if is_trade {
                page_budget.max(TRADE_PAGE_BUDGET)
            } else {
                page_budget
            };
            if pages_fetched >= page_limit {
                complete = false;
                stop_reason = Some(TickAbandon::OverPageBudget);
                interrupted = Some((slice_from, slice_to, start_len, cursor));
                break 'walk;
            }
            observer.pace(route.host());
            let page = match fetch(slice_from, slice_to, cursor) {
                Ok(page) => page,
                Err(rest::FetchError::UnknownSymbol) => {
                    complete = false;
                    venue_refused = true;
                    stop_reason = Some(TickAbandon::UnknownSymbol);
                    interrupted = Some((slice_from, slice_to, start_len, cursor));
                    break 'walk;
                }
                Err(rest::FetchError::Transient(_)) => {
                    complete = false;
                    venue_refused = true;
                    stop_reason = Some(TickAbandon::Transient);
                    interrupted = Some((slice_from, slice_to, start_len, cursor));
                    break 'walk;
                }
            };
            pages_fetched += 1;
            let mut rows = page.ticks;
            // D2-1: clip THIS page to the slice it was fetched for, before extending or counting
            // toward the budget — see this function's own doc comment for the vendor evidence.
            rows.retain(|t| {
                t.time_ms.is_finite()
                    && (t.time_ms as i64) >= slice_from
                    && (t.time_ms as i64) <= slice_to
            });
            ticks.extend(rows);
            // The focus can require many pages. Show its already-walked span before the tile
            // completes, but do not publish non-focus tiles that a budget may later discard.
            if is_focus && page.next.is_some() {
                if let Some(span) = walked_part(
                    &covered,
                    (slice_from, slice_to),
                    &ticks[start_len..],
                    page.next,
                ) {
                    let mut so_far = covered.clone();
                    so_far.add(span);
                    observer.progress(&ticks, &so_far);
                }
            }
            match page.next {
                Some(next_cursor) => cursor = Some(next_cursor),
                None => break,
            }
        }
        // The slice's own pagination completed. For a non-focus slice only, a tick budget crossed
        // during it removes the WHOLE slice rather than leaving it half-drawn.
        if !is_focus && ticks.len() > tick_budget {
            ticks.truncate(start_len);
            complete = false;
            stop_reason = Some(TickAbandon::OverTickBudget);
            break;
        }
        covered.add((slice_from, slice_to));
        observer.progress(&ticks, &covered);
    }

    if ticks.is_empty() {
        return TickVerdict::Abandoned(stop_reason.unwrap_or(TickAbandon::Empty));
    }
    // Add the interrupted tile's own paid-for stretch — the part of it its pagination direction
    // proves walked, see `walked_part` — so those rows are served rather than reclaimed by the
    // clip in `serve_ticks`. It coalesces with the completed stretch it abuts, or stands alone.
    if let Some((slice_from, slice_to, start, cursor)) = interrupted {
        if let Some(span) = walked_part(&covered, (slice_from, slice_to), &ticks[start..], cursor) {
            covered.add(span);
        }
    }
    if let Some(
        reason
        @ (TickAbandon::Deadline | TickAbandon::OverPageBudget | TickAbandon::OverTickBudget),
    ) = stop_reason
    {
        log::info!(
            "[x] trade-replay tick stage partial on {}: {reason:?}, covered={covered} ms, pages={pages_fetched}",
            route.host()
        );
    }
    TickVerdict::Ready(TickHarvest {
        ticks,
        covered,
        complete,
        venue_refused,
    })
}

/// The stretch of a partly walked tile its rows prove exhaustive, or `None` when nothing can be
/// said.
///
/// Within one slice a paginated run is contiguous, but its direction is per-venue: Binance's
/// `FromId` cursor walks FORWARD from the tile's own `slice_from`, so the rows so far are every
/// print from that edge to the last one seen; Bitget/OKX's `LessThanId` walks BACKWARD from
/// `slice_to`, so they are every print from the first one seen to that edge. Gate's
/// `Page`/`Offset` cursors carry an UNDOCUMENTED order (`venue_caps.rs`), so their rows prove
/// nothing while a completed stretch exists to keep honest — and only when NO slice completed at
/// all does the observed extent of the rows stand in, which can only UNDER-state true coverage,
/// never claim more than was walked (D2-2). `AfterMs` is treated as undocumented: no current route
/// emits it, so there is no evidence for which edge it walks from.
///
/// Args:
///     covered: The stretches completed so far.
///     slice: The interrupted tile's own bounds.
///     rows: The rows fetched for it so far.
///     cursor: The cursor most recently used for it.
///
/// Returns:
///     The proven stretch, to be added to `covered` — it coalesces with the stretch it abuts
///     or stands alone, so no unwalked ground is ever claimed either way.
fn walked_part(
    covered: &Coverage,
    slice: (i64, i64),
    rows: &[Tick],
    cursor: Option<rest::TradeCursor>,
) -> Option<(i64, i64)> {
    let first = rows.first()?;
    let (lo, hi) = rows.iter().fold(
        (first.time_ms as i64, first.time_ms as i64),
        |(lo, hi), t| (lo.min(t.time_ms as i64), hi.max(t.time_ms as i64)),
    );
    match cursor {
        Some(rest::TradeCursor::FromId(_)) => Some((slice.0, hi)),
        Some(rest::TradeCursor::LessThanId(_)) => Some((lo, slice.1)),
        _ if covered.is_empty() => Some((lo, hi)),
        _ => None,
    }
}

/// Build the frozen TICK series one tick stage answers with.
///
/// `ticks` must already be globally sorted ascending and clipped to the harvest's own
/// [`TickHarvest::covered`] range — this function does neither; [`serve_ticks`] does both before
/// calling it. `candles` is the EXCHANGE'S OWN klines carried forward from the candle stage that
/// ran first ([`TickStage::candles`]), never aggregated from `ticks`: the bar layer covers the
/// whole window even where the points, per `partial`, cover only part of it.
///
/// Args:
///     request: The request being served.
///     venue: Venue the ticks came from.
///     ticks: Trade points, ascending, already clipped to the harvest's covered range.
///     bucket_ms: The bucket [`fit_ticks`] thinned the points to; `0` means raw.
///     partial: Whether `ticks` covers only part of `request.window`.
///     covered: The walk's own exhaustive stretches, carried onto the series verbatim — the
///         chart withholds the bars lying inside them, and only this coverage knows that a
///         covered minute with no trade in it is still covered.
///     candles: The exchange klines to carry as the bar layer.
///
/// Returns:
///     The series to hand the chart.
///
/// `side_slots` is the per-second split of the SAME run before it was thinned
/// (`side_slots_of_ticks`), already merged and ascending; it is carried onto the series verbatim.
#[allow(clippy::too_many_arguments)]
fn compose_ticks(
    request: &TradeReplayRequest,
    venue: crate::venue::Venue,
    ticks: Vec<Tick>,
    bucket_ms: i64,
    side_slots: Vec<crate::market::source::SideSlot>,
    partial: bool,
    covered: Coverage,
    candles: Vec<ChartCandle>,
) -> TradeReplaySeries {
    TradeReplaySeries {
        source: TradeReplaySource::Ticks,
        venue,
        window: request.window,
        tf_ms: BAR_MS,
        candles,
        ticks,
        identity: request.identity,
        tick_status: TickStatus::Served,
        bucket_ms,
        partial,
        side_slots,
        covered,
    }
}

/// Answer the rows one cache write may actually carry, keyed on the series it came from.
///
/// The SQLite isolation seam (acceptance criterion 7): a [`TradeReplaySource::Ticks`] series must
/// NEVER reach [`write_cached_bars`], because that table is the SHARED kline cache the live
/// recorder writes too. In practice no call site ever offers one this way — [`serve`] is the only
/// caller and always passes [`TradeReplaySource::Klines1m`], since [`serve_ticks`] writes nothing
/// back to SQLite at all — but the guard is keyed on the TYPE rather than on that fact, so the
/// invariant survives a future call site instead of depending on every one of them getting it
/// right by omission.
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
        TradeReplaySource::Ticks | TradeReplaySource::CoreTicks => &[],
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
        tick_status: TickStatus::Pending,
        bucket_ms: 0,
        partial: false,
        side_slots: Vec::new(),
        // No tick walk ran, so nothing is covered and the chart keeps every bar.
        covered: Coverage::none(),
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
            // The slots ride the same entry and are not bounded by the tick budget, so they
            // count toward the same cap.
            Remembered::Ready { series, .. } => series.ticks.len() + series.side_slots.len(),
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

#[cfg(test)]
mod tests;
