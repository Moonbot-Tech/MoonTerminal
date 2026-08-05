use super::{
    effective_sid_expr, quote_breakdown_on, strategies_attached, unified_from, unified_from_mode,
    ProjectionMode, Query,
};
use std::collections::HashSet;

use crate::db::SideFilter;
use rusqlite::Connection;

fn cols(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

// ============================================================================
//  Strategy scope of a query - the tuner's Ctrl multi-select depends on it entirely.
// ============================================================================

/// Every selected strategy has to be in scope, not just the first. Scoping to one was
/// the bug where "plan vs fact" compared a single strategy while N were highlighted.
#[test]
fn scope_covers_every_selected_strategy() {
    let c = cols(&["closedate", "profitbtc", "strategyid", "core_uid"]);
    let period = "closedate >= ?1 AND closedate < ?2 AND closedate > 0";

    let one = Query {
        strategies: vec![(5, Some(7))],
        ..Default::default()
    };
    let branches = one.where_branches(period, &c, "strategyid", None, false);
    assert_eq!(branches.len(), 1);
    assert!(branches[0].contains("strategyid = 5"), "{branches:#?}");
    assert!(branches[0].contains("core_uid = 7"), "{branches:#?}");

    let many = Query {
        strategies: vec![(5, Some(7)), (9, Some(8)), (11, None)],
        ..Default::default()
    };
    let branches = many.where_branches(period, &c, "strategyid", None, false);
    assert_eq!(branches.len(), 1);
    let sql = branches.join(" UNION ALL ");
    for sid in ["= 5", "= 9", "= 11"] {
        assert!(
            sql.contains(sid),
            "strategy {sid} missing from the scope: {sql}"
        );
    }
    // The same strategy on ANOTHER core must not come along: each term pins its core.
    assert!(sql.contains("core_uid = 8"), "{sql}");

    // No selection = every strategy: no strategy predicate at all.
    let all = Query::default();
    let branches = all.where_branches(period, &c, "strategyid", None, false);
    assert_eq!(branches.len(), 1);
    assert!(
        !branches[0].contains("strategyid"),
        "unscoped query must not filter: {branches:#?}"
    );
}

/// A source with no `strategyid` column cannot say which strategy a row belongs to, so
/// under a strategy scope it must contribute NOTHING rather than every row it holds.
#[test]
fn scope_excludes_a_source_that_cannot_attribute() {
    let c = cols(&["closedate", "profitbtc", "core_uid"]);
    let q = Query {
        strategies: vec![(5, Some(7))],
        ..Default::default()
    };
    let branches = q.where_branches("closedate > 0", &c, "strategyid", None, false);
    assert_eq!(branches.len(), 1);
    assert!(branches[0].contains("1=0"), "{branches:#?}");
    assert!(
        !branches[0].contains("strategyid"),
        "no such column may be referenced: {branches:#?}"
    );
}

// ============================================================================
//  Liquidation attribution — the effective-strategy-id SQL expression.
// ============================================================================

fn full() -> HashSet<String> {
    cols(&[
        "strategyid",
        "core_uid",
        "channelname",
        "signaltype",
        "comment",
    ])
}

/// Off, or with no strategy database attached, the expression is the bare column — the
/// panel must behave exactly as it did before the feature existed.
#[test]
fn without_attribution_it_is_the_plain_column() {
    assert_eq!(effective_sid_expr("r", &full(), false), "r.\"strategyid\"");
    assert_eq!(effective_sid_expr("r", &full(), false), "r.\"strategyid\"");
}

/// `core_uid` is named by the correlated subquery, so a source without it must fall back:
/// naming it anyway makes the branch fail to PREPARE, and the whole window dies the moment
/// the switch is turned on.
#[test]
fn a_source_without_core_uid_falls_back() {
    let no_core = cols(&["strategyid", "channelname", "signaltype"]);
    assert_eq!(effective_sid_expr("r", &no_core, true), "r.\"strategyid\"");
}

/// The whole trimmed value is tried BEFORE the bracket-cut one, so a strategy whose own
/// name contains a bracket matches itself rather than a different strategy that happens to
/// be named after its prefix.
#[test]
fn the_whole_name_is_preferred_over_the_cut_one() {
    let e = effective_sid_expr("r", &full(), true);
    let whole = e
        .find("st.name = trim(COALESCE")
        .expect("whole-name lookup");
    let cut = e.find("st.name = trim(substr").expect("cut-name lookup");
    assert!(whole < cut, "the exact match must be tried first: {e}");
    // The obvious spelling — `IN (whole, cut) ORDER BY name = whole DESC` — compiles and
    // reads well, and SQLite refuses it: a correlated reference is not allowed in a
    // subquery's ORDER BY. It passed every string assertion and died at runtime.
    assert!(!e.contains("ORDER BY"), "{e}");
}

/// A source that cannot answer must not be made to guess: without `channelname` there is
/// no way to know a row is a liquidation, and without a name column no way to say whose.
#[test]
fn a_source_missing_its_inputs_falls_back() {
    let no_channel = cols(&["strategyid", "core_uid", "signaltype"]);
    assert_eq!(
        effective_sid_expr("r", &no_channel, true),
        "r.\"strategyid\""
    );
    let no_name = cols(&["strategyid", "core_uid", "channelname"]);
    assert_eq!(effective_sid_expr("r", &no_name, true), "r.\"strategyid\"");
}

/// The detection is an EXACT match. A substring test picks up 15 696 rows against 319 real
/// ones on the production database, because a strategy is named `Liquidations_Short_…`
/// and that name sits in `channelname` on every trade it makes.
#[test]
fn detection_is_exact_never_a_substring() {
    let e = effective_sid_expr("r", &full(), true);
    assert!(e.contains("= 'LIQUIDATION'"), "{e}");
    assert!(
        !e.contains("LIKE"),
        "a LIKE here would swallow a strategy's own name: {e}"
    );
}

/// The correlated subquery MUST name the outer row's core explicitly. Unqualified
/// `core_uid` binds to `strat.strategies` instead, and the lookup then matches a strategy
/// of that name on ANY core — silently attributing the loss to someone else's copy.
#[test]
fn the_subquery_qualifies_the_outer_core() {
    let e = effective_sid_expr("r", &full(), true);
    assert!(e.contains("st.core_uid = r.\"core_uid\""), "{e}");
    assert!(
        e.contains("st.deleted = 0"),
        "a deleted strategy is not an owner: {e}"
    );
}

/// An unmatched name yields 0, which is "Manual" — where the row already was. Measured on
/// the real database: 28 of 319 stay there (a deleted strategy, or no parseable name).
#[test]
fn an_unknown_name_stays_manual() {
    let e = effective_sid_expr("r", &full(), true);
    assert!(
        e.contains("), 0)"),
        "the lookup must COALESCE to 0, not NULL: {e}"
    );
}

/// `signaltype` is preferred and `comment` is the fallback — measured at 288/319 and
/// 304/319 respectively, so neither alone covers the set.
#[test]
fn the_name_comes_from_signaltype_then_comment() {
    let e = effective_sid_expr("r", &full(), true);
    let sig = e.find("signaltype").expect("signaltype used");
    let com = e.find("comment").expect("comment used");
    assert!(sig < com, "signaltype must be tried first: {e}");
    assert!(
        e.contains("instr("),
        "the name is cut at the first bracket: {e}"
    );
}

// ============================================================================
//  Liquidation attribution — unconditional whenever the strategy DB is attached.
// ============================================================================

fn conn_with_orders() -> Connection {
    let c = Connection::open_in_memory().expect("memory db");
    c.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, coin TEXT,
             isshort INTEGER, buydate INTEGER, closedate INTEGER, profitbtc REAL,
             strategyid INTEGER, emulator INTEGER, spentbtc REAL,
             channelname TEXT, signaltype TEXT, comment TEXT);",
    )
    .expect("schema");
    c
}

