//! Aggregates for the Analytics window over the `orders_rep` report replica.
//!
//! Every function scans SQLite for the complete requested period and must run
//! on a background executor so the UI thread never waits for disk. The source
//! combines the typed replica with the legacy
//! `closed_sell_reports` table while that read-only table still exists. Typed
//! syncs progressively purge its per-core rows, matching the Report panel.
//! Reads use a dedicated connection
//! (`open_reader`). Strategy names come from an attached `strategies.sqlite`,
//! joining the signed `strategyid` and `strategies.strategy_id`; without that
//! database, results use the numeric id.
//!
//! Period bounds are UTC Unix seconds with an exclusive `to`. Queries include
//! only closed, non-deleted trades (`closedate > 0`, `deleted = 0`). Raw profit is
//! `profitbtc`, denominated in each persisted row's pair quote currency.

use rusqlite::Connection;

use super::metrics::{profit_factor, winrate};
use super::read_fail::read_fail_on;
use super::{
    ProfitMetric, ProfitScope, ProfitUnit, QuoteBreakdown, QuoteScope, ReadFail, ReadResult,
};

mod calendar;
mod groups;
mod query;

pub use calendar::{
    calendar_cells, calendar_hours, hourly_profiles, CalendarPeriod, DayCell, HourStat,
};
pub use groups::{coin_groups, strategies_for_coins, GroupStat, KindCore, KindStat, TopTrade};
pub use query::Query;

pub(in crate::db) use groups::{coin_groups_on, strategies_for_coins_on};
use groups::{groups, kind_stats, top_trades};
use query::ProjectionMode;
use query::WHERE_UNDATED;
pub(in crate::db) use query::{
    attach_strategies, effective_sid_expr, quote_breakdown_on, strategies_attached, unified_from,
};
pub(in crate::db) use query::{unified_from_mode, ProjectionMode as ResolvedProjectionMode};

/// Period totals: counters and metrics computed from the trade sequence ordered by
/// `closedate`, including profit factor, maximum drawdown, streaks, and duration.
#[derive(Clone, Debug, Default)]
pub struct PeriodStats {
    pub n: i64,
    pub wins: i64,
    pub losses: i64,
    pub profit: f64,
    /// Sum of wins divided by the sum of losses (99 when there are wins but no losses).
    pub pf: f64,
    pub avg: f64,
    /// Maximum drawdown of the cumulative profit curve over the period.
    pub max_dd: f64,
    pub win_streak: i64,
    pub loss_streak: i64,
    /// Average trade duration in minutes, computed as `closedate - buydate`.
    pub avg_dur_min: f64,
}

impl PeriodStats {
    pub fn winrate(&self) -> f64 {
        winrate(self.wins, self.n)
    }
}

/// A point in an hourly, daily, or weekly series; see `Summary::bucket_secs`.
#[derive(Clone, Debug)]
pub struct DayPoint {
    /// Bucket start in UTC Unix seconds.
    pub start: i64,
    pub profit: f64,
    pub trades: i64,
}

/// One core's profit and trade-count series in the same buckets as `Summary::days`.
#[derive(Clone, Debug)]
pub struct CoreSeries {
    pub uid: u64,
    pub name: String,
    pub per_bucket: Vec<f64>,
    /// Trades in the SAME bucket — same grid and same length as `per_bucket`, filled in
    /// one pass with it (the Summary popups print profit and count on one line).
    pub per_bucket_trades: Vec<i64>,
    pub total: f64,
    /// The core's trades over the whole period = sum of `per_bucket_trades`.
    pub trades: i64,
}

