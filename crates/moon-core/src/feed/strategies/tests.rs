use super::*;

/// `0` in KeepInChart/AddToChart is a MEANING, so garbage has to yield `None` — and with it the
/// caller's default — rather than quietly folding to zero and reading as one of those meanings.
#[test]
fn garbage_is_none_not_zero() {
    assert_eq!(field_num(&FieldValue::Double(f64::NAN)), None);
    assert_eq!(field_num(&FieldValue::Single(f32::NAN)), None);
    assert_eq!(field_num(&FieldValue::Double(f64::INFINITY)), None);
    assert_eq!(field_num(&FieldValue::Int32(-1)), None);
    assert_eq!(field_num(&FieldValue::Int64(-1)), None);
    assert_eq!(field_num(&FieldValue::String("60".into())), None);
}

/// Too large is garbage too: neither wrapping modulo 2^32, which would fake a `0`, nor
/// saturating, which would open AddToChart tab number 4294967295.
#[test]
fn oversized_is_none_neither_wraps_nor_saturates() {
    assert_eq!(field_num(&FieldValue::UInt64(1u64 << 32)), None);
    assert_eq!(field_num(&FieldValue::Int64(1i64 << 32)), None);
    assert_eq!(field_num(&FieldValue::Double(1e30)), None);
    // The upper bound itself is a value, not garbage.
    assert_eq!(
        field_num(&FieldValue::UInt64(u32::MAX as u64)),
        Some(u32::MAX)
    );
}

#[test]
fn plain_values_pass_through() {
    assert_eq!(field_num(&FieldValue::Int32(0)), Some(0));
    assert_eq!(field_num(&FieldValue::Int32(60)), Some(60));
    assert_eq!(field_num(&FieldValue::Double(60.0)), Some(60));
    assert_eq!(field_num(&FieldValue::Bool(true)), Some(1));
}
