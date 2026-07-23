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
//! only closed, non-deleted trades (`closedate > 0`, `deleted = 0`). Profit is
//! `profitbtc`, denominated in the pair quote currency (USDT here).

use rusqlite::Connection;

use super::read_fail::read_fail;
use super::{ProfitMetric, ReadFail, ReadResult, SideFilter};

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
    /// Attribute LIQUIDATION rows to a strategy by the name the core leaves in the row.
    ///
    /// A liquidation arrives with `strategyid = 0` and `channelname = 'LIQUIDATION'`, so it
    /// lands in "Manual (no strategy)" and the strategy that actually took the loss never
    /// sees it. The name IS there — `signaltype`/`comment` carry `MainShotS  ( MoonShot )` —
    /// and matching it against `strat.strategies` by `(core_uid, name)` recovers the owner.
    ///
    /// Off by default and switchable, because turning it on MOVES money between strategies
    /// retroactively: measured on the real database, 291 of 319 liquidations attach and
    /// −4582.89 USDT leaves "Manual". The 28 that do not attach (a deleted strategy, or no
    /// parseable name) stay there rather than being guessed at.
    ///
    /// Deliberately NOT mirrored in the Report window: it reads through its own queries, and
    /// the two panels are allowed to disagree here. Anyone "fixing" that divergence should
    /// know it was chosen.
    pub attribute_liq: bool,
    /// Which quantity every profit figure is measured in: absolute money (`Usdt`) or
    /// return on spent capital (`Percent`, the report's `Profit` column). Applied once in
    /// [`unified_from`]'s projected `pnl` column, so every reader below shares the choice.
    pub metric: ProfitMetric,
}

