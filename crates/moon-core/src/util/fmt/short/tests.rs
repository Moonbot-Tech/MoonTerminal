use super::*;

/// Label in the MoonBot notation.
fn mb(v: f64) -> String {
    short_in(v, Form::Label, Notation::MoonBot)
}

/// Label in the Compact notation.
fn cp(v: f64) -> String {
    short_in(v, Form::Label, Notation::Compact)
}

/// Column in the MoonBot notation.
fn col(v: f64) -> String {
    short_in(v, Form::Column, Notation::MoonBot)
}

/// Scale in the MoonBot notation.
fn sc(v: f64) -> String {
    short_in(v, Form::Scale, Notation::MoonBot)
}

/// The two notations differ in the unit's spelling and nowhere else: the same digits, a lowercase
/// letter after a space against an uppercase one glued on.
///
/// Breakage: a notation that also changed the digits, or a unit table out of order (`k` for a
/// million), fails here.
#[test]
fn notations_spell_the_same_digits_differently() {
    for (v, moonbot, compact) in [
        (1_500.0, "1.5 k", "1.5K"),
        (2_300_000.0, "2.3 m", "2.3M"),
        (1_500_000_000.0, "1.5 b", "1.5B"),
        (2_500_000_000_000.0, "2.5 t", "2.5T"),
    ] {
        assert_eq!(mb(v), moonbot);
        assert_eq!(cp(v), compact);
    }
}

/// The label mantissa keeps its own zeros. Trimming the whole string turned 100 000 into `1K` —
/// a hundredfold understatement on every round value, which is exactly the shape a turnover lands
/// on. Below the first unit nothing is suffixed.
#[test]
fn label_trims_the_fraction_but_never_the_mantissa() {
    assert_eq!(cp(100_000.0), "100K");
    assert_eq!(cp(100_000_000.0), "100M");
    assert_eq!(cp(-100_000.0), "-100K");
    assert_eq!(cp(20_000.0), "20K");
    assert_eq!(cp(2_000_000.0), "2M");
    assert_eq!(mb(1_234.56), "1.23 k");
    assert_eq!(mb(163_400.0), "163 k");
    assert_eq!(mb(999.0), "999");
    assert_eq!(mb(87.5), "87.5");
    assert_eq!(mb(0.0312), "0.0312");
}

/// A label mantissa that rounds up to a thousand belongs to the next unit. Without the carry the
/// label reads `1000K` — a figure no other surface prints for that value.
#[test]
fn label_carries_a_rounded_thousand_into_the_next_unit() {
    assert_eq!(cp(999_600.0), "1M");
    assert_eq!(cp(-999_600.0), "-1M");
    assert_eq!(cp(999_700_000.0), "1B");
    assert_eq!(mb(999_999.0), "1 m");
    // The first unit carries too: two places would print `999.996` as a bare `1000`.
    assert_eq!(mb(999.996), "1 k");
    assert_eq!(mb(-999.996), "-1 k");
    assert_eq!(mb(999.994), "999.99");
    // Below the carry point the unit stays.
    assert_eq!(cp(999_400.0), "999K");
}

/// The column form is MoonBot's report ladder (#1390), checked against the output of the SELECT
/// MoonBot's report runs, with the two departures the terminal decided: a figure that rounds up
/// to the next tier prints there (`999 999` is `1.0 m`, `9999.96` is `10.0 k`) and billions have
/// their letter (`1.5 b`).
///
/// Breakage: two places on every tier leaves `188790000` as wide as before; dropping the carry
/// prints `10000.0`, a fifth whole digit a column has no room for; trimming `.0` breaks the
/// line-up of the points.
#[test]
fn column_follows_moonbots_report_ladder() {
    for (v, expected) in [
        (0.0, "0.0"),
        (3.456, "3.46"),
        (9.0, "9.0"),
        (9.996, "10.0"),
        (10.0, "10.0"),
        (950.0, "950.0"),
        (1_234.56, "1234.6"),
        (9_999.94, "9999.9"),
        (9_999.96, "10.0 k"),
        (10_000.0, "10.0 k"),
        (12_345.0, "12.3 k"),
        (163_400.0, "163.4 k"),
        (999_999.0, "1.0 m"),
        (1_000_000.0, "1.0 m"),
        (12_345_678.9, "12.3 m"),
        (188_790_000.0, "188.8 m"),
        (1_500_000_000.0, "1.5 b"),
        (-1.5, "-1.5"),
        (-12_345.0, "-12.3 k"),
    ] {
        assert_eq!(col(v), expected, "{v}");
    }
    assert_eq!(short_in(12_345.0, Form::Column, Notation::Compact), "12.3K");
}

