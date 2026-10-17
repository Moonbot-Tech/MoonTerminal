use super::*;

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
    let need = required_spans(&held_trade.2).hull().expect("a need");
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
    let old = now - CORE_HORIZON_MS - 1;
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
        row_path(defaults, false, false, now - CORE_HORIZON_MS, now),
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
        row_path(both, true, false, now - CORE_HORIZON_MS - 1, now),
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
