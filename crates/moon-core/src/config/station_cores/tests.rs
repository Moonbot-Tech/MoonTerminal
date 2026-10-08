//! Synthetic selections protect partial installation without needing credentials or a station.

use super::select_station_cores;
use crate::config::{CoreKeyEntry, Secret};

/// Build a synthetic stored credential with independently chosen eligibility inputs.
fn core(uid: u64, name: &str, active: bool, key: &str) -> CoreKeyEntry {
    CoreKeyEntry {
        uid,
        name: name.into(),
        active,
        key: Secret::new(key),
        endpoint_override: String::new(),
        transport: None,
    }
}

/// Rejecting one empty key must not prevent another enabled core from reaching the station.
#[test]
fn enabled_keyed_cores_survive_and_keyless_cores_are_named() {
    let selected = select_station_cores(vec![
        core(3, "Ready", true, "synthetic-key"),
        core(5, "Needs key", true, ""),
        core(7, "Also ready", true, "synthetic-second-key"),
        core(9, "Also needs key", true, ""),
    ]);
    assert_eq!(
        selected
            .keyed
            .iter()
            .map(|core| core.uid)
            .collect::<Vec<_>>(),
        [3, 7]
    );
    assert_eq!(selected.skipped, ["Needs key", "Also needs key"]);
}

/// Sending disabled credentials or naming disabled rows would ignore the user's active switch.
#[test]
fn disabled_cores_are_neither_sent_nor_reported_as_skipped() {
    let selected = select_station_cores(vec![
        core(3, "Disabled keyed", false, "synthetic-key"),
        core(5, "Disabled keyless", false, ""),
    ]);
    assert!(selected.keyed.is_empty());
    assert!(selected.skipped.is_empty());
}

/// Accepting an all-keyless selection would install a station without any usable core.
#[test]
fn all_keyless_selection_has_no_sendable_credentials() {
    let selected = select_station_cores(vec![core(3, "Needs key", true, "")]);
    assert!(selected.keyed.is_empty());
    assert_eq!(selected.skipped, ["Needs key"]);
}
