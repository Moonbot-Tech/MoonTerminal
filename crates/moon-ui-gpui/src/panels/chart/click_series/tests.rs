use super::*;

/// A press position every test that does not care about travel reuses.
const SPOT: (f32, f32) = (400.0, 300.0);

/// Well inside any system double-click interval, whose Windows range is 200..=900 ms.
const SOON_MS: f64 = 60.0;

/// Well outside it, so these presses can never read as one series.
const LATER_MS: f64 = 60_000.0;

#[test]
fn a_first_press_is_a_single_click() {
    let mut series = ClickSeries::default();
    assert_eq!(series.observe(MouseButton::Left, 1, 0.0, SPOT), 1);
}

#[test]
fn a_double_click_this_panel_saw_whole_counts_as_two() {
    let mut series = ClickSeries::default();
    series.observe(MouseButton::Left, 1, 0.0, SPOT);
    assert_eq!(
        series.observe(MouseButton::Left, 2, SOON_MS, SPOT),
        2,
        "an honest double click on this chart must still trade"
    );
}

/// The reported bug: closing charts one after another on the same × delivers press two to the
/// chart the reflow moved under the cursor, with the window's count already at two.
#[test]
fn a_double_click_whose_first_press_went_elsewhere_counts_as_one() {
    let mut series = ClickSeries::default();
    assert_eq!(
        series.observe(MouseButton::Left, 2, 0.0, SPOT),
        1,
        "a panel that never saw press one must not treat press two as a double click"
    );
}

/// The same bug on a panel with a HISTORY: any chart the user has single-clicked before also sits
/// at native 1, so matching the native chain alone would let a stranger's press two chain onto an
/// observation minutes old.
#[test]
fn a_stale_observation_does_not_absorb_a_stranger_press() {
    let mut series = ClickSeries::default();
    series.observe(MouseButton::Left, 1, 0.0, SPOT);
    assert_eq!(
        series.observe(MouseButton::Left, 2, LATER_MS, SPOT),
        1,
        "a press too late to pair with the last one this panel saw starts a new series"
    );
}

#[test]
fn a_press_somewhere_else_does_not_extend_the_series() {
    let mut series = ClickSeries::default();
    series.observe(MouseButton::Left, 1, 0.0, SPOT);
    assert_eq!(
        series.observe(MouseButton::Left, 2, SOON_MS, (SPOT.0 + 200.0, SPOT.1)),
        1
    );
}

#[test]
fn the_series_resumes_from_the_presses_this_panel_saw() {
    let mut series = ClickSeries::default();
    // Press one went to the close button of the chart that was here before.
    series.observe(MouseButton::Left, 2, 0.0, SPOT);
    assert_eq!(
        series.observe(MouseButton::Left, 3, SOON_MS, SPOT),
        2,
        "two presses in a row on this chart are its own double click, whatever preceded them"
    );
}

#[test]
fn a_press_the_panel_missed_breaks_the_series() {
    let mut series = ClickSeries::default();
    series.observe(MouseButton::Left, 1, 0.0, SPOT);
    // Native count 3 means the panel never received press two.
    assert_eq!(series.observe(MouseButton::Left, 3, SOON_MS, SPOT), 1);
}

#[test]
fn another_button_starts_its_own_series() {
    let mut series = ClickSeries::default();
    series.observe(MouseButton::Left, 1, 0.0, SPOT);
    assert_eq!(series.observe(MouseButton::Right, 2, SOON_MS, SPOT), 1);
}

#[test]
fn no_close_yet_leaves_every_press_alone() {
    assert!(!press_is_close_residue(None, LATER_MS, SPOT));
}

/// The chain the per-panel count cannot see: presses three, four and five of a closing spree are
/// honest pairs for the chart that happens to sit there, and only the pixel gives them away.
#[test]
fn presses_parked_on_the_close_spot_stay_residue() {
    let close = Some((1_000.0, SPOT));
    for after in [200.0, 800.0, 1_600.0, 2_800.0] {
        assert!(
            press_is_close_residue(close, 1_000.0 + after, SPOT),
            "a press {after} ms after the close, still on its pixel, is part of that closing"
        );
    }
}

/// A hand stabbing the same button drifts a few pixels per press. `render_input::press_count` moves
/// the mark's POSITION onto every press it rejects — but not its clock, so the sequence still ends.
#[test]
fn a_drifting_stab_sequence_stays_residue_while_the_mark_follows() {
    let mut mark = Some((0.0, SPOT));
    let mut pos = SPOT;
    for step in 1..=6 {
        let at = f64::from(step) * 300.0;
        pos = (pos.0 + 5.0, pos.1 + 3.0);
        assert!(
            press_is_close_residue(mark, at, pos),
            "press {step} of the sequence escaped the mark"
        );
        // What press_count stores back: the new position against the ORIGINAL close time.
        mark = mark.map(|(at_ms, _)| (at_ms, pos));
    }
    assert!(
        !press_is_close_residue(mark, CLOSE_RESIDUE_MS + 1.0, pos),
        "following the presses must not keep the spot untradeable for as long as the user clicks"
    );
}

/// The same sequence against a mark left where the first × was: this is what moving it prevents.
#[test]
fn a_mark_left_behind_would_lose_that_sequence() {
    assert!(!press_is_close_residue(
        Some((0.0, SPOT)),
        900.0,
        (SPOT.0 + 15.0, SPOT.1 + 9.0)
    ));
}

#[test]
fn a_press_that_moved_off_the_close_spot_trades() {
    let close = Some((1_000.0, SPOT));
    assert!(!press_is_close_residue(
        close,
        1_000.0 + SOON_MS,
        (SPOT.0 + 40.0, SPOT.1)
    ));
}

#[test]
fn the_close_spot_frees_up_again() {
    assert!(
        !press_is_close_residue(Some((1_000.0, SPOT)), 1_000.0 + LATER_MS, SPOT),
        "the mark must not hold one pixel hostage for the rest of the session"
    );
}

/// A backwards wall-clock step (NTP, resume from sleep) must not latch the mark on.
#[test]
fn a_clock_step_backwards_does_not_latch_the_mark() {
    assert!(!press_is_close_residue(
        Some((1_000.0, SPOT)),
        1_000.0 - SOON_MS,
        SPOT
    ));
}

#[test]
fn a_close_no_press_explains_leaves_no_mark() {
    let mut series = ClickSeries::default();
    series.observe(MouseButton::Left, 1, 0.0, SPOT);
    assert_eq!(
        series.fresh_press_pos(LATER_MS),
        None,
        "Escape, a TTL expiry or a teardown must not mark the pixel of some old press"
    );
    assert_eq!(series.fresh_press_pos(SOON_MS), Some(SPOT));
}

/// A close fires on mouse UP, so a × held down for a second still has to leave its mark.
#[test]
fn a_slowly_held_close_still_marks_its_pixel() {
    let mut series = ClickSeries::default();
    series.observe(MouseButton::Left, 1, 0.0, SPOT);
    assert_eq!(series.fresh_press_pos(CLOSE_RESIDUE_MS - 1.0), Some(SPOT));
}

#[test]
fn a_reset_slot_starts_counting_again() {
    let mut series = ClickSeries::default();
    series.observe(MouseButton::Left, 1, 0.0, SPOT);
    series.reset();
    assert_eq!(
        series.observe(MouseButton::Left, 2, SOON_MS, SPOT),
        1,
        "a slot that took a new coin must not trade it on the previous coin's press"
    );
}
