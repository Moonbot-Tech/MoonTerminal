//! Updates opener routing and one-shot request regressions.

use super::{Route, RouteState, apply_updates_request, route, updates_row_for_core};
use crate::panels::core_status::CoreStatusMode;

/// Existing docked and detached surfaces must be reused instead of spawning a second panel.
#[test]
fn existing_core_status_chooses_focus_or_raise_not_open() {
    assert_eq!(
        route(RouteState {
            auto: false,
            docked: true,
            detached: false,
        }),
        Route::Focus
    );
    assert_eq!(
        route(RouteState {
            auto: false,
            docked: false,
            detached: true,
        }),
        Route::Raise
    );
}

/// Auto must use its group-owned dock even while a Classic detached spec is suspended.
#[test]
fn auto_workspace_never_forces_a_detached_window() {
    assert_eq!(
        route(RouteState {
            auto: true,
            docked: false,
            detached: true,
        }),
        Route::Focus
    );
}

/// Only a genuinely absent Classic panel may be created.
#[test]
fn absent_classic_panel_uses_open_route() {
    assert_eq!(
        route(RouteState {
            auto: false,
            docked: false,
            detached: false,
        }),
        Route::Open
    );
}

/// A request must switch to Updates once and leave nothing for a later panel or render to replay.
#[test]
fn updates_request_is_drained_once_and_selects_updates_mode() {
    let mut request = Some(7);
    let mut mode = CoreStatusMode::Flat;
    assert_eq!(
        apply_updates_request(&mut request, &mut mode, true),
        Some(7)
    );
    assert_eq!(mode, CoreStatusMode::Updates);
    assert_eq!(request, None);

    assert_eq!(apply_updates_request(&mut request, &mut mode, true), None);
    assert_eq!(mode, CoreStatusMode::Updates);
}

/// A panel in another group must leave the request for its owner.
#[test]
fn non_owner_does_not_drain_updates_request() {
    let mut request = Some(7);
    let mut mode = CoreStatusMode::Flat;
    assert_eq!(apply_updates_request(&mut request, &mut mode, false), None);
    assert_eq!(request, Some(7));
    assert_eq!(mode, CoreStatusMode::Flat);
}

/// The selected row must follow the table's live-first order, not the core's configuration order.
#[test]
fn updates_row_lookup_uses_live_then_history_positions() {
    assert_eq!(updates_row_for_core([2, 7], [7, 9], 7), Some(1));
    assert_eq!(updates_row_for_core([2, 7], [7, 9], 9), Some(3));
    assert_eq!(updates_row_for_core([2, 7], [7, 9], 4), None);
}
