//! Machine-facing values must not inherit the UI's six-decimal formatting loss.

use super::*;

/// UI precision and JavaScript number limits must not change model-visible strategy values.
#[test]
fn exact_values_survive_unrelated_strategy_fields_and_javascript_integer_limits() {
    assert_eq!(
        value(&FieldValue::Double(0.000000123456789)),
        json!(0.000000123456789)
    );
    assert_eq!(
        value(&FieldValue::UInt64(18_446_744_073_709_551_615)),
        json!("18446744073709551615")
    );
    assert_eq!(
        value(&FieldValue::Int64(-9_223_372_036_854_775_808)),
        json!("-9223372036854775808")
    );
}
