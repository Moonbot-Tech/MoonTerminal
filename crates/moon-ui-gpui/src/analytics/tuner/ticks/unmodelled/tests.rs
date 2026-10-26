use std::collections::HashMap;

// Not `super::*`: the parent's `gpui::*` brings gpui's own `test` attribute, which `#[test]` would
// then name, and it expands into itself.
use super::{search_warn_rows, unmodelled_map, warning_keys};
use moon_core::db::tuner::ticks::params::ParamSection;
use moon_core::feed::strategy_deps::FieldDeps;

fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// Entry-only searches must warn about cancellation risk while hiding unrelated exit fields.
#[test]
fn search_warning_rows_follow_the_searched_axes() {
    let rows = vec![(
        "CORE_A".into(),
        vec![
            (
                "CheckAfterBuy".into(),
                "YES".into(),
                "Filters".into(),
                false,
                ParamSection::Filters,
            ),
            (
                "SellByFilters".into(),
                "30".into(),
                "Sell order".into(),
                false,
                ParamSection::SellOrder,
            ),
        ],
    )];
    let entry = search_warn_rows(rows.clone(), 1, 0);
    assert_eq!(entry.len(), 1);
    assert_eq!(entry[0].1.len(), 1);
    assert_eq!(entry[0].1[0].0, "CheckAfterBuy");
    assert!(super::warn_lines(entry.clone(), Vec::new()).is_empty());
    assert_eq!(
        warning_keys(&entry),
        (
            "analytics.ticks.unmodelled_title_entry",
            "analytics.ticks.unmodelled_intro_entry"
        )
    );
    let exit = search_warn_rows(rows.clone(), 0, 1);
    assert_eq!(exit.len(), 1);
    assert_eq!(exit[0].1.len(), 1);
    assert_eq!(exit[0].1[0].0, "SellByFilters");
    assert_eq!(
        warning_keys(&exit),
        (
            "analytics.ticks.unmodelled_title",
            "analytics.ticks.unmodelled_intro"
        )
    );
    assert_eq!(
        warning_keys(&rows),
        (
            "analytics.ticks.unmodelled_title_mixed",
            "analytics.ticks.unmodelled_intro_mixed"
        )
    );
    // Save uses this same exit-only selector before formatting its exit-specific warning.
    assert!(
        exit.iter()
            .flat_map(|(_, fields)| fields)
            .all(|field| field.4 != ParamSection::Filters)
    );
    assert_eq!(search_warn_rows(rows.clone(), 1, 1), rows);
    assert!(search_warn_rows(rows, 0, 0).is_empty());
}

/// Every strategy read is in the map — one with a field outside the model with it, a clean one
/// with nothing — so a strategy absent from it is one the load did not read.
#[test]
fn the_map_holds_every_strategy_read() {
    let outside = values(&[("UseBV_SV_Stop", "YES")]);
    let plain = values(&[("SellPrice", "1.5"), ("IgnoreSellShot", "YES")]);
    let map = unmodelled_map(
        [((1, Some(7)), &outside), ((2, Some(7)), &plain)],
        &HashMap::new(),
        &FieldDeps::bundled(),
    );
    assert_eq!(map[&(1, Some(7))][0].key, "UseBV_SV_Stop");
    // Read and clean is not "not read": the strategy is there, with nothing to say.
    assert!(map[&(2, Some(7))].is_empty());
    assert!(!map.contains_key(&(3, Some(7))));
}
