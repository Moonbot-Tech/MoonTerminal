//! The Analytics window's filter tuner, adapted from Analytics V3 (an Excel dashboard):
//! threshold what-if analysis for report market fields. It calculates the "Fact vs variants"
//! KPIs (a variant is a set of lower/upper ranges on entry fields) and a profit-distribution
//! histogram over QUANTILE buckets for the selected field. V3's fixed scale does not fit our
//! data: values are percentages with extreme outliers, while hvol/dvol are volumes. The source
//! is the same replica-and-legacy UNION used by `analytics`.
//!
//! Query and evaluation entry points that scan report periods belong on a background
//! executor; pure formatting and metadata helpers do not scan those periods.

use rusqlite::Connection;

use super::analytics::{
    coin_groups_from_source, strategies_for_coins_on, GroupStat, HourStat, Query,
};
use super::metrics::{improvement_margin, winrate, Tally};
use super::read_fail::read_fail_on;
use super::{ReadFail, ReadResult};

mod fields;
/// The shared "best contiguous slice by profit" kernel both searches decide with.
mod range_pick;
mod strategy_read;
/// Automatic threshold search over all fields at once, scan plus DB-free optimizer.
pub mod threshold_search;
mod time;

pub use fields::{slot_type_for, FieldClass, FieldSpec, FIELDS};
pub use strategy_read::{
    strategy_cores, strategy_current_values, strategy_current_values_opt, strategy_filters,
    StratFilters,
};
pub use time::{
    format_week_span, format_working_time, slider_profiles, suggest_time, SliderProfiles, TimeAxes,
    TimeSuggest, TimeWindow,
};

/// Range for one field; `None` means the bound is unset.
#[derive(Clone, Debug, Default)]
pub struct Bound {
    pub field: String,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

/// Trade OPEN time for schedules: WorkingTime/WorkingWeekTime gate ENTRY into a trade, so the
/// day and minute come from `buydate` (when the trade opened), not `closedate` (when it closed).
/// Fall back to `closedate` when the open time is missing (0/NULL).
const OPEN_TS: &str = "COALESCE(NULLIF(o.buydate, 0), o.closedate)";

/// A "what-if" variant = extra conditions on top of the base selection. Empty = "Fact".
/// The "By filter" axis sets `bounds` (field ranges); "By time" — two INDEPENDENT
/// strategy fields combined with AND: `week_span` (WorkingWeekTime — a continuous span
/// over the MINUTE OF THE WEEK) and `tod` (WorkingTime — a single time window);
/// "By coin" — `coins` (a set of coins). Every condition folds into ONE WHERE, which is
/// what keeps this struct universal across the axes.
#[derive(Clone, Debug, Default)]
pub struct Variant {
    pub bounds: Vec<Bound>,
    /// WorkingWeekTime: continuous inclusive span over the MINUTE OF THE WEEK `(from, to)`,
    /// where the week minute is `day*1440 + minute_of_day` (0..10079, day 0=Mon..6=Sun).
    /// `from > to` wraps from Sunday to Monday; `None` means unrestricted.
    pub week_span: Option<(u16, u16)>,
    /// WorkingTime: one time window. `None` means unrestricted by time.
    pub tod: Option<TimeWindow>,
    /// The "By coin" axis, whitelist side: `Some(list)` keeps ONLY those coins, `None`
    /// places no restriction. Names are exactly as the `coin` column holds them — the
    /// caller expands its coin tokens against the very grouping that draws the table.
    ///
    /// `Some(empty)` is NOT the same as `None`: it means an active whitelist that no
    /// traded coin satisfies, which must keep nothing. Modelled as an option precisely so
    /// that case cannot collapse into "no whitelist at all" and score the fact instead.
    pub coins_in: Option<Vec<String>>,
    /// The "By coin" axis, blacklist side: trades of these coins are EXCLUDED. Applied on
    /// top of `coins_in`, mirroring how a strategy evaluates its two lists.
    pub coins_out: Vec<String>,
}

impl Variant {
    /// Does this variant add nothing, i.e. is it the "Fact" column?
    ///
    /// Asked of `where_sql` rather than re-listing the dimensions: a second listing is a
    /// second place to remember a new axis, and the two had already drifted — `where_sql`
    /// gates `bounds` through the `FIELDS` whitelist while a hand-written check did not,
    /// so a bound on an unknown field claimed "not the fact" over an EMPTY condition.
    pub fn is_empty(&self) -> bool {
        self.where_sql().is_empty()
    }

