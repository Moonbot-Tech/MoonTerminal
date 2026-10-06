//! passes items for report reads.

use super::*;

/// Execute one complete Report totals pass with fresh accumulators.
///
/// Args:
///     conn: Open report reader or snapshot.
///     f: Complete Report filter.
///     sources: Physical report sources discovered from `main`.
///     meta: Strategy metadata of this read, resolved once for both attempts.
///     include_valuation: Whether the historical mode may join the attached derived cache; the
///         current-rate mode does not depend on it.
///
/// Returns:
///     Exact quote profit totals over closed rows, two-sided traded volume over the reconstructed
///     rows of each quote, and counted entry-spend subtotals over the same closed rows,
///     optionally carrying active-mode USDT coverage for each metric, plus the unrealized tally of
///     the rows still open.
///
/// Errors:
///     Returns the underlying SQLite error from any physical-source aggregate.
pub(super) fn query_totals_attempt(
    conn: &Connection,
    f: &ReportFilter,
    sources: &[ReadSource],
    meta: &StrategyMeta,
    include_valuation: bool,
) -> rusqlite::Result<ReportTotals> {
    let mut sink = totals::TotalsSink::default();
    let mut open_groups = Vec::new();
    // Loop-invariant: `projection` yields a builder for the current-rate mode whatever the cache is
    // doing, and for the historical one exactly when the cache may be joined.
    let valuation_present = f.valuation == ValuationMode::Current || include_valuation;
    // The two scopes are aggregated by SEPARATE statements rather than one query sliced by CASE.
    // Two independent reasons, and either alone would be enough. Correctness: the valuation
    // coverage columns test only whether a row's quote is known, never whether it closed, so a
    // combined result set folds unrealized money into the USDT coverage and breaks its own
    // "every eligible row is valued" completeness rule. Speed: the combined arm's
    // `((closed AND window) OR open)` puts a non-sargable disjunct beside the window, and SQLite
    // will not use `idx_rep_core_close` for an OR unless every branch is indexable — which would
    // cost the footer its index on the DEFAULT period, the hottest read in the panel.
    //
    // The realized pass FAILS OPEN on a source that cannot express `closedate`: it asks for the
    // combined scope there, which emits no row predicate at all, so a replica whose schema has not
    // arrived yet keeps stating its money instead of reporting an empty period. That degradation
    // is the OPPOSITE of the row query's, and deliberately: withholding a row the user has no
    // other way to see is a smaller harm than blanking the figure they are reading. The open pass
    // still fails CLOSED on the same source — an unprovable position must never be invented. The
    // realized pass's scope choice lives in `totals::ClosedPass::new`.
    for src in sources {
        totals::ClosedPass::new(src, f, include_valuation, meta).run_grouped(conn, &mut sink)?;
    }
    // The open pass: a plain per-quote tally, with no window, no coverage and no volume — none of
    // those mean anything for a position that has not closed. Skipped entirely for a caller that
    // asked for closed rows, which is what keeps chart history and the purge scan on one query.
    if f.rows != RowScope::Closed {
        let open_scope = ReportFilter {
            rows: RowScope::Open,
            ..f.clone()
        };
        for src in sources {
            let (where_sql, params) = build_where(&open_scope, &src.cols, meta);
            let profit = profit_column(src).aggregate_sql();
            let (quote, group_by) = crate::db::quote::trusted_quote_group("r", &src.cols);
            let sql = format!(
                "SELECT {quote}, {profit}, COUNT(*) FROM {} r{where_sql}{group_by}",
                src.table,
            );
            let refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|b| b.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(refs.as_slice())?;
            while let Some(row) = rows.next()? {
                let raw = row.get::<_, Value>(0)?;
                let ordinal = crate::db::quote::report_ordinal_from_value(&raw);
                open_groups.push((ordinal, row.get::<_, f64>(1)?, row.get::<_, i64>(2)?));
            }
        }
    }
    Ok(sink.finish(valuation_present, open_groups))
}