impl Query {
    /// Build the period-and-filter WHERE clause for ONE source, referencing only columns
    /// that source HAS (like the Report window's `build_where`; filtering on a missing
    /// column would fail the entire SELECT). Placeholders ?1/?2 are from/to; cores, side,
    /// and emulator are integer literals from configuration, so injection is impossible.
    fn where_sql(&self, cols: &std::collections::HashSet<String>, sid: &str) -> String {
        self.where_with(WHERE_PERIOD, cols, sid)
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
    fn where_with(
        &self,
        period: &str,
        cols: &std::collections::HashSet<String>,
        sid: &str,
    ) -> String {
        let has = |n: &str| cols.contains(n);
        let mut w = String::from(period);
        if has("deleted") {
            w.push_str(" AND COALESCE(deleted,0) = 0");
        }
        if !self.cores.is_empty() {
            let list = self
                .cores
                .iter()
                .map(|c| (*c as i64).to_string())
                .collect::<Vec<_>>()
                .join(",");
            w.push_str(&format!(" AND core_uid IN ({list})"));
        }
        if has("isshort") {
            match self.side {
                SideFilter::All => {}
                SideFilter::Long => w.push_str(" AND COALESCE(isshort,0) = 0"),
                SideFilter::Short => w.push_str(" AND COALESCE(isshort,0) = 1"),
            }
        }
        if has("emulator") {
            match self.emulator {
                None => {}
                Some(false) => w.push_str(" AND COALESCE(emulator,0) = 0"),
                Some(true) => w.push_str(" AND COALESCE(emulator,0) = 1"),
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
                            format!("(COALESCE({sid},0) = {want} AND core_uid = {})", *c as i64)
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
];

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
fn effective_sid_expr(alias: &str, cols: &std::collections::HashSet<String>, on: bool) -> String {
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
pub(super) fn unified_from(conn: &Connection, q: &Query) -> ReadResult<Option<String>> {
    let cols: Vec<&str> = UNIFIED_COLS
        .iter()
        .copied()
        .chain(super::tuner::FIELDS.iter().map(|s| s.col))
        .collect();
    // Is the strategy database attached? Probed rather than passed down: the callers that
    // attach it do so on this same connection, and the tests deliberately do not.
    let has_names = q.attribute_liq && strategies_attached(conn);
    // Percent mode measures each trade as profit ÷ spent, so a trade without a positive
    // `spentbtc` has no percent at all.
    let pct = q.metric == ProfitMetric::Percent;
    let mut branches = Vec::new();
    for src in super::read_sources_res(conn)? {
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
        let proj = cols
            .iter()
            .map(|c| {
                if *c == "strategyid" && src.cols.contains(*c) {
                    // The attributed value is published UNDER THE ORIGINAL NAME, so every
                    // query outside this source keeps reading `o.strategyid` and gets it.
                    format!("{sid} AS \"strategyid\"")
                } else if src.cols.contains(*c) {
                    format!("r.\"{c}\"")
                } else {
                    format!("NULL AS \"{c}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        // The active profit metric, projected ONCE here as `pnl` so every aggregation below and
        // the tuner sweep read one column. `Usdt` is the raw money; `Percent` is the report's
        // `Profit` column (profit ÷ spent × 100 = return on spent capital). The `spentbtc > 0`
        // filter appended below (percent mode) guarantees the ratio is finite and non-NULL, so
        // COUNT, SUM, wins, streaks and the top list all describe the SAME set of trades. Sign is
        // preserved (spent > 0), so win/loss, profit factor and best/worst stay correct on `pnl`.
        let pnl = if pct {
            "r.\"profitbtc\" / r.\"spentbtc\" * 100.0 AS \"pnl\""
        } else {
            "r.\"profitbtc\" AS \"pnl\""
        };
        let mut where_sql = q.where_sql(&src.cols, &sid);
        if pct {
            where_sql.push_str(" AND spentbtc > 0");
        }
        branches.push(format!(
            "SELECT {proj}, {pnl} FROM {} r WHERE {where_sql}",
            src.table
        ));
    }
    if branches.is_empty() {
        Ok(None)
    } else {
        Ok(Some(format!("({}) o", branches.join(" UNION ALL "))))
    }
}

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
        if self.n > 0 {
            self.wins as f64 / self.n as f64 * 100.0
        } else {
            0.0
        }
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

/// A calendar heatmap cell containing the aggregate for one UTC day or hour.
#[derive(Clone, Debug, Default)]
pub struct DayCell {
    /// Cell start in UTC Unix seconds.
    pub start: i64,
    pub profit: f64,
    pub trades: i64,
    /// Winning trades in the cell; losses equal `trades - wins`.
    pub wins: i64,
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

/// A top-trade row from the period's best or worst trades.
#[derive(Clone, Debug)]
pub struct TopTrade {
    pub closedate: i64,
    pub coin: String,
    pub strategy: String,
    pub core_name: String,
    pub profit: f64,
    pub is_short: bool,
}

/// Aggregate for a strategy-and-core or coin group.
///
/// `Default` is "a group with no trades": every field is already zero/empty/None, and callers
/// need it to show a coin that is PRESENT in the set but was not traded in the current scope.
#[derive(Clone, Debug, Default)]
pub struct GroupStat {
    /// Group key: `strategyid@core_uid` for strategies, or the coin name for coins.
    pub key: String,
    /// Display name, read from strategies.sqlite for strategies or falling back to the id.
    pub name: String,
    /// Strategy type (`SignalType` of the current version); empty for coins or without the DB.
    pub kind: String,
    /// One core name from the group and the number of distinct cores (`Core` column).
    pub core: String,
    pub cores_n: i64,
    /// Current status from the strategies.sqlite head for the strategy's core: `None`
    /// means no strategy DB or a coin group; 0 means deleted, 1 present but disabled,
    /// and 2 present and enabled.
    pub alive: Option<i64>,
    pub n: i64,
    pub profit: f64,
    pub wins: i64,
    /// Group win sum divided by its loss sum. Returns 99 when the win sum is positive
    /// and the loss sum is zero, or 0 when both sums are zero.
    pub pf: f64,
    pub best: f64,
    pub worst: f64,
    /// Strategy's last edit date — the `LastEditDate` field from the current version's
    /// raw_json (strategies.sqlite). Empty for coins / when the strategy DB is absent.
    pub lastedit: String,
    /// How many DISTINCT coins the strategy's `CoinsBlackList` / `CoinsWhiteList` name.
    ///
    /// Counted over normalized tokens, not raw entries: a list may repeat a coin in two
    /// spellings (`BTC`, `btc_rp`), and reporting the raw entry count would overstate what
    /// the list actually covers.
    ///
    /// Zero means BOTH "the list is empty" and "we cannot see it" (no strategy DB, deleted
    /// strategy) — callers that filter on it must not present the second as the first.
    pub bl: i64,
    pub wl: i64,
}

impl GroupStat {
    pub fn winrate(&self) -> f64 {
        if self.n > 0 {
            self.wins as f64 / self.n as f64 * 100.0
        } else {
            0.0
        }
    }

    pub fn avg(&self) -> f64 {
        if self.n > 0 {
            self.profit / self.n as f64
        } else {
            0.0
        }
    }
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

/// One core's share of a strategy type — the popup behind a `KindStat` bar.
#[derive(Clone, Debug)]
pub struct KindCore {
    pub uid: u64,
    pub name: String,
    pub profit: f64,
    pub trades: i64,
}

/// Profit of ONE strategy type (`SignalType`) over the period, with the per-core split
/// behind it. Built only for single-day periods, where a per-day series would be one bar:
/// the chart then groups by type instead of by day, and the popup opens the cores.
///
/// The type comes from the strategies DB; without it every trade lands in one unnamed
/// group (`kind` empty) rather than the chart disappearing.
#[derive(Clone, Debug)]
pub struct KindStat {
    /// `SignalType` of the strategy; empty when unknown (UI shows a dash).
    pub kind: String,
    pub profit: f64,
    pub trades: i64,
    /// Cores that traded this type, most profitable first.
    pub cores: Vec<KindCore>,
}

const WHERE_PERIOD: &str = "closedate >= ?1 AND closedate < ?2 AND closedate > 0";

/// Rows a period can never contain: the close date is absent or non-positive.
const WHERE_UNDATED: &str = "(closedate IS NULL OR closedate <= 0)";

/// Closed trades the core never dated, and the money they carry.
///
/// Every analytics figure is computed under [`WHERE_PERIOD`], which ends in `closedate > 0`.
/// So these rows are in NO period — not even "all history" — and their profit is simply
/// absent from every number this window shows. On a real replica that was 370 trades and
/// -434 USDT: closed for certain (they carry a sell price and a sell reason) and invisible
/// everywhere.
///
/// This is a CORE-side omission, not a replica bug: `db::rep::open_row_ids` already reports
/// such rows back for re-checking and they stay undated regardless. The terminal cannot
/// invent the missing timestamp — the only honest thing it can do is say how much is missing,
/// which is what this returns.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UndatedCloses {
    pub n: i64,
    pub profit: f64,
}

impl UndatedCloses {
    /// Nothing to report — the usual case, and the one the UI must stay silent about.
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
}

/// Count them under the CURRENT filters but outside any period — see [`UndatedCloses`].
pub fn undated_closes(q: &Query) -> ReadResult<UndatedCloses> {
    const CTX: &str = "analytics: trades with no close date";
    let conn = super::open_reader()?;
    let mut out = UndatedCloses::default();
    for src in super::read_sources_res(&conn)? {
        // A source without these columns cannot hold such a row, and asking would fail the
        // whole statement rather than contribute zero.
        if !src.cols.contains("closedate") || !src.cols.contains("profitbtc") {
            continue;
        }
        // Attribution deliberately OFF here: the banner counts rows the PERIOD threw away,
        // and which strategy they belong to changes nothing about that count. (The caller
        // passes no strategy scope either, so the expression is never consulted.)
        let sid = effective_sid_expr("r", &src.cols, false);
        let sql = format!(
            "SELECT COUNT(*), COALESCE(SUM(r.profitbtc), 0) FROM {} r WHERE {}",
            src.table,
            q.where_with(WHERE_UNDATED, &src.cols, &sid)
        );
        let (n, profit): (i64, f64) = conn
            .query_row(&sql, [], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| read_fail(CTX, e))?;
        out.n += n;
        out.profit += profit;
    }
    Ok(out)
}

/// Aggregate the selected period and filters without collapsing read failures.
///
/// Returns `NotReady` when the replica or required core schema is absent, and
/// `Failed` when a required current-period filesystem or SQLite read fails. A
/// failed comparison scan leaves `Summary::prev` absent. A successful empty
/// period has zero current-period counters.
pub fn summary(q: &Query) -> ReadResult<Summary> {
    let conn = super::open_reader()?;
    // `open_reader` has already attached it (a second ATTACH under the same alias fails).
    let has_strat_names = strategies_attached(&conn);
    // One snapshot for the whole summary. Its counters, series, top trades and
    // group tables are separate statements, and the writer commits between them
    // during catch-up — without this they could disagree about which trades the
    // period contains while being published as one coherent result.
    let snap = super::read_snapshot(&conn)?;
    summary_on(&snap, q, has_strat_names)
}

/// Aggregate a summary on an existing connection for tests and [`summary`].
///
/// `has_strat_names` is supplied so tests do not attach the developer's real
/// strategies database. Missing required source schema returns `NotReady`;
/// schema, query, and row failures return `Failed`.
pub(super) fn summary_on(
    conn: &Connection,
    q: &Query,
    has_strat_names: bool,
) -> ReadResult<Summary> {
    let mut q = q.clone();
    if q.from < 0 {
        q.from = min_closedate(conn)?;
    }
    let Some(src) = unified_from(conn, &q)? else {
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
    let prev = scan_period(conn, &src, q.from - len, q.from, bucket)
        .map(|(st, _, _)| st)
        .ok();

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
    Ok(Summary {
        cur,
        prev,
        bucket_secs: bucket,
        days,
        core_days,
        best: with_name_fallback(&mut names, |n| top_trades(conn, &src, &q, n, true))?,
        worst: with_name_fallback(&mut names, |n| top_trades(conn, &src, &q, n, false))?,
        strategies: with_name_fallback(&mut names, |n| groups(conn, &src, &q, n, true))?,
        coins: with_name_fallback(&mut names, |n| groups(conn, &src, &q, n, false))?,
        best_hour,
        // Only for a day or less — on a longer period the daily chart works and this
        // aggregation's per-row subquery would be paid for nothing.
        kinds: if one_day {
            with_name_fallback(&mut names, |n| kind_stats(conn, &src, &q, n))?
        } else {
            Vec::new()
        },
        cores: super::distinct_cores(conn)?,
        from: q.from,
        to: q.to,
    })
}

/// Profit per strategy type (`SignalType`) with the per-core split behind each one, in ONE
/// pass: the bars and their popups can never disagree about a number.
///
/// Without the strategies DB the type expression is a constant empty string, so every trade
/// folds into one unnamed group instead of the chart vanishing.
fn kind_stats(
    conn: &Connection,
    src: &str,
    q: &Query,
    has_names: bool,
) -> ReadResult<Vec<KindStat>> {
    const CTX: &str = "analytics: kind_stats";
    let kind = if has_names {
        "COALESCE((SELECT json_extract(v.raw_json, '$.SignalType')
                   FROM strat.strategy_versions v
                   WHERE v.core_uid = o.core_uid
                     AND v.strategy_id = o.strategyid
                     AND v.valid_to IS NULL), '')"
    } else {
        "''"
    };
    let sql = format!(
        "SELECT {kind} AS k, o.core_uid, MAX(COALESCE(o.core_name,'')),
                COUNT(*), COALESCE(SUM(o.pnl),0)
         FROM {src} GROUP BY k, o.core_uid"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                // NULL core_uid: a source without the column projects it as NULL, and a
                // hard read would abort the whole summary over one unattributable row.
                r.get::<_, Option<i64>>(1)?.unwrap_or(0) as u64,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, f64>(4)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut map: std::collections::HashMap<String, KindStat> = std::collections::HashMap::new();
    for row in rows {
        let (kind, uid, name, trades, profit) = row.map_err(|e| read_fail(CTX, e))?;
        let e = map.entry(kind.clone()).or_insert_with(|| KindStat {
            kind,
            profit: 0.0,
            trades: 0,
            cores: Vec::new(),
        });
        e.profit += profit;
        e.trades += trades;
        e.cores.push(KindCore {
            uid,
            name,
            profit,
            trades,
        });
    }
    let mut out: Vec<KindStat> = map.into_values().collect();
    for k in &mut out {
        k.cores.sort_by(|a, b| b.profit.total_cmp(&a.profit));
    }
    out.sort_by(|a, b| b.profit.total_cmp(&a.profit));
    Ok(out)
}

/// Dense daily cells for calendar heatmaps (GitHub-style Year or the large Month view).
/// One bucket is one UTC day; `GROUP BY closedate/86400` yields its PnL, trade count, and
/// wins. The range is filled COMPLETELY with empty cells for days without trades so the
/// calendar grid remains regular; unlike [`summary`], it NEVER widens buckets to a week.
/// `None` means a database, schema, or query read failed; `Some(empty)` means the period has
/// no closed trades or the source schema has not arrived yet.
///
/// NOTE: this surface still collapses a read failure into `None`, the pattern
/// the rest of this module moved away from. Converting it is left to the owners
/// of the calendar feature rather than rewritten here; the `.ok()?` calls below
/// only adapt it to the now-fallible helpers.
pub fn calendar_cells(q: &Query) -> Option<Vec<DayCell>> {
    calendar_cells_from(&super::open_reader().ok()?, q)
}

/// Core of [`calendar_cells`] over an existing connection and the entry point for unit tests,
/// which seed an in-memory `orders_rep` and verify bucketing, gaps, and win counts.
fn calendar_cells_from(conn: &Connection, q: &Query) -> Option<Vec<DayCell>> {
    let mut q = q.clone();
    let all_history = q.from < 0;
    if all_history {
        q.from = min_closedate(conn).ok()?;
    }
    let Some(src) = unified_from(conn, &q).ok()? else {
        // Source schemas have not arrived yet, so return an empty calendar (as `summary`
        // returns its default), NOT None; otherwise the tab would remain on Loading.
        return Some(Vec::new());
    };
    // Daily bucket: PnL, trade count, and wins for W/L and win rate.
    let sql = format!(
        "SELECT (o.closedate / 86400) * 86400 AS d,
                COALESCE(SUM(o.pnl), 0), COUNT(*), COALESCE(SUM(o.pnl > 0), 0)
         FROM {src} GROUP BY d ORDER BY d"
    );
    let mut stmt = conn.prepare(&sql).ok()?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .ok()?;
    let mut map: std::collections::HashMap<i64, DayCell> = std::collections::HashMap::new();
    let (mut first, mut last) = (i64::MAX, i64::MIN);
    for (d, profit, n, wins) in rows.flatten() {
        first = first.min(d);
        last = last.max(d);
        map.insert(
            d,
            DayCell {
                start: d,
                profit,
                trades: n,
                wins,
            },
        );
    }
    if map.is_empty() {
        return Some(Vec::new()); // A period without trades has an empty calendar.
    }
    // Dense-grid bounds start at the first day with data for All (avoiding years of empty
    // cells since the epoch), or at the requested period's start otherwise. Because `to` is
    // exclusive, the last day is day(to - 1); do not extend into the future.
    let now = crate::util::now_unix_ms_i64() / 1000;
    let today0 = now.div_euclid(86_400) * 86_400;
    let day0 = if all_history {
        first
    } else {
        q.from.div_euclid(86_400) * 86_400
    };
    let last_grid = ((q.to - 1).div_euclid(86_400) * 86_400)
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
        t += 86_400;
    }
    Some(out)
}

/// Hourly cells for the calendar's Day mode: `start` is the UTC hour start, with PnL,
/// trades, and wins. The result is sparse (only hours with trades); the UI builds the 24xN
/// grid. `None` means the DB or query is unavailable; `Some(empty)` means no trades or schema.
pub fn calendar_hours(q: &Query) -> Option<Vec<DayCell>> {
    let conn = super::open_reader().ok()?;
    let src = match unified_from(&conn, q) {
        Ok(Some(s)) => s,
        Ok(None) => return Some(Vec::new()), // The schema has not arrived yet.
        Err(_) => return None,
    };
    let sql = format!(
        "SELECT (o.closedate / 3600) * 3600 AS h,
                COALESCE(SUM(o.pnl), 0), COUNT(*), COALESCE(SUM(o.pnl > 0), 0)
         FROM {src} GROUP BY h ORDER BY h"
    );
    let mut stmt = conn.prepare(&sql).ok()?;
    let out = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok(DayCell {
                start: r.get(0)?,
                profit: r.get(1)?,
                trades: r.get(2)?,
                wins: r.get(3)?,
            })
        })
        .ok()?
        .flatten()
        .collect();
    Some(out)
}

/// Hour-of-day profile (0..23 UTC): PnL, trades, and wins aggregated across all days in
/// the period. This supplies a cell in the Tuning by-time lower heatmap.
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
    let conn = super::open_reader()?;
    let snap = super::read_snapshot(&conn)?;
    let conn = &*snap;
    let mut out = Vec::with_capacity(ranges.len());
    for &(from, to) in ranges {
        out.push(hour_profile_one(conn, base, from, to)?);
    }
    Ok(out)
}

/// One hour-of-day profile column for `[from, to)` on an existing snapshot.
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
    let mut prof = [HourStat::default(); 24];
    // Source schemas have not arrived yet, so return an empty profile like summary/calendar.
    let Some(src) = unified_from(conn, &q)? else {
        return Ok(prof);
    };
    // Hour of day comes from the trade OPEN time (`buydate`), matching the schedule and tuner
    // sliders that gate ENTRY. Fall back to `closedate` when the open time is absent (0/NULL).
    // The period itself still uses `closedate`, which defines the analysis window.
    let sql = format!(
        "SELECT ((COALESCE(NULLIF(o.buydate, 0), o.closedate) % 86400) / 3600) AS h,
                COALESCE(SUM(o.pnl), 0), COUNT(*), COALESCE(SUM(o.pnl > 0), 0)
         FROM {src} GROUP BY h ORDER BY h"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;
    for row in rows {
        let (h, profit, trades, wins) = row.map_err(|e| read_fail(CTX, e))?;
        if (0..24).contains(&h) {
            prof[h as usize] = HourStat {
                profit,
                trades,
                wins,
            };
        }
    }
    Ok(prof)
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
            .map_err(|e| read_fail(CTX, e))?;
        if let Some(v) = got {
            min = min.min(v);
        }
    }
    // No rows anywhere is a legitimate empty history, not a failure.
    Ok(if min == i64::MAX { 1 } else { min })
}

/// Which STRATEGIES traded any of `coins`, as the very keys the strategy list rows carry
/// (`strategyid@core_uid`).
///
/// Answers "who is behind this coin?" for the coin table's selection, so the two panels agree
/// by construction: the key is built by the same expression [`groups`] builds it with, and a
/// row highlighted here is the row the user is looking at.
///
/// An empty `coins` selects nothing and is answered without touching the database.
pub fn strategies_for_coins(q: &Query, coins: &[String]) -> ReadResult<Vec<String>> {
    const CTX: &str = "analytics: strategies_for_coins";
    if coins.is_empty() {
        return Ok(Vec::new());
    }
    let conn = super::open_reader()?;
    let snap = super::read_snapshot(&conn)?;
    let conn = &*snap;
    let mut q = q.clone();
    // The same floor the coin table and the KPI use — one screen, one period.
    if q.from < 0 {
        q.from = 1;
    }
    let Some(src) = unified_from(conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
    // Coin names come from the same replica column the caller read them out of, and still go
    // through the shared escaper rather than being interpolated here.
    let list = super::tuner::sql_str_list(coins);
    let sql = format!(
        "SELECT DISTINCT CAST(o.strategyid AS TEXT) || '@' || CAST(o.core_uid AS TEXT)
         FROM {src} WHERE COALESCE(o.coin,'') IN ({list})"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| r.get::<_, String>(0))
        .map_err(|e| read_fail(CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(out)
}

/// Per-coin aggregates for exactly the trades `q` selects.
///
/// The same grouping [`summary`] publishes as `Summary::coins`, but callable on
/// its own query — the coin panel needs it TWICE per refresh over two different
/// scopes (every coin the selected cores traded vs. the numbers of the selected
/// strategies), and re-running a whole summary for one `GROUP BY` would pay for
/// the series, the top trades and the strategy table as well.
///
/// Names never come from the strategies DB here — the group key IS the coin. It is attached
/// all the same (`open_reader` does it for every reader) because liquidation attribution
/// decides WHICH strategy's coins these are, and this must agree with the panel beside it.
/// `NotReady` when no source has the required schema.
pub fn coin_groups(q: &Query) -> ReadResult<Vec<GroupStat>> {
    let conn = super::open_reader()?;
    // One snapshot, like every other aggregation: the writer commits between
    // statements during catch-up.
    let snap = super::read_snapshot(&conn)?;
    let conn = &*snap;
    let mut q = q.clone();
    // The same floor `variant_stats` uses, NOT `min_closedate`: the coin table and the
    // "Fact vs v1" matrix sit on one screen and must cover the same span — resolving
    // "all history" differently here would let the two halves describe two periods.
    if q.from < 0 {
        q.from = 1;
    }
    let Some(src) = unified_from(conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
    groups(conn, &src, &q, false, false)
}

/// Decode one aggregate row ordered as key, labels, counts, profit, and extrema.
fn group_from_row(r: &rusqlite::Row) -> rusqlite::Result<GroupStat> {
    let wsum: f64 = r.get(9)?;
    let lsum: f64 = r.get(10)?;
    Ok(GroupStat {
        key: r.get(0)?,
        name: r.get(1)?,
        kind: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        core: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        cores_n: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
        alive: r.get(5)?,
        n: r.get(6)?,
        profit: r.get(7)?,
        wins: r.get(8)?,
        pf: if lsum > 0.0 {
            wsum / lsum
        } else if wsum > 0.0 {
            99.0
        } else {
            0.0
        },
        best: r.get(11)?,
        worst: r.get(12)?,
        lastedit: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
        // The lists arrive as their raw field text; counting distinct tokens is the same
        // rule the analytics coin table matches by, so the number here and the markers
        // there can never disagree about what a list covers.
        bl: count_list(r.get::<_, Option<String>>(14)?),
        wl: count_list(r.get::<_, Option<String>>(15)?),
    })
}

/// Distinct coins named by a raw `CoinsBlackList` / `CoinsWhiteList` field value.
fn count_list(text: Option<String>) -> i64 {
    text.map_or(0, |t| crate::symbol::parse_coin_list(&t).len() as i64)
}

/// Is the strategy database available on this connection?
///
/// Probed rather than tracked: `open_reader` attaches it, but a caller may hand us a
/// connection it opened itself, and a second ATTACH under the same alias fails. Asking the
/// connection is the only answer that cannot go stale.
pub(super) fn strategies_attached(conn: &Connection) -> bool {
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
pub(super) fn attach_strategies(conn: &Connection) -> bool {
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
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![from, to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;

    let (mut wsum, mut lsum) = (0.0f64, 0.0f64);
    let (mut cum, mut peak) = (0.0f64, 0.0f64);
    let (mut cur_w, mut cur_l) = (0i64, 0i64);
    let mut dur_ms_total = 0i64;
    for row in rows {
        // Every column here moves a number, so no row is skippable: dropping
        // one would silently understate n / profit / pf / max_dd.
        let (close, buy, profit) = row.map_err(|e| read_fail(CTX, e))?;
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
        st.pf = if lsum > 0.0 {
            wsum / lsum
        } else if wsum > 0.0 {
            99.0
        } else {
            0.0
        };
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
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
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
        .map_err(|e| read_fail(CTX, e))?;
    let mut map: std::collections::HashMap<u64, (String, Vec<f64>, Vec<i64>)> =
        std::collections::HashMap::new();
    for row in rows {
        // core_uid / closedate / profitbtc all move numbers here.
        let (uid, name, close, profit) = row.map_err(|e| read_fail(CTX, e))?;
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

/// Strategy name in SQL, from the attached strategy DB or falling back to the bare id.
fn strategy_name_expr(has_names: bool) -> &'static str {
    if has_names {
        "COALESCE((SELECT st.name FROM strat.strategies st
                   WHERE st.core_uid = o.core_uid AND st.strategy_id = o.strategyid),
                  CAST(o.strategyid AS TEXT))"
    } else {
        "CAST(o.strategyid AS TEXT)"
    }
}

/// Return the five best or worst period trades, failing if any ranked row is unreadable.
fn top_trades(
    conn: &Connection,
    src: &str,
    q: &Query,
    has_names: bool,
    best: bool,
) -> ReadResult<Vec<TopTrade>> {
    const CTX: &str = "analytics: top_trades";
    let order = if best { "DESC" } else { "ASC" };
    let name = strategy_name_expr(has_names);
    let sql = format!(
        "SELECT o.closedate, COALESCE(o.coin,''), {name}, o.core_name,
                COALESCE(o.pnl,0), COALESCE(o.isshort,0)
         FROM {src} WHERE o.pnl IS NOT NULL
         ORDER BY o.pnl {order} LIMIT 5"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok(TopTrade {
                closedate: r.get(0)?,
                coin: r.get(1)?,
                strategy: r.get(2)?,
                core_name: r.get(3)?,
                profit: r.get(4)?,
                is_short: r.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        // The ranked row carries closedate/profitbtc/isshort, so it is metric-
        // bearing end to end: skipping it would silently drop the extreme trade
        // the user opened this widget to see.
        out.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(out)
}

/// Group the period by strategy id or coin, sorted by descending profit.
///
/// Any query or aggregate-row failure aborts the complete grouping.
fn groups(
    conn: &Connection,
    src: &str,
    q: &Query,
    has_names: bool,
    by_strategy: bool,
) -> ReadResult<Vec<GroupStat>> {
    const CTX: &str = "analytics: groups";
    // Strategy key is `id@core_uid`, splitting PER CORE so each core's activity is visible.
    // Renames do not create groups, distinct strategies sharing a name do not merge, and the
    // name is only a label.
    // Raw text of a strategy field from its current version, or a NULL literal when the
    // strategies DB is not attached.
    let field = |name: &str| {
        if has_names {
            // CAST because `json_extract` returns the value's own type: a list stored as a
            // number or a bool would otherwise come back as INTEGER/REAL and fail the row
            // decode, taking the WHOLE grouping down over one odd field.
            //
            // `st.deleted = 0` mirrors `db::tuner::strategy_current_values`, which is what
            // the coin table reads the same lists through. Without it a deleted strategy
            // shows a count in this column that its own coin table cannot produce.
            format!(
                "MAX((SELECT CAST(json_extract(v.raw_json, '$.{name}') AS TEXT)
                      FROM strat.strategy_versions v
                      JOIN strat.strategies st
                        ON st.core_uid = v.core_uid AND st.strategy_id = v.strategy_id
                      WHERE v.core_uid = o.core_uid
                        AND v.strategy_id = o.strategyid
                        AND v.valid_to IS NULL
                        AND st.deleted = 0))"
            )
        } else {
            "NULL".to_string()
        }
    };
    let (blacklist, whitelist) = if by_strategy {
        (field("CoinsBlackList"), field("CoinsWhiteList"))
    } else {
        ("NULL".to_string(), "NULL".to_string())
    };
    let (key, name, kind, alive, lastedit) = if by_strategy {
        (
            "CAST(o.strategyid AS TEXT) || '@' || CAST(o.core_uid AS TEXT)".to_string(),
            format!("MAX({})", strategy_name_expr(has_names)),
            // Type (`SignalType`) from the strategy's current version; JSON1 is available
            // because rusqlite is bundled.
            if has_names {
                "MAX(COALESCE((SELECT json_extract(v.raw_json, '$.SignalType')
                               FROM strat.strategy_versions v
                               WHERE v.core_uid = o.core_uid
                                 AND v.strategy_id = o.strategyid
                                 AND v.valid_to IS NULL), ''))"
            } else {
                "''"
            },
            // Current status from the strategy DB heads: 2 means enabled, 1 present but
            // disabled, and 0 deleted; take the maximum within the group.
            if has_names {
                "MAX(COALESCE((SELECT CASE WHEN st.deleted <> 0 THEN 0
                                            WHEN COALESCE(st.checked,0) <> 0 THEN 2
                                            ELSE 1 END
                               FROM strat.strategies st
                               WHERE st.core_uid = o.core_uid
                                 AND st.strategy_id = o.strategyid), 0))"
            } else {
                "NULL"
            },
            // Last edit date: the `LastEditDate` field of the strategy's current version.
            if has_names {
                "MAX(COALESCE((SELECT json_extract(v.raw_json, '$.LastEditDate')
                               FROM strat.strategy_versions v
                               WHERE v.core_uid = o.core_uid
                                 AND v.strategy_id = o.strategyid
                                 AND v.valid_to IS NULL), ''))"
            } else {
                "''"
            },
        )
    } else {
        let coin = "COALESCE(o.coin,'')".to_string();
        (coin.clone(), coin, "''", "NULL", "''")
    };
    let sql = format!(
        "SELECT {key} AS k, {name}, {kind}, MAX(o.core_name), COUNT(DISTINCT o.core_uid),
                {alive},
                COUNT(*), COALESCE(SUM(o.pnl),0),
                COALESCE(SUM(o.pnl > 0),0),
                COALESCE(SUM(CASE WHEN o.pnl > 0 THEN o.pnl END),0),
                COALESCE(SUM(CASE WHEN o.pnl <= 0 THEN -o.pnl END),0),
                COALESCE(MAX(o.pnl),0), COALESCE(MIN(o.pnl),0),
                {lastedit}, {blacklist}, {whitelist}
         FROM {src}
         GROUP BY k ORDER BY 8 DESC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], group_from_row)
        .map_err(|e| read_fail(CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        // Each row carries the group's COUNT/SUM, so dropping one would
        // understate the table the user reads as complete.
        out.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(out)
}

/// What attributing a LIQUIDATION row to its named strategy does to the figures.
#[cfg(test)]
mod liq_attribution_tests;
/// The liquidation-attribution flag must reach the generated SQL from every call site.
#[cfg(test)]
mod liq_wiring_tests;
/// The strategy scope of a query — the tuner's Ctrl multi-select depends on it entirely.
#[cfg(test)]
mod scope_tests;

/// Regression tests for the read-failure contract: a damaged replica must
/// surface as an error, never as an empty period.
#[cfg(test)]
mod read_failure_tests;

#[cfg(test)]
mod tests;
