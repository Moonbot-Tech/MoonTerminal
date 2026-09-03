//! Read layer for the Reports window: filters, source projection, sort/merge, and aggregates.

use rusqlite::Connection;
use rusqlite::types::Value;

use super::name_fold::{install_unicode_casefold, strategy_name_casefold};
use super::read_fail::read_fail;
use super::rep;
use super::valuation::ValuationMode;
use super::{
    QuoteBreakdown, QuoteCurrency, ReadResult, ReadSource, read_sources_res, table_columns_res,
};

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
    "valuation_profit_usdt",
    "profitpct",
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

/// Everything one Report totals read states: realized money, and the open positions beside it.
///
/// The two are separate FIELDS rather than one merged figure because they answer different
/// questions and must never be added together — [`Self::quotes`] is settled history, while
/// [`Self::open`] is what the market is showing right now and will change before it is a fact.
/// They also live here rather than as a field on [`QuoteBreakdown`], which Analytics builds for
/// its own surfaces and would carry an eternally empty open tally.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReportTotals {
    /// Realized profit per known currency over CLOSED rows only, plus traded volume, entry-spend
    /// subtotals, and coverage.
    pub quotes: QuoteBreakdown,
    /// Unrealized money on the positions still running, counted apart from every figure above.
    pub open: super::OpenPositions,
}

/// One durable closed trade projected for an exact chart core and market.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartTradeRecord {
    /// Stable report-replica record identity.
    pub record_id: i64,
    /// Runtime core identity that owns this trade.
    pub core_uid: u64,
    /// Coin identity stored by the originating core.
    pub coin: String,
    /// Entry timestamp, in seconds on the CORE's own wall clock — NOT true UTC. Lift it through
    /// `ReportAxis::to_utc(secs, core_uid)` before treating it as a Unix instant; see the
    /// `report_axis` module for why.
    pub buy_date: i64,
    /// Close timestamp, same core-local caveat as `buy_date` above.
    pub close_date: i64,
    /// Entry price.
    pub buy_price: f64,
    /// Exit price.
    pub sell_price: f64,
    /// Filled quantity reported by the core.
    pub quantity: f64,
    /// Whether the trade is short.
    pub is_short: bool,
    /// Whether an EMULATOR order made this trade, rather than a live one.
    ///
    /// Carried per row so the chart's trade-kind checkboxes can hide marks at DRAWING time. The
    /// alternative — narrowing the query itself — would make a display toggle decide which rows were
    /// read, and since the row cap is applied after the filter, hiding emulator trades would free
    /// slots and surface OLDER REAL trades that had been truncated away. A checkbox must not change
    /// what the history contains.
    ///
    /// OPTIONAL at the source, exactly like [`Self::profit`]: a replica whose table predates the
    /// `emulator` column reports every trade as REAL. That direction is deliberate — hiding real
    /// trades on old data is the unrecoverable error, while showing an emulated one as real is
    /// visible and recoverable. On such a replica the "emulator trades" checkbox appears inert,
    /// which is the correct failure.
    pub emulator: bool,
    /// Realized profit as the row SETTLED it, or `None` when this source carries no profit column.
    ///
    /// Read through `quote::settled_amount_expr`, the same correction the Report grid and the
    /// footer apply, so a COIN-M liquidation is not off by its own entry price. An absence is
    /// never a zero: a legacy source without `profitbtc` still returns every trade, and the hover
    /// card says the figure is unknown rather than printing a profit of nothing.
    pub profit: Option<f64>,
    /// Currency [`Self::profit`] is denominated in, decided by `quote::effective_ordinal_expr`.
    ///
    /// The ONE place a row's currency is decided, and it is not derivable from the coin: COIN-M
    /// rows share a coin spelling with USD-M while settling in BTC. Carried beside the amount
    /// because a bare number labelled with the wrong ticker is worse than no number.
    pub quote: Option<QuoteCurrency>,
    /// Realized profit as a PERCENTAGE of the amount spent, or `None` when either leg is missing.
    ///
    /// Both legs are settled amounts, so a COIN-M liquidation divides like for like — the exact
    /// definition the Report's own profit-percent column already uses. Unitless, and therefore
    /// readable even where [`Self::quote`] could not be resolved.
    pub profit_pct: Option<f64>,
}

/// Bounded durable chart-history result with explicit truncation state.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartTradeHistory {
    /// Newest-first records in the exact requested scope.
    pub records: Vec<ChartTradeRecord>,
    /// Whether at least one older matching record was omitted by the cap.
    pub truncated: bool,
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

/// Which trades one report query returns: closed, open, or both.
///
/// A trade is CLOSED once it carries a usable positive `closedate`, and OPEN until then. The two
/// are not the same kind of fact and that is why this is an enum rather than a pair of flags: a
/// closed trade is a historical event that a date window can contain, while an open one is the
/// present state of a position and belongs to no window at all. Representing both as booleans
/// would admit the meaningless "closed only, but include the open ones" state.
///
/// The default is [`Self::ClosedAndOpen`], which is what an unset filter has always meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowScope {
    /// Only trades with a usable positive `closedate` — Analytics' closed-trade universe.
    ///
    /// Every boundary that publishes durable history takes this arm: chart trade history, the
    /// strategy purge scan, and the Analytics-owned scoped Report window.
    Closed,
    /// Closed trades inside the date window, plus every open position regardless of the window.
    #[default]
    ClosedAndOpen,
    /// Only trades still running. The date window never applies to them.
    ///
    /// Reached through the Report's second row pass and its totals aggregate; a caller asking for
    /// the present state alone would set it too.
    Open,
    /// Closed trades inside the window, plus open positions only where the window still reaches
    /// the present ON THAT CORE'S OWN CLOCK.
    ///
    /// This is an INTENT, not a resolved answer, and it is a separate variant because the answer
    /// is no longer single-valued. `date_to` is compared against a CORE-LOCAL column while "now"
    /// is this machine's true UTC, so one window can have demonstrably ended for a core running
    /// four hours behind and still be current for one running three ahead. Resolving it in the UI
    /// — as this scope's predecessor did — forces one verdict onto a fleet that does not share
    /// one, which is why the decision moved down to [`append_row_scope`], the one place that
    /// knows both the axis and which cores are in play.
    ///
    /// The asymmetry that governs the per-group decision is unchanged: admitting an open row into
    /// a window that had already ended shows a position the user can see is still running, while
    /// DROPPING one silently removes money from a report that still looks complete.
    ClosedAndOpenIfCurrent,
    /// The OPEN half of [`Self::ClosedAndOpenIfCurrent`], for the two-pass row query alone.
    ///
    /// The row query runs open and closed as separate passes so the open block can carry its own
    /// newest-first order and its own guaranteed slots. That split needs a scope meaning "open
    /// rows, but only from the cores whose window still reaches the present" — which plain
    /// [`Self::Open`] cannot say, since it deliberately ignores the window entirely. No caller
    /// outside that splitter sets this.
    OpenIfCurrent,
}