    /// Variant WHERE suffix. Fields are gated through the `FIELDS` whitelist; NULL counts as
    /// zero (as in other report filters); numbers are literals from form-provided f64 values,
    /// so they cannot inject SQL. Hours, days, and minutes are integers and likewise safe.
    fn where_sql(&self) -> String {
        let mut w = String::new();
        for b in &self.bounds {
            if !FIELDS.iter().any(|s| s.col == b.field) {
                continue;
            }
            if let Some(v) = b.from.filter(|v| v.is_finite()) {
                w.push_str(&format!(" AND COALESCE(o.\"{}\",0) >= {v}", b.field));
            }
            if let Some(v) = b.to.filter(|v| v.is_finite()) {
                w.push_str(&format!(" AND COALESCE(o.\"{}\",0) <= {v}", b.field));
            }
        }
        if let Some((f, t)) = self.week_span {
            // Week minute from OPEN time = day*1440 + minute_of_day (0..10079,
            // day 0=Mon..6=Sun); a continuous span with `from > to` wrapping Sun -> Mon.
            let wk = "o.__mt_week";
            let (f, t) = (f.min(10079), t.min(10079));
            if f <= t {
                w.push_str(&format!(" AND ({wk} BETWEEN {f} AND {t})"));
            } else {
                w.push_str(&format!(" AND ({wk} <= {t} OR {wk} >= {f})"));
            }
        }
        if let Some(tw) = self.tod {
            w.push_str(&time_window_where(tw));
        }
        // The variant's only STRING terms — every other one is a number from the form. The
        // names come out of the same replica's `coin` column, but they still go through the
        // shared escaper rather than being interpolated here.
        if let Some(list) = &self.coins_in {
            if list.is_empty() {
                // An active whitelist matching nothing keeps nothing. `IN ()` is not valid
                // SQL, so the impossible predicate is written out explicitly.
                w.push_str(" AND 0=1");
            } else {
                w.push_str(" AND COALESCE(o.coin,'') IN (");
                w.push_str(&sql_str_list(list));
                w.push(')');
            }
        }
        if !self.coins_out.is_empty() {
            w.push_str(" AND COALESCE(o.coin,'') NOT IN (");
            w.push_str(&sql_str_list(&self.coins_out));
            w.push(')');
        }
        w
    }
}

/// A comma-separated list of SQL string literals, each quote doubled per SQLite.
///
/// The variant's only STRING literals live here so the escaping rule sits in ONE place
/// rather than inside whichever axis happened to need it first.
pub(super) fn sql_str_list(items: &[String]) -> String {
    let mut out = String::new();
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('\'');
        // Doubling is the whole escape: one raw apostrophe would end the literal early
        // and break the WHOLE WHERE, not just its own term.
        for ch in s.chars() {
            if ch == '\'' {
                out.push('\'');
            }
            out.push(ch);
        }
        out.push('\'');
    }
    out
}

/// SQL condition for the `WorkingTime` field. `Day` uses the minute of day from `OPEN_TS` in
/// 0..1439; `Hour` uses the minute within the hour from `OPEN_TS` in 0..59. A window with
/// `from <= to` uses `BETWEEN`; `from > to` wraps (`<= to` OR `>= from`), because a reversed
/// `BETWEEN` would silently select zero trades. Integer literals cannot inject SQL.
fn time_window_where(tw: TimeWindow) -> String {
    // Calculate the minute from OPEN TIME because the schedule gates entry.
    let (expr, f, t, hi): (String, u16, u16, u16) = match tw {
        TimeWindow::Day(f, t) => ("o.__mt_day".to_string(), f, t, 1439),
        TimeWindow::Hour(f, t) => ("(o.__mt_day % 60)".to_string(), f as u16, t as u16, 59),
    };
    let (f, t) = (f.min(hi), t.min(hi));
    if f <= t {
        format!(" AND ({expr} BETWEEN {f} AND {t})")
    } else {
        format!(" AND ({expr} <= {t} OR {expr} >= {f})")
    }
}