/// Data for the Summary tab, returned as one aggregate result.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub cur: PeriodStats,
    /// The preceding period of equal length, used for KPI comparisons.
    ///
    /// A failed comparison scan is non-fatal and yields `None`, preserving a
    /// readable current period while making its unavailable deltas explicit.
    pub prev: Option<PeriodStats>,
    /// Series bucket size: one hour for periods up to a day, one day normally, or one week
    /// for very long periods.
    pub bucket_secs: i64,
    pub days: Vec<DayPoint>,
    pub best: Vec<TopTrade>,
    pub worst: Vec<TopTrade>,
    /// Strategy groups keyed by `strategyid@core_uid`, so the same id on different cores
    /// stays separate and distinct strategies sharing a name do not merge; profit descends.
    pub strategies: Vec<GroupStat>,
    /// Coin groups sorted by descending profit.
    pub coins: Vec<GroupStat>,
    /// Per-core series on the `days` grid, sorted by descending total profit, for the
    /// Summary charts and their popups.
    pub core_days: Vec<CoreSeries>,
    /// Most profitable UTC hour as `(hour, profit, trades)`.
    pub best_hour: Option<(u32, f64, i64)>,
    /// Profit per strategy type, descending. Filled only for periods of one day or less.
    /// The main series is hourly for those periods, daily through 400 days, and weekly
    /// for periods longer than 400 days.
    pub kinds: Vec<KindStat>,
    /// Cores found in the replica for the filter combo box, WITHOUT applying filters so a
    /// selection cannot collapse the list.
    pub cores: Vec<(u64, String)>,
    /// Effective period bounds after resolving the All period.
    pub from: i64,
    pub to: i64,
}

/// Closed trades the core never dated, split safely by quote currency.
///
/// Every analytics figure is computed under [`WHERE_PERIOD`], which ends in `closedate > 0`.
/// So these rows are in NO period — not even "all history" — and their profit is simply
/// absent from every number this window shows. One measured USDT replica had 370 such trades
/// carrying -434 USDT: closed for certain (they carry a sell price and a sell reason) and
/// invisible everywhere.
///
/// This is a CORE-side omission, not a replica bug: `db::rep::open_row_ids` already reports
/// such rows back for re-checking and they stay undated regardless. The terminal cannot
/// invent the missing timestamp — the only honest thing it can do is say how much is missing,
/// which is what this returns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UndatedCloses {
    /// Raw-money totals and complete row count for undated closes.
    pub totals: QuoteBreakdown,
}

/// Independently classified data rendered from one report snapshot.
pub struct AnalyticsRead<T> {
    /// Primary surface payload.
    pub data: ReadResult<T>,
    /// Closed rows excluded from every dated period.
    pub undated: ReadResult<UndatedCloses>,
    /// Optional throttled core-selector refresh.
    pub cores: Option<ReadResult<Vec<(u64, String)>>>,
}

/// Calendar payload and its always-visible metadata from one report snapshot.
pub struct CalendarRead {
    /// Visible Calendar cells and optional comparison aggregate.
    pub period: ReadResult<ProfitScope<CalendarPeriod>>,
    /// Closed rows excluded from every dated Calendar cell.
    pub undated: ReadResult<UndatedCloses>,
    /// Optional throttled core-selector refresh.
    pub cores: Option<ReadResult<Vec<(u64, String)>>>,
}

/// Minimal base required by the Strategies tab.
#[derive(Clone, Debug, Default)]
pub struct StrategyBase {
    /// Strategy rows keyed by strategy and core.
    pub strategies: Vec<GroupStat>,
    /// Total trades represented by the strategy rows.
    pub trades: i64,
    /// Coin groups used by the By-coin axis.
    pub coins: Vec<GroupStat>,
    /// Effective lower period bound.
    pub from: i64,
    /// Effective exclusive upper period bound.
    pub to: i64,
}

impl UndatedCloses {
    /// Nothing to report — the usual case, and the one the UI must stay silent about.
    ///
    /// Returns:
    ///     `true` when no undated closed rows exist in the selected scope.
    pub fn is_empty(&self) -> bool {
        self.totals.orders == 0
    }
}

/// Count them under the CURRENT filters but outside any period — see [`UndatedCloses`].
///
/// Args:
///     q: Active Analytics filters; period bounds are intentionally replaced.
///
/// Returns:
///     Safe per-quote undated totals or a classified read failure.
pub fn undated_closes(q: &Query) -> ReadResult<UndatedCloses> {
    let conn = super::open_reader()?;
    super::with_read_snapshot(&conn, |snapshot| undated_closes_on(snapshot, q))
}

