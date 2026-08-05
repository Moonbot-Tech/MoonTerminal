//! Regression tests for the shared exchange-name presentation policy.

use super::exchange_display_name_with_spot;

/// `exchange_label.rs:exchange_display_name_with_spot` must append the market type only to
/// unsuffixed spot names; appending it unconditionally makes the last two assertions red and
/// renders duplicated or contradictory exchange labels across the application.
#[test]
fn display_name_makes_spot_explicit_once() {
    assert_eq!(
        exchange_display_name_with_spot("BitGet", "Spot"),
        "BitGet Spot"
    );
    assert_eq!(
        exchange_display_name_with_spot("Hyper", "Spot"),
        "Hyper Spot"
    );
    assert_eq!(
        exchange_display_name_with_spot("Bybit Spot", "Spot"),
        "Bybit Spot"
    );
    assert_eq!(
        exchange_display_name_with_spot("Hyper Futures", "Spot"),
        "Hyper Futures"
    );
    assert_eq!(
        exchange_display_name_with_spot("  Hyper  ", "Spot"),
        "Hyper Spot"
    );
    assert_eq!(exchange_display_name_with_spot("   ", "Spot"), "");
}
