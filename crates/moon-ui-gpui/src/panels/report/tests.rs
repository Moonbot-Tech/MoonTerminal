//! Unit tests for the report width-budget clamp policy ([`super::plan_clamp`]).
//!
//! These pin the invariant a real regression would break: the clamp must never keep
//! asking to change widths once no fitting solution exists, or the observe→notify loop
//! that drives it spins forever (CPU pegged, window frozen — the reported freeze when
//! every column is shown in a narrow detached window and a column is dragged).
//!
//! Explicit imports only — the parent re-exports `gpui::*`, whose `test` would shadow the
//! built-in attribute and make `#[test]` expand recursively.

use super::plan_clamp;
use std::collections::HashMap;

fn widths(pairs: &[(&str, f32)]) -> HashMap<String, f32> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn cols(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn sum_visible(w: &HashMap<String, f32>, visible: &[String]) -> f32 {
    visible.iter().map(|c| w[c]).sum()
}

/// Columns already within the viewport → no change requested.
#[test]
fn within_budget_is_noop() {
    let cur = widths(&[("a", 100.0), ("b", 100.0)]);
    let vis = cols(&["a", "b"]);
    assert!(plan_clamp(&cur, &vis, &cur, 900.0).is_none());
}

/// Every column visible in a window narrower than `40px × columns` has no fitting
/// packing. The clamp must decline to touch the widths — the case that used to spin the
/// observe→notify loop (all report columns selected in a detached window).
#[test]
fn infeasible_layout_is_noop() {
    // 40 columns, 40px floor each = 1600px minimum, viewport only 1200.
    let names: Vec<String> = (0..40).map(|i| format!("c{i}")).collect();
    let cur: HashMap<String, f32> = names.iter().map(|n| (n.clone(), 90.0)).collect();
    assert!(plan_clamp(&cur, &names, &cur, 1200.0).is_none());
}

/// A feasible overflow is scaled to fit, and applying the result again is a no-op: the
/// clamp converges instead of re-arming its own observer. The second pass returning
/// `None` is the anti-loop oracle, independent of how the first pass computed widths.
#[test]
fn feasible_overflow_fits_then_terminates() {
    let cur = widths(&[("a", 500.0), ("b", 500.0), ("c", 500.0)]);
    let vis = cols(&["a", "b", "c"]);
    let next = plan_clamp(&cur, &vis, &cur, 900.0).expect("overflow should be clamped");
    assert!(
        sum_visible(&next, &vis) <= 900.5,
        "clamped sum must fit the viewport"
    );
    assert!(
        plan_clamp(&next, &vis, &next, 900.0).is_none(),
        "clamp must terminate, not re-notify on a settled layout"
    );
}

/// Simulate the live observe→notify→clamp feedback loop: feed each `plan_clamp` result
/// back as its own input and confirm it reaches a fixpoint (`None`) in a bounded number
/// of steps. An unbounded iteration here is exactly the reported freeze; the step cap is
/// the oracle, independent of the width arithmetic.
#[test]
fn clamp_feedback_converges_for_every_layout() {
    let cases: &[(Vec<(&str, f32)>, f32)] = &[
        (vec![("a", 500.0), ("b", 500.0), ("c", 500.0)], 900.0), // feasible overflow
        (vec![("a", 300.0), ("b", 60.0), ("c", 60.0)], 200.0),   // proportional hits the floor
        (vec![("a", 90.0), ("b", 90.0)], 120.0),                 // tight but feasible
        (vec![("a", 300.0), ("b", 300.0), ("c", 300.0)], 100.0), // infeasible: 3×40 > 100
    ];
    for (pairs, vp) in cases {
        let names: Vec<&str> = pairs.iter().map(|(k, _)| *k).collect();
        let vis = cols(&names);
        let mut cur = widths(pairs);
        let mut prev = cur.clone();
        let mut steps = 0;
        while let Some(next) = plan_clamp(&cur, &vis, &prev, *vp) {
            prev = cur;
            cur = next;
            steps += 1;
            assert!(steps < 32, "clamp must converge, not loop: {names:?} vp={vp}");
        }
    }
}

/// A single-column drag takes the overflow off the dragged column only; its neighbours
/// keep their widths.
#[test]
fn single_drag_spares_neighbours() {
    let prev = widths(&[("a", 100.0), ("b", 100.0), ("c", 100.0)]);
    let cur = widths(&[("a", 400.0), ("b", 100.0), ("c", 100.0)]);
    let vis = cols(&["a", "b", "c"]);
    let next = plan_clamp(&cur, &vis, &prev, 450.0).expect("overflow should be clamped");
    assert_eq!(next["b"], 100.0, "neighbour b must be untouched");
    assert_eq!(next["c"], 100.0, "neighbour c must be untouched");
    assert!(next["a"] < 400.0, "dragged column absorbs the overflow");
    assert!(sum_visible(&next, &vis) <= 450.5);
}
