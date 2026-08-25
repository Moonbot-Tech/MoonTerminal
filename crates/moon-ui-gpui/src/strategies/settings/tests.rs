//! Unit tests for resolving optional Strategies display preferences.

use moon_core::config::layout::WindowLayout;

use super::{ACTIVE_ONLY, GROUP_BY_VENUE, POPUP_ROWS, PREF_ROWS, StrategiesPrefs};

/// `POPUP_ROWS` is a hand-written subset of `PREF_ROWS`, so nothing in the type system stops it
/// from listing a row that is edited elsewhere — which would give one preference two controls — or
/// from going empty, which renders the group as a caption over an empty frame. Persistence covers
/// the full set either way, and a row absent from BOTH lists would be saved but uneditable.
#[test]
fn the_popup_renders_a_non_empty_subset_that_excludes_the_filter_row_toggle() {
    assert!(
        !POPUP_ROWS.is_empty(),
        "an empty popup group renders a caption over an empty frame"
    );
    assert!(
        !POPUP_ROWS.iter().any(|row| row.id == ACTIVE_ONLY.id),
        "active-only is edited from the filter row, never from the popup"
    );
    for row in &POPUP_ROWS {
        assert!(
            PREF_ROWS.iter().any(|known| known.id == row.id),
            "{} is rendered but never restored or persisted",
            row.id
        );
    }
}

/// `StrategiesPrefs::default` changing active-only to ON would hide unchecked strategies after an
/// upgrade, while changing grouping to OFF would flatten the established exchange hierarchy.
#[test]
fn absent_preferences_keep_the_shipped_tree_defaults() {
    let restored = StrategiesPrefs::restore(&WindowLayout::default());
    assert!(restored.group_by_venue, "venue grouping must ship enabled");
    assert!(!restored.active_only, "active-only must ship disabled");
}

/// `StrategiesPrefs::restore` stamping one absent key from its saved neighbor would make editing a
/// single checkbox silently freeze both defaults for future launches.
#[test]
fn saved_preferences_restore_independently() {
    let mut grouped_only = WindowLayout::default();
    (GROUP_BY_VENUE.store)(&mut grouped_only, false);
    let restored = StrategiesPrefs::restore(&grouped_only);
    assert!(!restored.group_by_venue);
    assert!(
        !restored.active_only,
        "the absent active-only key keeps its default"
    );

    let mut active_only = WindowLayout::default();
    (ACTIVE_ONLY.store)(&mut active_only, true);
    let restored = StrategiesPrefs::restore(&active_only);
    assert!(
        restored.group_by_venue,
        "the absent grouping key keeps its default"
    );
    assert!(restored.active_only);

    let mut explicit = WindowLayout::default();
    (GROUP_BY_VENUE.store)(&mut explicit, false);
    (ACTIVE_ONLY.store)(&mut explicit, true);
    assert_eq!(
        StrategiesPrefs::restore(&explicit),
        StrategiesPrefs {
            group_by_venue: false,
            active_only: true,
        }
    );
}
