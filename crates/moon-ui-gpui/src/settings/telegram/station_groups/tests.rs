use moon_core::config::CoreGroup;

use super::{names, same_groups};

/// Comparing raw numbers would call the wrong group identical and keep offering the correct one.
#[test]
fn issue_890_comparison_uses_the_same_translated_memberships_as_the_payload() {
    use crate::backend::station::cores_sync::LocalCore;
    use moon_core::station_api::ListedCore;

    let here: Vec<_> = [(1, "XALPHA"), (2, "XBETA")]
        .into_iter()
        .map(|(uid, name)| LocalCore {
            uid,
            name: name.into(),
            address: Some(format!("{name}.example.invalid:4510")),
            key_address: Some(format!("{name}.example.invalid:4510")),
            endpoint_override: String::new(),
            key_fp: name.into(),
        })
        .collect();
    let listing: Vec<_> = here
        .iter()
        .zip([11, 1])
        .map(|(core, uid)| ListedCore {
            uid,
            name: core.name.clone(),
            address: core.address.clone(),
            key_fp: Some(core.key_fp.clone()),
            endpoint_override: Some(String::new()),
        })
        .collect();
    let ours = [group("UNIT_01", &[1])];
    assert!(super::same_station_groups(
        &ours,
        &[group("UNIT_01", &[11])],
        &here,
        &listing
    ));
    assert!(!super::same_station_groups(
        &ours,
        &[group("UNIT_01", &[1])],
        &here,
        &listing
    ));
    assert!(!super::same_station_groups(
        &ours,
        &[group("UNIT_01", &[])],
        &here,
        &[]
    ));
}

fn group(name: &str, cores: &[u64]) -> CoreGroup {
    CoreGroup {
        name: name.into(),
        cores: cores.to_vec(),
    }
}

/// Ignoring group positions would disable sending a reorder; member order is not displayed.
#[test]
fn groups_compare_positions_but_ignore_member_order() {
    let ours = vec![group("main", &[1, 2]), group("AAA", &[3])];
    assert!(!same_groups(
        &ours,
        &[group("AAA", &[3]), group("main", &[2, 1])]
    ));
    assert!(same_groups(
        &ours,
        &[group("main", &[2, 1]), group("AAA", &[3])]
    ));
    assert!(!same_groups(&ours, &[group("main", &[1, 2])]));
    assert!(!same_groups(
        &ours,
        &[group("main", &[1, 2]), group("AAB", &[3])]
    ));
    assert!(!same_groups(
        &ours,
        &[group("main", &[1]), group("AAA", &[2, 3])]
    ));
    assert!(same_groups(&[], &[]));
}

/// The line names the groups as listed; none reads as a dash.
#[test]
fn groups_are_named_in_order() {
    assert_eq!(
        names(&[group("main", &[1]), group("margo", &[2])]),
        "main, margo"
    );
    assert_eq!(names(&[]), "\u{2014}");
}
