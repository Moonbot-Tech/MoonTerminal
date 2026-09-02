use super::*;

/// One line of a chrome row at the shipped font step: an Action-sized control.
const SECTION_H: f32 = 26.0;
/// The same row on two lines — two sections, the wrap gap, and the row's own padding.
const TWO_LINES_H: f32 = 26.0 * 2.0 + 8.0 + 8.0;
/// One line of that row, padding included.
const ONE_LINE_H: f32 = 26.0 + 8.0;
/// What compacting both selectors gives back, in measured pixels.
const SAVING: f32 = 60.0;

/// Build a measurement for a row of `row_w` that rendered at `row_h`.
fn metrics(row_w: f32, row_h: f32) -> RowMetrics {
    RowMetrics {
        row_w,
        row_h,
        section_h: SECTION_H,
        fit: RowFit {
            saving: SAVING,
            signature: 7,
        },
    }
}

/// The same measurement with one of the caller-supplied halves replaced.
fn with_fit(m: RowMetrics, saving: f32, signature: u64) -> RowMetrics {
    RowMetrics {
        fit: RowFit { saving, signature },
        ..m
    }
}

/// The starting fit of a row that has already agreed on its composition.
fn full() -> WrapFit {
    WrapFit {
        overflow_w: None,
        signature: 7,
    }
}

#[test]
fn a_row_that_fits_one_line_changes_nothing() {
    assert_eq!(full().resolve(metrics(900.0, ONE_LINE_H)), None);
}

#[test]
fn a_section_much_taller_than_the_measured_one_is_not_a_wrap() {
    // An input or a date field on the row stands taller than the dropdown the line height is read
    // from. Even at half again its height, plus the row's padding, that must not read as a line.
    let tallest = SECTION_H * 1.5 + 8.0;
    assert_eq!(full().resolve(metrics(900.0, tallest)), None);
}

#[test]
fn a_full_row_that_wrapped_goes_compact_and_remembers_its_width() {
    let next = full()
        .resolve(metrics(700.0, TWO_LINES_H))
        .expect("an overflowing full row resolves to compact");
    assert!(next.compact(), "the row's controls shed their words first");
    assert_eq!(next.overflow_w, Some(700.0));
}

#[test]
fn a_compact_row_does_not_expand_at_the_width_that_overflowed() {
    let compact = full().resolve(metrics(700.0, TWO_LINES_H)).unwrap();
    // Compacting fitted the row back onto one line. Expanding here is exactly the oscillation the
    // remembered width exists to prevent.
    assert_eq!(compact.resolve(metrics(700.0, ONE_LINE_H)), None);
}

#[test]
fn a_compact_row_that_still_wraps_stays_compact() {
    let compact = full().resolve(metrics(700.0, TWO_LINES_H)).unwrap();
    assert_eq!(compact.resolve(metrics(700.0, TWO_LINES_H)), None);
}

#[test]
fn a_compact_row_that_still_wraps_stays_compact_however_wide_it_gets() {
    // A detached Report in a narrow window wraps even compact. Expanding it because the host grew
    // past the remembered width puts the FULL row back onto two lines, and the next frame compacts
    // it again — one flash of the full row per step of a widening drag.
    let compact = full().resolve(metrics(700.0, TWO_LINES_H)).unwrap();
    assert_eq!(
        compact.resolve(metrics(700.0 + SAVING * 3.0, TWO_LINES_H)),
        None,
        "the second line is the answer once the words are already gone"
    );
    // It expands only once compacting actually buys the row a single line again.
    assert!(
        compact
            .resolve(metrics(700.0 + SAVING, ONE_LINE_H))
            .is_some_and(|fit| !fit.compact())
    );
}

#[test]
fn a_compact_row_expands_once_it_is_wider_by_what_compacting_saved() {
    let compact = full().resolve(metrics(700.0, TWO_LINES_H)).unwrap();
    assert_eq!(
        compact.resolve(metrics(700.0 + SAVING - 1.0, ONE_LINE_H)),
        None,
        "one pixel short of the saving is still not enough room for the full row"
    );
    let expanded = compact
        .resolve(metrics(700.0 + SAVING, ONE_LINE_H))
        .expect("the full row provably fits once the saving is available again");
    assert!(!expanded.compact());
    assert_eq!(expanded.overflow_w, None, "nothing is owed a re-expansion");
}

