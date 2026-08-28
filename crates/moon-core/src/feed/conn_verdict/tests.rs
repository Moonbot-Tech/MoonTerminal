//! Unit tests for [`super::diagnose`]. Owned by the breakage gate, which authors every
//! deterministic test in this repository.

use super::*;
use crate::feed::{ConnStatus, CoreIdentityFacts, INIT_STEPS_TOTAL};

/// A transport timeout must distinguish no return path from packets that reached the process but
/// were rejected above the UDP socket.
///
/// Breakage: classifying only by accepted Sliced bytes makes both cases read "not one byte" even
/// when MoonProto counted physical inbound datagrams before protocol validation.
#[test]
fn connect_timeout_retains_physical_udp_evidence_from_both_sockets() {
    let fault = ConnFault {
        kind: ConnFaultKind::ConnectTimedOut { timeout_ms: 15_000 },
        identity: CoreIdentityFacts::default(),
        startup: CoreStartupStatus {
            current_port_sent_packets: 9,
            current_port_received_packets: 2,
            sent_packets_before_last_port_change: 7,
            received_packets_before_last_port_change: 3,
            received_sliced_bytes: 0,
            ..Default::default()
        },
    };

    let diagnosis = diagnose(
        &ConnStatus::Failed("connect timeout".to_string()),
        Some(&fault),
        &CoreStartupStatus::default(),
    )
    .expect("a retained connect timeout must have a diagnosis");

    assert_eq!(
        diagnosis.class,
        FailureClass::NoResponse {
            packets_sent: 16,
            packets_received: 5,
            bytes: 0,
            elapsed_ms: 15_000,
        }
    );
}

/// `conn_verdict.rs:diagnose` must require a populated identity field before setting
/// `legacy_core`; relaxing its guard to a missing MoonProto version alone would falsely tell a
/// current core that failed authorization to update MoonBot instead of fixing its access setup.
#[test]
fn an_unpublished_identity_never_claims_the_core_is_legacy() {
    let fault = ConnFault {
        kind: ConnFaultKind::NotAuthenticated,
        identity: CoreIdentityFacts::default(),
        startup: CoreStartupStatus::default(),
    };

    let diagnosis = diagnose(
        &ConnStatus::Failed("authorization failed".to_string()),
        Some(&fault),
        &CoreStartupStatus::default(),
    )
    .expect("a retained authorization fault must have a diagnosis");

    assert_eq!(
        diagnosis.class,
        FailureClass::Access {
            refused: true,
            message: None,
        }
    );
    assert!(
        !diagnosis.legacy_core,
        "an all-empty identity is no evidence that this core predates the terminal"
    );
}

/// `conn_verdict.rs:step_class` must keep a recognized post-authorization failure in the syncing
/// row; moving its catch-all row above the access row would misdirect a stalled market sync toward
/// key or Kernel(VPS) troubleshooting.
#[test]
fn a_recognized_late_init_failure_is_a_stalled_sync_not_an_access_failure() {
    let fault = ConnFault {
        kind: ConnFaultKind::InitStepTimedOut {
            step: Some(CoreInitStep::GetMarketsList),
            raw_step: "GetMarketsList".to_string(),
        },
        identity: CoreIdentityFacts::default(),
        startup: CoreStartupStatus {
            completed_mask: 0b11,
            elapsed_ms: 12_345,
            ..CoreStartupStatus::default()
        },
    };

    let diagnosis = diagnose(
        &ConnStatus::Failed("initialization timed out".to_string()),
        Some(&fault),
        &CoreStartupStatus::default(),
    )
    .expect("a retained init failure must have a diagnosis");

    assert!(
        matches!(
            diagnosis.class,
            FailureClass::Syncing {
                step: Some(CoreInitStep::GetMarketsList),
                stalled: true,
                ..
            }
        ),
        "a failure after authorization must retain its late init stage rather than claim access failed"
    );
}

/// `conn_verdict.rs:diagnose` must keep a startup this terminal GAVE UP on out of `step_class`.
///
/// Breakage: routing it through `ConnFaultKind::InitStepTimedOut` instead reads the step as
/// evidence about the core — `BaseCheck` becomes "wrong core build or key", `AuthCheck` becomes
/// "Kernel(VPS) is off". Nothing answered during a stall, so the step is a location and those are
/// confident wrong causes on the two likeliest stall steps.
#[test]
fn a_startup_the_terminal_gave_up_on_is_a_stalled_sync_not_a_core_verdict() {
    for step in [CoreInitStep::BaseCheck, CoreInitStep::AuthCheck] {
        let fault = ConnFault {
            kind: ConnFaultKind::StartupStalled,
            identity: CoreIdentityFacts::default(),
            startup: CoreStartupStatus {
                current_step: Some(step),
                completed_mask: 0b1,
                elapsed_ms: 300_000,
                ..CoreStartupStatus::default()
            },
        };

        let diagnosis = diagnose(
            &ConnStatus::Failed("startup stalled".to_string()),
            Some(&fault),
            &CoreStartupStatus::default(),
        )
        .expect("a retained stall fault must have a diagnosis");

        assert_eq!(
            diagnosis.class,
            FailureClass::Syncing {
                step: Some(step),
                done: 1,
                total: INIT_STEPS_TOTAL,
                elapsed_ms: 300_000,
                stalled: true,
            },
            "{step:?} stalled must read as a stalled sync"
        );
    }
}
