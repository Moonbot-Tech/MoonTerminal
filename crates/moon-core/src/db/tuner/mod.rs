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
    coin_groups_on, strategies_for_coins_on, unified_from, GroupStat, HourStat, Query,
};
use super::metrics::{improvement_margin, profit_factor, winrate};
use super::read_fail::read_fail;
use super::{ReadFail, ReadResult};

mod fields;
mod strategy_read;
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
            let wk = format!(
                "((((({OPEN_TS} / 86400) + 4) % 7 + 6) % 7) * 1440 + ({OPEN_TS} % 86400) / 60)"
            );
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
        TimeWindow::Day(f, t) => (format!("(({OPEN_TS} % 86400) / 60)"), f, t, 1439),
        TimeWindow::Hour(f, t) => (
            format!("((({OPEN_TS} % 86400) / 60) % 60)"),
            f as u16,
            t as u16,
            59,
        ),
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
    /// Average entry size (`spentbtc`; our quote currency is USDT).
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
pub fn filter_tuner_data(
    q: &Query,
    variants: &[Variant],
    field: &str,
    buckets: usize,
) -> FilterTunerData {
    let compound = super::open_reader().and_then(|conn| {
        super::with_read_snapshot(&conn, |snapshot| {
            Ok(FilterTunerData {
                stats: variant_stats_on(snapshot, q, variants),
                histogram: histogram_on(snapshot, q, field, buckets),
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
            let slider = time::slider_profiles_on(snapshot, q);
            let profiles = slider
                .as_ref()
                .map(|profiles| vec![profiles.entry_hours])
                .map_err(Clone::clone);
            Ok(TimeTunerData {
                profiles,
                stats: variant_stats_on(snapshot, q, variants),
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
            Ok(CoinTunerData {
                stats: coin_groups_on(snapshot, q),
                kpi: variant_stats_on(snapshot, q, variants),
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
///     The floored query and `FROM` source, or `NotReady` when no source can answer.
fn tuner_source_on(conn: &Connection, q: &Query) -> ReadResult<(Query, String)> {
    let mut q = q.clone();
    q.floor_all_history();
    let Some(src) = unified_from(conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
    Ok((q, src))
}

/// Open a reader and build the unified tuner source for `q`, applying the tuner's all-history
/// floor (`from = 1`, the same floor `coin_groups`/`strategies_for_coins` use so the coin
/// table and the KPI matrix cover one period). `NotReady` when no source carries the required
/// schema. Returns the connection, the floored query, and the `FROM` source string.
///
/// The plain-reader preamble shared by every non-snapshot tuner scan; `variant_stats` pins a
/// WAL snapshot of its own instead and does not use this.
pub(super) fn open_tuner_source(q: &Query) -> ReadResult<(Connection, Query, String)> {
    let conn = super::open_reader()?;
    let (q, src) = tuner_source_on(&conn, q)?;
    Ok((conn, q, src))
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
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(ctx, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?))
        })
        .map_err(|e| read_fail(ctx, e))?;
    let mut out: Vec<(f64, f64)> = Vec::new();
    for row in rows {
        let pair = row.map_err(|e| read_fail(ctx, e))?;
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
    let sql = format!(
        "SELECT ((({OPEN_TS} / 86400) + 4) % 7 + 6) % 7 AS wd,
                ({OPEN_TS} % 86400) / 60 AS mn,
                COALESCE(o.pnl, 0)
         FROM {src}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(ctx, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })
        .map_err(|e| read_fail(ctx, e))?;
    for row in rows {
        let (weekday, minute, profit) = row.map_err(|e| read_fail(ctx, e))?;
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
    variants
        .iter()
        .map(|variant| one_variant(conn, &src, &q, variant))
        .collect()
}

/// Scan one variant in `closedate` order, failing if any metric row is unreadable.
fn one_variant(conn: &Connection, src: &str, q: &Query, v: &Variant) -> ReadResult<VarStats> {
    const CTX: &str = "tuner: one_variant";
    let mut st = VarStats::default();
    let wh = v.where_sql();
    let sql = format!(
        "SELECT COALESCE(o.pnl,0), COALESCE(o.spentbtc,0)
         FROM {src} WHERE 1=1{wh} ORDER BY o.closedate"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let (mut wsum, mut lsum, mut spent) = (0.0f64, 0.0f64, 0.0f64);
    let (mut cum, mut peak) = (0.0f64, 0.0f64);
    for row in rows {
        // profit/spent are the whole point of the variant KPI — never skip.
        let (profit, sp) = row.map_err(|e| read_fail(CTX, e))?;
        st.n += 1;
        st.profit += profit;
        spent += sp;
        if profit > 0.0 {
            st.wins += 1;
            wsum += profit;
        } else {
            lsum -= profit;
        }
        cum += profit;
        peak = peak.max(cum);
        st.max_dd = st.max_dd.max(peak - cum);
    }
    if st.n > 0 {
        st.avg = st.profit / st.n as f64;
        st.avg_spent = spent / st.n as f64;
        st.avg_win = if st.wins > 0 {
            wsum / st.wins as f64
        } else {
            0.0
        };
        let losses = st.n - st.wins;
        st.avg_loss = if losses > 0 {
            lsum / losses as f64
        } else {
            0.0
        };
        st.pf = profit_factor(wsum, lsum);
    }
    Ok(st)
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
    let mut pairs = scan_field_pairs(conn, &q, &src, field, "tuner: histogram")?;
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
/// filter. Shared by `best_range` and `tuner_smart::smart_suggest`.
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
    let (conn, q, src) = open_tuner_source(q)?;
    let mut vals = scan_field_pairs(&conn, &q, &src, field, "tuner: suggest_field")?;
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
    let total = pre[len];
    let mut best: Option<(f64, usize, usize)> = None;
    for i in 0..edges {
        for j in (i + 1)..=edges {
            let (a, b) = (pos[i], pos[j]);
            if b - a < min_n {
                continue;
            }
            // Full data coverage (min..max) is a no-op filter, not a candidate.
            if a == 0 && b == len {
                continue;
            }
            let profit = pre[b] - pre[a];
            if best.is_none_or(|(bp, _, _)| profit > bp) {
                best = Some((profit, i, j));
            }
        }
    }
    // Suggest a range only when it REALLY improves profit over no filter by more than
    // floating-point summation noise. Otherwise a range excluding only zero-profit trades
    // can appear to "win" by about 1e-12.
    let margin = improvement_margin(total);
    best.filter(|(profit, _, _)| *profit > total + margin)
        .map(|(profit, i, j)| {
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
                profit,
                n: (b - a) as i64,
            }
        })
}

#[cfg(test)]
mod tests;
