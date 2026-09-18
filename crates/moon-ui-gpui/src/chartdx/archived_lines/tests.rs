//! The ranking behind the lines style's asks, and the store it draws.

use std::collections::HashMap;
use std::sync::Arc;

use moon_core::config::ChartGraphicsCfg;
use moon_core::db::{ChartTradeRecord, ReportAxis};
use moon_core::feed::{ArchivedLineKind, ArchivedOrderTrace};

// Named imports, not `super::*`: the parent's parent glob-imports gpui, whose `test` attribute
// would shadow the built-in one under a glob.
use super::{archived_store, rank_wanted};

fn record(
    id: i64,
    core: u64,
    close_date: i64,
    uid: Option<i64>,
    emulator: bool,
) -> ChartTradeRecord {
    ChartTradeRecord {
        record_id: id,
        core_uid: core,
        coin: "ADAUSDT".into(),
        buy_date: close_date - 600,
        close_date,
        buy_ms: None,
        close_ms: None,
        buy_price: 1.0,
        sell_price: 1.1,
        quantity: 100.0,
        is_short: false,
        emulator,
        profit: None,
        quote: None,
        profit_pct: None,
        report_uid: uid,
    }
}

fn lines() -> Arc<[ArchivedOrderTrace]> {
    Arc::from(vec![ArchivedOrderTrace {
        own: true,
        kind: ArchivedLineKind::Entry,
        stop_price: None,
        stop_time_ms: None,
        points: vec![(1_700_000_000_000.0, 1.0)],
    }])
}

#[test]
fn ranking_is_nearest_the_right_edge_first_and_skips_what_cannot_be_asked() {
    let axis = ReportAxis::identity_core_local();
    let records = vec![
        record(1, 7, 1_700_000_000, Some(11), false),
        record(2, 7, 1_700_000_500, Some(12), false),
        record(3, 7, 1_700_000_900, Some(13), false),
        record(4, 7, 1_700_000_950, None, false),
        record(5, 7, 1_700_000_960, Some(15), true),
        record(6, 8, 1_700_000_990, Some(16), false),
    ];
    let graphics = ChartGraphicsCfg {
        show_emulator_trades: false,
        ..ChartGraphicsCfg::default()
    };
    // The edge sits at the last trade's close; nearest first, other core and hidden kind out.
    let edge = 1_700_000_960_000.0;
    let wanted = rank_wanted(&records, &[(7, edge)], &graphics, &axis, 10);
    assert_eq!(wanted, vec![13, 12, 11]);
    // The cap cuts the far end.
    assert_eq!(
        rank_wanted(&records, &[(7, edge)], &graphics, &axis, 1),
        vec![13]
    );
    // A second pane on the other core brings its own trade, once.
    let two = rank_wanted(
        &records,
        &[(7, edge), (8, edge), (8, edge)],
        &graphics,
        &axis,
        10,
    );
    assert_eq!(two, vec![16, 13, 12, 11]);
}

#[test]
fn store_holds_only_the_answered_trades_of_the_pane_and_none_means_nothing_drawn() {
    let axis = ReportAxis::identity_core_local();
    let records = vec![
        record(1, 7, 1_700_000_000, Some(11), false),
        record(2, 7, 1_700_000_500, Some(12), false),
        record(3, 8, 1_700_000_900, Some(13), false),
    ];
    let mut resolved = HashMap::new();
    resolved.insert(11, lines());
    resolved.insert(13, lines());
    resolved.insert(99, lines());
    let graphics = ChartGraphicsCfg::default();
    let store = archived_store(&records, &resolved, 7, "ADAUSDT", &[], &graphics, &axis)
        .expect("one trade of core 7 is answered");
    let drawn = store.market_draw_orders("ADAUSDT", usize::MAX);
    assert_eq!(
        drawn.len(),
        1,
        "12 is unanswered, 13 is another core's, 99 is nobody's"
    );
    assert!(
        drawn.iter().all(|order| order.subject),
        "the lines style draws in full colour, as Moonbot does"
    );
    assert_eq!(drawn[0].closed_ms, Some(1_700_000_000_000.0));
    assert!(archived_store(&records, &resolved, 9, "ADAUSDT", &[], &graphics, &axis).is_none());
    assert!(
        archived_store(
            &records,
            &HashMap::new(),
            7,
            "ADAUSDT",
            &[],
            &graphics,
            &axis
        )
        .is_none()
    );
}

#[test]
fn a_trade_whose_order_closed_this_session_is_left_to_the_live_store() {
    let axis = ReportAxis::identity_core_local();
    let records = vec![
        record(1, 7, 1_700_000_000, Some(11), false),
        record(2, 7, 1_700_000_500, Some(12), false),
    ];
    let mut resolved = HashMap::new();
    resolved.insert(11, lines());
    resolved.insert(12, lines());
    let graphics = ChartGraphicsCfg::default();
    // The live store closed an order 3 s after the row's close instant: same trade, skipped.
    let live = [1_700_000_003_000.0];
    let store = archived_store(&records, &resolved, 7, "ADAUSDT", &live, &graphics, &axis)
        .expect("the other trade still draws from the archive");
    let drawn = store.market_draw_orders("ADAUSDT", usize::MAX);
    assert_eq!(drawn.len(), 1);
    assert_eq!(drawn[0].closed_ms, Some(1_700_000_500_000.0));
    // Both twinned: nothing is drawn from the archive at all.
    let both = [1_700_000_003_000.0, 1_700_000_490_000.0];
    assert!(archived_store(&records, &resolved, 7, "ADAUSDT", &both, &graphics, &axis).is_none());
}
