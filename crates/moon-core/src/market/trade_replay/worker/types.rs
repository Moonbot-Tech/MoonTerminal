//! Replay request types, budgets, queues and tick observation.

use super::*;

/// Milliseconds per one-minute bar.
pub(super) const BAR_MS: i64 = 60_000;

/// Widest gap in cached bars still counted as coverage rather than a hole.
///
/// A quiet market legitimately has minutes with no trade at all, so demanding a bar per minute
/// would send every window to the network forever. Three bars is wide enough for a thin market and
/// narrow enough that a genuinely interrupted fetch is still recognised as incomplete.
pub(super) const MAX_GAP_BARS: i64 = 3;

/// Normal stage deadline; trade-only tick tiles use [`TRADE_DEADLINE`] instead.
///
/// The HTTP client bounds each REQUEST at fifteen seconds, which says nothing about a paginated
/// job: a wide window can be many requests, and without this a single slow venue would hold the
/// one worker — and therefore every later window — for minutes. On expiry the caller is told the
/// fetch is transient, which is true and retryable.
pub(super) const JOB_DEADLINE: Duration = Duration::from_secs(45);

/// Hard deadline for the position itself, so dense trades get priority without blocking forever.
pub(super) const TRADE_DEADLINE: Duration = Duration::from_secs(180);

/// How many answered windows the in-memory outcome cache remembers.
///
/// Small on purpose: this exists so a reopen costs nothing, not to be a history store. Each entry
/// holds one bounded window's rows.
pub(super) const OUTCOME_CACHE_LEN: usize = 8;

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
pub(super) const OUTCOME_CACHE_MAX_TICKS: usize = 4 * TICK_BUDGET;

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
pub(super) const TICK_PAGE_BUDGET: usize = 60;

/// Hard page allowance for trade-only tiles; context never receives this extension.
pub(super) const TRADE_PAGE_BUDGET: usize = 240;

/// What one answered question is remembered as.
///
/// An authoritative EMPTY is an answer too, and a valuable one: a delisted or halted market
/// answers empty every time, so refetching it on each reopen spends the host's budget to learn
/// something already known.
// `Ready` owns the series a reopen draws. Boxing it allocates on every remembered answer.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub(super) enum Remembered {
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
pub(super) struct OutcomeKey {
    /// Venue the rows were fetched for.
    ///
    /// The host ALONE will not do, and the reason is not hypothetical. One host commonly answers
    /// both of a brand's markets — `api.bybit.com`, `api.gateio.ws` and `api.bitget.com` each do —
    /// while the exchange-native market name is frequently IDENTICAL across them: Bybit spot and
    /// Bybit linear are both `BTCUSDT`, and so are BitGet's two. Keyed on the host alone, a spot
    /// replay and a futures replay of the same pair over the same window are one entry, and
    /// whichever ran first serves the other its candles — or its authoritative `Empty`. The venue
    /// is what actually separates them, so it is what the key carries.
    pub(super) venue: crate::venue::Venue,
    /// Host the rows came from, which is the rate-limit budget they were fetched under.
    pub(super) host: &'static str,
    /// Exchange-native market name.
    pub(super) market: String,
    /// Window the rows cover.
    pub(super) from_ms: i64,
    pub(super) to_ms: i64,
    /// Prints asked for around the position ([`ReplayWindow::margin_ms`]): the bar window above
    /// does not depend on it, so without this a settled answer fetched under one margin would be
    /// served unchanged after the Storage tab moved it.
    pub(super) margin_ms: i64,
    /// The long-position threshold ([`ReplayWindow::long_position_ms`]), for the same reason: it
    /// decides whether the prints were walked whole or as two ends.
    pub(super) long_position_ms: i64,
}

