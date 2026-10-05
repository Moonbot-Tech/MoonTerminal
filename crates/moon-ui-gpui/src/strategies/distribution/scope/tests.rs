use std::collections::{HashMap, HashSet};

use moon_core::feed::StrategyRow;
use moon_core::session::CoreId;

use super::board_scope;
use crate::strategies::filter::StrategyFilter;

fn row(id: u64, folder_path: &str, checked: bool) -> StrategyRow {
    StrategyRow {
        id,
        name: format!("s{id}"),
        kind: "Test".to_string(),
        kind_ordinal: 0,
        folder_path: folder_path.to_string(),
        checked,
        is_short: false,
        fields: Vec::new(),
    }
}

/// Two cores: core 1 holds a nested tree, core 2 a flat one.
fn store() -> HashMap<CoreId, Vec<StrategyRow>> {
    HashMap::from([
        (
            1,
            vec![
                row(10, "", true),
                row(11, "alpha", true),
                row(12, "alpha/deep", false),
                row(13, "alpha/deep/deeper", true),
                row(14, "alphabet", true),
                row(15, "beta", true),
            ],
        ),
        (2, vec![row(20, "", true), row(21, "gamma", false)]),
    ])
}

/// Run `board_scope` over `store()` and return each core's ids sorted, for stable comparison.
fn scope(
    folders: &[(CoreId, &str)],
    fallback: Vec<(CoreId, u64)>,
    filter: &StrategyFilter,
    shown: &[CoreId],
) -> Vec<(CoreId, Vec<u64>)> {
    let rows = store();
    let folders: Vec<(CoreId, String)> = folders
        .iter()
        .map(|(core, path)| (*core, path.to_string()))
        .collect();
    let got = board_scope(
        &folders,
        move || fallback,
        |core| rows.get(&core).map(Vec::as_slice),
        &filter.prepare(),
        |core| shown.contains(&core),
    );
    let mut out: Vec<(CoreId, Vec<u64>)> = got
        .into_iter()
        .map(|(core, ids)| {
            let mut ids: Vec<u64> = ids.into_iter().collect();
            ids.sort_unstable();
            (core, ids)
        })
        .collect();
    out.sort_unstable();
    out
}

#[test]
fn a_selected_folder_covers_its_nested_strategies_and_outranks_a_stale_selection() {
    let got = scope(
        &[(1, "alpha")],
        vec![(1, 15), (2, 20)],
        &StrategyFilter::default(),
        &[1, 2],
    );
    // Every depth under `alpha`, and not the sibling `alphabet` that merely shares the prefix.
    assert_eq!(got, vec![(1, vec![11, 12, 13])]);
}

#[test]
fn a_core_node_covers_every_strategy_of_that_core() {
    let got = scope(&[(2, "")], Vec::new(), &StrategyFilter::default(), &[1, 2]);
    assert_eq!(got, vec![(2, vec![20, 21])]);
}

#[test]
fn a_parent_and_its_child_in_one_range_do_not_duplicate_rows() {
    let got = scope(
        &[(1, "alpha"), (1, "alpha/deep"), (1, "")],
        Vec::new(),
        &StrategyFilter::default(),
        &[1, 2],
    );
    assert_eq!(got, vec![(1, vec![10, 11, 12, 13, 14, 15])]);
}

#[test]
fn active_only_keeps_only_the_checked_strategies_under_the_folder() {
    let filter = StrategyFilter {
        active_only: true,
        ..StrategyFilter::default()
    };
    let got = scope(&[(1, "alpha"), (2, "")], Vec::new(), &filter, &[1, 2]);
    assert_eq!(got, vec![(1, vec![11, 13]), (2, vec![20])]);
}

#[test]
fn search_narrows_the_folder_to_matching_names() {
    let filter = StrategyFilter {
        search: "s12".to_string(),
        ..StrategyFilter::default()
    };
    let got = scope(&[(1, "alpha")], Vec::new(), &filter, &[1, 2]);
    assert_eq!(got, vec![(1, vec![12])]);
}

#[test]
fn a_core_the_exchange_filter_hides_contributes_nothing() {
    let got = scope(
        &[(1, "alpha"), (2, "")],
        Vec::new(),
        &StrategyFilter::default(),
        &[1],
    );
    assert_eq!(got, vec![(1, vec![11, 12, 13])]);
}

#[test]
fn a_backslash_wire_path_matches_its_slash_joined_folder_key() {
    let rows = HashMap::from([(
        3,
        vec![row(30, "alpha\\deep", true), row(31, "alpha", true)],
    )]);
    let got = board_scope(
        &[(3, "alpha/deep".to_string())],
        Vec::new,
        |core| rows.get(&core).map(Vec::as_slice),
        &StrategyFilter::default().prepare(),
        |_| true,
    );
    assert_eq!(got, HashMap::from([(3, HashSet::from([30]))]));
}

#[test]
fn without_a_folder_selection_the_strategy_selection_is_the_scope() {
    let got = scope(
        &[],
        vec![(1, 15), (2, 21), (1, 10)],
        &StrategyFilter::default(),
        &[1, 2],
    );
    assert_eq!(got, vec![(1, vec![10, 15]), (2, vec![21])]);
}
