//! Single-pass current-period aggregation for the Analytics Summary.

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, types::Value};

use super::groups::group_quote_scope;
use super::strategy_meta::{StrategyMetadata, SummaryMetadata, read_metadata};
use super::{CoreSeries, DayPoint, GroupStat, KindCore, KindStat, PeriodStats, Query, TopTrade};
use crate::db::metrics::profit_factor;
use crate::db::read_fail::{read_fail, read_fail_on};
use crate::db::report_axis::{MAX_OFFSET_SECS, MIN_OFFSET_SECS};
use crate::db::{QuoteScope, ReadResult};

/// Maximum true-UTC span SQLite may sort for one current-period Summary statement.
const QUERY_WINDOW_SECS: i64 = 7 * 86_400;
/// Maximum difference between the retained and predicate-baked offsets for one row.
const OFFSET_SCAN_PADDING_SECS: i64 = MAX_OFFSET_SECS as i64 - MIN_OFFSET_SECS as i64;
/// Maximum raw `closedate` span each production window may scan from a physical source.
const MAX_INNER_SCAN_SECS: i64 = QUERY_WINDOW_SECS + 2 * OFFSET_SCAN_PADDING_SECS;

/// Fully finalized current-period pieces consumed by [`super::summary_on`].
pub(super) struct SummaryParts {
    pub(super) cur: PeriodStats,
    pub(super) days: Vec<DayPoint>,
    pub(super) core_days: Vec<CoreSeries>,
    pub(super) best: Vec<TopTrade>,
    pub(super) worst: Vec<TopTrade>,
    pub(super) strategies: Vec<GroupStat>,
    pub(super) coins: Vec<GroupStat>,
    pub(super) best_hour: Option<(u32, f64, i64)>,
    pub(super) kinds: Vec<KindStat>,
}

/// Raw-money fields accumulated independently of the active Analytics lens.
#[derive(Clone, Debug, Default)]
struct RawGroup {
    profit: f64,
    spent: f64,
    positive_spent: i64,
    quote_min: Option<i64>,
    quote_max: Option<i64>,
    quote_integral: i64,
    rows: i64,
}

impl RawGroup {
    /// Add one raw-money row using SQLite's aggregate NULL and type rules.
    ///
    /// Args:
    ///     profit: Raw quote-currency profit, or SQL NULL.
    ///     spent: Raw entry spend, or SQL NULL.
    ///     quote: SQLite value carrying the report quote ordinal and storage class.
    fn push(&mut self, profit: Option<f64>, spent: Option<f64>, quote: &Value) {
        self.rows += 1;
        self.profit += profit.unwrap_or(0.0);
        if let Some(spent) = spent.filter(|spent| *spent > 0.0) {
            self.spent += spent;
            self.positive_spent += 1;
        }
        if let Value::Integer(ordinal) = quote {
            self.quote_integral += 1;
            self.quote_min = Some(self.quote_min.map_or(*ordinal, |value| value.min(*ordinal)));
            self.quote_max = Some(self.quote_max.map_or(*ordinal, |value| value.max(*ordinal)));
        }
    }

    /// Return SQLite-compatible average positive spend for this group.
    ///
    /// Returns:
    ///     Average over positive spend values, or zero when no positive value exists.
    fn avg_order(&self) -> f64 {
        if self.positive_spent == 0 {
            0.0
        } else {
            self.spent / self.positive_spent as f64
        }
    }

    /// Classify the group's raw quote identity from its row metadata.
    ///
    /// Returns:
    ///     Empty, single, mixed, or unknown quote scope.
    fn quote(&self) -> QuoteScope {
        group_quote_scope(
            self.quote_min,
            self.quote_max,
            self.quote_integral,
            self.rows,
        )
    }
}

/// Active-lens metrics shared by strategy and coin Summary groups.
#[derive(Clone, Debug, Default)]
struct GroupAccumulator {
    n: i64,
    profit: f64,
    wins: i64,
    win_sum: f64,
    loss_sum: f64,
    best: Option<f64>,
    worst: Option<f64>,
    core_name: Option<String>,
    cores: HashSet<i64>,
    strategy_id: Option<i64>,
    strategy_core: Option<i64>,
    raw: RawGroup,
}