/// KPI values for one column of the "Fact vs v1..vN" matrix.
#[derive(Clone, Debug, Default)]
pub struct VarStats {
    pub n: i64,
    pub wins: i64,
    pub profit: f64,
    pub pf: f64,
    pub avg: f64,
    /// Average win and absolute average loss.
    pub avg_win: f64,
    pub avg_loss: f64,
    /// Average entry size in the group's exact persisted quote currency (`spentbtc`).
    pub avg_spent: f64,
    pub max_dd: f64,
}

impl VarStats {
    pub fn winrate(&self) -> f64 {
        winrate(self.wins, self.n)
    }
}

/// Report-derived results rendered together by the By-time tuner axis.
pub struct TimeTunerData {
    /// Hour-of-day profile for the visible period.
    pub profiles: ReadResult<Vec<[HourStat; 24]>>,
    /// Fact-versus-schedule KPI values.
    pub stats: ReadResult<Vec<VarStats>>,
    /// Color profiles for the three schedule sliders.
    pub slider: ReadResult<SliderProfiles>,
}

/// Report-derived results rendered together by the By-filter tuner axis.
pub struct FilterTunerData {
    /// Fact-versus-threshold KPI values.
    pub stats: ReadResult<Vec<VarStats>>,
    /// Distribution for the currently selected report field.
    pub histogram: ReadResult<Vec<HistBucket>>,
}

/// Read the By-filter KPI and histogram from one SQLite snapshot.
///
/// Args:
///     q: Shared report scope and profit metric.
///     variants: Ordered baseline and threshold counterfactuals.
///     field: Whitelisted field rendered by the histogram.
///     buckets: Requested approximate histogram bucket count.
///
/// Returns:
///     Independently classified KPI and histogram results from one validated source.
pub fn filter_tuner_data(
    q: &Query,
    variants: &[Variant],
    field: &str,
    buckets: usize,
) -> FilterTunerData {
    let compound = super::open_reader().and_then(|conn| {
        super::with_read_snapshot(&conn, |snapshot| {
            let (query, source) = match tuner_source_on(snapshot, q) {
                Ok(source) => source,
                Err(error) => {
                    return Ok(FilterTunerData {
                        stats: Err(error.clone()),
                        histogram: Err(error),
                    });
                }
            };
            Ok(FilterTunerData {
                stats: variant_stats_from_source(snapshot, &query, &source, variants),
                histogram: histogram_from_source(snapshot, &query, &source, field, buckets),
            })
        })
    });
    match compound {
        Ok(data) => data,
        Err(error) => FilterTunerData {
            stats: Err(error.clone()),
            histogram: Err(error),
        },
    }
}

/// Read every report-derived By-time result from one SQLite snapshot.
///
/// Args:
///     q: Shared report scope and period.
///     variants: Ordered baseline and schedule counterfactuals.
///
/// Returns:
///     Independently classified surface results that all observed one committed generation.
pub fn time_tuner_data(q: &Query, variants: &[Variant]) -> TimeTunerData {
    let compound = super::open_reader().and_then(|conn| {
        super::with_read_snapshot(&conn, |snapshot| {
            let (query, source) = match tuner_source_on(snapshot, q) {
                Ok(source) => source,
                Err(error) => {
                    return Ok(TimeTunerData {
                        profiles: Err(error.clone()),
                        stats: Err(error.clone()),
                        slider: Err(error),
                    });
                }
            };
            let slider = time::slider_profiles_from_source(snapshot, &query, &source);
            let profiles = slider
                .as_ref()
                .map(|profiles| vec![profiles.entry_hours])
                .map_err(Clone::clone);
            Ok(TimeTunerData {
                profiles,
                stats: variant_stats_from_source(snapshot, &query, &source, variants),
                slider,
            })
        })
    });
    match compound {
        Ok(data) => data,
        Err(error) => TimeTunerData {
            profiles: Err(error.clone()),
            stats: Err(error.clone()),
            slider: Err(error),
        },
    }
}

