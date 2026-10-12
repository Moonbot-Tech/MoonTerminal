use super::*;

fn group(name: &str, cores: &[u64]) -> CoreGroup {
    CoreGroup {
        name: name.into(),
        cores: cores.to_vec(),
    }
}

fn shape<'a>(sections: &[Section<'a>]) -> Vec<(Option<&'a str>, Vec<usize>)> {
    sections
        .iter()
        .map(|section| {
            (
                section.group.map(|group| group.name.as_str()),
                section.members.clone(),
            )
        })
        .collect()
}

/// Reintroducing name sorting would ignore the saved order; ungrouped cores remain last.
#[test]
fn groups_by_saved_order_then_the_rest() {
    let groups = [group("main", &[30, 10]), group("AAA", &[20])];
    let got = sections(&[10, 20, 30, 40], &groups).unwrap();
    assert_eq!(
        shape(&got),
        vec![
            (Some("main"), vec![0, 2]),
            (Some("AAA"), vec![1]),
            (None, vec![3]),
        ]
    );
}

/// A core saved in two groups is listed under both.
#[test]
fn a_core_in_two_groups_is_in_both() {
    let groups = [group("a", &[1, 2]), group("b", &[2])];
    let got = sections(&[1, 2], &groups).unwrap();
    assert_eq!(
        shape(&got),
        vec![(Some("a"), vec![0, 1]), (Some("b"), vec![1])]
    );
}

/// One caption over everything says nothing: no group, an empty one, or one holding every core.
#[test]
fn one_section_stays_flat() {
    assert_eq!(sections(&[1, 2], &[]), None);
    assert_eq!(sections(&[1, 2], &[group("gone", &[9])]), None);
    assert_eq!(sections(&[1, 2], &[group("all", &[1, 2])]), None);
    assert_eq!(sections(&[], &[group("a", &[1])]), None);
}

/// A rename must not reorder sections; inactive saved groups must not add empty headers.
#[test]
fn renamed_groups_keep_their_positions_and_absent_cores_stay_hidden() {
    let groups = [
        group("Zulu renamed", &[1]),
        group("Inactive", &[9]),
        group("alpha", &[2]),
    ];
    let got = sections(&[1, 2, 3], &groups).unwrap();
    assert_eq!(
        shape(&got),
        vec![
            (Some("Zulu renamed"), vec![0]),
            (Some("alpha"), vec![1]),
            (None, vec![2])
        ]
    );
}
