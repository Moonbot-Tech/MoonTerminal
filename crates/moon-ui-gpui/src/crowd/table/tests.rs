use super::{DeltaSign, dollars, signed_compact};

#[test]
fn a_day_figure_carries_one_sign_and_one_rounding() {
    // The magnitude is whatever is PRINTED, and the sign is classified off that rather than off
    // the raw value — so the digits and the colour cannot disagree.
    assert_eq!(
        signed_compact(6624.66),
        ("+6.62K".to_string(), DeltaSign::Positive)
    );
    assert_eq!(
        signed_compact(-1954.69),
        ("-1.95K".to_string(), DeltaSign::Negative)
    );
    // A literal negative zero must not print a minus and must not be coloured as a loss: that is
    // the misreading this classification exists to remove.
    assert_eq!(
        signed_compact(-0.0),
        ("0.00".to_string(), DeltaSign::Zero),
        "a negative zero wore a sign"
    );
    assert_eq!(signed_compact(0.0).1, DeltaSign::Zero);
}

#[test]
fn every_figure_is_written_to_the_hundredth() {
    // One shape per column: a row printing "2", the next "0.1" and the next "10K" gives the eye
    // three shapes to compare before it reaches the numbers.
    assert_eq!(dollars(2.0), "2.00");
    assert_eq!(dollars(0.1), "0.10");
    assert_eq!(dollars(42.523), "42.52");
    assert_eq!(dollars(10_000.0), "10.00K");
    assert_eq!(dollars(1_234_567.0), "1.23M");
    // The suffix arrives at a thousand, so the whole part never outgrows three digits and a sign —
    // which is exactly the column this table reserves for it.
    assert_eq!(dollars(999.99), "999.99");
    assert_eq!(dollars(1_000.0), "1.00K");
}
