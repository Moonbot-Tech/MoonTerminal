use super::*;

fn scored(profit: f64) -> Screened {
    let mut tally = Tally::default();
    tally.push(profit);
    Screened::Scored(Some(tally))
}

#[test]
fn the_best_come_first_and_a_tie_keeps_the_earlier() {
    let moves = vec![
        ("a", scored(1.0)),
        ("b", Screened::Scored(None)),
        ("c", scored(5.0)),
        ("d", scored(3.0)),
        ("e", scored(5.0)),
    ];
    assert_eq!(best(moves, 3, 1), vec!["c", "e", "d"]);
}

#[test]
fn a_refused_move_comes_last_and_a_skipped_one_never() {
    let moves = vec![
        ("skipped", Screened::Skip),
        ("refused", Screened::Scored(None)),
        ("scored", scored(-2.0)),
    ];
    assert_eq!(best(moves, KEEP, 1), vec!["scored", "refused"]);
    assert!(best(Vec::<((), Screened)>::new(), KEEP, 1).is_empty());
}

#[test]
fn with_no_score_anywhere_the_first_listed_are_kept() {
    let moves: Vec<(usize, Screened)> = (0..6).map(|i| (i, Screened::Scored(None))).collect();
    assert_eq!(best(moves, KEEP, 1), vec![0, 1, 2]);
}