/// Report-derived results rendered together by the By-coin tuner axis.
pub struct CoinTunerData {
    /// Per-coin aggregates for the selected strategy scope.
    pub stats: ReadResult<Vec<GroupStat>>,
    /// Fact-versus-working-list KPI values.
    pub kpi: ReadResult<Vec<VarStats>>,
    /// Strategy keys behind the currently picked coins.
    pub picked_strategies: ReadResult<Vec<String>>,
}

/// Read every report-derived By-coin result from one SQLite snapshot.
///
/// Args:
///     q: Selected-strategy report scope used by the table and KPI.
///     variants: Ordered baseline and working-list counterfactuals.
///     picked_q: All-strategies scope used by the picked-coin highlight.
///     picked: Exact report coin names currently selected.
///
/// Returns:
///     Independently classified surface results that all observed one committed generation.
pub fn coin_tuner_data(
    q: &Query,
    variants: &[Variant],
    picked_q: &Query,
    picked: &[String],
) -> CoinTunerData {
    let compound = super::open_reader().and_then(|conn| {
        super::with_read_snapshot(&conn, |snapshot| {
            let source = tuner_source_on(snapshot, q);
            let (stats, kpi) = match source {
                Ok((query, source)) => (
                    coin_groups_from_source(snapshot, &query, &source),
                    variant_stats_from_source(snapshot, &query, &source, variants),
                ),
                Err(error) => (Err(error.clone()), Err(error)),
            };
            Ok(CoinTunerData {
                stats,
                kpi,
                picked_strategies: strategies_for_coins_on(snapshot, picked_q, picked),
            })
        })
    });
    match compound {
        Ok(data) => data,
        Err(error) => CoinTunerData {
            stats: Err(error.clone()),
            kpi: Err(error.clone()),
            picked_strategies: Err(error),
        },
    }
}

/// Build the unified tuner source on an existing connection, applying the shared all-history
/// floor.
///
/// Args:
///     conn: Existing report reader or compound-read snapshot.
///     q: Report scope and period.
///
/// Returns:
///     The floored query and `FROM` source, `IncomparableQuote` for unsafe raw money, or
///     `NotReady` when no source can answer.
fn tuner_source_on(conn: &Connection, q: &Query) -> ReadResult<(Query, String)> {
    let mut q = q.clone();
    q.floor_all_history();
    let projection = crate::db::analytics::projection_mode_on(conn, &q)?;
    let Some(src) = crate::db::analytics::unified_from_mode(conn, &q, projection)? else {
        return Err(ReadFail::NotReady);
    };
    Ok((q, src))
}

/// Run quote preflight and row materialization inside one pinned tuner snapshot.
///
/// Args:
///     conn: Open report reader used to create the snapshot.
///     q: Report scope and period before the tuner all-history floor.
///     read: Row materializer that must finish before the snapshot is released.
///
/// Returns:
///     Materialized input for CPU-only optimization, or a classified read failure.
fn tuner_read_on<T>(
    conn: &Connection,
    q: &Query,
    read: impl FnOnce(&Connection, &Query, &str) -> ReadResult<T>,
) -> ReadResult<T> {
    super::with_read_snapshot(conn, |snapshot| {
        let (q, src) = tuner_source_on(snapshot, q)?;
        read(snapshot, &q, &src)
    })
}

/// Open a reader and materialize tuner rows in the same snapshot as quote preflight.
///
/// The snapshot ends when `read` returns, before the caller performs CPU-heavy optimization.
/// This prevents a report commit from adding another quote between scope validation and the
/// scan without holding a read transaction for the search itself.
///
/// Args:
///     q: Report scope and period before the tuner all-history floor.
///     read: Row materializer executed inside the pinned snapshot.
///
/// Returns:
///     Materialized tuner input, `NotReady`, `IncomparableQuote`, or a classified read failure.
pub(super) fn read_tuner_rows<T>(
    q: &Query,
    read: impl FnOnce(&Connection, &Query, &str) -> ReadResult<T>,
) -> ReadResult<T> {
    let conn = super::open_reader()?;
    tuner_read_on(&conn, q, read)
}