/// Count undated closes on an existing report snapshot.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     q: Active Analytics filters; period bounds are intentionally replaced.
///
/// Returns:
///     Safe per-quote undated totals or a classified read failure.
fn undated_closes_on(conn: &Connection, q: &Query) -> ReadResult<UndatedCloses> {
    const CTX: &str = "analytics: trades with no close date";
    let mut groups = Vec::new();
    for src in super::read_sources_res(conn)? {
        // A source without these columns cannot hold such a row, and asking would fail the
        // whole statement rather than contribute zero.
        if !src.cols.contains("closedate") || !src.cols.contains("profitbtc") {
            continue;
        }
        // Attribution deliberately OFF here: the banner counts rows the PERIOD threw away,
        // and which strategy they belong to changes nothing about that count. (The caller
        // passes no strategy scope either, so the expression is never consulted.)
        let sid = effective_sid_expr("r", &src.cols, false);
        let (quote, group_by) =
            super::quote::trusted_quote_group("r.basecurrency", src.cols.contains("basecurrency"));
        let sql = format!(
            "SELECT {quote}, COALESCE(SUM(r.profitbtc), 0), COUNT(*) \
             FROM {} r WHERE {}{group_by}",
            src.table,
            q.where_with(WHERE_UNDATED, &src.cols, &sid)
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
        let rows = stmt
            .query_map([], |row| {
                let ordinal = super::quote::report_ordinal_from_value(
                    &row.get::<_, rusqlite::types::Value>(0)?,
                );
                Ok((ordinal, row.get::<_, f64>(1)?, row.get::<_, i64>(2)?))
            })
            .map_err(|e| read_fail_on(conn, CTX, e))?;
        for row in rows {
            groups.push(row.map_err(|e| read_fail_on(conn, CTX, e))?);
        }
    }
    Ok(UndatedCloses {
        totals: QuoteBreakdown::from_groups(groups),
    })
}

/// Read Summary and its undated-close warning from one committed snapshot.
///
/// Args:
///     q: Summary scope and period.
///     read_cores: Whether the shared core-selector cadence is due.
///
/// Returns:
///     Independently classified visible results from one snapshot.
pub fn summary_data(q: &Query, read_cores: bool) -> AnalyticsRead<ProfitScope<Summary>> {
    let compound = super::open_reader().and_then(|conn| {
        let has_strat_names = strategies_attached(&conn);
        super::with_read_snapshot(&conn, |snapshot| {
            Ok(AnalyticsRead {
                data: summary_on(snapshot, q, has_strat_names, false),
                undated: undated_closes_on(snapshot, q),
                cores: read_cores.then(|| super::distinct_cores(snapshot)),
            })
        })
    });
    match compound {
        Ok(data) => data,
        Err(error) => AnalyticsRead {
            data: Err(error.clone()),
            undated: Err(error.clone()),
            cores: read_cores.then_some(Err(error)),
        },
    }
}

/// Read only the list/coin base needed by Strategies, plus its undated warning.
///
/// Args:
///     q: Strategies scope and period.
///     read_cores: Whether the shared core-selector cadence is due.
///
/// Returns:
///     Independently classified visible results from one snapshot.
pub fn strategy_base_data(q: &Query, read_cores: bool) -> AnalyticsRead<ProfitScope<StrategyBase>> {
    let compound = super::open_reader().and_then(|conn| {
        let has_strat_names = strategies_attached(&conn);
        super::with_read_snapshot(&conn, |snapshot| {
            Ok(AnalyticsRead {
                data: strategy_base_on(snapshot, q, has_strat_names),
                undated: undated_closes_on(snapshot, q),
                cores: read_cores.then(|| super::distinct_cores(snapshot)),
            })
        })
    });
    match compound {
        Ok(data) => data,
        Err(error) => AnalyticsRead {
            data: Err(error.clone()),
            undated: Err(error.clone()),
            cores: read_cores.then_some(Err(error)),
        },
    }
}

/// Read Calendar cells and visible report metadata from one committed snapshot.
///
/// Args:
///     q: Visible Calendar scope and time range.
///     previous: Optional comparison scope.
///     hourly: Whether Calendar renders hour buckets.
///     read_cores: Whether the throttled core selector is due for refresh.
///
/// Returns:
///     Independently classified Calendar, undated, and optional core results.
pub fn calendar_data(
    q: &Query,
    previous: Option<&Query>,
    hourly: bool,
    read_cores: bool,
) -> CalendarRead {
    let compound = super::open_reader().and_then(|conn| {
        super::with_read_snapshot(&conn, |snapshot| {
            Ok(CalendarRead {
                period: calendar::calendar_period_from(snapshot, q, previous, hourly),
                undated: undated_closes_on(snapshot, q),
                cores: read_cores.then(|| super::distinct_cores(snapshot)),
            })
        })
    });
    match compound {
        Ok(data) => data,
        Err(error) => CalendarRead {
            period: Err(error.clone()),
            undated: Err(error.clone()),
            cores: read_cores.then_some(Err(error)),
        },
    }
}

/// Preflight result that prevents unsafe raw-money analytics from carrying scalar data.
enum ScopeDecision {
    /// Scalar values share this explicit unit.
    Comparable {
        /// Exact unit published to UI consumers.
        unit: ProfitUnit,
        /// Row-level money projection used by every SQL consumer.
        projection: ProjectionMode,
    },
    /// The query has no rows and therefore no quote to infer.
    Empty {
        /// Projection retained for empty-source query construction.
        projection: ProjectionMode,
    },
    /// Raw money is mixed or unknown and only split/count totals are safe.
    Split(QuoteBreakdown),
}

/// Resolve whether one query can produce comparable scalar profit values.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     q: Query with concrete period bounds.
///
/// Returns:
///     Percent comparability, one exact quote, fully valued mixed USDT, a legitimate empty scope,
///     or split totals.
///
/// Errors:
///     Returns a classified report read failure when quote or valuation preflight cannot complete.
fn scope_decision_on(conn: &Connection, q: &Query) -> ReadResult<ScopeDecision> {
    if q.metric == ProfitMetric::Percent {
        return Ok(ScopeDecision::Comparable {
            unit: ProfitUnit::Percent,
            projection: ProjectionMode::Percent,
        });
    }
    let totals = quote_breakdown_on(conn, q)?;
    Ok(match totals.scope() {
        QuoteScope::Empty => ScopeDecision::Empty {
            projection: ProjectionMode::Native,
        },
        QuoteScope::Single(currency) => ScopeDecision::Comparable {
            unit: ProfitUnit::Quote(currency),
            projection: ProjectionMode::Native,
        },
        QuoteScope::Mixed if totals.unified_usdt().is_some() => ScopeDecision::Comparable {
            unit: ProfitUnit::Quote(crate::db::QuoteCurrency::usdt()),
            projection: ProjectionMode::Usdt,
        },
        QuoteScope::Mixed | QuoteScope::Unknown => ScopeDecision::Split(totals),
    })
}

/// Resolve one query to a safe row-level projection for non-`ProfitScope` consumers.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     q: Concrete Analytics query.
///
/// Returns:
///     Native, historical USDT, or percent projection.
///
/// Errors:
///     Returns `IncomparableQuote` for partial or unknown raw-money scopes, or propagates a
///     classified report read failure from quote and valuation preflight.
pub(in crate::db) fn projection_mode_on(
    conn: &Connection,
    q: &Query,
) -> ReadResult<ResolvedProjectionMode> {
    scope_decision_on(conn, q)?
        .projection()
        .ok_or(ReadFail::IncomparableQuote)
}

/// Resolve lens-neutral group money without hiding safe native single-quote subgroups.
///
/// A completely valued mixed scope uses USDT. Partial or unknown overall scopes remain native so
/// individual homogeneous groups can still publish exact raw quote columns in Percent mode.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     q: Concrete query whose metric is ignored.
///
/// Returns:
///     Historical USDT for fully valued mixed known scopes, native money otherwise.
///
/// Errors:
///     Returns a classified report read failure when quote or valuation preflight cannot complete.
fn raw_money_projection_on(conn: &Connection, q: &Query) -> ReadResult<ProjectionMode> {
    let mut raw = q.clone();
    raw.metric = ProfitMetric::Quote;
    let totals = quote_breakdown_on(conn, &raw)?;
    Ok(
        if matches!(totals.scope(), QuoteScope::Mixed) && totals.unified_usdt().is_some() {
            ProjectionMode::Usdt
        } else {
            ProjectionMode::Native
        },
    )
}

impl ScopeDecision {
    /// Projection established by this comparable or empty preflight.
    ///
    /// Returns:
    ///     Native, historical USDT, or percent mode; split scopes return no projection.
    fn projection(&self) -> Option<ProjectionMode> {
        match self {
            Self::Comparable { projection, .. } | Self::Empty { projection } => Some(*projection),
            Self::Split(_) => None,
        }
    }
}

/// Attach a preflight decision to a completed scalar payload.
///
/// Args:
///     decision: Previously resolved comparability state.
///     data: Scalar payload built only for comparable or empty scopes.
///
/// Returns:
///     A typed Analytics payload; split decisions retain no scalar data.
fn scoped<T>(decision: ScopeDecision, data: T) -> ProfitScope<T> {
    match decision {
        ScopeDecision::Comparable { unit, .. } => ProfitScope::Comparable { unit, data },
        ScopeDecision::Empty { .. } => ProfitScope::Empty(data),
        ScopeDecision::Split(totals) => ProfitScope::Split(totals),
    }
}

/// Build the Strategies base without the Summary-only series and rankings.
///
/// Args:
///     conn: Pinned report snapshot.
///     q: Active report filters and period.
///     has_strat_names: Whether the optional strategy-name database is readable.
///
/// Returns:
///     Comparable or empty strategy data, split totals, or a classified read failure.
fn strategy_base_on(
    conn: &Connection,
    q: &Query,
    has_strat_names: bool,
) -> ReadResult<ProfitScope<StrategyBase>> {
    let mut q = q.clone();
    if q.from < 0 {
        q.from = min_closedate(conn)?;
    }
    let decision = match scope_decision_on(conn, &q)? {
        ScopeDecision::Split(totals) => return Ok(ProfitScope::Split(totals)),
        decision => decision,
    };
    let projection = decision
        .projection()
        .expect("split decisions return before source construction");
    let Some(src) = unified_from_mode(conn, &q, projection)? else {
        return Err(ReadFail::NotReady);
    };
    let raw_src = groups::raw_source(conn, &q)?;
    let mut names = has_strat_names;
    let strategies = with_name_fallback(&mut names, |enabled| {
        groups(conn, &src, raw_src.as_deref(), &q, enabled, true)
    })?;
    let trades = strategies.iter().map(|strategy| strategy.n).sum();
    let data = StrategyBase {
        strategies,
        trades,
        coins: with_name_fallback(&mut names, |enabled| {
            groups(conn, &src, raw_src.as_deref(), &q, enabled, false)
        })?,
        from: q.from,
        to: q.to,
    };
    Ok(scoped(decision, data))
}

/// Aggregate the selected period and filters without collapsing read failures.
///
/// Returns `NotReady` when the replica or required core schema is absent, and
/// `Failed` when a required current-period filesystem or SQLite read fails. A
/// failed comparison scan leaves `Summary::prev` absent. A successful empty
/// period has zero current-period counters.
///
/// Args:
///     q: Active Analytics filters, period, and profit metric.
///
/// Returns:
///     Comparable or empty Summary data, split totals, or a classified read failure.
pub fn summary(q: &Query) -> ReadResult<ProfitScope<Summary>> {
    let conn = super::open_reader()?;
    // `open_reader` has already attached it (a second ATTACH under the same alias fails).
    let has_strat_names = strategies_attached(&conn);
    // One snapshot for the whole summary. Its counters, series, top trades and
    // group tables are separate statements, and the writer commits between them
    // during catch-up — without this they could disagree about which trades the
    // period contains while being published as one coherent result.
    let snap = super::read_snapshot(&conn)?;
    summary_on(&snap, q, has_strat_names, true)
}

/// Aggregate a summary on an existing connection for tests and [`summary`].
///
/// `has_strat_names` is supplied so tests do not attach the developer's real
/// strategies database. `read_cores` skips the throttled selector scan for
/// compound UI reads. Missing required source schema returns `NotReady`; schema,
/// query, and row failures return `Failed`.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     q: Active Analytics filters, period, and profit metric.
///     has_strat_names: Whether the optional strategy-name database is readable.
///     read_cores: Whether to include the core selector scan.
///
/// Returns:
///     Comparable or empty Summary data, split totals, or a classified read failure.
pub(super) fn summary_on(
    conn: &Connection,
    q: &Query,
    has_strat_names: bool,
    read_cores: bool,
) -> ReadResult<ProfitScope<Summary>> {
    let mut q = q.clone();
    if q.from < 0 {
        q.from = min_closedate(conn)?;
    }
    let decision = match scope_decision_on(conn, &q)? {
        ScopeDecision::Split(totals) => return Ok(ProfitScope::Split(totals)),
        decision => decision,
    };
    let projection = decision
        .projection()
        .expect("split decisions return before source construction");
    let Some(src) = unified_from_mode(conn, &q, projection)? else {
        return Err(ReadFail::NotReady);
    };
    let len = (q.to - q.from).max(1);
    // A day or less: a daily series would be a single bar and a cumulative curve a single
    // point, so the grid drops to HOURS — the same series, the same charts, just a scale
    // that has something to show. `core_series` follows this bucket, so the per-core lines
    // and every popup come along for free.
    let one_day = len <= 86_400;
    // The series uses hourly buckets for periods up to a day, daily buckets normally, and
    // weekly buckets for a multi-year All period to avoid thousands of bars.
    let bucket = if one_day {
        3_600
    } else if len / 86_400 > 400 {
        7 * 86_400
    } else {
        86_400
    };

    let (cur, days, hours) = scan_period(conn, &src, q.from, q.to, bucket)?;
    // The comparison is best-effort because it must not hide a readable current
    // period; see `Summary::prev`.
    let prev = previous_stats(conn, &src, &q, len, bucket, &decision);

    let best_hour = hours
        .iter()
        .enumerate()
        .filter(|(_, (p, n))| *n > 0 && *p > 0.0)
        .max_by(|a, b| a.1 .0.total_cmp(&b.1 .0))
        .map(|(h, (p, n))| (h as u32, *p, *n));

    let core_days = core_series(conn, &src, &q, &days, bucket)?;
    // Shared latch: the first name-related failure turns enrichment off for the
    // remaining aggregations of THIS summary.
    let mut names = has_strat_names;
    let raw_src = groups::raw_source(conn, &q)?;
    let data = Summary {
        cur,
        prev,
        bucket_secs: bucket,
        days,
        core_days,
        best: with_name_fallback(&mut names, |n| top_trades(conn, &src, &q, n, true))?,
        worst: with_name_fallback(&mut names, |n| top_trades(conn, &src, &q, n, false))?,
        strategies: with_name_fallback(&mut names, |n| {
            groups(conn, &src, raw_src.as_deref(), &q, n, true)
        })?,
        coins: with_name_fallback(&mut names, |n| {
            groups(conn, &src, raw_src.as_deref(), &q, n, false)
        })?,
        best_hour,
        // Only for a day or less — on a longer period the daily chart works and this
        // aggregation's per-row subquery would be paid for nothing.
        kinds: if one_day {
            with_name_fallback(&mut names, |n| kind_stats(conn, &src, &q, n))?
        } else {
            Vec::new()
        },
        cores: if read_cores {
            super::distinct_cores(conn)?
        } else {
            Vec::new()
        },
        from: q.from,
        to: q.to,
    };
    Ok(scoped(decision, data))
}

/// Read a comparison period only when its unit is compatible with the current period.
///
/// Args:
///     conn: Pinned report snapshot.
///     src: Unified source for the current query filters.
///     q: Current query.
///     len: Current period length in seconds.
///     bucket: Series bucket size.
///     current: Current-period comparability decision.
///
/// Returns:
///     Comparable previous stats, or `None` when the read fails or quote identity differs.
fn previous_stats(
    conn: &Connection,
    src: &str,
    q: &Query,
    len: i64,
    bucket: i64,
    current: &ScopeDecision,
) -> Option<PeriodStats> {
    if q.metric == ProfitMetric::Quote {
        let mut previous = q.clone();
        previous.from = q.from - len;
        previous.to = q.from;
        let compatible = match current {
            ScopeDecision::Comparable {
                unit: ProfitUnit::Quote(current_quote),
                projection: ProjectionMode::Native,
            } => matches!(
                scope_decision_on(conn, &previous).ok()?,
                ScopeDecision::Comparable {
                    unit: ProfitUnit::Quote(previous_quote),
                    projection: ProjectionMode::Native,
                } if *current_quote == previous_quote
            ),
            ScopeDecision::Comparable {
                unit: ProfitUnit::Quote(current_quote),
                projection: ProjectionMode::Usdt,
            } if *current_quote == crate::db::QuoteCurrency::usdt() => {
                let totals = quote_breakdown_on(conn, &previous).ok()?;
                totals.unknown_orders == 0 && totals.unified_usdt().is_some()
            }
            _ => false,
        };
        if !compatible {
            return None;
        }
    }
    scan_period(conn, src, q.from - len, q.from, bucket)
        .map(|(stats, _, _)| stats)
        .ok()
}

/// Run a query that may join the ATTACHed strategies DB, retrying WITHOUT names
/// if it fails while names were in play.
///
/// `strategies.sqlite` is optional enrichment: a successful ATTACH does not
/// prove it is readable, and a failing scalar subquery against it must degrade
/// strategy labels to bare ids — not sink an otherwise healthy reports summary.
/// A failure of the reports DB itself simply fails again on the retry.
fn with_name_fallback<T>(
    names: &mut bool,
    mut run: impl FnMut(bool) -> ReadResult<T>,
) -> ReadResult<T> {
    match run(*names) {
        // Retry on ANY failure while names are in play, permanent included:
        // these statements also read the ATTACHed strategies DB, and the error
        // carries no database provenance, so corruption first met there would
        // otherwise sink a perfectly readable reports summary. The latch below
        // caps the cost at ONE extra scan per summary, not four.
        Err(e) if *names => {
            log::warn!("analytics: strategy names unavailable, retrying with bare ids: {e}");
            // Latch it off: without this, all four aggregations of one summary
            // each pay a failed scan before falling back.
            *names = false;
            run(false)
        }
        other => other,
    }
}

/// Find the earliest `closedate` across both sources for the all-time period.
///
/// A failed probe remains an error; `1` is reserved for a genuinely empty history.
fn min_closedate(conn: &Connection) -> ReadResult<i64> {
    const CTX: &str = "analytics: min_closedate";
    let mut min = i64::MAX;
    for src in super::read_sources_res(conn)? {
        if !src.cols.contains("closedate") {
            continue;
        }
        let sql = format!(
            "SELECT MIN(closedate) FROM {} WHERE closedate > 0",
            src.table
        );
        let got: Option<i64> = conn
            .query_row(&sql, [], |r| r.get::<_, Option<i64>>(0))
            .map_err(|e| read_fail_on(conn, CTX, e))?;
        if let Some(v) = got {
            min = min.min(v);
        }
    }
    // No rows anywhere is a legitimate empty history, not a failure.
    Ok(if min == i64::MAX { 1 } else { min })
}

/// Scan period trades by `closedate` into sequence metrics, buckets, and UTC hours.
///
/// `src` is the filtered source built by [`unified_from`]. Any query or
/// row-conversion failure aborts the complete aggregation.
fn scan_period(
    conn: &Connection,
    src: &str,
    from: i64,
    to: i64,
    bucket: i64,
) -> ReadResult<(PeriodStats, Vec<DayPoint>, [(f64, i64); 24])> {
    const CTX: &str = "analytics: scan_period";
    let mut st = PeriodStats::default();
    let mut days: Vec<DayPoint> = Vec::new();
    let mut hours = [(0.0f64, 0i64); 24];
    let sql = format!(
        "SELECT o.closedate, COALESCE(o.buydate, o.closedate), COALESCE(o.pnl, 0)
         FROM {src} ORDER BY o.closedate"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![from, to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })
        .map_err(|e| read_fail_on(conn, CTX, e))?;

    let (mut wsum, mut lsum) = (0.0f64, 0.0f64);
    let (mut cum, mut peak) = (0.0f64, 0.0f64);
    let (mut cur_w, mut cur_l) = (0i64, 0i64);
    let mut dur_ms_total = 0i64;
    for row in rows {
        // Every column here moves a number, so no row is skippable: dropping
        // one would silently understate n / profit / pf / max_dd.
        let (close, buy, profit) = row.map_err(|e| read_fail_on(conn, CTX, e))?;
        st.n += 1;
        st.profit += profit;
        dur_ms_total += (close - buy).max(0);
        if profit > 0.0 {
            st.wins += 1;
            wsum += profit;
            cur_w += 1;
            cur_l = 0;
            st.win_streak = st.win_streak.max(cur_w);
        } else {
            st.losses += 1;
            lsum -= profit;
            cur_l += 1;
            cur_w = 0;
            st.loss_streak = st.loss_streak.max(cur_l);
        }
        cum += profit;
        peak = peak.max(cum);
        st.max_dd = st.max_dd.max(peak - cum);

        let start = close.div_euclid(bucket) * bucket;
        match days.last_mut() {
            Some(d) if d.start == start => {
                d.profit += profit;
                d.trades += 1;
            }
            _ => days.push(DayPoint {
                start,
                profit,
                trades: 1,
            }),
        }
        let h = close.rem_euclid(86_400) / 3600;
        let slot = &mut hours[h as usize];
        slot.0 += profit;
        slot.1 += 1;
    }
    if st.n > 0 {
        st.avg = st.profit / st.n as f64;
        st.avg_dur_min = dur_ms_total as f64 / st.n as f64 / 60.0;
        st.pf = profit_factor(wsum, lsum);
    }
    // Fill series gaps (days without trades) with zeroes so bars stay evenly spaced in time.
    if !days.is_empty() {
        let mut filled = Vec::with_capacity(days.len());
        let mut t = days[0].start;
        let mut it = days.into_iter().peekable();
        while let Some(d) = it.peek() {
            if d.start == t {
                filled.push(it.next().unwrap());
            } else {
                filled.push(DayPoint {
                    start: t,
                    profit: 0.0,
                    trades: 0,
                });
            }
            t += bucket;
        }
        days = filled;
    }
    Ok((st, days, hours))
}

/// Scan core profit AND trade count into the `days` buckets, sorting profitable cores
/// first. Both series share one pass and one grid, so a bucket's profit and its count can
/// never come from different rows.
///
/// Any query or row-conversion failure aborts the complete series.
fn core_series(
    conn: &Connection,
    src: &str,
    q: &Query,
    days: &[DayPoint],
    bucket: i64,
) -> ReadResult<Vec<CoreSeries>> {
    const CTX: &str = "analytics: core_series";
    let Some(t0) = days.first().map(|d| d.start) else {
        return Ok(Vec::new());
    };
    let nb = days.len();
    let sql = format!(
        "SELECT o.core_uid, COALESCE(o.core_name,''), o.closedate,
                COALESCE(o.pnl,0)
         FROM {src}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            // core_uid as Option: `unified_from` projects NULL for a source that has no
            // such column, and a hard i64 read there aborts core_series — which throws the
            // WHOLE summary away, `days` included, over one unattributable row.
            Ok((
                r.get::<_, Option<i64>>(0)?.unwrap_or(0) as u64,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, f64>(3)?,
            ))
        })
        .map_err(|e| read_fail_on(conn, CTX, e))?;
    let mut map: std::collections::HashMap<u64, (String, Vec<f64>, Vec<i64>)> =
        std::collections::HashMap::new();
    for row in rows {
        // core_uid / closedate / profitbtc all move numbers here.
        let (uid, name, close, profit) = row.map_err(|e| read_fail_on(conn, CTX, e))?;
        let idx = (((close - t0) / bucket).max(0) as usize).min(nb - 1);
        let e = map
            .entry(uid)
            .or_insert_with(|| (name.clone(), vec![0.0; nb], vec![0i64; nb]));
        if e.0.is_empty() {
            e.0 = name;
        }
        e.1[idx] += profit;
        e.2[idx] += 1;
    }
    let mut out: Vec<CoreSeries> = map
        .into_iter()
        .map(|(uid, (name, per_bucket, per_bucket_trades))| {
            let total = per_bucket.iter().sum();
            let trades = per_bucket_trades.iter().sum();
            CoreSeries {
                uid,
                name,
                per_bucket,
                per_bucket_trades,
                total,
                trades,
            }
        })
        .collect();
    out.sort_by(|a, b| b.total.total_cmp(&a.total));
    Ok(out)
}

/// Regression tests for the read-failure contract: a damaged replica must
/// surface as an error, never as an empty period. (Scope, attribution, and wiring tests
/// live beside their code in `query/tests.rs`; calendar tests in `calendar/tests.rs`.)
#[cfg(test)]
mod tests;
