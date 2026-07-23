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

use super::analytics::{unified_from, Query};
use super::metrics::{improvement_margin, profit_factor, winrate};
use super::read_fail::read_fail;
use super::{ReadFail, ReadResult};

/// Field class indicating which strategy Ignore flag disables its filter.
/// `IgnoreFilters` disables ALL classes; `IgnoreDelta` and `IgnoreVolume` disable their own.
/// `DeltaSlot` covers deltas without dedicated strategy parameters: they use Delta2/Delta3
/// slots (type plus min/max), so at most two can be saved in a strategy; they are ignored as
/// deltas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldClass {
    Filter,
    /// BV/SV filter: its own `UseBV_SV_Filter` switch (`false` means disabled)
    /// in addition to the general `IgnoreFilters` flag.
    BvSv,
    /// PriceBug: the Filters/Ping section with its own `IgnorePing` flag.
    Ping,
    /// Filters/Base section with its own `IgnoreBase` flag (leverage and MarkPrice delta).
    Base,
    Delta,
    DeltaSlot,
    Volume,
}

/// Description of one tuner field. This is the ONLY place to edit for a new report column or
/// strategy parameter: one row in `FIELDS`; everything else (the UNION SQL projection, UI grid,
/// chips, automatic search, and strategy persistence) is derived from this table. Columns in
/// reports.sqlite are extended automatically from the core schema (db/rep.rs).
pub struct FieldSpec {
    /// Report-replica column (lowercase).
    pub col: &'static str,
    /// Grid label (as in MB/V3).
    pub label: &'static str,
    /// Class indicating which strategy Ignore flag disables the filter.
    pub class: FieldClass,
    /// Strategy filter parameters (Min, Max); `None` means the value is not saved.
    pub p_min: Option<&'static str>,
    pub p_max: Option<&'static str>,
    /// `DeltaN_Type` value for slot fields (`class == DeltaSlot`): the threshold is saved
    /// through a Delta2/Delta3 slot instead of dedicated parameters.
    pub slot_type: Option<&'static str>,
}

const fn field(
    col: &'static str,
    label: &'static str,
    class: FieldClass,
    p_min: Option<&'static str>,
    p_max: Option<&'static str>,
    slot_type: Option<&'static str>,
) -> FieldSpec {
    FieldSpec {
        col,
        label,
        class,
        p_min,
        p_max,
        slot_type,
    }
}

impl FieldSpec {
    /// Whether the threshold can be saved to the strategy through parameters or a slot.
    /// Unmapped fields (da1m, d5s) are marked in the grid and excluded from automatic
    /// suggestions by default.
    pub fn mapped(&self) -> bool {
        self.p_min.is_some() || self.p_max.is_some() || self.slot_type.is_some()
    }
}

impl FieldClass {
    /// Parent MoonBot section: BV/SV lives under Filters/Volume, Delta2/Delta3 slots live
    /// under Filters/Delta, and every other class is its own parent.
    pub fn parent(self) -> FieldClass {
        match self {
            FieldClass::BvSv => FieldClass::Volume,
            FieldClass::DeltaSlot => FieldClass::Delta,
            other => other,
        }
    }
}

