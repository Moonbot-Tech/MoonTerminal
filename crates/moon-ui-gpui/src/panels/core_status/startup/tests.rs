//! Startup-column presentation: cell decisions, the progress denominator, and the hover facts.

use super::*;
use moon_core::session::INIT_STEPS_TOTAL;

/// A minimal terminal snapshot, everything else default.
fn status(state: CoreStartupState) -> CoreStartupStatus {
    CoreStartupStatus {
        state,
        ..Default::default()
    }
}

/// `startup_cell`: a `Ready` snapshot always yields `Done`, never `Progress` — the frozen figure
/// must never read as live once the core has settled.
#[test]
fn a_ready_snapshot_never_reads_as_progress() {
    let cell = startup_cell(&ConnStatus::Ready, &status(CoreStartupState::Ready));
    assert!(matches!(cell, StartupCell::Done { .. }));
}

/// `startup_cell`: a dead CONNECTION yields `Absent` even when the retained snapshot still says
/// `Connecting` — the status column and this column must not contradict each other on one row.
#[test]
fn a_disconnected_connection_hides_a_stale_connecting_snapshot() {
    let cell = startup_cell(
        &ConnStatus::Disconnected,
        &status(CoreStartupState::Connecting),
    );
    assert_eq!(cell, StartupCell::Absent);

    let cell = startup_cell(
        &ConnStatus::Failed("boom".to_string()),
        &status(CoreStartupState::Initializing),
    );
    assert_eq!(cell, StartupCell::Absent);
}

/// `startup_cell`: `Connecting` with zero completed steps still reports `Progress{0, 8}`, not
/// `Absent` — the core is genuinely starting and the column must say so.
#[test]
fn connecting_with_no_completed_steps_still_shows_progress() {
    let cell = startup_cell(
        &ConnStatus::Stage("connecting…".to_string()),
        &status(CoreStartupState::Connecting),
    );
    assert_eq!(
        cell,
        StartupCell::Progress {
            done: 0,
            total: INIT_STEPS_TOTAL,
            elapsed_ms: 0,
        }
    );
}

/// `progress_pair`: the denominator is clamped UP, never left showing the observed count over a
/// stale constant.
#[test]
fn the_denominator_clamps_up_to_the_observed_completed_count() {
    let s = CoreStartupStatus {
        state: CoreStartupState::Initializing,
        completed_mask: 0b1_1111_1111, // 9 bits set
        ..Default::default()
    };
    assert_eq!(s.progress_pair(), (9, 9));
}

/// `progress_pair`: a normal in-progress count stays under the fixed total.
#[test]
fn a_normal_completed_count_stays_under_the_fixed_total() {
    let s = CoreStartupStatus {
        state: CoreStartupState::Initializing,
        completed_mask: 0b0000_0111, // 3 bits set
        ..Default::default()
    };
    assert_eq!(s.progress_pair(), (3, INIT_STEPS_TOTAL));
}

/// `startup_facts`: an upstream `None` (e.g. `path_mtu_bytes`) contributes NO line, and the
/// tooltip's line count equals the fact count — the two can never disagree about what is shown.
#[test]
fn an_absent_upstream_value_contributes_no_fact_line() {
    let s = CoreStartupStatus {
        state: CoreStartupState::Connecting,
        round_trip_ms: None,
        path_mtu_bytes: None,
        downlink_delivery_percent: None,
        ..Default::default()
    };
    let facts = startup_facts(&s);
    assert!(!facts.iter().any(|f| f.label.contains("MTU")));
    assert!(
        facts
            .iter()
            .all(|f| !f.value.is_empty() || !f.label.is_empty())
    );

    let tooltip = startup_tooltip(&facts);
    assert_eq!(tooltip.lines().count(), facts.len());
}

/// `startup_facts`/`startup_tooltip`: supplying the optional values adds exactly one line each, and
/// the tooltip still carries one line per fact.
#[test]
fn present_upstream_values_each_add_exactly_one_fact_line() {
    let without = CoreStartupStatus {
        state: CoreStartupState::Connecting,
        round_trip_ms: None,
        path_mtu_bytes: None,
        downlink_delivery_percent: None,
        ..Default::default()
    };
    let with = CoreStartupStatus {
        round_trip_ms: Some(42),
        path_mtu_bytes: Some(1200),
        downlink_delivery_percent: Some(97),
        ..without
    };

    let facts_without = startup_facts(&without);
    let facts_with = startup_facts(&with);
    assert_eq!(facts_with.len(), facts_without.len() + 3);
    assert_eq!(
        facts_with
            .iter()
            .filter(|f| f.label == t!("core_status.startup.f.received"))
            .count(),
        1
    );

    let tooltip = startup_tooltip(&facts_with);
    assert_eq!(tooltip.lines().count(), facts_with.len());
}

/// Local-port facts are omitted until a socket exists, then current, previous, and change-count
/// inputs add exactly one line each without taking over the Sliced-byte `Received` label.
#[test]
fn local_udp_ports_add_three_distinct_optional_fact_lines() {
    let without = CoreStartupStatus {
        state: CoreStartupState::Connecting,
        ..Default::default()
    };
    let with = CoreStartupStatus {
        current_local_udp_port: Some(31_002),
        current_port_sent_packets: 17,
        current_port_received_packets: 23,
        previous_local_udp_port: Some(31_001),
        sent_packets_before_last_port_change: 11,
        received_packets_before_last_port_change: 13,
        local_port_change_count: 2,
        ..without
    };

    let facts_without = startup_facts(&without);
    let facts_with = startup_facts(&with);
    assert_eq!(facts_with.len(), facts_without.len() + 3);
    let current = facts_with
        .iter()
        .find(|f| f.label == t!("core_status.startup.f.local_port"))
        .expect("the current socket must have its own fact");
    assert_eq!(
        current.value,
        format!(
            "31002 · {}",
            t!("core_status.startup.packets", sent = 17, recv = 23)
        )
    );
    let previous = facts_with
        .iter()
        .find(|f| f.label == t!("core_status.startup.f.previous_port"))
        .expect("the previous socket must have its own fact");
    assert_eq!(
        previous.value,
        format!(
            "31001 · {}",
            t!("core_status.startup.packets", sent = 11, recv = 13)
        )
    );
}

/// Problem surfaces must retain both the actionable verdict and its own failed-attempt socket facts.
///
/// Breakage: pairing a retained fault with the live retry snapshot makes the reason describe one
/// socket while the counters beneath it silently describe another.
#[test]
fn problem_diagnostic_keeps_verdict_and_socket_facts_on_the_failed_attempt() {
    let failed = CoreStartupStatus {
        current_local_udp_port: Some(31_002),
        current_port_sent_packets: 17,
        current_port_received_packets: 23,
        ..Default::default()
    };
    let fault = moon_core::feed::ConnFault {
        kind: moon_core::feed::ConnFaultKind::ConnectTimedOut { timeout_ms: 15_000 },
        identity: Default::default(),
        startup: failed,
    };
    let live_retry = CoreStartupStatus {
        current_local_udp_port: Some(32_004),
        current_port_sent_packets: 1,
        ..Default::default()
    };
    let diagnosis = moon_core::feed::diagnose(&ConnStatus::Connecting, Some(&fault), &live_retry)
        .expect("the retained timeout must remain diagnosable during its retry");

    let text = problem_diagnostic_text(&diagnosis, Some(&fault), &live_retry, None);
    assert!(text.contains("31002"));
    assert!(text.contains("17"));
    assert!(text.contains("23"));
    assert!(!text.contains("32004"));
    assert!(text.lines().count() > startup_facts(&failed).len());
}
