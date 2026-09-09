use super::*;

fn trade(at_ms: u64, coin: &str, profit: f64) -> Trade {
    Trade::new(at_ms, coin, profit)
}

#[test]
fn wins_and_losses_are_summed_apart() {
    let mut minute = Minute::new();
    minute.push(trade(0, "BTC", 9_000.0));
    minute.push(trade(10, "BTC", -8_000.0));
    minute.tick(20);

    let stat = minute.get("BTC").expect("BTC is in the window");
    assert_eq!(stat.plus, 9_000.0);
    assert_eq!(stat.minus, 8_000.0);
    // A loud minute, not a quiet one: that is the whole reason the two are kept apart.
    assert_eq!(stat.money, 17_000.0);
    assert_eq!(stat.net(), 1_000.0);
}

#[test]
fn a_trade_that_ages_out_takes_its_money_with_it() {
    // The column is headed MINUTE, so it has to mean the minute. An earlier version held each
    // coin's loudest moment instead, and since a coin that trades every few seconds never leaves
    // the window at all, that peak was really its loudest moment since the screen opened — a row
    // still claiming fifty thousand long after the minute that made it had gone.
    let mut minute = Minute::new();
    minute.push(trade(0, "SOL", 100.0));
    minute.push(trade(59_000, "SOL", 10.0));
    minute.tick(59_100);
    let stat = minute.get("SOL").expect("in window");
    assert_eq!(stat.money, 110.0);
    assert_eq!(stat.trades, 2);

    minute.tick(61_000);
    let stat = minute.get("SOL").expect("still in window");
    assert_eq!(
        stat.money, 10.0,
        "a trade that left the window kept paying into it"
    );
    assert_eq!(stat.plus, 10.0);
    assert_eq!(stat.trades, 1);
    assert_eq!(
        stat.trades_plus, 1,
        "the count outlived the money it counted"
    );
}

#[test]
fn a_coin_that_leaves_the_window_leaves_entirely() {
    let mut minute = Minute::new();
    minute.push(trade(0, "PEPE", 50.0));
    minute.tick(100);
    assert!(minute.get("PEPE").is_some());

    minute.tick(WINDOW_MS + 1_000);
    assert!(minute.get("PEPE").is_none(), "a dead coin kept its figures");
    assert!(minute.is_empty());
}

#[test]
fn a_broken_profit_is_dropped_at_the_door() {
    let mut minute = Minute::new();
    minute.push(trade(0, "ETH", f64::NAN));
    minute.push(trade(0, "ETH", f64::INFINITY));
    minute.push(trade(0, "", 10.0));
    minute.tick(10);
    assert!(minute.is_empty());
    assert!(minute.get("ETH").is_none());
}

#[test]
fn ranking_is_by_the_size_of_the_minute_and_is_stable() {
    let mut minute = Minute::new();
    minute.push(trade(0, "AAA", 40.0));
    minute.push(trade(0, "AAA", -40.0));
    minute.push(trade(0, "BBB", 50.0));
    minute.push(trade(0, "CCC", 50.0));
    minute.tick(10);

    let ranked = minute.ranked();
    assert_eq!(
        ranked[0].0, "AAA",
        "a loud two-sided coin must outrank a quiet winner"
    );
    // Equal money resolves by name, so a run is reproducible: hash order is not.
    assert_eq!(ranked[1].0, "BBB");
    assert_eq!(ranked[2].0, "CCC");
}

#[test]
fn the_table_shows_the_loudest_and_no_more_than_it_was_asked_for() {
    let mut minute = Minute::new();
    for (index, coin) in ["AAA", "BBB", "CCC", "DDD"].iter().enumerate() {
        minute.push(trade(0, coin, 100.0 * (index as f64 + 1.0)));
    }
    minute.tick(10);
    let rows = minute.standings(2);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].coin, "DDD");
    assert_eq!(rows[1].coin, "CCC");
}

#[test]
fn a_quiet_market_is_an_empty_table_and_not_a_stale_one() {
    let mut minute = Minute::new();
    minute.push(trade(0, "BTC", 500.0));
    minute.tick(10);
    assert_eq!(minute.standings(10).len(), 1);
    // A minute later that trade is out of the window, and the table has to empty with it.
    minute.tick(WINDOW_MS + 1_000);
    assert!(minute.standings(10).is_empty());
}

#[test]
fn a_step_with_no_arrivals_changes_nothing() {
    // The sums are a function of the buffer, so a tick where nothing arrived and nothing aged out
    // must leave them exactly as they were — that is what lets the pass over the whole minute be
    // skipped on a quiet second.
    let mut minute = Minute::new();
    minute.push(trade(0, "BTC", 900.0));
    minute.push(trade(0, "BTC", -400.0));
    minute.tick(100);
    let settled = minute.get("BTC").expect("a coin").clone();

    for step in 1..=60 {
        minute.tick(100 + step * 16);
    }
    let after = minute.get("BTC").expect("the coin survived");
    assert_eq!(after.plus, settled.plus);
    assert_eq!(after.minus, settled.minus);
    assert_eq!(after.money, settled.money);
    assert_eq!(after.trades, settled.trades);
    assert_eq!(after.trades_plus, settled.trades_plus);
    assert_eq!(after.trades_minus, settled.trades_minus);

    // And a trade arriving after all that silence is still counted.
    minute.push(trade(1_100, "BTC", 50.0));
    minute.tick(1_120);
    assert_eq!(minute.get("BTC").expect("a coin").trades, 3);
}
