//! Identity round trips cover values a JavaScript number cannot represent.

use crate::automation::Action;
use serde_json::json;

/// A rounded numeric UID must never select a different order.
#[test]
fn identifiers_round_trip_at_full_width_and_reject_numeric_coercion() {
    let action = Action::OrderCancel { uid: u64::MAX };
    let wire = serde_json::to_value(action).unwrap();
    assert_eq!(wire["uid"], "18446744073709551615");
    assert!(matches!(
        serde_json::from_value::<Action>(wire).unwrap(),
        Action::OrderCancel { uid: u64::MAX }
    ));
    for uid in [
        json!(9007199254740992u64),
        json!("18446744073709551616"),
        json!("-1"),
        json!("1e3"),
        json!(" 12"),
    ] {
        assert!(
            serde_json::from_value::<Action>(json!({"kind":"order_cancel","uid":uid})).is_err()
        );
    }
}

/// Optional strategy identity follows the same precision contract, while absence stays explicit.
#[test]
fn optional_strategy_identity_is_exact() {
    let mut wire = json!({"kind":"order_place","market":"BTCUSDT","short":false,"price":100,"balance_amount":25,"use_market_stop":false});
    assert!(matches!(
        serde_json::from_value::<Action>(wire.clone()).unwrap(),
        Action::OrderPlace {
            strategy_id: None,
            ..
        }
    ));
    wire["strategy_id"] = json!("9007199254740993");
    assert!(matches!(
        serde_json::from_value::<Action>(wire).unwrap(),
        Action::OrderPlace {
            strategy_id: Some(9007199254740993),
            ..
        }
    ));
}