/// Scan one field into `(value, pnl)` pairs over the period, dropping NULL and non-finite
/// values (COALESCE gives NULL pnl a 0). Shared by `histogram` and `suggest_field`; `ctx`
/// names the CALLER for the read-failure log, so a failure keeps pointing at the surface the
/// user is looking at rather than at this shared helper.
fn scan_field_pairs(
    conn: &Connection,
    q: &Query,
    src: &str,
    field: &str,
    ctx: &'static str,
) -> ReadResult<Vec<(f64, f64)>> {
    let sql = format!(
        "SELECT o.\"{field}\", COALESCE(o.pnl,0)
         FROM {src} WHERE o.\"{field}\" IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, ctx, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?))
        })
        .map_err(|e| read_fail_on(conn, ctx, e))?;
    let mut out: Vec<(f64, f64)> = Vec::new();
    for row in rows {
        let pair = row.map_err(|e| read_fail_on(conn, ctx, e))?;
        if pair.0.is_finite() {
            out.push(pair);
        }
    }
    Ok(out)
}

/// Scan the period into `(weekday, minute_of_day, pnl)` rows from the trade OPEN time
/// (`OPEN_TS`): weekday `0=Mon..6=Sun`, minute `0..1439`. Shared by `suggest_time` and
/// `slider_profiles`, whose only difference is what they do with the rows afterward; `ctx`
/// names the CALLER for the read-failure log.
fn scan_time_rows(
    conn: &Connection,
    q: &Query,
    src: &str,
    ctx: &'static str,
) -> ReadResult<Vec<(i64, i64, f64)>> {
    let mut out = Vec::new();
    visit_time_rows(conn, q, src, ctx, |weekday, minute, profit| {
        out.push((weekday, minute, profit));
    })?;
    Ok(out)
}

/// Stream `(weekday, minute_of_day, pnl)` rows to a bounded-memory consumer.
fn visit_time_rows(
    conn: &Connection,
    q: &Query,
    src: &str,
    ctx: &'static str,
    mut visit: impl FnMut(i64, i64, f64),
) -> ReadResult<()> {
    super::analytics::time_zone::install(conn, &q.axis)
        .map_err(|e| read_fail_on(conn, ctx, e))?;
    let sql = format!(
        "SELECT mt_minute_of_week({OPEN_TS}, o.core_uid) / 1440 AS wd,
                mt_minute_of_day({OPEN_TS}, o.core_uid) AS mn,
                COALESCE(o.pnl, 0)
         FROM {src}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, ctx, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })
        .map_err(|e| read_fail_on(conn, ctx, e))?;
    for row in rows {
        let (weekday, minute, profit) = row.map_err(|e| read_fail_on(conn, ctx, e))?;
        visit(weekday, minute, profit);
    }
    Ok(())
}

/// Compute KPI values in input order; an empty variant represents the baseline.
///
/// A healthy empty period produces one zero-valued KPI result per variant.
/// Returns `NotReady` when the replica or required schema is absent and `Failed`
/// when opening the replica, pinning the snapshot, or scanning a variant fails.
pub fn variant_stats(q: &Query, variants: &[Variant]) -> ReadResult<Vec<VarStats>> {
    let conn = super::open_reader()?;
    super::with_read_snapshot(&conn, |snapshot| variant_stats_on(snapshot, q, variants))
}

/// Compute variant KPIs on an existing connection or compound-read snapshot.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should be queried.
///     q: Report scope and period.
///     variants: Ordered baseline and counterfactual definitions.
///
/// Returns:
///     KPI values in input order or a classified read failure.
fn variant_stats_on(
    conn: &Connection,
    q: &Query,
    variants: &[Variant],
) -> ReadResult<Vec<VarStats>> {
    let (q, src) = tuner_source_on(conn, q)?;
    variant_stats_from_source(conn, &q, &src, variants)
}

