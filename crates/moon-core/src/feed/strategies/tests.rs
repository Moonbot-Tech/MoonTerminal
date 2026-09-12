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

/// A Russian keyboard types the decimal separator as a COMMA. `parse::<f64>` rejects "0,5", and
/// `unwrap_or(0.0)` used to turn that into a silent zero which the core then answered with its own
/// default — the user saw a parameter he never typed. Reported live on MShotPriceMin/MShotPrice.
#[test]
fn a_decimal_comma_reads_as_a_dot() {
    assert_eq!(
        fv_from_str(Some(&FieldValue::Double(7.0)), None, "0,5"),
        Some(FieldValue::Double(0.5))
    );
    // Also when only the schema knows the type, which is the path for a field the core omitted
    // because it still holds its default.
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Single), " 1,25 "),
        Some(FieldValue::Single(1.25))
    );
}

/// The reason the zero was worse than a refusal: it is a VALID parameter. "no distance", "no
/// stop", "no size" all read as deliberate to the core, so text that is not a number must leave
/// the field alone instead.
#[test]
fn unparsable_text_is_refused_not_zeroed() {
    for text in ["", "  ", "0.5%", "0 5", "abc", "-", "1,2,3"] {
        assert_eq!(
            fv_from_str(Some(&FieldValue::Double(7.0)), None, text),
            None,
            "{text:?} must not become a number"
        );
    }
    // A fraction is not an integer: truncating it would send a value the user did not type.
    assert_eq!(fv_from_str(Some(&FieldValue::Int32(3)), None, "2.5"), None);
}

/// Out of range is refused rather than wrapped: `300 as u8` used to reach the core as 44, and
/// a byte field holding 44 is indistinguishable from one the user set to 44 on purpose.
#[test]
fn out_of_range_is_refused_not_wrapped() {
    assert_eq!(fv_from_str(Some(&FieldValue::Byte(1)), None, "300"), None);
    assert_eq!(fv_from_str(Some(&FieldValue::Word(1)), None, "70000"), None);
    assert_eq!(
        fv_from_str(Some(&FieldValue::Int32(1)), None, "3000000000"),
        None
    );
    assert_eq!(fv_from_str(Some(&FieldValue::UInt32(1)), None, "-1"), None);
    // The boundary itself is a value.
    assert_eq!(
        fv_from_str(Some(&FieldValue::Byte(1)), None, "255"),
        Some(FieldValue::Byte(255))
    );
}

/// A checkbox has two states and a string field takes any text, so neither can refuse: only the
/// numeric conversions gained a way to say no.
#[test]
fn bool_and_string_stay_total() {
    assert_eq!(
        fv_from_str(Some(&FieldValue::Bool(true)), None, "nonsense"),
        Some(FieldValue::Bool(false))
    );
    assert_eq!(
        fv_from_str(Some(&FieldValue::Bool(false)), None, "Yes"),
        Some(FieldValue::Bool(true))
    );
    assert_eq!(
        fv_from_str(None, None, "Last1hDelta"),
        Some(FieldValue::String("Last1hDelta".into()))
    );
}

/// Text that parses perfectly well and still is not a value of the field: `1e400` becomes `inf`,
/// `1e-400` a real `0.0`, and an f64 that survives its own type underflows to `0.0f32` on the way
/// into a Single field. The zeros are the silent zero this function exists to stop; the infinity is
/// a threshold that would compare false to everything the core measures against it.
#[test]
fn a_number_that_parses_but_is_not_a_value_is_refused() {
    // Overflow: `1e400` parses to `inf`, and `1e300` survives f64 only to overflow f32.
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Double), "1e400"),
        None
    );
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Single), "1e300"),
        None
    );
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Double), "1e-400"),
        None
    );
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Single), "1e-60"),
        None
    );
    // A zero the user did type stays a zero, however it is spelled.
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Double), "0,000"),
        Some(FieldValue::Double(0.0))
    );
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Single), "-0.0"),
        Some(FieldValue::Single(-0.0))
    );
}

/// The panel's marker and the sender must agree, or a row reads as accepted and is then dropped on
/// the way out. This compares the two ACROSS both of the sender's branches — the schema type and
/// the type of the value the core last sent — because `field_text_is_valid` can only consult the
/// first, and a check that walked one branch would agree with itself by construction.
#[test]
fn the_ui_check_agrees_with_the_conversion_on_both_branches() {
    let cases: [(&str, StrategyFieldType, FieldValue); 6] = [
        ("Double", StrategyFieldType::Double, FieldValue::Double(1.0)),
        ("Single", StrategyFieldType::Single, FieldValue::Single(1.0)),
        ("Int32", StrategyFieldType::Int32, FieldValue::Int32(1)),
        ("Byte", StrategyFieldType::Byte, FieldValue::Byte(1)),
        ("Word", StrategyFieldType::Word, FieldValue::Word(1)),
        ("UInt64", StrategyFieldType::UInt64, FieldValue::UInt64(1)),
    ];
    for text in [
        "0,5", "0.5", "0,5%", "2.5", "300", "70000", "-1", "", "abc", "1e-400", "12",
    ] {
        for (name, stype, existing) in &cases {
            let marker = field_text_is_valid(name, text);
            assert_eq!(
                marker,
                fv_from_str(None, Some(*stype), text).is_some(),
                "{name} {text:?}: marker disagrees with the schema-type branch"
            );
            assert_eq!(
                marker,
                fv_from_str(Some(existing), None, text).is_some(),
                "{name} {text:?}: marker disagrees with the stored-value branch"
            );
        }
    }
    // A string field takes anything, and so does a type name this build does not know.
    assert!(field_text_is_valid("String", "anything at all"));
    assert!(field_text_is_valid("Unknown", ""));
}

/// A `SoundKind` value is a file name, not an entry in a list this crate keeps.
///
/// Plausible breakage: reinstating a whitelist of the embedded stems here would turn a user's own
/// `MYSOUND.wav` — legitimately named by a Moonbot strategy — back into "no sound field", which the
/// caller then reads as the schema default. The player is the one place that knows which names
/// exist, and it reports a missing one rather than swallowing it.
#[test]
fn a_sound_kind_is_normalized_not_filtered() {
    assert_eq!(sound_stem("BABYTOY"), Some("babytoy".into()));
    assert_eq!(sound_stem(" ding1.wav "), Some("ding1".into()));
    assert_eq!(sound_stem("MySound.WAV"), Some("mysound".into()));
    assert_eq!(
        sound_stem("NotEmbedded"),
        Some("notembedded".into()),
        "an unknown name must travel to the player, which reports it; dropping it here is silent"
    );
}

/// `NONE` and an empty value both mean silence, in either spelling Moonbot uses.
#[test]
fn none_and_empty_mean_silence() {
    assert_eq!(sound_stem("NONE"), None);
    assert_eq!(sound_stem("none"), None);
    assert_eq!(sound_stem(""), None);
    assert_eq!(sound_stem("   "), None);
    assert_eq!(
        sound_stem(".wav"),
        None,
        "an extension with no stem names nothing"
    );
}
