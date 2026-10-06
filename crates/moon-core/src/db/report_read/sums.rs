//! sums items for report reads.

use super::*;

/// SQL projecting the rec id the soft-delete protocol addresses a row by.
///
/// `newrecid` is a real column only on the typed replica; a legacy source projects `0`, which
/// marks its rows as not soft-deletable — `0` is never a real rec id. Both the Report table and
/// the strategy purge read go through here, so a source that gains or loses the column cannot end
/// up soft-deletable in one reader and not the other.
pub(super) fn rec_id_expr(src: &ReadSource) -> &'static str {
    if src.cols.contains("newrecid") {
        "r.newrecid"
    } else {
        "0"
    }
}

/// SQL projecting the identity a chart trade is ADDRESSED by, for one source.
///
/// The typed replica's `newrecid` where it has one, and the source's own row id where it does not —
/// which is not a detail: a typed row whose `newrecid` is still zero is handed out under `id`, and a
/// reader that looked it up by `newrecid` would find a DIFFERENT trade wearing that number.
///
/// One definition, because two readers agreeing on what a record id means is the whole point:
/// `query_chart_trade_history` MINTS these ids and `crate::db::trade_meta::query_trade_meta` resolves
/// them back.
///
/// Args:
///     src: The source the expression is built against.
///
/// Returns:
///     A SQL expression over the alias `r`.
pub(in crate::db) fn record_identity_expr(src: &ReadSource) -> String {
    let fallback_id = if src.cols.contains("id") {
        "r.id"
    } else if src.legacy && src.cols.contains("db_id") {
        "r.db_id"
    } else {
        "0"
    };
    format!(
        "COALESCE(NULLIF({}, 0), {fallback_id}, 0)",
        rec_id_expr(src)
    )
}

/// Rows of one strategy that a report purge can address, plus the ones it cannot.
pub struct StrategyPurgeRows {
    /// Soft-deletable `newrecid`s from the typed replica.
    pub rec_ids: Vec<i64>,
    /// Rows matching the same strategy in a legacy source, which carries no rec id and therefore
    /// cannot be addressed by the protocol. Counted so the confirmation can say so; never deleted.
    pub legacy_rows: i64,
}

/// Collect every soft-deletable row of one strategy, across the strategy's whole report history.
///
/// The scope is deliberately NOT the Analytics period: deleting only in-period trades would strand
/// rows attributed to a strategy the user can no longer find in the table to clean up. Only closed
/// trades are addressed, matching the closed-trade universe the Analytics row counts — an open
/// trade is still in flight and is not the caller's to hide.
///
/// Strategy matching goes through `build_where`'s exact-key predicate, which resolves the
/// EFFECTIVE strategy id (`effective_sid_expr`): liquidation rows physically carry `strategyid = 0`
/// and are attributed by name, and the Analytics row already counts them. Matching the raw column
/// instead would leave those rows behind, so the strategy would keep a non-zero trade count after a
/// "complete" purge.
///
/// Args:
///     conn: Open report reader; `strat` is expected to be attached for liquidation attribution.
///     key: Exact strategy identity to purge.
///
/// Returns:
///     Addressable rec ids and the count of unaddressable legacy rows.
///
/// Errors:
///     Returns `Failed` for source discovery, SQL, or row conversion errors. A read failure is
///     never collapsed into an empty result — an empty purge and an unreadable one must not look
///     the same to a confirmation dialog.
pub fn strategy_purge_rows(
    conn: &Connection,
    key: ReportStrategyKey,
) -> ReadResult<StrategyPurgeRows> {
    const CTX: &str = "reports: strategy_purge_rows";
    let meta = StrategyMeta::without_mask(crate::db::analytics::strategies_attached(conn));
    let filter = ReportFilter {
        strategies: Some(vec![key]),
        rows: RowScope::Closed,
        ..ReportFilter::default()
    };

    let mut out = StrategyPurgeRows {
        rec_ids: Vec::new(),
        legacy_rows: 0,
    };
    for src in read_sources_res(conn)? {
        // `build_where` already turns a source without the identity columns into a no-match
        // constraint, so a partial schema contributes nothing instead of failing to prepare.
        let (mut where_sql, params) = build_where(&filter, &src.cols, &meta);
        // A narrowing clause the strategy predicate already implies, added so the index can serve
        // it. `append_strategy_filter` matches the EFFECTIVE id, whose `CASE` expression is not
        // sargable, leaving only the `core_uid` prefix of `idx_rep_strat` usable — and this read
        // has no `LIMIT`, so that means scanning a core's whole report history. A row can only
        // match the effective id by carrying it raw, or by being an attributed liquidation, which
        // by construction carries `strategyid` 0 or NULL. So this excludes no matching row.
        if src.cols.contains("strategyid") {
            where_sql.push_str(&format!(
                " AND (COALESCE(r.strategyid, 0) = {sid} OR COALESCE(r.strategyid, 0) = 0)",
                sid = key.strategy_id
            ));
        }
        let rec_id = rec_id_expr(&src);
        let sql = format!("SELECT {rec_id} FROM {} r{where_sql}", src.table);
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
        let rows = stmt
            .query_map(refs.as_slice(), |row| row.get::<_, i64>(0))
            .map_err(|e| read_fail(CTX, e))?;
        for row in rows {
            match row.map_err(|e| read_fail(CTX, e))? {
                0 => out.legacy_rows += 1,
                rec_id => out.rec_ids.push(rec_id),
            }
        }
    }
    Ok(out)
}