/// Report fields available to filters. This is the ONLY source of column names allowed into
/// tuner SQL (the whitelist). The order matches the grid and the MoonBot Filters sections:
/// Base -> Ping -> Volume (with BV/SV nested inside) -> Delta (with Delta2/Delta3 slots nested
/// inside). The parameter mapping was checked on 2026-07-17 against the union of parameters
/// from every strategy type. Fields WITHOUT parameters or a slot type (da1m, d5s) are shown
/// with a "no parameter" marker: what-if calculations work for them, but there is nowhere to
/// save the threshold in a strategy. Slot types 2h/30m/Pump5m have no report column and cannot
/// be represented.
pub const FIELDS: &[FieldSpec] = &[
    // Filters/Base (IgnoreFilters | IgnoreBase): leverage and mark-price delta (±%).
    field(
        "lev",
        "Lev",
        FieldClass::Base,
        Some("MinLeverage"),
        Some("MaxLeverage"),
        None,
    ),
    field(
        "dmark",
        "dMark",
        FieldClass::Base,
        Some("MarkPriceMin"),
        Some("MarkPriceMax"),
        None,
    ),
    // Filters/Ping (IgnoreFilters | IgnorePing).
    field(
        "pricebug",
        "PriceBug",
        FieldClass::Ping,
        Some("BinancePriceBugMin"),
        Some("BinancePriceBug"),
        None,
    ),
    // Filters/Volume (IgnoreFilters | IgnoreVolume).
    field(
        "hvol",
        "H.Vol",
        FieldClass::Volume,
        Some("MinHourlyVolume"),
        Some("MaxHourlyVolume"),
        None,
    ),
    field(
        "hvolf",
        "H.VolF",
        FieldClass::Volume,
        Some("MinHourlyVolFast"),
        Some("MaxHourlyVolFast"),
        None,
    ),
    field(
        "dvol",
        "D.Vol",
        FieldClass::Volume,
        Some("MinVolume"),
        Some("MaxVolume"),
        None,
    ),
    field(
        "vd1m",
        "Vd1m",
        FieldClass::Volume,
        Some("MinuteVolDeltaMin"),
        Some("MinuteVolDeltaMax"),
        None,
    ),
    // BV/SV is a Volume SUBGROUP: its own UseBV_SV_Filter switch applies in addition to
    // IgnoreVolume; these are filter parameters, not the BV_SV_Ratio detector parameters.
    field(
        "bvsvratio",
        "bvsv",
        FieldClass::BvSv,
        Some("BV_SV_FilterRatio"),
        Some("BV_SV_FilterRatioMax"),
        None,
    ),
    // Filters/Delta (IgnoreFilters | IgnoreDelta).
    field(
        "d24h",
        "d24h",
        FieldClass::Delta,
        Some("Delta_24h_Min"),
        Some("Delta_24h_Max"),
        None,
    ),
    field(
        "d3h",
        "d3h",
        FieldClass::Delta,
        Some("Delta_3h_Min"),
        Some("Delta_3h_Max"),
        None,
    ),
    field("da1m", "da1m", FieldClass::Delta, None, None, None),
    field("d5s", "d5s", FieldClass::Delta, None, None, None),
    field(
        "btc1hdelta",
        "dBTC",
        FieldClass::Delta,
        Some("Delta_BTC_Min"),
        Some("Delta_BTC_Max"),
        None,
    ),
    field(
        "exchange1hdelta",
        "dMarket",
        FieldClass::Delta,
        Some("Delta_Market_Min"),
        Some("Delta_Market_Max"),
        None,
    ),
    field(
        "btc24hdelta",
        "d24BTC",
        FieldClass::Delta,
        Some("Delta_BTC_24_Min"),
        Some("Delta_BTC_24_Max"),
        None,
    ),
    field(
        "exchange24hdelta",
        "dM24",
        FieldClass::Delta,
        Some("Delta_Market_24_Min"),
        Some("Delta_Market_24_Max"),
        None,
    ),
    field(
        "btc5mdelta",
        "dBTC5m",
        FieldClass::Delta,
        Some("Delta_BTC_5m_Min"),
        Some("Delta_BTC_5m_Max"),
        None,
    ),
    field(
        "dbtc1m",
        "dBTC1m",
        FieldClass::Delta,
        Some("Delta_BTC_1m_Min"),
        Some("Delta_BTC_1m_Max"),
        None,
    ),
    // Delta2/Delta3 slots form a Delta SUBGROUP (at most two per strategy).
    field("d1h", "d1h", FieldClass::DeltaSlot, None, None, Some("1h")),
    field(
        "d15m",
        "d15m",
        FieldClass::DeltaSlot,
        None,
        None,
        Some("15m"),
    ),
    field("d5m", "d5m", FieldClass::DeltaSlot, None, None, Some("5m")),
    field("d1m", "d1m", FieldClass::DeltaSlot, None, None, Some("1m")),
    field(
        "pump1h",
        "Pump1H",
        FieldClass::DeltaSlot,
        None,
        None,
        Some("Pump1h"),
    ),
    field(
        "dump1h",
        "Dump1H",
        FieldClass::DeltaSlot,
        None,
        None,
        Some("Dump1h"),
    ),
];

/// `DeltaN_Type` value for a slot field (`None` means the field is not a slot).
pub fn slot_type_for(field: &str) -> Option<&'static str> {
    FIELDS
        .iter()
        .find(|s| s.col == field)
        .and_then(|s| s.slot_type)
}