/// Which route, cache key and bar layer a queued tick stage answers.
///
/// Carried on [`LaneJob::Ticks`] alongside the original [`TradeReplayRequest`] rather than re-derived
/// when the stage finally runs, so a stage queued behind a long line of candle jobs answers the
/// same question `serve` decided it should — never a re-lookup that could disagree. `candles` is
/// the exchange klines `serve` already composed for this window: carrying them here removes the
/// dependence on the outcome ring not having evicted this key's entry between the two jobs, and is
/// what lets a tick outcome keep the bar layer whole even where its own points, per
/// [`TradeReplaySeries::partial`], cover only part of the window.
#[derive(Clone, Debug)]
pub(crate) struct TickStage {
    /// Previously published cache answer; every retry must retain at least this tick span.
    pub(super) baseline: Option<TradeReplaySeries>,
    /// Which venue endpoint to ask, or `None` on a venue with no public trade route — then
    /// the stage asks nothing and serves what the tile store and its disk already hold for the
    /// focus (a capture from the core's archive, filed when the trade closed — or, for a tiles
    /// reader, filed by this stage itself out of the ring, see [`ReplayIntent::files_core`]),
    /// or prints [`TickStatus::NoRoute`] as before when they hold nothing. A route whose
    /// retention the whole focus is past is served the same way, printing
    /// [`TickStatus::OutOfRetention`] instead.
    pub(super) route: Option<TradeRoute>,
    /// The ring key this stage's answer replaces on success.
    pub(super) key: OutcomeKey,
    /// The exchange klines to carry forward as the bar layer of the eventual tick series.
    pub(super) candles: Vec<ChartCandle>,
}

/// One unit of the coordinator's internal priority queue — everything that is not a venue call.
/// The venue calls themselves ([`LaneJob`]) go to the host's lane the moment they arrive.
pub(crate) enum Job {
    /// Boxed: a request with its wait is several times the size of the other units.
    Native(Box<(TradeReplayRequest, NativeWait)>),
    /// Copy the stretches of a just-closed trade out of the core's retained archive into the
    /// tile store and its disk — see [`CaptureRequest`] and [`capture_spans`]. The flag names
    /// the settle pass, the one that runs after the trail has printed and schedules nothing.
    Capture(CaptureRequest, Coverage, bool),
    /// Answer a [`TickQuery`] from the tiles and the disk alone.
    Held(TickQuery),
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
    /// Prints to copy around the trade, per end — [`super::margin_ms`] at close time, whose
    /// floor is the tuner's run-up and tail; see [`ReplayWindow::margin_ms`].
    pub margin_ms: i64,
    /// The long-position threshold at close time ([`super::long_position_ms`]): carried so the
    /// capture's first pass and its settle pass file the same shape whatever the Storage tab
    /// did between them; see [`ReplayWindow::long_position_ms`].
    pub long_position_ms: i64,
}

/// How long after the exit the settle pass waits past the margin: a few seconds for the core's
/// own feed to catch up to wall time.
pub(super) const CAPTURE_SETTLE_SLACK: Duration = Duration::from_secs(5);

/// A read of what the terminal ALREADY holds for a market over some stretches — the tiles in
/// memory and the disk behind them — with no venue and no core asked.
///
/// The Entry/Exit tuner's question: is this trade's window covered, and if so, hand me the
/// prints. Asked once per report row of a table, so it must cost a lock and a disk read, never
/// a page. It goes to the coordinator's queue, which no venue call ever holds: the walks run
/// on the lanes, so a table of rows is answered while every venue is being paged.
pub struct TickQuery {
    /// The exchange half of the tile key — [`ReplayAddress::exchange_key`].
    pub exchange_key: String,
    /// Exchange-native market name.
    pub market: String,
    /// The stretches asked for, ascending and disjoint.
    pub spans: Coverage,
    /// Where the answer goes. A dead receiver is normal and is not an error.
    pub reply: Sender<TickAnswer>,
}

