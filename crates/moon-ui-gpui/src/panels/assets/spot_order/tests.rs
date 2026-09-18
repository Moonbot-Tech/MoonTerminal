//! Unit tests for the spot `Order` dialog's seeds and parsing.

// Explicit imports, NOT `use super::*`: the parent re-exports `gpui::*`, which carries its
// own `test` and shadows the built-in attribute — `#[test]` then expands recursively.
use super::{fmt_seed, parse_positive, seed_sell_price};

/// The price seed is Moonbot's `Fixed` figure: live price moved by the core's main TP percent.
///
/// Mutation: drop the percent, or apply it as a fraction. The window then opens at the current
/// price (an order that fills at once, not the take-profit the trader expects) or at 4x it.
#[test]
fn the_sell_price_seed_is_the_live_price_plus_the_main_take_profit() {
    let seed = seed_sell_price(9.83, 3.0).expect("a live price seeds");
    assert!(
        (seed - 10.1249).abs() < 1e-9,
        "9.83 * 1.03 = 10.1249, got {seed}"
    );
    assert_eq!(seed_sell_price(2.0, 0.0), Some(2.0));
    assert_eq!(seed_sell_price(2.0, f64::NAN), Some(2.0));
}

/// No live price means an EMPTY field, never a zero: `price=0` is the input that once made the
/// core substitute the lot step for the quantity (2026-09-03).
#[test]
fn a_missing_or_zero_live_price_seeds_nothing() {
    assert_eq!(seed_sell_price(0.0, 3.0), None);
    assert_eq!(seed_sell_price(f64::NAN, 3.0), None);
    assert_eq!(seed_sell_price(-1.0, 3.0), None);
    assert_eq!(fmt_seed(0.0), "");
    assert_eq!(fmt_seed(f64::INFINITY), "");
}

/// Seeds render as plain decimals without trailing zeroes, so the field is editable as typed.
#[test]
fn seeds_render_as_plain_decimals() {
    assert_eq!(fmt_seed(10.1249), "10.1249");
    assert_eq!(fmt_seed(1.28974853), "1.28974853");
    assert_eq!(fmt_seed(0.00000123), "0.00000123");
}

/// OK accepts a comma decimal and refuses anything that is not a finite positive number.
///
/// Mutation: accept zero. The core then invents a quantity from a zero price (2026-09-03 log).
#[test]
fn ok_takes_only_finite_positive_numbers() {
    assert_eq!(parse_positive("10,125"), Some(10.125));
    assert_eq!(parse_positive(" 1.5 "), Some(1.5));
    assert_eq!(parse_positive("0"), None);
    assert_eq!(parse_positive("-3"), None);
    assert_eq!(parse_positive("abc"), None);
    assert_eq!(parse_positive(""), None);
    assert_eq!(parse_positive("inf"), None);
}

/// A micro price keeps its significant digits: a fixed eight decimals printed 4e-9 as "0", the one
/// seed the dialog must never produce (mutation: go back to `{v:.8}`).
#[test]
fn micro_prices_keep_their_significant_digits() {
    assert_eq!(fmt_seed(4.12e-9), "0.00000000412");
    assert_eq!(fmt_seed(0.000000123), "0.000000123");
    assert_eq!(fmt_seed(1e-18), "0.000000000000000001");
    assert_ne!(
        fmt_seed(1e-20),
        "0",
        "a value f64 cannot print stays empty, never zero"
    );
}
