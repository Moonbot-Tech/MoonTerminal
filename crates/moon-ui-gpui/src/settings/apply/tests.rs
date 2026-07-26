//! Core-move contracts for Settings window reconciliation and chart persistence.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{moved_cores, sanitize_moved_chart_specs};
use crate::persistence::chart_persist::ChartTabSpec;
use moon_core::config::ChartBucket;

/// Regression target: comparing only the set of group names leaves a detached G1 host alive after
/// core 7 moves to G2, so its hotkeys edit G1 while its chart order reads core 7's new G2 settings.
#[test]
fn moving_a_core_requires_an_old_group_topology_rebuild() {
    let before = vec![
        (7, "desk-a".to_string()),
        (9, "desk-b".to_string()),
        (11, "desk-a".to_string()),
    ];
    let after = vec![
        (7, "desk-b".to_string()),
        (9, "desk-b".to_string()),
        (11, "desk-a".to_string()),
    ];

    let moved = moved_cores(&before, &after);
    assert_eq!(moved.len(), 1);
    assert!(moved.contains(&(7, "desk-a".to_string())));
    assert!(!moved.contains(&(7, "desk-b".to_string())));
}

/// Regression target: rebuilding windows without pruning moved cores restores an old-group custom
/// chart whose hotkeys edit desk-a while its order target now reads desk-b settings.
#[test]
fn moved_cores_are_removed_from_old_group_chart_specs() {
    let mut mixed = ChartTabSpec::new("desk-a".to_string(), 1, ChartBucket::Shared);
    mixed.custom_coins = Some(vec![
        (7, "ETHUSDT".to_string()),
        (11, "BTCUSDT".to_string()),
    ]);
    mixed.compare_anchor = Some((7, "ETHUSDT".to_string()));
    let dedicated = ChartTabSpec::new("desk-a".to_string(), 2, ChartBucket::Core(7));
    let unrelated = ChartTabSpec::new("desk-b".to_string(), 3, ChartBucket::Core(7));
    let mut specs = vec![mixed, dedicated, unrelated];

    assert!(sanitize_moved_chart_specs(
        &mut specs,
        &[(7, "desk-a".to_string())]
    ));
    assert_eq!(specs.len(), 2);
    let old_group = specs
        .iter()
        .find(|spec| spec.group == "desk-a")
        .expect("mixed old-group custom tab remains");
    assert_eq!(
        old_group.custom_coins.as_deref(),
        Some([(11, "BTCUSDT".to_string())].as_slice())
    );
    assert_eq!(old_group.compare_anchor, None);
    assert!(specs.iter().any(|spec| spec.group == "desk-b"));
}
