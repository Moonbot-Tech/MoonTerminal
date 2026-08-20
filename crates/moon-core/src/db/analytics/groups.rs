//! Strategy/coin group aggregates and top-trade lists.

#[cfg(test)]
mod tests;

use rusqlite::Connection;

use super::super::metrics::{profit_factor, winrate};
use super::super::read_fail::read_fail_on;
use super::super::{QuoteCurrency, QuoteScope, ReadFail, ReadResult};
use super::{raw_money_projection_on, scope_decision_on, unified_from, unified_from_mode, Query};

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
    /// Raw quote-currency profit, independent of the active Analytics profit lens.
    pub raw_profit: f64,
    /// Average positive order spend in quote currency over the lens-neutral trade scope.
    pub avg_order: f64,
    /// Homogeneity of the raw-money fields for this group.
    pub quote: QuoteScope,
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
    /// Return the group's winning-trade share as a percentage.
    pub fn winrate(&self) -> f64 {
        winrate(self.wins, self.n)
    }

    /// Return average active-lens profit per trade, or zero for an empty group.
    ///
    /// Returns:
    ///     Profit divided by trade count, or zero when the group is empty.
    pub fn avg(&self) -> f64 {
        if self.n > 0 {
            self.profit / self.n as f64
        } else {
            0.0
        }
    }

    /// Return raw total profit as a percentage of average positive order size.
    ///
    /// Mixed, unknown, empty, or non-positive order scopes return NaN so UI consumers cannot
    /// mistake unavailable raw money for a real zero.
    ///
    /// Returns:
    ///     Raw profit as a percentage of average order, or NaN when unavailable.
    pub fn profit_pct_of_avg_order(&self) -> f64 {
        if matches!(self.quote, QuoteScope::Single(_)) && self.avg_order > 0.0 {
            self.raw_profit / self.avg_order * 100.0
        } else {
            f64::NAN
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
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| r.get::<_, String>(0))
        .map_err(|e| read_fail_on(conn, CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| read_fail_on(conn, CTX, e))?);
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
    let projection = scope_decision_on(conn, &q)?
        .projection()
        .ok_or(ReadFail::IncomparableQuote)?;
    let Some(src) = unified_from_mode(conn, &q, projection)? else {
        return Err(ReadFail::NotReady);
    };
    coin_groups_from_source(conn, &q, &src)
}

/// Aggregate coins from a source already validated for the supplied query and snapshot.
///
/// Args:
///     conn: Pinned report snapshot.
///     q: Floored query used to build `src`.
///     src: Unified active-lens source whose quote scope was already validated.
///
/// Returns:
///     Per-coin aggregates or a classified raw-enrichment failure.
pub(in crate::db) fn coin_groups_from_source(
    conn: &Connection,
    q: &Query,
    src: &str,
) -> ReadResult<Vec<GroupStat>> {
    let raw_src = raw_source(conn, &q)?;
    groups(conn, &src, raw_src.as_deref(), &q, false, false)
}

/// Build the lens-neutral source used by raw-profit and average-order enrichments.
///
/// Quote mode needs no second source because the active aggregate can compute raw fields in the same
/// pass. Percent mode rebuilds without its positive-spend filter so the new columns do not change
/// when the display lens changes.
///
/// Args:
///     conn: Open analytics read connection.
///     q: Active filtered Analytics query.
/// Returns:
///     Optional lens-neutral source needed only for Percent mode, or a classified read failure.
pub(super) fn raw_source(conn: &Connection, q: &Query) -> ReadResult<Option<String>> {
    if q.metric == crate::db::ProfitMetric::Quote {
        return Ok(None);
    }
    let mut raw_query = q.clone();
    raw_query.metric = crate::db::ProfitMetric::Quote;
    let projection = raw_money_projection_on(conn, &raw_query)?;
    unified_from_mode(conn, &raw_query, projection)?
        .ok_or(ReadFail::NotReady)
        .map(Some)
}

