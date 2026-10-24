use super::*;

fn key() -> Key {
    ("1:00000000".to_string(), "ACEUSDT".to_string())
}

const TRADE: TradeId = TradeId {
    core: 7,
    rec_id: 42,
};

#[test]
fn a_later_close_of_the_same_trade_replaces_the_first() {
    let mut closed = ClosedTrades::default();
    closed.closed(TRADE, &key(), 1_000, 2_000);
    closed.closed(TRADE, &key(), 1_000, 3_000);
    assert_eq!(closed.live.len(), 1);
    assert_eq!(closed.live[&TRADE].close_ms, 3_000);
}

#[test]
fn a_close_is_forgotten_after_the_memory_runs_out() {
    let mut closed = ClosedTrades::default();
    closed.closed(TRADE, &key(), 1_000, 2_000);
    let seen = closed.live[&TRADE].seen;
    closed.prune(seen + CLOSED_MEMORY - Duration::from_secs(1));
    assert!(closed.live.contains_key(&TRADE));
    closed.prune(seen + CLOSED_MEMORY);
    assert!(closed.live.is_empty());
}

/// A listed row of [`TRADE`] on [`key`], as the protection set holds it.
fn listed_row(open_ms: i64, close_ms: i64) -> CatchupTrade {
    CatchupTrade {
        id: TRADE,
        exchange: key().0,
        market: key().1,
        venue: None,
        core: None,
        open_ms,
        close_ms,
        emulator: false,
        tunable: true,
    }
}

/// A listed row answers from the protection set for the hour after its own page, as its copy
/// in the map used to, and keeps no copy of its own here.
#[test]
fn a_listed_row_answers_for_the_hour_after_its_page() {
    let mut closed = ClosedTrades::default();
    let row = listed_row(1_000, 2_000);
    closed.listed(std::slice::from_ref(&row));
    let at = closed.listed[&TRADE];
    assert!(
        closed.live.is_empty(),
        "a listed row is not copied into the map"
    );
    let found = closed.lookup(
        TRADE,
        Some(&row),
        at + CLOSED_MEMORY - Duration::from_secs(1),
    );
    assert_eq!(found, Some((key(), 1_000, 2_000)));
    assert_eq!(closed.lookup(TRADE, Some(&row), at + CLOSED_MEMORY), None);
    assert_eq!(closed.lookup(TRADE, None, at), None);
}

/// A trade the protection set holds but no listing page carried — a protecting page or a live
/// close's protect — stays unknown here, as it was when only pages and closes wrote the map.
#[test]
fn a_row_no_page_carried_is_not_answered() {
    let mut closed = ClosedTrades::default();
    let mut other = listed_row(1_000, 2_000);
    other.id.rec_id += 1;
    closed.listed(std::slice::from_ref(&other));
    let row = listed_row(1_000, 2_000);
    assert_eq!(closed.lookup(TRADE, Some(&row), Instant::now()), None);
}

/// The later of a live close and a listing page decides the answer, as when both wrote here.
#[test]
fn the_later_of_a_live_close_and_a_listing_wins() {
    let mut closed = ClosedTrades::default();
    let row = listed_row(1_000, 2_000);
    closed.closed(TRADE, &key(), 1_100, 2_100);
    let now = Instant::now();
    assert_eq!(
        closed.lookup(TRADE, Some(&row), now),
        Some((key(), 1_100, 2_100))
    );
    closed.listed(std::slice::from_ref(&row));
    assert_eq!(
        closed.lookup(TRADE, Some(&row), now),
        Some((key(), 1_000, 2_000))
    );
    closed.closed(TRADE, &key(), 1_200, 2_200);
    assert_eq!(
        closed.lookup(TRADE, Some(&row), now),
        Some((key(), 1_200, 2_200))
    );
    assert!(!closed.listed.contains_key(&TRADE));
}

/// Stale listing stamps are dropped at most once a minute, and only past the hour.
#[test]
fn stale_listing_stamps_are_dropped_on_the_minute() {
    let mut closed = ClosedTrades::default();
    let row = listed_row(1_000, 2_000);
    closed.listed(std::slice::from_ref(&row));
    let at = closed.listed[&TRADE];
    let first = at + CLOSED_MEMORY - Duration::from_secs(30);
    closed.prune(first);
    assert!(closed.listed.contains_key(&TRADE), "still inside its hour");
    closed.prune(at + CLOSED_MEMORY);
    assert!(
        closed.listed.contains_key(&TRADE),
        "stale, but within a minute of the last drop"
    );
    closed.prune(first + LISTED_PRUNE_EVERY);
    assert!(closed.listed.is_empty());
}

/// A listed row the protection set drops keeps answering until its page's hour ends, from a copy
/// of its own; a row past its hour, or never listed, gets none.
#[test]
fn a_dropped_listed_row_answers_out_its_hour() {
    let mut closed = ClosedTrades::default();
    let row = listed_row(1_000, 2_000);
    closed.listed(std::slice::from_ref(&row));
    let at = closed.listed[&TRADE];
    closed.expired(row.clone(), at + Duration::from_secs(10));
    assert!(!closed.listed.contains_key(&TRADE));
    let before_end = at + CLOSED_MEMORY - Duration::from_secs(1);
    assert_eq!(
        closed.lookup(TRADE, None, before_end),
        Some((key(), 1_000, 2_000))
    );
    closed.prune(at + CLOSED_MEMORY);
    assert_eq!(closed.lookup(TRADE, None, at + CLOSED_MEMORY), None);

    let mut late = ClosedTrades::default();
    late.listed(std::slice::from_ref(&row));
    let at = late.listed[&TRADE];
    late.expired(row.clone(), at + CLOSED_MEMORY);
    assert!(late.live.is_empty() && late.listed.is_empty());

    let mut never = ClosedTrades::default();
    never.expired(row, Instant::now());
    assert!(never.live.is_empty());
}
