use super::*;

fn book() -> Arc<ListingBook> {
    Arc::new(ListingBook::default())
}

fn builds(claim: &Claim) -> bool {
    matches!(claim, Claim::Build(_))
}

/// The record a build leaves for these holds: each clock, and its client as one that was read.
fn clocks(holds: &[(WarmKey, i64)]) -> WarmClocks {
    let mut out = WarmClocks::default();
    for &(key, since) in holds {
        out.since.insert(key, since);
        out.read.insert((key.0, key.1));
    }
    out
}

/// One caller per expiry builds; everyone else is served the kept book, stale included.
///
/// Breakage: handing the build to every caller that finds the book expired. A keystroke in the
/// Screener's Market box starts a publication, so a burst of them at an expiry would each re-read
/// every market of every core in parallel, under the market locks the feed threads need.
#[test]
fn only_one_caller_builds_per_expiry() {
    let mut cache = ListingCache::default();
    assert!(builds(&cache.claim(1_000_000)), "nothing built yet");
    assert!(!builds(&cache.claim(1_000_001)), "a build is in flight");
    cache.store(1_000_000, book(), WarmClocks::default());
    assert!(!builds(&cache.claim(1_000_000 + BOOK_TTL_MS - 1)), "fresh");
    assert!(builds(&cache.claim(1_000_000 + BOOK_TTL_MS)), "expired");
    assert!(!builds(&cache.claim(1_000_000 + BOOK_TTL_MS * 5)));
}

/// A build that did not finish gives the right back, or the book would never be rebuilt again.
#[test]
fn an_abandoned_build_can_be_claimed_again() {
    let mut cache = ListingCache::default();
    assert!(builds(&cache.claim(1_000_000)));
    cache.abandon();
    assert!(builds(&cache.claim(1_000_001)));
}

/// A clock set back makes the kept book look as if it came from the future; that is expired.
#[test]
fn a_book_from_the_future_is_rebuilt() {
    let mut cache = ListingCache::default();
    assert!(builds(&cache.claim(5_000_000)));
    cache.store(5_000_000, book(), WarmClocks::default());
    assert!(builds(&cache.claim(1_000_000)));
}

/// The warm-up clocks pass from one book to the next, but not across a gap nobody watched.
///
/// Breakage: carrying them whatever the gap. With the Screener closed for ten minutes a donor
/// can drop, reconnect and be half filled; the old clock would call it warm on the first build
/// after the window reopens, and its unfilled markets would read one-sided.
#[test]
fn warm_up_clocks_survive_a_period_and_not_a_closed_window() {
    let key: WarmKey = (7, 1, 3);
    let kept = clocks(&[(key, 900_000)]);
    let mut cache = ListingCache::default();
    assert!(builds(&cache.claim(1_000_000)));
    cache.store(1_000_000, book(), kept.clone());

    match cache.claim(1_000_000 + BOOK_TTL_MS) {
        Claim::Build(carried) => assert_eq!(carried, kept),
        Claim::Serve(_) => panic!("an expired book must be rebuilt"),
    }
    cache.store(1_000_000 + BOOK_TTL_MS, book(), kept.clone());

    match cache.claim(1_000_000 + BOOK_TTL_MS + WARM_CARRY_MS) {
        // The record of clients read goes with the clocks: after the gap every client is one
        // nobody has looked at, and is aged by its own connection.
        Claim::Build(carried) => assert_eq!(carried, WarmClocks::default()),
        Claim::Serve(_) => panic!("an expired book must be rebuilt"),
    }
}

/// A donor's warm-up is counted from when its client began holding a catalog, not from the first
/// time anybody looked at it.
///
/// Breakage (shipped to the developer's bench on 2026-11-16): starting every clock at the build
/// that first sees the donor. Nothing builds a book while no Screener is open, so every open —
/// with cores connected for hours — began a fresh minute in which no row could read `Spot Only`
/// or `Futures Only`: ticking either in the Listed list emptied the table, and it "started
/// working" a minute later whatever was clicked meanwhile.
#[test]
fn a_donor_connected_before_the_first_look_is_already_warm() {
    let now = 10_000_000;
    let hour = 3_600_000;
    assert!(
        warmed_up(now, warm_origin(now, false, Some(hour))),
        "believed at once"
    );
    // The warm-up to the millisecond: exactly that long is enough, one short is not.
    assert!(warmed_up(
        now,
        warm_origin(now, false, Some(DONOR_WARMUP_MS))
    ));
    assert!(!warmed_up(
        now,
        warm_origin(now, false, Some(DONOR_WARMUP_MS - 1))
    ));
    // A venue that turns up on a client the builds were already reading was not carried a build
    // ago, however long the client has been connected: its slots are filling right now.
    assert_eq!(warm_origin(now, true, Some(hour)), now);
    // No market list on record is no evidence of age.
    assert_eq!(warm_origin(now, false, None), now);
    // An age cannot be negative; one that came out so must not put the clock ahead of now.
    assert_eq!(warm_origin(now, false, Some(-9_000)), now);
}

