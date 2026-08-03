//! Selection filters and the unified report source (`Query`, `unified_from`).

use rusqlite::types::Value;
use rusqlite::Connection;

use super::super::valuation::ValuationMode;
use super::super::{ProfitMetric, QuoteBreakdown, ReadResult, SideFilter};

/// Selection filters shared by all Analytics tabs.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// UTC Unix seconds; `from < 0` means all history. `to` is exclusive.
    pub from: i64,
    pub to: i64,
    /// Selected cores (multi-select, as in Orders); empty means all cores.
    pub cores: Vec<u64>,
    pub side: SideFilter,
    /// `None` means all, `Some(false)` means real, and `Some(true)` means emulated.
    /// A NULL column value counts as real, as it does in the Report window.
    pub emulator: Option<bool>,
    /// Scope: the SELECTED strategies as `(strategyid, core_uid)`. The list is split per
    /// core, so `None` in the second slot means "this strategy on any core" (a legacy key
    /// without one). Empty = every strategy.
    ///
    /// A LIST rather than one id because the tuner has Ctrl multi-select: the KPI matrix,
    /// the histogram and the sweep must all describe the SAME set the user sees highlighted
    /// and that Save writes to. Scoping to the clicked row alone was the bug where
    /// "plan vs fact" compared one strategy while N were selected.
    pub strategies: Vec<(i64, Option<u64>)>,
    /// Which quantity every profit figure is measured in: absolute quote money (`Quote`) or
    /// return on spent capital (`Percent`, the report's `Profit` column). Applied once in
    /// [`unified_from`]'s projected `pnl` column, so every reader below shares the choice.
    pub metric: ProfitMetric,
    /// Which conversion turns quote money into USDT: per-trade historical rates, or the latest
    /// known ones. Carried on the query rather than passed down as a parameter so every Analytics
    /// reader — summary, calendar, coin groups, the tuner source — inherits one answer; a reader
    /// that missed the parameter would render a differently converted number under the same label.
    pub valuation: ValuationMode,
}

impl Query {
    /// Resolve the "all history" sentinel: a negative `from` means the whole replica, which
    /// every single-scan tuner reader (the KPI matrix, the histogram, the sweep, the coin
    /// groups) represents as `from = 1` — the earliest possible second, distinct from the
    /// `min_closedate` floor the summary and calendar use. ONE definition of the sentinel so
    /// the coin table and the KPI matrix beside it cannot resolve "all history" to two spans.
    pub(in crate::db) fn floor_all_history(&mut self) {
        if self.from < 0 {
            self.from = 1;
        }
    }

    /// Build the period-and-filter WHERE clause for ONE source, referencing only columns
    /// that source HAS (like the Report window's `build_where`; filtering on a missing
    /// column would fail the entire SELECT). Placeholders ?1/?2 are from/to; cores, side,
    /// and emulator are integer literals from configuration, so injection is impossible.
    pub(super) fn where_sql(&self, cols: &std::collections::HashSet<String>, sid: &str) -> String {
        self.where_with_alias(WHERE_PERIOD, cols, sid, None)
    }

    /// Build the standard filter with every physical report column qualified by one row alias.
    ///
    /// Args:
    ///     cols: Columns available on the physical source.
    ///     sid: Effective strategy-id expression, already qualified when needed.
    ///     alias: SQL alias of the physical report table.
    ///
    /// Returns:
    ///     Filter safe to append after valuation joins carrying overlapping column names.
    fn where_sql_qualified(
        &self,
        cols: &std::collections::HashSet<String>,
        sid: &str,
        alias: &str,
    ) -> String {
        let period =
            format!("{alias}.closedate >= ?1 AND {alias}.closedate < ?2 AND {alias}.closedate > 0");
        self.where_with_alias(&period, cols, sid, Some(alias))
    }

    /// The same filters under a DIFFERENT row predicate.
    ///
    /// Split out because one reader asks the opposite question of every other: which rows the
    /// period predicate throws away (see [`undated_closes`]). Sharing the filter tail is what
    /// keeps that count describing the same cores, side and emulator setting the figures
    /// beside it were computed under.
    /// `sid` is the SQL that yields a row's strategy id — plain `COALESCE(strategyid,0)`, or
    /// the liquidation-aware form. It is passed in rather than built here so the scope filter
    /// and the projected column cannot disagree about which strategy a row belongs to: a row
    /// attributed in the projection but not in the filter would show up in a strategy's list
    /// and vanish from its own detail.
    pub(super) fn where_with(
        &self,
        period: &str,
        cols: &std::collections::HashSet<String>,
        sid: &str,
    ) -> String {
        self.where_with_alias(period, cols, sid, None)
    }

