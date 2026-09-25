//! The knobs' grids.

use super::*;

/// The corridor's grids step by 0.05 to 8, and hold a strategy's own spellings exactly: 1.7 is a
/// step, spelled back as `1.7`, not snapped to 1.75.
#[test]
fn the_corridor_grids_step_by_a_twentieth_to_eight() {
    for key in ["MShotPrice", "MShotPriceMin"] {
        let field = TICK_PARAMS.iter().find(|f| f.key == key).expect("a knob");
        let ParamKind::Num { grid } = &field.kind else {
            panic!("{key} is a number");
        };
        assert_eq!(grid.len(), 160, "{key}");
        assert_eq!(grid.first().copied(), Some(0.05));
        assert_eq!(grid.last().copied(), Some(8.0));
        assert!(grid.contains(&1.7) && grid.contains(&1.2), "{key}");
        assert_eq!(format!("{}", grid[33]), "1.7");
    }
}

/// A write's corridor warning must key on every field that moves a MoonShot's corridor: the
/// Entry group, and `MaxModifier` of the Exit group, which caps the `MShotAdd*` sum too — and on
/// no field the corridor does not read.
#[test]
fn max_modifier_moves_the_entry_like_the_entry_fields() {
    assert!(moves_entry("MaxModifier"));
    assert!(moves_entry("MShotPrice") && moves_entry("MShotAddHourlyDelta"));
    assert!(!moves_entry("SellModifier") && !moves_entry("Add1minDelta"));
    assert!(!moves_entry("SellPrice"));
    // The shared field is read by the entry builder: were it not, the warning would be noise.
    let values: HashMap<String, String> = [
        ("MaxModifier".to_string(), "2".to_string()),
        ("MShotAddHourlyDelta".to_string(), "1".to_string()),
    ]
    .into();
    let defaults = HashMap::new();
    let sv = StrategyValues {
        values: &values,
        defaults: &defaults,
    };
    assert_eq!(
        mshot_params(&sv, ModelSettings::default()).max_modifier,
        2.0
    );
}
