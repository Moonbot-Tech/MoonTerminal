//! Regressions for the rail popup's saved preferences.

use moon_core::config::layout::WindowLayout;

use super::{CONTROL_ROWS, GROUPING_ROWS, RailPrefs};
use crate::workspace::RosterGrouping;

/// Catches a profile that never opened the popup getting server headings, or losing its exchange
/// headings, after the upgrade that added the grouping switches.
#[test]
fn untouched_layout_keeps_the_exchange_only_rail() {
    let prefs = RailPrefs::restore(&WindowLayout::default());
    assert_eq!(
        prefs.grouping(),
        RosterGrouping {
            by_server: false,
            by_exchange: true,
        }
    );
}

/// Catches a grouping row wired to the wrong `layout.toml` key or the wrong field: each saved key
/// must come back as its own switch and nothing else.
#[test]
fn each_grouping_key_restores_its_own_switch() {
    let mut layout = WindowLayout::default();
    layout.workspace_rail_group_by_server = Some(true);
    let prefs = RailPrefs::restore(&layout);
    assert!(prefs.group_by_server);
    assert!(prefs.group_by_exchange);

    let mut layout = WindowLayout::default();
    layout.workspace_rail_group_by_exchange = Some(false);
    let prefs = RailPrefs::restore(&layout);
    assert!(!prefs.group_by_server);
    assert!(!prefs.group_by_exchange);
}

/// Catches a row whose `store` writes a key its `saved` does not read back — the checkbox would
/// flip on screen and snap back after the next restore — or one that stamps a sibling's key.
#[test]
fn every_row_stores_only_the_key_it_reads() {
    for (index, row) in GROUPING_ROWS.iter().chain(&CONTROL_ROWS).enumerate() {
        let mut layout = WindowLayout::default();
        (row.store)(&mut layout, false);
        assert_eq!((row.saved)(&layout), Some(false), "row {}", row.id);
        for (other_index, other) in GROUPING_ROWS.iter().chain(&CONTROL_ROWS).enumerate() {
            if other_index != index {
                assert_eq!(
                    (other.saved)(&layout),
                    None,
                    "{} stamped {}",
                    row.id,
                    other.id
                );
            }
        }
    }
}
