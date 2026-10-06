use super::*;

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
    let current = current_minute_utc();
    let mut groups: BTreeMap<(i64, &'static str), BTreeSet<i64>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut changed = false;
    for input in inputs {
        let minute = valuation_minute(axis, input);
        if !seen.insert((input.quote_ordinal, minute)) {
            continue;
        }
        if minute >= current {
            continue;
        }
        let Some(currency) = crate::db::QuoteCurrency::from_report_ordinal(input.quote_ordinal)
        else {
            continue;
        };
        match crate::db::valuation::cached_rate(store, input.quote_ordinal, minute) {
            Ok(Some(_)) => continue,
            Ok(None) => {
                match crate::db::valuation::covering_successor_rate(
                    store,
                    input.quote_ordinal,
                    minute,
                ) {
                    Ok(Some(rate)) => {
                        match crate::db::valuation::store_rate(store, &rate, now_unix_ms_i64()) {
                            Ok(stored) => changed |= stored > 0,
                            Err(error) => {
                                return Err(PrefetchError {
                                    fault: crate::db::valuation::store_fault(error),
                                    changed,
                                });
                            }
                        }
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Err(PrefetchError {
                            fault: crate::db::valuation::store_fault(error),
                            changed,
                        });
                    }
                }
                match crate::db::valuation::rate_search_start(
                    store,
                    input.quote_ordinal,
                    minute,
                    now_unix_ms_i64(),
                ) {
                    Ok(None) => continue,
                    Ok(Some(search_start)) if search_start > minute => continue,
                    Ok(Some(_)) => {}
                    Err(error) => {
                        return Err(PrefetchError {
                            fault: crate::db::valuation::store_fault(error),
                            changed,
                        });
                    }
                }
                groups
                    .entry((input.quote_ordinal, currency.ticker()))
                    .or_default()
                    .insert(minute);
            }
            Err(error) => {
                return Err(PrefetchError {
                    fault: crate::db::valuation::store_fault(error),
                    changed,
                });
            }
        }
    }
    let mut canonical_exact_missing = BTreeSet::new();
    let mut provider_fault = None;
    for ((quote_ordinal, ticker), minutes) in groups {
        let minutes = minutes.into_iter().collect::<Vec<_>>();
        let mut start = 0;
        while start < minutes.len() {
            let first = minutes[start];
            let mut end = start + 1;
            while end < minutes.len()
                && end - start < 1_000
                && minutes[end].saturating_sub(first) <= 999 * 60
            {
                end += 1;
            }
            let batch = resolve_rate_batch(source, quote_ordinal, ticker, &minutes[start..end]);
            if batch.transient.is_none() {
                debug_assert_eq!(batch.ready.len() + batch.missing.len(), end - start);
            }
            let fetched_at = now_unix_ms_i64();
            canonical_exact_missing
                .extend(batch.missing.iter().map(|minute| (quote_ordinal, *minute)));
            for rate in &batch.ready {
                if rate.candle_close_ms >= fetched_at {
                    return Err(PrefetchError {
                        fault: FaultCause::new(
                            FailureKind::Provider,
                            format!(
                                "{} {} returned an unclosed minute {}",
                                rate.provider, rate.symbol, rate.minute_utc
                            ),
                        ),
                        changed,
                    });
                }
                match crate::db::valuation::store_rate(store, rate, fetched_at) {
                    Ok(stored) => changed |= stored > 0,
                    Err(error) => {
                        return Err(PrefetchError {
                            fault: crate::db::valuation::store_fault(error),
                            changed,
                        });
                    }
                }
            }
            if let Some(error) = batch.transient {
                for minute in &minutes[start..end] {
                    if batch.ready.iter().any(|rate| rate.minute_utc == *minute) {
                        continue;
                    }
                    crate::db::valuation::store_rate_search(
                        store,
                        quote_ordinal,
                        *minute,
                        minute.saturating_sub(60),
                        fetched_at,
                        false,
                    )
                    .map_err(|error| PrefetchError {
                        fault: crate::db::valuation::store_fault(error),
                        changed,
                    })?;
                }
                provider_fault.get_or_insert_with(|| FaultCause::new(FailureKind::Provider, error));
            }
            start = end;
        }
    }
    Ok(PrefetchOutcome {
        provider_fault,
        changed,
        canonical_exact_missing,
    })
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

/// Per-batch report-row loader: probes each source's layout once and reuses its statement.
///
/// The layout is derived from the replica schema, which cannot change under one reader
/// connection's batch, so resolving it per event only repeated two `PRAGMA table_info` probes and
/// the COIN-M scan for every row.
#[derive(Default)]
pub(in crate::db::valuation::worker) struct TradeLoader {
    /// Lazily built single-row query per source; inner `None` means the source cannot be valued.
    sql: [Option<Option<String>>; 2],
}

impl TradeLoader {
    /// Load one current eligible report row through the batch's cached layout and statement.
    ///
    /// Args:
    ///     conn: Report reader observing committed source data, the same for the whole batch.
    ///     source: Typed or legacy physical source.
    ///     core_uid: Runtime core identity.
    ///     row_id: `newrecid` or `db_id` according to `source`.
    ///
    /// Returns:
    ///     Complete valuation inputs, no eligible/current row, or a classified read failure.
    pub(in crate::db::valuation::worker) fn load(
        &mut self,
        conn: &Connection,
        source: TradeSource,
        core_uid: i64,
        row_id: i64,
    ) -> ReadResult<Option<TradeInput>> {
        let slot = &mut self.sql[usize::from(source == TradeSource::Legacy)];
        if slot.is_none() {
            *slot = Some(load_trade_sql(conn, source)?);
        }
        let Some(Some(sql)) = slot.as_ref() else {
            return Ok(None);
        };
        let read_fail =
            |error| crate::db::read_fail::read_fail_on(conn, "valuation: load report row", error);
        let mut stmt = conn.prepare_cached(sql).map_err(read_fail)?;
        stmt.query_row(rusqlite::params![core_uid, row_id], |row| {
            Ok(TradeInput {
                source,
                core_uid: row.get(0)?,
                row_id: row.get(1)?,
                closedate: row.get(2)?,
                quote_ordinal: row.get(3)?,
                profit_quote: row.get(4)?,
                spent_quote: row.get(5)?,
            })
        })
        .optional()
        .map_err(read_fail)
    }
}

/// Build the single-row eligibility query for one source's current layout.
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
        "SELECT r.core_uid, r.{id_column}, r.closedate, ({quote}), {settled_profit}, {spent}
         FROM {table} r
         WHERE r.core_uid=?1 AND r.{id_column}=?2
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

use rusqlite::OptionalExtension;
