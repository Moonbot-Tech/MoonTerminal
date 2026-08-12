use super::*;

/// Regression guard: in `fmt.rs:compact`, removing the `!s.contains('.')` early-return guard makes
/// the 330 and 1000 assertions fail by truncating user-visible integers to "33" and "1".
#[test]
fn compact_keeps_integer_zeros() {
    assert_eq!(compact(330.0, 0), "330");
    assert_eq!(compact(1000.0, 0), "1000");
    assert_eq!(compact(0.0, 0), "0");
    assert_eq!(compact(-500.0, 0), "-500");
    // Fractional trailing zeros are trimmed.
    assert_eq!(compact(1.5, 6), "1.5");
    assert_eq!(compact(2.0, 6), "2");
    assert_eq!(compact(45.20, 2), "45.2");
}

/// The SI mantissa must keep its own zeros. Trimming the whole string turned 100_000 into "1K"
/// and 100_000_000 into "1M" — a hundredfold understatement on every round value, which is exactly
/// the shape a turnover figure lands on.
#[test]
fn compact_si_trims_the_fraction_but_never_the_mantissa() {
    assert_eq!(compact_si(100_000.0), "100K");
    assert_eq!(compact_si(100_000_000.0), "100M");
    assert_eq!(compact_si(-100_000.0), "-100K");
    assert_eq!(compact_si(20_000.0), "20K");
    // Fractional zeros still go.
    assert_eq!(compact_si(1_500.0), "1.5K");
    assert_eq!(compact_si(2_300_000.0), "2.3M");
    assert_eq!(compact_si(2_000_000.0), "2M");
    // Below the first unit nothing is suffixed.
    assert_eq!(compact_si(999.0), adaptive(999.0));
}

#[test]
fn adaptive_thousands_intact() {
    assert_eq!(adaptive(25000.0), "25000");
    assert_eq!(adaptive(1000.0), "1000");
}

#[test]
fn group_thousands_splits_by_three() {
    assert_eq!(group_thousands("1111"), "1 111");
    assert_eq!(group_thousands("999"), "999");
    assert_eq!(group_thousands("1000000"), "1 000 000");
    assert_eq!(group_thousands(""), "");
}

#[test]
fn group_thousands_keeps_sign_out_of_the_grouping() {
    // The leading sign does not participate in three-digit grouping.
    assert_eq!(group_thousands("-123456"), "-123 456");
    assert_eq!(group_thousands("-1234"), "-1 234");
    assert_eq!(group_thousands("-999"), "-999");
}

#[test]
fn signed_pct_rounds_before_signing() {
    assert_eq!(signed_pct(2.04, 1).unwrap().0, "+2.0%");
    assert_eq!(signed_pct(-0.31, 1).unwrap().0, "-0.3%");
    // A small negative rounds to canonical, unsigned zero.
    assert_eq!(
        signed_pct(-0.04, 1).unwrap(),
        ("0.0%".to_string(), DeltaSign::Zero)
    );
    // Literal -0.0 is canonicalized and classified as zero.
    assert_eq!(
        signed_pct(-0.0, 1).unwrap(),
        ("0.0%".to_string(), DeltaSign::Zero)
    );
    assert_eq!(signed_pct(0.0, 2).unwrap().0, "0.00%");
}

#[test]
fn signed_pct_rejects_what_cannot_be_a_percentage() {
    assert!(signed_pct(f64::NAN, 1).is_none());
    assert!(signed_pct(f64::INFINITY, 1).is_none());
    // A finite input is rejected when scaling makes the rounded result non-finite.
    assert!(signed_pct(f64::MAX, 1).is_none());
    assert!(signed_pct(f64::MAX / 5.0, 1).is_none());
    // Just inside the range: still a real number, still formatted.
    assert!(signed_pct(1e300, 1).is_some());
}

#[test]
fn group_decimal_groups_only_the_integer_part() {
    assert_eq!(group_decimal("19983.48"), "19 983.48");
    assert_eq!(group_decimal("999.5"), "999.5");
}

#[test]
fn usd_grouped_is_one_precision_for_every_amount_surface() {
    // The header balance and the Assets panel must not drift on the same figure.
    assert_eq!(usd_grouped(19983.48), "19 983.48");
    assert_eq!(usd_grouped(19983.50), "19 983.5");
    assert_eq!(usd_grouped(-1234.0), "-1 234.0");
}

#[test]
fn pct_shares_the_signed_rounding_without_the_sign() {
    assert_eq!(pct(2.04, 1).unwrap().0, "2.0%");
    assert_eq!(pct(-0.31, 1).unwrap().0, "-0.3%");
    // The no-forced-plus form also canonicalizes a rounded zero.
    assert_eq!(
        pct(-0.04, 1).unwrap(),
        ("0.0%".to_string(), DeltaSign::Zero)
    );
    assert!(pct(f64::NAN, 1).is_none());
}

/// A loss too small to survive rounding must not keep its minus sign or its negative
/// classification, and the returned sign must always describe the string beside it.
///
/// Breakage: taking the sign from the raw value — `let sign = if v < 0.0 { "-" } else { "+" }`
/// before rounding, or returning `classify(v)` instead of `classify(rounded)`. Either renders
/// `-0.001` as a red "-0.00": a figure that reads as zero while being coloured and signed as a
/// loss. The `1.5` case pins the half-away-from-zero rule this module shares with `pct`, so a
/// switch to `{:.*}`'s half-to-even cannot slip in unnoticed.
#[test]
fn signed_amount_takes_its_sign_from_the_rounded_value() {
    assert_eq!(
        signed_amount(-0.001, 2),
        ("+0".to_string(), DeltaSign::Zero),
        "a loss that rounds away is neither negative nor minus-signed"
    );
    assert_eq!(
        signed_amount(-0.02, 2),
        ("-0.02".to_string(), DeltaSign::Negative)
    );
    assert_eq!(
        signed_amount(12.5, 2),
        ("+12.5".to_string(), DeltaSign::Positive)
    );
    assert_eq!(
        signed_amount(1.5, 0),
        ("+2".to_string(), DeltaSign::Positive),
        "midpoints round away from zero, as pct() already does"
    );
    assert_eq!(
        signed_amount(f64::NAN, 2),
        ("+0".to_string(), DeltaSign::Zero),
        "a non-finite amount has no sign worth stating"
    );
}
