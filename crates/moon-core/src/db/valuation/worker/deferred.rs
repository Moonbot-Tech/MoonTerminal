use super::*;

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
///     deferred: Current-minute rows retained by identity.
///
/// Returns:
///     Completed stage turn indicating whether to loop immediately, or a classified failure.
pub(in crate::db::valuation::worker) fn process_deferred(
    store: &Connection,
    source: &dyn SpotRateSource,
    axis: &ReportAxis,
    generation: &AtomicU64,
    dirty: &AtomicBool,
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
) -> Result<StageTurn, FaultCause> {
    let current = current_minute_utc();
    let blocked =
        crate::db::valuation::blocked_rate_searches(store, now_unix_ms_i64()).unwrap_or_default();
    let keys = deferred
        .iter()
        .filter(|(_, input)| {
            let minute = valuation_minute(axis, input);
            minute < current && !blocked.contains(&(input.quote_ordinal, minute))
        })
        .map(|(key, _)| *key)
        .take(DEFERRED_BATCH)
        .collect::<Vec<_>>();
    let inputs = keys
        .iter()
        .filter_map(|key| deferred.get(key).cloned())
        .collect::<Vec<_>>();
    let prefetched = settle_prefetch(
        prefetch_rates(store, source, axis, &inputs),
        generation,
        dirty,
    )?;
    let mut changed = prefetched.changed;
    let mut provider_fault = prefetched.provider_fault;
    for key in &keys {
        let Some(input) = deferred.get(key).cloned() else {
            continue;
        };
        let minute = valuation_minute(axis, &input);
        match prepare_trade(
            store,
            source,
            axis,
            &input,
            prefetched
                .canonical_exact_missing
                .contains(&(input.quote_ordinal, minute)),
        ) {
            PrepareResult::Complete {
                changed: input_changed,
            } => {
                changed |= input_changed;
                deferred.remove(key);
            }
            PrepareResult::Deferred {
                changed: input_changed,
            } => changed |= input_changed,
            PrepareResult::Retry(error) if error.kind == FailureKind::Provider => {
                changed |= defer_provider_trade(store, axis, &input)?;
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
        more: current_minute_closed_any(store, axis, deferred),
    })
}

/// Whether any retained row's candle minute has closed.
///
/// Args:
///     store: Open valuation store carrying persisted retry boundaries.
///     axis: Per-core time axis rate minutes are resolved against.
///     deferred: Current-minute and unresolved rows retained by identity.
///
/// Returns:
///     `true` when at least one row can now be retried.
pub(in crate::db::valuation::worker) fn current_minute_closed_any(
    store: &Connection,
    axis: &ReportAxis,
    deferred: &BTreeMap<(i64, i64, i64), TradeInput>,
) -> bool {
    let current = current_minute_utc();
    let blocked =
        crate::db::valuation::blocked_rate_searches(store, now_unix_ms_i64()).unwrap_or_default();
    deferred.values().any(|input| {
        let minute = valuation_minute(axis, input);
        minute < current && !blocked.contains(&(input.quote_ordinal, minute))
    })
}
