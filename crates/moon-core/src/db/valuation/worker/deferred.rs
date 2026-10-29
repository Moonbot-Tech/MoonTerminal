use super::*;

/// Retain an unresolved input without an in-memory entry per deal; restart re-derives the queue.
pub(in crate::db::valuation::worker) fn defer_trade(
    store: &Connection,
    axis: &ReportAxis,
    input: &TradeInput,
) -> Result<(), FaultCause> {
    let minute = valuation_minute(axis, input);
    store
        .prepare_cached(
            &format!("INSERT INTO temp.deferred_trades VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,
                MAX(?9,COALESCE((SELECT next_retry_at_ms FROM rate_searches WHERE algorithm_version={}
                  AND quote_ordinal=?5 AND minute_utc=?8),0)))
        ON CONFLICT(source_kind,core_uid,row_id) DO UPDATE SET closedate=excluded.closedate,
        quote_ordinal=excluded.quote_ordinal,profit_quote=excluded.profit_quote,
        spent_quote=excluded.spent_quote,minute_utc=excluded.minute_utc,
        closed_at_ms=excluded.closed_at_ms,ready_at_ms=excluded.ready_at_ms",super::super::ALGORITHM_VERSION),
        )
        .and_then(|mut statement| {
            statement.execute(rusqlite::params![
                input.source.code(),
                input.core_uid,
                input.row_id,
                input.closedate,
                input.quote_ordinal,
                input.profit_quote,
                input.spent_quote,
                minute,
                minute.saturating_add(60).saturating_mul(1000),
            ])
        })
        .map_err(super::super::store_fault)?;
    Ok(())
}

/// Retire only the pending identities whose values were made durable by this batch.
pub(in crate::db::valuation::worker) fn forget_deferred(
    store: &Connection,
    keys: &BTreeSet<(i64, i64, i64)>,
) -> Result<(), FaultCause> {
    let keys = keys.iter().copied().collect::<Vec<_>>();
    for chunk in keys.chunks(256) {
        let values = vec!["(?,?,?)"; chunk.len()].join(",");
        store.prepare_cached(&format!("DELETE FROM temp.deferred_trades WHERE (source_kind,core_uid,row_id) IN (VALUES {values})"))
            .and_then(|mut statement| statement.execute(rusqlite::params_from_iter(chunk.iter().flat_map(|(source,core,row)| [*source,*core,*row]))))
            .map_err(super::super::store_fault)?;
    }
    Ok(())
}

/// Seek the earliest due rows without scanning the blocked backlog or loading its rate keys.
fn deferred_inputs(store: &Connection, limit: usize) -> Result<Vec<TradeInput>, FaultCause> {
    let mut statement = store.prepare_cached("SELECT d.source_kind,d.core_uid,d.row_id,d.closedate,d.quote_ordinal,d.profit_quote,d.spent_quote
        FROM temp.deferred_trades d INDEXED BY deferred_due WHERE d.ready_at_ms<=?1 AND d.minute_utc<?2
        ORDER BY d.ready_at_ms,d.source_kind,d.core_uid,d.row_id LIMIT ?3").map_err(super::super::store_fault)?;
    let rows = statement
        .query_map(
            rusqlite::params![now_unix_ms_i64(), current_minute_utc(), limit as i64],
            |row| {
                Ok(TradeInput {
                    source: TradeSource::from_code(row.get(0)?)?,
                    core_uid: row.get(1)?,
                    row_id: row.get(2)?,
                    closedate: row.get(3)?,
                    quote_ordinal: row.get(4)?,
                    profit_quote: row.get(5)?,
                    spent_quote: row.get(6)?,
                })
            },
        )
        .map_err(super::super::store_fault)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(super::super::store_fault)
}

/// Whether any minute boundary can make pending work eligible, without loading the backlog.
pub(in crate::db::valuation::worker) fn has_deferred(store: &Connection) -> bool {
    store
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM temp.deferred_trades)",
            [],
            |row| row.get(0),
        )
        .unwrap_or(true)
}

/// Resolution of one source's physical layout, distinguishing a durable absence from a startup
/// schema that has not finished arriving.
///
/// These are different facts and must not be conflated: a fully-migrated user's missing legacy
/// table is a permanent, correct "nothing here", while a fresh reports database that has not yet
/// run `apply_schema` is a transient state that must be retried, not treated as drained.
pub(in crate::db::valuation::worker) enum SourceLayout {
    /// Physical table and stable row identity resolved; query-specific input columns may still be
    /// absent.
    Found {
        /// Physical table this source resolves to.
        table: &'static str,
        /// Columns the attached schema currently carries for that table.
        columns: std::collections::HashSet<String>,
        /// Stable row-id column name for this source (`newrecid` or `db_id`).
        id_column: &'static str,
    },
    /// No layout matches this source at all — e.g. a fully-migrated user has no legacy table.
    /// Permanent for the life of this schema.
    Absent,
    /// A layout matched, but its `core_uid` or id column has not arrived yet — the startup schema
    /// `apply_schema` delivers gradually. Transient; becomes `Found` once the column lands.
    Incomplete,
}

