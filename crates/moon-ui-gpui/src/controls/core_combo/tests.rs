//! Regression coverage for shared core-selector grouping and summaries.

use std::collections::{HashMap, HashSet};

use moon_core::venue::CoreVenue;

use super::{
    CoreAllRowMode, core_menu_sections, core_selection_is_all, core_selection_summary,
    toggle_exchange_cores,
};
use crate::controls::venue_label;

/// Build one core's venue as the session publishes it.
fn venue(code: u8) -> CoreVenue {
    CoreVenue::identify(code, "", None)
}

/// `core_combo.rs:core_menu_sections` must keep unidentified cores first, sort exchange sections by
/// their visible caption, and preserve the incoming canonical member order. Replacing the shared
/// section helper with one direct pass over `cores` removes the exchange hierarchy from every core
/// dropdown.
#[test]
fn menu_sections_are_unknown_first_alphabetical_and_member_stable() {
    let cores = vec![
        (1, "Bybit first".to_string()),
        (2, "Unknown".to_string()),
        (3, "Binance first".to_string()),
        (4, "Bybit second".to_string()),
        (5, "Binance second".to_string()),
    ];
    let venues = HashMap::from([(1, venue(2)), (3, venue(4)), (4, venue(2)), (5, venue(4))]);

    let sections = core_menu_sections(&cores, &venues);
    let actual: Vec<(Option<String>, Vec<u64>)> = sections
        .into_iter()
        .map(|(venue, members)| {
            (
                venue.map(venue_label),
                members.into_iter().map(|(core, _)| core).collect(),
            )
        })
        .collect();

    assert_eq!(
        actual,
        vec![
            (None, vec![2]),
            (Some("Binance Futures".to_string()), vec![3, 5]),
            (Some("Bybit Futures".to_string()), vec![1, 4]),
        ]
    );
}

/// `core_combo.rs:core_selection_summary` must format a one-core partial selection as a count. Restoring
/// the old sole-name branch exposes arbitrary server text instead of the compact summary promised
/// by the shared selector.
#[test]
fn one_selected_core_uses_the_compact_count_summary() {
    let cores = vec![
        (1, "A deliberately very long server name".to_string()),
        (2, "Second".to_string()),
        (3, "Third".to_string()),
    ];
    let selected = HashSet::from([1]);

    let summary = core_selection_summary(
        &cores,
        &selected,
        CoreAllRowMode::ImplicitOrComplete,
        "All cores",
        &|n| format!("Cores: {n}"),
    );

    assert_eq!(summary.label, "Cores: 1");
    assert!(!summary.all_on);
    assert_eq!(summary.selected, 1);
}

/// `core_combo.rs:core_selection_summary` must preserve empty/full All semantics and reject a stale
/// equal-cardinality selection. Replacing membership counting with `selected.len() == cores.len()`
/// makes missing current cores appear selected and labels an empty result as All.
#[test]
fn all_summary_requires_every_available_core() {
    let cores = vec![(1, "First".to_string()), (2, "Second".to_string())];

    let summary = |selected: HashSet<u64>| {
        let out = core_selection_summary(
            &cores,
            &selected,
            CoreAllRowMode::ImplicitOrComplete,
            "All cores",
            &|n| format!("Cores: {n}"),
        );
        (out.label, out.all_on)
    };
    let empty = summary(HashSet::new());
    let full = summary(HashSet::from([1, 2]));
    let full_with_stale = summary(HashSet::from([1, 2, 99]));
    let stale_equal_cardinality = summary(HashSet::from([98, 99]));

    assert_eq!(empty, ("All cores".to_string(), true));
    assert_eq!(full, ("All cores".to_string(), true));
    assert_eq!(stale_equal_cardinality, ("Cores: 0".to_string(), false));
    assert_eq!(full_with_stale, ("All cores".to_string(), true));
}

/// `CoreAllRowMode::ImplicitOnly` must not check All for a complete explicit selection.
///
/// Plausible edit this catches: routing `ImplicitOnly` through `core_selection_is_all` in
/// `core_combo.rs:core_selection_summary`. On a one-core Analytics installation, clicking that core
/// would leave both its row and All checked.
#[test]
fn implicit_only_mode_keeps_a_complete_explicit_selection_out_of_all() {
    let cores = vec![(1, "Only core".to_string())];
    let selected = HashSet::from([1]);

    let summary = core_selection_summary(
        &cores,
        &selected,
        CoreAllRowMode::ImplicitOnly,
        "All cores",
        &|n| format!("Cores: {n}"),
    );

    assert_eq!(
        (summary.label, summary.all_on),
        ("Cores: 1".to_string(), false)
    );
}

/// `core_combo.rs:toggle_exchange_cores` must remove an exchange only when every available member
/// is selected while preserving other exchanges. Replacing `all` with `any` removes a partial
/// exchange instead of completing it; unconditional extension makes the second click unable to
/// clear its checkmarks.
#[test]
fn exchange_toggle_adds_or_removes_only_available_members() {
    let available = HashSet::from([1, 2, 3, 4]);
    let mut selected = HashSet::new();

    assert!(toggle_exchange_cores(&mut selected, &available, [1, 2, 99]));
    assert_eq!(selected, HashSet::from([1, 2]));

    selected.insert(4);
    assert!(toggle_exchange_cores(&mut selected, &available, [1, 2]));
    assert_eq!(selected, HashSet::from([4]));

    selected.insert(1);
    assert!(toggle_exchange_cores(&mut selected, &available, [1, 2]));
    assert_eq!(selected, HashSet::from([1, 2, 4]));

    let mut exchange_only = HashSet::from([1, 2]);
    assert!(toggle_exchange_cores(
        &mut exchange_only,
        &available,
        [1, 2]
    ));
    assert!(exchange_only.is_empty());

    assert!(!toggle_exchange_cores(&mut selected, &available, [98, 99]));
    assert_eq!(selected, HashSet::from([1, 2, 4]));
}

/// `core_combo.rs:core_selection_is_all` must weigh EVERY available core, first one included.
///
/// It drives every `ImplicitOrComplete` dropdown summary, so an off-by-one would mark All while
/// leaving one available core's own checkbox unchecked.
///
/// Breakage this pins: consuming the availability iterator to test emptiness and then scanning
/// only what is left (the shape a `next()`-then-`all()` pair produces, where `Peekable` is what
/// makes the current form correct). The first available core would drop out of the comparison, so
/// selecting every core EXCEPT the first would read as "all cores" and query all of them.
#[test]
fn the_all_cores_predicate_weighs_the_first_available_core() {
    // Everything but the first: a partial selection that must NOT read as All.
    assert!(!core_selection_is_all([1, 2, 3], &HashSet::from([2, 3])));
    // Only the first: likewise partial.
    assert!(!core_selection_is_all([1, 2, 3], &HashSet::from([1])));
    // Genuinely every one, and the implicit form.
    assert!(core_selection_is_all([1, 2, 3], &HashSet::from([1, 2, 3])));
    assert!(core_selection_is_all([1, 2, 3], &HashSet::new()));
    // A stale id cannot stand in for a missing available one.
    assert!(!core_selection_is_all(
        [1, 2, 3],
        &HashSet::from([1, 2, 99])
    ));
    // No scope at all is not "all": there is nothing for the selection to cover.
    assert!(!core_selection_is_all([], &HashSet::from([7])));
}
