//! Read layer for the Reports window: filters, source projection, sort/merge, and aggregates.

use rusqlite::types::Value;
use rusqlite::Connection;

use super::read_fail::read_fail;
use super::rep;
use super::valuation::ValuationMode;
use super::{read_sources_res, table_columns_res, QuoteBreakdown, ReadResult, ReadSource};

/// Columns and ordering displayed in the Reports window; the window owns titles and widths.
///
/// `core_uid` and `newrecid` are hidden service columns. `id` is the shared display key for the
/// typed source's `id` and the legacy source's `db_id`.
pub const DISPLAY_COLUMNS: &[&str] = &[
    "buydate",
    "closedate",
    "sellsetdate",
    "core_name",
    "id",
    "taskid",
    "exorderid",
    "coin",
    "isshort",
    "quantity",
    "boughtq",
    "buyprice",
    "sellprice",
    "spentbtc",
    "gainedbtc",
    "profitbtc",
    "profitpct",
    "valuation_profit_usdt",
    "valuation_rate",
    "valuation_rate_source",
    "lev",
    "strategyid",
    "source",
    "channel",
    "channelname",
    "signaltype",
    "fname",
    "basecurrency",
    "emulator",
    "status",
    "sellreason",
    "comment",
    "btc1hdelta",
    "exchange1hdelta",
    "btc24hdelta",
    "exchange24hdelta",
    "btc5mdelta",
    "bvsvratio",
    "pump1h",
    "dump1h",
    "d24h",
    "d3h",
    "d1h",
    "d15m",
    "d5m",
    "d1m",
    "dbtc1m",
    "vd1m",
    "pricebug",
    "hvol",
    "hvolf",
    "dvol",
    "takeprofitlag",
    "last_update_at",
];

/// Synthetic report column containing per-trade return on positive spent capital.
pub const PROFIT_PERCENT_COLUMN: &str = "profitpct";

/// Synthetic report column carrying one trade's profit converted to USDT.
pub const VALUATION_PROFIT_COLUMN: &str = "valuation_profit_usdt";

/// Synthetic report column carrying the USDT rate applied to one trade.
pub const VALUATION_RATE_COLUMN: &str = "valuation_rate";

/// Synthetic report column naming where that rate came from.
pub const VALUATION_SOURCE_COLUMN: &str = "valuation_rate_source";

/// Report columns introduced after the `v2` visible-column schema.
///
/// A saved visible-column set is explicit, so schema generations that include these columns must
/// restore them when reading an earlier set. Both [`crate::db::load_visible`] and the per-context
/// window-layout migration read this list, which keeps the two persisted stores aligned.
pub const COLUMNS_ADDED_SINCE_V2: &[&str] = &[VALUATION_PROFIT_COLUMN];

/// One Report column computed by a SQL expression rather than read from a source table.
struct Synthetic {
    /// Runtime column key, also used as the raw export header.
    name: &'static str,
    /// Report columns the expression reads. A source missing any of them cannot compute it.
    inputs: &'static [&'static str],
}

/// Every synthetic Report column, and everything that distinguishes one from a stored column.
///
/// One entry per column, and one [`synthetic_expression`] serving BOTH the projection and the
/// `ORDER BY`, so a column cannot ship half-wired. That failure mode is silent and expensive: each
/// physical source is truncated by `LIMIT` before the Rust merge, so a column that could project
/// but not sort would return the wrong global top rows with no error anywhere.
const SYNTHETIC: &[Synthetic] = &[
    Synthetic {
        name: PROFIT_PERCENT_COLUMN,
        inputs: &["profitbtc", "spentbtc"],
    },
    Synthetic {
        name: VALUATION_PROFIT_COLUMN,
        inputs: super::valuation::REQUIRED_TRADE_INPUTS,
    },
    Synthetic {
        name: VALUATION_RATE_COLUMN,
        inputs: super::valuation::REQUIRED_TRADE_INPUTS,
    },
    Synthetic {
        name: VALUATION_SOURCE_COLUMN,
        inputs: super::valuation::REQUIRED_TRADE_INPUTS,
    },
];

/// Look one column up in the synthetic table.
fn synthetic(col: &str) -> Option<&'static Synthetic> {
    SYNTHETIC.iter().find(|entry| entry.name == col)
}

