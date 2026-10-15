//! The knobs and what writing one moves.

use super::*;

/// Omitted dump fields use the absolute step, while an explicit relative step survives parsing.
#[test]
fn price_down_relative_uses_the_core_default_when_omitted() {
    let defaults = HashMap::new();
    for (raw, expected) in [
        (r#"{"StrategyName":"UNIT_01","Coins":"XALPHA"}"#, false),
        (
            r#"{"StrategyName":"UNIT_01","Coins":"XALPHA","PriceDownRelative":true}"#,
            true,
        ),
    ] {
        let json: HashMap<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
        let values = json
            .into_iter()
            .map(|(key, value)| (key, value.to_string()))
            .collect();
        let strategy = StrategyValues {
            values: &values,
            defaults: &defaults,
        };
        assert_eq!(
            exit_params(&strategy, ModelSettings::default()).price_down_relative,
            expected,
            "{raw}"
        );
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
