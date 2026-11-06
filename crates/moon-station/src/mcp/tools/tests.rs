//! Validate model input against the advertised schema before an owner-thread command can run.

use super::*;

/// Optional invalid values must not silently turn into defaults or truncate across integer widths.
#[test]
fn schemas_reject_wrong_types_extra_keys_and_out_of_range_arguments() {
    for (name, arguments) in [
        ("strategy_schema", json!({"core":"1","strategy_kind":256})),
        ("strategy_list", json!({"core":"1","offset":"0"})),
        ("station_overview", json!({"cores":[42]})),
        ("reports_query", json!({"cores":[],"from_ms":1,"to_ms":2})),
        ("logs_start", json!({"cores":["1"],"seconds":1000})),
        (
            "action_preview",
            json!({"core":"1","action":{"kind":"order_place","market":"BTCUSDT","short":false,"price":100,"base_quantity":1,"use_market_stop":false}}),
        ),
    ] {
        let (_, validator) = catalog()
            .iter()
            .find(|(tool, _)| tool.name == name)
            .unwrap();
        assert!(!validator.is_valid(&arguments), "{name}");
    }
}

/// The real order interface accepts a balance-currency amount and excludes the old incorrect unit.
#[test]
fn order_amount_schema_agrees_with_the_typed_action() {
    let (_, validator) = catalog()
        .iter()
        .find(|(tool, _)| tool.name == "action_preview")
        .unwrap();
    let arguments = json!({"core":"1","action":{"kind":"order_place","market":"BTCUSDT","short":false,"price":100,"balance_amount":25,"use_market_stop":false}});
    assert!(validator.is_valid(&arguments));
    let action: Action = serde_json::from_value(arguments["action"].clone()).unwrap();
    assert!(matches!(
        action,
        Action::OrderPlace {
            balance_amount: 25.0,
            ..
        }
    ));
}
