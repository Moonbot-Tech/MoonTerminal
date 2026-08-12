//! Differential coverage for the aggregate-level and outer enrichment SQL forms.
//!
//! Production evaluates the six strategy lookups over the aggregate, once per group. The
//! reference form keeps them under `MAX(...)` inside the `GROUP BY`, where they are evaluated
//! per trade. Equivalence depends on each subquery being constant within a group, so the fixture
//! seeds the cases where the two forms can disagree and compares every output field.
//!
//! The reference is a test-only oracle, not a second implementation: it is never called by
//! production code, and a deliberate contract change is expected to update it in the same commit
//! that makes the two disagree.

use super::*;
use crate::db::{ProfitMetric, SideFilter};

/// The report scope every case here uses: all cores, all history, no filters.
fn q() -> Query {
    Query {
        time_zone: chrono_tz::UTC,
        previous_period_basis: Default::default(),
        from: 0,
        to: i64::MAX,
        cores: Vec::new(),
        side: SideFilter::All,
        emulator: None,
        strategies: Vec::new(),
        metric: Default::default(),
        valuation: Default::default(),
        prefer_usdt: false,
    }
}

/// One trade row: `(core_uid, core_name, coin, strategyid, pnl)`.
type Trade = (i64, &'static str, &'static str, i64, f64);

/// Build the replica plus an attached strategy DB covering every case the two SQL forms
/// could disagree on.
///
/// Seeded deliberately:
/// - strategy 7 on cores 1 AND 2 — a group spanning several rows, and the same id split per core;
/// - strategy 8 — `deleted = 1`: still has a status, and its lists must NOT count (the
///   `st.deleted = 0` join);
/// - strategy 9 — present, `checked = 0`, `name` NULL, and NO version at all: the case where
///   sourcing `alive` from `strategy_versions` would silently report it as deleted;
/// - strategy 10 — absent from `strategies` but carrying a current version: name falls back to
///   the bare id while the type still resolves;
/// - strategy 11 — only a historical version (`valid_to` set), so the current-version lookups
///   must find nothing;
/// - strategy 0 — manual orders;
/// - strategies 12 and 13 — equal profit sums, so the ordering of a tie is compared too.
///
/// Returns:
///     Seeded report and strategy schemas covering enrichment edge cases.
fn fixture() -> Connection {
    let c = Connection::open_in_memory().expect("memory db");
    c.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, coin TEXT,
             isshort INTEGER, buydate INTEGER, closedate INTEGER, profitbtc REAL,
             strategyid INTEGER, emulator INTEGER, spentbtc REAL, basecurrency INTEGER);
         ATTACH ':memory:' AS strat;
         CREATE TABLE strat.strategies (core_uid INTEGER, strategy_id INTEGER,
             name TEXT, deleted INTEGER, checked INTEGER);
         CREATE TABLE strat.strategy_versions (core_uid INTEGER, strategy_id INTEGER,
             valid_from INTEGER, valid_to INTEGER, raw_json TEXT);",
    )
    .expect("schema");

    let trades: &[Trade] = &[
        (1, "alpha", "BTC", 7, 10.0),
        (1, "alpha", "ETH", 7, -4.0),
        (2, "beta", "BTC", 7, 3.0),
        (1, "alpha", "SOL", 8, 5.0),
        (1, "alpha", "BTC", 9, -1.0),
        (1, "alpha", "ETH", 10, 2.0),
        (1, "alpha", "SOL", 11, 7.0),
        (1, "alpha", "BTC", 0, 1.5),
        (1, "alpha", "ETH", 12, 6.0),
        (1, "alpha", "SOL", 13, 6.0),
    ];
    for (i, (core, core_name, coin, sid, pnl)) in trades.iter().enumerate() {
        let close = 1_000 + i as i64;
        c.execute(
            "INSERT INTO orders_rep (core_uid, core_name, coin, isshort, buydate, closedate,
                 profitbtc, strategyid, emulator, spentbtc, basecurrency)
             VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, 0, 100.0, 1)",
            rusqlite::params![core, core_name, coin, close - 60, close, pnl, sid],
        )
        .expect("insert trade");
    }

    // (core, id, name, deleted, checked)
    let heads: &[(i64, i64, Option<&str>, i64, i64)] = &[
        (1, 7, Some("Seven"), 0, 1),
        (2, 7, Some("Seven on beta"), 0, 1),
        (1, 8, Some("Eight"), 1, 1),
        (1, 9, None, 0, 0),
        (1, 11, Some("Eleven"), 0, 1),
        (1, 12, Some("Twelve"), 0, 1),
        (1, 13, Some("Thirteen"), 0, 1),
    ];
    for (core, sid, name, deleted, checked) in heads {
        c.execute(
            "INSERT INTO strat.strategies (core_uid, strategy_id, name, deleted, checked)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![core, sid, name, deleted, checked],
        )
        .expect("insert head");
    }

    // (core, id, valid_from, valid_to, raw_json)
    let versions: &[(i64, i64, i64, Option<i64>, &str)] = &[
        (
            1,
            7,
            1,
            None,
            r#"{"SignalType":"Pump","LastEditDate":"2026-01-02","CoinsBlackList":"BTC,btc_rp,ETH","CoinsWhiteList":"SOL"}"#,
        ),
        (
            2,
            7,
            1,
            None,
            r#"{"SignalType":"Dump","LastEditDate":"2026-01-03","CoinsBlackList":"XRP"}"#,
        ),
        (
            1,
            8,
            1,
            None,
            r#"{"SignalType":"Deleted","LastEditDate":"2026-01-04","CoinsBlackList":"AAA,BBB"}"#,
        ),
        (
            1,
            10,
            1,
            None,
            r#"{"SignalType":"Orphan","LastEditDate":"2026-01-05","CoinsBlackList":"CCC"}"#,
        ),
        // Historical only: `valid_to` is set, so no current-version lookup may match.
        (
            1,
            11,
            1,
            Some(9),
            r#"{"SignalType":"Old","LastEditDate":"2025-12-31","CoinsBlackList":"DDD"}"#,
        ),
    ];
    for (core, sid, from, to, json) in versions {
        c.execute(
            "INSERT INTO strat.strategy_versions (core_uid, strategy_id, valid_from, valid_to,
                 raw_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![core, sid, from, to, json],
        )
        .expect("insert version");
    }
    c
}

