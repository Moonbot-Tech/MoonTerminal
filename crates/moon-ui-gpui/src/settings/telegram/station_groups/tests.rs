use moon_core::config::CoreGroup;

use super::{names, same_groups};

fn group(name: &str, cores: &[u64]) -> CoreGroup {
    CoreGroup {
        name: name.into(),
        cores: cores.to_vec(),
    }
}

/// The same groups in another order, or with members in another order, are the same set; a
/// renamed group, a moved core or a missing group is not.
#[test]
fn groups_compare_as_sets() {
    let ours = vec![group("main", &[1, 2]), group("AAA", &[3])];
    assert!(same_groups(
        &ours,
        &[group("AAA", &[3]), group("main", &[2, 1])]
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