/// Range for one field; `None` means the bound is unset.
#[derive(Clone, Debug, Default)]
pub struct Bound {
    pub field: String,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

/// WorkingTime: one strategy time range. `Day` is a time-of-day window (minutes 0..1439,
/// serialized as "hh:mm-hh:mm"); `Hour` is a minute window within EVERY HOUR (0..59,
/// serialized as "N-M"). Both are single continuous ranges (`from > to` wraps across the end
/// of the day or hour).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TimeWindow {
    Day(u16, u16),
    Hour(u8, u8),
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

/// Convert a week-minute span `(from, to)` (0..10079) into a `WorkingWeekTime` field string,
/// `day.hh:mm-day.hh:mm` (1-based day: 1=Mon..7=Sun). Shorten a bound on a day boundary to
/// just `day`: minute 0 for the start and minute 1439 for the end. Thus `1-5` means Mon 00:00
/// through Fri 23:59, while `1.23:44-6.22:22` includes explicit times. The caller does not
/// write a full week because it means no restriction.
pub fn format_week_span((f, t): (u16, u16)) -> String {
    let ends = |m: u16, at_end: bool| {
        let (day, tod) = ((m / 1440) % 7 + 1, m % 1440);
        // Omit the time for a start at minute 0 or an end at minute 1439.
        if (!at_end && tod == 0) || (at_end && tod == 1439) {
            day.to_string()
        } else {
            format!("{day}.{}", fmt_min(tod))
        }
    };
    format!("{}-{}", ends(f, false), ends(t, true))
}

/// Convert `TimeWindow` into a `WorkingTime` field string. `Day` becomes "hh:mm-hh:mm";
/// `Hour` becomes "N-M" (MoonBot identifies minutes within each hour by the absence of ":").
pub fn format_working_time(tw: TimeWindow) -> String {
    match tw {
        TimeWindow::Day(f, t) => format!("{}-{}", fmt_min(f), fmt_min(t)),
        TimeWindow::Hour(f, t) => format!("{f}-{t}"),
    }
}

/// Convert minutes of the day to "hh:mm".
fn fmt_min(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
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

/// Open a reader and build the unified tuner source for `q`, applying the tuner's all-history
/// floor (`from = 1`, the same floor `coin_groups`/`strategies_for_coins` use so the coin
/// table and the KPI matrix cover one period). `NotReady` when no source carries the required
/// schema. Returns the connection, the floored query, and the `FROM` source string.
///
/// The plain-reader preamble shared by every non-snapshot tuner scan; `variant_stats` pins a
/// WAL snapshot of its own instead and does not use this.
pub(super) fn open_tuner_source(q: &Query) -> ReadResult<(Connection, Query, String)> {
    let conn = super::open_reader()?;
    let mut q = q.clone();
    q.floor_all_history();
    let Some(src) = unified_from(&conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
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
    let mut out: Vec<(i64, i64, f64)> = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| read_fail(ctx, e))?);
    }
    Ok(out)
}

/// Compute KPI values in input order; an empty variant represents the baseline.
///
/// A healthy empty period produces one zero-valued KPI result per variant.
/// Returns `NotReady` when the replica or required schema is absent and `Failed`
/// when opening the replica, pinning the snapshot, or scanning a variant fails.
pub fn variant_stats(q: &Query, variants: &[Variant]) -> ReadResult<Vec<VarStats>> {
    let conn = super::open_reader()?;
    // One snapshot for the whole comparison. Each variant is its own scan, and
    // this table exists precisely to score variants AGAINST the baseline — a row
    // computed over a different set of trades than the baseline it is compared
    // to would still be published as one coherent comparison.
    let snap = super::read_snapshot(&conn)?;
    let mut q = q.clone();
    q.floor_all_history();
    let Some(src) = unified_from(&snap, &q)? else {
        return Err(ReadFail::NotReady);
    };
    variants
        .iter()
        .map(|v| one_variant(&snap, &src, &q, v))
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
    if !FIELDS.iter().any(|s| s.col == field) {
        // Programmer error (unknown field), not a read failure.
        return Ok(Vec::new());
    }
    let (conn, q, src) = open_tuner_source(q)?;
    let mut pairs = scan_field_pairs(&conn, &q, &src, field, "tuner: histogram")?;
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

/// Strategy parameters (min, max) for a report field; `(None, None)` means no mapping.
pub fn params_for(field: &str) -> (Option<&'static str>, Option<&'static str>) {
    FIELDS
        .iter()
        .find(|s| s.col == field)
        .map(|s| (s.p_min, s.p_max))
        .unwrap_or((None, None))
}

/// Open strategies.sqlite READ-ONLY, or `None` when it is absent or will not open.
///
/// A 3 s `busy_timeout` covers the strat_db writer committing on its own thread; without it a
/// write landing under our read is an instant SQLITE_BUSY that a caller would misread as "no
/// such strategy". Shared by every strategy read in this module.
fn open_strategies_ro() -> Option<Connection> {
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return None;
    }
    let conn =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    Some(conn)
}