/// Build the historical per-trade strategy-name expression used by the SQL reference below.
fn strategy_name_expr(has_names: bool) -> String {
    name_expr(
        has_names,
        "o.core_uid",
        "o.strategyid",
        "CAST(o.strategyid AS TEXT)",
    )
}

/// Reference statement with every enrichment lookup wrapped in `MAX(...)` inside the aggregate.
///
/// Both forms include the `, 1` tie-break so this comparison isolates enrichment placement rather
/// than query-plan-dependent ordering. [`equally_profitable_groups_are_ordered_by_key`] pins the
/// ordering contract separately.
///
/// Args:
///     src: Unified report source SQL.
///     has_names: Whether strategy-name enrichment is enabled.
///     by_strategy: Whether rows are grouped by strategy rather than coin.
///
/// Returns:
///     Independent reference aggregation statement.
fn reference_sql(src: &str, has_names: bool, by_strategy: bool) -> String {
    let field = |name: &str| {
        if has_names {
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
            if has_names {
                "MAX(COALESCE((SELECT json_extract(v.raw_json, '$.SignalType')
                               FROM strat.strategy_versions v
                               WHERE v.core_uid = o.core_uid
                                 AND v.strategy_id = o.strategyid
                                 AND v.valid_to IS NULL), ''))"
            } else {
                "''"
            },
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
    format!(
        "SELECT {key} AS k, {name}, {kind}, MAX(o.core_name), COUNT(DISTINCT o.core_uid),
                {alive},
                COUNT(*), COALESCE(SUM(o.pnl),0),
                COALESCE(SUM(o.pnl > 0),0),
                COALESCE(SUM(CASE WHEN o.pnl > 0 THEN o.pnl END),0),
                COALESCE(SUM(CASE WHEN o.pnl <= 0 THEN -o.pnl END),0),
                COALESCE(MAX(o.pnl),0), COALESCE(MIN(o.pnl),0),
                {lastedit}, {blacklist}, {whitelist},
                COALESCE(SUM(o.profitbtc), 0),
                COALESCE(AVG(CASE WHEN o.spentbtc > 0 THEN o.spentbtc END), 0),
                MIN(CASE WHEN typeof(o.basecurrency) = 'integer' THEN o.basecurrency END),
                MAX(CASE WHEN typeof(o.basecurrency) = 'integer' THEN o.basecurrency END),
                COUNT(CASE WHEN typeof(o.basecurrency) = 'integer' THEN 1 END),
                COUNT(*)
         FROM {src}
         GROUP BY k ORDER BY 8 DESC, 1"
    )
}