impl GroupAccumulator {
    /// Add one active-lens row while preserving SQL aggregate semantics.
    ///
    /// Args:
    ///     row: Chronological report row to aggregate.
    ///     include_raw: Whether raw-money fields belong to this pass instead of a neutral source.
    fn push(&mut self, row: &TradeRow, include_raw: bool) {
        self.n += 1;
        let profit = row.pnl.unwrap_or(0.0);
        self.profit += profit;
        if profit > 0.0 {
            self.wins += 1;
            self.win_sum += profit;
        } else {
            self.loss_sum -= profit;
        }
        if let Some(value) = row.pnl {
            self.best = Some(self.best.map_or(value, |best| best.max(value)));
            self.worst = Some(self.worst.map_or(value, |worst| worst.min(value)));
        }
        if let Some(name) = &row.core_name {
            if self
                .core_name
                .as_ref()
                .map_or(true, |current| name > current)
            {
                self.core_name = Some(name.clone());
            }
        }
        if let Some(core_uid) = row.core_uid {
            self.cores.insert(core_uid);
        }
        if include_raw {
            self.raw.push(row.raw_profit, row.spent, &row.basecurrency);
        }
    }
}

/// One chronological active-lens row projected by the unified source.
#[derive(Clone, Debug)]
struct TradeRow {
    closedate: i64,
    buydate: i64,
    pnl: Option<f64>,
    core_uid: Option<i64>,
    core_name: Option<String>,
    coin: String,
    strategy_text: Option<String>,
    strategy_id: Option<i64>,
    is_short: bool,
    raw_profit: Option<f64>,
    spent: Option<f64>,
    basecurrency: Value,
}

/// Core-local bucket accumulation before the dense Summary grid is known.
#[derive(Clone, Debug, Default)]
struct CoreAccumulator {
    name: String,
    buckets: HashMap<i64, (f64, i64)>,
}

/// Intermediate current-period state filled by one chronological row stream.
#[derive(Default)]
struct Accumulator {
    stats: PeriodStats,
    days: Vec<DayPoint>,
    hours: [(f64, i64); 24],
    win_sum: f64,
    loss_sum: f64,
    cumulative: f64,
    peak: f64,
    current_wins: i64,
    current_losses: i64,
    duration_seconds: i64,
    cores: HashMap<u64, CoreAccumulator>,
    strategies: HashMap<String, GroupAccumulator>,
    coins: HashMap<String, GroupAccumulator>,
    best_rows: Vec<TradeRow>,
    worst_rows: Vec<TradeRow>,
}