/// Attach an empty strategy database, as every real reader does.
fn attach_strategies(c: &Connection) {
    c.execute_batch(
        "ATTACH ':memory:' AS strat;
         CREATE TABLE strat.strategies (core_uid INTEGER, strategy_id INTEGER,
             name TEXT, deleted INTEGER);",
    )
    .expect("attach");
}

/// Build the baseline Analytics query used by liquidation-attribution tests.
fn q() -> Query {
    Query {
        from: 0,
        to: i64::MAX,
        cores: Vec::new(),
        side: SideFilter::All,
        emulator: None,
        strategies: Vec::new(),
        metric: Default::default(),
        valuation: Default::default(),
    }
}

/// Read the projected `strategyid` of every row, in insertion order.
fn projected_sids(c: &Connection, src: &str) -> Vec<i64> {
    let sql = format!("SELECT o.strategyid FROM {src} ORDER BY o.closedate");
    let mut stmt = c
        .prepare(&sql)
        .unwrap_or_else(|e| panic!("the generated SQL must be valid: {e}\n{src}"));
    let rows = stmt
        .query_map(rusqlite::params![0i64, i64::MAX], |r| r.get::<_, i64>(0))
        .expect("query");
    rows.map(|r| r.expect("row")).collect()
}

/// Aggregate the rows selected from a generated Analytics source.
///
/// Args:
///     connection: Database holding the source's physical tables.
///     source: Generated unified source SQL.
///
/// Returns:
///     Selected row count and exact profit sum.
fn selected_count_and_profit(connection: &Connection, source: &str) -> (i64, f64) {
    connection
        .query_row(
            &format!("SELECT COUNT(*), COALESCE(SUM(o.pnl), 0.0) FROM {source}"),
            rusqlite::params![0_i64, i64::MAX],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("aggregate generated source")
}

/// The assertion that matters: a liquidation really is re-attributed to the strategy
/// named in the row, and nothing else moves.
///
/// This runs the projection against real rows because matching SQL substrings cannot prove that
/// the expression resolves the same owner for the strategy list and that strategy's KPI.
#[test]
fn a_liquidation_is_attributed_to_the_strategy_named_in_the_row() {
    let c = conn_with_orders();
    attach_strategies(&c);
    assert!(
        strategies_attached(&c),
        "the probe must see the attached db"
    );
    c.execute_batch(
        "INSERT INTO strat.strategies VALUES (7, 42, 'MainShotS', 0);
         -- 1: an ordinary trade of another strategy — must not move.
         INSERT INTO orders_rep VALUES
             (7,'c','BTCUSDT',0,900,1000,3.5,13,0,100.0,'','','');
         -- 2: a liquidation whose name matches — must become 42.
         INSERT INTO orders_rep VALUES
             (7,'c','BTCUSDT',0,900,2000,-5.0,0,0,100.0,'LIQUIDATION','MainShotS  ( MoonShot )','');
         -- 3: a liquidation naming a strategy that does not exist — must stay in Manual.
         INSERT INTO orders_rep VALUES
             (7,'c','BTCUSDT',0,900,3000,-2.0,0,0,100.0,'LIQUIDATION','GhostStrat','');
         -- 4: same name, but on a DIFFERENT core — the match is per core, so it stays 0.
         INSERT INTO orders_rep VALUES
             (9,'c','BTCUSDT',0,900,4000,-1.0,0,0,100.0,'LIQUIDATION','MainShotS','');",
    )
    .expect("rows");

    let src = unified_from(&c, &q()).expect("read").expect("a source");
    assert_eq!(
        projected_sids(&c, &src),
        vec![13, 42, 0, 0],
        "only the matching liquidation moves, and only on its own core"
    );
}

/// A deleted strategy must not reclaim the money: the row stays in "Manual" rather than
/// being handed to a name that no longer exists.
#[test]
fn a_deleted_strategy_does_not_reclaim_its_liquidations() {
    let c = conn_with_orders();
    attach_strategies(&c);
    c.execute_batch(
        "INSERT INTO strat.strategies VALUES (7, 42, 'MainShotS', 1);
         INSERT INTO orders_rep VALUES
             (7,'c','BTCUSDT',0,900,1000,-5.0,0,0,100.0,'LIQUIDATION','MainShotS','');",
    )
    .expect("rows");

    let src = unified_from(&c, &q()).expect("read").expect("a source");
    assert_eq!(projected_sids(&c, &src), vec![0]);
}

/// Strategy scopes must cover direct and attributed rows without duplicating a physical trade.
///
/// Replacing `Query::where_branches` with one `COALESCE(effective_sid, 0)` predicate makes the
/// planner lose the physical strategy key. Dropping a zero/NULL branch omits a liquidation or a
/// Manual row; using overlapping branches counts it twice. This fixture proves the same exact
/// sets through both the quote preflight and payload source, across typed and legacy tables.
/// Expanding one compound branch per selected strategy makes the 300-key case exceed SQLite's
/// 500-term limit instead of returning the same scoped result.
#[test]
fn strategy_branches_are_disjoint_and_semantically_complete() {
    let connection = conn_with_orders();
    connection
        .execute_batch(
            "ALTER TABLE orders_rep ADD COLUMN basecurrency INTEGER;
             ALTER TABLE orders_rep ADD COLUMN deleted INTEGER;
             CREATE TABLE closed_sell_reports AS SELECT * FROM orders_rep WHERE 0;
             CREATE INDEX typed_strategy_close
                 ON orders_rep(core_uid, strategyid, closedate);
             CREATE INDEX legacy_strategy_close
                 ON closed_sell_reports(core_uid, strategyid, closedate);",
        )
        .expect("two-source schema");
    attach_strategies(&connection);
    connection
        .execute_batch(
            "INSERT INTO strat.strategies VALUES (7, 42, 'Owner', 0);
             INSERT INTO strat.strategies VALUES (7, 43, 'DeletedOwner', 1);
             INSERT INTO orders_rep VALUES
                 (7,'c','BTCUSDT',0,900,1000,1.0,42,0,100.0,'','','',1,0),
                 (7,'c','BTCUSDT',0,900,2000,2.0,0,0,100.0,'LIQUIDATION','Owner','',1,0),
                 (7,'c','BTCUSDT',0,900,3000,3.0,NULL,0,100.0,'LIQUIDATION','Owner','',1,0),
                 (7,'c','BTCUSDT',0,900,4000,4.0,0,0,100.0,'LIQUIDATION','Ghost','',1,0),
                 (7,'c','BTCUSDT',0,900,5000,5.0,NULL,0,100.0,'LIQUIDATION','Ghost','',1,0),
                 (7,'c','BTCUSDT',0,900,6000,6.0,0,0,100.0,'LIQUIDATION','DeletedOwner','',1,0),
                 (7,'c','BTCUSDT',0,900,7000,7.0,0,0,100.0,'ORDINARY','','',1,0),
                 (7,'c','BTCUSDT',0,900,8000,8.0,NULL,0,100.0,'ORDINARY','','',1,0),
                 (9,'c','BTCUSDT',0,900,9000,9.0,42,0,100.0,'','','',1,0);
             INSERT INTO closed_sell_reports SELECT * FROM orders_rep;",
        )
        .expect("strategy branch fixture");

    let selected = Query {
        strategies: vec![(42, Some(7))],
        ..q()
    };
    let source = unified_from(&connection, &selected)
        .expect("strategy source")
        .expect("strategy tables");
    assert_eq!(selected_count_and_profit(&connection, &source), (6, 12.0));
    let preflight = quote_breakdown_on(&connection, &selected).expect("strategy quote preflight");
    assert_eq!(preflight.orders, 6);
    assert_eq!(preflight.totals[0].profit, 12.0);

    let manual = Query {
        strategies: vec![(0, Some(7))],
        ..q()
    };
    let source = unified_from(&connection, &manual)
        .expect("manual source")
        .expect("manual tables");
    assert_eq!(selected_count_and_profit(&connection, &source), (10, 60.0));
    let preflight = quote_breakdown_on(&connection, &manual).expect("manual quote preflight");
    assert_eq!(preflight.orders, 10);
    assert_eq!(preflight.totals[0].profit, 60.0);

    let any_core_deduplicated = Query {
        strategies: vec![(42, Some(7)), (42, Some(7)), (42, None), (42, Some(9))],
        ..q()
    };
    let source = unified_from(&connection, &any_core_deduplicated)
        .expect("any-core source")
        .expect("strategy tables");
    assert_eq!(selected_count_and_profit(&connection, &source), (8, 30.0));

    let large_selection = Query {
        strategies: (1..=300).map(|sid| (sid, Some(7))).collect(),
        ..q()
    };
    let source = unified_from(&connection, &large_selection)
        .expect("large strategy source")
        .expect("strategy tables");
    assert_eq!(
        source.matches(" UNION ALL ").count(),
        5,
        "two sources must stay bounded at three raw-id branches each"
    );
    assert_eq!(selected_count_and_profit(&connection, &source), (6, 12.0));
    let preflight =
        quote_breakdown_on(&connection, &large_selection).expect("large strategy quote preflight");
    assert_eq!(preflight.orders, 6);
    assert_eq!(preflight.totals[0].profit, 12.0);
}

/// Without the strategy database there is nothing to match against, so the expression
/// must fall back rather than emit SQL that cannot run.
#[test]
fn without_the_strategy_db_it_falls_back_silently() {
    let c = conn_with_orders();
    assert!(!strategies_attached(&c));
    c.execute_batch(
        "INSERT INTO orders_rep VALUES
             (7,'c','BTCUSDT',0,900,1000,-5.0,0,0,100.0,'LIQUIDATION','MainShotS','');",
    )
    .expect("rows");
    let src = unified_from(&c, &q()).expect("read").expect("a source");
    assert!(!src.contains("LIQUIDATION"), "{src}");
    // RUN IT: a string-matching test happily passed SQL that SQLite refuses ("no such
    // column" from a correlated reference in a subquery's ORDER BY).
    assert_eq!(projected_sids(&c, &src), vec![0]);
}

/// Selecting the current-rate mode must change the SQL, not merely the label above it.
///
/// Breakage: `analytics/query/mod.rs::unified_from_mode` keeping `mode == ProjectionMode::Usdt`
/// where it now asks `mode.valuation()`. A new mode would fall into the `else` arm, emit the plain
/// native projection, and every Analytics surface would show unconverted money under a heading
/// that promised current-rate USDT.
#[test]
fn the_current_rate_mode_emits_its_own_projection() {
    let c = conn_with_orders();
    c.execute_batch(
        "ATTACH ':memory:' AS valuation;
         CREATE TABLE valuation.trade_values (source_kind INTEGER, core_uid INTEGER,
             row_id INTEGER, algorithm_version INTEGER, closedate INTEGER,
             quote_ordinal INTEGER, profit_quote REAL, spent_quote REAL,
             profit_usdt REAL, spent_usdt REAL, rate_usdt REAL, rate_minute_utc INTEGER);
         CREATE TABLE valuation.rates (algorithm_version INTEGER, quote_ordinal INTEGER,
             minute_utc INTEGER, status INTEGER, provider TEXT, symbol TEXT,
             orientation INTEGER);
         ALTER TABLE orders_rep ADD COLUMN basecurrency INTEGER;
         ALTER TABLE orders_rep ADD COLUMN newrecid INTEGER;",
    )
    .expect("valuation fixture");

    let historical = unified_from_mode(&c, &q(), ProjectionMode::Usdt)
        .expect("read")
        .expect("a source");
    let current = unified_from_mode(&c, &q(), ProjectionMode::UsdtCurrent)
        .expect("read")
        .expect("a source");

    let native = unified_from_mode(&c, &q(), ProjectionMode::Native)
        .expect("read")
        .expect("a source");

    assert_ne!(historical, current, "the two conversions cannot share SQL");
    assert!(
        historical.contains("valuation.trade_values"),
        "the historical projection reads the derived cache"
    );
    assert!(
        !current.contains("valuation."),
        "the current-rate projection joins nothing at all, got {current}"
    );
    // Both negative assertions above are also satisfied by the plain NATIVE projection, which is
    // exactly what the named breakage produces. These two are the positive half: the current-rate
    // mode must APPLY a conversion, not merely decline to read the cache.
    assert_ne!(
        native, current,
        "falling through to native money is the failure this test exists to catch"
    );
    assert!(
        current.contains("CASE r.basecurrency WHEN 1 THEN 1.0"),
        "the current-rate projection must carry the rate CASE, got {current}"
    );
}