/// Query result containing column names and generic value rows for every column.
///
/// `cols` is a RUNTIME list from `PRAGMA table_info`, so core-added fields appear
/// without code changes: known columns keep canonical order and new ones follow.
pub struct ReportTable {
    pub cols: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// `core_uid` for each row, parallel to `rows`.
    ///
    /// This service column is absent from `cols` and `DISPLAY_COLUMNS`, but lets a
    /// report coin click open the chart ON THE CORE that made the trade
    /// (`core_uid` equals the runtime `CoreId`).
    pub core_uids: Vec<u64>,
    /// `newrecid` (the replica replication key) for each row, parallel to `rows`.
    ///
    /// Also a hidden service column. It is the id the soft-delete protocol addresses, so the
    /// Report panel actions read it to build `set_report_rows_deleted`. Legacy rows, which have no
    /// `newrecid` and cannot be soft-deleted, carry `0` — never a real rec id.
    pub rec_ids: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideFilter {
    #[default]
    All,
    Long,
    Short,
}

/// Which quantity every profit figure in the Analytics window is measured in.
///
/// `Quote` uses raw `profitbtc` only when every contributing row has one known quote.
/// `Percent` measures each trade as `profitbtc / spentbtc * 100` — the exact formula of the
/// MoonBot report's `Profit` column: return on the capital spent, independent of order size.
/// The choice is a per-`Query` lens, applied once in the source projection (see
/// `analytics::unified_from`), so every aggregation and the tuner sweep read the same metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfitMetric {
    /// Absolute money in one exact quote currency.
    #[default]
    Quote,
    /// Return on spent capital in percent — the report's `Profit` column.
    Percent,
}

/// Complete filter shared by Report rows, totals, export, and strategy discovery.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReportFilter {
    /// Selected cores for the multi-select filter; empty means all cores.
    pub core_uids: Vec<u64>,
    pub date_from: Option<i64>,
    pub date_to: Option<i64>,
    pub coin: String,
    pub side: SideFilter,
    /// Emulator orders: `None` selects all, `Some(false)` only real orders, and
    /// `Some(true)` only emulator orders. A NULL column value counts as real.
    pub emulator: Option<bool>,
    /// Soft-deleted trades (the core-supplied `deleted` column): `false` hides them,
    /// `true` shows ONLY them. A NULL column value counts as not deleted, matching
    /// the analytics filter; a source without the column holds no soft-deleted rows.
    pub deleted_only: bool,
    /// Require a positive close timestamp, matching Analytics' closed-trade universe.
    pub closed_only: bool,
    /// Exact strategy identities; `None` selects all strategies, while `Some` remains constrained.
    ///
    /// The core is part of every key because strategy ids repeat across cores. An explicit empty
    /// collection intentionally matches no rows so a lost/stale selection cannot broaden a query.
    pub strategies: Option<Vec<ReportStrategyKey>>,
    /// Which conversion the three USDT columns and the totals row apply.
    ///
    /// Carried on the filter rather than passed as a parameter because rows, totals and export all
    /// already receive this one value: a mode that reached the rows but not the totals would print
    /// a footer that does not sum the column above it.
    pub valuation: ValuationMode,
}

/// Exact report strategy identity across all connected cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReportStrategyKey {
    /// Runtime core identity stored by the report replica.
    pub core_uid: u64,
    /// Delphi-signed strategy id stored in reports and `strategies.sqlite`.
    pub strategy_id: i64,
}

/// Strategy option shown by the Report filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportStrategy {
    /// Exact database identity used by the filter.
    pub key: ReportStrategyKey,
    /// Strategy name from `strategies.sqlite`, or the numeric id when metadata is unavailable.
    pub name: String,
}