/// Compute variant KPIs from a source already validated in the same snapshot.
///
/// Args:
///     conn: Pinned report snapshot.
///     q: Floored tuner query used by `src`.
///     src: Unified source whose quote scope was already validated.
///     variants: Ordered baseline and counterfactual definitions.
///
/// Returns:
///     KPI values in input order or a classified row-scan failure.
fn variant_stats_from_source(
    conn: &Connection,
    q: &Query,
    src: &str,
    variants: &[Variant],
) -> ReadResult<Vec<VarStats>> {
    const CTX: &str = "tuner: variant_stats";
    if variants.is_empty() {
        return Ok(Vec::new());
    }
    super::analytics::time_zone::install(conn, &q.axis)
        .map_err(|e| read_fail_on(conn, CTX, e))?;
    let sql = variant_stats_sql(src, variants);
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |row| {
            let mut matched = Vec::with_capacity(variants.len());
            for index in 0..variants.len() {
                matched.push(row.get::<_, i64>(index + 2)? != 0);
            }
            Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?, matched))
        })
        .map_err(|e| read_fail_on(conn, CTX, e))?;
    let mut tallies = vec![Tally::default(); variants.len()];
    let mut spent = vec![0.0f64; variants.len()];
    for row in rows {
        let (profit, row_spent, matched) = row.map_err(|e| read_fail_on(conn, CTX, e))?;
        for (index, is_match) in matched.into_iter().enumerate() {
            if is_match {
                tallies[index].push(profit);
                spent[index] += row_spent;
            }
        }
    }
    Ok(tallies
        .into_iter()
        .zip(spent)
        .map(|(tally, spent)| stats_from_tally(tally, spent))
        .collect())
}

