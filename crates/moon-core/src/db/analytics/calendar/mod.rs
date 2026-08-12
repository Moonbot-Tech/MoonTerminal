//! Calendar heatmap cells and hour-of-day profiles.

use rusqlite::Connection;

use super::super::read_fail::read_fail_on;
use super::super::ReadResult;
use super::{
    min_closedate, scope_decision_on, scoped, unified_from_mode, ProjectionMode, Query,
    ScopeDecision,
};
use crate::db::{ProfitScope, ProfitUnit};

/// Rows that carry money but are not trades.
///
/// Funding is booked as a pseudo-order: `buydate == closedate`, entry price == exit price, and
/// `spentbtc == boughtq`. Measured on a live replica the two populations coincide exactly — 1 910
/// rows carry this reason and 1 910 rows carry that shape, with no row in one set and not the
/// other. Its money is real and stays in `profit`; its presence in a trade COUNT is not, and it
/// would also add the funded position to turnover and a zero-spread trade to the fee.
const NOT_A_TRADE: &str = "COALESCE(o.sellreason,'') <> 'Funding'";

/// Trade duration in seconds, falling back to zero when the open time is absent.
///
/// Mirrors the fallback [`hour_profile_one`] applies to the same pair of columns, so "when did it
/// open" is answered identically wherever it is asked.
const DURATION: &str = "MAX(o.closedate - COALESCE(NULLIF(o.buydate, 0), o.closedate), 0)";

/// The per-cell aggregate shared by every calendar view. ONE definition so the daily grid, the
/// hour grid, the hour-of-day profile and the comparison period can never disagree about what
/// counts as a winning trade, as turnover, or as a trade at all.
///
/// Money notes, all of which are silent if got wrong:
/// - `pnl` is the ACTIVE metric (quote money, or percent return); `profitbtc` and `spentbtc` are
///   always money, already converted by the projection. Turnover and fee are therefore built from
///   the latter pair, never from `pnl`.
/// - the fee's gross leg comes from raw quantities and prices, which no projection converts, so it
///   is multiplied by `quote_rate` to land in the same currency as `profitbtc`.
/// - turnover multiplies spend by `factor` for the cores that book margin instead of notional.
///
/// Args:
///     factor: Notional multiplier from [`super::basis::notional_factor_expr`].
///     money: Whether absolute money figures are summable in this projection.
///
/// Returns:
///     Comma-separated aggregate expressions.
fn cell_agg(factor: &str, money: bool) -> String {
    // A percent projection measures each trade against its own spend, so trades in different
    // quotes share one scale. Turnover and fee have no such scale — summing them would add USDT to
    // USDC to BTC — so they are withheld rather than silently mixed.
    let (volume, fee, fee_rows) = if money {
        (
            format!("SUM(CASE WHEN {NOT_A_TRADE} THEN o.spentbtc * {factor} END)"),
            format!("SUM(CASE WHEN {NOT_A_TRADE} THEN {FEE_ROW} END)"),
            format!("SUM(CASE WHEN {NOT_A_TRADE} AND ({FEE_ROW}) IS NOT NULL THEN 1 END)"),
        )
    } else {
        ("NULL".to_string(), "NULL".to_string(), "NULL".to_string())
    };
    format!(
        "COALESCE(SUM(o.pnl), 0),
         COALESCE(SUM({NOT_A_TRADE}), 0),
         COALESCE(SUM({NOT_A_TRADE} AND o.pnl > 0), 0),
         {volume},
         {fee},
         COALESCE({fee_rows}, 0),
         COALESCE(SUM(CASE WHEN {NOT_A_TRADE} THEN {DURATION} END), 0)"
    )
}