/// Decode one aggregate row and its optional strategy/core metadata identity.
///
/// Args:
///     r: SQLite row matching the aggregate SELECT layout.
///
/// Returns:
///     Group aggregate paired with a numeric identity for later enrichment, or a SQLite
///     conversion error.
fn group_from_row(r: &rusqlite::Row) -> rusqlite::Result<(GroupStat, Option<(i64, i64)>)> {
    let wsum: f64 = r.get(7)?;
    let lsum: f64 = r.get(8)?;
    let quote = group_quote_scope(r.get(13)?, r.get(14)?, r.get(15)?, r.get(16)?);
    let comparable = matches!(quote, QuoteScope::Single(_));
    // `key` decodes as TEXT BEFORE `pair`, and on purpose: an unattributable bucket carries a
    // NULL key, so failing here is how such a bucket is rejected rather than enriched from a
    // fabricated `(MAX(strategyid), MAX(core_uid))`. The ordering that matters is key-then-pair,
    // not key-first — moving it after the quote fields is fine, moving it after `pair` is not.
    let key: String = r.get(0)?;
    // Both halves must be present before a group can be labelled. `MAX(o.strategyid)` is NULL
    // for the unattributable bucket, and the coin path projects both columns as NULL.
    let pair = r
        .get::<_, Option<i64>>(17)?
        .zip(r.get::<_, Option<i64>>(18)?);
    let group = GroupStat {
        key,
        // Every field below that the strategy database owns is filled by `enrich`; the bare id
        // (or the coin) stands in until then, and stays if no head names this pair.
        name: r.get(1)?,
        kind: String::new(),
        core: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        cores_n: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
        alive: None,
        n: r.get(4)?,
        profit: r.get(5)?,
        wins: r.get(6)?,
        pf: profit_factor(wsum, lsum),
        best: r.get(9)?,
        worst: r.get(10)?,
        lastedit: String::new(),
        bl: 0,
        wl: 0,
        raw_profit: if comparable { r.get(11)? } else { f64::NAN },
        avg_order: if comparable { r.get(12)? } else { f64::NAN },
        quote,
    };
    Ok((group, pair))
}

/// Classify one group's raw quote fields from SQLite aggregate metadata.
///
/// Args:
///     min: Minimum integral quote ordinal.
///     max: Maximum integral quote ordinal.
///     integral: Rows carrying an integral quote value.
///     rows: Complete rows in the group.
///
/// Returns:
///     Empty, one known quote, mixed known quotes, or unknown.
pub(super) fn group_quote_scope(
    min: Option<i64>,
    max: Option<i64>,
    integral: i64,
    rows: i64,
) -> QuoteScope {
    if rows == 0 {
        return QuoteScope::Empty;
    }
    if integral != rows {
        return QuoteScope::Unknown;
    }
    match (min, max) {
        (Some(min), Some(max)) if min == max => QuoteCurrency::from_report_ordinal(min)
            .map(QuoteScope::Single)
            .unwrap_or(QuoteScope::Unknown),
        (Some(min), Some(max))
            if QuoteCurrency::from_report_ordinal(min).is_some()
                && QuoteCurrency::from_report_ordinal(max).is_some() =>
        {
            QuoteScope::Mixed
        }
        _ => QuoteScope::Unknown,
    }
}

/// Distinct coins named by a raw `CoinsBlackList` / `CoinsWhiteList` field value.
pub(super) fn count_list(text: Option<String>) -> i64 {
    text.map_or(0, |t| crate::symbol::parse_coin_list(&t).len() as i64)
}

