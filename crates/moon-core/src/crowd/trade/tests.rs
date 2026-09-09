use super::*;

fn collect(seed: u64, seconds: u64) -> Vec<Trade> {
    let mut feed = SyntheticFeed::new(seed);
    let mut out = Vec::new();
    let mut now = 0;
    while now < seconds * 1000 {
        now += 100;
        out.extend(feed.drain(now));
    }
    out
}

#[test]
fn the_same_seed_replays_the_same_market() {
    assert_eq!(collect(42, 120), collect(42, 120));
}

#[test]
fn different_seeds_produce_different_markets() {
    assert_ne!(collect(1, 120), collect(2, 120));
}

#[test]
fn trades_arrive_in_order_and_inside_the_clock() {
    let mut previous = 0;
    for trade in collect(7, 300) {
        assert!(trade.at_ms >= previous, "the feed went backwards");
        previous = trade.at_ms;
        assert!(!trade.coin.is_empty());
        assert!(trade.profit.is_finite() && trade.profit != 0.0);
    }
}

#[test]
fn the_feed_produces_both_sides_and_a_believable_rate() {
    let trades = collect(11, 600);
    let winners = trades.iter().filter(|trade| trade.profit > 0.0).count();
    let losers = trades.len() - winners;
    assert!(winners > 0 && losers > 0, "the feed was one-sided");
    // The measured crowd stream runs 0.24-1.45 trades a second; the synthetic one has no business
    // being an order of magnitude away from that.
    let per_second = trades.len() as f32 / 600.0;
    assert!(
        (0.15..3.0).contains(&per_second),
        "{per_second} trades a second is not a market"
    );
}

#[test]
fn a_stalled_host_does_not_get_a_minute_of_trades_in_one_step() {
    let mut feed = SyntheticFeed::new(3);
    let burst = feed.drain(600_000);
    assert!(
        burst.len() <= 64,
        "{} trades landed in one call",
        burst.len()
    );
}

#[test]
fn quiet_stretches_exist() {
    // Silence is a state of the market, not a failure: the design leans on it for the rest between
    // bursts, so the generator has to produce it.
    let trades = collect(5, 900);
    let mut longest = 0;
    for pair in trades.windows(2) {
        longest = longest.max(pair[1].at_ms - pair[0].at_ms);
    }
    assert!(longest > 3_000, "the longest gap was only {longest} ms");
}

#[test]
fn the_storm_fires_at_the_rate_it_says_it_does() {
    // A measuring stick that is out by six times measures nothing. The floor between two trades
    // is twenty milliseconds for a market — which caps it at fifty a second — and the storm needs
    // its own, or the floor is what gets certified instead of the screen.
    let mut storm = SyntheticFeed::storm(4);
    let mut count = 0;
    for second in 1..=10 {
        count += storm.drain(second * 1000).len();
    }
    let rate = count as f32 / 10.0;
    assert!(
        rate > STORM_PER_SECOND * 0.5,
        "the storm managed {rate} trades a second against {STORM_PER_SECOND}"
    );
    // And it is a storm, not a market: no quiet stretches to fall into.
    assert!(storm.drain(11_000).len() > 100, "the storm went quiet");
}

#[test]
fn an_ordinary_market_is_not_a_storm() {
    // The same source without the flag stays at the rate the live feed actually shows: at most a
    // couple of trades a second over a long stretch.
    let mut feed = SyntheticFeed::new(4);
    let mut count = 0;
    for second in 1..=60 {
        count += feed.drain(second * 1000).len();
    }
    let rate = count as f32 / 60.0;
    assert!(
        rate < 10.0,
        "an ordinary market fired {rate} trades a second"
    );
}
