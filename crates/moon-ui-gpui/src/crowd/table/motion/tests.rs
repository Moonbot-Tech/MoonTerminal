use super::{FAREWELL, GLOW, Motion, SLIDE};
use std::time::{Duration, Instant};

/// A board, the way a view hands one over: what names each row, and the row itself. These tests
/// are about the MOVEMENT, so the row is nothing.
fn board(keys: &[&str]) -> Vec<(String, ())> {
    keys.iter().map(|key| ((*key).to_string(), ())).collect()
}

/// A moment `millis` after `start`.
fn later(start: Instant, millis: u64) -> Instant {
    start + Duration::from_millis(millis)
}

#[test]
fn a_row_that_climbs_slides_from_where_it_was() {
    let mut motion: Motion<()> = Motion::default();
    let now = Instant::now();
    motion.settle(&board(&["A", "B", "C"]), now);
    // Nothing has moved yet: a board seen for the first time is not a board that jumped.
    assert!(!motion.moving(now), "the first board arrived moving");
    assert_eq!(motion.of("C", now).from, None);

    motion.settle(&board(&["C", "A", "B"]), now);
    assert_eq!(motion.of("C", now).from, Some(2), "C forgot it was third");
    assert_eq!(motion.of("A", now).from, Some(0));
    assert!(motion.moving(now), "a reshuffled board is standing still");
}

#[test]
fn a_slide_runs_from_nothing_to_all_of_it_and_then_stops() {
    // The screen reads this at whatever rate it likes, so the progress has to be a function of
    // the clock and nothing else — and it has to END, or the screen never goes back to sleep.
    let mut motion: Motion<()> = Motion::default();
    let now = Instant::now();
    motion.settle(&board(&["A", "B"]), now);
    motion.settle(&board(&["B", "A"]), now);
    assert_eq!(motion.of("A", now).slid, 0.0, "the slide started finished");
    let half = motion
        .of("A", later(now, SLIDE.as_millis() as u64 / 2))
        .slid;
    assert!(
        (half - 0.5).abs() < 0.05,
        "halfway through the slide it was {half} of the way"
    );
    let after = later(now, SLIDE.as_millis() as u64 + 1);
    assert_eq!(motion.of("A", after).from, None, "the slide never ended");
    assert!(!motion.moving(after), "a finished slide still wants frames");
}

#[test]
fn a_row_that_arrives_is_not_slid_in_from_somewhere_it_never_was() {
    let mut motion: Motion<()> = Motion::default();
    let now = Instant::now();
    motion.settle(&board(&["A", "B"]), now);
    motion.settle(&board(&["A", "B", "NEW"]), now);
    assert_eq!(
        motion.of("NEW", now).from,
        None,
        "a new row was slid in from a place it never held"
    );
    assert!(motion.of("A", now).still(), "an untouched row was moved");
}

#[test]
fn a_row_that_drops_off_is_shown_leaving_and_then_forgotten() {
    let mut motion: Motion<()> = Motion::default();
    let now = Instant::now();
    motion.settle(&board(&["A", "B", "C"]), now);
    motion.settle(&board(&["A", "B"]), now);
    let leaving: Vec<_> = motion.leaving(now).collect();
    assert_eq!(leaving.len(), 1, "the row left without being seen to");
    assert_eq!(leaving[0].0, "C");
    assert_eq!(leaving[0].2, 2, "it left from the wrong place");
    assert_eq!(leaving[0].3.leaving, Some(0.0));

    // Once the farewell is over it is gone for good, and the table is at rest again.
    let after = later(now, FAREWELL.as_millis() as u64 + 1);
    assert_eq!(
        motion.leaving(after).count(),
        0,
        "the row never finished leaving"
    );
    assert!(!motion.moving(after));
    motion.settle(&board(&["A", "B"]), after);
    assert_eq!(motion.leaving(after).count(), 0);
}

#[test]
fn a_row_that_comes_back_is_not_also_leaving() {
    // The trader board is fifty rows of which twenty are shown, so a row on the edge crosses the
    // line in both directions all evening. Drawn twice at once it would read as a duplicate.
    let mut motion: Motion<()> = Motion::default();
    let now = Instant::now();
    motion.settle(&board(&["A", "B", "C"]), now);
    motion.settle(&board(&["A", "B"]), now);
    motion.settle(&board(&["A", "B", "C"]), now);
    assert_eq!(
        motion.leaving(now).count(),
        0,
        "a row that came back was still on its way out"
    );
}

#[test]
fn a_glow_starts_when_the_figures_move_and_goes_out_by_itself() {
    let mut motion: Motion<()> = Motion::default();
    let now = Instant::now();
    motion.settle(&board(&["A", "B"]), now);
    motion.beat("A", true, now);
    assert_eq!(motion.of("A", now).glow, 1.0, "the row did not light up");
    assert!(
        motion.of("A", now).gain,
        "the glow forgot which way it went"
    );
    assert_eq!(motion.of("B", now).glow, 0.0);
    assert!(motion.moving(now));

    let half = motion.of("A", later(now, GLOW.as_millis() as u64 / 2)).glow;
    assert!(
        (half - 0.5).abs() < 0.05,
        "halfway through the glow it was at {half}"
    );
    let after = later(now, GLOW.as_millis() as u64 + 1);
    assert_eq!(motion.of("A", after).glow, 0.0, "the glow never went out");
    assert!(!motion.moving(after), "a settled board still wants frames");
}

#[test]
fn a_key_that_arrives_twice_is_taken_once() {
    // Everything on the screen is keyed by this: two rows with one name would slide together,
    // glow together and leave together — one row wearing two sets of figures. The service has
    // been seen to omit the field the trader board is keyed by, which is exactly how it happens.
    let mut motion: Motion<()> = Motion::default();
    let now = Instant::now();
    motion.settle(&board(&["A", "DUP", "DUP", "B"]), now);
    assert_eq!(
        motion.of("DUP", now).from,
        None,
        "a duplicate arrived already moving"
    );
    // The FIRST of them owns the place: the second is drawn over it rather than moving it.
    motion.settle(&board(&["DUP", "A", "B"]), now);
    assert_eq!(
        motion.of("DUP", now).from,
        Some(1),
        "the duplicate was tracked from the wrong place"
    );
}

#[test]
fn a_board_that_does_not_move_asks_for_nothing() {
    // The whole cost model in one test: the same board twice leaves nothing to draw and nothing
    // to draw it with.
    let mut motion: Motion<()> = Motion::default();
    let now = Instant::now();
    motion.settle(&board(&["A", "B", "C"]), now);
    motion.settle(&board(&["A", "B", "C"]), now);
    assert!(!motion.moving(now));
    for key in ["A", "B", "C"] {
        assert!(motion.of(key, now).still(), "{key} was drawn as moving");
    }
}
