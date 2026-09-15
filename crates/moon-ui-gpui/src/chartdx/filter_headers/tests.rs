//! Header-target regressions independent of a GPU device.

use super::FilterHeaderHit;
use moon_core::config::ChartLabelsCfg;
use std::rc::Rc;

/// Growing the hit target to the whole module consumes filter-body clicks; accepting a changed
/// configuration lets a stale row index toggle a different module after a profile edit.
#[test]
fn header_hit_is_bounded_and_rejects_a_replaced_profile() {
    let mut cfg = ChartLabelsCfg::empty();
    cfg.rows[0] = moon_core::config::strategy_filters_row();
    let hit = FilterHeaderHit {
        rect: [30.0, 50.0, 80.0, 20.0],
        row: 0,
        cfg: Rc::new(cfg.clone()),
    };
    assert!(hit.contains(30.0, 50.0, &cfg));
    assert!(hit.contains(110.0, 70.0, &cfg));
    assert!(!hit.contains(29.0, 50.0, &cfg));
    assert!(!hit.contains(50.0, 71.0, &cfg));
    cfg.rows.swap(0, 1);
    assert!(!hit.contains(50.0, 60.0, &cfg));
}