/// Everything one Report row pass needs except whether the derived cache may be joined.
///
/// Held together so the retry after a corrupt cache reruns the SAME request, differing in exactly
/// the one flag the corruption bears on.
struct ReportPass<'a> {
    /// Complete Report filter.
    filter: &'a ReportFilter,
    /// Shared runtime display columns, resolved once for both attempts.
    cols: &'a [String],
    /// Validated runtime sort-column key.
    sort_col: &'a str,
    /// Whether to sort descending.
    desc: bool,
    /// Maximum merged rows.
    limit: usize,
    /// Physical report sources discovered from `main`.
    sources: &'a [ReadSource],
    /// Strategy metadata of this read, resolved once for both attempts.
    meta: &'a StrategyMeta,
}

/// Execute one complete Report row request: the closed rows, and the open ones ahead of them.
///
/// Each entry is `(core_uid, rec_id, data)`; `rec_id` is the replica `newrecid`, or 0 for a legacy
/// row that has none.
///
/// The two scopes are queried SEPARATELY, each with its own limit, rather than merged into one
/// truncated pass. Open positions are a handful of rows against a ledger of tens of thousands, so
/// under any sort the user actually picks — profit, spent, coin — a shared limit would push every
/// one of them past the cut and the period would look as though nothing were running. Their block
/// leads the result for the same reason: an open row has no close time to be ordered by, and
/// scattering it through a realized ledger is how it gets read as realized.
///
/// Args:
///     conn: Open report reader or snapshot.
///     pass: The request, identical across both attempts.
///     include_valuation: Whether the historical mode may join the attached derived cache; the
///         current-rate mode does not depend on it.
///
/// Returns:
///     The open block followed by the globally sorted closed rows, at most `limit` rows in total.
///
/// Errors:
///     Returns the underlying SQLite error from any physical-source query.
fn query_reports_attempt(
    conn: &Connection,
    pass: &ReportPass,
    include_valuation: bool,
) -> rusqlite::Result<Vec<(u64, i64, Vec<Value>)>> {
    // A single-scope request runs exactly one query; only the combined scope pays for two.
    match pass.filter.rows {
        RowScope::Closed => {
            run_row_pass(conn, pass, include_valuation, RowScope::Closed, pass.limit)
        }
        RowScope::Open => run_row_pass(conn, pass, include_valuation, RowScope::Open, pass.limit),
        RowScope::OpenIfCurrent => run_row_pass(
            conn,
            pass,
            include_valuation,
            RowScope::OpenIfCurrent,
            pass.limit,
        ),
        // Both combined scopes split into the same two passes; they differ only in whether the
        // open half is filtered to the cores whose window still reaches the present.
        RowScope::ClosedAndOpen | RowScope::ClosedAndOpenIfCurrent => {
            let open_scope = if pass.filter.rows == RowScope::ClosedAndOpen {
                RowScope::Open
            } else {
                RowScope::OpenIfCurrent
            };
            let mut open = run_row_pass(conn, pass, include_valuation, open_scope, pass.limit)?;
            // The open block SPENDS from the caller's budget rather than sitting outside it, so the
            // result is still at most `limit` rows and every consumer sized by that cap stays
            // correct. The second pass does not buy EXTRA rows, it buys GUARANTEED ones: a handful
            // of running positions can no longer be sorted out of the head by tens of thousands of
            // closed trades.
            let remaining = pass.limit.saturating_sub(open.len());
            let closed = run_row_pass(conn, pass, include_valuation, RowScope::Closed, remaining)?;
            open.extend(closed);
            Ok(open)
        }
    }
}