impl Accumulator {
    /// Consume one current-period row into every Summary projection.
    ///
    /// Args:
    ///     row: Chronological report row shared by every projection.
    ///     bucket: Time-grid bucket width in seconds.
    ///     include_inline_raw: Whether this stream owns raw-money group aggregation.
    ///     axis: Per-core time axis. `closedate` is the CORE's own wall clock, so it reaches
    ///         true UTC through the axis before any civil bucket or hour slot is derived.
    ///
    /// Returns:
    ///     Success after every projection accepts the row.
    ///
    /// Errors:
    ///     Returns a row-shape error when strategy or core identity is absent.
    fn push(
        &mut self,
        row: TradeRow,
        bucket: i64,
        include_inline_raw: bool,
        axis: &crate::db::ReportAxis,
    ) -> rusqlite::Result<()> {
        let profit = row.pnl.unwrap_or(0.0);
        // `closedate` is the CORE's wall clock; it reaches true UTC before any civil bucket or
        // hour slot is taken from it. The duration below is a DIFFERENCE on that one clock and
        // cancels the offset, so it deliberately stays raw.
        let core_uid = row.core_uid.unwrap_or(0) as u64;
        let closedate_utc = axis.to_utc(row.closedate, core_uid);
        self.stats.n += 1;
        self.stats.profit += profit;
        self.duration_seconds += (row.closedate - row.buydate).max(0);
        if profit > 0.0 {
            self.stats.wins += 1;
            self.win_sum += profit;
            self.current_wins += 1;
            self.current_losses = 0;
            self.stats.win_streak = self.stats.win_streak.max(self.current_wins);
        } else {
            self.stats.losses += 1;
            self.loss_sum -= profit;
            self.current_losses += 1;
            self.current_wins = 0;
            self.stats.loss_streak = self.stats.loss_streak.max(self.current_losses);
        }
        self.cumulative += profit;
        self.peak = self.peak.max(self.cumulative);
        self.stats.max_dd = self.stats.max_dd.max(self.peak - self.cumulative);

        let bucket_start =
            crate::util::display_time::bucket_start(closedate_utc, bucket, axis.zone())
                .unwrap_or(closedate_utc);
        match self.days.last_mut() {
            Some(day) if day.start == bucket_start => {
                day.profit += profit;
                day.trades += 1;
            }
            _ => self.days.push(DayPoint {
                start: bucket_start,
                profit,
                trades: 1,
            }),
        }
        let hour = crate::util::display_time::minute_of_day(closedate_utc, axis.zone())
            .map(|minute| minute as usize / 60)
            .unwrap_or(0);
        self.hours[hour].0 += profit;
        self.hours[hour].1 += 1;

        let core = self.cores.entry(core_uid).or_default();
        if core.name.is_empty() {
            core.name = row.core_name.clone().unwrap_or_default();
        }
        let core_bucket = core.buckets.entry(bucket_start).or_default();
        core_bucket.0 += profit;
        core_bucket.1 += 1;

        let strategy_key = match (&row.strategy_text, row.core_uid) {
            (Some(strategy_text), Some(core_uid)) => format!("{strategy_text}@{core_uid}"),
            _ => {
                return Err(rusqlite::Error::InvalidColumnType(
                    6,
                    "strategyid/core_uid".to_string(),
                    rusqlite::types::Type::Null,
                ));
            }
        };
        let strategy = self.strategies.entry(strategy_key).or_default();
        if strategy.n == 0 {
            strategy.strategy_id = row.strategy_id;
            strategy.strategy_core = row.core_uid;
        } else if strategy.strategy_id != row.strategy_id {
            strategy.strategy_id = None;
        }
        strategy.push(&row, include_inline_raw);
        self.coins
            .entry(row.coin.clone())
            .or_default()
            .push(&row, include_inline_raw);
        if row.pnl.is_some() {
            push_top_row(&mut self.best_rows, row.clone(), true);
            push_top_row(&mut self.worst_rows, row, false);
        }
        Ok(())
    }

    /// Finalize sequence metrics and fill missing time buckets.
    ///
    /// Args:
    ///     bucket: Time-grid bucket width in seconds.
    ///     zone: Selected IANA zone used to step the civil bucket grid.
    ///
    /// Returns:
    ///     Nothing; aggregate fields and dense buckets are finalized in place.
    fn finish_period(&mut self, bucket: i64, zone: chrono_tz::Tz) {
        if self.stats.n > 0 {
            self.stats.avg = self.stats.profit / self.stats.n as f64;
            self.stats.avg_dur_min = self.duration_seconds as f64 / self.stats.n as f64 / 60.0;
            self.stats.pf = profit_factor(self.win_sum, self.loss_sum);
        }
        if self.days.is_empty() {
            return;
        }
        let mut filled = Vec::with_capacity(self.days.len());
        let mut start = self.days[0].start;
        let mut days = std::mem::take(&mut self.days).into_iter().peekable();
        while let Some(day) = days.peek() {
            if day.start == start {
                filled.push(days.next().expect("peeked day exists"));
            } else {
                filled.push(DayPoint {
                    start,
                    profit: 0.0,
                    trades: 0,
                });
            }
            let Some(next) = crate::util::display_time::next_bucket(start, bucket, zone) else {
                break;
            };
            start = next;
        }
        self.days = filled;
    }
}

