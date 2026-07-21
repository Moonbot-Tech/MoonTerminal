use super::*;
use std::collections::HashSet;

fn cols(names: &[&str]) -> HashSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// Every selected strategy has to be in scope, not just the first. Scoping to one was
/// the bug where "plan vs fact" compared a single strategy while N were highlighted.
#[test]
fn scope_covers_every_selected_strategy() {
    let c = cols(&["closedate", "profitbtc", "strategyid", "core_uid"]);

    let one = Query {
        strategies: vec![(5, Some(7))],
        ..Default::default()
    };
    let w = one.where_sql(&c);
    assert!(w.contains("COALESCE(strategyid,0) = 5"), "{w}");
    assert!(w.contains("core_uid = 7"), "{w}");

    let many = Query {
        strategies: vec![(5, Some(7)), (9, Some(8)), (11, None)],
        ..Default::default()
    };
    let w = many.where_sql(&c);
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
    let w = all.where_sql(&c);
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
    let w = q.where_sql(&c);
    assert!(w.contains("1=0"), "{w}");
    assert!(
        !w.contains("strategyid"),
        "no such column may be referenced: {w}"
    );
}
