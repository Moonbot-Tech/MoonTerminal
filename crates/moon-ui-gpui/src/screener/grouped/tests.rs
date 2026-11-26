//! Account aggregation and chart core selection.

use super::*;

/// Preserve every member's account contribution and a common active leverage.
#[test]
fn n_cores_on_one_market_sum_orders_pnl_pos() {
    let members = (0..33).map(|core| MemberAcct {
        core,
        leverage_x: 10,
        max_leverage: 10,
        isolated: Some(true),
        pnl: if core == 0 { -30.41 } else { -19.0 },
        session: Some(0.25),
        pos: 0.5,
        orders: 2,
    });
    let agg = aggregate(members);
    assert_eq!(agg.orders, 66);
    assert!((agg.pnl - (-638.41)).abs() < 1e-9);
    assert_eq!(agg.session, Some(8.25));
    assert_eq!(agg.pos, 16.5);
    assert_eq!(
        agg.lev,
        LevSummary::Same {
            x: 10,
            max: 10,
            mode: Mode::I
        }
    );
}

/// Report the range when two sets of accounts use different active leverage.
#[test]
fn leverage_range_when_members_differ() {
    let members = (0..27).map(|core| MemberAcct {
        core,
        leverage_x: if core < 5 { 5 } else { 3 },
        max_leverage: 5,
        isolated: Some(true),
        ..Default::default()
    });
    assert_eq!(
        aggregate(members).lev,
        LevSummary::Range {
            lo: 3,
            hi: 5,
            max: 5,
            mode: Mode::I
        }
    );
}

/// Keep both stated modes in the summary even if the lead reports only one.
#[test]
fn mixed_margin_modes_keep_both_modes() {
    let members = [(5, Some(true)), (20, Some(false)), (10, None)].map(|(x, mode)| MemberAcct {
        leverage_x: x,
        max_leverage: 20,
        isolated: mode,
        ..Default::default()
    });
    assert_eq!(
        aggregate(members).lev,
        LevSummary::Range {
            lo: 5,
            hi: 20,
            max: 20,
            mode: Mode::Mixed
        }
    );
}

/// A stated zero or cancelling counters still produce an amount.
#[test]
fn session_dash_only_when_no_member_states_it() {
    let summaries = |values: [Option<f64>; 3]| {
        aggregate(values.map(|session| MemberAcct {
            session,
            ..Default::default()
        }))
        .session
    };
    assert_eq!(summaries([None, None, None]), None);
    assert_eq!(summaries([None, Some(0.0), None]), Some(0.0));
    assert_eq!(summaries([Some(3.0), None, Some(-3.0)]), Some(0.0));
}

/// Prefer activity in publication order, with the lead as the empty-account fallback.
#[test]
fn chart_core_prefers_member_with_position_or_orders() {
    let core_a = MemberAcct {
        core: 1,
        ..Default::default()
    };
    let core_b = MemberAcct {
        core: 2,
        ..Default::default()
    };
    let position = MemberAcct {
        pos: -2.0,
        ..core_b
    };
    let orders = MemberAcct {
        orders: 1,
        ..core_a
    };
    assert_eq!(chart_core([core_a, position]), Some(2));
    assert_eq!(chart_core([orders, position]), Some(1));
    assert_eq!(chart_core([position, orders]), Some(2));
    assert_eq!(chart_core([core_a, core_b]), Some(1));
    assert_eq!(chart_core([]), None);
}

/// Without an active setting, show the highest positive cap and no mode.
#[test]
fn unset_leverage_members_fall_back_to_max() {
    let members = [10, 20, 0].map(|max_leverage| MemberAcct {
        max_leverage,
        ..Default::default()
    });
    assert_eq!(aggregate(members).lev, LevSummary::Max(20));
    assert_eq!(aggregate([MemberAcct::default()]).lev, LevSummary::None);
}