/// Retain one best-or-worst candidate in a stable five-row ranking.
///
/// Args:
///     rows: Existing bounded ranking in display order.
///     row: New chronological trade candidate.
///     best: Whether larger profit ranks first.
fn push_top_row(rows: &mut Vec<TradeRow>, row: TradeRow, best: bool) {
    rows.push(row);
    rows.sort_by(|left, right| {
        let left = left.pnl.unwrap_or(0.0);
        let right = right.pnl.unwrap_or(0.0);
        if best {
            right.total_cmp(&left)
        } else {
            left.total_cmp(&right)
        }
    });
    rows.truncate(5);
}

/// Return the bounded raw-date range supplied to the unified source for one outer UTC window.
///
/// `period_predicate_on` bakes one offset per core at the whole period's upper bound, while the
/// outer `mt_to_utc` residual resolves the offset retained for each row. Those two valid offsets
/// can differ by at most the full [`MIN_OFFSET_SECS`]..=[`MAX_OFFSET_SECS`] range, so padding both
/// sides by that difference cannot exclude an outer-accepted row. Identity axes have no such
/// difference and keep the exact index range.
///
/// Args:
///     axis: Resolved historical report axis used by the outer residual.
///     outer_from: Inclusive true-UTC window bound.
///     outer_to: Exclusive true-UTC window bound.
///
/// Returns:
///     Inclusive and exclusive raw-date bounds for the unified source.
fn inner_window_bounds(axis: &crate::db::ReportAxis, outer_from: i64, outer_to: i64) -> (i64, i64) {
    if axis.is_utc_identity() {
        (outer_from, outer_to)
    } else {
        (
            outer_from.saturating_sub(OFFSET_SCAN_PADDING_SECS),
            outer_to.saturating_add(OFFSET_SCAN_PADDING_SECS),
        )
    }
}

/// Build the prepared current-stream SQL shared by production and plan-contract tests.
///
/// The order tail covers every value observable by [`Accumulator`]. Rows still tied after the
/// tuple are therefore sequence-equivalent for streaks, drawdown, aggregates, and displayed top
/// rows even when SQLite changes UNION branch or index traversal order.
///
/// Args:
///     src: Unified filtered source retaining its inner `?1`/`?2` range.
///     true_time: Exact true-UTC expression used by the residual and primary order key.
///     inline_raw: Raw-money projection or NULL placeholders for the neutral-source mode.
///
/// Returns:
///     One reusable statement with inner range and exact outer residual parameters.
fn current_stream_sql(src: &str, true_time: &str, inline_raw: &str) -> String {
    format!(
        "SELECT o.closedate, COALESCE(o.buydate, o.closedate), o.pnl,
                o.core_uid, o.core_name, COALESCE(o.coin,''),
                CAST(o.strategyid AS TEXT),
                CASE WHEN typeof(o.strategyid) = 'integer' THEN o.strategyid END,
                COALESCE(o.isshort,0), {inline_raw}
         FROM {src}
         WHERE {true_time} >= ?3 AND {true_time} < ?4
         ORDER BY {true_time},
                  o.closedate, o.core_uid, o.pnl IS NULL, o.pnl,
                  COALESCE(o.buydate, o.closedate), o.core_name IS NULL, o.core_name,
                  COALESCE(o.coin,''), typeof(o.strategyid), CAST(o.strategyid AS TEXT),
                  COALESCE(o.isshort,0), o.profitbtc IS NULL, o.profitbtc,
                  o.spentbtc IS NULL, o.spentbtc,
                  typeof(o.basecurrency), CAST(o.basecurrency AS TEXT)"
    )
}

/// Read and finalize all current-period Summary projections.
///
/// Args:
///     conn: Pinned report snapshot.
///     src: Unified active-lens report source.
///     raw_src: Optional lens-neutral source used only for Percent raw enrichments.
///     q: Concrete Summary query.
///     bucket: Hour, day, or week bucket size.
///     include_kinds: Whether the one-day kind chart is visible.
///     has_names: Whether the attached strategy database can enrich labels.
///
/// Returns:
///     Every current-period Summary projection or one classified read failure.
pub(super) fn read(
    conn: &Connection,
    src: &str,
    raw_src: Option<&str>,
    q: &Query,
    axis: &crate::db::ReportAxis,
    bucket: i64,
    include_kinds: bool,
    has_names: bool,
) -> ReadResult<SummaryParts> {
    read_with_window(
        conn,
        src,
        raw_src,
        q,
        axis,
        StreamReadConfig {
            bucket,
            include_kinds,
            has_names,
            query_window_secs: QUERY_WINDOW_SECS,
        },
    )
}