/// One trade's execution cost: the gross move its prices imply, minus the profit actually booked.
///
/// No core publishes a commission field — neither the protocol nor MoonBot's own report table has
/// one — but the money columns are already net of it: `profitbtc == gainedbtc - spentbtc` holds
/// exactly on 99.24% of a live replica, and the spend/gain pair sits a fee's width off the raw
/// notional. The remainder recovered here matched the venue's published round-trip rate on every
/// core measured (Binance futures 0.0402%, the same with BNB discount 0.0324%, Bybit 0.0803%,
/// Hyperliquid 0.1199%, Bitget 0.1622%).
///
/// For a perpetual this also absorbs funding accrued INSIDE the position, which the report folds
/// into profit without a separate column — so the figure is execution cost, not a venue invoice.
///
/// NULL — meaning "this trade has no recoverable cost" rather than "it cost nothing" — whenever
/// the gross leg cannot be trusted:
/// - `prices_in_money_quote` is false: an inverse COIN-M row counts contracts against USD prices
///   while settling in coin, so quantity × price is off by the price of a bitcoin. Measured: 100
///   such rows produced 12.8M of "cost" against a real monthly turnover of 2.3M.
/// - a quantity or either price is missing: 12 live rows close with no exit price, and their gross
///   leg alone subtracted 71.5 from the total.
/// - the row is a LIQUIDATION: entry price equals exit price, so the gross leg is zero and the
///   "cost" becomes the whole lost position. Pre-2024 COIN-M liquidations additionally escape the
///   quote check above — their label is never rewritten — so contracts times USD prices would land
///   in the tile while `fee_is_complete()` still called it exact.
///
/// Both cases leave the trade out of `fee_trades` as well, which is what makes the shortfall
/// visible as an approximate figure instead of a confidently wrong one.
const FEE_ROW: &str = "CASE WHEN o.prices_in_money_quote
                             AND COALESCE(o.sellreason,'') <> 'LIQUIDATION'
                             AND o.boughtq > 0 AND o.buyprice > 0 AND o.sellprice > 0
                        THEN o.boughtq
                             * (CASE WHEN COALESCE(o.isshort, 0) = 1 THEN o.buyprice - o.sellprice
                                     ELSE o.sellprice - o.buyprice END)
                             * o.quote_rate
                           - o.profitbtc
                        END";

/// Everything one calendar cell or comparison period aggregates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CellTotals {
    /// Sum of the ACTIVE profit metric: quote money, or percent return per trade.
    pub profit: f64,
    /// Trades in the cell, funding rows excluded.
    pub trades: i64,
    /// Winning trades in the cell; losses equal `trades - wins`.
    pub wins: i64,
    /// Traded notional in the projection's currency, or `None` when it cannot be summed.
    pub volume: Option<f64>,
    /// Execution cost over the same trades, or `None` when it cannot be summed.
    pub fee: Option<f64>,
    /// Trades that actually contributed to `fee`; below `trades` when a source lacks prices.
    pub fee_trades: i64,
    /// Summed trade duration in seconds, for the cell's average.
    pub duration_secs: i64,
}

impl CellTotals {
    /// Read one aggregate row written by [`cell_agg`].
    ///
    /// Args:
    ///     row: Result row positioned at the first aggregate column.
    ///     at: Index of that first column.
    ///
    /// Returns:
    ///     The decoded totals, or the row's own conversion failure.
    fn read(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            profit: row.get(at)?,
            trades: row.get(at + 1)?,
            wins: row.get(at + 2)?,
            volume: row.get(at + 3)?,
            fee: row.get(at + 4)?,
            fee_trades: row.get(at + 5)?,
            duration_secs: row.get(at + 6)?,
        })
    }

    /// Average trade duration in seconds, or `None` without trades.
    pub fn avg_duration_secs(&self) -> Option<f64> {
        (self.trades > 0).then(|| self.duration_secs as f64 / self.trades as f64)
    }

    /// Win rate over counted trades, or `None` without any.
    pub fn winrate(&self) -> Option<f64> {
        (self.trades > 0).then(|| self.wins as f64 / self.trades as f64 * 100.0)
    }

    /// Fold one cell into a running period total.
    ///
    /// `None` money means "this cell contributed no summable money" — a percent projection, or a
    /// day whose only rows were funding. It therefore adds nothing rather than poisoning the sum:
    /// a month of ordinary days plus one funding-only day still has a turnover. The total stays
    /// `None` only while EVERY folded cell was `None`, which is what a percent projection produces
    /// and what the UI reads to hide the money tiles entirely.
    ///
    /// Args:
    ///     other: Cell to add.
    pub fn merge(&mut self, other: &Self) {
        self.profit += other.profit;
        self.trades += other.trades;
        self.wins += other.wins;
        self.fee_trades += other.fee_trades;
        self.duration_secs += other.duration_secs;
        self.volume = merge_money(self.volume, other.volume);
        self.fee = merge_money(self.fee, other.fee);
    }

    /// Whether the fee covers every trade the cell counted.
    ///
    /// A source without execution prices contributes no fee, and a total that silently omits those
    /// trades reads as complete. Callers label the figure approximate when this is false.
    pub fn fee_is_complete(&self) -> bool {
        self.fee.is_some() && self.fee_trades == self.trades
    }
}

