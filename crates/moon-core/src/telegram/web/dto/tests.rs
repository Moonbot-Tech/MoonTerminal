//! Wire-format regressions for shared Mini App response allocations.

use super::*;

/// Moving Orders rows from Vec to Arc must not change the JSON or deep-clone a cached response.
#[test]
fn orders_shared_rows_keep_the_wire_array_and_clone_cheaply() {
    let dto = OrdersDto {
        orders: Vec::new().into(),
        can_control: true,
    };
    assert_eq!(
        serde_json::to_string(&dto).expect("serialize Orders"),
        r#"{"orders":[],"can_control":true}"#
    );
    let cloned = dto.clone();
    assert!(std::sync::Arc::ptr_eq(&dto.orders, &cloned.orders));
}
