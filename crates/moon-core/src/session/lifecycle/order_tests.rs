//! Config-order insertion tests that avoid constructing live feed handles.

use super::{insert_index, rank_of};

/// Protects `SessionManager::insert_session`: replacing ranked insertion with `push`
/// moves a reactivated core to the end of every panel list.
#[test]
fn a_reactivated_core_returns_to_its_configured_place() {
    let order = vec![10u64, 20, 30];
    // Core 20 is absent while inactive.
    let live = vec![10u64, 30];
    // Reactivation restores its configured position.
    assert_eq!(insert_index(&live, &order, 20), 1);
}

/// Protects `insert_index`: empty, head, and tail insertions pin its boundary behavior.
#[test]
fn insertion_handles_both_ends() {
    let order = vec![10u64, 20, 30];
    assert_eq!(insert_index(&[], &order, 20), 0, "first core goes at 0");
    assert_eq!(insert_index(&[20, 30], &order, 10), 0, "lowest rank leads");
    assert_eq!(
        insert_index(&[10, 20], &order, 30),
        2,
        "highest rank trails"
    );
}

/// A core absent from the config (removed while its session lingers) must sink to the
/// tail rather than displace a configured one.
#[test]
fn an_unconfigured_core_sinks_to_the_tail() {
    let order = vec![10u64, 20];
    assert_eq!(rank_of(&order, 99), order.len());
    assert_eq!(insert_index(&[10, 20], &order, 99), 2);
}