#[test]
fn a_near_zero_saving_still_needs_a_real_widening() {
    let m = with_fit(metrics(700.0, TWO_LINES_H), 0.0, 7);
    let compact = full().resolve(m).unwrap();
    assert_eq!(
        compact.resolve(with_fit(
            metrics(700.0 + MIN_REEXPAND_MARGIN - 1.0, ONE_LINE_H),
            0.0,
            7
        )),
        None
    );
    assert!(
        compact
            .resolve(with_fit(
                metrics(700.0 + MIN_REEXPAND_MARGIN, ONE_LINE_H),
                0.0,
                7
            ))
            .is_some_and(|fit| !fit.compact())
    );
}

#[test]
fn re_expansion_that_wraps_again_raises_the_threshold_instead_of_cycling() {
    // The pessimal case: every re-expansion overflows again. The threshold must climb each time,
    // or the row alternates forever at one width.
    let mut fit = full();
    let mut w = 700.0;
    for _ in 0..4 {
        fit = fit.resolve(metrics(w, TWO_LINES_H)).expect("goes compact");
        assert!(fit.compact());
        w += SAVING;
        fit = fit
            .resolve(metrics(w, ONE_LINE_H))
            .expect("tries full again");
        assert!(!fit.compact());
    }
    assert_eq!(
        w,
        700.0 + SAVING * 4.0,
        "each retry needs strictly more room"
    );
    // And at a width that never overflowed, the row simply stays full.
    assert_eq!(fit.resolve(metrics(w, ONE_LINE_H)), None);
}

#[test]
fn an_unmeasured_row_decides_nothing() {
    assert_eq!(full().resolve(metrics(0.0, 0.0)), None);
    assert_eq!(
        full().resolve(RowMetrics {
            section_h: 0.0,
            ..metrics(700.0, TWO_LINES_H)
        }),
        None,
        "a zero line height would read as a wrap against any row at all"
    );
}

#[test]
fn non_finite_pixels_decide_nothing_and_are_never_remembered() {
    for bad in [f32::NAN, f32::INFINITY] {
        assert_eq!(full().resolve(metrics(bad, TWO_LINES_H)), None);
        assert_eq!(
            full().resolve(RowMetrics {
                row_h: bad,
                ..metrics(700.0, TWO_LINES_H)
            }),
            None
        );
        assert_eq!(
            full().resolve(RowMetrics {
                section_h: bad,
                ..metrics(700.0, TWO_LINES_H)
            }),
            None
        );
    }
    // A non-finite saving falls back to the fixed margin instead of poisoning the threshold: with
    // NaN every later comparison would be false and the row could never expand again.
    let compact = full().resolve(metrics(700.0, TWO_LINES_H)).unwrap();
    assert!(
        compact
            .resolve(with_fit(
                metrics(700.0 + MIN_REEXPAND_MARGIN, ONE_LINE_H),
                f32::NAN,
                7
            ))
            .is_some_and(|fit| !fit.compact())
    );
}

#[test]
fn a_changed_composition_starts_the_fit_over() {
    let compact = full().resolve(metrics(700.0, TWO_LINES_H)).unwrap();
    let reset = compact
        .resolve(with_fit(metrics(700.0, ONE_LINE_H), SAVING, 8))
        .expect("a row that no longer holds the same sections re-measures from scratch");
    assert!(
        !reset.compact(),
        "a smaller font or a shorter locale must get its full row back without a resize"
    );
    assert_eq!(
        reset.overflow_w, None,
        "the old anchor described another row"
    );
    assert_eq!(reset.signature, 8);
    // A row that still cannot hold its words simply compacts again on the next frame.
    assert!(
        reset
            .resolve(with_fit(metrics(700.0, TWO_LINES_H), SAVING, 8))
            .is_some_and(WrapFit::compact)
    );
}

#[test]
fn a_changed_composition_on_a_full_row_only_adopts_the_signature() {
    let adopted = WrapFit::default()
        .resolve(metrics(700.0, ONE_LINE_H))
        .expect("the default fit carries no signature yet");
    assert_eq!(
        adopted,
        WrapFit {
            overflow_w: None,
            signature: 7,
        }
    );
    assert_eq!(
        adopted.resolve(metrics(700.0, ONE_LINE_H)),
        None,
        "and settles immediately"
    );
}

#[test]
fn the_frame_that_reported_a_new_composition_is_not_thrown_away_with_it() {
    // The FIRST frame of every panel takes this path — a default fit's signature 0 against a real
    // digest — so discarding its measurement would leave an already-wrapped row full until some
    // unrelated repaint happened to arrive.
    let adopted = WrapFit::default()
        .resolve(metrics(700.0, TWO_LINES_H))
        .expect("a first frame that already wrapped decides in that same frame");
    assert!(adopted.compact());
    assert_eq!(adopted.overflow_w, Some(700.0));
    assert_eq!(adopted.signature, 7);
}
