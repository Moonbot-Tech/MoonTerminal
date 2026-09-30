//! Validation failures retain kinds until the presentation boundary.

use super::{core_keys, first_access, picked_core_key, show_journal, show_status};
use moon_core::config::{CoreKeyEntry, Secret};
use moon_remote::error::StationError;
use moon_remote::station::bot::BotState;

/// Collect the exact lines the production job would send to Settings.
fn lines(show: impl FnOnce(&mut dyn FnMut(moon_remote::progress::Progress))) -> Vec<String> {
    let mut lines = Vec::new();
    show(&mut |event| {
        if let Some(text) = super::super::text::progress(event) {
            lines.push(text);
        }
    });
    lines
}

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

/// Suppressing helper tokens must not suppress stopped/no-API Status feedback in Station.
#[test]
fn status_without_api_still_reaches_settings() {
    let _locale = crate::test_locale::force("en");
    let stopped = BotState {
        stopped: true,
        ..BotState::default()
    };
    assert_eq!(
        lines(|say| show_status(&stopped, say)),
        ["The service on the server is stopped."]
    );
    let old = BotState {
        no_api: true,
        ..BotState::default()
    };
    let shown = lines(|say| show_status(&old, say));
    assert!(shown[0].contains("Update the service"));
}

/// Treating the Logs result as hidden helper tokens would remove the requested journal.
#[test]
fn requested_journal_remains_visible_as_secondary_details() {
    let _locale = crate::test_locale::force("en");
    let shown = lines(|say| show_journal("synthetic journal entry\nsecond entry", say));
    assert_eq!(
        shown,
        [
            "Station journal:",
            "Details: synthetic journal entry",
            "Details: second entry"
        ]
    );
}
