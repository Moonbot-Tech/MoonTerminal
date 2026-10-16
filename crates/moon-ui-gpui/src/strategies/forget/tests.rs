//! Full-roster history purge planning, independent of drawn rows and filtered badges.

use super::ForgetPlan;
use moon_core::strat_db::stats::HeadRow;
use std::collections::HashMap;

/// Build a synthetic locally retained deleted strategy.
fn head(core_uid: u64, strategy_id: i64) -> HeadRow {
    HeadRow {
        core_uid,
        strategy_id,
        name: format!("UNIT_{strategy_id:02}"),
        kind: "Long".into(),
        kind_ordinal: 1,
        folder_path: String::new(),
        is_short: false,
    }
}

/// Using drawn roots or a filtered badge would miss CORE_B's collapsed/search-hidden history;
/// iterating the whole deleted cache would also purge another group's CORE_C.
#[test]
fn forget_plan_covers_full_roster_and_counts_all_deleted_rows() {
    let deleted = HashMap::from([
        (11, vec![head(11, 1), head(11, 2)]),
        (22, vec![head(22, 1)]),
        (33, vec![head(33, 1)]),
        (44, Vec::new()),
    ]);
    let plan = ForgetPlan::capture(&[11, 22, 44, 55], &deleted);
    assert_eq!(plan.targets, vec![(11, vec![1, 2]), (22, vec![1])]);
    assert_eq!(plan.count(), 3);
    assert!(plan.available());
    let single = ForgetPlan::capture(&[11, 44], &deleted);
    assert_eq!(single.count(), 2);
    assert!(!single.available());
    let empty = ForgetPlan::capture(&[44, 55], &deleted);
    assert_eq!(empty.count(), 0);
    assert!(!empty.available());
}
