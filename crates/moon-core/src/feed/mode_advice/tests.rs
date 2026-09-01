//! Regression coverage for fleet-wide transport-mode advice.

use super::{SiblingOutcome, suggest_alternate_mode};
use crate::config::TransportVersion;

/// `mode_advice.rs:suggest_alternate_mode` must not infer a mode without any sibling evidence.
///
/// Breakage: returning an arbitrary mode for an empty fleet would tell an operator to change a
/// working core's transport setting without evidence.
#[test]
fn no_siblings_offer_no_alternate_mode() {
    assert_eq!(suggest_alternate_mode(TransportVersion::V0, []), None);
}

/// `mode_advice.rs:suggest_alternate_mode` must ignore siblings that are not connected.
///
/// Breakage: treating another failed core's configured mode as evidence would recommend a change
/// even though no core has proved that mode works.
#[test]
fn disconnected_siblings_offer_no_alternate_mode() {
    assert_eq!(
        suggest_alternate_mode(
            TransportVersion::V0,
            [SiblingOutcome {
                mode: TransportVersion::V1,
                connected: false,
            }],
        ),
        None
    );
}

/// `mode_advice.rs:suggest_alternate_mode` must reject a connected sibling on the failing mode.
///
/// Breakage: removing the same-mode guard would recommend V1 even though a connected V0 sibling
/// proves that changing transport mode is not the explanation for this core's failure.
#[test]
fn a_connected_same_mode_sibling_disproves_the_suggestion_regardless_of_order() {
    assert_eq!(
        suggest_alternate_mode(
            TransportVersion::V0,
            [SiblingOutcome {
                mode: TransportVersion::V0,
                connected: true
            }],
        ),
        None
    );
    let siblings = [
        SiblingOutcome {
            mode: TransportVersion::V1,
            connected: true,
        },
        SiblingOutcome {
            mode: TransportVersion::V0,
            connected: true,
        },
    ];
    assert_eq!(suggest_alternate_mode(TransportVersion::V0, siblings), None);
    assert_eq!(
        suggest_alternate_mode(TransportVersion::V0, siblings.into_iter().rev()),
        None
    );
}

/// `mode_advice.rs:suggest_alternate_mode` must name the one other proven transport mode.
///
/// Breakage: dropping the unanimous candidate would hide the only fleet evidence that can help an
/// operator repair a failing core's transport setting.
#[test]
fn unanimous_connected_other_mode_is_suggested() {
    assert_eq!(
        suggest_alternate_mode(
            TransportVersion::V0,
            [
                SiblingOutcome {
                    mode: TransportVersion::V1,
                    connected: true
                },
                SiblingOutcome {
                    mode: TransportVersion::V1,
                    connected: true
                }
            ]
        ),
        Some(TransportVersion::V1)
    );
}

/// `mode_advice.rs:suggest_alternate_mode` must reject connected siblings that disagree.
///
/// Breakage: retaining the first candidate through disagreement would present one unsupported
/// mode as a fix when the fleet has not established a single working alternative.
#[test]
fn split_connected_other_modes_offer_no_alternate_mode() {
    assert_eq!(
        suggest_alternate_mode(
            TransportVersion::V0,
            [
                SiblingOutcome {
                    mode: TransportVersion::V1,
                    connected: true
                },
                SiblingOutcome {
                    mode: TransportVersion::V2,
                    connected: true
                }
            ]
        ),
        None
    );
}
