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

/// A loss too small to survive rounding must render UNSIGNED and classify [`DeltaSign::Zero`], so
/// the text and the colour a caller picks from that sign cannot disagree.
///
/// Breakage: signing the zero branch — `format!("{:+.*}", ..)` for every arm, or classifying `v`
/// instead of `rounded`. Either paints a red, minus-signed `-0.00` on a figure that reads as
/// break-even, which is the visible symptom this helper exists to remove.
#[test]
fn signed_fixed_renders_a_rounded_away_loss_unsigned_and_zero() {
    assert_eq!(
        signed_fixed(-0.004, 2).unwrap(),
        ("0.00".to_string(), DeltaSign::Zero),
        "a loss that rounds away is neither minus-signed nor negative"
    );
    // A gain that rounds away must not claim a "+" either.
    assert_eq!(
        signed_fixed(0.004, 2).unwrap(),
        ("0.00".to_string(), DeltaSign::Zero)
    );
    // Literal -0.0 is canonicalized rather than printed with its minus.
    assert_eq!(
        signed_fixed(-0.0, 2).unwrap(),
        ("0.00".to_string(), DeltaSign::Zero)
    );
    assert_eq!(
        signed_fixed(0.0, 2).unwrap(),
        ("0.00".to_string(), DeltaSign::Zero)
    );
}

/// The whole reason this sibling of [`signed_amount`] exists: a right-aligned money COLUMN needs
/// every decimal place, and `signed_amount` trims them through `compact`.
///
/// Breakage: routing `signed_fixed` through `compact` like its sibling. The two assertions are
/// written as a PAIR so what is pinned is the DIFFERENCE between them — one of them alone would
/// still pass if the helpers were merged back into one.
#[test]
fn signed_fixed_keeps_the_places_signed_amount_trims() {
    assert_eq!(signed_fixed(12.0, 2).unwrap().0, "+12.00");
    assert_eq!(signed_amount(12.0, 2).0, "+12");
    assert_eq!(signed_fixed(12.5, 2).unwrap().0, "+12.50");
    assert_eq!(signed_amount(12.5, 2).0, "+12.5");
    assert_ne!(
        signed_fixed(12.0, 2).unwrap().0,
        signed_amount(12.0, 2).0,
        "the fixed form must not collapse back onto the trimmed one"
    );
}

/// A value with a sign keeps its explicit `+`/`-` at exactly the requested precision.
#[test]
fn signed_fixed_signs_both_directions_at_the_requested_precision() {
    assert_eq!(
        signed_fixed(3.5, 2).unwrap(),
        ("+3.50".to_string(), DeltaSign::Positive)
    );
    assert_eq!(
        signed_fixed(-3.5, 2).unwrap(),
        ("-3.50".to_string(), DeltaSign::Negative)
    );
    // The precision is the caller's, not a fixed two places.
    assert_eq!(signed_fixed(-3.5, 0).unwrap().0, "-4");
    assert_eq!(signed_fixed(3.5, 4).unwrap().0, "+3.5000");
}

/// A non-finite amount returns `None` so each caller supplies its own placeholder.
///
/// Breakage: `unwrap_or(0.0)` on the rounding, as [`signed_amount`] does. A money column would
/// then print a confident `0.00` — or, without the guard at all, a literal `NaN` — where the
/// figure is simply unknown.
#[test]
fn signed_fixed_rejects_what_cannot_be_an_amount() {
    assert!(signed_fixed(f64::NAN, 2).is_none());
    assert!(signed_fixed(f64::INFINITY, 2).is_none());
    assert!(signed_fixed(f64::NEG_INFINITY, 2).is_none());
    // A finite input whose scaling overflows the rounding is rejected too.
    assert!(signed_fixed(f64::MAX, 2).is_none());
    // Just inside the range: still a real number, still formatted.
    assert!(signed_fixed(1e300, 2).is_some());
}

/// Rounding precedes the sign choice, and it is half-AWAY-from-zero, as `pct` and `signed_amount`
/// already are.
///
/// Breakage: classifying before rounding (the `0.004` cases regain a sign), or dropping
/// [`round_to`] and letting `{:+.*}` round on its own — `{:.0}` is half-to-EVEN, so `2.5` would
/// print `+2` and `-0.5` would print `-0`, disagreeing with every other formatter in this module.
#[test]
fn signed_fixed_rounds_before_choosing_the_sign() {
    // Just under half a unit in the last place: no sign survives.
    assert_eq!(
        signed_fixed(-0.00499, 2).unwrap(),
        ("0.00".to_string(), DeltaSign::Zero)
    );
    assert_eq!(
        signed_fixed(0.00499, 2).unwrap(),
        ("0.00".to_string(), DeltaSign::Zero)
    );
    // Exactly half a unit: rounds away from zero and keeps the sign it earned.
    assert_eq!(
        signed_fixed(0.005, 2).unwrap(),
        ("+0.01".to_string(), DeltaSign::Positive)
    );
    assert_eq!(
        signed_fixed(2.5, 0).unwrap(),
        ("+3".to_string(), DeltaSign::Positive),
        "midpoints round away from zero, not to even"
    );
    assert_eq!(
        signed_fixed(-0.5, 0).unwrap(),
        ("-1".to_string(), DeltaSign::Negative),
        "midpoints round away from zero, not to even"
    );
}

/// Regression guard: in `fmt.rs:core_build`, changing the minor formatter from `{:02}` to `{}`
/// turns build `707` into `7.7`, so a reader can mistake it for older than a neighbouring `7.09`.
///
/// A one-digit minor is the boundary that needs the pad; `710` confirms that the product keeps
/// the same two-digit wire convention immediately beyond it.
#[test]
fn core_build_zero_pads_single_digit_minor_versions() {
    assert_eq!(core_build(707), "7.07");
    assert_eq!(core_build(710), "7.10");
}
