//! Regression coverage for panel clock pacing and hidden-tab catch-up.

use super::{GateStep, RenderGate};
use std::time::{Duration, Instant};

/// Admit a subsequent sample without sleeping or weakening the production floor.
fn elapse_floor(gate: &mut RenderGate) {
    gate.last_notify_at = Some(Instant::now() - Duration::from_millis(251));
}

/// A clock-only tick must not request row collection.
#[test]
fn second_rollover_repaints_without_rebuild() {
    let mut gate = RenderGate::default();
    assert_eq!(gate.step(7, 1000.0), GateStep::Rebuild);
    elapse_floor(&mut gate);
    assert_eq!(gate.step(7, 2000.0), GateStep::Repaint);
}

/// Data changes must still rebuild even within the same wall-clock second.
#[test]
fn sig_move_rebuilds() {
    let mut gate = RenderGate::default();
    gate.step(7, 1000.0);
    elapse_floor(&mut gate);
    assert_eq!(gate.step(8, 1100.0), GateStep::Rebuild);
}

/// Rejected samples must remain eligible after the shared floor expires.
#[test]
fn floor_suppresses_both() {
    let mut gate = RenderGate::default();
    gate.step(7, 1000.0);
    assert_eq!(gate.step(7, 2000.0), GateStep::Idle);
    assert_eq!(gate.step(8, 2000.0), GateStep::Idle);
    elapse_floor(&mut gate);
    assert_eq!(gate.step(8, 2000.0), GateStep::Rebuild);
    assert_eq!(gate.step(8, 3000.0), GateStep::Idle);
    elapse_floor(&mut gate);
    assert_eq!(gate.step(8, 3000.0), GateStep::Repaint);
}

/// Hidden samples coalesce and cannot cause a second activation catch-up.
#[test]
fn inactive_gate_defers_then_catches_up_once() {
    let mut gate = RenderGate::default();
    assert!(!gate.set_active(false));
    assert_eq!(gate.step(7, 1000.0), GateStep::Idle);
    elapse_floor(&mut gate);
    assert_eq!(gate.step(8, 2000.0), GateStep::Idle);
    assert!(gate.set_active(true));
    assert!(!gate.set_active(true));
    elapse_floor(&mut gate);
    assert_eq!(gate.step(8, 2000.0), GateStep::Idle);
    gate.set_active(false);
    assert!(!gate.set_active(true));
}

/// Detached panels lacking activation events must keep receiving updates.
#[test]
fn default_gate_is_active() {
    let mut gate = RenderGate::default();
    assert_eq!(gate.step(1, 1000.0), GateStep::Rebuild);
}

/// Merely switching an unchanged tab must not collect rows.
#[test]
fn activation_without_pending_is_noop() {
    let mut gate = RenderGate::default();
    assert!(!gate.set_active(true));
    assert!(!gate.set_active(false));
    assert!(!gate.set_active(true));
}

/// The Orders compatibility wrapper must keep the same admission decisions.
#[test]
fn should_notify_matches_step() {
    let mut step = RenderGate::default();
    let mut notify = RenderGate::default();
    for (sig, ms) in [(1, 1000.0), (1, 1000.0), (1, 2000.0), (2, 2100.0)] {
        elapse_floor(&mut step);
        elapse_floor(&mut notify);
        assert_eq!(
            notify.should_notify(sig, ms),
            step.step(sig, ms) != GateStep::Idle
        );
    }
    step.set_active(false);
    notify.set_active(false);
    elapse_floor(&mut step);
    elapse_floor(&mut notify);
    assert_eq!(
        notify.should_notify(3, 3000.0),
        step.step(3, 3000.0) != GateStep::Idle
    );
}