/// The current version's `raw_json` for a strategy, scoped to `core` when known.
///
/// `None` — no live head row for this `(strategy_id, core)`. Rows are per-core
/// (`strategyid@core_uid`), so scoping to the exact core reflects the SAME core a write
/// targets, not the newest-checked one. Shared by `strategy_current_values_opt` and
/// `strategy_filters`.
fn load_head_raw_json(conn: &Connection, strategy_id: i64, core: Option<u64>) -> Option<String> {
    let core_clause = if core.is_some() {
        " AND s.core_uid = ?2"
    } else {
        ""
    };
    let sql = format!(
        "SELECT v.raw_json FROM strategies s
             JOIN strategy_versions v
               ON v.core_uid = s.core_uid AND v.strategy_id = s.strategy_id
             WHERE s.strategy_id = ?1{core_clause} AND s.deleted = 0 AND v.valid_to IS NULL
             ORDER BY s.checked DESC LIMIT 1"
    );
    match core {
        Some(c) => conn
            .query_row(&sql, rusqlite::params![strategy_id, c as i64], |r| r.get(0))
            .ok(),
        None => conn.query_row(&sql, [strategy_id], |r| r.get(0)).ok(),
    }
}

/// Cores where the strategy currently exists (strategies.sqlite heads with `deleted=0`),
/// which are the targets for saving thresholds.
pub fn strategy_cores(strategy_id: i64) -> Vec<u64> {
    let Some(conn) = open_strategies_ro() else {
        return Vec::new();
    };
    conn.prepare("SELECT core_uid FROM strategies WHERE strategy_id = ?1 AND deleted = 0")
        .ok()
        .and_then(|mut st| {
            st.query_map([strategy_id], |r| r.get::<_, i64>(0))
                .ok()
                .map(|rows| rows.flatten().map(|c| c as u64).collect())
        })
        .unwrap_or_default()
}

/// Strategy filter card: Ignore flags plus NON-default thresholds for tuner fields.
/// The flags drive both chips (hide ignored classes) and threshold persistence
/// (enable the required classes before writing).
#[derive(Clone, Debug, Default)]
pub struct StratFilters {
    pub found: bool,
    pub ignore_filters: bool,
    pub ignore_delta: bool,
    pub ignore_volume: bool,
    /// Whether the Filters/Base section (leverage, MarkPrice) is ignored.
    pub ignore_base: bool,
    /// BV/SV filter switch (`UseBV_SV_Filter`); `false` means disabled.
    pub use_bvsv: bool,
    /// Whether the Filters/Ping section (PriceBug and pings) is ignored.
    pub ignore_ping: bool,
    pub bounds: std::collections::HashMap<&'static str, (Option<f64>, Option<f64>)>,
    /// Occupied Delta2/Delta3 slots: (number 2|3, report field, min, max).
    /// Slots whose type has no report column (2h/30m/Pump5m) are omitted.
    pub slots: Vec<(u8, &'static str, Option<f64>, Option<f64>)>,
    /// Slots occupied by a type WITHOUT a report column (2h/30m/Pump5m) and configured
    /// thresholds: a live filter invisible to the tuner. Saving must overwrite such a slot
    /// only as a last resort and with a warning.
    pub foreign_slots: Vec<(u8, String)>,
    /// Current strategy comment (`Comment` field); saving appends an analyzer stamp without
    /// erasing the user's description.
    pub comment: String,
}

impl StratFilters {
    /// Whether the current strategy flags ignore the field class.
    pub fn class_ignored(&self, class: FieldClass) -> bool {
        self.ignore_filters
            || match class {
                FieldClass::Filter => false,
                // BV/SV is a Filters/Volume subgroup gated by BOTH IgnoreVolume and its own
                // UseBV_SV_Filter switch.
                FieldClass::BvSv => self.ignore_volume || !self.use_bvsv,
                FieldClass::Ping => self.ignore_ping,
                FieldClass::Base => self.ignore_base,
                FieldClass::Delta | FieldClass::DeltaSlot => self.ignore_delta,
                FieldClass::Volume => self.ignore_volume,
            }
    }

