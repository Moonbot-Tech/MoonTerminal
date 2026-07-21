use super::*;

#[test]
fn compact_keeps_integer_zeros() {
    // Регрессия: слепой трим нулей калечил целые (330 → «33», 1000 → «1»).
    assert_eq!(compact(330.0, 0), "330");
    assert_eq!(compact(1000.0, 0), "1000");
    assert_eq!(compact(0.0, 0), "0");
    assert_eq!(compact(-500.0, 0), "-500");
    // Fractional trailing zeros are trimmed.
    assert_eq!(compact(1.5, 6), "1.5");
    assert_eq!(compact(2.0, 6), "2");
    assert_eq!(compact(45.20, 2), "45.2");
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
