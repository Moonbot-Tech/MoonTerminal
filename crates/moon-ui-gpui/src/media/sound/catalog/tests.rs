//! Sound catalog and picker regression coverage without a running GPUI app.

use super::{Catalog, Source};

/// Removing path normalization duplicates a folder sound and selects default playback.
/// The picker must select the existing flat file instead of appending a missing-name row.
#[test]
fn sound_archive_name_resolves_and_has_one_selected_catalog_row() {
    let mut catalog = Catalog::embedded();
    let mut hook = catalog.entries()[0].clone();
    hook.stem = "hook".into();
    hook.label = "hook".into();
    hook.ordinal = None;
    hook.source = Source::Folder;
    catalog.add(hook).expect("add fixture sound");
    assert_eq!(
        catalog.find("sounds/hook").map(|e| e.stem.as_str()),
        Some("hook")
    );
    assert!(catalog.find("sounds/").is_none());
    super::super::CATALOG.with(|slot| *slot.borrow_mut() = catalog);

    let choices = crate::panels::common::SoundChoices::for_current("sounds/hook");
    assert_eq!(
        choices
            .stems
            .iter()
            .filter(|stem| stem.as_str() == "hook")
            .count(),
        1
    );
    assert_eq!(
        choices.selected.map(|i| choices.stems[i].as_str()),
        Some("hook")
    );
}
