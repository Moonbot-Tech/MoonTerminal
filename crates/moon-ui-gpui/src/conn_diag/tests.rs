//! Unit tests for the connection-verdict wording. Owned by the breakage gate, which authors
//! every deterministic test in this repository.

use super::*;
use moon_core::config::{FeedFlags, Secret};

/// Build a server whose stored mode is the effective mode under this test's explicit fixtures.
fn server(id: CoreId, mode: Option<TransportVersion>) -> ServerConfig {
    ServerConfig {
        id,
        uid: id,
        name: format!("core-{id}"),
        active: true,
        show_window: true,
        feed: FeedFlags::default(),
        key: Secret::new(""),
        group: "fleet".to_string(),
        market: "BTCUSDT".to_string(),
        color: [0, 0, 0],
        synthetic: false,
        chart_bundle: String::new(),
        default_alert_strategy: 0,
        own_trade_config: false,
        strat_slots: None,
        manual_strategy: None,
        trade: None,
        transport: mode,
        workspace_membership: moon_core::config::WorkspaceMembership::default(),
    }
}

/// Physical inbound datagrams must not reuse either the silence sentence or its firewall advice.
///
/// Breakage: keying the verdict only off accepted Sliced bytes tells an operator to open a port
/// after the socket has already proved that packets reached the terminal.
#[test]
fn unparsed_datagrams_do_not_use_the_silent_wording() {
    let silent = FailureClass::NoResponse {
        packets_sent: 9,
        packets_received: 0,
        bytes: 0,
        elapsed_ms: 12_000,
    };
    let unparsed = FailureClass::NoResponse {
        packets_sent: 9,
        packets_received: 73,
        bytes: 0,
        elapsed_ms: 12_000,
    };

    assert_ne!(reason(&silent), reason(&unparsed));
    assert_ne!(next_step(&silent, None), next_step(&unparsed, None));
    assert_ne!(fault_short(&silent), fault_short(&unparsed));
    assert!(reason(&unparsed).contains("73"));
}

/// `conn_diag.rs:fleet_mode_suggestion` must compare a core with other configured cores only.
///
/// Breakage: including the failing core in its own sibling evidence would make a one-core fleet fabricate a transport recommendation from the very failure it is meant to explain.
#[test]
fn fleet_mode_suggestion_excludes_the_failing_core_and_uses_ready_siblings() {
    let alone = [server(1, Some(TransportVersion::V0))];
    assert_eq!(fleet_mode_suggestion(1, &alone, |_| true), None);
    let fleet = [
        server(1, Some(TransportVersion::V0)),
        server(2, Some(TransportVersion::V1)),
    ];
    assert_eq!(
        fleet_mode_suggestion(1, &fleet, |id| id == 2),
        Some(TransportVersion::V1)
    );
}

/// `conn_diag.rs:fleet_mode_suggestion` must not infer a mode when the failing core has none.
///
/// Breakage: using only sibling modes when this core's mode is unreadable would recommend a setting without knowing whether it differs from the failing configuration.
#[test]
fn fleet_mode_suggestion_requires_the_failing_cores_effective_mode() {
    let fleet = [server(1, None), server(2, Some(TransportVersion::V1))];
    assert_eq!(fleet_mode_suggestion(1, &fleet, |id| id == 2), None);
}

/// `conn_diag.rs:next_step` must append mode advice only to no-response verdicts.
///
/// Breakage: widening its `(FailureClass::NoResponse, Some(_))` match to every class tells users to change transport mode after a handshake that already proved transport worked.
#[test]
fn only_no_response_verdicts_receive_a_mode_suggestion() {
    let silent = FailureClass::NoResponse {
        packets_sent: 3,
        packets_received: 0,
        bytes: 0,
        elapsed_ms: 1_000,
    };
    let unparsed = FailureClass::NoResponse {
        packets_sent: 3,
        packets_received: 2,
        bytes: 0,
        elapsed_ms: 1_000,
    };
    for no_response in [&silent, &unparsed] {
        assert_ne!(
            next_step(no_response, Some(TransportVersion::V1)),
            next_step(no_response, None),
            "both packet-count no-response variants must surface the proven V1 alternative"
        );
    }
    let other_classes = [
        FailureClass::LocalPort { attempts: 1 },
        FailureClass::Access {
            refused: true,
            message: None,
        },
        FailureClass::CoreUnidentified { message: None },
        FailureClass::Syncing {
            step: None,
            done: 1,
            total: 8,
            elapsed_ms: 1_000,
            stalled: false,
        },
        FailureClass::Aborted,
        FailureClass::Undetermined {
            raw_stage: "unknown".to_string(),
        },
    ];
    for class in &other_classes {
        assert_eq!(
            next_step(class, Some(TransportVersion::V1)),
            next_step(class, None)
        );
    }
}
