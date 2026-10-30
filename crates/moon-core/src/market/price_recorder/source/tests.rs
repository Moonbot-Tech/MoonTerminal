use super::*;

// The registry is process-wide: every test uses core ids and exchange keys of its own.

#[test]
fn a_core_that_named_no_exchange_records_nothing() {
    assert_eq!(claim(9_100_001, 0), None);
}

#[test]
fn the_first_delivering_core_owns_until_it_falls_silent() {
    let (a, b) = (9_100_011, 9_100_012);
    set_exchange(a, "src-own:00000000");
    set_exchange(b, "src-own:00000000");
    assert_eq!(claim(a, 1_000).as_deref(), Some("src-own:00000000"));
    assert_eq!(claim(b, 1_500), None);
    assert!(claim(a, 3_000).is_some());
    assert_eq!(claim(b, 3_000 + TAKEOVER_MS), None);
    assert!(claim(b, 3_000 + TAKEOVER_MS + 1).is_some());
    assert_eq!(claim(a, 3_000 + TAKEOVER_MS + 2), None);
}

#[test]
fn a_core_that_goes_away_hands_its_exchange_on_at_once() {
    let (a, b) = (9_100_021, 9_100_022);
    set_exchange(a, "src-gone:00000000");
    set_exchange(b, "src-gone:00000000");
    assert!(claim(a, 1_000).is_some());
    forget(a);
    assert!(claim(b, 1_001).is_some());
    assert_eq!(claim(a, 1_002), None);
}

#[test]
fn a_core_that_moves_to_another_exchange_releases_the_old_one() {
    let (a, b) = (9_100_031, 9_100_032);
    set_exchange(a, "src-move-a:00000000");
    set_exchange(b, "src-move-a:00000000");
    assert!(claim(a, 1_000).is_some());
    set_exchange(a, "src-move-b:00000000");
    assert_eq!(claim(a, 1_001).as_deref(), Some("src-move-b:00000000"));
    assert_eq!(claim(b, 1_002).as_deref(), Some("src-move-a:00000000"));
}
