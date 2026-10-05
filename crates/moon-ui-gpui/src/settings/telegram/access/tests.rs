//! Station grants must not inherit the terminal's unrelated uid namespace.
use super::station_catalog;

/// Using local uid 3 would grant address B instead of the selected terminal address A.
#[test]
fn station_catalog_uses_station_uids_and_preserves_station_only_cores() {
    use moon_core::station_api::ListedCore;
    let local = vec![(3, "Address A".into())];
    let listing = vec![
        ListedCore {
            uid: 3,
            name: "Address B".into(),
            address: Some("198.51.100.12:4510".into()),
            key_fp: None,
        },
        ListedCore {
            uid: 9,
            name: "Address A".into(),
            address: Some("198.51.100.11:4510".into()),
            key_fp: None,
        },
    ];
    assert_eq!(
        station_catalog(local.clone(), Some(Some(&listing))),
        vec![(3, "Address B".into()), (9, "Address A".into())]
    );
    assert_eq!(station_catalog(local.clone(), Some(None)), local);
    assert!(station_catalog(local.clone(), Some(Some(&[]))).is_empty());
    assert!(station_catalog(local, None).is_empty());
}

/// A fresh listing must not silently erase a previously granted removed identity.
#[test]
fn missing_granted_cores_remain_archived_choices() {
    let mut catalog = vec![(9, "Live".into())];
    super::retain_granted(
        &mut catalog,
        [9, 3, 3, 0, moon_core::config::NO_MATCH_CORE_UID],
        "Archived",
    );
    assert_eq!(catalog, [(9, "Live".into()), (3, "Archived".into())]);
}
