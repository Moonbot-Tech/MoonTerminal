//! Pick toggling of the Summary view state.

use super::{CoreLines, SummaryUi};

/// A table click that does not switch the chart to `Selected` leaves the picked line invisible
/// under the default top-5 mode, so the click looks broken.
#[test]
fn picking_a_core_switches_the_chart_to_selected() {
    let mut ui = SummaryUi::default();
    assert_eq!(ui.lines, CoreLines::Top5);
    ui.toggle_pick(7);
    assert_eq!(ui.lines, CoreLines::Selected);
    assert!(ui.picked.contains(&7));
}

/// A second click must remove the core again; the mode stays `Selected` so the chart does not
/// jump back to five unrelated lines.
#[test]
fn a_double_toggle_removes_the_pick_and_keeps_the_mode() {
    let mut ui = SummaryUi::default();
    ui.toggle_pick(7);
    ui.toggle_pick(7);
    assert!(ui.picked.is_empty());
    assert_eq!(ui.lines, CoreLines::Selected);
}

/// Picks of cores a fresh snapshot lacks are dropped; picks it still has stay. Keeping a stale
/// pick leaves Selected mode drawing nothing while the hint believes something is picked.
#[test]
fn retain_picks_drops_cores_the_snapshot_lacks() {
    let mut ui = SummaryUi::default();
    ui.toggle_pick(7);
    ui.toggle_pick(9);
    ui.retain_picks(&[9, 11].into_iter().collect());
    assert_eq!(ui.picked.iter().copied().collect::<Vec<_>>(), [9]);
}