    /// Slot assigned to the field, if any: (number, min, max).
    pub fn slot_of(&self, field: &str) -> Option<(u8, Option<f64>, Option<f64>)> {
        self.slots
            .iter()
            .find(|(_, f, _, _)| *f == field)
            .map(|(n, _, lo, hi)| (*n, *lo, *hi))
    }
}

/// Current strategy parameter values for the "now -> next" write-confirmation dialog.
/// Keys are parameter names from the edit list; a key absent from raw_json is omitted from
/// the result. Values are strings in strategy format (booleans are YES/NO). This accesses
/// strategies.sqlite, so call it from a background executor.
pub fn strategy_current_values(
    strategy_id: i64,
    core: Option<u64>,
    keys: &[String],
) -> std::collections::HashMap<String, String> {
    strategy_current_values_opt(strategy_id, core, keys).unwrap_or_default()
}

/// The same read, but able to say "I could not look".
///
/// `None` — the strategy's row could not be read at all (no database file, the file would not
/// open, no live row for this `(strategy_id, core)`, unparseable `raw_json`). `Some(map)` — the
/// row WAS read, and a key missing from the map means the field is genuinely absent from the
/// strategy, i.e. empty.
///
/// The flattened form above collapses those two into an empty map, which is fine for filling
/// a "now → next" preview but not for anything that must not guess. A caller checking whether
/// an overwrite destroys data has to tell "this strategy lists no coins" from "I have no idea
/// what this strategy lists" — reporting the second as the first says a whole-field overwrite
/// was verified safe when nothing was verified at all.
pub fn strategy_current_values_opt(
    strategy_id: i64,
    core: Option<u64>,
    keys: &[String],
) -> Option<std::collections::HashMap<String, String>> {
    let mut out = std::collections::HashMap::new();
    let conn = open_strategies_ro()?;
    let raw = load_head_raw_json(&conn, strategy_id, core)?;
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&raw) else {
        return None;
    };
    for key in keys {
        let Some(v) = map.get(key) else { continue };
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => if *b { "YES" } else { "NO" }.to_string(),
            // A list-valued field (a coin list spelled as a JSON array) flattens to the
            // comma form the callers parse. Dropping it here while the SQL column reads it
            // is what makes one screen count coins the other cannot see.
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|i| match i {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(","),
            _ => continue,
        };
        out.insert(key.clone(), s);
    }
    Some(out)
}

