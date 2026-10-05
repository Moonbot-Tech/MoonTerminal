//! Confirmation and layout invariants from the approved station cores mockup.
use super::{CoreCache, removable, retained_arm, state_label, table_fits};
use crate::backend::station::cores_sync::{self, LocalCore, RowState};
use moon_core::station_api::ListedCore;
use moon_ui::MoonTone;

/// A reused uid, changed identity or newly local match must discard destructive consent.
#[test]
fn confirmation_is_cleared_when_identity_changes_or_becomes_local() {
    let listing = [3, 9].map(|uid| ListedCore {
        uid,
        name: format!("Core {uid}"),
        address: Some(format!("198.51.100.{uid}:4510")),
        key_fp: None,
    });
    let rows = cores_sync::reconcile(&[], &listing, None);
    let arm = (9, listing[1].address.clone(), listing[1].name.clone());
    assert_eq!(retained_arm(Some(arm.clone()), &rows), Some(arm.clone()));
    assert_eq!(retained_arm(Some(arm.clone()), &rows[1..]), None);
    assert!(removable(&rows[1], 2));
    let mut changed = listing.clone();
    changed[1].name = "Reused".into();
    assert_eq!(
        retained_arm(
            Some(arm.clone()),
            &cores_sync::reconcile(&[], &changed, None)
        ),
        None
    );
    changed[1].name = listing[1].name.clone();
    changed[1].address = Some("198.51.100.99:4510".into());
    assert_eq!(
        retained_arm(
            Some(arm.clone()),
            &cores_sync::reconcile(&[], &changed, None)
        ),
        None
    );
    let local = LocalCore {
        uid: 17,
        name: "Now local".into(),
        address: listing[1].address.clone(),
        key_fp: "fixture".into(),
    };
    assert_eq!(
        retained_arm(Some(arm), &cores_sync::reconcile(&[local], &listing, None)),
        None
    );
}

/// The mockup reserves positive tone for equal cores and warning/info for push actions.
#[test]
fn actionable_states_follow_mockup_invariants() {
    for state in [
        RowState::Same,
        RowState::OnlyHere,
        RowState::OnlyOnStation,
        RowState::NameDiffers,
        RowState::KeyDiffers,
    ] {
        let (_, tone) = state_label(state);
        if matches!(tone, MoonTone::Warning | MoonTone::Info) {
            assert!(matches!(
                state,
                RowState::OnlyHere | RowState::NameDiffers | RowState::KeyDiffers
            ));
        }
        if state == RowState::OnlyOnStation {
            assert!(!matches!(tone, MoonTone::Positive));
        }
        if state == RowState::Same {
            assert!(!matches!(
                tone,
                MoonTone::Warning | MoonTone::Info | MoonTone::Danger
            ));
        }
    }
}

/// A longer translated action must raise the table breakpoint instead of overflowing.
#[test]
fn layout_switches_at_the_measured_content_boundary() {
    assert!(table_fits(600.0, 200.0, 100.0, 100.0, 150.0, 50.0));
    assert!(!table_fits(599.0, 200.0, 100.0, 100.0, 150.0, 50.0));
    assert!(!table_fits(600.0, 300.0, 100.0, 100.0, 150.0, 50.0));
    assert!(table_fits(700.0, 300.0, 100.0, 100.0, 150.0, 50.0));
}

/// Stale cached rows would keep a removed core actionable or hide a saved local rename.
#[test]
fn cache_invalidates_on_listing_and_local_revision() {
    let mut cache = CoreCache::default();
    let remote = ListedCore {
        uid: 9,
        name: "Core".into(),
        address: Some("198.51.100.9:4510".into()),
        key_fp: Some("fp".into()),
    };
    let local = LocalCore {
        uid: 3,
        name: "Core".into(),
        address: remote.address.clone(),
        key_fp: "fp".into(),
    };
    let listing = Some(Some(vec![remote]));
    cache.update(&listing, vec![local.clone()], None);
    assert_eq!(cache.rows[0].state, RowState::Same);
    let mut renamed = local;
    renamed.name = "Renamed".into();
    cache.update(&listing, vec![renamed], None);
    assert_eq!(cache.rows[0].state, RowState::NameDiffers);
    cache.update(&Some(Some(vec![])), vec![], None);
    assert!(cache.rows.is_empty());
}

/// Retaining cached allocations after a floor change would offer a uid from retired history.
#[test]
fn cache_invalidates_when_only_retirement_floor_changes() {
    let mut cache = CoreCache::default();
    let local = LocalCore {
        uid: 3,
        name: "New".into(),
        address: Some("198.51.100.3:4510".into()),
        key_fp: "fixture".into(),
    };
    let listing = Some(Some(vec![]));
    cache.update(&listing, vec![local.clone()], Some(7));
    assert_eq!(cache.rows[0].station_uid, Some(8));
    cache.update(&listing, vec![local], Some(12));
    assert_eq!(cache.rows[0].station_uid, Some(13));
}
