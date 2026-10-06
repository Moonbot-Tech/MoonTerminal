//! chart items for report reads.

use super::*;

/// Chart trade history needs `strat` for name masks and liquidation attribution
/// and never references the `valuation` schema, so it does not pay for it.
pub const CHART_TRADE_HISTORY_ATTACH: crate::db::AttachSet = crate::db::AttachSet::STRATEGIES_ONLY;

/// Read a bounded newest-first closed-trade history for one exact chart market on an explicit
/// core set.
///
/// `core_uids` is the chart's own core first, then every other core the caller admitted. An empty
/// slice means no core and is mapped to [`crate::config::NO_MATCH_CORE_UID`], not to every core:
/// [`ReportFilter::core_uids`] empty means the whole fleet, so a present-but-empty set must name
/// the sentinel or the read widens.
///
/// The caller may provide a published Report filter to retain its date, side, emulator, deletion,
/// and strategy predicates. This boundary always overwrites the core, substring coin, exact coin,
/// and row-scope fields so a stale or global filter cannot widen the chart scope. The row scope in
/// particular must keep being overwritten now that a published Report filter admits open positions
/// by default: chart history draws closed trades and nothing else.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     core_uids: Explicit runtime cores, the chart's own core first. Empty matches nothing.
///     exact_coins: Case-insensitive stored coin identities accepted for the canonical market.
///     filter: Optional published Report scope; `None` selects all durable closed trades.
///     limit: Maximum returned records; one additional row detects truncation.
///
/// Returns:
///     Parsed chart records and whether older matches were truncated.
///
/// Errors:
///     Propagates replica readiness, schema, SQL, and row-conversion failures.
pub fn query_chart_trade_history_for_cores(
    conn: &Connection,
    core_uids: &[u64],
    exact_coins: &[String],
    filter: Option<&ReportFilter>,
    limit: usize,
) -> ReadResult<ChartTradeHistory> {
    const CONTEXT: &str = "reports: chart trade history";
    const REQUIRED_COLUMNS: &[&str] = &[
        "core_uid",
        "coin",
        "buydate",
        "closedate",
        "buyprice",
        "sellprice",
        "quantity",
        "isshort",
    ];
    let mut scope = filter.cloned().unwrap_or_default();
    scope.core_uids = if core_uids.is_empty() {
        vec![crate::config::NO_MATCH_CORE_UID]
    } else {
        core_uids.to_vec()
    };
    scope.coin.clear();
    scope.exact_coins = Some(exact_coins.to_vec());
    scope.rows = RowScope::Closed;

    let requested = limit.saturating_add(1);
    let meta = report_strategy_meta(conn, &scope).map_err(|error| read_fail(CONTEXT, error))?;
    let mut compatible_source = false;
    let mut records = Vec::new();
    for source in read_sources_res(conn)? {
        if !REQUIRED_COLUMNS
            .iter()
            .all(|column| source.cols.contains(*column))
        {
            continue;
        }
        compatible_source = true;
        let (where_sql, mut params) = build_where(&scope, &source.cols, &meta);
        let record_id = record_identity_expr(&source);
        // Money is OPTIONAL here, deliberately: `REQUIRED_COLUMNS` names none of these columns, so
        // a source that cannot produce a figure still returns every trade and the chart still draws
        // it. Both legs go through `settled_amount_expr` — the same correction the Report grid and
        // its footer apply — or a COIN-M liquidation would be off by its own entry price.
        let (profit_sql, percent_sql) = if source.cols.contains("profitbtc") {
            let profit = crate::db::quote::settled_amount_expr("r", &source.cols, "profitbtc");
            let percent = if source.cols.contains("spentbtc") {
                let spent = crate::db::quote::settled_amount_expr("r", &source.cols, "spentbtc");
                // Settled over settled, so the ratio is unit-free even where the two would have
                // needed different corrections. Same definition as the Report's percent column.
                format!("CASE WHEN {spent} > 0 THEN {profit} / {spent} * 100.0 END")
            } else {
                "NULL".to_string()
            };
            (profit, percent)
        } else {
            ("NULL".to_string(), "NULL".to_string())
        };
        // The currency the amount above is IN. Not derivable from the coin — a COIN-M row spells
        // its coin like a USD-M one while settling in BTC — so it travels with the amount.
        let quote_sql = crate::db::quote::effective_ordinal_expr("r", &source.cols);
        // OPTIONAL exactly like the money columns above, and for the same reason: `REQUIRED_COLUMNS`
        // does not name it, so a source predating the column must still return every trade. It falls
        // back to 0 = REAL, which is the recoverable direction — hiding real trades on old data
        // would not be.
        let emulator_sql = if source.cols.contains("emulator") {
            "COALESCE(r.emulator, 0)"
        } else {
            "0"
        };
        // OPTIONAL exactly like `emulator` above: a replica whose table predates these columns, and the
        // legacy `closed_sell_reports` source which never has them, project NULL and fall back to the
        // seconds columns. `REQUIRED_COLUMNS` must NOT name them, or such a source would return no
        // trades at all.
        let buy_ms_sql = if source.cols.contains("buydatems") {
            "r.buydatems"
        } else {
            "NULL"
        };
        let close_ms_sql = if source.cols.contains("closedatems") {
            "r.closedatems"
        } else {
            "NULL"
        };
        let report_uid_sql = if source.cols.contains("reportuid") {
            "r.reportuid"
        } else {
            "NULL"
        };
        // The exit order's creation, optional the same way: a source without it reads as `0`, and
        // the record then dates the exit's placement at the entry.
        let sell_set_sql = if source.cols.contains("sellsetdate") {
            "r.sellsetdate"
        } else {
            "0"
        };
        let sell_set_ms_sql = if source.cols.contains("sellsetdatems") {
            "r.sellsetdatems"
        } else {
            "NULL"
        };
        // The entry order's creation and its saved corridor, optional the same way: cores file
        // them since 2026-09-21, nothing backfills them, and a source without them reads NULL.
        let optional = |column: &'static str| {
            if source.cols.contains(column) {
                format!("r.{column}")
            } else {
                "NULL".to_string()
            }
        };
        let buy_set_ms_sql = optional("buysetdatems");
        let corridor_down_sql = optional("buycorridordown");
        let corridor_up_sql = optional("buycorridorup");
        let sql = format!(
            "SELECT {record_id}, r.core_uid, r.coin, r.buydate, r.closedate, \
             r.buyprice, r.sellprice, r.quantity, r.isshort, \
             {profit_sql}, {quote_sql}, {percent_sql}, {emulator_sql}, \
             {buy_ms_sql}, {close_ms_sql}, {report_uid_sql}, {sell_set_sql}, {sell_set_ms_sql}, \
             {buy_set_ms_sql}, {corridor_down_sql}, {corridor_up_sql} \
             FROM {} r{where_sql} \
             ORDER BY r.closedate DESC, {record_id} DESC LIMIT ?",
            source.table
        );
        params.push(Box::new(requested as i64));
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        let mut statement = conn
            .prepare(&sql)
            .map_err(|error| read_fail(CONTEXT, error))?;
        let rows = statement
            .query_map(refs.as_slice(), |row| {
                Ok((
                    row.get::<_, Value>(0)?,
                    row.get::<_, Value>(1)?,
                    row.get::<_, Value>(2)?,
                    row.get::<_, Value>(3)?,
                    row.get::<_, Value>(4)?,
                    row.get::<_, Value>(5)?,
                    row.get::<_, Value>(6)?,
                    row.get::<_, Value>(7)?,
                    row.get::<_, Value>(8)?,
                    row.get::<_, Value>(9)?,
                    row.get::<_, Value>(10)?,
                    row.get::<_, Value>(11)?,
                    row.get::<_, Value>(12)?,
                    row.get::<_, Value>(13)?,
                    row.get::<_, Value>(14)?,
                    row.get::<_, Value>(15)?,
                    row.get::<_, Value>(16)?,
                    row.get::<_, Value>(17)?,
                    (
                        row.get::<_, Value>(18)?,
                        row.get::<_, Value>(19)?,
                        row.get::<_, Value>(20)?,
                    ),
                ))
            })
            .map_err(|error| read_fail(CONTEXT, error))?;
        for row in rows {
            let (
                record_id,
                row_core,
                coin,
                buy_date,
                close_date,
                buy_price,
                sell_price,
                quantity,
                is_short,
                profit,
                quote,
                profit_percent,
                emulator,
                buy_ms,
                close_ms,
                report_uid,
                sell_set_date,
                sell_set_ms,
                (buy_set_ms, corridor_down, corridor_up),
            ) = row.map_err(|error| read_fail(CONTEXT, error))?;
            let Some(buy_date) = report_value_i64(&buy_date) else {
                continue;
            };
            let Some(close_date) = report_value_i64(&close_date) else {
                continue;
            };
            let Some(buy_price) = report_value_f64(&buy_price) else {
                continue;
            };
            let Some(sell_price) = report_value_f64(&sell_price) else {
                continue;
            };
            if buy_date <= 0
                || close_date <= 0
                || !buy_price.is_finite()
                || buy_price <= 0.0
                || !sell_price.is_finite()
                || sell_price <= 0.0
            {
                continue;
            }
            records.push(ChartTradeRecord {
                record_id: report_value_i64(&record_id).unwrap_or_default(),
                core_uid: report_value_i64(&row_core).unwrap_or_default() as u64,
                coin: report_value_text(&coin).unwrap_or_default(),
                buy_date,
                close_date,
                // `report_value_i64` maps NULL to `None`, which is exactly the absence the wire means.
                buy_ms: report_value_i64(&buy_ms),
                close_ms: report_value_i64(&close_ms),
                sell_set_date: report_value_i64(&sell_set_date).unwrap_or_default(),
                sell_set_ms: report_value_i64(&sell_set_ms),
                // Zero is the column's "unavailable", never an epoch date or a zero price.
                buy_set_ms: report_value_i64(&buy_set_ms).filter(|ms| *ms > 0),
                corridor: match (
                    report_value_f64(&corridor_down),
                    report_value_f64(&corridor_up),
                ) {
                    (Some(down), Some(up)) if down > 0.0 && up > 0.0 => Some((down, up)),
                    _ => None,
                },
                buy_price,
                sell_price,
                quantity: report_value_f64(&quantity).unwrap_or_default(),
                is_short: report_value_i64(&is_short).unwrap_or_default() != 0,
                // An unreadable or absent flag means REAL, matching the `0` fallback in the SELECT.
                emulator: report_value_i64(&emulator).unwrap_or_default() != 0,
                // `report_value_f64` already rejects a non-finite cell, which keeps the derived
                // `PartialEq` on this record meaningful: a NaN here would make two identical
                // histories compare unequal forever and republish on every poll.
                profit: report_value_f64(&profit),
                quote: QuoteCurrency::from_report_value(&quote),
                profit_pct: report_value_f64(&profit_percent),
                // NULL is `None`, an absent column is NULL, and so is a stored ZERO: the replica
                // writes 0 for a row received before the core reported the column, and the core
                // never issues 0 as an identity. All three mean "not askable".
                report_uid: report_value_i64(&report_uid).filter(|uid| *uid != 0),
            });
        }
    }
    if !compatible_source {
        return Err(crate::db::ReadFail::NotReady);
    }
    records.sort_by(|left, right| {
        right
            .close_date
            .cmp(&left.close_date)
            .then_with(|| right.record_id.cmp(&left.record_id))
            .then_with(|| right.buy_date.cmp(&left.buy_date))
            .then_with(|| right.coin.cmp(&left.coin))
    });
    let truncated = records.len() > limit;
    records.truncate(limit);
    Ok(ChartTradeHistory { records, truncated })
}

/// Single-core form of [`query_chart_trade_history_for_cores`]: one chart core, not a set.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     core_uid: Exact runtime core that owns the chart.
///     exact_coins: Case-insensitive stored coin identities accepted for the canonical market.
///     filter: Optional published Report scope; `None` selects all durable closed trades.
///     limit: Maximum returned records; one additional row detects truncation.
///
/// Returns:
///     Parsed chart records and whether older matches were truncated.
///
/// Errors:
///     Propagates replica readiness, schema, SQL, and row-conversion failures.
pub fn query_chart_trade_history(
    conn: &Connection,
    core_uid: u64,
    exact_coins: &[String],
    filter: Option<&ReportFilter>,
    limit: usize,
) -> ReadResult<ChartTradeHistory> {
    query_chart_trade_history_for_cores(conn, &[core_uid], exact_coins, filter, limit)
}
