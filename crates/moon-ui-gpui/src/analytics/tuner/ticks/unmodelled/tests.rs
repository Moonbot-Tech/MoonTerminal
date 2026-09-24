use std::collections::HashMap;

// Not `super::*`: the parent's `gpui::*` brings gpui's own `test` attribute, which `#[test]` would
// then name, and it expands into itself.
use super::unmodelled_map;
use moon_core::feed::strategy_deps::FieldDeps;

fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// Every strategy read is in the map — one with a field outside the model with it, a clean one
/// with nothing — so a strategy absent from it is one the load did not read.
#[test]
fn the_map_holds_every_strategy_read() {
    let ladder = values(&[("UseSecondStop", "YES")]);
    let plain = values(&[("SellPrice", "1.5"), ("IgnoreSellShot", "YES")]);
    let map = unmodelled_map(
        [((1, Some(7)), &ladder), ((2, Some(7)), &plain)],
        &HashMap::new(),
        &FieldDeps::bundled(),
    );
    assert_eq!(map[&(1, Some(7))][0].key, "UseSecondStop");
    // Read and clean is not "not read": the strategy is there, with nothing to say.
    assert!(map[&(2, Some(7))].is_empty());
    assert!(!map.contains_key(&(3, Some(7))));
}
