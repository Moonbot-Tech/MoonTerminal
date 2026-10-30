use super::*;

/// Prepare cached rows together; unresolved rates still follow the canonical resolver.
///
/// Returns the ready identities and whether any monetary value changed. The caller groups
/// distinct identities between event barriers so writing ahead cannot reorder a delete/reset.
/// Cleanup attempts release even if rollback fails, returning the rollback error first.
pub(in crate::db::valuation::worker) fn prepare_ready_trades(
    store: &Connection,
    axis: &ReportAxis,
    inputs: &[TradeInput],
) -> Result<(BTreeSet<(i64, i64, i64)>, bool), FaultCause> {
    let current = current_minute_utc();
    let keys = inputs
        .iter()
        .filter(|input| {
            valuation_minute(axis, input) < current
                && crate::db::QuoteCurrency::from_report_ordinal(input.quote_ordinal).is_some()
        })
        .map(|input| (input.quote_ordinal, valuation_minute(axis, input)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let rates = super::super::cached_rates(store, &keys).map_err(super::super::store_fault)?;
    let ready = inputs
        .iter()
        .filter_map(|input| {
            let minute = valuation_minute(axis, input);
            if minute >= current
                || crate::db::QuoteCurrency::from_report_ordinal(input.quote_ordinal).is_none()
            {
                return None;
            }
            rates
                .get(&(input.quote_ordinal, minute))
                .map(|rate| (input, rate))
        })
        .collect::<Vec<_>>();
    if ready.is_empty() {
        return Ok((BTreeSet::new(), false));
    }
    store
        .execute_batch("SAVEPOINT ready_values")
        .map_err(super::super::store_fault)?;
    let result = (|| {
        let changed = super::super::store_trade_values(store, &ready, now_unix_ms_i64())
            .map_err(super::super::store_fault)?
            > 0;
        let keys = ready.iter().map(|(input, _)| trade_key(input)).collect();
        forget_deferred(store, &keys)?;
        store
            .execute_batch("RELEASE ready_values")
            .map_err(super::super::store_fault)?;
        Ok((keys, changed))
    })();
    if result.is_err() {
        let rollback = store.execute_batch("ROLLBACK TO ready_values");
        // A failed rollback must not skip releasing the connection's savepoint.
        let release = store.execute_batch("RELEASE ready_values");
        rollback.map_err(super::super::store_fault)?;
        release.map_err(super::super::store_fault)?;
    }
    result
}

/// Prepare one current committed report row from cache or the canonical spot provider.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     axis: Per-core time axis rate minutes are resolved against.
///     input: Complete current report inputs.
///     canonical_exact_prefetched: Whether canonical direct/inverse exact routes were absent.
///
/// Returns:
///     Completed, current-minute deferred, or transient-retry result.
pub(in crate::db::valuation::worker) fn prepare_trade(
    store: &Connection,
    source: &dyn SpotRateSource,
    axis: &ReportAxis,
    input: &TradeInput,
    canonical_exact_prefetched: bool,
) -> PrepareResult {
    let minute_utc = valuation_minute(axis, input);
    if minute_utc >= current_minute_utc() {
        return PrepareResult::Deferred { changed: false };
    }
    let Some(currency) = crate::db::QuoteCurrency::from_report_ordinal(input.quote_ordinal) else {
        return delete_trade(store, input.source, input.core_uid, input.row_id);
    };
    let rate = match crate::db::valuation::cached_rate(store, input.quote_ordinal, minute_utc) {
        Ok(Some(rate)) => rate,
        Ok(None) => {
            let now_ms = now_unix_ms_i64();
            match crate::db::valuation::covering_successor_rate(
                store,
                input.quote_ordinal,
                minute_utc,
            ) {
                Ok(Some(rate)) => {
                    if let Err(error) = crate::db::valuation::store_rate(store, &rate, now_ms) {
                        return PrepareResult::Retry(crate::db::valuation::store_fault(error));
                    }
                    rate
                }
                Ok(None) => {
                    let search_start = match crate::db::valuation::rate_search_start(
                        store,
                        input.quote_ordinal,
                        minute_utc,
                        now_ms,
                    ) {
                        Ok(None) => {
                            return match delete_trade(
                                store,
                                input.source,
                                input.core_uid,
                                input.row_id,
                            ) {
                                PrepareResult::Complete { changed } => {
                                    PrepareResult::Deferred { changed }
                                }
                                result => result,
                            };
                        }
                        Ok(Some(search_start)) => search_start,
                        Err(error) => {
                            return PrepareResult::Retry(crate::db::valuation::store_fault(error));
                        }
                    };
                    let latest_closed = current_minute_utc().saturating_sub(60);
                    match resolve_historical_rate(
                        source,
                        currency,
                        minute_utc,
                        search_start,
                        latest_closed,
                        canonical_exact_prefetched,
                    ) {
                        Ok(rate) => {
                            if rate.candle_close_ms >= now_ms {
                                return PrepareResult::Deferred { changed: false };
                            }
                            if let Err(error) =
                                crate::db::valuation::store_rate(store, &rate, now_ms)
                            {
                                return PrepareResult::Retry(crate::db::valuation::store_fault(
                                    error,
                                ));
                            }
                            rate
                        }
                        Err(FetchFailure::Missing) => {
                            if let Err(error) = crate::db::valuation::store_rate_search(
                                store,
                                input.quote_ordinal,
                                minute_utc,
                                latest_closed,
                                now_ms,
                                true,
                            ) {
                                return PrepareResult::Retry(crate::db::valuation::store_fault(
                                    error,
                                ));
                            }
                            let scheduled =
                                delete_trade(store, input.source, input.core_uid, input.row_id);
                            return match scheduled {
                                PrepareResult::Complete { changed } => {
                                    PrepareResult::Deferred { changed }
                                }
                                result => result,
                            };
                        }
                        Err(FetchFailure::Transient(error) | FetchFailure::Unavailable(error)) => {
                            return PrepareResult::Retry(FaultCause::new(
                                FailureKind::Provider,
                                error,
                            ));
                        }
                    }
                }
                Err(error) => {
                    return PrepareResult::Retry(crate::db::valuation::store_fault(error));
                }
            }
        }
        Err(error) => return PrepareResult::Retry(crate::db::valuation::store_fault(error)),
    };
    match crate::db::valuation::store_trade_value(store, input, &rate, now_unix_ms_i64()) {
        Ok(changed) => PrepareResult::Complete {
            changed: changed > 0,
        },
        Err(error) => PrepareResult::Retry(crate::db::valuation::store_fault(error)),
    }
}

/// Prepare one trade from a completed resolution without crossing the provider boundary.
///
/// Args:
///     store: Transactional valuation writer connection.
///     axis: Per-core time axis rate minutes are resolved against.
///     input: Current report input.
///     prefetched: Complete exact/successor resolution persisted by this batch.
///
/// Returns:
///     Completed, deferred, or transient-retry result using only cache and recorded outcomes.
pub(in crate::db::valuation::worker) fn prepare_prefetched_trade(
    store: &Connection,
    axis: &ReportAxis,
    input: &TradeInput,
    prefetched: &PrefetchOutcome,
) -> PrepareResult {
    let minute = valuation_minute(axis, input);
    if minute >= current_minute_utc() {
        return PrepareResult::Deferred { changed: false };
    }
    if crate::db::QuoteCurrency::from_report_ordinal(input.quote_ordinal).is_none() {
        return delete_trade(store, input.source, input.core_uid, input.row_id);
    }
    match crate::db::valuation::cached_rate(store, input.quote_ordinal, minute) {
        Ok(Some(rate)) => {
            match crate::db::valuation::store_trade_value(store, input, &rate, now_unix_ms_i64()) {
                Ok(changed) => PrepareResult::Complete {
                    changed: changed > 0,
                },
                Err(error) => PrepareResult::Retry(crate::db::valuation::store_fault(error)),
            }
        }
        Ok(None) => match prefetched.unresolved.get(&(input.quote_ordinal, minute)) {
            Some(UnresolvedRate::Missing) => {
                match delete_trade(store, input.source, input.core_uid, input.row_id) {
                    PrepareResult::Complete { changed } => PrepareResult::Deferred { changed },
                    result => result,
                }
            }
            Some(UnresolvedRate::Provider(error)) => PrepareResult::Retry(error.clone()),
            Some(UnresolvedRate::Deferred) | None => PrepareResult::Deferred { changed: false },
        },
        Err(error) => PrepareResult::Retry(crate::db::valuation::store_fault(error)),
    }
}

/// Persist outage pacing without claiming any minute was searched, then retire stale row values.
pub(in crate::db::valuation::worker) fn defer_provider_trade(
    store: &Connection,
    axis: &ReportAxis,
    input: &TradeInput,
) -> Result<bool, FaultCause> {
    let minute = valuation_minute(axis, input);
    crate::db::valuation::store_rate_search(
        store,
        input.quote_ordinal,
        minute,
        minute.saturating_sub(60),
        now_unix_ms_i64(),
        false,
    )
    .map_err(crate::db::valuation::store_fault)?;
    match delete_trade(store, input.source, input.core_uid, input.row_id) {
        PrepareResult::Complete { changed } => Ok(changed),
        PrepareResult::Retry(error) => Err(error),
        PrepareResult::Deferred { .. } => {
            unreachable!("deleting a cached value does not await a rate")
        }
    }
}

/// Return requested minutes lacking a ready provider rate, indexed for constant-time membership.
///
/// Args:
///     minutes: Requested closed UTC minute starts.
///     ready: Rates returned before a transient provider failure.
///
/// Returns:
///     Requested minutes that still need durable outage pacing, in request order.
pub(in crate::db::valuation::worker) fn minutes_without_ready(
    minutes: &[i64],
    ready: &[super::super::ResolvedRate],
) -> Vec<i64> {
    let ready_minutes = ready
        .iter()
        .map(|rate| rate.minute_utc)
        .collect::<std::collections::HashSet<_>>();
    minutes
        .iter()
        .copied()
        .filter(|minute| !ready_minutes.contains(minute))
        .collect()
}

/// Populate every uncached closed rate needed by one report-row batch.
///
/// Requests are grouped by quote and bounded to provider windows of at most 1,000 minutes. Sparse
/// trade history therefore costs one request per time window rather than one request per order,
/// while the persistent rate table makes later reconciliation and restarts network-free.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     axis: Per-core time axis rate minutes are resolved against.
///     inputs: Current report inputs about to be prepared.
///
/// Returns:
///     Durable change state plus canonical exact misses, or a transient reason.
pub(in crate::db::valuation::worker) fn prefetch_rates(
    store: &Connection,
    source: &dyn SpotRateSource,
    axis: &ReportAxis,
    inputs: &[TradeInput],
) -> Result<PrefetchOutcome, PrefetchError> {
    let mut outcome = resolve_rates(store, source, axis, inputs)?;
    persist_prefetched(store, &mut outcome)?;
    Ok(outcome)
}

/// Resolve every uncached exact, successor, and two-leg rate without writing to the store.
///
/// Args:
///     store: Valuation cache read connection, outside any write transaction.
///     source: Historical closed-candle boundary.
///     axis: Per-core time axis rate minutes are resolved against.
///     inputs: Current report inputs about to be prepared.
///
/// Returns:
///     Complete rates, unresolved outcomes, and retry schedules to persist as one batch.
///
/// Errors:
///     Cache read failures or an invalid unclosed exact-provider response.
pub(in crate::db::valuation::worker) fn resolve_rates(
    store: &Connection,
    source: &dyn SpotRateSource,
    axis: &ReportAxis,
    inputs: &[TradeInput],
) -> Result<PrefetchOutcome, PrefetchError> {
    let current = current_minute_utc();
    let latest_closed = current.saturating_sub(60);
    let mut groups: BTreeMap<(i64, &'static str), BTreeMap<i64, i64>> = BTreeMap::new();
    let mut outcome = PrefetchOutcome::default();
    let mut seen = BTreeSet::new();
    let keys = inputs
        .iter()
        .filter(|input| {
            valuation_minute(axis, input) < current
                && crate::db::QuoteCurrency::from_report_ordinal(input.quote_ordinal).is_some()
        })
        .map(|input| (input.quote_ordinal, valuation_minute(axis, input)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let cached =
        crate::db::valuation::cached_rates(store, &keys).map_err(|error| PrefetchError {
            fault: crate::db::valuation::store_fault(error),
            changed: false,
        })?;
    for input in inputs {
        let minute = valuation_minute(axis, input);
        if !seen.insert((input.quote_ordinal, minute)) || minute >= current {
            continue;
        }
        let Some(currency) = crate::db::QuoteCurrency::from_report_ordinal(input.quote_ordinal)
        else {
            continue;
        };
        if cached.contains_key(&(input.quote_ordinal, minute)) {
            continue;
        }
        match crate::db::valuation::covering_successor_rate(store, input.quote_ordinal, minute) {
            Ok(Some(rate)) => {
                outcome.rates.push(rate);
                continue;
            }
            Ok(None) => {}
            Err(error) => {
                return Err(PrefetchError {
                    fault: crate::db::valuation::store_fault(error),
                    changed: false,
                });
            }
        }
        match crate::db::valuation::rate_search_start(
            store,
            input.quote_ordinal,
            minute,
            now_unix_ms_i64(),
        ) {
            Ok(None) => {
                outcome
                    .unresolved
                    .insert((input.quote_ordinal, minute), UnresolvedRate::Missing);
            }
            Ok(Some(search_start)) => {
                groups
                    .entry((input.quote_ordinal, currency.ticker()))
                    .or_default()
                    .insert(minute, search_start);
            }
            Err(error) => {
                return Err(PrefetchError {
                    fault: crate::db::valuation::store_fault(error),
                    changed: false,
                });
            }
        }
    }
    for ((quote_ordinal, ticker), minutes) in groups {
        let exact = minutes
            .iter()
            .filter(|(minute, search_start)| **search_start <= **minute)
            .map(|(minute, _)| *minute)
            .collect::<Vec<_>>();
        let mut ready = BTreeSet::new();
        let mut start = 0;
        while start < exact.len() {
            let first = exact[start];
            let mut end = start + 1;
            while end < exact.len()
                && end - start < 1_000
                && exact[end].saturating_sub(first) <= 999 * 60
            {
                end += 1;
            }
            let batch = resolve_rate_batch(source, quote_ordinal, ticker, &exact[start..end]);
            if batch.transient.is_none() {
                debug_assert_eq!(batch.ready.len() + batch.missing.len(), end - start);
            }
            outcome
                .canonical_exact_missing
                .extend(batch.missing.iter().map(|minute| (quote_ordinal, *minute)));
            for rate in &batch.ready {
                if rate.candle_close_ms >= now_unix_ms_i64() {
                    return Err(PrefetchError {
                        fault: FaultCause::new(
                            FailureKind::Provider,
                            format!(
                                "{} {} returned an unclosed minute {}",
                                rate.provider, rate.symbol, rate.minute_utc
                            ),
                        ),
                        changed: false,
                    });
                }
                ready.insert(rate.minute_utc);
                outcome.rates.push(rate.clone());
            }
            if let Some(error) = batch.transient {
                for minute in minutes_without_ready(&exact[start..end], &batch.ready) {
                    outcome.searches.push(PendingRateSearch {
                        quote_ordinal,
                        minute_utc: minute,
                        searched_through_minute: minute.saturating_sub(60),
                        now_ms: now_unix_ms_i64(),
                        grow: false,
                    });
                }
                outcome
                    .provider_fault
                    .get_or_insert_with(|| FaultCause::new(FailureKind::Provider, error));
            }
            start = end;
        }
        for (minute, search_start) in minutes {
            if ready.contains(&minute) {
                continue;
            }
            let canonical_exact_prefetched = outcome
                .canonical_exact_missing
                .contains(&(quote_ordinal, minute));
            let resolved = resolve_historical_rate(
                source,
                crate::db::QuoteCurrency::from_report_ordinal(quote_ordinal)
                    .expect("grouped quote is known"),
                minute,
                search_start,
                latest_closed,
                canonical_exact_prefetched,
            );
            record_resolution(&mut outcome, quote_ordinal, minute, latest_closed, resolved);
        }
    }
    Ok(outcome)
}

/// Record one fully resolved provider outcome and the retry schedule it implies.
fn record_resolution(
    outcome: &mut PrefetchOutcome,
    quote_ordinal: i64,
    minute: i64,
    latest_closed: i64,
    resolved: Result<super::super::ResolvedRate, FetchFailure>,
) {
    let key = (quote_ordinal, minute);
    match resolved {
        Ok(rate) if rate.candle_close_ms >= now_unix_ms_i64() => {
            outcome.unresolved.insert(key, UnresolvedRate::Deferred);
        }
        Ok(rate) => outcome.rates.push(rate),
        Err(FetchFailure::Missing) => {
            outcome.searches.push(PendingRateSearch {
                quote_ordinal,
                minute_utc: minute,
                searched_through_minute: latest_closed,
                now_ms: now_unix_ms_i64(),
                grow: true,
            });
            outcome.unresolved.insert(key, UnresolvedRate::Missing);
        }
        Err(FetchFailure::Transient(error) | FetchFailure::Unavailable(error)) => {
            let fault = FaultCause::new(FailureKind::Provider, error);
            outcome.searches.push(PendingRateSearch {
                quote_ordinal,
                minute_utc: minute,
                searched_through_minute: minute.saturating_sub(60),
                now_ms: now_unix_ms_i64(),
                grow: false,
            });
            outcome
                .unresolved
                .insert(key, UnresolvedRate::Provider(fault.clone()));
            outcome.provider_fault.get_or_insert(fault);
        }
    }
}

/// Persist a completed resolution inside the caller's batch transaction.
///
/// Returns:
///     `Ok(())` after all rates and schedules are written, with `outcome.changed` updated.
///
/// Errors:
///     Any SQLite write failure, carrying changes made earlier in this transaction.
pub(in crate::db::valuation::worker) fn persist_prefetched(
    store: &Connection,
    outcome: &mut PrefetchOutcome,
) -> Result<(), PrefetchError> {
    for rate in &outcome.rates {
        match crate::db::valuation::store_rate(store, rate, now_unix_ms_i64()) {
            Ok(stored) => outcome.changed |= stored > 0,
            Err(error) => {
                return Err(PrefetchError {
                    fault: crate::db::valuation::store_fault(error),
                    changed: outcome.changed,
                });
            }
        }
    }
    for search in &outcome.searches {
        crate::db::valuation::store_rate_search(
            store,
            search.quote_ordinal,
            search.minute_utc,
            search.searched_through_minute,
            search.now_ms,
            search.grow,
        )
        .map_err(|error| PrefetchError {
            fault: crate::db::valuation::store_fault(error),
            changed: outcome.changed,
        })?;
    }
    Ok(())
}

/// Publish durable prefetch progress even when a later request in the batch must retry.
///
/// Args:
///     result: Completed prefetch flag or transient failure with committed progress.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///
/// Returns:
///     Completed prefetch outcome, or the classified cause after publishing earlier progress.
pub(in crate::db::valuation::worker) fn settle_prefetch(
    result: Result<PrefetchOutcome, PrefetchError>,
    generation: &AtomicU64,
    dirty: &AtomicBool,
) -> Result<PrefetchOutcome, FaultCause> {
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            if error.changed {
                publish(generation, dirty);
            }
            Err(error.fault)
        }
    }
}

/// Load current inputs in bounded joins, retaining a distinct slot for every ordered event.
///
/// Duplicate identities stay duplicated and missing rows leave their own slot empty. No input
/// from a different core or source can satisfy a requested identity. Each query uses at most
/// 768 variables, including the event slots, below SQLite's historical 999-variable limit.
pub(in crate::db::valuation::worker) fn load_trades(
    conn: &Connection,
    events: &[OutboxEvent],
) -> ReadResult<Vec<Option<TradeInput>>> {
    let mut loaded = vec![None; events.len()];
    for source in [TradeSource::Typed, TradeSource::Legacy] {
        let wanted = events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.source == source && event.action == OutboxAction::Row)
            .collect::<Vec<_>>();
        if wanted.is_empty() {
            continue;
        }
        let Some(projection) = load_trade_sql(conn, source)? else {
            continue;
        };
        for chunk in wanted.chunks(256) {
            let values = vec!["(?,?,?)"; chunk.len()].join(",");
            let sql =
                format!("WITH requested(slot,core_uid,row_id) AS (VALUES {values}) {projection}");
            let read_fail = |error| {
                crate::db::read_fail::read_fail_on(conn, "valuation: load report rows", error)
            };
            let mut statement = conn.prepare_cached(&sql).map_err(read_fail)?;
            let rows =
                statement
                    .query_map(
                        rusqlite::params_from_iter(chunk.iter().flat_map(|(slot, event)| {
                            [*slot as i64, event.core_uid, event.row_id]
                        })),
                        |row| {
                            Ok((
                                row.get::<_, i64>(6)? as usize,
                                TradeInput {
                                    source,
                                    core_uid: row.get(0)?,
                                    row_id: row.get(1)?,
                                    closedate: row.get(2)?,
                                    quote_ordinal: row.get(3)?,
                                    profit_quote: row.get(4)?,
                                    spent_quote: row.get(5)?,
                                },
                            ))
                        },
                    )
                    .map_err(read_fail)?;
            for row in rows {
                let (slot, input) = row.map_err(read_fail)?;
                loaded[slot] = Some(input);
            }
        }
    }
    Ok(loaded)
}

/// Build one source's eligibility projection joined to a bounded requested-identity relation.
///
/// Args:
///     conn: Report reader observing committed source data.
///     source: Typed or legacy physical source.
///
/// Returns:
///     The query, `None` when the source is absent, incomplete, or lacks valuation inputs, or a
///     classified schema-probe failure.
fn load_trade_sql(conn: &Connection, source: TradeSource) -> ReadResult<Option<String>> {
    let (table, columns, id_column) = match source_layout(conn, source)? {
        SourceLayout::Found {
            table,
            columns,
            id_column,
        } => (table, columns, id_column),
        SourceLayout::Absent | SourceLayout::Incomplete => return Ok(None),
    };
    if !crate::db::valuation::has_required_trade_inputs(&columns) {
        return Ok(None);
    }
    let spent = if columns.contains("spentbtc") {
        &format!(
            "CASE WHEN typeof(r.spentbtc) IN ('integer','real') THEN {} END",
            crate::db::quote::settled_amount_expr("r", &columns, "spentbtc")
        )
    } else {
        "NULL"
    };
    // The prepared value is keyed by the quote it was computed in, so this projection must yield
    // the EFFECTIVE ordinal — the same one every reader joins the cache with.
    let quote = crate::db::quote::effective_ordinal_expr("r", &columns);
    // Cached under the SETTLED amount, so a reader that corrects a COIN-M liquidation still
    // matches the entry the worker wrote for it.
    let settled_profit = crate::db::quote::settled_amount_expr("r", &columns, "profitbtc");
    Ok(Some(format!(
        "SELECT r.core_uid, r.{id_column}, r.closedate, ({quote}), {settled_profit}, {spent}, requested.slot
         FROM requested JOIN {table} r ON r.core_uid=requested.core_uid AND r.{id_column}=requested.row_id
         WHERE 1=1
           AND typeof(r.closedate)='integer' AND r.closedate>0
           AND ({quote}) BETWEEN 0 AND 20
           AND typeof(r.profitbtc) IN ('integer','real')"
    )))
}

/// Read one keyset batch whose prepared inputs are absent or stale, newest trade first.
///
/// The walk descends `closedate` because a report window shows recent trades: valuing the newest
/// rows first covers what the user is looking at within the first batches, instead of after the
/// whole history. `closedate` is not unique, so the cursor carries `core_uid` and the row id as
/// tie-breaks — a date-only cursor would either skip the rest of a tied group or re-read it
/// forever.
///
/// Time-clustered batches also collapse provider traffic: `prefetch_rates` requests one inclusive
/// minute range per quote, so rows sharing nearby minutes need roughly one request per quote per
/// batch, where a single core's consecutive row ids spanned weeks and cost dozens.
///
/// This ordering DEPENDS on `rep::REP_INDEXES`' `idx_rep_closedate`; the legacy table uses
/// `idx_csr_closedate`. Each index lets the planner seek by `closedate` and block-sort only the
/// `core_uid`/row-id ties; without the source's index, the statement requires a full sort per
/// batch because its primary-key and core indexes do not lead with `closedate`. `ensure_indexes`
/// creates the typed index as soon as that column exists, while database initialization creates
/// the legacy index whenever the legacy table exists. A typed replica still waiting for the
/// `closedate` column has no rows eligible for this query.
///
/// Args:
///     conn: Report reader with `valuation.sqlite` attached.
///     source: Typed or legacy physical source.
///     after: Exclusive descending cursor; `None` starts above the newest row.
///     limit: Maximum mismatched rows to return.
///
/// Returns:
///     Current complete inputs requiring preparation, ordered newest first. `None` when this
///     source's schema has not finished delivering the columns this query needs — a matched
///     layout still missing `core_uid`/its id column, or an attached table not yet carrying
///     `closedate`/`basecurrency`/`profitbtc`. A source with NO layout at all is durably absent
///     and reports that as a real, empty batch (`Some(Vec::new())`) instead, since a
///     fully-migrated user's missing legacy table must not poll forever.
///
/// Errors:
///     Returns a classified read failure on a schema probe or query error, including when the
///     derived cache is not attached to this reader. The latter is a cache-health fact distinct
///     from either "not ready" case above and follows the normal failure/cache-recovery path.
pub(in crate::db::valuation::worker) fn reconciliation_batch(
    conn: &Connection,
    source: TradeSource,
    after: Option<ReconcileCursor>,
    limit: usize,
) -> ReadResult<Option<Vec<TradeInput>>> {
    let (table, columns, id_column) = match source_layout(conn, source)? {
        SourceLayout::Found {
            table,
            columns,
            id_column,
        } => (table, columns, id_column),
        SourceLayout::Absent => return Ok(Some(Vec::new())),
        SourceLayout::Incomplete => return Ok(None),
    };
    if !crate::db::valuation::has_required_trade_inputs(&columns) {
        return Ok(None);
    }
    if !crate::db::valuation::is_attached(conn) {
        return Err(ReadFail::failed(
            FailKind::Other,
            "valuation cache is not attached to this reader",
            crate::config::paths::reports_db_path(),
            "valuation: attach",
            crate::db::FailCode::None,
        ));
    }
    let spent = if columns.contains("spentbtc") {
        &format!(
            "CASE WHEN typeof(r.spentbtc) IN ('integer','real') THEN {} END",
            crate::db::quote::settled_amount_expr("r", &columns, "spentbtc")
        )
    } else {
        "NULL"
    };
    let spent_match = if columns.contains("spentbtc") {
        &format!(
            "v.spent_quote IS CASE WHEN typeof(r.spentbtc) IN ('integer','real') THEN {} END",
            crate::db::quote::settled_amount_expr("r", &columns, "spentbtc")
        )
    } else {
        "v.spent_quote IS NULL"
    };
    // Seeded above every real key so the first batch starts at the newest row. The comparison is
    // strict, so the one key it cannot admit is a row holding `i64::MAX` in ALL THREE columns at
    // once; `closedate` is a Unix second and `core_uid`/`row_id` are allocated counters, so that
    // combination cannot occur. A row at `closedate == i64::MAX` alone is still admitted, through
    // the `core_uid` and row-id comparisons — which is why narrowing this cursor to `closedate`
    // alone would drop it. The `closedate>0` guard below plays no part in any of this.
    let (after_close, after_core, after_row) = after.unwrap_or((i64::MAX, i64::MAX, i64::MAX));
    // One expression for the projection, both joins and the guard: a batch that selected the raw
    // ordinal while joining the effective one would re-prepare the same row on every pass.
    let quote = crate::db::quote::effective_ordinal_expr("r", &columns);
    // Cached under the SETTLED amount, so a reader that corrects a COIN-M liquidation still
    // matches the entry the worker wrote for it.
    let settled_profit = crate::db::quote::settled_amount_expr("r", &columns, "profitbtc");
    let sql = format!(
        "SELECT r.core_uid, r.{id_column}, r.closedate, ({quote}), {settled_profit}, {spent}
         FROM {table} r
         LEFT JOIN valuation.trade_values v
           ON v.source_kind={source_kind}
          AND v.core_uid=r.core_uid AND v.row_id=r.{id_column}
          AND v.algorithm_version={algorithm_version}
          AND v.closedate=r.closedate AND v.quote_ordinal=({quote})
          AND v.profit_quote={settled_profit} AND {spent_match}
         WHERE typeof(r.core_uid)='integer' AND typeof(r.{id_column})='integer'
           AND typeof(r.closedate)='integer' AND r.closedate>0
           AND ({quote}) BETWEEN 0 AND 20
           AND typeof(r.profitbtc) IN ('integer','real')
           AND (r.closedate, r.core_uid, r.{id_column}) < (?1, ?2, ?3)
           AND v.row_id IS NULL
         ORDER BY r.closedate DESC, r.core_uid DESC, r.{id_column} DESC LIMIT ?4",
        source_kind = source.code(),
        algorithm_version = crate::db::valuation::ALGORITHM_VERSION,
    );
    let mut stmt = conn.prepare(&sql).map_err(|error| {
        crate::db::read_fail::read_fail_on(conn, "valuation: reconcile prepare", error)
    })?;
    let rows = stmt
        .query_map(
            rusqlite::params![after_close, after_core, after_row, limit as i64],
            |row| {
                Ok(TradeInput {
                    source,
                    core_uid: row.get(0)?,
                    row_id: row.get(1)?,
                    closedate: row.get(2)?,
                    quote_ordinal: row.get(3)?,
                    profit_quote: row.get(4)?,
                    spent_quote: row.get(5)?,
                })
            },
        )
        .map_err(|error| {
            crate::db::read_fail::read_fail_on(conn, "valuation: reconcile query", error)
        })?;
    let mut inputs = Vec::new();
    for row in rows {
        inputs.push(row.map_err(|error| {
            crate::db::read_fail::read_fail_on(conn, "valuation: reconcile row", error)
        })?);
    }
    Ok(Some(inputs))
}