/// Execute ONE row pass over every physical source at one row scope, then merge and truncate.
///
/// Args:
///     conn: Open report reader or snapshot.
///     pass: The request, identical across both attempts.
///     include_valuation: Whether the historical mode may join the attached derived cache.
///     rows: The scope this pass alone selects, overriding the request's own.
///     limit: Maximum merged rows for this pass.
///
/// Returns:
///     Globally sorted rows of that scope, truncated to `limit`.
///
/// Errors:
///     Returns the underlying SQLite error from any physical-source query.
fn run_row_pass(
    conn: &Connection,
    pass: &ReportPass,
    include_valuation: bool,
    rows: RowScope,
    limit: usize,
) -> rusqlite::Result<Vec<(u64, i64, Vec<Value>)>> {
    // The open block carries its OWN order, newest opening first, and does not follow the column
    // the table is sorted by. It is not part of that ordering to begin with — it is a separate
    // leading block of present state — and the question it answers is "what is running right
    // now", whose natural reading is most-recent-first. Following an ascending sort would put the
    // position opened weeks ago at the top of the panel, which is the least interesting row in
    // the block. Falls back to the caller's own sort on a source too early in its schema to have
    // `buydate`.
    let open_order = matches!(rows, RowScope::Open | RowScope::OpenIfCurrent)
        && pass.cols.iter().any(|col| col == "buydate");
    let (sort_col, desc) = if open_order {
        ("buydate", true)
    } else {
        (pass.sort_col, pass.desc)
    };
    let dir = if desc { "DESC" } else { "ASC" };
    let sort_ix = pass.cols.iter().position(|c| c == sort_col);
    // This pass's own scope; every other predicate stays exactly as the caller built it.
    let scoped = ReportFilter {
        rows,
        ..pass.filter.clone()
    };
    // Query the top N from EACH source separately so indexes work, then merge below.
    let mut merged: Vec<(u64, i64, Vec<Value>)> = Vec::new();
    for src in pass.sources {
        let (where_sql, mut params) = build_where(&scoped, &src.cols, pass.meta);
        let valuation = crate::db::valuation::projection(
            pass.filter.valuation,
            include_valuation,
            "r",
            &src.cols,
            source_partition(src),
        );
        let joins = valuation
            .as_ref()
            .map(|parts| parts.per_row.joins.as_str())
            .unwrap_or("");
        let select = source_select(src, pass.cols, valuation.as_ref(), &pass.filter.core_names);
        let rec_id_select = rec_id_expr(src);
        // Sort in SQL only if this source can express the column; otherwise source order is
        // irrelevant, because the merge below reorders everything anyway.
        //
        // The DESC arm drops the leading `({expression}) IS NULL` term: SQLite orders NULL
        // lowest, so a plain `{expression} DESC` already places NULLs last, exactly what the
        // `IS NULL` term bought. Without it that leading term forces a temp b-tree sort instead
        // of using the source's index on `{expression}`. The ASC arm keeps the term — without it
        // NULLs would sort FIRST there, a real result change — so do not drop it from that arm.
        //
        // The DESC arm then MUST carry the primary key as a tie-break, and that is not a
        // refinement — it is what keeps the rewrite honest. `LIMIT` is applied per source
        // BEFORE the merge below, and SQL guarantees nothing about the relative order of rows
        // equal on every ORDER BY term. Dropping the leading term changes the plan from a temp
        // sort to a backwards index walk, so a tie straddling the limit boundary would surface
        // a DIFFERENT SET of rows, not merely a different order: measured on SQLite 3.50.4 with
        // 20 rows sharing one `closedate`, 19 of 25 limits returned a different set. The key
        // `(core_uid, newrecid)` is `rep.rs`'s own PRIMARY KEY, so it is total on the typed
        // replica and the whole result becomes DEFINED rather than planner-chosen. It costs the
        // sort only WITHIN each group of equal values — measured 2.4 ms -> 2.8 ms against
        // 4804 ms before the rewrite, so the gain survives it intact.
        //
        // The key is taken PER SOURCE, because the two sources do not share one. The typed
        // replica is keyed `(core_uid, newrecid)` (`rep.rs`'s own PRIMARY KEY) and the legacy
        // `closed_sell_reports` is keyed `(core_uid, db_id)` (`db/mod.rs` module docs) — and reaching for
        // `rec_id_expr` here instead would be a trap twice over: it yields the LITERAL `0` on a
        // source without `newrecid`, and SQLite reads a bare integer in ORDER BY as a COLUMN
        // ORDINAL even parenthesised, so it would fail the whole query rather than order it.
        // The same source-shape test `source_sort_expression` already uses for `id` is what
        // picks the right column. A source offering neither keeps today's undefined tie order,
        // which is no worse than before this rewrite.
        let order = match source_sort_expression(
            src,
            sort_col,
            valuation.as_ref(),
            &pass.filter.core_names,
        ) {
            Some(expression) if desc => {
                let mut order = format!("{expression} {dir}");
                if src.cols.contains("core_uid") {
                    order.push_str(&format!(", r.core_uid {dir}"));
                }
                if src.cols.contains("newrecid") {
                    order.push_str(&format!(", r.newrecid {dir}"));
                } else if src.legacy && src.cols.contains("db_id") {
                    order.push_str(&format!(", r.\"db_id\" {dir}"));
                }
                order
            }
            Some(expression) => format!("({expression}) IS NULL, {expression} {dir}"),
            None => "1".to_string(),
        };
        let sql = format!(
            "SELECT r.core_uid, {rec_id_select}, {select} FROM {} r{joins}{where_sql} ORDER BY {order} LIMIT ?",
            src.table
        );
        params.push(Box::new(limit as i64));
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let n = pass.cols.len();
        let mapped = stmt.query_map(refs.as_slice(), |r| {
            let core_uid = r.get::<_, i64>(0)? as u64;
            let rec_id = r.get::<_, i64>(1)?;
            let mut v = Vec::with_capacity(n);
            for i in 0..n {
                v.push(r.get::<_, Value>(i + 2)?);
            }
            Ok((core_uid, rec_id, v))
        })?;
        // Every row is a trade the user is entitled to see, and the same rows
        // are what the export writes — so no row error is skippable here.
        for row in mapped {
            merged.push(row?);
        }
    }

    // Merge with NULL always last, like `{col} IS NULL` in SQL, then apply direction.
    merged.sort_by(|a, b| {
        let va = sort_ix.and_then(|i| a.2.get(i)).unwrap_or(&Value::Null);
        let vb = sort_ix.and_then(|i| b.2.get(i)).unwrap_or(&Value::Null);
        match (matches!(va, Value::Null), matches!(vb, Value::Null)) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => {
                let o = cmp_values(va, vb);
                // `desc`, not `pass.desc`: this merge decides the ORDER THE USER SEES, so it must
                // follow the same direction the pass just selected its rows by. Reading the
                // caller's direction here would let SQL fetch the newest open positions and the
                // merge then hand them back oldest-first.
                if desc { o.reverse() } else { o }
            }
        }
    });
    merged.truncate(limit);
    Ok(merged)
}