/// Complete filter shared by Report rows, totals, export, and strategy discovery.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReportFilter {
    /// Selected cores for the multi-select filter; empty means all cores. A caller holding a
    /// scope that is PRESENT but EMPTY (every named core has been filtered out) must send
    /// [`crate::config::NO_MATCH_CORE_UID`] rather than an empty list, or the read broadens to
    /// every core instead of returning none.
    pub core_uids: Vec<u64>,
    pub date_from: Option<i64>,
    pub date_to: Option<i64>,
    pub coin: String,
    /// Exact case-insensitive coin identities used by chart history.
    ///
    /// `None` keeps the Report substring filter above. `Some` replaces it with an exact set;
    /// an explicit empty set matches no rows so a lost market identity cannot widen the query.
    pub exact_coins: Option<Vec<String>>,
    pub side: SideFilter,
    /// Emulator orders: `None` selects all, `Some(false)` only real orders, and
    /// `Some(true)` only emulator orders. A NULL column value counts as real.
    pub emulator: Option<bool>,
    /// Soft-deleted trades (the core-supplied `deleted` column): `false` hides them,
    /// `true` shows ONLY them. A NULL column value counts as not deleted, matching
    /// the analytics filter; a source without the column holds no soft-deleted rows.
    pub deleted_only: bool,
    /// Which trades this query returns — see [`RowScope`].
    pub rows: RowScope,
    /// Time axis the replicated date columns are read on.
    ///
    /// Carried on the filter rather than loaded inside each query for the same reason
    /// [`Self::valuation`] is: the rows, the totals, the export and the RENDERED cell all take
    /// this one value, so a window built on one axis can never disagree with a timestamp printed
    /// on another. A default-constructed axis is the identity, which is what every caller that
    /// has not yet been given one already means.
    pub axis: crate::db::ReportAxis,
    /// Exact strategy identities; `None` selects all strategies, while `Some` remains constrained.
    ///
    /// The core is part of every key because strategy ids repeat across cores. An explicit empty
    /// collection intentionally matches no rows so a lost/stale selection cannot broaden a query.
    pub strategies: Option<Vec<ReportStrategyKey>>,
    /// Literal case-insensitive substring matched against the effective strategy name.
    ///
    /// Empty or whitespace-only text adds no predicate. This stays independent of the exact
    /// strategy keys above, so using both filters narrows by their conjunction.
    pub strategy_name_mask: String,
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
            // Both legs are the SETTLED amounts, so a COIN-M liquidation divides like for like.
            let profit = super::quote::settled_amount_expr("r", &src.cols, "profitbtc");
            let spent = super::quote::settled_amount_expr("r", &src.cols, "spentbtc");
            format!("CASE WHEN {spent} > 0 THEN {profit} / {spent} * 100.0 END")
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
            } else if let Some(sql) = corrected_column_expression(src, c) {
                format!("{sql} AS \"{c}\"")
            } else if src.cols.contains(c) {
                format!("r.\"{c}\"")
            } else {
                format!("NULL AS \"{c}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Return the corrected SQL for a stored column the reader must not serve raw, if `col` is one.
///
/// Both the projection and the `ORDER BY` go through here for the same reason synthetic columns do:
/// every source is truncated by `LIMIT` before the Rust merge, so a column projected corrected and
/// sorted raw would order the rows by one value and then show another — silently returning the
/// wrong top rows when the user sorts by that column.
///
/// Args:
///     src: Physical source whose schema decides availability.
///     col: Runtime Report column key.
///
/// Returns:
///     The expression, or `None` for every column this reader serves as stored.
fn corrected_column_expression(src: &ReadSource, col: &str) -> Option<String> {
    if col == "basecurrency" && src.cols.contains(col) {
        // The displayed ticker must name the currency the row's own profit column is in, or the
        // table would print USDT beside a total the footer counted as BTC.
        return Some(super::quote::effective_ordinal_expr("r", &src.cols));
    }
    // The money columns for the same reason: the footer, the percent column and the USDT valuation
    // all read the settled amount, so a grid serving the stored one would show 0.00001826 in the
    // row, 0.01120 in the total, and sort by the dust.
    if !matches!(col, "profitbtc" | "spentbtc") || !src.cols.contains(col) {
        return None;
    }
    let settled = super::quote::settled_amount_expr("r", &src.cols, col);
    // A source that cannot evidence a liquidation gets the plain column back. Reporting that as a
    // "correction" would be a lie the sort/projection contract test rightly refuses.
    (settled != format!("r.\"{col}\"")).then_some(settled)
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
    if let Some(sql) = corrected_column_expression(src, col) {
        return Some(sql);
    }
    if src.cols.contains(col) {
        Some(format!("r.\"{col}\""))
    } else if src.legacy && col == "id" && src.cols.contains("db_id") {
        Some("r.\"db_id\"".to_string())
    } else {
        None
    }
}

/// Return the canonical "this row is closed" test for one aliased source.
///
/// The type check is load-bearing rather than decorative: SQLite orders TEXT above every number,
/// so a bare `closedate > 0` counts an unparseable timestamp as a close time. An unparseable
/// value is not a close time, so it reads as still open — the same judgement the traded-volume
/// eligibility test has always made, and this is now the ONE place either of them spells it.
///
/// Args:
///     cols: Columns available on this source.
///
/// Returns:
///     The predicate, or `None` when the source cannot express `closedate` at all.
fn closed_row_predicate(cols: &std::collections::HashSet<String>) -> Option<String> {
    cols.contains("closedate").then(|| {
        "(typeof(r.\"closedate\") IN ('integer','real') AND r.\"closedate\" > 0)".to_string()
    })
}

/// Return the canonical "this row is still open" test for one aliased source.
///
/// Built as the literal negation of [`closed_row_predicate`] so the two partition every row
/// exactly once by construction rather than by two spellings agreeing. `typeof(NULL)` is
/// `'null'`, so the inner expression is FALSE rather than NULL for an absent close time and no
/// three-valued logic escapes into the surrounding `OR`.
///
/// Args:
///     cols: Columns available on this source.
///
/// Returns:
///     The predicate, or `None` when the source cannot express `closedate` at all.
fn open_row_predicate(cols: &std::collections::HashSet<String>) -> Option<String> {
    closed_row_predicate(cols).map(|closed| format!("(NOT {closed})"))
}

/// Decide whether a report period reaches the present, and therefore admits open positions.
///
/// An open position has no `closedate`, so no date window can contain it as an event. What
/// decides its membership is whether the window reaches NOW: a period ending in the past is a
/// retrospective, where a still-running position would be a statement about a time it did not
/// hold. The period's LOWER bound is deliberately not consulted — a position opened last week and
/// still running belongs in "today" precisely because it is present state rather than history.
///
/// # Why the comparison is deliberately slack
///
/// `date_to` is resolved on the axis of the column it filters — the CORE's own wall clock — while
/// `now` is this machine's true UTC. `offset_secs` is what makes them comparable: the group's
/// cores read `now` as `now + offset_secs` on their own clocks, so that is the instant the bound
/// is tested against. A group with no measured offset passes `0` and lands exactly on the naive
/// comparison, which is correct for it.
///
/// This replaced a version that widened the comparison by the widest real time-zone offset in
/// BOTH directions, because it could not tell which core a row came from. That was deliberately
/// generous — the two ways to be wrong are not symmetric, since admitting an open row into a
/// window that had already ended shows a position the user can see is still running, while
/// DROPPING one silently removes money from a report that still looks complete. With the offset
/// known per group the generosity is no longer needed, and the slack it cost goes away.
///
/// Args:
///     date_to: Inclusive upper bound of the period, or `None` for an unbounded one.
///     now: Current Unix timestamp in seconds, on this machine's true UTC.
///     offset_secs: Seconds east of UTC on the clock of every core in this group.
///
/// Returns:
///     [`RowScope::ClosedAndOpen`] for a period still reaching the present on that clock,
///     [`RowScope::Closed`] for one that has demonstrably already ended there.
pub fn open_rows_for_bound(date_to: Option<i64>, now: i64, offset_secs: i32) -> RowScope {
    let ended = now.saturating_add(i64::from(offset_secs));
    match date_to {
        Some(to) if to < ended => RowScope::Closed,
        _ => RowScope::ClosedAndOpen,
    }
}

/// Append the row-scope predicate and the date window, which are ONE decision.
///
/// They are appended together because the window only ever applied to closed rows: an open
/// position carries no `closedate` to compare, so binding it to the window is what used to drop
/// it from every bounded period. Under [`RowScope::ClosedAndOpen`] the window therefore
/// constrains the closed side alone and the open side rides past it.
///
/// A source that cannot express `closedate` degrades per arm, and the asymmetry is deliberate.
/// `Closed` and `Open` both fail CLOSED — a source that cannot prove a row's state must not
/// assert it — while `ClosedAndOpen` emits nothing at all, which is exactly what an unset filter
/// did before this predicate existed and keeps a pre-schema replica showing its rows.
///
/// # The coarse range
///
/// With two or more offset groups and a bounded period on both sides, the closed branches share
/// one leading `closedate` range spanning the union of their shifted windows. It is a strict
/// SUPERSET of every branch's own window, so it admits no row those branches would not admit
/// themselves; what it buys is one index seek for the whole disjunction instead of one per
/// branch, measured at 1.94x over a 12-core half-million-row replica at four distinct zones.
///
/// It leads the CLOSED disjunct ALONE, never the whole predicate. An open position carries no
/// `closedate` at all, so a range in front of everything would drop every open row -- money
/// vanishing from a report that still looks complete, which is the exact failure the closed/open
/// asymmetry above exists to prevent.
///
/// Args:
///     sql: Predicate buffer being built.
///     params: Ordered bound values being built.
///     f: Complete Report filter.
///     cols: Columns available on this source.
fn append_row_scope(
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    f: &ReportFilter,
    cols: &std::collections::HashSet<String>,
) {
    // Open rows carry no `closedate`, so neither the window nor the axis reaches them: this arm
    // is offset-independent and stays exactly the single-branch shape it always was.
    if f.rows == RowScope::Open {
        match open_row_predicate(cols) {
            Some(open) => sql.push_str(&format!(" AND {open}")),
            None => sql.push_str(" AND 1=0"),
        }
        return;
    }
    // Read once for the whole predicate, the same way the UI used to read it once for the whole
    // filter: two branches resolving "does this window still reach the present" against two
    // different instants would be a difference nothing on screen could explain.
    let now = crate::util::now_unix_ms_i64().div_euclid(1_000);
    let groups = offset_groups(f, now);
    let mut parts: Vec<GroupPredicate> = Vec::new();
    for (offset, cores) in &groups {
        let mut guard = String::new();
        if let Some(cores) = cores {
            if cores.is_empty() {
                continue;
            }
            let ids = cores
                .iter()
                .map(|uid| (*uid as i64).to_string())
                .collect::<Vec<_>>()
                .join(",");
            // Leads with `core_uid` so this branch still opens `idx_rep_core_close` rather than
            // scanning: that index is what keeps the period filter at tens of milliseconds over a
            // half-million-row replica, and it is the whole reason the offset moves onto the
            // BOUND instead of wrapping the column in a conversion.
            guard.push_str(&format!("r.core_uid IN ({ids}) AND "));
        } else if let Some(excluded) = catch_all_exclusion(f) {
            guard.push_str(&format!("r.core_uid NOT IN ({excluded}) AND "));
        }
        // The bounds are true-UTC instants and the column is core-local, so the group's offset is
        // added to the BOUND. Converting the column instead would be the same arithmetic and would
        // cost the index.
        let mut window = String::new();
        let mut bounds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut shifted_from: Option<i64> = None;
        let mut shifted_to: Option<i64> = None;
        if let Some(from) = f.date_from {
            let shifted = crate::db::ReportAxis::shift_bound(from, *offset);
            window.push_str(" AND r.\"closedate\" >= ?");
            bounds.push(Box::new(shifted));
            shifted_from = Some(shifted);
        }
        if let Some(to) = f.date_to {
            let shifted = crate::db::ReportAxis::shift_bound(to, *offset);
            window.push_str(" AND r.\"closedate\" <= ?");
            bounds.push(Box::new(shifted));
            shifted_to = Some(shifted);
        }
        let current = open_rows_for_bound(f.date_to, now, *offset) == RowScope::ClosedAndOpen;
        if f.rows == RowScope::OpenIfCurrent {
            // A group whose window has already ended contributes NO open rows, so it contributes
            // no branch at all. Dropping the branch rather than emitting a false one is what lets
            // the empty case below fail closed.
            if !current {
                continue;
            }
            parts.push(GroupPredicate {
                guard,
                open: Some(open_row_predicate(cols).unwrap_or_else(|| "1=0".to_string())),
                ..GroupPredicate::default()
            });
            continue;
        }
        let resolved = match f.rows {
            RowScope::ClosedAndOpenIfCurrent if current => RowScope::ClosedAndOpen,
            RowScope::ClosedAndOpenIfCurrent => RowScope::Closed,
            other => other,
        };
        let mut part = GroupPredicate {
            guard,
            ..GroupPredicate::default()
        };
        match resolved {
            RowScope::Closed => match closed_row_predicate(cols) {
                Some(closed) => {
                    part.closed = Some(format!("{closed}{window}"));
                    part.bounds = bounds;
                    part.shifted_from = shifted_from;
                    part.shifted_to = shifted_to;
                }
                None => part.closed = Some("1=0".to_string()),
            },
            RowScope::ClosedAndOpen => {
                match (closed_row_predicate(cols), open_row_predicate(cols)) {
                    (Some(closed), Some(open)) => {
                        part.closed = Some(format!("{closed}{window}"));
                        part.bounds = bounds;
                        part.shifted_from = shifted_from;
                        part.shifted_to = shifted_to;
                        part.open = Some(open);
                    }
                    // Without the column there is no row state to test and no window to apply.
                    // Emitting nothing is what an unset filter always did and is what keeps a
                    // pre-schema replica showing its rows. Nothing has been bound yet, so this
                    // leaves both buffers exactly as it found them.
                    _ => return,
                }
            }
            // Every other variant was handled before this match: `Open` and `OpenIfCurrent`
            // returned or continued above, and `ClosedAndOpenIfCurrent` resolved into one of the
            // two arms here.
            RowScope::Open | RowScope::OpenIfCurrent | RowScope::ClosedAndOpenIfCurrent => return,
        }
        parts.push(part);
    }
    if let Some((from, to)) = coarse_range(&parts) {
        let closed = parts
            .iter()
            .filter_map(|p| p.closed.as_ref().map(|c| format!("{}{c}", p.guard)))
            .collect::<Vec<_>>()
            .join(") OR (");
        let open = parts
            .iter()
            .filter_map(|p| p.open.as_ref().map(|o| format!("{}{o}", p.guard)))
            .collect::<Vec<_>>()
            .join(") OR (");
        // The range is bound FIRST because it is emitted first, and the per-group bounds follow
        // in group order exactly as they did before this shape existed.
        params.push(Box::new(from));
        params.push(Box::new(to));
        for part in parts.iter_mut() {
            params.append(&mut part.bounds);
        }
        let closed_side = format!("r.\"closedate\" >= ? AND r.\"closedate\" <= ? AND (({closed}))");
        if open.is_empty() {
            sql.push_str(&format!(" AND ({closed_side})"));
        } else {
            sql.push_str(&format!(" AND (({closed_side}) OR (({open})))"));
        }
        return;
    }
    let branches = parts.iter().map(GroupPredicate::branch).collect::<Vec<_>>();
    for part in parts.iter_mut() {
        params.append(&mut part.bounds);
    }
    match branches.len() {
        // An open-only pass with no current group has nothing to show and must SAY so; every
        // other scope reaching zero branches had no predicate to apply in the first place.
        0 if f.rows == RowScope::OpenIfCurrent => sql.push_str(" AND 1=0"),
        0 => {}
        1 => sql.push_str(&format!(" AND {}", branches[0])),
        _ => sql.push_str(&format!(" AND (({}))", branches.join(") OR ("))),
    }
}

/// One offset group's contribution to the row-scope predicate, held apart so the closed and open
/// sides can be composed either per group or factored under a shared coarse range.
#[derive(Default)]
struct GroupPredicate {
    /// `core_uid` guard this group's branches lead with; empty for an unguarded single group.
    guard: String,
    /// Closed-side predicate including this group's own shifted window, or `None` when the scope
    /// asks for open rows alone.
    closed: Option<String>,
    /// Values bound by `closed`, in the order they appear in it.
    bounds: Vec<Box<dyn rusqlite::types::ToSql>>,
    /// Open-side predicate, or `None` when this group contributes no open rows.
    open: Option<String>,
    /// This group's lower bound after the offset shift; `None` for an unbounded period.
    shifted_from: Option<i64>,
    /// This group's upper bound after the offset shift; `None` for an unbounded period.
    shifted_to: Option<i64>,
}

impl GroupPredicate {
    /// Compose this group as ONE self-contained branch, the shape used whenever the coarse range
    /// does not apply.
    ///
    /// Returns:
    ///     Branch text, guard included.
    fn branch(&self) -> String {
        let mut branch = self.guard.clone();
        match (&self.closed, &self.open) {
            (Some(closed), Some(open)) => branch.push_str(&format!("(({closed}) OR {open})")),
            (Some(closed), None) => branch.push_str(closed),
            (None, Some(open)) => branch.push_str(open),
            // Every push site sets at least one side, so a part with neither is never built.
            (None, None) => {}
        }
        branch
    }
}

/// Widest window every closed branch fits inside, when factoring one out is worth doing.
///
/// Args:
///     parts: Every group that contributed to this predicate.
///
/// Returns:
///     Shifted lower and upper bound spanning all closed branches, or `None` when there is only
///     one of them, when the period is unbounded on either side, or when any closed branch
///     carries no window to widen.
fn coarse_range(parts: &[GroupPredicate]) -> Option<(i64, i64)> {
    let closed = parts
        .iter()
        .filter(|p| p.closed.is_some())
        .collect::<Vec<_>>();
    if closed.len() < 2 {
        return None;
    }
    let mut from = i64::MAX;
    let mut to = i64::MIN;
    for part in closed {
        from = from.min(part.shifted_from?);
        to = to.max(part.shifted_to?);
    }
    Some((from, to))
}

/// Resolve which offset groups this filter's rows fall into.
///
/// A scoped read names its cores and groups exactly those. An UNBOUNDED read cannot name them, so
/// it takes one branch per MEASURED offset plus a catch-all carrying `None` -- every core with no
/// measurement, which converts as the identity.
///
/// # Known limitation: grouped at ONE instant
///
/// A core is placed in the group its offset occupies at `now`, and that single offset then shifts
/// BOTH bounds. A period spanning an offset transition therefore has its far bound shifted by the
/// wrong segment, so trades within one delta of that edge can be admitted or dropped. Bounded by
/// the delta -- an hour across DST -- and only ever at the edge.
///
/// The alternative is a sub-branch per segment per core, which multiplies the disjunction the
/// coarse range in [`append_row_scope`] exists to keep cheap. Left deliberately, recorded here so
/// the next reader does not mistake it for an oversight.
///
/// Args:
///     f: Complete Report filter.
///     now: Current Unix timestamp in seconds, on this machine's true UTC.
///
/// Returns:
///     Offset and the cores it applies to, or `None` for the unbounded catch-all. A fleet with no
///     measurements at all collapses to a single identity group, which reproduces the predicate
///     this function had before offsets existed.
fn offset_groups(f: &ReportFilter, now: i64) -> Vec<(i32, Option<Vec<u64>>)> {
    if !f.core_uids.is_empty() {
        return f
            .axis
            .groups(&f.core_uids, now)
            .into_iter()
            .map(|(offset, cores)| (offset, Some(cores)))
            .collect();
    }
    let mut groups: Vec<(i32, Option<Vec<u64>>)> = f
        .axis
        .measured_groups(now)
        .into_iter()
        .map(|(offset, cores)| (offset, Some(cores)))
        .collect();
    groups.push((0, None));
    groups
}

/// Inline core-uid list a catch-all branch must exclude, or `None` when nothing is measured.
///
/// Args:
///     f: Complete Report filter.
///
/// Returns:
///     Comma-separated measured core uids, or `None` when the catch-all covers every core and
///     needs no guard at all.
fn catch_all_exclusion(f: &ReportFilter) -> Option<String> {
    let measured = f.axis.measured_cores();
    if measured.is_empty() {
        return None;
    }
    Some(
        measured
            .iter()
            .map(|uid| (*uid as i64).to_string())
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// Apply report predicates to one aliased source.
///
/// Before the core schema arrives, the replica may lack `closedate`, `coin`,
/// `isshort`, or `emulator`; filtering on an absent column would fail the entire
/// SELECT. A strategy predicate is different: a source without either identity column cannot
/// prove a match and therefore contributes zero rows.
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
    append_strategy_name_mask(&mut sql, &mut params, f, cols, has_strategy_names);
    append_row_scope(&mut sql, &mut params, f, cols);
    let coin = f.coin.trim();
    if let Some(coins) = &f.exact_coins {
        if coins.is_empty() || !has("coin") {
            sql.push_str(" AND 1=0");
        } else {
            sql.push_str(" AND (");
            for (index, exact) in coins.iter().enumerate() {
                if index > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str("r.coin COLLATE NOCASE = ?");
                params.push(Box::new(exact.clone()));
            }
            sql.push(')');
        }
    } else if !coin.is_empty() && has("coin") {
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

/// Return whether one filter needs the attached strategy-name metadata.
///
/// Exact keys need it only when attribution may change a physical strategy id. A non-empty name
/// mask always needs it because the report source stores ids rather than strategy names.
///
/// Args:
///     filter: Complete Report filter.
///
/// Returns:
///     Whether rows and totals should enable the shared strategy metadata path.
fn strategy_metadata_required(filter: &ReportFilter) -> bool {
    filter
        .strategies
        .as_ref()
        .is_some_and(|strategies| !strategies.is_empty())
        || !filter.strategy_name_mask.trim().is_empty()
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

/// Append a literal, case-insensitive strategy-name substring predicate.
///
/// `instr` gives `%`, `_`, and `\` no wildcard meaning, unlike `LIKE`, while the bound parameter
/// keeps arbitrary user text outside SQL syntax. The name is joined by the same effective strategy
/// id as the exact selector so liquidation attribution and physical strategy ids agree.
///
/// Args:
///     sql: Mutable WHERE clause receiving the name predicate.
///     params: Ordered SQL parameters paired with the WHERE clause.
///     filter: Complete Report filter containing the optional name mask.
///     columns: Columns available on the current report source.
///     has_strategy_names: Whether the attached strategy metadata is readable.
///
/// Returns:
///     Nothing; a non-empty mask fails closed when its identity or name metadata is unavailable.
fn append_strategy_name_mask(
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    filter: &ReportFilter,
    columns: &std::collections::HashSet<String>,
    has_strategy_names: bool,
) {
    let mask = filter.strategy_name_mask.trim();
    if mask.is_empty() {
        return;
    }
    if !has_strategy_names || !columns.contains("core_uid") || !columns.contains("strategyid") {
        sql.push_str(" AND 1=0");
        return;
    }

    let sid = super::analytics::effective_sid_expr("r", columns, has_strategy_names);
    sql.push_str(&format!(
        " AND EXISTS (SELECT 1 FROM strat.strategies mask_strategy \
         WHERE mask_strategy.core_uid = r.core_uid \
         AND mask_strategy.strategy_id = COALESCE({sid}, 0) \
         AND instr(mt_unicode_casefold(mask_strategy.name), ?) > 0)"
    ));
    params.push(Box::new(strategy_name_casefold(mask)));
}

/// Install the case folding for a Report read, only when its filter carries a mask.
///
/// Registration is skipped when the query has no mask, keeping unrelated Report reads unchanged.
///
/// Args:
///     conn: Open report reader or snapshot receiving the deterministic scalar function.
///     filter: Complete Report filter whose mask decides whether registration is required.
///
/// Returns:
///     Success after the function is installed or when no mask needs it.
///
/// Errors:
///     Returns SQLite's registration error when the function cannot be installed.
fn install_strategy_name_mask_function(
    conn: &Connection,
    filter: &ReportFilter,
) -> rusqlite::Result<()> {
    if filter.strategy_name_mask.trim().is_empty() {
        return Ok(());
    }
    install_unicode_casefold(conn)
}

/// SQL projecting the rec id the soft-delete protocol addresses a row by.
///
/// `newrecid` is a real column only on the typed replica; a legacy source projects `0`, which
/// marks its rows as not soft-deletable — `0` is never a real rec id. Both the Report table and
/// the strategy purge read go through here, so a source that gains or loses the column cannot end
/// up soft-deletable in one reader and not the other.
fn rec_id_expr(src: &ReadSource) -> &'static str {
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
/// `query_chart_trade_history` MINTS these ids and `super::trade_meta::query_trade_meta` resolves
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
    let has_strategy_names = super::analytics::strategies_attached(conn);
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
        let (mut where_sql, params) = build_where(&filter, &src.cols, has_strategy_names);
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
    install_strategy_name_mask_function(conn, f)
        .map_err(|error| read_fail("reports: install strategy mask", error))?;
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

/// Per-source SQL for two-sided Report volume over provable rows only.
struct TradedVolumeSql {
    /// Closed non-Funding row predicate, independent of the Report's profit/count scope.
    eligible: String,
    /// Eligible row whose native entry and exit notionals are dimensionally trustworthy.
    reconstructed: String,
    /// Unsigned native entry-plus-exit notional for a reconstructed row.
    native: String,
    /// Active-mode USDT rate, or SQL NULL when no valuation projection exists.
    rate: String,
}

impl TradedVolumeSql {
    /// Build the five grouped columns consumed by [`super::TradedVolume::from_groups`].
    ///
    /// Returns:
    ///     Eligible/reconstructed counts, native sum, valued count, and USDT sum in that order.
    fn aggregate_columns(&self) -> String {
        format!(
            "COALESCE(SUM(CASE WHEN {eligible} THEN 1 ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN {reconstructed} THEN 1 ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN {reconstructed} THEN {native} ELSE 0.0 END),0.0),
             COALESCE(SUM(CASE WHEN {reconstructed} AND ({rate}) IS NOT NULL
                               THEN 1 ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN {reconstructed} AND ({rate}) IS NOT NULL
                               THEN ({native}) * ({rate}) ELSE 0.0 END),0.0)",
            eligible = self.eligible,
            reconstructed = self.reconstructed,
            native = self.native,
            rate = self.rate,
        )
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
///     A `SUM` expression over the settled amount, or `0.0` on a source without `profitbtc`.
fn profit_sum_sql(src: &ReadSource) -> String {
    if src.cols.contains("profitbtc") {
        format!(
            "COALESCE(SUM({}),0.0)",
            super::quote::settled_amount_expr("r", &src.cols, "profitbtc")
        )
    } else {
        "0.0".to_string()
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
fn traded_volume_sql(src: &ReadSource, rate: Option<&str>) -> TradedVolumeSql {
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
    let quote_matches = super::quote::prices_share_money_quote_expr("r", &src.cols);
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
/// unified leg is this feature's OWN, never [`super::UsdtTotal::spent`], which carries no
/// positive-spend guard and no Funding exclusion.
struct EntrySpendSql {
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
    /// Build the six grouped columns consumed by [`super::EntrySpend::from_groups`].
    ///
    /// Returns:
    ///     Counted-row count, summed settled spend, summed settled profit, valued-row count, and
    ///     the summed USDT spend and profit over counted rows carrying a rate, in that order.
    fn aggregate_columns(&self) -> String {
        format!(
            "COALESCE(SUM(CASE WHEN {counted} THEN 1 ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN {counted} THEN {spent} ELSE 0.0 END),0.0),
             COALESCE(SUM(CASE WHEN {counted} THEN {profit} ELSE 0.0 END),0.0),
             COALESCE(SUM(CASE WHEN {counted} AND ({rate}) IS NOT NULL
                               THEN 1 ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN {counted} AND ({rate}) IS NOT NULL
                               THEN ({spent}) * ({rate}) ELSE 0.0 END),0.0),
             COALESCE(SUM(CASE WHEN {counted} AND ({rate}) IS NOT NULL
                               THEN ({profit}) * ({rate}) ELSE 0.0 END),0.0)",
            counted = self.counted,
            spent = self.spent,
            profit = self.profit,
            rate = self.rate,
        )
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
/// legs reuse [`super::valuation::source_predicates`] so this cannot silently disagree with the
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
fn entry_spend_sql(src: &ReadSource, rate: Option<&str>) -> EntrySpendSql {
    let has = |column: &str| src.cols.contains(column);
    let predicates = super::valuation::source_predicates("r", &src.cols);
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
            super::quote::settled_amount_expr("r", &src.cols, "profitbtc")
        } else {
            "0.0".to_string()
        },
        rate: rate.unwrap_or("NULL").to_string(),
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
///     Exact quote profit totals over closed rows, two-sided traded volume over the reconstructed
///     rows of each quote, and counted entry-spend subtotals over the same closed rows,
///     optionally carrying active-mode USDT coverage for each metric, plus the unrealized tally of
///     the rows still open.
///
/// Errors:
///     Returns the underlying SQLite error from any physical-source aggregate.
fn query_totals_attempt(
    conn: &Connection,
    f: &ReportFilter,
    sources: &[ReadSource],
    include_valuation: bool,
) -> rusqlite::Result<ReportTotals> {
    let mut groups = Vec::new();
    let mut open_groups = Vec::new();
    let mut volume_groups = Vec::new();
    let mut spend_groups = Vec::new();
    let mut coverage = super::valuation::CoverageAggregate::default();
    let has_strategy_names =
        strategy_metadata_required(f) && super::analytics::strategies_attached(conn);
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
    // still fails CLOSED on the same source — an unprovable position must never be invented.
    for src in sources {
        let closed_scope = ReportFilter {
            rows: if src.cols.contains("closedate") {
                RowScope::Closed
            } else {
                RowScope::ClosedAndOpen
            },
            ..f.clone()
        };
        let (where_sql, params) = build_where(&closed_scope, &src.cols, has_strategy_names);
        let profit = profit_sum_sql(src);
        let (quote, group_by) = super::quote::trusted_quote_group("r", &src.cols);
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
        let volume_sql = traded_volume_sql(
            src,
            valuation
                .as_ref()
                .map(|parts| parts.per_row.quote_rate.as_str()),
        );
        let volume_columns = volume_sql.aggregate_columns();
        let spend_sql = entry_spend_sql(
            src,
            valuation
                .as_ref()
                .map(|parts| parts.per_row.quote_rate.as_str()),
        );
        let spend_columns = spend_sql.aggregate_columns();
        let sql = format!(
            "SELECT {quote}, {profit}, COUNT(*){coverage_columns}, {volume_columns}, {spend_columns}
             FROM {} r{joins}{where_sql}{group_by}",
            src.table,
        );
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(refs.as_slice())?;
        let volume_offset = 3 + usize::from(valuation.is_some()) * 6;
        let spend_offset = volume_offset + 5;
        while let Some(row) = rows.next()? {
            let raw = row.get::<_, Value>(0)?;
            let ordinal = super::quote::report_ordinal_from_value(&raw);
            let profit = row.get::<_, f64>(1)?;
            let orders = row.get::<_, i64>(2)?;
            groups.push((ordinal, profit, orders));
            if valuation.is_some() {
                coverage.add_row(row, 3)?;
            }
            volume_groups.push((
                ordinal,
                row.get::<_, i64>(volume_offset)?,
                row.get::<_, i64>(volume_offset + 1)?,
                row.get::<_, f64>(volume_offset + 2)?,
                row.get::<_, i64>(volume_offset + 3)?,
                row.get::<_, f64>(volume_offset + 4)?,
            ));
            spend_groups.push((
                ordinal,
                row.get::<_, i64>(spend_offset)?,
                row.get::<_, f64>(spend_offset + 1)?,
                row.get::<_, f64>(spend_offset + 2)?,
                row.get::<_, i64>(spend_offset + 3)?,
                row.get::<_, f64>(spend_offset + 4)?,
                row.get::<_, f64>(spend_offset + 5)?,
            ));
        }
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
            let (where_sql, params) = build_where(&open_scope, &src.cols, has_strategy_names);
            let profit = profit_sum_sql(src);
            let (quote, group_by) = super::quote::trusted_quote_group("r", &src.cols);
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
                let ordinal = super::quote::report_ordinal_from_value(&raw);
                open_groups.push((ordinal, row.get::<_, f64>(1)?, row.get::<_, i64>(2)?));
            }
        }
    }
    let quotes = QuoteBreakdown::from_groups(groups)
        .with_traded_volume(super::TradedVolume::from_groups(volume_groups))
        .with_entry_spend(super::EntrySpend::from_groups(spend_groups));
    // Publish coverage whenever the selected mode can build a projection: always for current rates,
    // and only with an attached cache for historical rates.
    Ok(ReportTotals {
        quotes: if valuation_present {
            quotes.with_valuation(coverage.finish())
        } else {
            quotes
        },
        open: super::OpenPositions::from_groups(open_groups),
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
        let (where_sql, mut params) = build_where(&scoped, &src.cols, pass.has_strategy_names);
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
        let rec_id_select = rec_id_expr(src);
        // Sort in SQL only if this source can express the column; otherwise source order is
        // irrelevant, because the merge below reorders everything anyway.
        let order = match source_sort_expression(src, sort_col, valuation.as_ref()) {
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
    install_strategy_name_mask_function(conn, f)
        .map_err(|error| read_fail("reports: install strategy mask", error))?;
    let cols = display_columns(conn)?;
    let col = sort_column(&cols, sort_key);
    let sources = read_sources_res(conn)?;
    let has_strategy_names =
        strategy_metadata_required(f) && super::analytics::strategies_attached(conn);
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

/// Read a bounded newest-first closed-trade history for one exact chart core and coin identity set.
///
/// The caller may provide a published Report filter to retain its date, side, emulator, deletion,
/// and strategy predicates. This boundary always overwrites the core, substring coin, exact coin,
/// and row-scope fields so a stale or global filter cannot widen the chart scope. The row scope in
/// particular must keep being overwritten now that a published Report filter admits open positions
/// by default: chart history draws closed trades and nothing else.
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
    scope.core_uids = vec![core_uid];
    scope.coin.clear();
    scope.exact_coins = Some(exact_coins.to_vec());
    scope.rows = RowScope::Closed;

    let requested = limit.saturating_add(1);
    install_strategy_name_mask_function(conn, &scope).map_err(|error| read_fail(CONTEXT, error))?;
    let has_strategy_names =
        strategy_metadata_required(&scope) && super::analytics::strategies_attached(conn);
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
        let (where_sql, mut params) = build_where(&scope, &source.cols, has_strategy_names);
        let record_id = record_identity_expr(&source);
        // Money is OPTIONAL here, deliberately: `REQUIRED_COLUMNS` names none of these columns, so
        // a source that cannot produce a figure still returns every trade and the chart still draws
        // it. Both legs go through `settled_amount_expr` — the same correction the Report grid and
        // its footer apply — or a COIN-M liquidation would be off by its own entry price.
        let (profit_sql, percent_sql) = if source.cols.contains("profitbtc") {
            let profit = super::quote::settled_amount_expr("r", &source.cols, "profitbtc");
            let percent = if source.cols.contains("spentbtc") {
                let spent = super::quote::settled_amount_expr("r", &source.cols, "spentbtc");
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
        let quote_sql = super::quote::effective_ordinal_expr("r", &source.cols);
        // OPTIONAL exactly like the money columns above, and for the same reason: `REQUIRED_COLUMNS`
        // does not name it, so a source predating the column must still return every trade. It falls
        // back to 0 = REAL, which is the recoverable direction — hiding real trades on old data
        // would not be.
        let emulator_sql = if source.cols.contains("emulator") {
            "COALESCE(r.emulator, 0)"
        } else {
            "0"
        };
        let sql = format!(
            "SELECT {record_id}, r.core_uid, r.coin, r.buydate, r.closedate, \
             r.buyprice, r.sellprice, r.quantity, r.isshort, \
             {profit_sql}, {quote_sql}, {percent_sql}, {emulator_sql} \
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
            });
        }
    }
    if !compatible_source {
        return Err(super::ReadFail::NotReady);
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

/// Convert one generic Report value to an integer without accepting lossy non-integral reals.
///
/// Args:
///     value: SQLite value returned by the dynamic Report projection.
///
/// Returns:
///     Parsed integer, or `None` for NULL, blobs, malformed text, and non-integral reals.
pub(in crate::db) fn report_value_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(value) => Some(*value),
        Value::Real(value) if value.is_finite() && value.fract() == 0.0 => Some(*value as i64),
        Value::Text(value) => value.parse().ok(),
        Value::Null | Value::Blob(_) | Value::Real(_) => None,
    }
}

/// Convert one generic Report value to a finite floating-point number.
///
/// Args:
///     value: SQLite value returned by the dynamic Report projection.
///
/// Returns:
///     Parsed finite number, or `None` for NULL, blobs, malformed text, and non-finite values.
fn report_value_f64(value: &Value) -> Option<f64> {
    let value = match value {
        Value::Integer(value) => *value as f64,
        Value::Real(value) => *value,
        Value::Text(value) => value.parse().ok()?,
        Value::Null | Value::Blob(_) => return None,
    };
    value.is_finite().then_some(value)
}

/// Convert one generic Report value to its stored text identity.
///
/// Args:
///     value: SQLite value returned by the dynamic Report projection.
///
/// Returns:
///     Cloned text, or `None` for every non-text storage class.
pub(in crate::db) fn report_value_text(value: &Value) -> Option<String> {
    match value {
        Value::Text(value) => Some(value.clone()),
        Value::Null | Value::Integer(_) | Value::Real(_) | Value::Blob(_) => None,
    }
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
/// attached strategy database when available and otherwise use the signed numeric id. Both
/// strategy predicates are deliberately removed so an active checkbox or name mask does not hide
/// alternative strategies that match every other Report filter.
///
/// Args:
///     conn: Open report reader or snapshot with optional strategy attachment.
///     filter: Active Report filter; every non-strategy predicate scopes discovery.
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
    scope.strategy_name_mask.clear();
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
