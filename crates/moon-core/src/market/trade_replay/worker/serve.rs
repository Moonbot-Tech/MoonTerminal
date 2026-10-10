//! Candle serving, rate limits and native fallback.

use super::*;

/// What one candle job resolves to: the outcome to send, and whether it earned a tick upgrade.
pub(super) struct Served {
    pub(super) outcome: TradeReplayOutcome,
    /// `Some` only when the CANDLE outcome above was `Ready` and the window was not cancelled,
    /// so `run` may queue it onto the BACK of the deque; see [`tick_stage_for`].
    pub(super) tick_stage: Option<TickStage>,
}

/// What the response this thread just decoded asked the gate to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NoticeEffect {
    /// No rate-limit header the gate acts on.
    None,
    /// HTTP 429 or 418. `Retry-After` when it parsed, otherwise the backoff curve.
    Throttled,
    /// `X-MBX-USED-WEIGHT-1M` over 75% of the host's limit, until the next UTC minute.
    WeightStop,
}

/// Apply the calling thread's [`rest::exchange_notice`] to `host`.
///
/// A 429 or 418 honours `Retry-After` when the header is an integer number of seconds, and
/// the 30–600 s curve when it is missing or not. Every response, including those, then
/// applies `X-MBX-USED-WEIGHT-1M`: over 75% the host stays stopped until the next UTC minute
/// when that is the longer wait, and a longer ban or curve is not shortened. The notice is
/// empty when the call never produced a response, and then this records nothing.
///
/// Args:
///     gate: The process-wide replay gate.
///     host: Stable host key, from the route.
///
/// Returns:
///     Which wait, if any, was recorded.
pub(super) fn absorb_notice(gate: &ReplayGate, host: &'static str) -> NoticeEffect {
    let notice = rest::exchange_notice();
    let now = Instant::now();
    let throttled = notice.status == 429 || notice.status == 418;
    if throttled {
        match notice.retry_after_s {
            Some(seconds) => gate.honour_retry_after(host, now, seconds),
            None => gate.refuse(host, now),
        }
    }
    if let Some(used) = notice.used_weight_1m {
        let unix_ms = crate::util::time::now_unix_ms_i64();
        if unix_ms > 0 && gate.note_used_weight(host, used, now, (unix_ms as u64) / 1_000) {
            return match throttled {
                true => NoticeEffect::Throttled,
                false => NoticeEffect::WeightStop,
            };
        }
    }
    match throttled {
        true => NoticeEffect::Throttled,
        false => NoticeEffect::None,
    }
}

/// A candle job the gate will not send, carrying the wait [`ReplayGate::refused_for`] names.
///
/// Args:
///     gate: The process-wide replay gate.
///     host: Stable host key, from the route.
///
/// Returns:
///     A failed candle outcome with no tick stage.
pub(super) fn rate_limited(gate: &ReplayGate, host: &'static str) -> Served {
    Served {
        outcome: TradeReplayOutcome::Failed(TradeReplayFailure::RateLimited {
            retry_in_s: gate.refused_for(host, Instant::now()).unwrap_or(1),
        }),
        tick_stage: None,
    }
}

/// The status an abandoned walk with nothing to serve is answered with: the gate's own refusal
/// carries how long the host stays refused, read off the gate without taking a permit; every
/// other reason is a plain failure a reopen retries.
pub(super) fn abandon_status(
    gate: &ReplayGate,
    route: TradeRoute,
    abandoned: Option<TickAbandon>,
) -> TickStatus {
    match abandoned {
        Some(TickAbandon::RateLimited) => TickStatus::RateLimited {
            // `None` only when the wait ran out between the refusal and this read: one second
            // is then the honest number, not a claim the walk never made.
            retry_in_s: gate.refused_for(route.host(), Instant::now()).unwrap_or(1),
        },
        _ => TickStatus::Failed,
    }
}

/// The native follow-up for one request, or none when its intent does not wait for the core:
/// a model's request is then answered with the stage's own terminal status, untouched, and the
/// sender drops with it.
pub(super) fn arm_native_wait(
    request: &TradeReplayRequest,
    served: &mut Served,
    now: Instant,
) -> Option<NativeWait> {
    if !request.intent.awaits_core() {
        return None;
    }
    prepare_native_wait(served, now)
}

