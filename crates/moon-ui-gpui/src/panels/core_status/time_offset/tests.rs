//! Tz-offset presentation: the cell formatter, the never-measured marker, the sort rank, and the
//! hover being built FROM the facts rather than restated independently.

use super::*;

/// A positive whole-hour offset renders with the ASCII sign and two-digit fields.
#[test]
fn a_positive_offset_renders_utc_plus_two_digits() {
    let text = tz_offset_cell_text(TzOffsetCell::Measured { offset_secs: 7_200 });
    assert_eq!(text, "UTC+02:00");
}

/// A measured zero renders as `UTC+00:00`, never as the never-measured marker — that distinction
/// is the whole reason `offset_secs` is `Option<i32>` rather than a bare number.
#[test]
fn a_measured_zero_renders_as_a_real_offset() {
    let text = tz_offset_cell_text(TzOffsetCell::Measured { offset_secs: 0 });
    assert_eq!(text, "UTC+00:00");
}

/// A negative offset uses the ASCII minus, not a Unicode dash.
#[test]
fn a_negative_offset_renders_utc_minus_two_digits() {
    let text = tz_offset_cell_text(TzOffsetCell::Measured {
        offset_secs: -14_400,
    });
    assert_eq!(text, "UTC-04:00");
}

/// A quarter-hour offset (the estimator buckets at 900 s) keeps its minutes rather than rounding
/// to the hour.
#[test]
fn a_quarter_hour_offset_keeps_its_minutes() {
    let text = tz_offset_cell_text(TzOffsetCell::Measured {
        offset_secs: 20_700,
    });
    assert_eq!(text, "UTC+05:45");
}

/// `Unknown` never renders with the `UTC` prefix: a core that runs on UTC and a core nobody has
/// measured must not read the same.
#[test]
fn unknown_never_renders_as_utc() {
    let text = tz_offset_cell_text(TzOffsetCell::Unknown);
    assert!(!text.starts_with("UTC"), "unknown text was {text:?}");
}

/// `tz_offset_rank`: every measured value, whatever its sign, sorts before `Unknown`.
#[test]
fn the_rank_puts_unknown_last() {
    let mut cells = vec![
        TzOffsetCell::Unknown,
        TzOffsetCell::Measured { offset_secs: 7_200 },
        TzOffsetCell::Measured {
            offset_secs: -14_400,
        },
        TzOffsetCell::Measured { offset_secs: 0 },
    ];
    cells.sort_by_key(|&cell| tz_offset_rank(cell));
    assert_eq!(cells.last(), Some(&TzOffsetCell::Unknown));
}

/// `tz_offset_rank`: measured values order by the offset itself, not by magnitude.
#[test]
fn measured_values_order_by_signed_offset() {
    let west = tz_offset_rank(TzOffsetCell::Measured {
        offset_secs: -14_400,
    });
    let utc = tz_offset_rank(TzOffsetCell::Measured { offset_secs: 0 });
    let east = tz_offset_rank(TzOffsetCell::Measured { offset_secs: 7_200 });
    assert!(west < utc);
    assert!(utc < east);
}

/// `tz_offset_tooltip`: a `None` offset never claims a number — the hover carries only the
/// never-measured line and the sample count standing behind it.
#[test]
fn an_unmeasured_status_reports_no_offset_line() {
    let facts = TzOffsetFacts {
        offset_secs: None,
        samples: 2,
        observed_at_utc: 0,
        source: OffsetSource::None,
    };
    let tooltip = tz_offset_tooltip(&facts);
    assert!(!tooltip.contains("UTC+"));
    assert!(!tooltip.contains("UTC-"));
    assert!(tooltip.contains('2'));
}

/// `tz_offset_tooltip`: the sample count, the observed instant and the source all come FROM the
/// facts — changing any one of them changes the rendered hover, so the tooltip cannot be a fixed
/// string that ignores its argument.
#[test]
fn the_tooltip_is_built_from_the_facts_not_restated() {
    let base = TzOffsetFacts {
        offset_secs: Some(3_600),
        samples: 5,
        observed_at_utc: 1_700_000_000_000,
        source: OffsetSource::Log,
    };
    let base_tooltip = tz_offset_tooltip(&base);
    assert!(base_tooltip.contains('5'));
    assert!(base_tooltip.contains("UTC+01:00"));

    let more_samples = TzOffsetFacts {
        samples: 41,
        ..base
    };
    assert_ne!(tz_offset_tooltip(&more_samples), base_tooltip);
    assert!(tz_offset_tooltip(&more_samples).contains("41"));

    let other_source = TzOffsetFacts {
        source: OffsetSource::Skew,
        ..base
    };
    assert_ne!(tz_offset_tooltip(&other_source), base_tooltip);
}

/// `tz_offset_facts`: every field is carried through unchanged from the retained status — the
/// hover has nothing to say that the status itself did not report.
#[test]
fn facts_carry_the_retained_status_through_unchanged() {
    let status = CoreTimeOffsetStatus {
        offset_secs: Some(-1_800),
        observed_at_utc: 123_456_789,
        samples: 7,
        source: OffsetSource::Replica,
    };
    let facts = tz_offset_facts(&status);
    assert_eq!(facts.offset_secs, status.offset_secs);
    assert_eq!(facts.observed_at_utc, status.observed_at_utc);
    assert_eq!(facts.samples, status.samples);
    assert_eq!(facts.source, status.source);
}