/// Run an arbitrary group statement through the SAME decoder production uses.
fn run(conn: &Connection, sql: &str, q: &Query) -> Vec<GroupStat> {
    try_run(conn, sql, q).unwrap_or_else(|e| panic!("generated SQL must be valid: {e}\n{sql}"))
}

/// Compare two group lists BY INDEX, every field of every row.
///
/// `Debug` rather than sixteen hand-written assertions: a field added to `GroupStat` joins the
/// comparison by itself, where an enumerated list would silently stop covering the struct the
/// whole file exists to compare. It also distinguishes `-0.0` from `0.0`, which `==` on `f64`
/// does not.
///
/// Order is part of the contract because Summary reads the first and last entries as
/// "best/worst".
fn assert_same(label: &str, old: &[GroupStat], new: &[GroupStat]) {
    assert_eq!(old.len(), new.len(), "{label}: group count");
    for (i, (o, n)) in old.iter().zip(new).enumerate() {
        assert_eq!(
            format!("{o:?}"),
            format!("{n:?}"),
            "{label}[{i}]: group {} differs",
            o.key
        );
    }
}

/// Routing Coin Tuner back through `groups.rs:coin_groups_on` would repeat quote preflight and
/// reject this deliberately mixed fixture before the already-validated source can publish its
/// split-aware coin rows.
#[test]
fn source_consuming_coin_groups_do_not_repeat_scope_preflight() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE coin_rows(
            closedate INTEGER, pnl REAL, profitbtc REAL, spentbtc REAL,
            basecurrency INTEGER, coin TEXT, core_uid INTEGER, core_name TEXT
         );
         INSERT INTO coin_rows VALUES (10, 2.0, 2.0, 20.0, 1, 'BTC', 1, 'alpha');
         INSERT INTO coin_rows VALUES (20, 3.0, 3.0, 30.0, 8, 'BTC', 1, 'alpha');",
    )
    .expect("mixed coin fixture");
    let query = Query {
        from: 1,
        to: 30,
        ..Default::default()
    };
    let source = "(SELECT * FROM coin_rows WHERE closedate >= ?1 AND closedate < ?2) o";

    let groups = coin_groups_from_source(&conn, &query, source)
        .expect("source was validated by the compound caller");
    assert_eq!(groups.len(), 1);
    assert_eq!((groups[0].key.as_str(), groups[0].n), ("BTC", 2));
    assert!(matches!(groups[0].quote, QuoteScope::Mixed));
}

/// Outer enrichment must match the aggregate-level reference in all four `groups` query shapes.
///
/// Correlating an outer subquery on columns other than the pair used by the group key makes the
/// named field differ and can silently enrich a group from another strategy.
#[test]
fn lifting_enrichment_out_of_the_aggregate_preserves_every_group() {
    let c = fixture();
    let q = q();
    let src = unified_from(&c, &q)
        .expect("source")
        .expect("the replica exists");
    for by_strategy in [true, false] {
        for has_names in [true, false] {
            let label = format!("by_strategy={by_strategy} has_names={has_names}");
            let old = run(&c, &reference_sql(&src, has_names, by_strategy), &q);
            let new = groups(&c, &src, None, &q, has_names, by_strategy).expect("new groups");
            assert!(!old.is_empty(), "{label}: the fixture must produce groups");
            assert_same(&label, &old, &new);
        }
    }
}

/// Run a group statement without unwrapping, for the paths that are expected to fail.
fn try_run(conn: &Connection, sql: &str, q: &Query) -> rusqlite::Result<Vec<GroupStat>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params![q.from, q.to], group_from_row)?;
    rows.collect()
}