/// Threshold parameters of the SELECTED strategy for tuner fields.
/// The source is the current strategies.sqlite version (raw_json normalized with schema
/// defaults). `defaults` contains schema defaults (lowercase name -> number): a value EQUAL
/// to its default is hidden because it means "filter not configured," not a deliberate
/// threshold (such as ...100T). `found=false` means the database or row was not found.
pub fn strategy_filters(
    strategy_id: i64,
    core: Option<u64>,
    defaults: &std::collections::HashMap<String, f64>,
) -> StratFilters {
    let mut out = StratFilters::default();
    // Rows are per-core: scope the strategy card (Ignore flags, thresholds) to the SELECTED
    // core so the save diff is computed against the exact core the write targets.
    let Some(conn) = open_strategies_ro() else {
        return out;
    };
    let Some(raw) = load_head_raw_json(&conn, strategy_id, core) else {
        return out;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return out;
    };
    let Some(map) = json.as_object() else {
        return out;
    };
    let num = |key: Option<&str>| -> Option<f64> {
        let key = key?;
        let v = map.get(key)?;
        let f = match v {
            serde_json::Value::Number(n) => n.as_f64()?,
            serde_json::Value::String(s) => s
                .trim()
                .trim_end_matches('%')
                .replace(',', ".")
                .parse()
                .ok()?,
            _ => return None,
        };
        if !f.is_finite() {
            return None;
        }
        // A schema-default value means the filter was never configured, so hide it.
        if let Some(d) = defaults.get(&key.to_ascii_lowercase()) {
            if (f - d).abs() <= f64::EPSILON.max(d.abs() * 1e-9) {
                return None;
            }
        }
        Some(f)
    };
    // Ignore flags may be booleans or YES/TRUE/1 strings.
    let truthy = |key: &str| -> bool {
        match map.get(key) {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => {
                matches!(s.trim().to_ascii_uppercase().as_str(), "YES" | "TRUE" | "1")
            }
            Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
            _ => false,
        }
    };
    out.found = true;
    out.ignore_filters = truthy("IgnoreFilters");
    out.ignore_delta = truthy("IgnoreDelta");
    out.ignore_volume = truthy("IgnoreVolume");
    out.ignore_base = truthy("IgnoreBase");
    out.comment = map
        .get("Comment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    out.use_bvsv = truthy("UseBV_SV_Filter");
    out.ignore_ping = truthy("IgnorePing");
    for spec in FIELDS {
        let (lo, hi) = (num(spec.p_min), num(spec.p_max));
        if lo.is_some() || hi.is_some() {
            out.bounds.insert(spec.col, (lo, hi));
        }
    }
    // Delta2/Delta3 slots: map a string type ("15m"/"Pump1h"/...) to a report field.
    for (n, prefix) in [(2u8, "Delta2"), (3u8, "Delta3")] {
        let Some(serde_json::Value::String(t)) = map.get(&format!("{prefix}_Type")) else {
            continue;
        };
        let t = t.trim();
        let lo = num(Some(&format!("{prefix}_Min")));
        let hi = num(Some(&format!("{prefix}_Max")));
        let Some(field) = FIELDS
            .iter()
            .find(|s| s.slot_type.is_some_and(|ty| ty.eq_ignore_ascii_case(t)))
            .map(|s| s.col)
        else {
            // 2h/30m/Pump5m have no report column. With configured thresholds this is a
            // live filter, so mark the slot as occupied by a foreign type.
            if lo.is_some() || hi.is_some() {
                out.foreign_slots.push((n, t.to_string()));
            }
            continue;
        };
        out.slots.push((n, field, lo, hi));
    }
    out
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

/// Automatic-suggestion result for the "By time" axis: two independent strategy fields.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimeSuggest {
    /// WorkingWeekTime: best continuous span over the week minute (0..10079).
    /// `None` means no improvement.
    pub week_span: Option<(u16, u16)>,
    /// WorkingTime: the best window in the requested format. `None` — no improvement.
    pub tod: Option<TimeWindow>,
}

/// What the "By time" sweep may search, straight from the row checkboxes: a CHECKED row is
/// searched and may be rewritten; an UNCHECKED one is never rewritten.
///
/// `day` and `hour` are two FORMATS of the single `WorkingTime` field, so they are two
/// competing candidates, not two fields: with both checked the sweep tries each and keeps
/// whichever earns more, which is why the result is at most TWO windows (a week span plus
/// one `WorkingTime` window) no matter how many boxes are ticked.
///
/// Hence pinning is per FIELD, not per row. `WorkingWeekTime` is its own field, so an
/// unchecked "Weekly" row holding a value pins the sweep inside it (`fixed_week`). For
/// `WorkingTime` a pin only exists when NEITHER format may be searched (`fixed_tod`): with
/// either box ticked the sweep owns that field and its answer REPLACES whatever is in the
/// pair — a value there is a candidate being replaced, not a constraint, since the two
/// formats cannot both be written to one field.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimeAxes {
    /// The "Weekly" row is checked → search `WorkingWeekTime`.
    pub week: bool,
    /// The "Day" row is checked → try `WorkingTime` as `hh:mm-hh:mm` (minute of the day).
    pub day: bool,
    /// The "In hour" row is checked → try `WorkingTime` as `N-M` (minute of the hour).
    pub hour: bool,
    /// Week span pinned by an UNCHECKED "Weekly" row (`None` — the row is empty).
    pub fixed_week: Option<(u16, u16)>,
    /// `WorkingTime` window pinned when NEITHER of its two rows is checked.
    pub fixed_tod: Option<TimeWindow>,
}

impl TimeAxes {
    /// Is there anything at all to search (otherwise the whole read is pointless).
    pub fn is_empty(&self) -> bool {
        !self.week && !self.day && !self.hour
    }
}

/// Profit sweep over the schedule fields: a continuous span of days (`WorkingWeekTime`)
/// and one time window (`WorkingTime`). What to search is dictated by `axes` (the row
/// checkboxes); with both `WorkingTime` formats checked the sweep tries each and keeps the
/// better one, and a row that is unchecked but already holds a value pins the sweep inside
/// it. The two fields are independent; each is `None` when the best candidate does not
/// improve on the unrestricted sample. Using `OPEN_TS`, the UTC weekday is
/// `(((OPEN_TS/86400)+4)%7+6)%7` (0=Mon..6=Sun), and the minute is
/// `(OPEN_TS%86400)/60`.
pub fn suggest_time(
    q: &Query,
    min_n: i64,
    edges: usize,
    round: bool,
    axes: TimeAxes,
) -> ReadResult<TimeSuggest> {
    // Nothing is checked → nothing to search; skip the scan instead of reading the whole
    // period only to throw it away.
    if axes.is_empty() {
        return Ok(TimeSuggest::default());
    }
    let (conn, q, src) = open_tuner_source(q)?;
    let trades = scan_time_rows(&conn, &q, &src, "tuner: suggest_time")?;
    Ok(time_suggest_from_rows(&trades, min_n, edges, round, axes))
}