/// Add two optional money figures, keeping `None` only when neither side had one.
fn merge_money(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    }
}

/// A calendar heatmap cell containing one selected-zone civil day or hour aggregate.
#[derive(Clone, Debug, Default)]
pub struct DayCell {
    /// Cell start in UTC Unix seconds.
    pub start: i64,
    /// Trade aggregates for this cell.
    pub totals: CellTotals,
}

impl DayCell {
    /// Whether the cell has anything to show: a trade, or money booked without one.
    ///
    /// A day whose only row is funding counts zero trades yet moved real money, so emptiness
    /// cannot be decided on the trade count alone.
    pub fn has_activity(&self) -> bool {
        self.totals.trades > 0 || self.totals.profit != 0.0
    }
}

/// Current Calendar cells and the optional previous-period aggregate from one SQLite snapshot.
#[derive(Clone, Debug, Default)]
pub struct CalendarPeriod {
    /// Daily or hourly cells for the visible Calendar scope.
    pub current: Vec<DayCell>,
    /// Previous-period totals used by the Month KPI deltas.
    pub previous: Option<CellTotals>,
}

/// Build the cell aggregate for one projection over one connection.
///
/// The margin-basis probe is resolved HERE rather than inside each reader, so every aggregate of
/// one Calendar read shares a single verdict: cells and their comparison period cannot end up
/// multiplying the same core's turnover differently.
///
/// Args:
///     conn: Existing SQLite connection or pinned snapshot.
///     projection: Money projection resolved by the caller's quote preflight.
///
/// Returns:
///     Aggregate SQL, or a classified read failure from the basis probe.
fn agg_for(conn: &Connection, projection: ProjectionMode) -> ReadResult<String> {
    let margin = super::basis::margin_cores(conn)?;
    let factor = super::basis::notional_factor_expr("o", &margin);
    Ok(cell_agg(&factor, projection != ProjectionMode::Percent))
}

/// Read a Calendar period over an existing connection or pinned compound snapshot.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should serve both periods.
///     q: Visible Calendar scope and time range.
///     previous: Optional comparison scope.
///     hourly: Whether the visible result uses hour buckets.
///
/// Returns:
///     Comparable or empty Calendar data, split-only totals for unsafe raw money, or one
///     classified failure for the pair.
pub(super) fn calendar_period_from(
    conn: &Connection,
    q: &Query,
    previous: Option<&Query>,
    hourly: bool,
) -> ReadResult<ProfitScope<CalendarPeriod>> {
    let mut current_query = q.clone();
    if current_query.from < 0 {
        current_query.from = min_closedate(conn)?;
    }
    let decision = match scope_decision_on(conn, &current_query)? {
        ScopeDecision::Split(totals) => return Ok(ProfitScope::Split(totals)),
        decision => decision,
    };
    let current_projection = decision
        .projection()
        .expect("split decisions return before calendar construction");
    let current = if hourly {
        calendar_hours_from(conn, &current_query, current_projection)?
    } else {
        calendar_cells_from(conn, &current_query, current_projection)?
    };
    let previous_projection = previous
        .map(|query| comparison_projection(conn, &decision, query))
        .transpose()?
        .flatten();
    let previous = match (previous, previous_projection) {
        (Some(query), Some(projection)) => Some(calendar_total_from(conn, query, projection)?),
        (Some(_), None) | (None, _) => None,
    };
    Ok(scoped(decision, CalendarPeriod { current, previous }))
}

/// Aggregate one comparison period without constructing and folding daily cells.
///
/// Args:
///     conn: Existing SQLite connection or pinned snapshot.
///     q: Comparison scope and time range.
///     projection: Money projection compatible with the current period.
///
/// Returns:
///     Period totals, or a classified read failure.
fn calendar_total_from(
    conn: &Connection,
    q: &Query,
    projection: ProjectionMode,
) -> ReadResult<CellTotals> {
    const CTX: &str = "analytics: calendar_total";
    let mut q = q.clone();
    if q.from < 0 {
        q.from = min_closedate(conn)?;
    }
    let Some(src) = unified_from_mode(conn, &q, projection)? else {
        return Ok(CellTotals::default());
    };
    let agg = agg_for(conn, projection)?;
    let sql = format!("SELECT {agg} FROM {src}");
    conn.query_row(&sql, rusqlite::params![q.from, q.to], |row| {
        CellTotals::read(row, 0)
    })
    .map_err(|error| read_fail_on(conn, CTX, error))
}