/// Group the period by strategy id or coin, sorted by descending profit.
///
/// Any query or aggregate-row failure aborts the complete grouping.
///
/// Args:
///     conn: Open analytics read connection.
///     src: Active-lens filtered source used by existing metrics.
///     raw_src: Optional raw-quote source; absent when active quote aggregation can reuse its pass.
///     q: Filter and period supplying SQL parameters.
///     has_names: Whether strategy metadata enrichment is available.
///     by_strategy: Whether to group by strategy identity instead of coin.
///
/// Returns:
///     Ordered group aggregates or a classified read failure.
pub(super) fn groups(
    conn: &Connection,
    src: &str,
    raw_src: Option<&str>,
    q: &Query,
    has_names: bool,
    by_strategy: bool,
) -> ReadResult<Vec<GroupStat>> {
    const CTX: &str = "analytics: groups";
    // Strategy key is `id@core_uid`, splitting PER CORE so each core's activity is visible.
    // Renames do not create groups, distinct strategies sharing a name do not merge, and the
    // name is only a label.
    //
    // Every metadata column — name, type, status, edit date and the two coin-list counts — is
    // filled AFTER this query, from `strategy_meta::read_metadata`, which is the same batched
    // loader the Summary stream labels its own group rows with. It used to be six scalar
    // subqueries correlated on `(strategy id, core uid)` and evaluated once per group, four of
    // them parsing a whole `raw_json` blob: measured at 550 ms of a 709 ms Strategies read over
    // 2 400 groups, against 41 ms for the batched form. Two statements per 400 pairs replace
    // six per group. Partial metadata failures retain the surfaces' existing fallback policies.
    //
    // The aggregate still carries the correlation pair out (`sid`/`cid`), and the invariant
    // behind reading it is worth stating because it lives in another file. `core_uid` is NOT
    // NULL in both report sources (`rep.rs`'s replica declares it so, and the legacy table
    // carries it), while `strategyid` can be absent — `unified_from` projects a missing column
    // as `NULL`. Concatenating a NULL yields a NULL key, so every unattributable row lands in
    // ONE bucket whose key fails to decode as text, which is how such a bucket is rejected
    // rather than enriched from a fabricated pair. Because `core_uid` cannot be NULL, every row
    // in that bucket has `strategyid` NULL and `MAX(o.strategyid)` stays NULL. Should a future
    // source allow NULL `core_uid`, the bucket could mix rows missing opposite halves of the
    // pair; `groups/tests.rs` pins the current behaviour.
    //
    // Neither form defines a winner for two rows with `valid_to IS NULL`: these queries have no
    // `ORDER BY`, and `read_version_metadata` retains the first row SQLite emits. Keeping that
    // unspecified choice avoids inventing a version-selection policy in Analytics.
    let key = if by_strategy {
        "CAST(o.strategyid AS TEXT) || '@' || CAST(o.core_uid AS TEXT)".to_string()
    } else {
        "COALESCE(o.coin,'')".to_string()
    };
    // The label a group carries when metadata cannot supply one: the bare strategy id, or the
    // coin, which IS its own label. Enrichment replaces the strategy case when a head names it.
    let name_fallback = if by_strategy { "a.sid_text" } else { "a.k" };
    // The correlation pair is carried out of the aggregate only for strategy groups. The coin
    // path reads none of these columns, so it avoids an unused per-row CAST and three aggregates.
    // `sid_text` is separate from `sid` because the name falls back to
    // `CAST(o.strategyid AS TEXT)`, and taking the MAX of the CAST rather than casting the MAX
    // keeps that fallback identical even where the column's storage class is not the INTEGER its
    // affinity suggests (`rep::apply_upsert` writes whatever the core sends).
    //
    // Both halves of the correlation pair are gated on `typeof(...) = 'integer'`, the same guard
    // `summary_stream` puts on its own numeric strategy id — and here it is load-bearing rather
    // than tidy. `rep::apply_upsert` writes whatever the core sends, so `strategyid` can carry a
    // TEXT storage class despite the column's INTEGER affinity (which is exactly why `sid_text`
    // exists beside it). The retired subqueries never decoded these values in Rust — SQLite
    // compared them under affinity — but the pair IS decoded now, and `Option<i64>` over a TEXT
    // value is an error that would fail the WHOLE strategy-base read. Ungated, one odd row from
    // one core would blank the entire Strategies tab. Gated, that group simply keeps the bare
    // text id it already carries, which is the degradation the old form produced anyway.
    let (pair, pair_out) = if by_strategy {
        (
            "MAX(CASE WHEN typeof(o.strategyid) = 'integer' THEN o.strategyid END) AS sid,
         MAX(CAST(o.strategyid AS TEXT)) AS sid_text,
         MAX(CASE WHEN typeof(o.core_uid) = 'integer' THEN o.core_uid END) AS cid,",
            "a.sid, a.cid",
        )
    } else {
        ("", "NULL, NULL")
    };
    // `ORDER BY a.profit DESC, a.k` — the tie-break is what makes the order TOTAL, and it has to
    // be stated: profit alone leaves equally profitable groups in whatever order the sorter
    // happens to emit, which changes with the query plan. That order is read as an answer — the
    // Summary takes the first and last entry as "best" and "worst" — so leaving it to the plan
    // means the same data can name a different best strategy from one build to the next. The key
    // is unique per group, so this decides every tie. Tuning applies its own total comparators
    // whenever the user selects a table sort.
    let (raw_cte, raw_join, raw_profit, avg_order, quote_stats) = if let Some(raw_src) = raw_src {
        (
            format!(
                ", raw AS (
                     SELECT {key} AS k,
                            COALESCE(SUM(o.profitbtc), 0) AS raw_profit,
                            COALESCE(AVG(CASE WHEN o.spentbtc > 0 THEN o.spentbtc END), 0) AS avg_order,
                            MIN(CASE WHEN typeof(o.basecurrency) = 'integer' THEN o.basecurrency END) AS quote_min,
                            MAX(CASE WHEN typeof(o.basecurrency) = 'integer' THEN o.basecurrency END) AS quote_max,
                            COUNT(CASE WHEN typeof(o.basecurrency) = 'integer' THEN 1 END) AS quote_integral,
                            COUNT(*) AS quote_rows
                     FROM {raw_src}
                     GROUP BY k
                 )"
            ),
            "LEFT JOIN raw ON raw.k IS a.k",
            "COALESCE(raw.raw_profit, 0)",
            "COALESCE(raw.avg_order, 0)",
            "raw.quote_min, raw.quote_max, raw.quote_integral, raw.quote_rows",
        )
    } else {
        (
            String::new(),
            "",
            "a.raw_profit",
            "a.avg_order",
            "a.quote_min, a.quote_max, a.quote_integral, a.quote_rows",
        )
    };
    let sql = format!(
        "WITH a AS (
             SELECT {key} AS k,
                    {pair}
                    MAX(o.core_name) AS core_name,
                    COUNT(DISTINCT o.core_uid) AS cores_n,
                    COUNT(*) AS n,
                    COALESCE(SUM(o.pnl),0) AS profit,
                    COALESCE(SUM(o.pnl > 0),0) AS wins,
                    COALESCE(SUM(CASE WHEN o.pnl > 0 THEN o.pnl END),0) AS wsum,
                    COALESCE(SUM(CASE WHEN o.pnl <= 0 THEN -o.pnl END),0) AS lsum,
                    COALESCE(MAX(o.pnl),0) AS best,
                    COALESCE(MIN(o.pnl),0) AS worst,
                    COALESCE(SUM(o.profitbtc), 0) AS raw_profit,
                    COALESCE(AVG(CASE WHEN o.spentbtc > 0 THEN o.spentbtc END), 0) AS avg_order,
                    MIN(CASE WHEN typeof(o.basecurrency) = 'integer' THEN o.basecurrency END) AS quote_min,
                    MAX(CASE WHEN typeof(o.basecurrency) = 'integer' THEN o.basecurrency END) AS quote_max,
                    COUNT(CASE WHEN typeof(o.basecurrency) = 'integer' THEN 1 END) AS quote_integral,
                    COUNT(*) AS quote_rows
             FROM {src}
             GROUP BY k
         ){raw_cte}
         SELECT a.k, {name_fallback}, a.core_name, a.cores_n,
                a.n, a.profit, a.wins, a.wsum, a.lsum, a.best, a.worst,
                {raw_profit}, {avg_order}, {quote_stats}, {pair_out}
         FROM a {raw_join}
         ORDER BY a.profit DESC, a.k"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], group_from_row)
        .map_err(|e| read_fail_on(conn, CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        // Each row carries the group's COUNT/SUM, so dropping one would
        // understate the table the user reads as complete.
        out.push(row.map_err(|e| read_fail_on(conn, CTX, e))?);
    }
    enrich(conn, &mut out, has_names);
    Ok(out.into_iter().map(|(group, _)| group).collect())
}