/// Both SQL forms reject an unattributable trade whose group key is NULL.
///
/// `unified_from` projects a missing `strategyid` as NULL, so such rows all collapse into ONE
/// group whose key is NULL. The outer form correlates through `MAX(o.strategyid)` and
/// `MAX(o.core_uid)`; `core_uid` is NOT NULL in both report sources, so every row in this bucket
/// has a NULL `strategyid` and its `MAX` remains NULL even when the rows span cores.
///
/// Coalescing only the production key to an empty string makes its decoder succeed while the
/// reference still fails, turning a rejected unattributable trade into a visible empty group.
#[test]
fn an_unattributable_trade_fails_the_same_way_in_both_forms() {
    let c = fixture();
    c.execute(
        "INSERT INTO orders_rep (core_uid, core_name, coin, isshort, buydate, closedate,
             profitbtc, strategyid, emulator, spentbtc, basecurrency)
         VALUES (1, 'alpha', 'BTC', 0, 900, 960, 2.0, NULL, 0, 100.0, 1)",
        [],
    )
    .expect("insert unattributed trade");

    let q = q();
    let src = unified_from(&c, &q).expect("source").expect("replica");
    let old = try_run(&c, &reference_sql(&src, true, true), &q);
    let new = groups(&c, &src, None, &q, true, true);
    assert!(
        old.is_err(),
        "the pre-change form decoded a NULL key as an error"
    );
    assert!(
        new.is_err(),
        "the lifted form must fail on it too, not enrich the bucket from a fabricated pair"
    );
}

/// Compare both enrichment shapes on a realistically sized replica.
///
/// `#[ignore]` on purpose: it builds a few hundred thousand rows, so it is a measurement to run
/// deliberately (`cargo test -p moon-core -- --ignored --nocapture groups_enrichment_cost`), not
/// a timing assertion in CI — a wall-clock threshold on a shared runner is a flaky test, and a
/// flaky test is one people learn to ignore. It prints both timings and asserts only that the
/// two forms still agree on the data, which is the part that must never drift.
/// Correlating any outer lookup on the wrong group column makes that equality assertion fail;
/// timing output remains informational.
#[test]
#[ignore = "measurement: builds a large synthetic replica"]
fn groups_enrichment_cost_against_a_large_replica() {
    const TRADES: i64 = 300_000;
    const STRATEGIES: i64 = 1_200;
    const CORES: i64 = 8;

    let c = Connection::open_in_memory().expect("memory db");
    c.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, coin TEXT,
             isshort INTEGER, buydate INTEGER, closedate INTEGER, profitbtc REAL,
             strategyid INTEGER, emulator INTEGER, spentbtc REAL, basecurrency INTEGER);
         ATTACH ':memory:' AS strat;
         CREATE TABLE strat.strategies (core_uid INTEGER, strategy_id INTEGER,
             name TEXT, deleted INTEGER, checked INTEGER);
         CREATE TABLE strat.strategy_versions (core_uid INTEGER, strategy_id INTEGER,
             valid_from INTEGER, valid_to INTEGER, raw_json TEXT);
         CREATE INDEX strat.ix_heads ON strategies(core_uid, strategy_id);
         CREATE INDEX strat.ix_vers ON strategy_versions(core_uid, strategy_id, valid_to);",
    )
    .expect("schema");

    {
        let tx = c.unchecked_transaction().expect("tx");
        for i in 0..TRADES {
            let sid = i % STRATEGIES;
            let core = i % CORES;
            tx.execute(
                "INSERT INTO orders_rep (core_uid, core_name, coin, isshort, buydate, closedate,
                     profitbtc, strategyid, emulator, spentbtc, basecurrency)
                 VALUES (?1, 'core', 'BTC', 0, ?2, ?3, ?4, ?5, 0, 100.0, 1)",
                rusqlite::params![core, i, i + 60, (i % 17) as f64 - 8.0, sid],
            )
            .expect("trade");
        }
        // A realistic head: every strategy has a row and a current version whose raw_json is
        // big enough that parsing it per TRADE rather than per GROUP is the whole difference.
        let list: String = (0..60).map(|n| format!("COIN{n},")).collect();
        for sid in 0..STRATEGIES {
            for core in 0..CORES {
                tx.execute(
                    "INSERT INTO strat.strategies (core_uid, strategy_id, name, deleted, checked)
                     VALUES (?1, ?2, ?3, 0, 1)",
                    rusqlite::params![core, sid, format!("Strategy {sid}")],
                )
                .expect("head");
                tx.execute(
                    "INSERT INTO strat.strategy_versions (core_uid, strategy_id, valid_from,
                         valid_to, raw_json) VALUES (?1, ?2, 1, NULL, ?3)",
                    rusqlite::params![
                        core,
                        sid,
                        format!(
                            r#"{{"SignalType":"Pump","LastEditDate":"2026-01-02","CoinsBlackList":"{list}","CoinsWhiteList":"{list}"}}"#
                        )
                    ],
                )
                .expect("version");
            }
        }
        tx.commit().expect("commit");
    }

    let q = q();
    let src = unified_from(&c, &q).expect("source").expect("replica");

    let started = std::time::Instant::now();
    let old = run(&c, &reference_sql(&src, true, true), &q);
    let old_took = started.elapsed();

    let started = std::time::Instant::now();
    let new = groups(&c, &src, None, &q, true, true).expect("groups");
    let new_took = started.elapsed();

    println!(
        "[groups] {TRADES} trades / {} groups: enrichment inside the aggregate {old_took:?}, \
         lifted out {new_took:?}",
        new.len()
    );
    assert_same("large replica", &old, &new);
}