/// Core of `suggest_time` over ready rows `(weekday, minute, profit)` — the entry point
/// for unit tests (no DB). Only the axes opened by `axes` are searched; rows cut off by
/// the pinned windows (`fixed_*`) are dropped BEFORE the sweep, so the free axis is
/// optimized inside them. The week and time axes are searched INDEPENDENTLY (each the
/// best window of its own projection) but applied with AND. Their intersection can yield
/// LESS profit than the baseline (each drops different winning trades), so we score every
/// combination on the real rows and take the maximum — the baseline is a candidate too,
/// which is why the result is NEVER worse than it.
///
/// NB: with a `fixed_*` window the baseline is the PINNED subset, not the whole sample —
/// the pin is a constraint the sweep may not lift, so "never worse" is measured against
/// what the user fixed. Without pins the baseline is the full sample, i.e. the "Fact"
/// column the UI shows.
fn time_suggest_from_rows(
    rows: &[(i64, i64, f64)],
    min_n: i64,
    edges: usize,
    round: bool,
    axes: TimeAxes,
) -> TimeSuggest {
    if axes.is_empty() {
        return TimeSuggest::default();
    }
    let min_n = min_n.max(1) as usize;
    // Is a trade inside the mask (bounds inclusive; `from > to` = wraps past the edge).
    let span_ok = |v: i64, f: i64, t: i64| {
        if f <= t {
            f <= v && v <= t
        } else {
            v <= t || v >= f
        }
    };
    let in_tod = |mn: i64, tw: TimeWindow| match tw {
        TimeWindow::Day(f, t) => span_ok(mn, f as i64, t as i64),
        TimeWindow::Hour(f, t) => span_ok(mn % 60, f as i64, t as i64),
    };
    // An unchecked row with a value pins the sweep: drop everything outside it, so both the
    // candidates and the baseline below are measured on the subset the user fixed. Nothing
    // pinned → work on the original slice (no copy).
    let pinned: Option<Vec<(i64, i64, f64)>> =
        (axes.fixed_week.is_some() || axes.fixed_tod.is_some()).then(|| {
            rows.iter()
                .copied()
                .filter(|&(wd, mn, _)| {
                    axes.fixed_week
                        .is_none_or(|(f, t)| span_ok(wd * 1440 + mn, f as i64, t as i64))
                        && axes.fixed_tod.is_none_or(|tw| in_tod(mn, tw))
                })
                .collect()
        });
    let rows: &[(i64, i64, f64)] = pinned.as_deref().unwrap_or(rows);
    // Candidates for the opened axes (best window of the projection; None = unrestricted).
    let week = axes
        .week
        .then(|| best_week_span(rows, min_n, edges, round))
        .flatten();
    // The two WorkingTime formats are separate CANDIDATES for one field: each checked row
    // contributes one, and the comparison below keeps whichever earns more. Unchecked → no
    // candidate, so that format can never come out of the sweep.
    let day_w = axes
        .day
        .then(|| {
            let mut vals: Vec<(f64, f64)> =
                rows.iter().map(|&(_w, mn, p)| (mn as f64, p)).collect();
            best_range(&mut vals, min_n, edges, round).map(|s| {
                TimeWindow::Day(
                    s.from.unwrap_or(0.0).clamp(0.0, 1439.0) as u16,
                    s.to.unwrap_or(1439.0).clamp(0.0, 1439.0) as u16,
                )
            })
        })
        .flatten();
    let hour_w = axes
        .hour
        .then(|| {
            let mut vals: Vec<(f64, f64)> = rows
                .iter()
                .map(|&(_w, mn, p)| ((mn % 60) as f64, p))
                .collect();
            best_range(&mut vals, min_n, edges, round).map(|s| {
                TimeWindow::Hour(
                    s.from.unwrap_or(0.0).clamp(0.0, 59.0) as u8,
                    s.to.unwrap_or(59.0).clamp(0.0, 59.0) as u8,
                )
            })
        })
        .flatten();
    let prof = |wk: Option<(u16, u16)>, td: Option<TimeWindow>| -> f64 {
        rows.iter()
            .filter(|&&(wd, mn, _)| {
                wk.is_none_or(|(f, t)| span_ok(wd * 1440 + mn, f as i64, t as i64))
                    && td.is_none_or(|tw| in_tod(mn, tw))
            })
            .map(|r| r.2)
            .sum()
    };
    // The baseline (no window of our own, but already inside any pin) is the starting
    // candidate; every other one has to BEAT it.
    let base: f64 = rows.iter().map(|r| r.2).sum();
    let margin = improvement_margin(base);
    let mut best: (Option<(u16, u16)>, Option<TimeWindow>, f64) = (None, None, base);
    for wk in [None, week] {
        for td in [None, day_w, hour_w] {
            if wk.is_none() && td.is_none() {
                continue;
            }
            let pr = prof(wk, td);
            if pr > best.2 + margin {
                best = (wk, td, pr);
            }
        }
    }
    TimeSuggest {
        week_span: best.0,
        tod: best.1,
    }
}

