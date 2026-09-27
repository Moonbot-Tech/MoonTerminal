use super::*;

fn set(ids: &[CoreId]) -> HashSet<CoreId> {
    ids.iter().copied().collect()
}

fn group(name: &str, cores: &[CoreId]) -> CoreGroup {
    CoreGroup {
        name: name.to_string(),
        cores: cores.to_vec(),
    }
}

fn cores_n(n: usize) -> String {
    format!("cores: {n}")
}

#[test]
fn empty_narrowing_is_the_whole_scope() {
    assert_eq!(narrowed_ids(&[1, 2, 3], &HashSet::new()), None);
}

#[test]
fn narrowing_is_intersected_with_the_scope_in_scope_order() {
    assert_eq!(narrowed_ids(&[3, 1, 2], &set(&[2, 3, 9])), Some(vec![3, 2]));
}

#[test]
fn stale_narrowing_outside_the_scope_falls_back_to_the_whole_scope() {
    assert_eq!(narrowed_ids(&[1, 2], &set(&[7, 8])), None);
    assert!(shown_selection(&[1, 2], &set(&[7, 8])).is_empty());
}

#[test]
fn narrowing_covering_the_whole_scope_is_no_narrowing() {
    assert_eq!(narrowed_ids(&[1, 2], &set(&[1, 2, 5])), None);
}

#[test]
fn label_reads_overview_for_the_whole_scope() {
    let groups = [group("G", &[1])];
    assert_eq!(
        trigger_label(&[1, 2], &HashSet::new(), &groups, "Full", &cores_n),
        "Full"
    );
    assert_eq!(
        trigger_label(&[1, 2], &set(&[9]), &groups, "Full", &cores_n),
        "Full"
    );
}

#[test]
fn label_names_a_group_matched_within_the_scope() {
    // The group names a core outside the scope; within the scope it is exactly {1, 2}.
    let groups = [group("Other", &[3]), group("Pair", &[1, 2, 99])];
    assert_eq!(
        trigger_label(&[1, 2, 3], &set(&[1, 2]), &groups, "Full", &cores_n),
        "Pair"
    );
}

#[test]
fn label_counts_a_narrowing_that_is_no_group() {
    let groups = [group("Pair", &[1, 2])];
    assert_eq!(
        trigger_label(&[1, 2, 3, 4], &set(&[1, 3]), &groups, "Full", &cores_n),
        "cores: 2"
    );
}