/// Return the top `limit` reports for the filter and sort using all display columns.
///
/// Source, schema, query, and row-conversion errors map to `Failed`; no partial
/// table is returned, which also keeps exports from writing incomplete data.
/// The open connection means this function cannot return `NotReady`.
///
/// Args:
///     conn: Open report reader or snapshot.
///     f: Complete Report filter.
///     sort_key: Requested runtime column name.
///     desc: Whether to sort descending.
///     limit: Maximum merged rows.
///
/// Returns:
///     Runtime columns and the top matching report rows.
///
/// Errors:
///     Returns `Failed` for source, schema, SQL, or row conversion errors.
pub fn query_reports(
    conn: &Connection,
    f: &ReportFilter,
    sort_key: &str,
    desc: bool,
    limit: usize,
) -> ReadResult<ReportTable> {
    query_reports_with_columns(conn, f, sort_key, desc, limit, display_columns(conn)?)
}

/// Read closed Mini App trades with the Report's safe entry-notional conversion rate.
///
/// The extra column is confined to this read, leaving desktop display/export columns intact.
/// Missing inputs, inverse denomination, Funding and liquidation withhold the rate; database
/// failures retain the same `ReadFail` contract as [`query_reports`].
pub fn query_mini_trades(
    conn: &Connection,
    f: &ReportFilter,
    limit: usize,
) -> ReadResult<ReportTable> {
    query_closed_with(conn, f, limit, &[MINI_ENTRY_VOLUME_RATE_COLUMN])
}

/// [`query_mini_trades`] plus each trade's own money: its settled profit, its entry notional and
/// the currency both are in, read without any USDT valuation.
///
/// What the bot's trade cards print, so a card never has to wait for the valuation, and what the
/// rule thresholds fall back to on a USD stablecoin quote. The native columns go through the same
/// quote expressions as the Report's per-currency totals, so a COIN-M row reads in BTC, not in the
/// USDT its label claims.
///
/// Args:
///     conn: Report connection or snapshot.
///     f: Row filter; its scope is forced to closed rows.
///     limit: Maximum merged rows.
///
/// Returns:
///     [`query_mini_trades`]'s columns plus [`NOTIFY_PROFIT_NATIVE_COLUMN`],
///     [`NOTIFY_ENTRY_VOLUME_NATIVE_COLUMN`] and [`NOTIFY_QUOTE_COLUMN`].
///
/// Errors:
///     The same `ReadFail` contract as [`query_reports`].
pub fn query_notify_trades(
    conn: &Connection,
    f: &ReportFilter,
    limit: usize,
) -> ReadResult<ReportTable> {
    query_closed_with(
        conn,
        f,
        limit,
        &[
            MINI_ENTRY_VOLUME_RATE_COLUMN,
            NOTIFY_PROFIT_NATIVE_COLUMN,
            NOTIFY_ENTRY_VOLUME_NATIVE_COLUMN,
            NOTIFY_QUOTE_COLUMN,
            // The row's `ReportUID`, which its order traces are filed under: the deal chart draws
            // its lines. NULL from a source that predates the column.
            "reportuid",
        ],
    )
}

