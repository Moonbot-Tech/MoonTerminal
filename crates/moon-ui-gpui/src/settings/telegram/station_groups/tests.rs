use moon_core::config::CoreGroup;

use super::{names, same_groups};

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