/// Column rounding is half away from zero, SQLite's `ROUND`, not the half-to-even a bare `{:.2}`
/// applies to the binary value: `0.125` is exactly representable and must print `0.13`.
#[test]
fn column_rounds_half_away_from_zero() {
    assert_eq!(col(0.125), "0.13");
    assert_eq!(col(-0.125), "-0.13");
    // A small negative that rounds to nothing is a zero, never `-0.0`.
    assert_eq!(col(-0.001), "0.0");
}

/// The scale form is TIERED (issue #579): the `k` tier changes its places with the mantissa,
/// every higher tier keeps one, and a zero fraction goes. Replacing it with the label form (five
/// significant digits below a thousand) breaks every line here.
#[test]
fn scale_prints_one_density_per_tier() {
    // Below a thousand: two places and the `k` still there.
    assert_eq!(sc(860.0), "0.86 k");
    assert_eq!(sc(30.0), "0.03 k");
    assert_eq!(sc(500.0), "0.50 k");
    // 1–9.99 k: one place.
    assert_eq!(sc(1_600.0), "1.6 k");
    assert_eq!(sc(8_100.0), "8.1 k");
    // 10–999 k: none.
    assert_eq!(sc(16_000.0), "16 k");
    assert_eq!(sc(163_000.0), "163 k");
    assert_eq!(sc(999_000.0), "999 k");
    // Millions and up: one place at any mantissa.
    assert_eq!(sc(1_240_000.0), "1.2 m");
    assert_eq!(sc(875_300_000.0), "875.3 m");
    assert_eq!(sc(15_400_000_000.0), "15.4 b");
    assert_eq!(sc(2_500_000_000_000.0), "2.5 t");
    assert_eq!(short_in(1_600.0, Form::Scale, Notation::Compact), "1.6K");
}

/// A scale fraction of nothing but zeros is dropped; a zero in the last place of a real fraction
/// stays, and rounding chooses the tier.
///
/// Breakage: trimming through `compact` turns `0.50 k` into `0.5 k`; not trimming prints `1.0 b`;
/// picking the unit from the raw magnitude prints `1000 k`.
#[test]
fn scale_drops_only_an_all_zero_fraction_and_carries() {
    assert_eq!(sc(1_000_000_000.0), "1 b");
    assert_eq!(sc(1_000.0), "1 k");
    assert_eq!(sc(100.0), "0.10 k");
    assert_eq!(sc(999_600.0), "1 m");
    assert_eq!(sc(999_950_000.0), "1 b");
    assert_eq!(sc(9_960.0), "10 k");
    assert_eq!(sc(999.6), "1 k");
    assert_eq!(sc(999_400.0), "999 k");
}

/// Under five units the two-place `k` figure would read `0.00`: the bare number goes out instead,
/// so a BTC-quoted band does not label its turnover as nothing.
#[test]
fn scale_prints_a_figure_too_small_for_k_bare() {
    assert_eq!(sc(0.125), "0.125");
    assert_eq!(sc(4.9), "4.9");
    assert_eq!(sc(0.0), "0");
    assert_eq!(sc(5.0), "0.01 k");
}

/// What is not a number gets no unit in any form: a suffix glued to `inf` reads as a figure.
#[test]
fn non_finite_values_carry_no_unit() {
    for form in [Form::Label, Form::Column, Form::Scale] {
        for notation in Notation::ALL {
            for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let text = short_in(v, form, notation);
                assert!(
                    matches!(text.as_str(), "NaN" | "inf" | "-inf"),
                    "{form:?} {notation:?} {v} -> {text}"
                );
            }
        }
    }
}

/// The persisted code round-trips and an unknown one is refused rather than guessed.
#[test]
fn notation_codes_round_trip() {
    for notation in Notation::ALL {
        assert_eq!(Notation::from_code(notation.code()), Some(notation));
    }
    assert_eq!(Notation::from_code("SI"), None);
    assert_eq!(Notation::default(), Notation::MoonBot);
}
