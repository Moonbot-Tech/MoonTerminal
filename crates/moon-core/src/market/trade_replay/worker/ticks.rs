//! Tick serving and progressive replay upgrades.

use super::*;

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
///     to remember settled (see [`run_lane`]'s `Ticks` arm): it is set when the venue itself
///     refused mid-walk ([`TickHarvest::venue_refused`]), and when the walk was abandoned but the
///     store still had part of the focus to serve — the missing part is still owed.
///     `Err(Some(status))` when the stage ended for a reason the window must print instead — the
///     caller composes that second outcome from [`TickStage::candles`] carrying `status`.
///     `Err(None)` only for a cancelled window, where the requester is already gone and nothing
///     more is sent.
pub(super) fn serve_ticks(
    agent: &ureq::Agent,
    gate: &ReplayGate,
    request: &TradeReplayRequest,
    stage: &TickStage,
    tiles: &Mutex<TickTileStore>,
) -> Result<(TradeReplaySeries, bool), Option<TickStatus>> {
    // The archive may have arrived while other candle jobs had priority in the worker queue.
    // For a chart, answered straight from the ring and not filed: a thinned answer is not a
    // tile, and the chart shows the series it is sent. A tiles reader gets the ring THROUGH
    // the tiles instead (`file_ring` below): the capture at close time — the only other thing
    // that files the ring — never ran for a trade that closed while the terminal was down.
    let baseline_coverage = stage
        .baseline
        .as_ref()
        .map(|series| series.covered.clone())
        .unwrap_or_default();
    if !request.intent.files_core() {
        if let Some(mut series) = read_core(request)
            .filter(|series| preserves_coverage(&series.covered, &baseline_coverage))
        {
            series.candles = stage.candles.clone();
            return Ok((series, false));
        }
    }
    let key: TileKey = (request.address.exchange_key.clone(), request.market.clone());
    let focus = request.window.focus_spans();
    let persisted = super::trade_cache::handle();
    // A requester that reads the tiles gets the ring THROUGH them: what the ring holds inside
    // the focus is filed as `Core` tiles before the stage decides what is left to fetch, so a
    // trade the close-time capture missed costs the venue only what the ring does not hold.
    // Run after the disk hydrate on either branch below, and only for a focus the store does
    // not already hold whole: the ring copy scans the donor's whole retained ring, and a
    // retry of a row the disk answered would pay it for nothing.
    let file_ring = || {
        if !request.intent.files_core() {
            return;
        }
        let held_whole = {
            let store = lock_tiles(tiles);
            held_coverage(&store, &key, &focus, Coverage::none()).covers(&focus)
        };
        if held_whole {
            return;
        }
        // One deadline for the whole focus: a long position's two ends share it.
        let archive_deadline = Instant::now() + crate::market::source::ARCHIVE_WAIT;
        file_core_into_tiles(&request.address, &request.market, &focus, tiles, |span| {
            // Wait for the core's archive of this market first: the copy is final for the
            // stage, and a ring copied before the answer sends the venue — or, with no route,
            // nobody — the stretch the archive holds.
            request.address.history.capture_core_span(
                &request.address,
                &request.market,
                span.0,
                span.1,
                Some(archive_deadline),
            )
        });
    };
    // No venue to ask — none has a route, or the focus is past the route's retention: the focus
    // is served from what the tiles hold inside it — a capture from the core's archive, the
    // tuner's fetch, an earlier window — or the window prints `none`, the reason there is no
    // venue to ask.
    let serve_held = |none: TickStatus| {
        hydrate(tiles, persisted.as_ref(), &key, &focus);
        file_ring();
        let (covered, runs) = {
            let store = lock_tiles(tiles);
            let covered = held_coverage(&store, &key, &focus, Coverage::none());
            if covered.is_empty() {
                return Err(Some(none));
            }
            let runs = read_coverage(&store, &key, &covered);
            (covered, runs)
        };
        let (ticks, side_slots) = flatten_runs(runs, request.tick_value);
        if ticks.is_empty() {
            return Err(Some(none));
        }
        let partial = !covered.contains((request.window.from_ms, request.window.to_ms));
        let (ticks, thinning) = fit_ticks_around(ticks, TICK_BUDGET, &request.window.raw_spans());
        Ok((
            compose_ticks(
                request,
                request.address.venue,
                ticks,
                thinning,
                side_slots,
                partial,
                covered,
                stage.candles.clone(),
            ),
            // Not settled: a later capture (the settle pass, a neighbouring trade) may widen
            // what the tiles hold, and a reopen should see it.
            true,
        ))
    };
    let Some(route) = stage.route else {
        return serve_held(TickStatus::NoRoute);
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
    let plan = tick_plan(request.window, route, earliest_ms, request.intent);
    if plan.slices.is_empty() {
        // The FOCUS itself — the trade, not its optional context — lies entirely before
        // `earliest_ms`: nothing is left for the venue, and the tiles answer alone. With nothing
        // held either, this is reported as retention rather than as an empty venue answer.
        return serve_held(TickStatus::OutOfRetention {
            retention_ms: route.retention_ms().unwrap_or(0),
        });
    }
    hydrate(tiles, persisted.as_ref(), &key, &focus);
    file_ring();
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
    // A weight stop or a 429 cut the walk. A non-empty harvest is still served — the prints
    // were paid for — but it is not a settled tape: the tuner defers on RateLimited.
    let mut host_limited = false;
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
            let (points, thinning) =
                fit_ticks_around(points, TICK_BUDGET, &request.window.raw_spans());
            let mut series = compose_ticks(
                request,
                request.address.venue,
                points,
                thinning,
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
            page_interval: route.page_interval(),
            page_weight: route.request_weight(),
            progress: &mut publish_progress,
        };
        let upgrade = CoreUpgradeProbe::new(Instant::now());
        // When the walk's first page goes out: the instant a clear on its answer is judged
        // against — a refusal recorded after it is not this walk's to lift.
        let asked_at = Instant::now();
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
                    // A tiles reader never takes the ring in place of the walk: what the ring
                    // holds was filed before the walk, and an archive landing mid-walk is the
                    // next request's to file — replacing the walk would throw its pages away
                    // and file nothing.
                    || match request.intent.files_core() {
                        true => None,
                        false => read_core(request),
                    },
                )
            },
            |trade| Instant::now() >= if trade { trade_deadline } else { deadline },
            &mut observer,
            |from_ms, to_ms, cursor| match rest::fetch_trades(
                agent,
                route,
                &request.market,
                from_ms,
                to_ms,
                cursor,
            ) {
                Ok(page) => {
                    // A weight stop leaves this page in the harvest. The next iteration's
                    // claim is what refuses to send another one.
                    let _ = absorb_notice(gate, route.host());
                    Ok(page)
                }
                Err(rest::FetchError::Throttled { diagnostic, .. }) => {
                    let _ = absorb_notice(gate, route.host());
                    Err(rest::FetchError::Throttled {
                        retry_after_s: None,
                        diagnostic,
                    })
                }
                Err(rest::FetchError::Transient(diagnostic)) => {
                    match absorb_notice(gate, route.host()) {
                        // The header already named the wait. Returning Transient would have the
                        // walk record a curve refusal on top of it.
                        NoticeEffect::WeightStop | NoticeEffect::Throttled => {
                            Err(rest::FetchError::Throttled {
                                retry_after_s: None,
                                diagnostic,
                            })
                        }
                        NoticeEffect::None => Err(rest::FetchError::Transient(diagnostic)),
                    }
                }
                Err(rest::FetchError::UnknownSymbol) => {
                    let _ = absorb_notice(gate, route.host());
                    Err(rest::FetchError::UnknownSymbol)
                }
            },
        );
        if let Some(mut series) = upgrade.ready.into_inner() {
            // The paginator stopped on our replacement, not a host refusal: our own stop, which
            // says nothing about the venue and touches the gate no more than a cancel does.
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
                    stop,
                } = harvest;
                // The venue answered without refusing anywhere along the walk, so its refusal
                // history is stale — exactly the candle stage's own `gate.clear` above. A
                // refusal it gave us mid-walk (`Transient`) is recorded, or the next request to
                // this host sends blind into a burst it just declined; an `UnknownSymbol` is an
                // answer, not a refusal, and the host is fine.
                match stop {
                    Some(TickAbandon::Transient) => gate.refuse(route.host(), Instant::now()),
                    // The wait is already the header's: a curve refusal here would replace a
                    // Retry-After or a minute-boundary stop, and a clear would try to erase it.
                    // `clear` still runs for an unknown symbol, which is an answer.
                    Some(TickAbandon::RateLimited) => host_limited = true,
                    _ if !venue_refused || matches!(stop, Some(TickAbandon::UnknownSymbol)) => {
                        gate.clear(route.host(), asked_at)
                    }
                    _ => {}
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
                retry_on_reopen = venue_refused || host_limited;
                complete = walked_whole;
            }
            TickVerdict::Abandoned(reason) => {
                // The gate hears only the venue's own word. `Transient` is a refusal or failure
                // it just gave us: recorded, so the next request to this host waits it out.
                // `RateLimited` means the check refused us on a refusal already standing:
                // nothing to add. `Empty` and `UnknownSymbol` are answers — the host is fine,
                // and a refusal on record is stale. Our own stops — the user closed the window,
                // the deadline, either budget — say nothing about the venue and touch nothing:
                // another lane of this host may have just been refused for real.
                match reason {
                    TickAbandon::Transient => gate.refuse(route.host(), Instant::now()),
                    TickAbandon::Empty | TickAbandon::UnknownSymbol => {
                        gate.clear(route.host(), asked_at)
                    }
                    TickAbandon::RateLimited => host_limited = true,
                    TickAbandon::Cancelled
                    | TickAbandon::Deadline
                    | TickAbandon::OverPageBudget
                    | TickAbandon::OverTickBudget => {}
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
            return Err(Some(abandon_status(gate, route, abandoned)));
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
            let gained = !inside.is_empty();
            lock_tiles(tiles).insert(key.clone(), from_ms, to_ms, inside, TileSource::Venue);
            // After the tiles hold them, whatever became of the file write: held queries read
            // the tiles, so a reader that refused this market asks again (`filed_since`).
            if gained {
                super::super::trade_cache::note_filed(
                    &request.address.exchange_key,
                    &request.market,
                );
            }
        }
    }
    if ticks.is_empty() {
        // The covered run holds no print. With a walk abandoned this round that is not an
        // answer — the store's part is empty and the venue's part never arrived, so a retry is
        // honest. Otherwise it is authoritative: every stretch of the run was asked and answered
        // empty, and no retry can change it.
        return Err(Some(match abandoned {
            Some(_) => abandon_status(gate, route, abandoned),
            None => TickStatus::NoTrades,
        }));
    }
    // `partial` must reflect what `covered` actually spans, not merely whether the walk finished.
    // `tick_plan`'s own `earliest_ms` clip can make the PLAN narrower than `request.window` before
    // the walk even starts, so a retention-clipped plan that completes still leaves the served
    // ticks short of the requested window on one or both edges.
    let partial = !complete || !covered.contains((request.window.from_ms, request.window.to_ms));
    let (ticks, thinning) = fit_ticks_around(ticks, TICK_BUDGET, &request.window.raw_spans());
    let mut series = compose_ticks(
        request,
        request.address.venue,
        ticks,
        thinning,
        side_slots,
        partial,
        covered,
        stage.candles.clone(),
    );
    if host_limited {
        series.tick_status = TickStatus::RateLimited {
            retry_in_s: gate.refused_for(route.host(), Instant::now()).unwrap_or(1),
        };
        retry_on_reopen = true;
    }
    Ok((series, retry_on_reopen))
}
