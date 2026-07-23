use super::{effective_sid_expr, strategies_attached, unified_from, Query};
use std::collections::HashSet;

use crate::db::SideFilter;
use rusqlite::Connection;

fn cols(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

// ============================================================================
//  Strategy scope of a query — the tuner's Ctrl multi-select depends on it entirely.
// ============================================================================

/// Every selected strategy has to be in scope, not just the first. Scoping to one was
/// the bug where "plan vs fact" compared a single strategy while N were highlighted.
#[test]
fn scope_covers_every_selected_strategy() {
    let c = cols(&["closedate", "profitbtc", "strategyid", "core_uid"]);

    let one = Query {
        strategies: vec![(5, Some(7))],
        ..Default::default()
    };
    let w = one.where_sql(&c, "strategyid");
    assert!(w.contains("COALESCE(strategyid,0) = 5"), "{w}");
    assert!(w.contains("core_uid = 7"), "{w}");

    let many = Query {
        strategies: vec![(5, Some(7)), (9, Some(8)), (11, None)],
        ..Default::default()
    };
    let w = many.where_sql(&c, "strategyid");
    for sid in ["= 5", "= 9", "= 11"] {
        assert!(
            w.contains(sid),
            "strategy {sid} missing from the scope: {w}"
        );
    }
    assert_eq!(w.matches(" OR ").count(), 2, "three terms → two ORs: {w}");
    // The same strategy on ANOTHER core must not come along: each term pins its core.
    assert!(w.contains("core_uid = 8"), "{w}");

    // No selection = every strategy: no strategy predicate at all.
    let all = Query::default();
    let w = all.where_sql(&c, "strategyid");
    assert!(
        !w.contains("strategyid"),
        "unscoped query must not filter: {w}"
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
    let w = q.where_sql(&c, "strategyid");
    assert!(w.contains("1=0"), "{w}");
    assert!(
        !w.contains("strategyid"),
        "no such column may be referenced: {w}"
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
//  Liquidation-attribution wiring — the flag must reach the generated SQL.
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

fn q(on: bool) -> Query {
    Query {
        from: 0,
        to: i64::MAX,
        cores: Vec::new(),
        side: SideFilter::All,
        emulator: None,
        strategies: Vec::new(),
        attribute_liq: on,
        metric: Default::default(),
    }
}

/// The wiring test that matters: the flag must reach the generated SQL.
///
/// It did not, once. `attach_strategies` was called by `summary` alone while
/// `unified_from` is reached from fourteen places, so the expression fell back to the
/// plain column everywhere else — the strategy list and that strategy's own KPI would
/// have disagreed about which trades were its, silently and only on some screens.
#[test]
fn the_flag_reaches_the_generated_sql() {
    let c = conn_with_orders();
    c.execute_batch(
        "ATTACH ':memory:' AS strat;
         CREATE TABLE strat.strategies (core_uid INTEGER, strategy_id INTEGER,
             name TEXT, deleted INTEGER);",
    )
    .expect("attach");
    assert!(
        strategies_attached(&c),
        "the probe must see the attached db"
    );

    let off = unified_from(&c, &q(false)).expect("off").expect("a source");
    assert!(!off.contains("LIQUIDATION"), "{off}");

    let on = unified_from(&c, &q(true)).expect("on").expect("a source");
    assert!(on.contains("'LIQUIDATION'"), "{on}");
    assert!(on.contains("strat.strategies"), "{on}");
    // Published under the original name, so no outer query needed changing.
    assert!(on.contains("AS \"strategyid\""), "{on}");
    // RUN IT. Every other assertion here matches strings, and a string-matching test
    // happily passed SQL that SQLite refuses ("no such column" from a correlated
    // reference in a subquery's ORDER BY). Preparing the statement is what catches that.
    c.prepare(&format!("SELECT * FROM {on}"))
        .unwrap_or_else(|e| {
            panic!(
                "the generated SQL must be valid: {e}
{on}"
            )
        });
}

/// Without the strategy database there is nothing to match against, so the expression
/// must fall back rather than emit SQL that cannot run.
#[test]
fn without_the_strategy_db_it_falls_back_silently() {
    let c = conn_with_orders();
    assert!(!strategies_attached(&c));
    let on = unified_from(&c, &q(true)).expect("on").expect("a source");
    assert!(!on.contains("LIQUIDATION"), "{on}");
    c.prepare(&format!("SELECT * FROM {on}"))
        .expect("must still be valid SQL");
}