/// What the terminal holds for a [`TickQuery`].
#[derive(Clone, Debug, Default)]
pub struct TickAnswer {
    /// Every held print inside the covered stretches, ascending by time.
    pub ticks: Vec<Tick>,
    /// The parts of the asked stretches the held prints are exhaustive over. `contains` on it
    /// answers whether a window is covered; a hole inside a span means a fetch would be needed.
    pub covered: Coverage,
}

/// What reaches the coordinator's one inbound channel.
pub(super) enum Inbound {
    Replay(TradeReplayRequest),
    Capture(CaptureRequest),
    Held(TickQuery),
    /// A lane armed a native follow-up for a request it answered; the coordinator polls it.
    /// Boxed for the same reason as [`Job::Native`].
    NativeWait(Box<(TradeReplayRequest, NativeWait)>),
    /// Drop every held tile and remembered answer — see [`forget_tiles`].
    ForgetTiles,
}

/// One unit of a lane's own queue: the venue calls of one request.
///
/// A candle job and its own tick upgrade are two separate units on purpose: queuing the tick
/// stage inline would make a second report-row double-click on the same host wait behind it for
/// its OWN candles — see [`next_lane_job`], which is what keeps candle jobs strictly ahead.
pub(super) enum LaneJob {
    Candles(TradeReplayRequest),
    /// The stage is boxed: it carries the window's bars, several times the request's size.
    Ticks(TradeReplayRequest, Box<TickStage>),
}

/// The handle to one lane thread.
pub(super) struct Lane {
    pub(super) tx: Sender<LaneJob>,
}

