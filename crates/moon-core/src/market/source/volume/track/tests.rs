use super::*;

/// A track that has been seeded and can speak for the whole hour behind `now`.
fn seeded(now: i64) -> MarketTrack {
    let mut track = MarketTrack::new(now);
    track.seeded = true;
    track.earliest_ms = now - TRACK_SPAN_MS;
    track
}

fn trade(ms: i64, price: f32, qty: f32) -> TradeHistoryRow {
    TradeHistoryRow {
        time: MoonTime::from_unix_millis(ms),
        price,
        qty,
    }
}

fn mini(ms: i64, buy: f32, sell: f32, cnt: i32) -> MiniCandle {
    MiniCandle {
        time: MoonTime::from_unix_millis(ms),
        cnt,
        min_price: 1.0,
        max_price: 2.0,
        buy_vol: buy,
        sell_vol: sell,
    }
}

/// Only the buckets INSIDE the period may be summed.
///
/// Breakage: the whole point of keeping an hour is answering a minute from it. Summing the array
/// blindly would print the hour's volume under a caption that says `1м`.
#[test]
fn a_read_sums_only_the_buckets_the_period_covers() {
    let now = 3_600_000_000;
    let mut track = seeded(now);
    track.add_trade(trade(now - 10_000, 100.0, 1.0)); // inside a minute
    track.add_trade(trade(now - 30_000, 100.0, 2.0)); // inside a minute
    track.add_trade(trade(now - 400_000, 100.0, 5.0)); // older than a minute

    let minute = track.read_range(now - 60_000, now);
    assert_eq!(minute.buy_quote, 300.0);
    assert_eq!(minute.trades, 2);

    let ten_minutes = track.read_range(now - 600_000, now);
    assert_eq!(ten_minutes.buy_quote, 800.0);
    assert_eq!(ten_minutes.trades, 3);
}

/// A slot reused by a newer period must not carry the old one's figures.
///
/// Breakage: the buckets are addressed by `id % TRACK_BUCKETS`, so an hour later the same slot comes
/// round again. Without the restart the caption would add trading from exactly one hour ago to the
/// current minute — a figure that looks plausible and is wrong only sometimes.
#[test]
fn a_slot_that_comes_round_again_starts_empty() {
    let now = 3_600_000_000;
    let mut track = seeded(now);
    track.add_trade(trade(now - TRACK_SPAN_MS, 100.0, 7.0));
    track.add_trade(trade(now, 100.0, 1.0));

    let minute = track.read_range(now - 60_000, now);
    assert_eq!(minute.buy_quote, 100.0, "only the fresh trade counts");
    assert_eq!(minute.trades, 1);
}

/// A sale is spelled by the quantity's sign, and lands on the selling side as a positive amount.
#[test]
fn a_sale_lands_on_the_selling_side() {
    let now = 3_600_000_000;
    let mut track = seeded(now);
    track.add_trade(trade(now, 10.0, -3.0));

    let minute = track.read_range(now - 60_000, now);
    assert_eq!(minute.sell_quote, 30.0);
    assert_eq!(minute.sell_base, 3.0);
    assert_eq!(minute.buy_quote, 0.0);
}

/// A bucket seeded from an aggregate cannot state a coin amount, and says so.
///
/// Breakage: mini-candles carry `price × quantity`. A period touching one would otherwise report an
/// exact coin figure that is simply missing that stretch.
#[test]
fn an_aggregate_bucket_marks_the_coin_amount_inexact() {
    let now = 3_600_000_000;
    let mut track = seeded(now);
    track.add_mini(mini(now - 20_000, 500.0, 250.0, 9));
    track.add_trade(trade(now, 10.0, 1.0));

    let minute = track.read_range(now - 60_000, now);
    assert_eq!(minute.buy_quote, 510.0);
    assert_eq!(minute.trades, 10);
    assert!(!minute.base_exact);

    // A period that touches no aggregate keeps its exact quantities: the bucket holding the mini is
    // twenty seconds back, so a five-second window ending now is clear of it.
    let recent = track.read_range(now - 5_000, now);
    assert!(recent.base_exact);
    assert_eq!(recent.buy_base, 1.0);
}

/// A track that was seeded a minute ago cannot speak for the quarter hour.
///
/// Breakage: the completeness flag is what puts the `~` on the caption. A track reporting `complete`
/// for a period it only partly covers states a fraction as the whole.
#[test]
fn the_track_reports_what_it_cannot_cover() {
    let now = 3_600_000_000;
    let mut track = MarketTrack::new(now);
    track.seeded = true;
    track.earliest_ms = now - 60_000;

    assert!(track.read_range(now - 60_000, now).complete);
    assert!(!track.read_range(now - 900_000, now).complete);
}

/// A window reaching past the newest row is not covered, however far back it starts.
///
/// Breakage: the measuring anchor CENTRES its window on the pointer, so a pointer near the live edge
/// asks for a stretch half of which has not happened. Reporting that as whole prints half a period's
/// volume under a heading naming the whole one — and the reader has no way to see the difference.
#[test]
fn a_window_running_past_the_live_edge_is_marked() {
    let now = 3_600_000_000;
    let mut track = seeded(now);
    track.newest_ms = now;
    track.add_trade(trade(now - 10_000, 100.0, 1.0));

    let settled = track.read_range(now - 60_000, now);
    assert!(settled.complete, "a window that ends now is covered");

    let ahead = track.read_range(now - 30_000, now + 30_000);
    assert!(
        !ahead.complete,
        "half of this window is in the future and must say so"
    );
    assert_eq!(
        ahead.buy_quote, 100.0,
        "the half that did happen is still stated"
    );
}