/// Decide whether a Calendar comparison period shares the current profit unit.
///
/// Args:
///     conn: Pinned report snapshot.
///     current: Current-period comparability decision.
///     previous: Previous-period query.
///
/// Returns:
///     Compatible native, historical USDT, current-rate USDT, or percent projection; `None` when
///     the previous period cannot be expressed in the current unit.
///
/// Errors:
///     Returns a classified report read failure when previous-period preflight cannot complete.
fn comparison_projection(
    conn: &Connection,
    current: &ScopeDecision,
    previous: &Query,
) -> ReadResult<Option<ProjectionMode>> {
    Ok(match current {
        ScopeDecision::Comparable {
            unit: ProfitUnit::Percent,
            projection: ProjectionMode::Percent,
        } => Some(ProjectionMode::Percent),
        ScopeDecision::Comparable {
            unit: ProfitUnit::Quote(current_quote),
            projection: ProjectionMode::Native,
        } => match scope_decision_on(conn, previous)? {
            ScopeDecision::Comparable {
                unit: ProfitUnit::Quote(previous_quote),
                projection: ProjectionMode::Native,
            } if *current_quote == previous_quote => Some(ProjectionMode::Native),
            _ => None,
        },
        // Either USDT projection compares; the previous period is asked for the SAME one, since
        // `previous` carries the same valuation mode as the current query.
        ScopeDecision::Comparable {
            unit: ProfitUnit::Quote(current_quote),
            projection,
        } if projection.is_usdt() && *current_quote == crate::db::QuoteCurrency::usdt() => {
            let totals = super::quote_breakdown_on(conn, previous)?;
            (totals.unknown_orders == 0 && totals.unified_usdt().is_some()).then_some(*projection)
        }
        ScopeDecision::Comparable { .. }
        | ScopeDecision::Empty { .. }
        | ScopeDecision::Split(_) => None,
    })
}

/// Dense daily cells for calendar heatmaps (GitHub-style Year or the large Month view).
/// One bucket is one selected-zone civil day; the range is filled completely with empty cells for
/// days without trades so the calendar grid remains regular. Unlike [`summary`], it never widens
/// buckets to a week.
/// An empty result means the period has no closed trades or the source schema has not arrived yet;
/// database and query failures remain classified so UI callers can retry transient contention.
///
/// Args:
///     q: Report scope and time range to aggregate.
///
/// Returns:
///     Comparable or empty dense cells, split-only totals, or a classified database read failure.
pub fn calendar_cells(q: &Query) -> ReadResult<ProfitScope<Vec<DayCell>>> {
    let conn = super::super::open_reader()?;
    super::super::with_read_snapshot(&conn, |snapshot| {
        let mut q = q.clone();
        if q.from < 0 {
            q.from = min_closedate(snapshot)?;
        }
        let decision = match scope_decision_on(snapshot, &q)? {
            ScopeDecision::Split(totals) => return Ok(ProfitScope::Split(totals)),
            decision => decision,
        };
        let projection = decision
            .projection()
            .expect("split decisions return before calendar construction");
        Ok(scoped(
            decision,
            calendar_cells_from(snapshot, &q, projection)?,
        ))
    })
}