/// Aggregate profit, order count, and two-sided traded volume over the complete filter, not only
/// the top N.
///
/// Returns `Failed` when source discovery or any aggregate query fails; only a
/// successful empty result returns an empty breakdown. The open connection means this
/// function cannot return `NotReady`.
///
/// Args:
///     conn: Open report reader or snapshot.
///     f: Complete Report filter.
///
/// Returns:
///     Exact known-currency profit buckets over CLOSED rows, unknown and complete row counts,
///     closed non-Funding traded volume with its per-quote reconstruction counts, counted
///     entry-spend subtotals for [`QuoteBreakdown::average_order_return`], optional complete
///     active-mode USDT coverage, and the still-running positions counted separately beside them.
///
/// Errors:
///     Returns `Failed` for source, SQL, or row conversion errors.
pub fn query_totals(conn: &Connection, f: &ReportFilter) -> ReadResult<ReportTotals> {
    let meta = report_strategy_meta(conn, f)
        .map_err(|error| read_fail("reports: resolve strategy mask", error))?;
    let sources = read_sources_res(conn)?;
    with_valuation_fallback(
        conn,
        "reports: query_totals",
        "reports: query_totals native retry",
        |include_valuation| query_totals_attempt(conn, f, &sources, &meta, include_valuation),
    )
}

/// Run one read that may touch the derived valuation cache, retrying without it when it is corrupt.
///
/// The derived cache is disposable; the report replica is not. A corrupt `valuation.sqlite` must
/// therefore cost the USDT columns and nothing else — never the rows, and never the export that
/// re-runs the same read. Both Report reads that join it share this one dance so neither can drift
/// into failing closed on a cache the user never asked for.
///
/// Args:
///     conn: Open report reader or snapshot.
///     ctx: Log context for a failure the derived cache cannot explain.
///     retry_ctx: Log context for a failure of the cache-free retry. Separate rather than derived
///         because `read_fail` classifies against a `&'static str`, which no runtime concatenation
///         can produce.
///     attempt: One complete pass, told whether the valuation cache may be joined.
///
/// Returns:
///     The result of whichever pass succeeded.
///
/// Errors:
///     Returns `Failed` when the first pass fails for any reason other than proven derived
///     corruption, and when the cache-free retry fails in turn.
pub(super) fn with_valuation_fallback<T>(
    conn: &Connection,
    ctx: &'static str,
    retry_ctx: &'static str,
    attempt: impl Fn(bool) -> rusqlite::Result<T>,
) -> ReadResult<T> {
    let attached = crate::db::valuation::is_attached(conn);
    match attempt(attached) {
        Ok(value) => Ok(value),
        Err(error) if attached && crate::db::valuation::prove_derived_corruption(conn, &error) => {
            let _ = conn.execute(
                &format!("DETACH DATABASE {}", crate::db::valuation::SCHEMA),
                [],
            );
            attempt(false).map_err(|retry_error| read_fail(retry_ctx, retry_error))
        }
        // The guard above already performed schema attribution for this exact error.
        Err(error) => Err(read_fail(ctx, error)),
    }
}

