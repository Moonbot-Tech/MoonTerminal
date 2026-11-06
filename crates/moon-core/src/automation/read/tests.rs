//! Hedge exposure must not collapse to the dominant leg used by the compact UI row.

use super::*;

/// A zero one-way field does not erase simultaneous long and short exposure.
#[test]
fn opposite_legs_keep_their_own_quantity_and_entry() {
    let value = moonproto::state::MarketBalancePosition {
        pos_size: 0.0,
        long_pos_size: 1.0,
        short_pos_size: 2.0,
        long_pos_price: 100.0,
        short_pos_price: 120.0,
        ..Default::default()
    };
    let output = position(&value);
    assert_eq!(
        output["long"],
        json!({"size":1.0,"entry":100.0,"liquidation_price":0.0})
    );
    assert_eq!(
        output["short"],
        json!({"size":2.0,"entry":120.0,"liquidation_price":0.0})
    );
}