/// Equally profitable groups are ordered by their key.
///
/// The fixture holds three groups at exactly 6.0. Dropping the `, 1` from the statement's
/// `ORDER BY` leaves them in whatever order the sorter emits, and that order is READ: the
/// Summary presents the first entry as the period's best strategy and the last as its worst.
/// The result is a "best strategy" that can change without the data changing.
#[test]
fn equally_profitable_groups_are_ordered_by_key() {
    let c = fixture();
    let q = q();
    let src = unified_from(&c, &q).expect("source").expect("replica");
    let out = groups(&c, &src, None, &q, true, true).expect("groups");
    let tied: Vec<&str> = out
        .iter()
        .filter(|g| g.profit == 6.0)
        .map(|g| g.key.as_str())
        .collect();
    assert_eq!(
        tied,
        ["12@1", "13@1", "7@1"],
        "a three-way tie must be broken by the group key, ascending"
    );
}

/// `alive` must survive a strategy that has no current version.
///
/// Strategy 9 is present in `strategies` with `checked = 0` and owns no version row at all.
/// Joining the status lookup to `strategy_versions` reports it as deleted, which the Tuning list
/// draws as a hollow dot and its "active only" filter then hides.
#[test]
fn a_strategy_without_a_current_version_keeps_its_status() {
    let c = fixture();
    let q = q();
    let src = unified_from(&c, &q).expect("source").expect("replica");
    let out = groups(&c, &src, None, &q, true, true).expect("groups");
    let nine = out
        .iter()
        .find(|g| g.key == "9@1")
        .expect("strategy 9 traded in the period");
    assert_eq!(nine.alive, Some(1), "present but disabled, not deleted");
    assert_eq!(nine.kind, "", "no current version means no type");
    assert_eq!(nine.lastedit, "", "no current version means no edit date");
    // `name` is NULL in the head, so it falls back to the bare id — not to an empty label.
    assert_eq!(nine.name, "9");
}

/// A deleted strategy reports its status but not its coin lists.
///
/// The list lookups join `strategies` with `st.deleted = 0` while the status lookup does not,
/// because the coin table cannot show a deleted strategy's list. Removing that predicate reports
/// list counts that the table cannot reproduce, while incorrectly applying it to the status
/// lookup loses the deleted status asserted here.
#[test]
fn a_deleted_strategy_reports_status_but_no_lists() {
    let c = fixture();
    let q = q();
    let src = unified_from(&c, &q).expect("source").expect("replica");
    let out = groups(&c, &src, None, &q, true, true).expect("groups");
    let eight = out.iter().find(|g| g.key == "8@1").expect("strategy 8");
    assert_eq!(eight.alive, Some(0), "deleted");
    assert_eq!(
        (eight.bl, eight.wl),
        (0, 0),
        "lists hidden for a deleted head"
    );
    // The type still comes from the version: that lookup carries no `deleted` filter.
    assert_eq!(eight.kind, "Deleted");
}

/// A strategy the strategy DB never heard of still groups, under its bare id.
///
/// Strategy 10 has a current version but no head row: the name falls back to the id, the status
/// defaults to 0, and the list lookups — which must find a live head — count nothing. Requiring a
/// head row for every version loses its type or removes the group instead of preserving this
/// fallback.
#[test]
fn a_strategy_missing_from_the_head_table_falls_back_to_its_id() {
    let c = fixture();
    let q = q();
    let src = unified_from(&c, &q).expect("source").expect("replica");
    let out = groups(&c, &src, None, &q, true, true).expect("groups");
    let ten = out.iter().find(|g| g.key == "10@1").expect("strategy 10");
    assert_eq!(ten.name, "10");
    assert_eq!(ten.alive, Some(0));
    assert_eq!(ten.kind, "Orphan", "the version is readable without a head");
    assert_eq!((ten.bl, ten.wl), (0, 0), "no live head, no lists");
}