/// What a lane serves: one host's calls of one intent.
pub(super) type LaneKey = (&'static str, ReplayIntent);

/// The lane key of a request: the kline route's host — the budget every call of the request
/// is metered under (the trade route derives its host from the same table) — and the intent,
/// so a chart window and the tuner's batch on the same host walk side by side. A venue with no
/// route answers `NoEndpoint` without a call and shares one idle lane per intent.
pub(super) fn lane_key(request: &TradeReplayRequest) -> LaneKey {
    (
        kline_route(request.address.venue)
            .map(|route| route.host())
            .unwrap_or(""),
        request.intent,
    )
}

/// One bounded native follow-up independent of public tick-route eligibility.
pub(crate) struct NativeWait {
    pub(super) fallback: TradeReplayOutcome,
    pub(super) next: Instant,
    pub(super) expires: Instant,
}

impl NativeWait {
    /// Keep the original terminal outcome so a timeout does not leave a loading caption behind.
    pub(super) fn new(fallback: TradeReplayOutcome, now: Instant) -> Self {
        Self {
            fallback,
            next: now + Duration::from_millis(500),
            expires: now + Duration::from_secs(30),
        }
    }

    /// Finish on usable native data or expiry; otherwise retain the original fallback and retry.
    pub(super) fn advance(
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

/// Pop the coordinator's next unit of work, by kind: every pending [`Job::Held`], then every
/// [`Job::Native`], then the captures in arrival order; oldest first within each kind.
///
/// Args:
///     queue: The coordinator's own pending-work deque.
///
/// Returns:
///     The next job to run, or `None` when the queue is empty.
pub(super) fn next_job(queue: &mut VecDeque<Job>) -> Option<Job> {
    // A held-data query costs a lock and a disk read; it goes ahead of the native probes so a
    // table asking once per row is answered at once.
    for pick in [
        |job: &Job| matches!(job, Job::Held(_)),
        |job: &Job| matches!(job, Job::Native(..)),
    ] {
        if let Some(index) = queue.iter().position(pick) {
            return queue.remove(index);
        }
    }
    queue.pop_front()
}

/// Pop a lane's next unit of work: every pending candle job first, then the tick stages in
/// arrival order — a second window on the same host gets its bars before the first window's
/// paging starts.
///
/// Args:
///     queue: The lane's own pending-work deque.
///
/// Returns:
///     The next job to run, or `None` when the queue is empty.
pub(super) fn next_lane_job(queue: &mut VecDeque<LaneJob>) -> Option<LaneJob> {
    if let Some(index) = queue
        .iter()
        .position(|job| matches!(job, LaneJob::Candles(_)))
    {
        return queue.remove(index);
    }
    queue.pop_front()
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
    /// Why the walk stopped short, when it did; `None` for a walk that finished its plan. The
    /// gate reads it: only a `Transient` stop is a refusal to back off from.
    pub stop: Option<TickAbandon>,
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
/// `claim` is a REAL check of the host's refusal history rather than a trust in the candle
/// stage's: that stage ran before this one and may have been answered from a cache without
/// asking the gate at all, so a tick stage that skipped the check would send its bounded
/// requests blind to a refusal recorded for this host by any other request — the exact
/// escalation-to-ban path [`ReplayGate`] exists to prevent.
pub(crate) trait TickObserver {
    fn claim(&mut self, host: &str) -> Result<(), u32>;
    fn pace(&mut self, host: &str);
    /// Whether a page that already came back closed this host for further sends.
    ///
    /// The opening [`Self::claim`] is the one check a stage owes before it sends anything.
    /// This one is the stop a response just installed — a used-weight header over 75%, or a
    /// 429 whose wait was recorded by the fetch — so the next page is not sent into it. A
    /// test double that never records such a stop leaves the default, which is open.
    ///
    /// Args:
    ///     host: The route's host, ignored by a double that has no gate.
    ///
    /// Returns:
    ///     `true` when another page must not be sent.
    fn host_closed(&mut self, _host: &str) -> bool {
        false
    }
    /// Publish the completed stretches so far without claiming unfetched time between tiles.
    fn progress(&mut self, _ticks: &[Tick], _covered: &Coverage) {}
}

/// Callback that publishes a tick snapshot to one replay window.
pub(super) type TickProgress<'a> = dyn FnMut(&[Tick], &Coverage) + 'a;

/// Bridges the pure [`TickObserver`] seam to the real [`ReplayGate`] for production use.
///
/// Holds its own `host` rather than trusting the one handed to each call: [`TickObserver`]'s
/// methods take `&str` so the pure seam stays free of a lifetime a test double has no reason to
/// carry, while [`ReplayGate::claim`] and [`ReplayGate::pace`] need the `'static` the route
/// itself already guarantees. Pinning it at construction resolves that without widening the
/// trait's parameter type.
pub(super) struct GateObserver<'a> {
    pub(super) gate: &'a ReplayGate,
    pub(super) host: &'static str,
    /// The route's own floor between pages — see `TradeRoute::page_interval`.
    pub(super) page_interval: Duration,
    /// Documented weight of one page, spent against the host's budget. Zero for a venue
    /// that is not on the Binance weight ledger.
    pub(super) page_weight: u32,
    pub(super) progress: &'a mut TickProgress<'a>,
}

impl TickObserver for GateObserver<'_> {
    fn claim(&mut self, _host: &str) -> Result<(), u32> {
        self.gate.claim(self.host, Instant::now())
    }

    fn pace(&mut self, _host: &str) {
        self.gate
            .pace_weighted(self.host, self.page_interval, self.page_weight);
    }

    /// The real gate, read without counting as the stage's opening claim.
    fn host_closed(&mut self, _host: &str) -> bool {
        self.gate.claim(self.host, Instant::now()).is_err()
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
    /// Who is asking: decides the walk order of the margins, whether a short answer waits for
    /// the core's archive, and whether the remembered-answer ring is read or written — see
    /// [`ReplayIntent`].
    pub intent: ReplayIntent,
    /// Set by the requester when its window closes; checked between pages.
    pub cancel: Arc<AtomicBool>,
    /// Where the answer goes. A dead receiver is normal and is not an error.
    pub reply: Sender<TradeReplayOutcome>,
}
