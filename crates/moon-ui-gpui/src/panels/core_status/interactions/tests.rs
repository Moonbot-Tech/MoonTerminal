//! Pure health-chip, alarm, and Updates-scope transitions.

use moon_core::feed::ConnStatus;
use moon_core::session::{CoreStartupStatus, CoreSysStatus};

use super::{behind_in_scope, health_filter_click, open_alarms_state, updates_scope_ids};
use crate::backend::core_warn::LatencySeverity;
use crate::panels::core_status::health::HealthFilter;
use crate::panels::core_status::mode::CoreStatusMode;
use crate::panels::core_status::model::{ApiKeyState, CoreStatusRow};

/// The same chip clears itself. Only Flat stays. Problems moves to Flat, the same as Warnings,
/// because a chip left on Problems would look selected and filter nothing.
#[test]
fn the_same_health_chip_toggles_off_and_other_modes_move_to_flat() {
    let off = health_filter_click(
        CoreStatusMode::Flat,
        Some(HealthFilter::Ping),
        HealthFilter::Ping,
    );
    assert_eq!(off.mode, CoreStatusMode::Flat);
    assert_eq!(off.filter, None);

    let from_problems = health_filter_click(CoreStatusMode::Problems, None, HealthFilter::Offline);
    assert_eq!(from_problems.mode, CoreStatusMode::Flat);
    assert_eq!(from_problems.filter, Some(HealthFilter::Offline));

    let moved = health_filter_click(CoreStatusMode::Warnings, None, HealthFilter::Ping);
    assert_eq!(moved.mode, CoreStatusMode::Flat);
    assert_eq!(moved.filter, Some(HealthFilter::Ping));
}

/// The alarms control selects Warnings and carries no health chip.
#[test]
fn open_alarms_selects_warnings_and_clears_the_filter() {
    let (mode, filter) = open_alarms_state();
    assert_eq!(mode, CoreStatusMode::Warnings);
    assert_eq!(filter, None);
}

fn scoped_row(id: u64, behind: Option<u32>) -> CoreStatusRow {
    CoreStatusRow {
        fault: None,
        mode_suggestion: None,
        id,
        name: format!("c{id}"),
        status: ConnStatus::Ready,
        sys: CoreSysStatus::default(),
        endpoint: None,
        ping_warn: false,
        ping_sev: LatencySeverity::Normal,
        exch_sev: LatencySeverity::Normal,
        api_key: ApiKeyState::Unknown,
        api_warn: false,
        api_notice: false,
        api_quota: None,
        api_quota_warn: false,
        startup: CoreStartupStatus::default(),
        time_offset: moon_core::feed::CoreTimeOffsetStatus::default(),
        server_version: behind.map(|newest| newest.saturating_sub(1)),
        server_version_suffix: None,
        version_behind: behind,
        update: None,
    }
}

/// Updates commands every scoped core. The behind count is the cores that reported a lag.
#[test]
fn updates_scope_is_every_scoped_core_and_behind_counts_lagging() {
    let rows = vec![
        scoped_row(1, Some(800)),
        scoped_row(2, None),
        scoped_row(3, Some(800)),
    ];
    assert_eq!(
        updates_scope_ids(CoreStatusMode::Updates, &rows).as_deref(),
        Some(&[1, 2, 3][..])
    );
    assert_eq!(updates_scope_ids(CoreStatusMode::Flat, &rows), None);
    assert_eq!(updates_scope_ids(CoreStatusMode::Servers, &rows), None);
    assert_eq!(behind_in_scope(&rows), 2);
    assert_eq!(behind_in_scope(&[]), 0);
}