/// Execution and projection controls for one bounded current-period stream read.
#[derive(Clone, Copy)]
struct StreamReadConfig {
    /// Hour, day, or week bucket size.
    bucket: i64,
    /// Whether the one-day kind chart is visible.
    include_kinds: bool,
    /// Whether the attached strategy database can enrich labels.
    has_names: bool,
    /// Positive maximum true-UTC span sorted by one statement execution.
    query_window_secs: i64,
}

/// Read current-period Summary projections through bounded chronological SQL windows.
///
/// Each execution rebinds the unified source's `?1`/`?2` to a bounded raw-date range and applies
/// an exact half-open true-UTC `?3`/`?4` residual outside it. The source SQL, including its offset
/// grouping and metric projection, stays unchanged and the prepared statement is reused.
///
/// Args:
///     conn: Pinned report snapshot.
///     src: Unified active-lens report source.
///     raw_src: Optional lens-neutral source used only for Percent raw enrichments.
///     q: Concrete Summary query.
///     axis: Resolved historical report axis shared by filtering and accumulation.
///     config: Execution window and projection controls.
///
/// Returns:
///     Every current-period Summary projection or one classified read failure.
fn read_with_window(
    conn: &Connection,
    src: &str,
    raw_src: Option<&str>,
    q: &Query,
    axis: &crate::db::ReportAxis,
    config: StreamReadConfig,
) -> ReadResult<SummaryParts> {
    const CTX: &str = "analytics: summary current stream";
    let StreamReadConfig {
        bucket,
        include_kinds,
        has_names,
        query_window_secs,
    } = config;
    debug_assert!(query_window_secs > 0);
    // The axis arrives RESOLVED from the caller rather than being read here, for two reasons: it
    // must be the one the rest of this Summary read already used, and resolving it costs two
    // queries that `current_summary_rows_execute_multiple_bounded_statements` keeps out of this
    // path.
    // `q.axis` is deliberately NOT consulted — that field is the panel's copy from when the query
    // was built.
    let true_time = if axis.is_utc_identity() {
        "o.closedate"
    } else {
        // A shifted axis needs the registered scalar to merge cores by their true UTC order.
        super::time_zone::install(conn, axis).map_err(|error| read_fail_on(conn, CTX, error))?;
        "mt_to_utc(o.closedate, o.core_uid)"
    };
    let inline_raw = if raw_src.is_none() {
        "o.profitbtc, o.spentbtc, o.basecurrency"
    } else {
        "NULL, NULL, NULL"
    };
    let sql = current_stream_sql(src, true_time, inline_raw);
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let mut accumulator = Accumulator::default();
    let mut window_from = q.from;
    while window_from < q.to {
        let window_to = window_from
            .checked_add(query_window_secs)
            .unwrap_or(q.to)
            .min(q.to);
        let (inner_from, inner_to) = inner_window_bounds(axis, window_from, window_to);
        debug_assert!(
            inner_to.saturating_sub(inner_from)
                <= query_window_secs.saturating_add(2 * OFFSET_SCAN_PADDING_SECS)
        );
        if query_window_secs == QUERY_WINDOW_SECS {
            debug_assert!(inner_to.saturating_sub(inner_from) <= MAX_INNER_SCAN_SECS);
        }
        let rows = statement
            .query_map(
                rusqlite::params![inner_from, inner_to, window_from, window_to],
                |row| {
                    Ok(TradeRow {
                        closedate: row.get(0)?,
                        buydate: row.get(1)?,
                        pnl: row.get(2)?,
                        core_uid: row.get(3)?,
                        core_name: row.get(4)?,
                        coin: row.get(5)?,
                        strategy_text: row.get(6)?,
                        strategy_id: row.get(7)?,
                        is_short: row.get::<_, i64>(8)? != 0,
                        raw_profit: row.get(9)?,
                        spent: row.get(10)?,
                        basecurrency: row.get(11)?,
                    })
                },
            )
            .map_err(|error| read_fail_on(conn, CTX, error))?;
        for row in rows {
            let row = row.map_err(|error| read_fail_on(conn, CTX, error))?;
            accumulator
                .push(row, bucket, raw_src.is_none(), axis)
                .map_err(|error| read_fail_on(conn, CTX, error))?;
        }
        window_from = window_to;
    }
    accumulator.finish_period(bucket, axis.zone());

    let (raw_strategies, raw_coins) = match raw_src {
        Some(raw_src) => read_raw_groups(conn, raw_src, q)?,
        None => (HashMap::new(), HashMap::new()),
    };
    let pairs = accumulator
        .strategies
        .values()
        .filter_map(|group| group.strategy_id.zip(group.strategy_core))
        .collect::<HashSet<_>>();
    let metadata = if has_names {
        match read_metadata(conn, &pairs) {
            Ok(metadata) => metadata,
            Err(error) => {
                log::warn!(
                    "analytics: strategy names unavailable, using bare ids for current stream: {error}"
                );
                SummaryMetadata::default()
            }
        }
    } else {
        SummaryMetadata::default()
    };

    let best_hour = accumulator
        .hours
        .iter()
        .enumerate()
        .filter(|(_, (profit, trades))| *trades > 0 && *profit > 0.0)
        .max_by(|left, right| left.1.0.total_cmp(&right.1.0))
        .map(|(hour, (profit, trades))| (hour as u32, *profit, *trades));
    let core_days = finish_cores(&accumulator.cores, &accumulator.days);
    let kinds = if include_kinds {
        finish_kinds(&accumulator.strategies, metadata.groups.as_ref())
    } else {
        Vec::new()
    };
    let strategies = finish_groups(
        accumulator.strategies,
        (!raw_strategies.is_empty()).then_some(&raw_strategies),
        metadata.groups.as_ref(),
        true,
    );
    let coins = finish_groups(
        accumulator.coins,
        (!raw_coins.is_empty()).then_some(&raw_coins),
        None,
        false,
    );
    let (best, worst) = finish_top(
        accumulator.best_rows,
        accumulator.worst_rows,
        Some(&metadata.heads),
        axis,
    )?;
    Ok(SummaryParts {
        cur: accumulator.stats,
        days: accumulator.days,
        core_days,
        best,
        worst,
        strategies,
        coins,
        best_hour,
        kinds,
    })
}

