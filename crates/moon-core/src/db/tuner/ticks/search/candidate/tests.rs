use super::super::better_score;
use super::*;

/// A replay closing `profits` and leaving `open` deals open.
fn replayed(profits: &[f64], open: usize) -> Replayed {
    let mut closed = Tally::default();
    for p in profits {
        closed.push(*p);
    }
    Replayed { closed, open }
}

fn point(value: &str) -> Point {
    [("SellPrice", value.to_string())].into()
}

/// The exit under an entry point walks the open deals down before it earns: one deal fewer open
/// beats any profit, and among as many open the profit decides. With the open deals refused
/// outright the walk had no direction at all — every point but the one closing them all read the
/// same (2026-10-07).
#[test]
fn the_exit_walk_closes_deals_first_and_earns_second() {
    let score = |r: &Replayed| Some(closing_score(r));
    let rich_two_open = replayed(&[500.0, 500.0], 2);
    let poor_one_open = replayed(&[-50.0, 1.0], 1);
    assert!(better_score(
        &score(&poor_one_open),
        &score(&rich_two_open),
        1
    ));
    let poor_none_open = replayed(&[-90.0], 0);
    assert!(better_score(
        &score(&poor_none_open),
        &score(&poor_one_open),
        1
    ));
    let richer_one_open = replayed(&[-50.0, 3.0], 1);
    assert!(better_score(
        &score(&richer_one_open),
        &score(&poor_one_open),
        1
    ));
}

/// Only a point that closes every deal is ever the answer; the best one that leaves deals open is
/// kept apart, whatever it earns, and only while it closes at least the floor.
#[test]
fn an_open_point_is_a_candidate_never_the_answer() {
    let tracker = Tracker::default();
    tracker.offer(&point("1"), &replayed(&[1.0, 1.0], 0), 3, 2);
    tracker.offer(&point("2"), &replayed(&[50.0, 50.0], 4), 3, 2);
    // Closes one trade, under the floor of two: no candidate.
    tracker.offer(&point("3"), &replayed(&[900.0], 1), 3, 2);
    let answer = tracker.answer().expect("an answer");
    assert_eq!(answer.point, point("1"));
    assert_eq!(answer.open, 0);
    let candidate = tracker.candidate().expect("a candidate");
    assert_eq!(candidate.point, point("2"));
    assert_eq!(candidate.open, 4);
    assert!((candidate.score.profit - 100.0).abs() < 1e-9);
}

/// The answer is the best closing point met in ANY restart, and of equal ones the lowest restart
/// keeps its place whatever order the parallel restarts offered them in.
#[test]
fn the_best_closing_point_wins_and_the_lowest_restart_breaks_a_tie() {
    let tracker = Tracker::default();
    tracker.offer(&point("1"), &replayed(&[2.0], 0), 5, 1);
    tracker.offer(&point("2"), &replayed(&[2.0], 0), 2, 1);
    tracker.offer(&point("3"), &replayed(&[2.0], 0), 7, 1);
    assert_eq!(tracker.answer().expect("an answer").restart, 2);
    tracker.offer(&point("4"), &replayed(&[3.0], 0), 9, 1);
    let answer = tracker.answer().expect("an answer");
    assert_eq!((answer.restart, answer.point), (9, point("4")));
}
