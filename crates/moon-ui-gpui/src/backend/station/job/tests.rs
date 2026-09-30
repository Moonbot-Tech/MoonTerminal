//! Validation failures retain kinds until the presentation boundary.

use super::{core_keys, first_access, picked_core_key};
use moon_core::config::{CoreKeyEntry, Secret};
use moon_remote::error::StationError;

/// Returning a formatted string here makes an empty-password form show English in all locales.
#[test]
fn missing_password_and_core_selection_are_typed() {
    let failure = first_access("root".into(), None, None).err().unwrap();
    assert_eq!(
        failure.downcast_ref::<StationError>(),
        Some(&StationError::EmptyPassword)
    );
    let failure = core_keys(&[]).err().unwrap();
    assert_eq!(
        failure.downcast_ref::<StationError>(),
        Some(&StationError::NoCorePicked)
    );
}

/// Flattening lookup errors loses the missing uid or core name needed for recovery guidance.
#[test]
fn missing_and_keyless_cores_fail_before_server_work() {
    let entries = [CoreKeyEntry {
        uid: 17,
        name: "Fixture core".into(),
        key: Secret::default(),
        transport: None,
        active: true,
    }];
    let missing = picked_core_key(&entries, 19).err().unwrap();
    assert_eq!(
        missing.downcast_ref::<StationError>(),
        Some(&StationError::CoreMissing(19))
    );
    let keyless = picked_core_key(&entries, 17).err().unwrap();
    assert_eq!(
        keyless.downcast_ref::<StationError>(),
        Some(&StationError::CoreWithoutKey("Fixture core".into()))
    );
    assert!(!format!("{keyless:#}").contains("Secret"));
}