/// Scan the optional lens-neutral source once into strategy and coin raw aggregates.
///
/// Args:
///     conn: Pinned report snapshot.
///     src: Lens-neutral unified report source.
///     q: Concrete Summary query supplying the source parameters.
///
/// Returns:
///     Raw-money aggregates keyed by exact strategy/core identity and coin.
fn read_raw_groups(
    conn: &Connection,
    src: &str,
    q: &Query,
) -> ReadResult<(HashMap<String, RawGroup>, HashMap<String, RawGroup>)> {
    const CTX: &str = "analytics: summary raw groups";
    let sql = format!(
        "SELECT CAST(o.strategyid AS TEXT), o.core_uid, COALESCE(o.coin,''),
                o.profitbtc, o.spentbtc, o.basecurrency
         FROM {src}"
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let rows = statement
        .query_map(rusqlite::params![q.from, q.to], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Value>(5)?,
            ))
        })
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let mut strategies = HashMap::<String, RawGroup>::new();
    let mut coins = HashMap::<String, RawGroup>::new();
    for row in rows {
        let (strategy_text, core_uid, coin, profit, spent, quote) =
            row.map_err(|error| read_fail_on(conn, CTX, error))?;
        if let (Some(strategy_text), Some(core_uid)) = (strategy_text, core_uid) {
            strategies
                .entry(format!("{strategy_text}@{core_uid}"))
                .or_default()
                .push(profit, spent, &quote);
        }
        coins.entry(coin).or_default().push(profit, spent, &quote);
    }
    Ok((strategies, coins))
}

