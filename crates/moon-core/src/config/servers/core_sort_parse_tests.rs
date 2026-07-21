//! Robustness checks for parsing the global core-order setting.

use super::CoreSortMode;
use serde::Deserialize;

/// Minimal settings wrapper used to verify that parsing preserves an adjacent field.
#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    core_sort: CoreSortMode,
    #[serde(default)]
    keep: String,
}

/// Protect `CoreSortMode::deserialize`: an invalid word or type resets only this field while
/// preserving the rest of `SettingsFile`.
#[test]
fn a_bad_core_sort_value_never_costs_the_rest_of_the_file() {
    for bad in [
        r#"core_sort = "typo""#,
        "core_sort = 1",
        "core_sort = 1.5",
        "core_sort = true",
        "core_sort = []",
        "core_sort = {}",
    ] {
        let toml = format!("{bad}\nkeep = \"server meta\"\n");
        let probe: Probe = toml::from_str(&toml)
            .unwrap_or_else(|e| panic!("`{bad}` must not fail the whole file: {e}"));
        assert_eq!(probe.core_sort, CoreSortMode::Name, "for `{bad}`");
        assert_eq!(probe.keep, "server meta", "for `{bad}`");
    }
}

/// An unsupported mode code maps to `Name` without affecting adjacent fields.
///
/// The plausible breakage is adding a special mapping from an unknown code to one of the
/// insertion-order modes, producing an arbitrary result instead of the conservative default.
#[test]
fn a_retired_manual_setting_lands_on_the_new_default() {
    let probe: Probe = toml::from_str("core_sort = \"manual\"\nkeep = \"server meta\"\n")
        .expect("a retired code must not fail the file");
    assert_eq!(probe.core_sort, CoreSortMode::Name);
    assert_eq!(probe.keep, "server meta");
}

/// On-disk codes are fixed format values, not free-form identifiers.
///
/// The round-trip check below cannot catch this because it passes `code()` back into
/// `from_code()`. Changing both sides together stays green while an existing on-disk `"added"`
/// begins mapping to `Name`. These literals pin the contract independently.
#[test]
fn the_on_disk_codes_are_frozen() {
    assert_eq!(CoreSortMode::Name.code(), "name");
    assert_eq!(CoreSortMode::AddedOldest.code(), "added");
    assert_eq!(CoreSortMode::AddedNewest.code(), "added_newest");
}

/// Protect the accepted-code mapping used by `CoreSortMode` serialization.
#[test]
fn every_mode_round_trips_through_its_code() {
    for mode in [
        CoreSortMode::Name,
        CoreSortMode::AddedOldest,
        CoreSortMode::AddedNewest,
    ] {
        let toml = format!("core_sort = \"{}\"\nkeep = \"\"\n", mode.code());
        let probe: Probe = toml::from_str(&toml).expect("a valid code must parse");
        assert_eq!(probe.core_sort, mode);
    }
}