/// Build the single ordered statement that evaluates every tuner variant.
///
/// Args:
///     src: Unified active-lens report source.
///     variants: Ordered baseline and counterfactual definitions.
///
/// Returns:
///     SQL with one source scan and a Boolean match column per variant.
fn variant_stats_sql(src: &str, variants: &[Variant]) -> String {
    let needs_week = variants.iter().any(|variant| variant.week_span.is_some());
    let needs_day = variants.iter().any(|variant| variant.tod.is_some());
    let mut projections = Vec::new();
    if needs_week {
        projections.push(format!("mt_minute_of_week({OPEN_TS}, o.core_uid) AS __mt_week"));
    }
    if needs_day {
        projections.push(format!("mt_minute_of_day({OPEN_TS}, o.core_uid) AS __mt_day"));
    }
    let (prefix, source) = if projections.is_empty() {
        (String::new(), src.to_string())
    } else {
        (
            format!(
                "WITH projected AS MATERIALIZED (SELECT o.*, {} FROM {src}) ",
                projections.join(", ")
            ),
            "projected o".to_string(),
        )
    };
    let matches = variants
        .iter()
        .enumerate()
        .map(|(index, variant)| {
            format!(
                "CASE WHEN 1=1{} THEN 1 ELSE 0 END AS v{index}",
                variant.where_sql()
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{prefix}SELECT COALESCE(o.pnl,0), COALESCE(o.spentbtc,0), {matches}
         FROM {source}
         ORDER BY o.closedate, COALESCE(o.pnl,0), COALESCE(o.spentbtc,0)"
    )
}

/// Finalize one variant tally with its independently accumulated entry spend.
///
/// Args:
///     tally: Chronological profit sequence aggregate.
///     spent: Sum of entry sizes for the same matched rows.
///
/// Returns:
///     Complete KPI values for one variant.
fn stats_from_tally(tally: Tally, spent: f64) -> VarStats {
    let mut stats = VarStats {
        n: tally.n,
        wins: tally.wins,
        profit: tally.profit,
        max_dd: tally.max_dd,
        ..VarStats::default()
    };
    if stats.n > 0 {
        stats.avg = tally.avg();
        stats.avg_win = tally.avg_win();
        stats.avg_loss = tally.avg_loss();
        stats.pf = tally.profit_factor();
        stats.avg_spent = spent / stats.n as f64;
    }
    stats
}

/// Histogram bucket: `[lo, hi)`, with the last bucket including `hi`.
#[derive(Clone, Debug)]
pub struct HistBucket {
    pub lo: f64,
    pub hi: f64,
    pub n: i64,
    pub wins: i64,
    /// Bucket's sum of wins and absolute sum of losses.
    pub wsum: f64,
    pub lsum: f64,
}

/// Build at most `want` approximately equal-population buckets for one field.
///
/// NULL field values are excluded. An unknown field or healthy period without
/// values returns an empty vector. `NotReady` means the replica or required
/// schema is absent; `Failed` means opening or scanning it failed.
pub fn histogram(q: &Query, field: &str, want: usize) -> ReadResult<Vec<HistBucket>> {
    let conn = super::open_reader()?;
    super::with_read_snapshot(&conn, |snapshot| histogram_on(snapshot, q, field, want))
}

/// Build histogram buckets on an existing connection or compound snapshot.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     q: Report scope and profit metric.
///     field: Whitelisted report field to bucket.
///     want: Requested approximate bucket count.
///
/// Returns:
///     Histogram buckets or a classified read failure.
fn histogram_on(
    conn: &Connection,
    q: &Query,
    field: &str,
    want: usize,
) -> ReadResult<Vec<HistBucket>> {
    if !FIELDS.iter().any(|s| s.col == field) {
        // Programmer error (unknown field), not a read failure.
        return Ok(Vec::new());
    }
    let (q, src) = tuner_source_on(conn, q)?;
    histogram_from_source(conn, &q, &src, field, want)
}

/// Build histogram buckets from a source already validated in the same snapshot.
///
/// Args:
///     conn: Pinned report snapshot.
///     q: Floored tuner query used by `src`.
///     src: Unified source whose quote scope was already validated.
///     field: Whitelisted report field to bucket.
///     want: Requested approximate bucket count.
///
/// Returns:
///     Histogram buckets or a classified row-scan failure.
fn histogram_from_source(
    conn: &Connection,
    q: &Query,
    src: &str,
    field: &str,
    want: usize,
) -> ReadResult<Vec<HistBucket>> {
    if !FIELDS.iter().any(|spec| spec.col == field) {
        return Ok(Vec::new());
    }
    let mut pairs = scan_field_pairs(conn, q, src, field, "tuner: histogram")?;
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Quantile edges for `want` equally populated buckets; collapse duplicate edges
    // for fields with many identical values or zeros.
    let want = want.clamp(2, 64).min(pairs.len().max(2));
    let mut edges: Vec<f64> = Vec::with_capacity(want + 1);
    for i in 0..=want {
        let idx = (i * (pairs.len() - 1)) / want;
        let e = pairs[idx].0;
        if edges.last().is_none_or(|l| *l < e) {
            edges.push(e);
        }
    }
    if edges.len() < 2 {
        // Every value is identical, so use one bucket.
        edges = vec![pairs[0].0, pairs[0].0];
    }

    let nb = edges.len() - 1;
    let mut out: Vec<HistBucket> = (0..nb)
        .map(|i| HistBucket {
            lo: edges[i],
            hi: edges[i + 1],
            n: 0,
            wins: 0,
            wsum: 0.0,
            lsum: 0.0,
        })
        .collect();
    let mut bi = 0usize;
    for (v, profit) in pairs {
        while bi + 1 < nb && v >= out[bi].hi {
            bi += 1;
        }
        let b = &mut out[bi];
        b.n += 1;
        if profit > 0.0 {
            b.wins += 1;
            b.wsum += profit;
        } else {
            b.lsum -= profit;
        }
    }
    Ok(out)
}

/// Automatic-suggestion result: the best range for a field.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub from: Option<f64>,
    pub to: Option<f64>,
    /// Period profit under this filter and the number of remaining trades.
    pub profit: f64,
    pub n: i64,
}

/// Smart rounding for a suggested bound: three significant digits based on magnitude,
/// OUTWARD (`up=false` rounds a lower bound down; `up=true` rounds an upper bound up), so
/// the rounded range does not exclude any selected trades.
pub fn round_bound(v: f64, up: bool) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let mag = v.abs().log10().floor() as i32;
    let step = 10f64.powi(mag - 2);
    let r = if up {
        (v / step).ceil()
    } else {
        (v / step).floor()
    };
    r * step
}

