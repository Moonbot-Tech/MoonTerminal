use super::{strategies_attached, unified_from, Query};
use crate::db::SideFilter;
use rusqlite::Connection;

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
