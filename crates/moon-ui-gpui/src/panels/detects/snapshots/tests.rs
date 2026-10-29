//! Late-completion regression tests for the same publisher used by GPUI tasks.

use std::collections::VecDeque;

use moon_core::market::DetectSnapshot;

use super::publish;
use crate::panels::detects::tests::detect_item;

/// All fields from one read must publish together, and an old generation cannot overwrite them.
#[test]
fn late_snapshot_cannot_overwrite_a_replacement_or_revive_a_dismissed_card() {
    let mut items = VecDeque::from([detect_item(Some(1), "XALPHA", 100.0, 2)]);
    let snapshot = DetectSnapshot {
        bars: vec![(1.0, 4.0, 1.0, 3.0)],
        line: vec![1.0, 3.0],
        delta_24h: 200.0,
        delta_1h: 50.0,
        exchange_kind: "DEX".to_string(),
        ..Default::default()
    };
    assert!(!publish(&mut items, 1, snapshot.clone()));
    assert!(items[0].bars.is_empty());
    assert!(publish(&mut items, 2, snapshot.clone()));
    assert_eq!(items[0].bars, snapshot.bars);
    assert_eq!(items[0].line, snapshot.line);
    assert_eq!(items[0].delta_24h, snapshot.delta_24h);
    assert_eq!(items[0].delta_1h, snapshot.delta_1h);
    assert_eq!(items[0].exchange_kind, snapshot.exchange_kind);
    items.clear();
    assert!(!publish(&mut items, 2, snapshot.clone()));
    // Same key and timestamp after dismissal still identifies a new card.
    items.push_back(detect_item(Some(1), "XALPHA", 100.0, 3));
    assert!(!publish(&mut items, 2, snapshot));
    assert!(items[0].bars.is_empty());
}

/// Pruning a crowd card removes its generation just as dismissing an ordinary card does.
#[test]
fn pruned_crowd_card_rejects_its_pending_snapshot() {
    let mut items = VecDeque::from([detect_item(None, "XALPHA", 100.0, 1)]);
    let snapshot = DetectSnapshot {
        line: vec![7.0],
        ..Default::default()
    };
    items.clear();
    assert!(!publish(&mut items, 1, snapshot));
    assert!(items.is_empty());
}

/// Time only the application-thread completion work for the maximum 48-card queue.
#[test]
#[ignore = "synthetic snapshot publication timing"]
fn detects_publication_scale_probe() {
    let mut items: VecDeque<_> = (1..=48)
        .map(|i| detect_item(Some(1), "XALPHA", 100.0, i))
        .collect();
    let snapshots: Vec<_> = (0..48)
        .map(|_| DetectSnapshot {
            bars: vec![(1.0, 2.0, 1.0, 2.0); 24],
            line: vec![2.0; 288],
            ..Default::default()
        })
        .collect();
    let start = std::time::Instant::now();
    for (i, snapshot) in snapshots.into_iter().enumerate() {
        assert!(publish(&mut items, i as u64 + 1, snapshot));
    }
    eprintln!(
        "detects_snapshot after_48_ui_publication_ms={:.6}",
        start.elapsed().as_secs_f64() * 1000.0
    );
}