/// Best CONTINUOUS profit window over the week minute
/// (0..10079 = day*1440 + minute_of_day), found through quantile-based `best_range`.
/// Returns `None` when the best window does not improve on the full week. The linear search
/// (`from <= to`) does not suggest a window wrapping PAST Sunday, though the manual slider can.
fn best_week_span(
    rows: &[(i64, i64, f64)],
    min_n: usize,
    edges: usize,
    round: bool,
) -> Option<(u16, u16)> {
    let mut vals: Vec<(f64, f64)> = rows
        .iter()
        .filter(|(wd, ..)| (0..7).contains(wd))
        .map(|&(wd, mn, p)| ((wd * 1440 + mn) as f64, p))
        .collect();
    best_range(&mut vals, min_n, edges, round).map(|s| {
        (
            s.from.unwrap_or(0.0).clamp(0.0, 10079.0) as u16,
            s.to.unwrap_or(10079.0).clamp(0.0, 10079.0) as u16,
        )
    })
}

/// Average-profit profiles for coloring the three "By time" sliders:
/// `week[168]` (day x hour, 0=Mon 00:00), `day[1440]` (minute of day), and `hour[60]`
/// (minute within the hour). Each cell contains its average trade profit, or 0.0 if empty.
#[derive(Clone, Copy, Debug)]
pub struct SliderProfiles {
    pub week: [f32; 168],
    pub day: [f32; 1440],
    pub hour: [f32; 60],
}

/// Calculate `SliderProfiles` for the same scope as automatic suggestions (`tuner_query`).
pub fn slider_profiles(q: &Query) -> ReadResult<SliderProfiles> {
    let (conn, q, src) = open_tuner_source(q)?;
    let trades = scan_time_rows(&conn, &q, &src, "tuner: slider_profiles")?;
    Ok(slider_profiles_from_rows(&trades))
}

/// Core of `slider_profiles` over `(day, minute_of_day, profit)` rows, for tests.
fn slider_profiles_from_rows(rows: &[(i64, i64, f64)]) -> SliderProfiles {
    let (mut ws, mut wc) = ([0f64; 168], [0u32; 168]);
    let (mut ds, mut dc) = (vec![0f64; 1440], vec![0u32; 1440]);
    let (mut hs, mut hc) = ([0f64; 60], [0u32; 60]);
    for &(wd, mn, p) in rows {
        if !(0..7).contains(&wd) || !(0..1440).contains(&mn) {
            continue;
        }
        let (mi, h, moh) = (mn as usize, (mn / 60) as usize, (mn % 60) as usize);
        let wk = wd as usize * 24 + h;
        ws[wk] += p;
        wc[wk] += 1;
        ds[mi] += p; // Minute of day, 0..1439.
        dc[mi] += 1;
        hs[moh] += p;
        hc[moh] += 1;
    }
    let avg = |s: f64, c: u32| if c > 0 { (s / c as f64) as f32 } else { 0.0 };
    SliderProfiles {
        week: std::array::from_fn(|i| avg(ws[i], wc[i])),
        day: std::array::from_fn(|i| avg(ds[i], dc[i])),
        hour: std::array::from_fn(|i| avg(hs[i], hc[i])),
    }
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