/// Classify one discovered report source into its valuation partition.
///
/// Args:
///     src: Physical report source.
///
/// Returns:
///     The partition the valuation cache keys its rows by.
pub(in crate::db) fn source_partition(src: &ReadSource) -> crate::db::valuation::TradeSource {
    if src.legacy {
        crate::db::valuation::TradeSource::Legacy
    } else {
        crate::db::valuation::TradeSource::Typed
    }
}

/// Per-source SQL for two-sided Report volume over provable rows only.
pub(super) struct TradedVolumeSql {
    /// Closed non-Funding row predicate, independent of the Report's profit/count scope.
    eligible: String,
    /// Eligible row whose native entry and exit notionals are dimensionally trustworthy.
    pub(super) reconstructed: String,
    /// Unsigned native entry-plus-exit notional for a reconstructed row.
    native: String,
    /// Active-mode USDT rate, or SQL NULL when no valuation projection exists.
    pub(super) rate: String,
}

impl TradedVolumeSql {
    /// Describe the five grouped columns consumed by [`crate::db::TradedVolume::from_groups`].
    ///
    /// Returns:
    ///     Eligible/reconstructed counts, native sum, valued count, and USDT sum in that order.
    pub(super) fn sum_columns(&self) -> Vec<SumColumn> {
        let (eligible, reconstructed, native, rate) = (
            &self.eligible,
            &self.reconstructed,
            &self.native,
            &self.rate,
        );
        vec![
            SumColumn::sum(
                format!("CASE WHEN {eligible} THEN 1 ELSE 0 END"),
                SumZero::Integer,
            ),
            SumColumn::sum(
                format!("CASE WHEN {reconstructed} THEN 1 ELSE 0 END"),
                SumZero::Integer,
            ),
            SumColumn::sum(
                format!("CASE WHEN {reconstructed} THEN {native} ELSE 0.0 END"),
                SumZero::Real,
            ),
            SumColumn::sum(
                format!("CASE WHEN {reconstructed} AND ({rate}) IS NOT NULL THEN 1 ELSE 0 END"),
                SumZero::Integer,
            ),
            SumColumn::sum(
                format!(
                    "CASE WHEN {reconstructed} AND ({rate}) IS NOT NULL                      THEN ({native}) * ({rate}) ELSE 0.0 END"
                ),
                SumZero::Real,
            ),
        ]
    }
}

/// Build the settled-profit aggregate for one source, or a literal zero when it cannot carry one.
///
/// Shared by the closed and open totals passes: what differs between them is WHICH rows the
/// `WHERE` admits, never how their money is summed, so the sum is written once.
///
/// Args:
///     src: Physical Report source and its discovered columns.
///
/// Returns:
///     A sum over the settled amount, or the literal `0.0` on a source without `profitbtc`.
pub(super) fn profit_column(src: &ReadSource) -> SumColumn {
    if src.cols.contains("profitbtc") {
        SumColumn::sum(
            crate::db::quote::settled_amount_expr("r", &src.cols, "profitbtc"),
            SumZero::Real,
        )
    } else {
        SumColumn::ZeroReal
    }
}