/// Closed rows, newest close first, with the display columns plus `extra`.
fn query_closed_with(
    conn: &Connection,
    f: &ReportFilter,
    limit: usize,
    extra: &[&str],
) -> ReadResult<ReportTable> {
    let mut cols = display_columns(conn)?;
    cols.extend(extra.iter().map(|col| (*col).to_string()));
    let closed = ReportFilter {
        rows: RowScope::Closed,
        ..f.clone()
    };
    query_reports_with_columns(conn, &closed, "closedate", true, limit, cols)
}

/// SQL for one of the bot's native-money columns against `src`, or `None` when `col` is not one.
///
/// Args:
///     src: Physical source whose schema decides availability.
///     col: Requested column.
///
/// Returns:
///     The expression; `NULL` when the source lacks an input.
pub(super) fn notify_column_expression(src: &ReadSource, col: &str) -> Option<String> {
    let sql = match col {
        NOTIFY_PROFIT_NATIVE_COLUMN => {
            if src.cols.contains("profitbtc") {
                format!(
                    "CASE WHEN typeof(r.\"profitbtc\") IN ('integer','real') THEN {} END",
                    crate::db::quote::settled_amount_expr("r", &src.cols, "profitbtc")
                )
            } else {
                "NULL".to_string()
            }
        }
        // SQLite resolves every column of a CASE at prepare time, unreachable arms included.
        NOTIFY_ENTRY_VOLUME_NATIVE_COLUMN
            if !(src.cols.contains("boughtq") && src.cols.contains("buyprice")) =>
        {
            "NULL".to_string()
        }
        NOTIFY_ENTRY_VOLUME_NATIVE_COLUMN => {
            // The same proof the Report's traded volume needs: an ordinary closed trade whose
            // prices are in the currency of its money, so quantity × price is a notional in it.
            let volume = traded_volume_sql(src, None);
            format!(
                "CASE WHEN {} THEN ABS(r.\"boughtq\" * r.\"buyprice\") END",
                volume.reconstructed
            )
        }
        NOTIFY_QUOTE_COLUMN => crate::db::quote::effective_ordinal_expr("r", &src.cols),
        _ => return None,
    };
    Some(sql)
}

/// Execute the shared row reader with a caller-specific column projection.
fn query_reports_with_columns(
    conn: &Connection,
    f: &ReportFilter,
    sort_key: &str,
    desc: bool,
    limit: usize,
    cols: Vec<String>,
) -> ReadResult<ReportTable> {
    let meta = report_strategy_meta(conn, f)
        .map_err(|error| read_fail("reports: resolve strategy mask", error))?;
    let col = sort_column(&cols, sort_key);
    let sources = read_sources_res(conn)?;
    // The column set is deliberately resolved ONCE, outside the retry: both attempts must project
    // the same `cols`, or a cache-free retry would desynchronise `cols` from `rows`.
    let merged = {
        let pass = ReportPass {
            filter: f,
            cols: &cols,
            sort_col: &col,
            desc,
            limit,
            sources: &sources,
            meta: &meta,
        };
        with_valuation_fallback(
            conn,
            "отчёты: query_reports",
            "отчёты: query_reports native retry",
            |include_valuation| query_reports_attempt(conn, &pass, include_valuation),
        )?
    };

    let mut rows = Vec::with_capacity(merged.len());
    let mut core_uids = Vec::with_capacity(merged.len());
    let mut rec_ids = Vec::with_capacity(merged.len());
    for (uid, rec_id, row) in merged {
        core_uids.push(uid);
        rec_ids.push(rec_id);
        rows.push(row);
    }
    Ok(ReportTable {
        cols,
        rows,
        core_uids,
        rec_ids,
    })
}