    /// Assemble shared row filters with optional physical-column qualification.
    ///
    /// Args:
    ///     period: Caller-supplied period predicate.
    ///     cols: Columns available on the physical source.
    ///     sid: Effective strategy-id expression.
    ///     alias: Optional table alias used for every ordinary report column.
    ///
    /// Returns:
    ///     Complete period, deletion, core, side, emulator, and strategy predicate.
    fn where_with_alias(
        &self,
        period: &str,
        cols: &std::collections::HashSet<String>,
        sid: &str,
        alias: Option<&str>,
    ) -> String {
        let has = |n: &str| cols.contains(n);
        let column = |name: &str| match alias {
            Some(alias) => format!("{alias}.{name}"),
            None => name.to_string(),
        };
        let mut w = String::from(period);
        if has("deleted") {
            w.push_str(&format!(" AND COALESCE({},0) = 0", column("deleted")));
        }
        if !self.cores.is_empty() {
            let list = self
                .cores
                .iter()
                .map(|c| (*c as i64).to_string())
                .collect::<Vec<_>>()
                .join(",");
            w.push_str(&format!(" AND {} IN ({list})", column("core_uid")));
        }
        if has("isshort") {
            match self.side {
                SideFilter::All => {}
                SideFilter::Long => {
                    w.push_str(&format!(" AND COALESCE({},0) = 0", column("isshort")))
                }
                SideFilter::Short => {
                    w.push_str(&format!(" AND COALESCE({},0) = 1", column("isshort")))
                }
            }
        }
        if has("emulator") {
            match self.emulator {
                None => {}
                Some(false) => w.push_str(&format!(" AND COALESCE({},0) = 0", column("emulator"))),
                Some(true) => w.push_str(&format!(" AND COALESCE({},0) = 1", column("emulator"))),
            }
        }
        // Scope of the SELECTED strategies: rows matching any of the (strategy × its core)
        // pairs. One row can satisfy only one pair, so no aggregate double-counts.
        if !self.strategies.is_empty() {
            if has("strategyid") {
                let terms: Vec<String> = self
                    .strategies
                    .iter()
                    .map(|(want, core)| match core {
                        Some(c) => {
                            format!(
                                "(COALESCE({sid},0) = {want} AND {} = {})",
                                column("core_uid"),
                                *c as i64
                            )
                        }
                        None => format!("COALESCE({sid},0) = {want}"),
                    })
                    .collect();
                w.push_str(&format!(" AND ({})", terms.join(" OR ")));
            } else {
                // This source cannot say which strategy a row belongs to, so it cannot
                // satisfy a strategy-scoped query: it contributes nothing, rather than
                // every row it holds.
                w.push_str(" AND 1=0");
            }
        }
        w
    }
}

/// Base columns projected from both report sources. The tuner's market fields
/// (`db::tuner::FIELDS`) are chained AUTOMATICALLY, so a new tuner field cannot be
/// omitted from the projection and make its SQL fail silently.
const UNIFIED_COLS: &[&str] = &[
    "core_uid",
    "core_name",
    "coin",
    "isshort",
    "buydate",
    "closedate",
    "profitbtc",
    "strategyid",
    "emulator",
    "spentbtc",
    "basecurrency",
];

/// Money projection resolved by quote coverage before one analytical scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::db) enum ProjectionMode {
    /// Preserve native report money for comparable scopes and lens-neutral subgroup scans.
    Native,
    /// Replace profit, spend, quote identity, and active PnL with historical USDT values.
    Usdt,
    /// The same replacement, but every trade converted at the latest known rate.
    UsdtCurrent,
    /// Use per-trade return on spent capital without historical FX.
    Percent,
}

impl ProjectionMode {
    /// Which conversion this projection applies, if it converts at all.
    ///
    /// Every decision downstream asks this rather than comparing against one variant, so adding a
    /// conversion cannot silently fall through to native money in a branch nobody updated.
    ///
    /// Returns:
    ///     The valuation mode to build SQL for, or `None` for a projection that converts nothing.
    pub(in crate::db) const fn valuation(self) -> Option<ValuationMode> {
        match self {
            Self::Usdt => Some(ValuationMode::Historical),
            Self::UsdtCurrent => Some(ValuationMode::Current),
            Self::Native | Self::Percent => None,
        }
    }