/// Align each core's sparse buckets to the already-filled Summary time grid.
///
/// Args:
///     cores: Sparse per-core bucket aggregates.
///     days: Dense Summary grid defining output bucket order.
///
/// Returns:
///     Per-core dense series ordered by descending total profit.
fn finish_cores(cores: &HashMap<u64, CoreAccumulator>, days: &[DayPoint]) -> Vec<CoreSeries> {
    let mut output = cores
        .iter()
        .map(|(uid, core)| {
            let mut per_bucket = Vec::with_capacity(days.len());
            let mut per_bucket_trades = Vec::with_capacity(days.len());
            for day in days {
                let (profit, trades) = core.buckets.get(&day.start).copied().unwrap_or_default();
                per_bucket.push(profit);
                per_bucket_trades.push(trades);
            }
            CoreSeries {
                uid: *uid,
                name: core.name.clone(),
                total: per_bucket.iter().sum(),
                trades: per_bucket_trades.iter().sum(),
                per_bucket,
                per_bucket_trades,
            }
        })
        .collect::<Vec<_>>();
    output.sort_by(|left, right| right.total.total_cmp(&left.total));
    output
}

/// Finalize strategy or coin aggregates with raw-money and metadata enrichments.
///
/// Args:
///     groups: Active-lens aggregates keyed by their exact display identity.
///     raw: Optional lens-neutral raw-money replacement aggregates.
///     metadata: Optional numeric strategy metadata enrichment.
///     by_strategy: Whether keys represent strategies rather than coins.
///
/// Returns:
///     Display rows ordered by descending profit and stable key tie-break.
fn finish_groups(
    groups: HashMap<String, GroupAccumulator>,
    raw: Option<&HashMap<String, RawGroup>>,
    metadata: Option<&HashMap<(i64, i64), StrategyMetadata>>,
    by_strategy: bool,
) -> Vec<GroupStat> {
    let mut output = groups
        .into_iter()
        .map(|(key, group)| {
            let raw = raw
                .and_then(|raw| raw.get(&key))
                .cloned()
                .unwrap_or_else(|| group.raw.clone());
            let quote = raw.quote();
            let comparable = matches!(quote, QuoteScope::Single(_));
            let display_key = key;
            let pair = group.strategy_id.zip(group.strategy_core);
            let details = pair.and_then(|pair| metadata.and_then(|all| all.get(&pair)));
            let strategy_id = display_key
                .rsplit_once('@')
                .map_or_else(String::new, |(strategy_id, _)| strategy_id.to_string());
            GroupStat {
                key: display_key.clone(),
                name: if by_strategy {
                    details
                        .and_then(|item| item.name.clone())
                        .unwrap_or(strategy_id)
                } else {
                    display_key
                },
                kind: details.map(|item| item.kind.clone()).unwrap_or_default(),
                core: group.core_name.unwrap_or_default(),
                cores_n: group.cores.len() as i64,
                alive: if by_strategy {
                    metadata.map(|_| details.map_or(0, |item| item.alive))
                } else {
                    None
                },
                n: group.n,
                profit: group.profit,
                raw_profit: if comparable { raw.profit } else { f64::NAN },
                avg_order: if comparable {
                    raw.avg_order()
                } else {
                    f64::NAN
                },
                quote,
                wins: group.wins,
                pf: profit_factor(group.win_sum, group.loss_sum),
                best: group.best.unwrap_or(0.0),
                worst: group.worst.unwrap_or(0.0),
                lastedit: details
                    .map(|item| item.lastedit.clone())
                    .unwrap_or_default(),
                bl: details.map_or(0, StrategyMetadata::blacklist_count),
                wl: details.map_or(0, StrategyMetadata::whitelist_count),
            }
        })
        .collect::<Vec<_>>();
    output.sort_by(|left, right| {
        right
            .profit
            .total_cmp(&left.profit)
            .then_with(|| left.key.cmp(&right.key))
    });
    output
}