/// Build two-sided volume SQL without changing the Report's row/profit filter.
///
/// Args:
///     src: Physical Report source and its discovered columns.
///     rate: Active-mode quote-to-USDT expression when a projection is available.
///
/// Returns:
///     Fail-closed eligibility, reconstruction, native-notional, and valuation expressions.
pub(super) fn traded_volume_sql(src: &ReadSource, rate: Option<&str>) -> TradedVolumeSql {
    let has = |column: &str| src.cols.contains(column);
    // Volume eligibility IS the closed-row test plus the Funding exclusion, and it reads through
    // the shared predicate so a row can never count as closed for profit and open for volume.
    let eligible = match closed_row_predicate(&src.cols) {
        Some(closed) => {
            let funding = if has("sellreason") {
                " AND COALESCE(r.\"sellreason\", '') <> 'Funding'"
            } else {
                ""
            };
            format!("({closed}{funding})")
        }
        None => "0".to_string(),
    };
    let has_price_legs = has("boughtq") && has("buyprice") && has("sellprice");
    let native = if has_price_legs {
        "(ABS(r.\"boughtq\" * r.\"buyprice\") + ABS(r.\"boughtq\" * r.\"sellprice\"))".to_string()
    } else {
        "0.0".to_string()
    };
    let inputs = if has_price_legs {
        "typeof(r.\"boughtq\") IN ('integer','real') AND r.\"boughtq\" > 0
         AND typeof(r.\"buyprice\") IN ('integer','real') AND r.\"buyprice\" > 0
         AND typeof(r.\"sellprice\") IN ('integer','real') AND r.\"sellprice\" > 0"
            .to_string()
    } else {
        "0".to_string()
    };
    let ordinary = if has("sellreason") {
        "typeof(r.\"sellreason\")='text'
         AND TRIM(r.\"sellreason\") <> ''
         AND UPPER(r.\"sellreason\") <> 'LIQUIDATION'"
    } else {
        // Without the reason, a closed row cannot prove that it is a trade rather than Funding.
        "0"
    };
    let quote_matches = crate::db::quote::prices_share_money_quote_expr("r", &src.cols);
    TradedVolumeSql {
        reconstructed: format!("({eligible} AND {inputs} AND {ordinary} AND ({quote_matches}))"),
        eligible,
        native,
        rate: rate.unwrap_or("NULL").to_string(),
    }
}

/// Per-source SQL for the counted spend/profit subtotal, plus its own unified USDT leg, behind
/// [`QuoteBreakdown::average_order_return`], independent of the Report's row/profit filter —
/// exactly like [`TradedVolumeSql`] beside it, and for the same reason: `rate` is taken so the
/// unified leg is this feature's OWN, never [`crate::db::UsdtTotal::spent`], which carries no
/// positive-spend guard and no Funding exclusion.
pub(super) struct EntrySpendSql {
    /// Counted-row predicate: positive numeric settled spend, numeric settled profit, and
    /// non-Funding.
    counted: String,
    /// The row's settled spend, or SQL NULL when the source cannot evidence it.
    spent: String,
    /// The row's settled profit, or `0.0` when the source cannot evidence it.
    profit: String,
    /// Active-mode USDT rate, or SQL NULL when no valuation projection exists.
    rate: String,
}

impl EntrySpendSql {
    /// Describe the six grouped columns consumed by [`crate::db::EntrySpend::from_groups`].
    ///
    /// Returns:
    ///     Counted-row count, summed settled spend, summed settled profit, valued-row count, and
    ///     the summed USDT spend and profit over counted rows carrying a rate, in that order.
    pub(super) fn sum_columns(&self) -> Vec<SumColumn> {
        let (counted, spent, profit, rate) = (&self.counted, &self.spent, &self.profit, &self.rate);
        let rated = format!("{counted} AND ({rate}) IS NOT NULL");
        vec![
            SumColumn::sum(
                format!("CASE WHEN {counted} THEN 1 ELSE 0 END"),
                SumZero::Integer,
            ),
            SumColumn::sum(
                format!("CASE WHEN {counted} THEN {spent} ELSE 0.0 END"),
                SumZero::Real,
            ),
            SumColumn::sum(
                format!("CASE WHEN {counted} THEN {profit} ELSE 0.0 END"),
                SumZero::Real,
            ),
            SumColumn::sum(
                format!("CASE WHEN {rated} THEN 1 ELSE 0 END"),
                SumZero::Integer,
            ),
            SumColumn::sum(
                format!("CASE WHEN {rated} THEN ({spent}) * ({rate}) ELSE 0.0 END"),
                SumZero::Real,
            ),
            SumColumn::sum(
                format!("CASE WHEN {rated} THEN ({profit}) * ({rate}) ELSE 0.0 END"),
                SumZero::Real,
            ),
        ]
    }
}

/// Build the entry-spend SQL for one source.
///
/// A counted row is CLOSED (the caller already restricts the scope), non-Funding, with a positive
/// numeric settled spend and a numeric settled profit. It takes the POSITIVE-SPEND half from the
/// house average-order definition (`analytics::groups::avg_order`,
/// `analytics::profit_monitor::average_order`) and ADDS the Funding exclusion on top — the two are
/// deliberately NOT identical, because neither Analytics query filters `sellreason`, so a
/// positive-spend Funding row moves their averages and not this one. Do not "restore parity" in
/// either direction without deciding which surface is wrong. The numeric-profit and settled-spend
/// legs reuse [`crate::db::valuation::source_predicates`] so this cannot silently disagree with the
/// valuation cache about which rows are eligible.
///
/// On a source that cannot express `closedate` the realized pass widens to `ClosedAndOpen`, and
/// this leg inherits that exactly as the PROFIT total does rather than failing closed the way the
/// neighbouring volume leg does. That asymmetry is deliberate twice over: the percentage is a
/// ratio to the profit figure in the row's head, so a denominator that excluded rows the
/// numerator kept would state a ratio between two different scopes — the exact defect plan review
/// caught in the unified arm — and such a source is in practice a legacy ARCHIVE table of
/// already-closed trades, not a live one carrying open positions.
///
/// Args:
///     src: Physical Report source and its discovered columns.
///     rate: Active-mode quote-to-USDT expression when a projection is available, taken the same
///         way [`traded_volume_sql`] takes it.
///
/// Returns:
///     Fail-closed counted predicate, settled spend/profit expressions naming only columns the
///     source actually has, and the valuation rate expression.
pub(super) fn entry_spend_sql(src: &ReadSource, rate: Option<&str>) -> EntrySpendSql {
    let has = |column: &str| src.cols.contains(column);
    let predicates = crate::db::valuation::source_predicates("r", &src.cols);
    // Without `sellreason` a closed row cannot be proven NOT Funding, so it counts nothing — the
    // same fail-closed direction `traded_volume_sql` takes rather than risking a Funding row
    // inflating the average.
    let funding = if has("sellreason") {
        "COALESCE(r.\"sellreason\", '') <> 'Funding'".to_string()
    } else {
        "0".to_string()
    };
    // `> 0` on a NULL spend is NULL in SQLite, so a non-numeric or absent spend excludes itself
    // without a second `typeof` test.
    let counted = format!(
        "(({spent}) > 0 AND {numeric_profit} AND {funding})",
        spent = predicates.spent_value,
        numeric_profit = predicates.numeric_profit,
    );
    EntrySpendSql {
        counted,
        spent: predicates.spent_value,
        profit: if has("profitbtc") {
            crate::db::quote::settled_amount_expr("r", &src.cols, "profitbtc")
        } else {
            "0.0".to_string()
        },
        rate: rate.unwrap_or("NULL").to_string(),
    }
}
