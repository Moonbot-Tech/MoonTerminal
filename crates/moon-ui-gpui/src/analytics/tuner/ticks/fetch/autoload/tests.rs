use super::super::station::drop_held;
use super::*;
use moon_core::db::tuner::ticks::tail::{floor_ms, required_with_floor};
use moon_core::market::trade_replay::ReplayWindow;

/// Directory absence must select cores for a recent EX-XX row and stop after core retention.
#[test]
fn unlisted_rows_use_cores_only_within_retention() {
    let now = 100 * 86_400_000;
    for (age, expected) in [
        (3_600_000, RowPath::Cores),
        (49 * 3_600_000, RowPath::TooOldForCores),
    ] {
        let source = fetchable(None, "XALPHA", now - age, now);
        assert_eq!(
            row_path(
                (true, true),
                source != Fetchable::NoRoute,
                matches!(source, Fetchable::Rest | Fetchable::Archive),
                now - age,
                now
            ),
            expected
        );
    }
}

/// A trade `(exchange, market, open_ms, close_ms)` and its window.
fn trade(market: &str, open_ms: i64, close_ms: i64) -> (String, String, ReplayWindow) {
    let window = moon_core::market::trade_replay::replay_window_ms(open_ms, close_ms, 30_000)
        .expect("a window");
    ("x".to_string(), market.to_string(), window)
}

/// A row whose every needed stretch the disk holds is dropped; one with a hole, one on a market
/// the disk has nothing of, and every row of a market whose read did not happen are kept.
#[test]
fn only_the_rows_the_disk_fully_holds_are_dropped() {
    let base = 1_790_000_000_000;
    let held_trade = trade("M", base, base + 10_000);
    let need = required_with_floor(
        &held_trade.2,
        floor_ms(super::super::super::tail::current_s()),
    )
    .hull()
    .expect("a need");
    let hole_trade = trade("M", base + 3_600_000, base + 3_610_000);
    let other_market = trade("N", base, base + 10_000);
    let unread_market = trade("U", base, base + 10_000);
    let rows = vec![held_trade, hole_trade, other_market, unread_market];
    let (kept, held) = drop_held(
        rows,
        |row| row.clone(),
        |_, market, _, _| match market {
            // Two abutting spans over the first trade's need: one stretch.
            "M" => Some(vec![(need.0 - 5, need.0 + 100), (need.0 + 101, need.1 + 5)]),
            "N" => Some(Vec::new()),
            _ => None,
        },
    );
    assert_eq!(held, 1);
    let mut markets: Vec<(String, i64)> = kept
        .iter()
        .map(|(_, m, w)| (m.clone(), w.open_ms))
        .collect();
    markets.sort();
    assert_eq!(
        markets,
        vec![
            ("M".to_string(), base + 3_600_000),
            ("N".to_string(), base),
            ("U".to_string(), base),
        ]
    );
}

/// The defaults (cores on, venues off): every recent row goes to the cores — no route, past the
/// retention, or a route the venue still serves alike — and nothing older than the cores' reach.
#[test]
fn with_the_defaults_recent_rows_go_to_the_cores_and_never_to_the_venue() {
    let now = 1_790_000_000_000;
    let recent = now - 3_600_000;
    let old = now - CORE_ARCHIVE_RETENTION_MS - 1;
    let defaults = (false, true);
    assert_eq!(
        row_path(defaults, false, false, recent, now),
        RowPath::Cores
    );
    assert_eq!(row_path(defaults, true, false, recent, now), RowPath::Cores);
    assert_eq!(row_path(defaults, true, true, recent, now), RowPath::Cores);
    assert_eq!(
        row_path(defaults, true, true, old, now),
        RowPath::TooOldForCores
    );
    // The horizon's own edge still counts as recent.
    assert_eq!(
        row_path(defaults, false, false, now - CORE_ARCHIVE_RETENTION_MS, now),
        RowPath::Cores
    );
}

/// With both switches on, the venue takes what it serves (its fetch asks the core first) and the
/// cores take the rest that is recent enough.
#[test]
fn with_both_switches_the_venue_takes_what_it_serves() {
    let now = 1_790_000_000_000;
    let recent = now - 3_600_000;
    let both = (true, true);
    assert_eq!(row_path(both, true, true, recent, now), RowPath::Venue);
    assert_eq!(row_path(both, false, false, recent, now), RowPath::Cores);
    assert_eq!(row_path(both, true, false, recent, now), RowPath::Cores);
    assert_eq!(
        row_path(both, true, false, now - CORE_ARCHIVE_RETENTION_MS - 1, now),
        RowPath::TooOldForCores
    );
}

/// With the core switch off the split is the one before it existed, each skip under its reason.
#[test]
fn with_the_core_switch_off_skips_keep_their_reasons() {
    let now = 1_790_000_000_000;
    let recent = now - 3_600_000;
    assert_eq!(
        row_path((true, false), true, true, recent, now),
        RowPath::Venue
    );
    assert_eq!(
        row_path((true, false), false, false, recent, now),
        RowPath::NoRoute
    );
    assert_eq!(
        row_path((true, false), true, false, recent, now),
        RowPath::PastRetention
    );
    assert_eq!(
        row_path((false, false), true, true, recent, now),
        RowPath::VenueOff
    );
}

/// The tick holds the autoload lock on the UI thread while it starts a pass; a second take on the
/// same thread must fail loudly, never hang the terminal as the station-target read once did.
#[test]
#[should_panic(expected = "autoload lock taken twice")]
fn a_second_take_of_the_lock_on_one_thread_panics_instead_of_hanging() {
    let _held = super::lock();
    let _again = super::lock();
}

/// A tick-style caller reads the station target from the lock it already holds, and returns.
#[test]
fn the_station_target_is_read_under_the_held_lock_without_retaking_it() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let held = super::lock();
        let _ = super::station_target_of(&held);
        drop(held);
        tx.send(()).unwrap();
    });
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("station target read under the held lock did not return");
}