/// Build the runtime display-column list from the typed and legacy schemas.
///
/// Known columns follow `DISPLAY_COLUMNS`; extra columns follow alphabetically,
/// service columns are omitted, and legacy `db_id` is exposed as `id`. Schema
/// probe errors map to `Failed`; an absent table contributes no columns. This
/// function receives an open connection and therefore cannot return `NotReady`.
pub fn display_columns(conn: &Connection) -> ReadResult<Vec<String>> {
    const SERVICE: &[&str] = &[
        "core_uid",
        "newrecid",
        "db_id",
        "sql",
        "created_ms",
        "updated_ms",
    ];
    let mut have = rep::table_cols_res(conn)?;
    let legacy = table_columns_res(conn)?;
    if legacy.contains("db_id") {
        have.insert("id".to_string());
    }
    have.extend(legacy);
    // A synthetic column is offered on the REPORT schema alone, never on `valuation::is_attached`:
    // gating it on the derived cache would churn the column set — and with it every saved width and
    // visibility keyed to it — each time that cache detaches. An unattached cache renders empty
    // cells instead.
    let mut out: Vec<String> = DISPLAY_COLUMNS
        .iter()
        .filter(|c| {
            have.contains(**c)
                || synthetic(c)
                    .is_some_and(|entry| entry.inputs.iter().all(|name| have.contains(*name)))
        })
        .map(|c| (*c).to_string())
        .collect();
    let mut extra: Vec<String> = have
        .iter()
        .filter(|h| !SERVICE.contains(&h.as_str()) && !DISPLAY_COLUMNS.contains(&h.as_str()))
        .cloned()
        .collect();
    extra.sort();
    out.extend(extra);
    Ok(out)
}

/// Build one synthetic column's SQL against one physical source.
///
/// The single definition behind both the projection and the `ORDER BY`, so the two cannot disagree
/// about what a column means or about whether this source can produce it.
///
/// Args:
///     entry: Synthetic column being built.
///     src: Physical source whose schema decides availability.
///     valuation: Derived-cache fragments, absent when that cache is not joined.
///
/// Returns:
///     The expression, or `None` when this source cannot compute the column.
fn synthetic_expression(
    entry: &Synthetic,
    src: &ReadSource,
    valuation: Option<&super::valuation::CoverageSql>,
) -> Option<String> {
    if !entry.inputs.iter().all(|name| src.cols.contains(*name)) {
        return None;
    }
    Some(match entry.name {
        PROFIT_PERCENT_COLUMN => {
            "CASE WHEN r.\"spentbtc\" > 0 THEN r.\"profitbtc\" / r.\"spentbtc\" * 100.0 END"
                .to_string()
        }
        // A cache-free retry has no `v` or `ra` aliases, so the expression must vanish with the
        // joins that back it.
        VALUATION_PROFIT_COLUMN => valuation?.profit_usdt.clone(),
        VALUATION_RATE_COLUMN => valuation?.per_row.rate.clone(),
        VALUATION_SOURCE_COLUMN => valuation?.per_row.source.clone(),
        _ => return None,
    })
}

