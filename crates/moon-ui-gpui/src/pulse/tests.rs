use super::*;

/// A pulse that has run out must stop reporting progress: [`phase`] returning `None` is the only
/// thing that removes the decoration, so a version saturating at `1.0` would pin the News arrival
/// tint on screen permanently.
#[test]
fn a_finished_pulse_stops_reporting_progress() {
    let total = Duration::from_millis(20);
    let done = Instant::now()
        .checked_sub(Duration::from_millis(40))
        .expect("clock is far enough past boot");
    assert_eq!(phase(done, total), None);
    assert!(phase(Instant::now(), total).is_some());
}

/// Progress is a FRACTION of the pulse length, not elapsed seconds. Callers feed it straight into
/// easing curves that assume `0.0..1.0`; raw seconds would run a 2.6 s flash three times too fast
/// and clamp a 2 s fade to nothing.
#[test]
fn progress_is_normalised_to_the_pulse_length() {
    let at = Instant::now()
        .checked_sub(Duration::from_millis(500))
        .expect("clock is far enough past boot");
    let p = phase(at, Duration::from_millis(2000)).expect("still live");
    assert!(
        (0.2..0.35).contains(&p),
        "expected about a quarter, got {p}"
    );
}