/// Core of [`calendar_cells`] over an existing connection and the entry point for unit tests,
/// which seed an in-memory `orders_rep` and verify bucketing, gaps, and win counts.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should be queried.
///     q: Report scope and time range to aggregate.
///     projection: Money projection resolved by the caller's quote preflight.
///
/// Returns:
///     Dense daily cells or a classified database read failure.
fn calendar_cells_from(
    conn: &Connection,
    q: &Query,
    projection: ProjectionMode,
) -> ReadResult<Vec<DayCell>> {
    const CTX: &str = "analytics: calendar_cells";
    let mut q = q.clone();
    let all_history = q.from < 0;
    if all_history {
        q.from = min_closedate(conn)?;
    }
    let Some(src) = unified_from_mode(conn, &q, projection)? else {
        // Source schemas have not arrived yet, so return an empty calendar (as `summary`
        // returns its default), not an error; otherwise the tab would remain on Loading.
        return Ok(Vec::new());
    };
    super::time_zone::install(conn, q.time_zone).map_err(|e| read_fail_on(conn, CTX, e))?;
    // Daily bucket: profit, trades, wins, turnover, fee and duration for the card's four lines.
    let agg = agg_for(conn, projection)?;
    let sql = format!(
        "SELECT mt_local_bucket(o.closedate, 86400) AS d,
                {agg}
         FROM {src} GROUP BY d ORDER BY d"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok(DayCell {
                start: r.get::<_, i64>(0)?,
                totals: CellTotals::read(r, 1)?,
            })
        })
        .map_err(|e| read_fail_on(conn, CTX, e))?;
    let mut map: std::collections::HashMap<i64, DayCell> = std::collections::HashMap::new();
    let (mut first, mut last) = (i64::MAX, i64::MIN);
    for row in rows {
        let cell = row.map_err(|e| read_fail_on(conn, CTX, e))?;
        first = first.min(cell.start);
        last = last.max(cell.start);
        map.insert(cell.start, cell);
    }
    if map.is_empty() {
        return Ok(Vec::new()); // A period without trades has an empty calendar.
    }
    // Dense-grid bounds start at the first day with data for All (avoiding years of empty
    // cells since the epoch), or at the requested period's start otherwise. Because `to` is
    // exclusive, the last day is day(to - 1); do not extend into the future.
    let now = crate::util::now_unix_ms_i64() / 1000;
    let today0 = crate::util::display_time::bucket_start(now, 86_400, q.time_zone).unwrap_or(now);
    let day0 = if all_history {
        first
    } else {
        crate::util::display_time::bucket_start(q.from, 86_400, q.time_zone).unwrap_or(q.from)
    };
    let last_grid = crate::util::display_time::bucket_start(q.to - 1, 86_400, q.time_zone)
        .unwrap_or(q.to - 1)
        .min(today0)
        .max(last);
    let day0 = day0.min(last_grid);
    let mut out = Vec::with_capacity((((last_grid - day0) / 86_400) + 1).max(1) as usize);
    let mut t = day0;
    while t <= last_grid {
        out.push(map.remove(&t).unwrap_or(DayCell {
            start: t,
            ..Default::default()
        }));
        let Some(next) = crate::util::display_time::next_bucket(t, 86_400, q.time_zone) else {
            break;
        };
        t = next;
    }
    Ok(out)
}

/// Hourly cells for Calendar Day mode: `start` is the absolute UTC instant of a selected-zone
/// civil-hour bucket, with PnL, trades, and wins. The result is sparse; the UI supplies civil-hour
/// labels and leaves skipped transition hours empty. An empty result means no trades or source
/// schema; failures remain classified.
///
/// Args:
///     q: Report scope and one-day time range to aggregate.
///
/// Returns:
///     Comparable or empty sparse cells, split-only totals, or a classified database read failure.
pub fn calendar_hours(q: &Query) -> ReadResult<ProfitScope<Vec<DayCell>>> {
    let conn = super::super::open_reader()?;
    super::super::with_read_snapshot(&conn, |snapshot| {
        let decision = match scope_decision_on(snapshot, q)? {
            ScopeDecision::Split(totals) => return Ok(ProfitScope::Split(totals)),
            decision => decision,
        };
        let projection = decision
            .projection()
            .expect("split decisions return before calendar construction");
        Ok(scoped(
            decision,
            calendar_hours_from(snapshot, q, projection)?,
        ))
    })
}

/// Core of [`calendar_hours`] over an existing connection or pinned read snapshot.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should be queried.
///     q: Report scope and one-day time range to aggregate.
///     projection: Money projection resolved by the caller's quote preflight.
///
/// Returns:
///     Sparse hourly cells or a classified database read failure.
fn calendar_hours_from(
    conn: &Connection,
    q: &Query,
    projection: ProjectionMode,
) -> ReadResult<Vec<DayCell>> {
    const CTX: &str = "analytics: calendar_hours";
    let src = match unified_from_mode(conn, q, projection)? {
        Some(s) => s,
        None => return Ok(Vec::new()), // The schema has not arrived yet.
    };
    super::time_zone::install(conn, q.time_zone).map_err(|e| read_fail_on(conn, CTX, e))?;
    let agg = agg_for(conn, projection)?;
    let sql = format!(
        "SELECT mt_local_bucket(o.closedate, 3600) AS h,
                {agg}
         FROM {src} GROUP BY h ORDER BY h"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok(DayCell {
                start: r.get(0)?,
                totals: CellTotals::read(r, 1)?,
            })
        })
        .map_err(|e| read_fail_on(conn, CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| read_fail_on(conn, CTX, e))?);
    }
    Ok(out)
}

