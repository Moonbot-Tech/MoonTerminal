//! Unit tests for resolving optional Strategies display preferences.

use moon_core::config::layout::WindowLayout;

use super::StrategiesPrefs;

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
    let grouped_only = WindowLayout {
        strategies_group_by_venue: Some(false),
        ..WindowLayout::default()
    };
    let restored = StrategiesPrefs::restore(&grouped_only);
    assert!(!restored.group_by_venue);
    assert!(
        !restored.active_only,
        "the absent active-only key keeps its default"
    );

    let active_only = WindowLayout {
        strategies_active_only: Some(true),
        ..WindowLayout::default()
    };
    let restored = StrategiesPrefs::restore(&active_only);
    assert!(
        restored.group_by_venue,
        "the absent grouping key keeps its default"
    );
    assert!(restored.active_only);

    let explicit = WindowLayout {
        strategies_group_by_venue: Some(false),
        strategies_active_only: Some(true),
        ..WindowLayout::default()
    };
    assert_eq!(
        StrategiesPrefs::restore(&explicit),
        StrategiesPrefs {
            group_by_venue: false,
            active_only: true,
        }
    );
}