/// Which origin a hold first seen on this build gets is decided by what the previous builds
/// read, not by whether they left a clock.
///
/// Breakage: reading "this client was watched" off the clocks alone. A donor read for minutes
/// while it carried nothing — its mask ticked later, its stream late — leaves no clock; the
/// venue it then starts to carry would be aged by the whole connection and its silence believed
/// while its slots are still filling, turning half an exchange one-sided for a book period.
#[test]
fn a_hold_first_seen_is_aged_by_the_connection_only_for_a_client_no_build_has_read() {
    let now = 10_000_000;
    let hour = 3_600_000;
    let key: WarmKey = (7, 1, 3);
    // Nothing carried — the window just opened: the connection's own age.
    assert_eq!(
        WarmClocks::default().clock_of(key, now, Some(hour)),
        now - hour
    );
    // Read before, with nothing carried then: the hold starts now.
    let mut read_only = WarmClocks::default();
    read_only.read.insert((7, 1));
    assert_eq!(read_only.clock_of(key, now, Some(hour)), now);
    // Read before, carrying another venue: this one still starts now, and the carried clock
    // stays what it was.
    let kept = clocks(&[((7, 1, 4), now - 50_000)]);
    assert_eq!(kept.clock_of(key, now, Some(hour)), now);
    assert_eq!(kept.clock_of((7, 1, 4), now, Some(hour)), now - 50_000);
    // The core's previous client having been read says nothing about the one that replaced it.
    assert_eq!(kept.clock_of((7, 2, 3), now, Some(5_000)), now - 5_000);
}

/// A replaced or removed core loses its clocks at once and makes the kept book stale.
///
/// Breakage: leaving the cache alone on a reconnect. The new client starts with no arbitrage
/// slots; the old clocks would let its silence be read as an answer, and the kept book would go
/// on serving the previous client's markets for the rest of its period.
#[test]
fn forgetting_a_core_drops_its_clocks_and_expires_the_book() {
    let mut cache = ListingCache::default();
    assert!(builds(&cache.claim(1_000_000)));
    cache.store(
        1_000_000,
        book(),
        clocks(&[((7, 1, 3), 900_000), ((8, 1, 3), 900_000)]),
    );
    cache.forget_core(7);
    match cache.claim(1_000_001) {
        // The other core keeps its clock: one core reconnecting must not restart every donor's
        // warm-up and blank the one-sided answers of the whole table for a minute.
        Claim::Build(carried) => assert_eq!(carried, clocks(&[((8, 1, 3), 900_000)])),
        Claim::Serve(_) => panic!("the book must be rebuilt after a core changed"),
    }
    // And the rebuilt book is fresh again.
    cache.store(1_000_001, book(), WarmClocks::default());
    assert!(!builds(&cache.claim(1_000_002)));
}

/// A core that changes WHILE a build runs is forgotten again when that build lands.
///
/// Breakage: storing the build's clocks as they are. The build read the core's previous client,
/// and a respawned core's new slot counts its epochs from the start — so the stored clock would
/// carry the new client's own key, and a donor that has just reconnected with no slots yet would
/// be warm on the next build.
#[test]
fn a_core_forgotten_during_a_build_stays_forgotten() {
    let mut cache = ListingCache::default();
    assert!(builds(&cache.claim(1_000_000)));
    cache.forget_core(7);
    cache.store(
        1_000_000,
        book(),
        clocks(&[((7, 1, 3), 900_000), ((8, 1, 3), 900_000)]),
    );
    match cache.claim(1_000_001) {
        Claim::Build(carried) => assert_eq!(carried, clocks(&[((8, 1, 3), 900_000)])),
        Claim::Serve(_) => panic!("a book that read the old client must be rebuilt"),
    }
    // The next build, with no change during it, is kept as fresh.
    cache.store(1_000_001, book(), WarmClocks::default());
    assert!(!builds(&cache.claim(1_000_002)));
}