/// Selected-zone hour-of-day profile (0..23): PnL, trades, and wins across the period.
/// This supplies a cell in the Tuning by-time lower heatmap.
#[derive(Clone, Copy, Debug, Default)]
pub struct HourStat {
    pub profit: f64,
    pub trades: i64,
    pub wins: i64,
}

/// Build hour-of-day profiles for several periods at once (current/week/month/90 days) for
/// the Tuning by-time lower heatmap. One reader and one snapshot serve ALL ranges so columns
/// in the same map see the same data. Core, side, emulator, and strategy filters come from
/// `base`; each range's from/to override the period (`from < 0` means all history). The
/// returned profiles align with `ranges`.
pub fn hourly_profiles(base: &Query, ranges: &[(i64, i64)]) -> ReadResult<Vec<[HourStat; 24]>> {
    let conn = super::super::open_reader()?;
    super::super::with_read_snapshot(&conn, |snapshot| hourly_profiles_on(snapshot, base, ranges))
}

/// Build hour-of-day profiles on an existing connection or shared compound-read snapshot.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should be queried.
///     base: Shared report scope.
///     ranges: Ordered time ranges to aggregate.
///
/// Returns:
///     Profiles aligned with `ranges`, or a classified read failure.
fn hourly_profiles_on(
    conn: &Connection,
    base: &Query,
    ranges: &[(i64, i64)],
) -> ReadResult<Vec<[HourStat; 24]>> {
    let mut out = Vec::with_capacity(ranges.len());
    for &(from, to) in ranges {
        out.push(hour_profile_one(conn, base, from, to)?);
    }
    Ok(out)
}

/// One hour-of-day profile column for `[from, to)` on an existing snapshot.
///
/// Args:
///     conn: Pinned report snapshot.
///     base: Shared report filters and profit metric.
///     from: Inclusive period start, or a negative all-history sentinel.
///     to: Exclusive period end.
///
/// Returns:
///     Twenty-four comparable hourly aggregates or a classified read failure.
fn hour_profile_one(
    conn: &Connection,
    base: &Query,
    from: i64,
    to: i64,
) -> ReadResult<[HourStat; 24]> {
    const CTX: &str = "analytics: hour_profile";
    let mut q = base.clone();
    q.from = if from < 0 { min_closedate(conn)? } else { from };
    q.to = to;
    let decision = scope_decision_on(conn, &q)?;
    let Some(projection) = decision.projection() else {
        return Err(super::super::ReadFail::IncomparableQuote);
    };
    let mut prof = [HourStat::default(); 24];
    // Source schemas have not arrived yet, so return an empty profile like summary/calendar.
    let Some(src) = unified_from_mode(conn, &q, projection)? else {
        return Ok(prof);
    };
    super::time_zone::install(conn, q.time_zone).map_err(|e| read_fail_on(conn, CTX, e))?;
    // Hour of day comes from the trade OPEN time (`buydate`), matching the schedule and tuner
    // sliders that gate ENTRY. Fall back to `closedate` when the open time is absent (0/NULL).
    // The period itself still uses `closedate`, which defines the analysis window.
    let agg = agg_for(conn, projection)?;
    let sql = format!(
        "SELECT (mt_minute_of_day(COALESCE(NULLIF(o.buydate, 0), o.closedate)) / 60) AS h,
                {agg}
         FROM {src} GROUP BY h ORDER BY h"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((r.get::<_, i64>(0)?, CellTotals::read(r, 1)?))
        })
        .map_err(|e| read_fail_on(conn, CTX, e))?;
    for row in rows {
        let (h, totals) = row.map_err(|e| read_fail_on(conn, CTX, e))?;
        if (0..24).contains(&h) {
            prof[h as usize] = HourStat {
                profit: totals.profit,
                trades: totals.trades,
                wins: totals.wins,
            };
        }
    }
    Ok(prof)
}

#[cfg(test)]
mod tests;