    /// Whether this projection reports money in USDT.
    ///
    /// Returns:
    ///     True for either conversion.
    pub(in crate::db) const fn is_usdt(self) -> bool {
        self.valuation().is_some()
    }

    /// The USDT projection matching one requested valuation mode.
    ///
    /// Args:
    ///     mode: Conversion the caller asked for.
    ///
    /// Returns:
    ///     The projection that applies it.
    pub(in crate::db) const fn usdt_for(mode: ValuationMode) -> Self {
        match mode {
            ValuationMode::Historical => Self::Usdt,
            ValuationMode::Current => Self::UsdtCurrent,
        }
    }
}

/// Read safe raw-money totals for one Analytics query scope.
///
/// This query intentionally ignores the active profit lens: split totals always
/// use raw `profitbtc`, while the same period, core, side, emulator, and strategy
/// predicates as the analytical source keep the scope exact.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     q: Fully resolved Analytics query with concrete period bounds.
///
/// Returns:
///     Known quote buckets, unknown and complete row counts, and optional complete USDT coverage.
///
/// Errors:
///     Returns a classified report read failure when source discovery or either aggregate pass
///     cannot complete.
pub(in crate::db) fn quote_breakdown_on(
    conn: &Connection,
    q: &Query,
) -> ReadResult<QuoteBreakdown> {
    let sources = super::super::read_sources_res(conn)?;
    let valuation_attached = super::super::valuation::is_attached(conn);
    match quote_breakdown_attempt(conn, q, &sources, valuation_attached) {
        Ok(totals) => Ok(totals),
        Err(error)
            if valuation_attached
                && super::super::valuation::prove_derived_corruption(conn, &error) =>
        {
            let _ = conn.execute(
                &format!("DETACH DATABASE {}", super::super::valuation::SCHEMA),
                [],
            );
            quote_breakdown_attempt(conn, q, &sources, false).map_err(|retry_error| {
                super::super::read_fail::read_fail(
                    "analytics: quote breakdown native retry",
                    retry_error,
                )
            })
        }
        // The guard above already performed schema attribution for this exact error.
        Err(error) => Err(super::super::read_fail::read_fail(
            "analytics: quote breakdown",
            error,
        )),
    }
}

