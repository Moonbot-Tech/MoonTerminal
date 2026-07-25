//! Strategy/coin group aggregates and top-trade lists.

use rusqlite::Connection;

use super::super::metrics::{profit_factor, winrate};
use super::super::read_fail::read_fail;
use super::super::{ReadFail, ReadResult};
use super::{unified_from, Query};

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
        winrate(self.wins, self.n)
    }

    pub fn avg(&self) -> f64 {
        if self.n > 0 {
            self.profit / self.n as f64
        } else {
            0.0
        }
    }
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

/// Profit per strategy type (`SignalType`) with the per-core split behind each one, in ONE
/// pass: the bars and their popups can never disagree about a number.
///
/// Without the strategies DB the type expression is a constant empty string, so every trade
/// folds into one unnamed group instead of the chart vanishing.
pub(super) fn kind_stats(
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

/// Which STRATEGIES traded any of `coins`, as the very keys the strategy list rows carry
/// (`strategyid@core_uid`).
///
/// Answers "who is behind this coin?" for the coin table's selection, so the two panels agree
/// by construction: the key is built by the same expression [`groups`] builds it with, and a
/// row highlighted here is the row the user is looking at.
///
/// An empty `coins` selects nothing and is answered without touching the database.
pub fn strategies_for_coins(q: &Query, coins: &[String]) -> ReadResult<Vec<String>> {
    if coins.is_empty() {
        return Ok(Vec::new());
    }
    let conn = super::super::open_reader()?;
    super::super::with_read_snapshot(&conn, |snapshot| {
        strategies_for_coins_on(snapshot, q, coins)
    })
}

/// Find picked-coin strategy keys on an existing connection or compound-read snapshot.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should be queried.
///     q: Report scope.
///     coins: Exact report coin names selected by the user.
///
/// Returns:
///     Matching `strategyid@core_uid` keys or a classified read failure.
pub(in crate::db) fn strategies_for_coins_on(
    conn: &Connection,
    q: &Query,
    coins: &[String],
) -> ReadResult<Vec<String>> {
    const CTX: &str = "analytics: strategies_for_coins";
    if coins.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = q.clone();
    // The same floor the coin table and the KPI use — one screen, one period.
    q.floor_all_history();
    let Some(src) = unified_from(conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
    // Coin names come from the same replica column the caller read them out of, and still go
    // through the shared escaper rather than being interpolated here.
    let list = super::super::tuner::sql_str_list(coins);
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
    let conn = super::super::open_reader()?;
    super::super::with_read_snapshot(&conn, |snapshot| coin_groups_on(snapshot, q))
}

/// Aggregate coins on an existing connection or compound-read snapshot.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should be queried.
///     q: Report scope and period.
///
/// Returns:
///     Per-coin aggregates or a classified read failure.
pub(in crate::db) fn coin_groups_on(conn: &Connection, q: &Query) -> ReadResult<Vec<GroupStat>> {
    let mut q = q.clone();
    // The same floor `variant_stats` uses, NOT `min_closedate`: the coin table and the
    // "Fact vs v1" matrix sit on one screen and must cover the same span — resolving
    // "all history" differently here would let the two halves describe two periods.
    q.floor_all_history();
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
        pf: profit_factor(wsum, lsum),
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

/// Group the period by strategy id or coin, sorted by descending profit.
///
/// Any query or aggregate-row failure aborts the complete grouping.
pub(super) fn groups(
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

/// Return the five best or worst period trades, failing if any ranked row is unreadable.
pub(super) fn top_trades(
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