/// The distinct-coin count folds spellings, so a repeated coin is counted once.
///
/// Strategy 7 blacklists `BTC,btc_rp,ETH`: three entries, two coins. Counting raw entries would
/// overstate the asserted blacklist count, while the coin table matches by normalized token.
#[test]
fn coin_lists_are_counted_by_distinct_token() {
    let c = fixture();
    let q = q();
    let src = unified_from(&c, &q).expect("source").expect("replica");
    let out = groups(&c, &src, None, &q, true, true).expect("groups");
    let seven = out.iter().find(|g| g.key == "7@1").expect("strategy 7");
    assert_eq!(seven.bl, 2, "BTC, btc_rp and ETH name two distinct coins");
    assert_eq!(seven.wl, 1);
    assert_eq!(seven.cores_n, 1, "the key splits per core");
    assert_eq!(seven.n, 2, "both of core 1's trades land in one group");
}

/// Raw profit and average order stay identical when the active Analytics lens changes.
///
/// Breakage this pins: deriving these fields from the active Percent source in
/// `analytics::groups`. That source removes non-positive-spend trades and would make the visible
/// Avg order / Profit % columns change when the user only switches the global profit lens.
#[test]
fn raw_strategy_metrics_are_independent_of_the_profit_lens() {
    let c = fixture();
    c.execute(
        "UPDATE orders_rep SET spentbtc = CASE closedate WHEN 1000 THEN 100.0 ELSE 0.0 END
         WHERE core_uid = 1 AND strategyid = 7",
        [],
    )
    .expect("vary order sizes");

    let mut quote_query = q();
    quote_query.metric = ProfitMetric::Quote;
    let quote_src = unified_from(&c, &quote_query)
        .expect("raw quote source")
        .expect("replica");
    let quote = groups(&c, &quote_src, None, &quote_query, true, true).expect("raw quote groups");

    let mut percent_query = q();
    percent_query.metric = ProfitMetric::Percent;
    let percent_src = unified_from(&c, &percent_query)
        .expect("percent source")
        .expect("replica");
    let raw_src = raw_source(&c, &percent_query)
        .expect("raw source")
        .expect("percent lens needs raw source");
    let percent = groups(&c, &percent_src, Some(&raw_src), &percent_query, true, true)
        .expect("percent groups");

    let quote = quote
        .iter()
        .find(|group| group.key == "7@1")
        .expect("raw quote strategy 7");
    let percent = percent
        .iter()
        .find(|group| group.key == "7@1")
        .expect("percent strategy 7");
    assert_eq!(quote.raw_profit, 6.0);
    assert_eq!(quote.avg_order, 100.0);
    assert_eq!(quote.profit_pct_of_avg_order(), 6.0);
    assert_eq!(percent.raw_profit, quote.raw_profit);
    assert_eq!(percent.avg_order, quote.avg_order);
    assert_eq!(percent.profit_pct_of_avg_order(), 6.0);
}

/// A mixed-quote group keeps dimensionless metrics but suppresses every raw-money derivative.
///
/// Removing the per-group quote metadata from `analytics::groups` makes strategy 7 expose its
/// cross-currency raw profit, average order, and derived Profit %, even when the whole Analytics
/// view is in the safe Percent lens.
#[test]
fn mixed_quote_group_suppresses_raw_money_fields() {
    let c = fixture();
    c.execute(
        "UPDATE orders_rep SET basecurrency = 8
         WHERE core_uid = 1 AND strategyid = 7 AND coin = 'ETH'",
        [],
    )
    .expect("make strategy 7 mixed quote");
    let mut query = q();
    query.metric = ProfitMetric::Percent;
    let percent_src = unified_from(&c, &query)
        .expect("percent source")
        .expect("replica");
    let raw_src = raw_source(&c, &query)
        .expect("raw source")
        .expect("percent lens needs raw source");

    let groups =
        groups(&c, &percent_src, Some(&raw_src), &query, true, true).expect("percent groups");
    let mixed = groups
        .iter()
        .find(|group| group.key == "7@1")
        .expect("strategy 7");
    assert_eq!(mixed.quote, QuoteScope::Mixed);
    assert!(mixed.raw_profit.is_nan());
    assert!(mixed.avg_order.is_nan());
    assert!(mixed.profit_pct_of_avg_order().is_nan());

    let single = groups
        .iter()
        .find(|group| group.key == "7@2")
        .expect("strategy 7 on the other core");
    assert!(matches!(single.quote, QuoteScope::Single(_)));
    assert!(single.raw_profit.is_finite());
}