/// Project a source onto the shared `cols`: preserve its own columns, map legacy
/// `db_id` to `id`, and emit NULL for absent columns.
///
/// Every reference is qualified with the source alias because the valuation joins bring their own
/// `closedate`, `core_uid` and `status` columns into scope; an unqualified name would be ambiguous.
///
/// Args:
///     src: Physical report source and its discovered schema.
///     cols: Shared runtime display columns to project in order.
///     valuation: Derived-cache fragments when that cache is joined.
///
/// Returns:
///     Comma-separated SQL projection for the aliased source.
fn source_select(
    src: &ReadSource,
    cols: &[String],
    valuation: Option<&super::valuation::CoverageSql>,
) -> String {
    cols.iter()
        .map(|c| {
            if let Some(entry) = synthetic(c) {
                let sql = synthetic_expression(entry, src, valuation)
                    .unwrap_or_else(|| "NULL".to_string());
                format!("{sql} AS \"{c}\"")
            } else if src.legacy && c == "id" && src.cols.contains("db_id") {
                "r.\"db_id\" AS \"id\"".to_string()
            } else if src.cols.contains(c) {
                format!("r.\"{c}\"")
            } else {
                format!("NULL AS \"{c}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Compare values while merging sorted sources: numbers as `f64`, text
/// lexicographically. The caller handles NULL and always places it last.
fn cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Integer(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            _ => None,
        }
    }
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => match (a, b) {
            (Value::Text(x), Value::Text(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        },
    }
}

/// Validate the sort key against runtime columns to prevent injection.
///
/// Fall back to `closedate`, or to the always-present `newrecid` before the core
/// schema supplies `closedate`.
fn sort_column(cols: &[String], key: &str) -> String {
    if let Some(c) = cols.iter().find(|c| c.as_str() == key) {
        return c.clone();
    }
    if cols.iter().any(|c| c == "closedate") {
        "closedate".to_string()
    } else {
        "newrecid".to_string()
    }
}

/// Return the source-local SQL expression for a validated report sort column.
///
/// Synthetic columns need their defining expression here because every source is truncated before
/// the Rust merge. Sorting only after that truncation would return the wrong global top rows.
///
/// Args:
///     src: Physical source whose schema determines sort availability.
///     col: Validated runtime sort-column key.
///     valuation: Derived-cache fragments when that cache is joined.
///
/// Returns:
///     SQL expression when the source can sort by the column, otherwise `None`.
fn source_sort_expression(
    src: &ReadSource,
    col: &str,
    valuation: Option<&super::valuation::CoverageSql>,
) -> Option<String> {
    if let Some(entry) = synthetic(col) {
        return synthetic_expression(entry, src, valuation);
    }
    if src.cols.contains(col) {
        Some(format!("r.\"{col}\""))
    } else if src.legacy && col == "id" && src.cols.contains("db_id") {
        Some("r.\"db_id\"".to_string())
    } else {
        None
    }
}

/// Apply report predicates to one aliased source.
///
/// Before the core schema arrives, the replica may lack `closedate`, `coin`,
/// `isshort`, or `emulator`; filtering on an absent column would fail the entire
/// SELECT. An exact strategy is different: a source without either identity column
/// cannot prove a match and therefore contributes zero rows.
///
/// Args:
///     f: Complete Report filter.
///     cols: Columns available on this source.
///     has_strategy_names: Whether liquidation attribution metadata is readable.
///
/// Returns:
///     Parameterized SQL suffix and its ordered bound values.
fn build_where(
    f: &ReportFilter,
    cols: &std::collections::HashSet<String>,
    has_strategy_names: bool,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let has = |n: &str| cols.contains(n);
    let mut sql = String::from(" WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if !f.core_uids.is_empty() {
        // Inline numeric values safely; IN supports the multi-core selector.
        let ids = f
            .core_uids
            .iter()
            .map(|u| (*u as i64).to_string())
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND r.core_uid IN ({ids})"));
    }
    append_strategy_filter(&mut sql, f, cols, has_strategy_names);
    if f.closed_only {
        if has("closedate") {
            sql.push_str(" AND r.closedate > 0");
        } else {
            sql.push_str(" AND 1=0");
        }
    }
    if has("closedate") {
        if let Some(from) = f.date_from {
            sql.push_str(" AND r.closedate IS NOT NULL AND r.closedate >= ?");
            params.push(Box::new(from));
        }
        if let Some(to) = f.date_to {
            sql.push_str(" AND r.closedate IS NOT NULL AND r.closedate <= ?");
            params.push(Box::new(to));
        }
    }
    let coin = f.coin.trim();
    if !coin.is_empty() && has("coin") {
        sql.push_str(" AND r.coin LIKE ?");
        params.push(Box::new(format!("%{}%", coin.to_uppercase())));
    }
    if has("isshort") {
        match f.side {
            SideFilter::All => {}
            SideFilter::Long => sql.push_str(" AND r.isshort = 0"),
            SideFilter::Short => sql.push_str(" AND r.isshort = 1"),
        }
    }
    if has("emulator") {
        match f.emulator {
            None => {}
            Some(true) => sql.push_str(" AND COALESCE(r.emulator, 0) = 1"),
            Some(false) => sql.push_str(" AND COALESCE(r.emulator, 0) = 0"),
        }
    }
    // Deleted-mode semantics live on `ReportFilter::deleted_only`; the `1=0` arm makes
    // a column-less source contribute nothing when only deleted rows are wanted.
    if has("deleted") {
        sql.push_str(if f.deleted_only {
            " AND COALESCE(r.deleted, 0) <> 0"
        } else {
            " AND COALESCE(r.deleted, 0) = 0"
        });
    } else if f.deleted_only {
        sql.push_str(" AND 1=0");
    }
    (sql, params)
}

/// Append the exact multi-strategy predicate without consuming SQLite bind-variable capacity.
///
/// Strategy and core ids are typed integers, so grouping their numeric literals by core is safe and
/// keeps a very large checkbox selection below SQLite's parameter limit. An explicit empty set or
/// a source without strategy identity remains a no-match constraint.
///
/// Args:
///     sql: Mutable WHERE clause receiving the strategy predicate.
///     filter: Complete Report filter containing the optional exact-key collection.
///     columns: Columns available on the current report source.
///     has_strategy_names: Whether liquidation attribution metadata is readable.
///
/// Returns:
///     Nothing; `sql` is unchanged only when the strategy filter is implicit All.
fn append_strategy_filter(
    sql: &mut String,
    filter: &ReportFilter,
    columns: &std::collections::HashSet<String>,
    has_strategy_names: bool,
) {
    let Some(strategies) = &filter.strategies else {
        return;
    };
    if strategies.is_empty() || !columns.contains("core_uid") || !columns.contains("strategyid") {
        sql.push_str(" AND 1=0");
        return;
    }

    let mut by_core: std::collections::BTreeMap<u64, std::collections::BTreeSet<i64>> =
        std::collections::BTreeMap::new();
    for strategy in strategies {
        by_core
            .entry(strategy.core_uid)
            .or_default()
            .insert(strategy.strategy_id);
    }
    let sid = super::analytics::effective_sid_expr("r", columns, has_strategy_names);
    let groups = by_core
        .into_iter()
        .map(|(core_uid, strategy_ids)| {
            let ids = strategy_ids
                .into_iter()
                .map(|strategy_id| strategy_id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "(r.core_uid = {} AND COALESCE({sid}, 0) IN ({ids}))",
                core_uid as i64
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    sql.push_str(&format!(" AND ({groups})"));
}

/// Aggregate profit and order count over the complete filter, not only the top N.
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
///     Exact known-currency buckets, unknown and complete row counts, and optional complete USDT
///     valuation coverage.
///
/// Errors:
///     Returns `Failed` for source, SQL, or row conversion errors.
pub fn query_totals(conn: &Connection, f: &ReportFilter) -> ReadResult<QuoteBreakdown> {
    let sources = read_sources_res(conn)?;
    with_valuation_fallback(
        conn,
        "reports: query_totals",
        "reports: query_totals native retry",
        |include_valuation| query_totals_attempt(conn, f, &sources, include_valuation),
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
fn with_valuation_fallback<T>(
    conn: &Connection,
    ctx: &'static str,
    retry_ctx: &'static str,
    attempt: impl Fn(bool) -> rusqlite::Result<T>,
) -> ReadResult<T> {
    let attached = super::valuation::is_attached(conn);
    match attempt(attached) {
        Ok(value) => Ok(value),
        Err(error) if attached && super::valuation::prove_derived_corruption(conn, &error) => {
            let _ = conn.execute(&format!("DETACH DATABASE {}", super::valuation::SCHEMA), []);
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
pub(in crate::db) fn source_partition(src: &ReadSource) -> super::valuation::TradeSource {
    if src.legacy {
        super::valuation::TradeSource::Legacy
    } else {
        super::valuation::TradeSource::Typed
    }
}

/// Execute one complete Report totals pass with fresh accumulators.
///
/// Args:
///     conn: Open report reader or snapshot.
///     f: Complete Report filter.
///     sources: Physical report sources discovered from `main`.
///     include_valuation: Whether the historical mode may join the attached derived cache; the
///         current-rate mode does not depend on it.
///
/// Returns:
///     Exact quote totals, optionally carrying complete USDT coverage.
///
/// Errors:
///     Returns the underlying SQLite error from any physical-source aggregate.
fn query_totals_attempt(
    conn: &Connection,
    f: &ReportFilter,
    sources: &[ReadSource],
    include_valuation: bool,
) -> rusqlite::Result<QuoteBreakdown> {
    let mut groups = Vec::new();
    let mut coverage = super::valuation::CoverageAggregate::default();
    let has_strategy_names = f
        .strategies
        .as_ref()
        .is_some_and(|strategies| !strategies.is_empty())
        && super::analytics::strategies_attached(conn);
    // Loop-invariant: `projection` yields a builder for the current-rate mode whatever the cache is
    // doing, and for the historical one exactly when the cache may be joined.
    let valuation_present = f.valuation == ValuationMode::Current || include_valuation;
    for src in sources {
        let (where_sql, params) = build_where(f, &src.cols, has_strategy_names);
        let profit = if src.cols.contains("profitbtc") {
            "COALESCE(SUM(r.profitbtc),0.0)"
        } else {
            "0.0"
        };
        let (quote, group_by) =
            super::quote::trusted_quote_group("r.basecurrency", src.cols.contains("basecurrency"));
        let valuation = super::valuation::projection(
            f.valuation,
            include_valuation,
            "r",
            &src.cols,
            source_partition(src),
        );
        let joins = valuation
            .as_ref()
            .map(|parts| parts.joins.as_str())
            .unwrap_or("");
        let coverage_columns = valuation
            .as_ref()
            .map(|parts| format!(", {}", parts.aggregate_columns()))
            .unwrap_or_default();
        let sql = format!(
            "SELECT {quote}, {profit}, COUNT(*){coverage_columns}
             FROM {} r{joins}{where_sql}{group_by}",
            src.table,
        );
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(refs.as_slice())?;
        while let Some(row) = rows.next()? {
            let raw = row.get::<_, Value>(0)?;
            let ordinal = super::quote::report_ordinal_from_value(&raw);
            let profit = row.get::<_, f64>(1)?;
            let orders = row.get::<_, i64>(2)?;
            groups.push((ordinal, profit, orders));
            if valuation.is_some() {
                coverage.add_row(row, 3)?;
            }
        }
    }
    let totals = QuoteBreakdown::from_groups(groups);
    // Publish coverage whenever the selected mode can build a projection: always for current rates,
    // and only with an attached cache for historical rates.
    Ok(if valuation_present {
        totals.with_valuation(coverage.finish())
    } else {
        totals
    })
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
    /// Whether liquidation attribution metadata is readable.
    has_strategy_names: bool,
}

/// Execute one complete Report row pass and return its merged, truncated rows.
///
/// Each entry is `(core_uid, rec_id, data)`; `rec_id` is the replica `newrecid`, or 0 for a legacy
/// row that has none.
///
/// Args:
///     conn: Open report reader or snapshot.
///     pass: The request, identical across both attempts.
///     include_valuation: Whether the historical mode may join the attached derived cache; the
///         current-rate mode does not depend on it.
///
/// Returns:
///     Globally sorted rows, truncated to the requested limit.
///
/// Errors:
///     Returns the underlying SQLite error from any physical-source query.
fn query_reports_attempt(
    conn: &Connection,
    pass: &ReportPass,
    include_valuation: bool,
) -> rusqlite::Result<Vec<(u64, i64, Vec<Value>)>> {
    let dir = if pass.desc { "DESC" } else { "ASC" };
    let sort_ix = pass.cols.iter().position(|c| c == pass.sort_col);
    // Query the top N from EACH source separately so indexes work, then merge below.
    let mut merged: Vec<(u64, i64, Vec<Value>)> = Vec::new();
    for src in pass.sources {
        let (where_sql, mut params) = build_where(pass.filter, &src.cols, pass.has_strategy_names);
        let valuation = super::valuation::projection(
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
        let select = source_select(src, pass.cols, valuation.as_ref());
        // `newrecid` is a real column only on the typed replica; a legacy source projects 0, which
        // marks its rows as not soft-deletable (0 is never a real rec id).
        let rec_id_select = if src.cols.contains("newrecid") {
            "r.newrecid"
        } else {
            "0"
        };
        // Sort in SQL only if this source can express the column; otherwise source order is
        // irrelevant, because the merge below reorders everything anyway.
        let order = match source_sort_expression(src, pass.sort_col, valuation.as_ref()) {
            Some(expression) => format!("({expression}) IS NULL, {expression} {dir}"),
            None => "1".to_string(),
        };
        let sql = format!(
            "SELECT r.core_uid, {rec_id_select}, {select} FROM {} r{joins}{where_sql} ORDER BY {order} LIMIT ?",
            src.table
        );
        params.push(Box::new(pass.limit as i64));
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
                if pass.desc {
                    o.reverse()
                } else {
                    o
                }
            }
        }
    });
    merged.truncate(pass.limit);
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
    let cols = display_columns(conn)?;
    let col = sort_column(&cols, sort_key);
    let sources = read_sources_res(conn)?;
    let has_strategy_names = f
        .strategies
        .as_ref()
        .is_some_and(|strategies| !strategies.is_empty())
        && super::analytics::strategies_attached(conn);
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
            has_strategy_names,
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

/// Highest `core_uid` in one table, or `Ok(None)` when it is absent or holds no rows.
///
/// Shared by both stores keyed on `core_uid`, so the negative-value drop and the
/// absent-versus-failed distinction have one definition rather than one per caller.
pub(crate) fn max_core_uid_in(
    conn: &Connection,
    table: &str,
    ctx: &'static str,
) -> ReadResult<Option<u64>> {
    let present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )
        .map_err(|e| read_fail(ctx, e))?;
    if present == 0 {
        return Ok(None);
    }
    let found: Option<i64> = conn
        .query_row(&format!("SELECT MAX(core_uid) FROM {table}"), [], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .map_err(|e| read_fail(ctx, e))?;
    // A negative value cannot be a uid; dropping it beats wrapping into a huge `u64`.
    Ok(found.and_then(|v| u64::try_from(v).ok()))
}

/// Highest `core_uid` any report row has ever carried, across both schemas.
///
/// Feeds the durable uid high-water mark: rows here outlive the server that wrote them, so a
/// uid still present in this replica must never be handed to a new core. `Ok(None)` means the
/// read succeeded and found no rows — the caller must keep that distinct from a failure, since
/// only the former is safe to treat as "this store contributes nothing".
///
/// A source whose schema lacks `core_uid` is skipped rather than queried: `read_sources_res`
/// always reports the modern table, which does not exist until `rep::init` has run. Negative
/// values cannot be uids and are dropped instead of wrapping into a huge `u64`.
pub fn max_core_uid(conn: &Connection) -> ReadResult<Option<u64>> {
    const CTX: &str = "отчёты: max_core_uid";
    let mut max: Option<u64> = None;
    for src in read_sources_res(conn)? {
        if !src.cols.contains("core_uid") {
            continue;
        }
        // MAX over the leading PK column is an index seek, and one seek is all this needs —
        // `distinct_cores` below walks every core because it must NAME them all.
        max = max.max(max_core_uid_in(conn, src.table, CTX)?);
    }
    Ok(max)
}

/// Load cores for the filter selector.
///
/// Source, query, and row-conversion errors map to `Failed`; only a successful
/// query may return an empty list. The open connection means this function
/// cannot return `NotReady`.
pub fn distinct_cores(conn: &Connection) -> ReadResult<Vec<(u64, String)>> {
    const CTX: &str = "отчёты: distinct_cores";
    let mut out: Vec<(u64, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for src in read_sources_res(conn)? {
        let table = src.table;
        // Within each source, cores come out newest-first (sources are then concatenated, so
        // every replica core still precedes a legacy-only one). HOW that order is computed
        // depends on whether the source's recency key is index-ordered, and the two answers are
        // not interchangeable — each statement is the fast one for its table and the slow one
        // for the other.
        let sql = if src.legacy {
            // The legacy table's key (`updated_ms`) sits in no index, so asking per core for
            // its newest row would sort that core's rows, once per core: measured 317 ms
            // against 150 ms for this single grouped pass on a 300k-row legacy table. This
            // read-only, transitional table therefore keeps the statement it always had —
            // including its reliance on SQLite's bare-column rule to pick the name.
            format!(
                "SELECT core_uid, core_name FROM {table}
                 GROUP BY core_uid ORDER BY MAX(COALESCE(updated_ms, 0)) DESC"
            )
        } else {
            // The replica's key IS index-ordered (`newrecid` is the second column of the
            // primary key), so walk the distinct `core_uid` values by seeking past each one — a
            // LOOSE INDEX SCAN — instead of reading every row in the table to answer a question
            // about 19 values. Measured on a 458 MB replica (529 862 rows): 250 ms for the
            // grouped pass against under a millisecond here. Both panels now throttle the core
            // list to once a minute, so it is no longer paid per reload — but it is still paid
            // on every panel construction, and `report/state.rs` pays it synchronously on the
            // UI thread. It is what the app's own "медленный query 298ms" warning was reporting.
            //
            // The rows are IDENTICAL, order included (verified one by one against the old
            // statement on that replica): the name comes from the core's newest row, which is
            // the row SQLite's bare-column rule for a lone min/max aggregate was already
            // picking — implicitly, and only while that query keeps exactly one such aggregate.
            format!(
                "WITH RECURSIVE cores(uid) AS (
                     SELECT MIN(core_uid) FROM {table}
                     UNION ALL
                     SELECT (SELECT MIN(core_uid) FROM {table} WHERE core_uid > cores.uid)
                     FROM cores WHERE cores.uid IS NOT NULL
                 )
                 SELECT uid,
                        (SELECT core_name FROM {table}
                         WHERE core_uid = cores.uid ORDER BY newrecid DESC LIMIT 1),
                        (SELECT MAX(newrecid) FROM {table} WHERE core_uid = cores.uid)
                 FROM cores WHERE uid IS NOT NULL
                 ORDER BY 3 DESC"
            )
        };
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?))
            })
            .map_err(|e| read_fail(CTX, e))?;
        for row in rows {
            // core_uid keys the whole selector, so a conversion miss here is
            // never a skippable "dirty label".
            let (uid, name) = row.map_err(|e| read_fail(CTX, e))?;
            if seen.insert(uid) {
                out.push((uid, name));
            }
        }
    }
    Ok(out)
}

/// Load exact strategy identities present in report sources within the active Report scope.
///
/// Identity uses the same liquidation attribution and NULL-to-Manual semantics as
/// the exact filter, so every offered option can return the rows it represents.
/// Legacy sources that cannot identify strategies are skipped. Names come from the
/// attached strategy database when available and otherwise use the signed numeric id.
/// The strategy predicate itself is deliberately removed so an active checkbox does not hide
/// alternative strategies that match every other Report filter.
///
/// Args:
///     conn: Open report reader or snapshot with optional strategy attachment.
///     filter: Active Report filter; every predicate except `strategies` scopes discovery.
///
/// Returns:
///     Sorted exact strategy choices present in report sources.
///
/// Errors:
///     Returns `Failed` for source, strategy metadata, SQL, or row conversion errors.
pub fn distinct_strategies(
    conn: &Connection,
    filter: &ReportFilter,
) -> ReadResult<Vec<ReportStrategy>> {
    const CTX: &str = "reports: distinct_strategies";
    let mut scope = filter.clone();
    scope.strategies = None;
    let has_strategy_names = super::analytics::strategies_attached(conn);
    let mut names = std::collections::HashMap::new();
    if has_strategy_names {
        let mut stmt = conn
            .prepare("SELECT core_uid, strategy_id, name FROM strat.strategies")
            .map_err(|e| read_fail(CTX, e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| read_fail(CTX, e))?;
        for row in rows {
            let (core_uid, strategy_id, name) = row.map_err(|e| read_fail(CTX, e))?;
            names.insert((core_uid, strategy_id), name);
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for src in read_sources_res(conn)? {
        if !src.cols.contains("core_uid") || !src.cols.contains("strategyid") {
            continue;
        }
        let strategy_id = super::analytics::effective_sid_expr("r", &src.cols, has_strategy_names);
        let (where_sql, params) = build_where(&scope, &src.cols, has_strategy_names);
        let sql = format!(
            "SELECT DISTINCT r.core_uid, COALESCE({strategy_id}, 0) FROM {} r{where_sql}",
            src.table,
        );
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
        let rows = stmt
            .query_map(refs.as_slice(), |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)?))
            })
            .map_err(|e| read_fail(CTX, e))?;
        for row in rows {
            let (core_uid, strategy_id) = row.map_err(|e| read_fail(CTX, e))?;
            if seen.insert((core_uid, strategy_id)) {
                out.push(ReportStrategy {
                    key: ReportStrategyKey {
                        core_uid,
                        strategy_id,
                    },
                    name: names
                        .get(&(core_uid, strategy_id))
                        .cloned()
                        .unwrap_or_else(|| strategy_id.to_string()),
                });
            }
        }
    }
    out.sort_by_cached_key(|strategy| {
        (
            strategy.name.to_lowercase(),
            strategy.key.core_uid,
            strategy.key.strategy_id,
        )
    });
    Ok(out)
}

#[cfg(test)]
mod tests;