/// Label every strategy group from the batched strategy database, in place.
///
/// The order established by `ORDER BY a.profit DESC, a.k` is preserved exactly: this fills
/// fields only, and never reorders, adds or drops a group.
///
/// Enrichment is OPTIONAL and degrades IN PLACE, at two depths. An unreadable strategy
/// database leaves every group on the bare ids already in `name` — byte for byte what a scope
/// with no strategy database produces, which is why this cannot fail: the period itself is
/// perfectly readable and a label is not worth sinking it for. An unreadable strategy VERSION
/// keeps names and status and empties only the version-derived type, edit date and list counts.
///
/// That in-place degradation is what retired `with_name_fallback`, which used to answer the
/// same failure by re-running the ENTIRE period aggregate with enrichment switched off. Under
/// the per-group subqueries it had to: the labels were welded into the aggregate statement.
/// They are a separate pass now, so paying for a second full scan to discard a label would be
/// pure waste.
///
/// Args:
///     conn: Pinned report snapshot with the strategy database attached.
///     groups: Ordered aggregates paired with the correlation identity they were keyed on.
///     has_names: Whether the strategy database may be consulted at all.
fn enrich(conn: &Connection, groups: &mut [(GroupStat, Option<(i64, i64)>)], has_names: bool) {
    if !has_names {
        // No strategy database: the bare ids already in `name` are the honest label, and
        // `alive` stays absent rather than claiming "deleted" about a status nobody can read.
        return;
    }
    let pairs = groups
        .iter()
        .filter_map(|(_, pair)| *pair)
        .collect::<std::collections::HashSet<_>>();
    let metadata = match super::strategy_meta::read_metadata(conn, &pairs) {
        Ok(metadata) => metadata,
        Err(error) => {
            log::warn!("analytics: strategy names unavailable, groups keep bare ids: {error}");
            return;
        }
    };
    let by_pair = metadata.groups.as_ref().unwrap_or(&metadata.heads);
    for (group, pair) in groups.iter_mut() {
        let Some(pair) = pair else {
            // A coin group: its key is its own label and no strategy owns it.
            continue;
        };
        let details = by_pair.get(pair);
        // A present but nameless head, and a pair with no head at all, both keep the bare id
        // already sitting in `name` — never an empty label.
        if let Some(name) = details.and_then(|item| item.name.clone()) {
            group.name = name;
        }
        group.kind = details.map(|item| item.kind.clone()).unwrap_or_default();
        // An absent head reads as 0 ("deleted"), matching what the status lookup returned for a
        // pair the strategy database does not know.
        group.alive = Some(details.map_or(0, |item| item.alive));
        group.lastedit = details
            .map(|item| item.lastedit.clone())
            .unwrap_or_default();
        // The live-head gate lives on `StrategyMetadata` itself, so this caller and the
        // Summary stream cannot drift on it. The status lookup above is deliberately NOT gated
        // the same way — a deleted strategy still has a status to report.
        group.bl = details.map_or(0, super::strategy_meta::StrategyMetadata::blacklist_count);
        group.wl = details.map_or(0, super::strategy_meta::StrategyMetadata::whitelist_count);
    }
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
