//! Coordinator ingress and per-host replay lanes.

use super::*;

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

/// Queue one read of the held prints for some stretches of a market — see [`TickQuery`].
///
/// Returns immediately. The answer arrives on the query's own reply channel, or never, if the
/// asker dropped its receiver first.
///
/// Args:
///     query: The stretches and where to answer.
pub fn query_held(query: TickQuery) {
    send(Inbound::Held(query));
}

/// Drop every tile the worker holds in memory, and every remembered answer with them.
///
/// For the Storage tab, after it cut `trades.sqlite` down: the disk is the tile store's memory
/// and the two must not disagree about what is held — a held-data query reads the tiles first,
/// and would go on answering "held" for prints the file no longer has until the process
/// restarted. Emptied, the tiles fill again from the trimmed disk on the next ask. Returns at
/// once; a lane mid-walk files what it fetched into the emptied store as it always did.
pub fn forget_tiles() {
    send(Inbound::ForgetTiles);
}

pub(super) fn send(inbound: Inbound) {
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Inbound>();
        let back = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("trade-replay".into())
            .spawn(move || run(&rx, back));
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

/// Where an inbound message lands: a replay request on its host's lane, a lane's native
/// follow-up among the coordinator's timed waits, everything else in the coordinator's queue.
pub(super) fn enqueue(
    queue: &mut VecDeque<Job>,
    native_waits: &mut Vec<(TradeReplayRequest, NativeWait)>,
    lanes: &mut HashMap<LaneKey, Lane>,
    shared: &Arc<Shared>,
    inbound: Inbound,
) {
    match inbound {
        Inbound::Replay(request) => {
            let key = lane_key(&request);
            if let std::collections::hash_map::Entry::Vacant(slot) = lanes.entry(key) {
                if let Some(lane) = spawn_lane(key, shared.clone()) {
                    slot.insert(lane);
                }
            }
            // No lane (its thread did not start, now or earlier — the next request tries
            // again), or a send to a lane that is gone: the requester's own timeout is what
            // surfaces it.
            let sent = lanes
                .get(&key)
                .is_some_and(|lane| lane.tx.send(LaneJob::Candles(request)).is_ok());
            if !sent {
                log::warn!("[x] trade-replay request dropped: lane {key:?} is not running");
                lanes.remove(&key);
            }
        }
        Inbound::NativeWait(wait) => native_waits.push(*wait),
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
        Inbound::Held(query) => queue.push_back(Job::Held(query)),
        Inbound::ForgetTiles => {
            // Straight here, not through the queue: nothing queued behind it may keep
            // answering from tiles the disk has already lost.
            *lock_tiles(&shared.tiles) = TickTileStore::default();
            shared
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            log::info!("[x] trade-replay tiles and remembered answers dropped after a trim");
        }
    }
}

/// Coordinator loop: an internal priority queue, forever.
///
/// Held-data queries, bounded native follow-ups and captures share this queue: held-data first,
/// then native probes, then captures in arrival order ([`next_job`]); none of them calls a
/// venue, so none waits for a lane. A replay request is handed to its host's lane on arrival
/// ([`enqueue`]), and a lane hands back the native follow-up it arms. A capture is an in-process
/// copy out of a core's ring, milliseconds; the settle pass of each capture is timed
/// (`settle_waits`) and enters the queue when due. Idle waits end at the next native probe or
/// settle deadline; otherwise every already-queued message is drained non-blockingly first.
///
/// Args:
///     rx: Queue of pending requests.
///     back: A sender to the same queue, for the lanes' follow-ups.
pub(super) fn run(rx: &Receiver<Inbound>, back: Sender<Inbound>) {
    let shared = Arc::new(Shared {
        agent: rest::agent(),
        gate: ReplayGate::new(),
        cache: Mutex::new(VecDeque::new()),
        tiles: Mutex::new(TickTileStore::default()),
        back,
    });
    let tiles = &shared.tiles;
    let mut lanes: HashMap<LaneKey, Lane> = HashMap::new();
    let mut queue: VecDeque<Job> = VecDeque::new();
    let mut native_waits: Vec<(TradeReplayRequest, NativeWait)> = Vec::new();
    // Settle passes of captures, each due once the focus's trail has printed.
    let mut settle_waits: Vec<(CaptureRequest, Coverage, Instant)> = Vec::new();
    loop {
        let now = Instant::now();
        let mut index = 0;
        while index < native_waits.len() {
            if native_waits[index].1.next <= now {
                let wait = native_waits.remove(index);
                queue.push_back(Job::Native(Box::new(wait)));
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
                Ok(inbound) => enqueue(&mut queue, &mut native_waits, &mut lanes, &shared, inbound),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                // Every sender lives inside `WORKER` or a lane, and neither is ever dropped, so
                // this is unreachable in practice; exiting is the honest answer if it happens.
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        while let Ok(inbound) = rx.try_recv() {
            enqueue(&mut queue, &mut native_waits, &mut lanes, &shared, inbound);
        }
        let Some(job) = next_job(&mut queue) else {
            continue;
        };
        match job {
            Job::Capture(request, spans, settle_pass) => {
                for &span in spans.spans() {
                    capture_from_core(&request, span, tiles);
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
            Job::Held(query) => {
                let answer = held_answer(tiles, &query);
                // A dead receiver is the asker gone — a closed table — and costs nothing more.
                let _ = query.reply.send(answer);
            }
            Job::Native(native) => {
                let (request, mut wait) = *native;
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
        }
    }
}

/// Start the lane of one host and intent, or say why it could not.
pub(super) fn spawn_lane(key: LaneKey, shared: Arc<Shared>) -> Option<Lane> {
    let (tx, rx) = mpsc::channel::<LaneJob>();
    let (host, intent) = key;
    let spawned = std::thread::Builder::new()
        .name(format!(
            "trade-replay:{}:{}",
            if host.is_empty() { "-" } else { host },
            match intent {
                ReplayIntent::Chart => "chart",
                ReplayIntent::Model => "model",
            }
        ))
        .spawn(move || run_lane(&rx, &shared));
    match spawned {
        Ok(_) => Some(Lane { tx }),
        Err(error) => {
            log::warn!("[x] trade-replay lane {key:?} did not start: {error}");
            None
        }
    }
}

/// One host's lane: the venue calls of every request routed to it, one at a time, candles
/// before tick stages ([`next_lane_job`]). A tick stage is a separate job rather than an inline
/// continuation of its candle job so the first outcome reaches its window before any paging
/// starts, and so a second window's bars go ahead of the first window's pages. Every
/// already-queued job is drained non-blockingly before the pick, so a burst of report-row
/// clicks is batched before priority is applied rather than served one at a time.
///
/// Args:
///     rx: The lane's queue.
///     shared: The gate, the stores and the way back to the coordinator.
pub(super) fn run_lane(rx: &Receiver<LaneJob>, shared: &Shared) {
    let Shared {
        agent,
        gate,
        cache,
        tiles,
        back,
    } = shared;
    let mut queue: VecDeque<LaneJob> = VecDeque::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(job) => queue.push_back(job),
                // The coordinator holds the sender for as long as it lives, and it never exits.
                Err(_) => return,
            }
        }
        while let Ok(job) = rx.try_recv() {
            queue.push_back(job);
        }
        let Some(job) = next_lane_job(&mut queue) else {
            continue;
        };
        // A native follow-up armed here is the coordinator's to poll: the lane must be free
        // for the next call, and the core's archive is read off no venue.
        let hand_back = |request: TradeReplayRequest, wait: NativeWait| {
            let _ = back.send(Inbound::NativeWait(Box::new((request, wait))));
        };
        match job {
            LaneJob::Candles(request) => {
                // A window that closed while its request sat in the queue costs nothing at all:
                // this is the cheapest of the three cancellation guards and the only one that
                // prevents the work.
                if request.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                let mut served = serve_with_core(agent, gate, cache, &request);
                // No native wait either with the stage off: the wait is the core-archive half
                // of the same stage, and it would poll the archive for a window that asked for
                // the bars alone.
                let native_wait = match request.ticks {
                    true => arm_native_wait(&request, &mut served, Instant::now()),
                    false => None,
                };
                // The receiver is gone whenever the window closed mid-fetch. Normal, not an
                // error — and exactly the signal that a queued tick stage would now answer no
                // one, so it is never queued on a failed send.
                let sent = request.reply.send(served.outcome).is_ok();
                if sent {
                    if let Some(stage) = served.tick_stage {
                        queue.push_back(LaneJob::Ticks(request, Box::new(stage)));
                    } else if let Some(wait) = native_wait {
                        hand_back(request, wait);
                    }
                }
            }
            LaneJob::Ticks(request, stage) => {
                if request.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                let stage = *stage;
                match serve_ticks(agent, gate, &request, &stage, tiles) {
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
                            cache,
                            request.intent,
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
                        let wait = arm_native_wait(&request, &mut served, Instant::now());
                        if request.reply.send(served.outcome).is_ok() {
                            if let Some(wait) = wait {
                                hand_back(request, wait);
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
                                cache,
                                request.intent,
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
                        let wait = arm_native_wait(&request, &mut served, Instant::now());
                        if request.reply.send(served.outcome).is_ok() {
                            if let Some(wait) = wait {
                                hand_back(request, wait);
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
