use super::{Trader, rank_marks};

/// A board row: who, and where the service ranked them.
fn at(id: u64, place: u32) -> Trader {
    Trader {
        place,
        id,
        handle: "@".to_string(),
        profit: 1.0,
        trades: 1,
    }
}

#[test]
fn a_climb_is_positive_and_a_row_that_stayed_gets_nothing() {
    let was = vec![at(1, 1), at(2, 2), at(3, 3)];
    let next = vec![at(3, 1), at(1, 2), at(2, 3)];
    let marks = rank_marks(&was, &next);
    assert_eq!(marks[&3], 2, "a climb of two came out wrong");
    assert_eq!(marks[&1], -1, "a drop of one came out wrong");
    assert_eq!(marks[&2], -1);

    // The same board again: nobody moved, so nobody is marked.
    let still = rank_marks(&next, &next);
    assert!(
        still.values().all(|shift| *shift == 0),
        "a board that moved nobody marked somebody: {still:?}"
    );
}

#[test]
fn the_mark_is_counted_off_the_rank_and_not_off_the_screen_row() {
    // The service ranks fifty and we draw twenty; a row the parser drops takes a whole screen row
    // with it. Counted off the drawn order, everybody under it would wear "climbed one" while
    // their printed rank had not moved at all.
    let was = vec![at(1, 1), at(2, 2), at(3, 3)];
    // The second row did not survive the parse; the other two kept their ranks.
    let next = vec![at(1, 1), at(3, 3)];
    let marks = rank_marks(&was, &next);
    assert_eq!(
        marks[&1], 0,
        "the first row was marked for somebody else's fall"
    );
    assert_eq!(
        marks[&3], 0,
        "a row whose rank never moved was marked as climbing"
    );
}

#[test]
fn a_row_that_was_not_there_before_is_not_marked() {
    let was = vec![at(1, 1)];
    let next = vec![at(1, 2), at(9, 1)];
    let marks = rank_marks(&was, &next);
    assert_eq!(marks[&9], 0, "an arrival was marked as a climb");
    assert_eq!(marks[&1], -1);
}
