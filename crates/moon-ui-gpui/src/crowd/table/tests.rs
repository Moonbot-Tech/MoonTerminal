use super::{DeltaSign, FRAC_CHARS, INT_CHARS, dollars, signed_compact};

#[test]
fn a_day_figure_carries_one_sign_and_one_rounding() {
    // The magnitude is whatever is PRINTED, and the sign is classified off that rather than off
    // the raw value — so the digits and the colour cannot disagree.
    assert_eq!(
        signed_compact(6624.66),
        ("+6624.7".to_string(), DeltaSign::Positive)
    );
    assert_eq!(
        signed_compact(-19_546.9),
        ("-19.5 k".to_string(), DeltaSign::Negative)
    );
    // A literal negative zero must not print a minus and must not be coloured as a loss: that is
    // the misreading this classification exists to remove.
    assert_eq!(
        signed_compact(-0.0),
        ("0.0".to_string(), DeltaSign::Zero),
        "a negative zero wore a sign"
    );
    assert_eq!(signed_compact(0.0).1, DeltaSign::Zero);
    // A loss too small for the two places under ten prints as nothing lost.
    assert_eq!(signed_compact(-0.001).1, DeltaSign::Zero);
}

#[test]
fn every_figure_carries_a_point() {
    // One shape per column: a row printing "2", the next "0.1" and the next "10K" gives the eye
    // three shapes to compare before it reaches the numbers. The column form always has a point.
    assert_eq!(dollars(2.0), "2.0");
    assert_eq!(dollars(0.1), "0.1");
    assert_eq!(dollars(3.456), "3.46");
    assert_eq!(dollars(42.523), "42.5");
    assert_eq!(dollars(999.99), "1000.0");
    assert_eq!(dollars(10_000.0), "10.0 k");
    assert_eq!(dollars(1_234_567.0), "1.2 m");
}

/// Every figure the column form can print fits the halves this table reserves around the point.
///
/// Breakage: a form that kept two places above ten, or a unit wider than a space and a letter,
/// overflows `FRAC_CHARS` and pushes the next column sideways.
#[test]
fn every_figure_fits_the_reserved_halves() {
    // The edges first: a figure rounding up to ten thousand is where a fifth whole digit would come
    // from, and a unit's space and letter are where the fraction is widest.
    let edges = [
        9_999.96, -9_999.96, 9_999.94, 999_999.0, 12_345.0, 9.996, 0.0,
    ];
    let sweep = std::iter::successors(Some(0.001_f64), |v| (*v < 1e13).then_some(v * 3.7))
        .flat_map(|v| [v, -v, v * 9.999]);
    for value in edges.into_iter().chain(sweep) {
        let (text, _) = signed_compact(value);
        let point = text.find('.').expect("the column form always has a point");
        // The whole half holds the sign; the fraction half holds the point, the places and a unit.
        assert!(point as f32 <= INT_CHARS, "{value} -> {text}");
        assert!(
            (text.len() - point) as f32 <= FRAC_CHARS,
            "{value} -> {text}"
        );
    }
}
