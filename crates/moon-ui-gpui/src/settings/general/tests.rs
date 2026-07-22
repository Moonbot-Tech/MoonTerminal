use super::{font_delta_text, parse_font_delta};

#[test]
fn text_is_integer_canonical() {
    assert_eq!(font_delta_text(2.0), "2");
    assert_eq!(font_delta_text(-2.0), "-2");
    assert_eq!(font_delta_text(0.0), "0");
    assert_eq!(font_delta_text(-0.0), "0"); // No negative-zero representation.
    assert_eq!(font_delta_text(2.6), "3"); // Rounded to the integer step.
    assert_eq!(font_delta_text(2.4), "2");
}

#[test]
fn parse_rounds_and_clamps() {
    assert_eq!(parse_font_delta("3"), Some(3.0));
    assert_eq!(parse_font_delta("2.6"), Some(3.0)); // Rounded to the integer step.
    assert_eq!(parse_font_delta("2.4"), Some(2.0));
    assert_eq!(parse_font_delta("2,4"), Some(2.0)); // A comma is accepted as the decimal point.
    assert_eq!(parse_font_delta("+4"), Some(4.0));
    assert_eq!(parse_font_delta("  5  "), Some(5.0)); // trim
    assert_eq!(parse_font_delta("10"), Some(6.0)); // Clamped at the upper bound.
    assert_eq!(parse_font_delta("-9"), Some(-2.0)); // Clamped at the lower bound.
}

#[test]
fn parse_rejects_garbage_and_nonfinite() {
    assert_eq!(parse_font_delta(""), None);
    assert_eq!(parse_font_delta("-"), None); // Incomplete input.
    assert_eq!(parse_font_delta("abc"), None);
    assert_eq!(parse_font_delta("nan"), None); // NaN must not reach the configuration.
    assert_eq!(parse_font_delta("inf"), None); // Infinity must not reach it either.
}

#[test]
fn parse_normalizes_negative_zero() {
    // Normalize "-0" to canonical +0.0 so IEEE negative zero does not reach settings.toml.
    let v = parse_font_delta("-0").unwrap();
    assert_eq!(v, 0.0);
    assert!(v.is_sign_positive());
}
