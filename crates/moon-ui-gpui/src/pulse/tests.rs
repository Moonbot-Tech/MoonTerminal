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

/// Build an `Arrivals` whose one stamp is already the given age.
fn aged(key: &str, age: Duration) -> Arrivals<String> {
    let mut arrivals = Arrivals::default();
    arrivals.mark([key.to_string()]);
    arrivals.stamps.insert(
        key.to_string(),
        Instant::now()
            .checked_sub(age)
            .expect("clock is far enough past boot"),
    );
    arrivals
}

/// A stamp is live exactly while it can still be drawn, and `prune` erases it once it cannot.
///
/// Breakage: dropping the elapsed comparison from `live` re-arms the pulse chain forever on stamps
/// that draw nothing — a permanent 10 Hz repaint of the owning view, the exact cost this module
/// exists to avoid. Dropping it from `prune` leaves the map growing instead. The two halves are
/// asserted together because either alone can look correct: a `live` that never ends is invisible
/// until someone reads a diag counter.
#[test]
fn a_stamp_stays_live_exactly_as_long_as_it_can_be_drawn() {
    let mut fresh = aged("a", Duration::from_millis(0));
    assert!(fresh.live(), "a just-marked arrival must be drawable");
    assert!(fresh.get(&"a".to_string()).is_some());
    fresh.prune();
    assert!(fresh.live(), "pruning must not drop a live stamp");

    let mut over = aged("a", FLASH + Duration::from_millis(50));
    assert!(!over.live(), "a finished stamp must end the chain");
    assert!(
        arrival_tint(
            0x00FF00,
            over.get(&"a".to_string()).expect("still recorded")
        )
        .is_none(),
        "a finished stamp must draw nothing"
    );
    over.prune();
    assert!(over.get(&"a".to_string()).is_none(), "prune must erase it");
}

/// `retain_live` must drop what the owner no longer shows, not only what expired.
///
/// Breakage: pruning by age alone leaves stamps for items that scrolled out of a rotating feed, so
/// the map grows with everything that ever arrived and `snapshot` copies it on every render.
#[test]
fn retain_live_forgets_items_that_left_the_surface() {
    let mut arrivals = Arrivals::default();
    arrivals.mark(["kept".to_string(), "gone".to_string()]);

    arrivals.retain_live(|key| key == "kept");
    assert!(arrivals.get(&"kept".to_string()).is_some());
    assert!(
        arrivals.get(&"gone".to_string()).is_none(),
        "an item no longer on screen must not keep a stamp"
    );

    arrivals.clear();
    assert!(!arrivals.live(), "clear must end the fade immediately");
}

/// `snapshot` must hand out what is recorded, and `mark` must re-stamp an item that arrives again.
///
/// Breakage: a `snapshot` returning an empty map compiles, passes every other test here, and
/// silently kills the News arrival tint — the panel reads its stamps only through this copy. A
/// `mark` that inserts instead of overwriting keeps the FIRST arrival's age, so an item that
/// arrives, fades, and arrives again never lights up a second time.
#[test]
fn snapshot_carries_the_stamps_and_mark_restamps_a_repeat_arrival() {
    let mut arrivals = aged("repeat", FLASH + Duration::from_millis(50));
    assert_eq!(
        arrivals.snapshot().len(),
        1,
        "the copy must carry what is recorded, not an empty map"
    );
    assert!(!arrivals.live(), "the fixture starts expired");

    arrivals.mark(["repeat".to_string()]);
    assert!(
        arrivals.live(),
        "an item arriving again must light up again, not keep its first stamp"
    );
    assert_eq!(
        arrivals.snapshot().get("repeat").copied(),
        arrivals.get(&"repeat".to_string()),
        "the copy must agree with the live lookup the other surface uses"
    );
}