/// Resolve one source's physical table, column set, and stable row-id column.
///
/// Args:
///     conn: Open report reader.
///     source: Typed or legacy source partition.
///
/// Returns:
///     Source layout classified as found, durably absent, or not yet complete; or a classified
///     schema-probe failure.
pub(in crate::db::valuation::worker) fn source_layout(
    conn: &Connection,
    source: TradeSource,
) -> ReadResult<SourceLayout> {
    for layout in crate::db::read_sources_res(conn)? {
        if layout.legacy == (source == TradeSource::Legacy) {
            let id_column = if layout.legacy { "db_id" } else { "newrecid" };
            if !layout.cols.contains(id_column) || !layout.cols.contains("core_uid") {
                return Ok(SourceLayout::Incomplete);
            }
            return Ok(SourceLayout::Found {
                table: layout.table,
                columns: layout.cols,
                id_column,
            });
        }
    }
    Ok(SourceLayout::Absent)
}

/// Delete one prepared value, reporting whether storage changed.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Typed or legacy source partition.
///     core_uid: Runtime core identity.
///     row_id: Physical source row identity.
///
/// Returns:
///     Completed result carrying the delete-change flag, or a retry result on SQLite failure.
pub(in crate::db::valuation::worker) fn delete_trade(
    store: &Connection,
    source: TradeSource,
    core_uid: i64,
    row_id: i64,
) -> PrepareResult {
    if let Err(error) = forget_deferred(store, &BTreeSet::from([(source.code(), core_uid, row_id)]))
    {
        return PrepareResult::Retry(error);
    }
    match store
        .prepare_cached(
            "DELETE FROM trade_values WHERE source_kind=?1 AND core_uid=?2 AND row_id=?3",
        )
        .and_then(|mut statement| {
            statement.execute(rusqlite::params![source.code(), core_uid, row_id])
        }) {
        Ok(changed) => PrepareResult::Complete {
            changed: changed > 0,
        },
        Err(error) => PrepareResult::Retry(crate::db::valuation::store_fault(error)),
    }
}

/// Delete one prepared source/core partition after reset or legacy purge.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Typed or legacy source partition.
///     core_uid: Runtime core identity.
///
/// Returns:
///     Completed result carrying the delete-change flag, or a retry result on SQLite failure.
pub(in crate::db::valuation::worker) fn delete_partition(
    store: &Connection,
    source: TradeSource,
    core_uid: i64,
) -> PrepareResult {
    if let Err(error) = store
        .prepare_cached("DELETE FROM temp.deferred_trades WHERE source_kind=?1 AND core_uid=?2")
        .and_then(|mut statement| statement.execute(rusqlite::params![source.code(), core_uid]))
    {
        return PrepareResult::Retry(super::super::store_fault(error));
    }
    match store
        .prepare_cached("DELETE FROM trade_values WHERE source_kind=?1 AND core_uid=?2")
        .and_then(|mut statement| statement.execute(rusqlite::params![source.code(), core_uid]))
    {
        Ok(changed) => PrepareResult::Complete {
            changed: changed > 0,
        },
        Err(error) => PrepareResult::Retry(crate::db::valuation::store_fault(error)),
    }
}

/// Process one bounded batch of deferred rows whose containing minute is now closed.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     axis: Per-core time axis rate minutes are resolved against.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///
/// Returns:
///     Completed stage turn indicating whether to loop immediately, or a classified failure.
pub(in crate::db::valuation::worker) fn process_deferred(
    store: &Connection,
    source: &dyn SpotRateSource,
    axis: &ReportAxis,
    generation: &AtomicU64,
    dirty: &AtomicBool,
) -> Result<StageTurn, FaultCause> {
    let inputs = deferred_inputs(store, DEFERRED_BATCH)?;
    let prefetched = settle_prefetch(
        prefetch_rates(store, source, axis, &inputs),
        generation,
        dirty,
    )?;
    let mut changed = prefetched.changed;
    let mut provider_fault = prefetched.provider_fault;
    let (ready, ready_changed) = match prepare_ready_trades(store, axis, &inputs) {
        Ok(ready) => ready,
        Err(error) => {
            if changed {
                publish(generation, dirty);
            }
            return Err(error);
        }
    };
    changed |= ready_changed;
    for input in &inputs {
        let key = trade_key(input);
        if ready.contains(&key) {
            continue;
        }
        let minute = valuation_minute(axis, input);
        match prepare_trade(
            store,
            source,
            axis,
            input,
            prefetched
                .canonical_exact_missing
                .contains(&(input.quote_ordinal, minute)),
        ) {
            PrepareResult::Complete {
                changed: input_changed,
            } => {
                changed |= input_changed;
                forget_deferred(store, &BTreeSet::from([key]))?;
            }
            PrepareResult::Deferred {
                changed: input_changed,
            } => {
                changed |= input_changed;
                defer_trade(store, axis, input)?;
            }
            PrepareResult::Retry(error) if error.kind == FailureKind::Provider => {
                changed |= defer_provider_trade(store, axis, input)?;
                defer_trade(store, axis, input)?;
                provider_fault.get_or_insert(error);
            }
            PrepareResult::Retry(error) => {
                if changed {
                    publish(generation, dirty);
                }
                return Err(error);
            }
        }
    }
    if changed {
        publish(generation, dirty);
    }
    if let Some(error) = provider_fault {
        return Err(error);
    }
    Ok(StageTurn::Ran {
        more: current_minute_closed_any(store),
    })
}

/// Whether any retained row's candle minute has closed.
///
/// Args:
///     store: Open valuation store carrying persisted retry boundaries.
///
/// Returns:
///     `true` when at least one row can now be retried.
pub(in crate::db::valuation::worker) fn current_minute_closed_any(store: &Connection) -> bool {
    // Let the fallible stage report a query failure; an eligibility failure must not masquerade
    // as an empty queue and suppress that stage forever.
    deferred_inputs(store, 1)
        .map(|inputs| !inputs.is_empty())
        .unwrap_or(true)
}