/// Execute one complete Analytics quote-total pass with fresh accumulators.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     q: Fully resolved Analytics query.
///     sources: Physical report sources discovered from `main`.
///     include_valuation: Whether the historical mode may join the attached derived cache; the
///         current-rate mode does not depend on it.
///
/// Returns:
///     Exact quote totals, optionally carrying complete USDT coverage.
///
/// Errors:
///     Returns the underlying SQLite error from any physical-source aggregate.
fn quote_breakdown_attempt(
    conn: &Connection,
    q: &Query,
    sources: &[super::super::ReadSource],
    include_valuation: bool,
) -> rusqlite::Result<QuoteBreakdown> {
    let has_names = strategies_attached(conn);
    let mut groups = Vec::new();
    let mut coverage = super::super::valuation::CoverageAggregate::default();
    // Loop-invariant: current-rate coverage is publishable without the cache; historical coverage
    // is publishable only while the derived cache is attached.
    let valuation_present =
        q.valuation == super::super::valuation::ValuationMode::Current || include_valuation;
    for src in sources {
        if !src.cols.contains("closedate") || !src.cols.contains("profitbtc") {
            continue;
        }
        let sid = effective_sid_expr("r", &src.cols, has_names);
        let where_sql = q.where_sql(&src.cols, &sid);
        let coverage_where_sql = q.where_sql_qualified(&src.cols, &sid, "r");
        let (quote, group_by) = super::super::quote::trusted_quote_group(
            "r.basecurrency",
            src.cols.contains("basecurrency"),
        );
        let source = super::super::report_read::source_partition(src);
        let valuation = super::super::valuation::projection(
            q.valuation,
            include_valuation,
            "r",
            &src.cols,
            source,
        );
        let joins = valuation
            .as_ref()
            .map(|parts| parts.joins.as_str())
            .unwrap_or("");
        let coverage_columns = valuation
            .as_ref()
            .map(|parts| format!(", {}", parts.aggregate_columns()))
            .unwrap_or_default();
        let active_where = if valuation.is_some() {
            &coverage_where_sql
        } else {
            &where_sql
        };
        let sql = format!(
            "SELECT {quote}, COALESCE(SUM(r.profitbtc), 0.0), COUNT(*){coverage_columns} \
             FROM {} r{joins} WHERE {active_where}{group_by}",
            src.table,
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params![q.from, q.to])?;
        while let Some(row) = rows.next()? {
            let raw = row.get::<_, Value>(0)?;
            let ordinal = super::super::quote::report_ordinal_from_value(&raw);
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

/// A row's strategy id, with LIQUIDATION rows attributed to the strategy named in them.
///
/// THE ONE PLACE. It is applied inside [`unified_from`] — both in the projection and in the
/// scope filter — so the column every outer query already reads as `o.strategyid` carries the
/// attributed value, and none of them needed changing. Two readers cannot drift because there
/// is only one definition of "which strategy is this row".
///
/// A liquidation arrives with `strategyid = 0` and `channelname = 'LIQUIDATION'` exactly. The
/// exactness matters: a substring test matches 15 696 rows on the real database against 319
/// real ones, because a strategy is NAMED `Liquidations_Short_250620_184426_318` and its name
/// appears both in `channelname` and inside the `sellreason` text of its ordinary trades.
///
/// The owner's name is in `signaltype` (`MainShotS  ( MoonShot )`), with `comment` as the
/// fallback — measured at 288/319 and 304/319. Cutting at the first bracket is enough; no
/// regex, and it evaluates only for rows the CASE has already narrowed.
///
/// Returns the plain column whenever attribution is off, the strategy database is not
/// attached, or the source lacks a column this needs — a source that cannot answer must not
/// be made to guess.
///
/// Args:
///     alias: SQL alias of the report source.
///     cols: Columns available on that source.
///     on: Whether attached strategy metadata may be used.
///
/// Returns:
///     A SQL expression yielding the effective signed strategy id.
pub(in crate::db) fn effective_sid_expr(
    alias: &str,
    cols: &std::collections::HashSet<String>,
    on: bool,
) -> String {
    let plain = format!("{alias}.\"strategyid\"");
    // EVERY column the expression names. `core_uid` belongs here as much as the rest: a legacy
    // source can lack it (the projection right below emits `NULL AS "core_uid"` for exactly
    // that case), and naming it anyway makes the branch fail to PREPARE — sinking the whole
    // window the moment the switch is turned on.
    const NEEDED: [&str; 3] = ["strategyid", "channelname", "core_uid"];
    if !on || NEEDED.iter().any(|c| !cols.contains(*c)) {
        return plain;
    }
    let mut sources = Vec::new();
    for c in ["signaltype", "comment"] {
        if cols.contains(c) {
            sources.push(format!("NULLIF({alias}.\"{c}\",'')"));
        }
    }
    if sources.is_empty() {
        return plain;
    }
    sources.push("''".to_string());
    let raw = format!("COALESCE({})", sources.join(", "));
    // "MainShotS  ( MoonShot )" -> "MainShotS". The WHOLE trimmed value is tried first, so a
    // strategy whose own name contains a bracket matches itself instead of being truncated to
    // its prefix — and then possibly matching a DIFFERENT strategy that is named that prefix.
    let whole = format!("trim({raw})");
    let cut = format!(
        "trim(substr({raw}, 1, CASE WHEN instr({raw},'(') > 0 \
         THEN instr({raw},'(') - 1 ELSE length({raw}) END))"
    );
    // A name that matches nothing (a deleted strategy, or no name at all) yields 0 and the row
    // stays in "Manual" — the same place it was, which is the honest answer.
    // Preference expressed as NESTED COALESCE, not as `IN (…) ORDER BY <match> DESC`: SQLite
    // rejects a correlated reference inside a subquery's ORDER BY ("no such column: r.comment"),
    // so that form compiled, passed every string-matching test, and would have failed at
    // runtime the first time the switch was turned on.
    let lookup = |n: &str| {
        format!(
            "(SELECT st.strategy_id FROM strat.strategies st \
             WHERE st.core_uid = {alias}.\"core_uid\" AND st.deleted = 0 AND st.name = {n})"
        )
    };
    format!(
        "CASE WHEN COALESCE({alias}.\"strategyid\",0) = 0 \
         AND upper(trim(COALESCE({alias}.\"channelname\",''))) = 'LIQUIDATION' \
         THEN COALESCE({}, {}, 0) ELSE {plain} END",
        lookup(&whole),
        lookup(&cut)
    )
}

/// Build the unified replica-and-legacy `FROM` source with filters inside each
/// branch so the `closedate` indexes remain usable.
///
/// Missing columns project as NULL. `Ok(None)` means no source has received the
/// required schema; a failed schema probe remains an error because opening a
/// database does not validate its schema b-tree.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     q: Period, filters, and profit metric projected into each source branch.
///
/// Returns:
///     Unified filtered source SQL, `None` when no source is ready, or a classified failure.
pub(in crate::db) fn unified_from(conn: &Connection, q: &Query) -> ReadResult<Option<String>> {
    let mode = if q.metric == ProfitMetric::Percent {
        ProjectionMode::Percent
    } else {
        ProjectionMode::Native
    };
    unified_from_mode(conn, q, mode)
}

/// Build the unified source under one previously resolved money projection.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     q: Period and row filters.
///     mode: Native, historical USDT, current-rate USDT, or percent projection established by
///         preflight.
///
/// Returns:
///     Unified filtered source SQL, no ready source, or a classified read failure.
pub(in crate::db) fn unified_from_mode(
    conn: &Connection,
    q: &Query,
    mode: ProjectionMode,
) -> ReadResult<Option<String>> {
    let cols: Vec<&str> = UNIFIED_COLS
        .iter()
        .copied()
        .chain(super::super::tuner::FIELDS.iter().map(|s| s.col))
        .collect();
    // Attribute LIQUIDATION rows to the strategy named in the row, whenever the strategy
    // database is attached.
    //
    // A liquidation arrives with `strategyid = 0` and `channelname = 'LIQUIDATION'`, so it
    // lands in "Manual (no strategy)" and the strategy that actually took the loss never sees
    // it. The name IS there — `signaltype`/`comment` carry `MainShotS  ( MoonShot )` — and
    // matching it against `strat.strategies` by `(core_uid, name)` recovers the owner. Rows
    // that do not attach (a deleted strategy, or no parseable name) stay in "Manual" rather
    // than being guessed at.
    //
    // The Report strategy filter reuses this same expression, so opening a strategy from
    // Analytics includes the liquidation rows that contributed to its summary.
    //
    // Attachment is PROBED rather than passed down: the callers that attach the strategy
    // database do so on this same connection, and the tests deliberately do not. Not to be
    // confused with `super::super::summary`'s own `has_strat_names`, which uses the same probe
    // independently to resolve strategy display names.
    let has_names = strategies_attached(conn);
    // Percent mode measures each trade as profit ÷ spent, so a trade without a positive
    // `spentbtc` has no percent at all.
    let pct = mode == ProjectionMode::Percent;
    let mut branches = Vec::new();
    for src in super::super::read_sources_res(conn)? {
        if !src.cols.contains("closedate") || !src.cols.contains("profitbtc") {
            continue; // The core schema has not arrived yet, so there is nothing to aggregate.
        }
        // A source without `spentbtc` (e.g. the legacy `closed_sell_reports`) can form no
        // percent, so in percent mode it contributes NOTHING — not a column of NULLs that the
        // COUNT(*)/COALESCE(pnl,0) consumers below would miscount as zero-profit losing trades.
        if pct && !src.cols.contains("spentbtc") {
            continue;
        }
        // The branch table is aliased so the attribution's correlated subquery can name the
        // OUTER row explicitly. Unqualified `core_uid` inside it would bind to `strat.strategies`
        // and silently match every strategy of that name on any core.
        let sid = effective_sid_expr("r", &src.cols, has_names);
        let source = super::super::report_read::source_partition(&src);
        // `true` for attachment: a USDT projection is only ever chosen after `scope_decision_on`
        // proved coverage on this same snapshot, and the current-rate mode needs no cache at all.
        let valuation = mode.valuation().and_then(|valuation_mode| {
            super::super::valuation::projection(valuation_mode, true, "r", &src.cols, source)
        });
        let proj = cols
            .iter()
            .map(|c| {
                if *c == "strategyid" && src.cols.contains(*c) {
                    // The attributed value is published UNDER THE ORIGINAL NAME, so every
                    // query outside this source keeps reading `o.strategyid` and gets it.
                    format!("{sid} AS \"strategyid\"")
                } else if mode.is_usdt() && *c == "profitbtc" {
                    format!(
                        "{} AS \"profitbtc\"",
                        valuation
                            .as_ref()
                            .expect("USDT projection has coverage SQL")
                            .profit_usdt
                    )
                } else if mode.is_usdt() && *c == "spentbtc" {
                    format!(
                        "{} AS \"spentbtc\"",
                        valuation
                            .as_ref()
                            .expect("USDT projection has coverage SQL")
                            .spent_usdt
                    )
                } else if mode.is_usdt() && *c == "basecurrency" {
                    "1 AS \"basecurrency\"".to_string()
                } else if src.cols.contains(*c) {
                    format!("r.\"{c}\"")
                } else {
                    format!("NULL AS \"{c}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        // The active profit metric, projected ONCE here as `pnl` so every aggregation below and
        // the tuner sweep read one column. `Quote` is raw money; `Percent` is the report's
        // `Profit` column (profit ÷ spent × 100 = return on spent capital). The `spentbtc > 0`
        // filter appended below (percent mode) guarantees the ratio is finite and non-NULL, so
        // COUNT, SUM, wins, streaks and the top list all describe the SAME set of trades. Sign is
        // preserved (spent > 0), so win/loss, profit factor and best/worst stay correct on `pnl`.
        let pnl = if pct {
            "r.\"profitbtc\" / r.\"spentbtc\" * 100.0 AS \"pnl\"".to_string()
        } else if let Some(parts) = &valuation {
            format!("{} AS \"pnl\"", parts.profit_usdt)
        } else {
            "r.\"profitbtc\" AS \"pnl\"".to_string()
        };
        let mut where_sql = if valuation.is_some() {
            q.where_sql_qualified(&src.cols, &sid, "r")
        } else {
            q.where_sql(&src.cols, &sid)
        };
        if pct {
            where_sql.push_str(" AND spentbtc > 0");
        }
        if let Some(parts) = &valuation {
            where_sql.push_str(&format!(" AND ({})", parts.valued));
        }
        let joins = valuation
            .as_ref()
            .map(|parts| parts.joins.as_str())
            .unwrap_or("");
        branches.push(format!(
            "SELECT {proj}, {pnl} FROM {} r{joins} WHERE {where_sql}",
            src.table,
        ));
    }
    if branches.is_empty() {
        Ok(None)
    } else {
        Ok(Some(format!("({}) o", branches.join(" UNION ALL "))))
    }
}

const WHERE_PERIOD: &str = "closedate >= ?1 AND closedate < ?2 AND closedate > 0";

/// Rows a period can never contain: the close date is absent or non-positive.
pub(super) const WHERE_UNDATED: &str = "(closedate IS NULL OR closedate <= 0)";

/// Is the strategy database available on this connection?
///
/// Probed rather than tracked: `open_reader` attaches it, but a caller may hand us a
/// connection it opened itself, and a second ATTACH under the same alias fails. Asking the
/// connection is the only answer that cannot go stale.
///
/// Args:
///     conn: SQLite connection that may carry the `strat` attachment.
///
/// Returns:
///     Whether the attached strategy table can execute a read.
pub(in crate::db) fn strategies_attached(conn: &Connection) -> bool {
    // EXECUTED, not merely prepared. `prepare` validates the schema and nothing else, so a
    // corrupt or unreadable strategies.sqlite passed this check and then failed mid-scan —
    // and because the attribution subquery is baked into the unified source, that failure
    // sank the whole summary instead of degrading to "no attribution". Running the statement
    // is what turns a broken strategy database back into a silently absent one.
    match conn.query_row("SELECT 1 FROM strat.strategies LIMIT 1", [], |_| Ok(())) {
        Ok(()) => true,
        // An EMPTY table is still a usable one: nothing will match, every liquidation stays
        // in "Manual", and that is a correct answer rather than a failure.
        Err(rusqlite::Error::QueryReturnedNoRows) => true,
        Err(e) => {
            log::debug!("analytics: strategies.sqlite unreadable, attribution off: {e}");
            false
        }
    }
}

/// Attach the optional strategies database used to enrich strategy names.
pub(in crate::db) fn attach_strategies(conn: &Connection) -> bool {
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return false;
    }
    let sql = format!(
        "ATTACH DATABASE '{}' AS strat",
        path.to_string_lossy().replace('\'', "''")
    );
    // An absent file is normal and silent; failure to attach an existing file
    // is logged as a real enrichment fault.
    match conn.execute(&sql, []) {
        Ok(_) => true,
        Err(e) => {
            // Logged ONCE. It now runs per reader (every Report refresh included), and a
            // permanently broken file would otherwise write a warn line forever.
            use std::sync::Once;
            static SAID: Once = Once::new();
            SAID.call_once(|| log::warn!("analytics: strategies.sqlite did not attach: {e}"));
            false
        }
    }
}

/// Strategy scope, liquidation-attribution expression, and the flag-wiring contract.
#[cfg(test)]
mod tests;
