use super::*;
use crate::figures::kind::FigureKind;
use crate::figures::tools::tests::{build, ctx, TestProj};

/// A long: opened at 100, taken at 120, and — by default — cut at 90.
fn long() -> Position {
    Position::placed(
        FigNode::new(TestProj::T0_MS, 100.0),
        FigNode::new(TestProj::T0_MS + 10_000.0, 120.0),
    )
}

/// Two clicks give three prices: the stop is PLACED, not left on the entry.
///
/// A stop on the entry would draw a zero-height risk zone, read as a trade that cannot lose, and
/// make the ratio infinite — so the tool opens at two to one and the stop is dragged from there.
#[test]
fn two_clicks_place_a_stop_at_two_to_one() {
    let p = long();
    assert_eq!((p.entry, p.target), (100.0, 120.0));
    assert_eq!(p.stop, 90.0);
    assert_eq!(p.risk_reward(), Some(2.0));
    assert_eq!(
        (p.t0_ms, p.t1_ms),
        (TestProj::T0_MS, TestProj::T0_MS + 10_000.0)
    );
}

/// The same two clicks the other way round are a SHORT, and the stop lands above the entry.
///
/// Nothing switches modes: the direction is read from the geometry, so one tool draws both.
#[test]
fn a_target_below_the_entry_is_a_short() {
    let p = Position::placed(
        FigNode::new(TestProj::T0_MS, 100.0),
        FigNode::new(TestProj::T0_MS + 10_000.0, 80.0),
    );
    assert!(!p.is_long());
    assert_eq!(p.stop, 110.0, "a short is cut ABOVE its entry");
    assert_eq!(p.risk_reward(), Some(2.0));
}

/// Dragging the target through the entry flips the position. The zones follow, because which one
/// is profit is not stored anywhere to go stale.
#[test]
fn dragging_the_target_through_the_entry_flips_the_position() {
    let mut p = long();
    assert!(p.is_long());
    assert!(p.move_handle(1, FigNode::new(p.t1_ms, 70.0)));
    assert!(!p.is_long(), "the target moved below the entry");
}

/// Three handles, and each owns exactly what it should: the entry carries the box's start in time,
/// the target its end, and the stop only a price — two handles sharing the right edge must not
/// both drag it, or the width would jump depending on which was grabbed.
#[test]
fn the_stop_handle_moves_no_edge_in_time() {
    let mut p = long();
    assert_eq!(p.handle_count(), 3);
    assert_eq!(p.handle(3), None);
    let t_before = p.t1_ms;
    assert!(p.move_handle(2, FigNode::new(t_before + 99_000.0, 95.0)));
    assert_eq!(p.stop, 95.0);
    assert_eq!(p.t1_ms, t_before, "the stop must not stretch the box");
    assert!(
        !p.move_handle(2, FigNode::new(0.0, 95.0)),
        "no change reported"
    );
}

/// A stop dragged onto the entry has no ratio to state. It must answer `None` rather than divide
/// by zero and label the chart with infinity.
#[test]
fn a_stop_on_the_entry_has_no_ratio() {
    let mut p = long();
    assert!(p.move_handle(2, FigNode::new(p.t1_ms, p.entry)));
    assert_eq!(p.risk_reward(), None);
    let rec = build(&FigureKind::Position(p), ctx(true, false));
    assert_eq!(
        rec.labels.len(),
        2,
        "the two exits are still labelled; the ratio is not"
    );
}

/// The figure is two zones and three lines, in that order — the lines are drawn over their own
/// fill, and the profit zone carries the tool's green rather than the style's colour.
#[test]
fn it_draws_two_zones_and_three_lines() {
    let rec = build(&FigureKind::Position(long()), ctx(false, false));
    assert_eq!(rec.bands.len(), 2);
    assert_eq!(rec.bands[0], (long().t0_ms, long().t1_ms, 100.0, 120.0));
    assert_eq!(rec.bands[1], (long().t0_ms, long().t1_ms, 100.0, 90.0));
    let green = rec.band_colors[0];
    let red = rec.band_colors[1];
    assert!(
        green[1] > green[0] && red[0] > red[1],
        "profit must be the greener fill and risk the redder one, got {green:?} and {red:?}"
    );
    assert_eq!(rec.segs.len(), 3, "entry, target and stop");
}

/// Hovering states the three numbers the box exists for: how far each exit is, and what the trade
/// pays for what it stakes.
#[test]
fn hovering_states_both_distances_and_the_ratio() {
    let rec = build(&FigureKind::Position(long()), ctx(true, false));
    assert_eq!(rec.labels.len(), 3);
    let texts: Vec<_> = rec.labels.iter().map(|(_, _, t)| *t).collect();
    assert!(texts.contains(&crate::figures::LabelText::PctDelta {
        from: 100.0,
        to: 120.0
    }));
    assert!(texts.contains(&crate::figures::LabelText::PctDelta {
        from: 100.0,
        to: 90.0
    }));
    assert!(texts.contains(&crate::figures::LabelText::RiskReward(2.0)));
}

/// A body drag moves the whole plan, keeping every distance and the ratio intact.
#[test]
fn a_body_drag_keeps_the_ratio() {
    let mut p = long();
    assert!(p.translate(1_000.0, 5.0));
    assert_eq!((p.entry, p.target, p.stop), (105.0, 125.0, 95.0));
    assert_eq!(p.risk_reward(), Some(2.0));
    assert!(!p.translate(0.0, 0.0));
}

/// The core has no chart-object type for a position, so it can never be armed.
#[test]
fn a_position_is_not_alertable() {
    assert!(!FigureTool::Position.def().alertable);
    assert!(crate::alert_blob::encode(
        &FigureKind::Position(long()),
        [1, 2, 3, 4],
        1.0,
        crate::figures::LineKind::Solid,
        0.0,
        0,
        1,
    )
    .is_none());
}