/// Arm native observation whenever no public tick job can complete this candle/failed answer.
pub(super) fn prepare_native_wait(served: &mut Served, now: Instant) -> Option<NativeWait> {
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
pub(super) fn attach_context(
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
pub(super) fn serve_with_core(
    agent: &ureq::Agent,
    gate: &ReplayGate,
    cache: &Mutex<VecDeque<(OutcomeKey, Remembered)>>,
    request: &TradeReplayRequest,
) -> Served {
    if !request.ticks {
        return bars_only(serve(agent, gate, cache, request));
    }
    if request.intent.files_core() {
        // The requester reads the tiles: the ring is filed into them by the tick stage
        // (`file_core_into_tiles`), never handed back in place of it.
        return serve(agent, gate, cache, request);
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
pub(super) fn bars_only(mut served: Served) -> Served {
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
pub(super) fn core_first(
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
pub(super) fn read_core(request: &TradeReplayRequest) -> Option<TradeReplaySeries> {
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
    let (ticks, thinning) =
        fit_ticks_around(native.ticks, TICK_BUDGET, &request.window.raw_spans());
    if ticks.is_empty() {
        return None;
    }
    let mut series = compose_ticks(
        request,
        request.address.venue,
        ticks,
        thinning,
        side_slots,
        native.covered != (request.window.from_ms, request.window.to_ms),
        Coverage::one(native.covered),
        Vec::new(),
    );
    series.source = TradeReplaySource::CoreTicks;
    Some(series)
}

/// Cooperatively replace a REST walk when a requested core archive arrives between pages.
pub(super) struct CoreUpgradeProbe {
    pub(super) next: Cell<Instant>,
    pub(super) ready: RefCell<Option<TradeReplaySeries>>,
}

impl CoreUpgradeProbe {
    /// Space expensive retained-ring scans while an exchange page is in flight.
    pub(super) fn new(now: Instant) -> Self {
        Self {
            next: Cell::new(now + Duration::from_millis(500)),
            ready: RefCell::new(None),
        }
    }

    /// Stop the walk on cancellation or replacement; never sleep or delay a network page.
    pub(super) fn stop(
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
pub(super) fn preserves_coverage(candidate: &Coverage, published: &Coverage) -> bool {
    !candidate.is_empty() && candidate.covers(published)
}

/// A retry can update the cache only if it retains all previously available tick coverage.
pub(super) fn retain_baseline(
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
pub(super) fn serve(
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
        long_position_ms: request.window.long_position_ms,
    };
    // A model's request takes no remembered answer, the authoritative empty included: one
    // candle page per row on a delisted market is the price of never inheriting a chart's
    // budget-stopped walk — see `ReplayIntent::reuses_answers`.
    let remembered = match request.intent.reuses_answers() {
        true => remember_lookup(cache, &key, request.identity),
        false => None,
    };
    match remembered {
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
            let tick_stage = Some(stage_and_stamp(venue, &key, &mut series));
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
        let tick_stage = stage_and_stamp(venue, &key, &mut series);
        // Never settled here: a stage is always queued (`tick_stage_for`), its `Pending` must be
        // re-decided — or answered — on the next open, and the stage's own answer is what is
        // remembered settled or not.
        remember_store(
            cache,
            request.intent,
            key.clone(),
            Remembered::Ready {
                series: series.clone(),
                ticks_settled: false,
            },
        );
        return Served {
            outcome: TradeReplayOutcome::Ready(series),
            tick_stage: Some(tick_stage),
        };
    }

    let asked_at = Instant::now();
    if let Err(retry_in_s) = gate.claim(route.host(), asked_at) {
        return Served {
            outcome: TradeReplayOutcome::Failed(TradeReplayFailure::RateLimited { retry_in_s }),
            tick_stage: None,
        };
    }
    let category = bybit_category(venue, &request.market);
    let deadline = asked_at + JOB_DEADLINE;
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
    let page_spans = pages(request.window, BAR_MS, route.max_rows());
    for (index, (from_ms, to_ms)) in page_spans.iter().copied().enumerate() {
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
        // The opening claim is before the loop. A later page looks again: the previous
        // response, or the other lane of this host, may have stopped it since.
        if index > 0 && gate.claim(route.host(), Instant::now()).is_err() {
            return rate_limited(gate, route.host());
        }
        gate.pace_weighted(
            route.host(),
            super::gate::MIN_INTERVAL,
            route.request_weight(),
        );
        match rest::fetch_klines(
            agent,
            route,
            &request.market,
            category,
            from_ms,
            to_ms,
            route.max_rows(),
        ) {
            Ok(page) => {
                rows.extend(page);
                // Record the header. The next iteration's claim is what refuses another page,
                // including when a stop was already in force and this call did not install a
                // new one. The last page is kept: it was already paid for.
                let _ = absorb_notice(gate, route.host());
            }
            Err(rest::FetchError::UnknownSymbol) => {
                // The venue ANSWERED; it simply does not list this symbol, and a refusal on
                // record for the host is stale. A weight header on that answer can still stop
                // the host for every other market that shares it.
                gate.clear(route.host(), asked_at);
                let _ = absorb_notice(gate, route.host());
                return Served {
                    outcome: TradeReplayOutcome::Failed(TradeReplayFailure::UnknownSymbol),
                    tick_stage: None,
                };
            }
            Err(rest::FetchError::Throttled { diagnostic, .. }) => {
                let _ = absorb_notice(gate, route.host());
                log::warn!("[x] trade-replay {diagnostic} on {}", route.host());
                return rate_limited(gate, route.host());
            }
            Err(rest::FetchError::Transient(diagnostic)) => {
                // A 5xx that already reports the IP over 75% waits out the minute. Anything
                // else is the curve: the venue's own failure is what starts the backoff.
                match absorb_notice(gate, route.host()) {
                    NoticeEffect::WeightStop | NoticeEffect::Throttled => {
                        return rate_limited(gate, route.host());
                    }
                    NoticeEffect::None => {
                        gate.refuse(route.host(), Instant::now());
                        return Served {
                            outcome: TradeReplayOutcome::Failed(TradeReplayFailure::Transient {
                                diagnostic,
                            }),
                            tick_stage: None,
                        };
                    }
                }
            }
        }
    }
    // The venue answered, so its refusal history is stale whatever the rows say.
    gate.clear(route.host(), asked_at);
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
            remember_store(cache, request.intent, key, Remembered::Empty);
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
        Some(stage_and_stamp(venue, &key, &mut series))
    };
    if complete {
        // Settled exactly when no stage was queued — a cancelled window, whose `Failed` is
        // final; a queued stage's `Pending` must be re-decided, see the SQLite-hit branch above.
        remember_store(
            cache,
            request.intent,
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

/// The tick stage for a just-built candle series ([`tick_stage_for`]), with the series stamped
/// to match: a tick series already in hand becomes the stage's baseline and reads `Streaming`;
/// a candle series keeps the `Pending` [`compose`] gave it.
///
/// One helper for the three sites in [`serve`] that each produce a fresh candle answer: the
/// ring-hit-but-not-settled branch, the SQLite-cache-hit branch, and the completed-network-fetch
/// branch — the last of these calls it only on its NON-CANCELLED path; the cancelled sub-branch
/// skips it entirely and stamps [`TickStatus::Failed`] directly, since a window that is already
/// gone has no stage to queue.
///
/// Args:
///     venue: Venue the candles came from.
///     key: The ring key this stage would replace on success.
///     series: The just-built series; its `tick_status` reads `Streaming` when it already carries
///         ticks.
///
/// Returns:
///     The stage to queue.
pub(super) fn stage_and_stamp(
    venue: crate::venue::Venue,
    key: &OutcomeKey,
    series: &mut TradeReplaySeries,
) -> TickStage {
    let mut stage = tick_stage_for(venue, key, &series.candles);
    if series.source.is_ticks() {
        stage.baseline = Some(series.clone());
        series.tick_status = TickStatus::Streaming;
    }
    stage
}

/// The tick upgrade a just-built CANDLE series earns — always one: nothing here refuses it.
///
/// The "already settled" short-circuit this used to take as a parameter no longer lives here: it
/// is checked once, in [`serve`]'s ring-hit branch, before this is ever called — a settled entry
/// is sent exactly as stored, with no stage queued and nothing here re-decided.
///
/// Neither a venue with no public route nor a window older than the route's retention is a
/// refusal: the stage is what reads the tile store and its disk — what an earlier window, the
/// tuner's fetch or the close-time capture filed — and it runs against them alone, printing
/// `NoRoute` / `OutOfRetention` itself when they hold nothing for the focus ([`serve_ticks`]).
/// The retention refusal used to be decided here, before the stage was queued, and every trade
/// older than it (48 h on Binance futures) showed candles alone however much of its tape the
/// terminal held (2026-09-23).
///
/// Args:
///     venue: Venue the candles came from.
///     key: The ring key this stage would replace on success.
///     candles: The exchange klines just composed, carried forward as the eventual tick series'
///         bar layer — see [`TickStage::candles`].
///
/// Returns:
///     The stage to queue.
pub(super) fn tick_stage_for(
    venue: crate::venue::Venue,
    key: &OutcomeKey,
    candles: &[ChartCandle],
) -> TickStage {
    TickStage {
        baseline: None,
        route: trade_route(venue),
        key: key.clone(),
        candles: candles.to_vec(),
    }
}

/// Whether a window is within a trade route's own documented retention.
///
/// Judges the FOCUS's own right edge — the trade's EXIT ([`ReplayWindow::focus`]) — never the
/// window's padded `from_ms` (D2-3), and never the focus's left edge either: the lead context is
/// optional padding, but the trade itself is not, and [`tick_plan`]'s own retention clip already
/// asks only that the exit be inside retention, clipping everything older. Judging the entry
/// instead was a STRICTLY STRONGER check, and made `tick_plan`'s whole retention-clipping
/// recovery path unreachable for exactly the windows it was written to rescue: a Binance futures
/// trade held ~10 h and closed 40 h ago (retention 48 h) was refused outright although its
/// exit's ticks were comfortably inside retention.
///
/// Free. Not a gate of the tick stage — a window past the retention is still served from the
/// tiles ([`tick_stage_for`]) — but of the tuner's fetch: its startup autoload asks it before
/// queueing a row at all, and its load before calling a row it holds no tape for fetchable, so a
/// row the venue would refuse anyway pays no candle page ahead of the refusal.
///
/// Args:
///     route: The trade route in question.
///     window: The window to check.
///     now_ms: Current Unix time in milliseconds.
///
/// Returns:
///     `true` when the route documents no retention limit, or when the focus's own right edge
///     falls inside the one it does document.
pub fn inside_retention(route: TradeRoute, window: ReplayWindow, now_ms: i64) -> bool {
    route
        .retention_ms()
        .is_none_or(|r| window.focus().1 >= now_ms - r)
}
