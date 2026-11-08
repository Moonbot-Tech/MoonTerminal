//! One streamed pass over the closed trades a Report filter selects.

use std::cell::RefCell;

use super::*;

/// The name each archived strategy carries now, keyed by stored core uid and strategy id.
///
/// Read from the strategy database attached to the report reader, so it shares the trade pass's
/// snapshot. Empty when no strategy database is attached: names then fall back to what the trade
/// rows themselves recorded.
///
/// Errors:
///     A strategy database that is attached but fails to answer.
pub fn archive_strategy_names(conn: &Connection) -> ReadResult<HashMap<(u64, i64), String>> {
    if !crate::db::analytics::strategies_attached(conn) {
        return Ok(HashMap::new());
    }
    let ctx = "trade pass: archived strategy names";
    let mut stmt = conn
        .prepare("SELECT core_uid, strategy_id, name FROM strat.strategies WHERE name IS NOT NULL")
        .map_err(|error| read_fail(ctx, error))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                (row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)?),
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| read_fail(ctx, error))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|error| read_fail(ctx, error))
}

/// Visit every closed row the filter selects, in no particular order, without collecting them.
///
/// The rows are projected exactly as the Report table projects them — settled money, the active
/// USDT valuation, corrected currency and core names — so a figure computed from this pass agrees
/// with the Report and the Analytics window on the same rows. Unlike [`query_reports`] it applies
/// no sort and no limit: an aggregate over a year of trades must not hold that year as rows.
///
/// Each visit receives the core, the recorder id (0 on a legacy row), the row's effective strategy
/// id (a liquidation attributed to its owner where the strategy database can tell) and the cells in
/// `cols` order. A visit returning `false` ends the pass early.
///
/// The pass may run twice: when the derived valuation cache proves corrupt it is detached and the
/// pass reruns without it. `reset` is called before every attempt, so whatever the visits built
/// from a failed attempt is dropped rather than counted twice.
///
/// Args:
///     conn: Open report reader or snapshot.
///     f: Complete Report filter; its row scope is forced to closed rows.
///     cols: Report columns to project, display names or synthetic columns.
///     visit: Called once per row.
///     reset: Called before each attempt.
///
/// Errors:
///     The same `ReadFail` contract as [`query_reports`].
pub fn scan_closed_trades(
    conn: &Connection,
    f: &ReportFilter,
    cols: &[String],
    visit: &mut dyn FnMut(u64, i64, i64, &[Value]) -> bool,
    reset: &mut dyn FnMut(),
) -> ReadResult<()> {
    let _money = super::super::pin_money_read(conn);
    let closed = ReportFilter {
        rows: RowScope::Closed,
        ..f.clone()
    };
    let meta = report_strategy_meta(conn, &closed)
        .map_err(|error| read_fail("trade pass: resolve strategy mask", error))?;
    let sources = read_sources_res(conn)?;
    let margin = if cols.iter().any(|c| c == PROFIT_ABS_COLUMN) {
        crate::db::analytics::basis::margin_cores(conn)?
    } else {
        std::collections::HashSet::new()
    };
    let attribution = crate::db::analytics::strategies_attached(conn);
    let visit = RefCell::new(visit);
    let reset = RefCell::new(reset);
    with_valuation_fallback(
        conn,
        "trade pass",
        "trade pass native retry",
        |include_valuation| {
            (reset.borrow_mut())();
            for src in &sources {
                let (where_sql, params) = build_where(&closed, &src.cols, &meta);
                let valuation = crate::db::valuation::projection(
                    closed.valuation,
                    include_valuation,
                    "r",
                    &src.cols,
                    source_partition(src),
                );
                let joins = valuation
                    .as_ref()
                    .map(|parts| parts.per_row.joins.as_str())
                    .unwrap_or("");
                // A tuner field the core writes into the comment has no column of its own; it is
                // parsed here exactly as the tuner parses it, or it would read as NULL.
                let select = cols
                    .iter()
                    .map(|col| {
                        match crate::db::tuner::comment_field::derived_sql(col, "r", &src.cols) {
                            Some(sql) => format!("{sql} AS \"{col}\""),
                            None => source_select(
                                src,
                                std::slice::from_ref(col),
                                valuation.as_ref(),
                                &closed.core_names,
                                &margin,
                            ),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let sid = if src.cols.contains("strategyid") {
                    crate::db::analytics::effective_sid_expr("r", &src.cols, attribution)
                } else {
                    "NULL".to_string()
                };
                let sql = format!(
                    "SELECT r.core_uid, {}, CAST(COALESCE({sid}, 0) AS INTEGER), {select} FROM {} r{joins}{where_sql}",
                    rec_id_expr(src),
                    src.table
                );
                let refs: Vec<&dyn rusqlite::types::ToSql> =
                    params.iter().map(|b| b.as_ref()).collect();
                let mut stmt = conn.prepare(&sql)?;
                let mut rows = stmt.query(refs.as_slice())?;
                let mut cells = Vec::with_capacity(cols.len());
                while let Some(row) = rows.next()? {
                    cells.clear();
                    for index in 0..cols.len() {
                        cells.push(row.get::<_, Value>(index + 3)?);
                    }
                    let core = row.get::<_, i64>(0)? as u64;
                    if !(visit.borrow_mut())(core, row.get(1)?, row.get(2)?, &cells) {
                        return Ok(());
                    }
                }
            }
            Ok(())
        },
    )
}