/// Round `(from, to)` outward to three significant digits, but keep the RAW pair when doing so
/// would push both bounds past the observed range `[lo, hi]` and turn the pair into a no-op
/// filter. Shared by `best_range` and `threshold_search::suggest`.
pub(super) fn round_pair_outward(from: f64, to: f64, lo: f64, hi: f64) -> (f64, f64) {
    let (rf, rt) = (round_bound(from, false), round_bound(to, true));
    if rf > lo || rt < hi {
        (rf, rt)
    } else {
        (from, to)
    }
}

/// Find the best threshold range for one field.
///
/// `edges` controls the quantile search resolution. With `round`, boundaries
/// round outward unless that would stop the range from filtering data.
/// `Ok(None)` means the field is unknown, the sample is too small, or no range
/// beats the baseline.
/// `NotReady` means the replica or required schema is absent; `Failed` means
/// opening or scanning it failed.
///
/// Args:
///     q: Report scope and profit metric.
///     field: Whitelisted report field to optimize.
///     min_n: Minimum trades retained by a candidate range.
///     edges: Quantile search resolution.
///     round: Whether accepted bounds are rounded outward.
///
/// Returns:
///     Best improving range, no suggestion, or a classified read failure.
pub fn suggest_field(
    q: &Query,
    field: &str,
    min_n: i64,
    edges: usize,
    round: bool,
) -> ReadResult<Option<Suggestion>> {
    if !FIELDS.iter().any(|s| s.col == field) {
        // Programmer error (unknown field), not a read failure.
        return Ok(None);
    }
    let mut vals = read_tuner_rows(q, |conn, q, src| {
        scan_field_pairs(conn, q, src, field, "tuner: suggest_field")
    })?;
    // The outer result reports read status; the inner option reports whether a
    // threshold improves on the baseline.
    Ok(best_range(&mut vals, min_n.max(1) as usize, edges, round))
}

/// Best range for one field over `(value, profit)` samples.
fn best_range(
    vals: &mut Vec<(f64, f64)>,
    min_n: usize,
    edges: usize,
    round: bool,
) -> Option<Suggestion> {
    if vals.len() < min_n.max(1) {
        return None;
    }
    vals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let len = vals.len();
    // Use no more buckets than data points because extra ones are redundant. The upper cap
    // was raised from 128 to 512 for the time axis: with few trades per day, this gives one
    // slice per trade and maximum window precision. `min(len)` does not affect the filter
    // RESULT: when edges >= len, positions `k*len/edges` cover exactly {0..len}, the same set
    // of distinct boundaries as edges=len, without duplicate iterations. Filter datasets
    // usually contain thousands of rows, so this branch rarely applies there.
    let edges = edges.clamp(4, 512).min(len.max(4));
    // Profit prefix sums plus quantile-edge positions.
    let mut pre = Vec::with_capacity(len + 1);
    pre.push(0.0f64);
    for (_, p) in vals.iter() {
        pre.push(pre.last().unwrap() + p);
    }
    let pos: Vec<usize> = (0..=edges).map(|k| k * len / edges).collect();
    // Cumulative profit AT each edge, which is what the shared picker slices. Full data
    // coverage (min..max) is a no-op filter, and the picker rejects it by exact count.
    let profit_at: Vec<f64> = pos.iter().map(|&p| pre[p]).collect();
    let total = pre[len];
    // Suggest a range only when it REALLY improves profit over no filter by more than
    // floating-point summation noise. Otherwise a range excluding only zero-profit trades
    // can appear to "win" by about 1e-12.
    let floor = total + improvement_margin(total);
    range_pick::best_pair(&profit_at, &pos, min_n, len, floor).map(|(i, j)| {
        let (a, b) = (pos[i], pos[j]);
        // Always return both bounds; distribution edges use the observed
        // minimum or maximum rather than an open interval.
        let (mut from, mut to) = (vals[a].0, vals[b - 1].0);
        if round {
            (from, to) = round_pair_outward(from, to, vals[0].0, vals[len - 1].0);
        }
        Suggestion {
            from: Some(from),
            to: Some(to),
            profit: pre[b] - pre[a],
            n: (b - a) as i64,
        }
    })
}

#[cfg(test)]
mod tests;
