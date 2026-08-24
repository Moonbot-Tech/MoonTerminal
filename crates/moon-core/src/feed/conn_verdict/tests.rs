//! Unit tests for [`super::diagnose`]. Owned by the breakage gate, which authors every
//! deterministic test in this repository.

use super::*;
use crate::feed::{ConnStatus, CoreIdentityFacts};

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