/// Resolve the five best and five worst rows with one shared metadata lookup.
///
/// Args:
///     best_rows: Bounded descending-profit candidates.
///     worst_rows: Bounded ascending-profit candidates.
///     metadata: Optional strategy-head names for numeric identities.
///     axis: Per-core time axis. `closedate` is the CORE's own wall clock, so the RETAINED stamp
///         reaches true UTC through the axis exactly as the ordering and bucketing stages already
///         do. Converting here rather than in the UI is what keeps [`TopTrade`] free of a core
///         uid: without one, a consumer holding the raw value could not convert it even in
///         principle, so the card would render an uncorrected time beside a corrected grid.
///
/// Returns:
///     Final best and worst trade rows, or one classified row-shape failure.
fn finish_top(
    best_rows: Vec<TradeRow>,
    worst_rows: Vec<TradeRow>,
    metadata: Option<&HashMap<(i64, i64), StrategyMetadata>>,
    axis: &crate::db::ReportAxis,
) -> ReadResult<(Vec<TopTrade>, Vec<TopTrade>)> {
    let convert = |row: &TradeRow| -> rusqlite::Result<TopTrade> {
        let core_uid = row.core_uid.ok_or_else(|| {
            rusqlite::Error::InvalidColumnType(
                6,
                "strategyid/core_uid".to_string(),
                rusqlite::types::Type::Null,
            )
        })?;
        let strategy_text = row.strategy_text.clone().ok_or_else(|| {
            rusqlite::Error::InvalidColumnType(
                6,
                "strategyid/core_uid".to_string(),
                rusqlite::types::Type::Null,
            )
        })?;
        let pair = row.strategy_id.map(|strategy_id| (strategy_id, core_uid));
        Ok(TopTrade {
            closedate: axis.to_utc(row.closedate, core_uid as u64),
            coin: row.coin.clone(),
            strategy: pair
                .and_then(|pair| metadata.and_then(|all| all.get(&pair)))
                .and_then(|item| item.name.clone())
                .unwrap_or(strategy_text),
            core_name: row.core_name.clone().ok_or_else(|| {
                rusqlite::Error::InvalidColumnType(
                    4,
                    "core_name".to_string(),
                    rusqlite::types::Type::Null,
                )
            })?,
            profit: row.pnl.unwrap_or(0.0),
            is_short: row.is_short,
        })
    };
    let best = best_rows
        .iter()
        .take(5)
        .map(convert)
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| read_fail("analytics: summary top rows", error))?;
    let worst = worst_rows
        .iter()
        .take(5)
        .map(convert)
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| read_fail("analytics: summary top rows", error))?;
    Ok((best, worst))
}

/// Fold strategy/core aggregates into the one-day kind chart.
///
/// Args:
///     groups: Strategy aggregates carrying core identity and profit.
///     metadata: Optional version metadata supplying strategy kinds.
///
/// Returns:
///     Kind aggregates ordered by profit, each with ordered core contributions.
fn finish_kinds(
    groups: &HashMap<String, GroupAccumulator>,
    metadata: Option<&HashMap<(i64, i64), StrategyMetadata>>,
) -> Vec<KindStat> {
    let mut kinds = HashMap::<String, HashMap<i64, KindCore>>::new();
    for group in groups.values() {
        let pair = group.strategy_id.zip(group.strategy_core);
        let kind = metadata
            .and_then(|all| pair.and_then(|pair| all.get(&pair)))
            .map(|item| item.kind.clone())
            .unwrap_or_default();
        let core_uid = group.strategy_core.unwrap_or_default();
        let core = kinds
            .entry(kind)
            .or_default()
            .entry(core_uid)
            .or_insert(KindCore {
                uid: core_uid as u64,
                name: group.core_name.clone().unwrap_or_default(),
                profit: 0.0,
                trades: 0,
            });
        if let Some(name) = &group.core_name {
            if name > &core.name {
                core.name = name.clone();
            }
        }
        core.profit += group.profit;
        core.trades += group.n;
    }
    let mut output = kinds
        .into_iter()
        .map(|(kind, cores)| {
            let mut cores = cores.into_values().collect::<Vec<_>>();
            cores.sort_by(|left, right| right.profit.total_cmp(&left.profit));
            KindStat {
                kind,
                profit: cores.iter().map(|core| core.profit).sum(),
                trades: cores.iter().map(|core| core.trades).sum(),
                cores,
            }
        })
        .collect::<Vec<_>>();
    output.sort_by(|left, right| right.profit.total_cmp(&left.profit));
    output
}
