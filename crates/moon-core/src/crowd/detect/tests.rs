use super::*;
use crate::crowd::trade::Trade;

/// Build a window holding exactly these trades, all inside it.
fn window(trades: &[(&str, f64)]) -> Minute {
    let mut minute = Minute::new();
    for (coin, profit) in trades {
        minute.push(Trade {
            coin: (*coin).to_string(),
            profit: *profit,
            at_ms: 1_000,
        });
    }
    minute.tick(1_000);
    minute
}

/// A rule that fires on anything with two trades and a dollar of profit.
fn loud() -> CrowdRule {
    CrowdRule {
        enabled: true,
        profit: 1.0,
        trades: 2,
    }
}

/// A detector already watching `rule`, with the re-seed its thresholds asked for behind it.
///
/// Moving a line re-seeds (see `a_moved_line_...` below), so a test that only wants "this rule is
/// being watched" burns that pass against an empty window first — where it seeds nothing.
fn watching(rule: CrowdRule) -> Detector {
    let mut detector = Detector::new();
    detector.set_rule(rule);
    detector.scan(&Minute::new(), 0);
    detector
}

#[test]
fn nothing_is_announced_while_the_rule_is_off() {
    let mut detector = Detector::new();
    let minute = window(&[("BTC", 100.0), ("BTC", 100.0)]);
    assert_eq!(detector.scan(&minute, 10_000), 0);
    assert_eq!(detector.head(), 0);
}

#[test]
fn a_coin_over_both_lines_is_announced_once_and_not_again() {
    let mut detector = watching(loud());
    let minute = window(&[("BTC", 100.0), ("BTC", 100.0)]);

    assert_eq!(detector.scan(&minute, 10_000), 1);
    let row = detector.since(0).next().expect("one detect");
    assert_eq!(row.coin, "BTC");
    assert_eq!(row.trades, 2);
    assert!((row.profit - 200.0).abs() < 1e-9);

    // The same window, still above the line: it has already been announced.
    assert_eq!(detector.scan(&minute, 11_000), 0);
    assert_eq!(detector.since(0).count(), 1);
}

#[test]
fn switching_the_rule_on_announces_what_is_already_loud() {
    // Enabling is not a threshold change, so it does NOT re-seed: somebody who has just asked to be
    // told about loud coins is told about the loud ones, rather than waiting for the next dip.
    let mut detector = Detector::new();
    let rule = CrowdRule {
        enabled: true,
        ..CrowdRule::default()
    };
    assert!(detector.set_rule(rule));
    let minute = window(&[("BTC", 900.0), ("BTC", 900.0), ("BTC", 900.0)]);
    assert_eq!(detector.scan(&minute, 10_000), 0, "three trades is not ten");

    detector.set_rule(CrowdRule { trades: 3, ..rule });
    // A moved line seeds instead of firing, and the fall below it is what arms the next crossing.
    assert_eq!(detector.scan(&minute, 20_000), 0);
    assert_eq!(detector.scan(&Minute::new(), 30_000), 0);
    assert_eq!(detector.scan(&minute, 100_000), 1);
}

#[test]
fn the_money_alone_is_not_enough() {
    let mut detector = watching(loud());
    // One big trade clears the money and fails the count: a person, not a crowd.
    let minute = window(&[("BTC", 5_000.0)]);
    assert_eq!(detector.scan(&minute, 10_000), 0);
}

#[test]
fn losses_are_netted_before_the_line_is_read() {
    let mut detector = watching(loud());
    // A loud minute the crowd did NOT make money on: both columns are large and the net is not.
    let minute = window(&[("BTC", 900.0), ("BTC", -899.5)]);
    assert_eq!(
        detector.scan(&minute, 10_000),
        0,
        "a coin the crowd broke even on was announced as a win"
    );
}

#[test]
fn it_takes_a_fall_and_a_gap_before_the_same_coin_fires_again() {
    let mut detector = watching(loud());
    let hot = window(&[("BTC", 100.0), ("BTC", 100.0)]);
    assert_eq!(detector.scan(&hot, 10_000), 1);

    // Back below the line with room to spare: armed again, but still inside the gap.
    let cold = window(&[("BTC", 0.1), ("BTC", 0.1)]);
    assert_eq!(detector.scan(&cold, 20_000), 0);
    assert_eq!(detector.scan(&hot, 30_000), 0, "fired inside the gap");

    // Past the gap, and it is a genuine second crossing.
    assert_eq!(detector.scan(&cold, 80_000), 0);
    assert_eq!(detector.scan(&hot, 90_000), 1);
    assert_eq!(detector.since(0).count(), 2);
}

#[test]
fn hovering_on_the_line_does_not_announce_twice() {
    let mut detector = watching(CrowdRule {
        enabled: true,
        profit: 100.0,
        trades: 2,
    });
    let over = window(&[("BTC", 60.0), ("BTC", 60.0)]);
    assert_eq!(detector.scan(&over, 10_000), 1);
    // 90 is under the line but well inside the hysteresis band, so nothing is re-armed.
    let just_under = window(&[("BTC", 45.0), ("BTC", 45.0)]);
    assert_eq!(detector.scan(&just_under, 200_000), 0);
    assert_eq!(detector.scan(&over, 300_000), 0, "the band did not hold");
}

#[test]
fn a_moved_line_takes_the_market_instead_of_emptying_it_into_the_feed() {
    let mut detector = watching(CrowdRule {
        enabled: true,
        profit: 10_000.0,
        trades: 2,
    });
    let minute = window(&[("BTC", 100.0), ("BTC", 100.0)]);
    assert_eq!(detector.scan(&minute, 10_000), 0);

    // Lowering the line under a coin already above the new one must not announce it.
    detector.set_rule(loud());
    assert_eq!(
        detector.scan(&minute, 20_000),
        0,
        "moving the line emptied the board into the feed"
    );
    // A coin that falls back and crosses the NEW line afterwards is a real crossing.
    let quiet = window(&[("BTC", 0.1), ("BTC", 0.1)]);
    assert_eq!(detector.scan(&quiet, 30_000), 0);
    assert_eq!(detector.scan(&minute, 100_000), 1);
}

#[test]
fn switching_the_rule_off_forgets_what_each_coin_was_doing() {
    let mut detector = watching(loud());
    let minute = window(&[("BTC", 100.0), ("BTC", 100.0)]);
    assert_eq!(detector.scan(&minute, 10_000), 1);

    let mut off = loud();
    off.enabled = false;
    assert!(detector.set_rule(off));
    assert_eq!(detector.scan(&minute, 20_000), 0);

    // Switched back on, the coin is new again — gap included. Somebody who switches the rule off
    // and on is asking the question again, and the honest answer is what is loud NOW; the refire
    // gap exists to stop a coin flapping across the line, not to remember a rule nobody was
    // watching.
    assert!(detector.set_rule(loud()));
    assert_eq!(detector.scan(&minute, 30_000), 1);
}

#[test]
fn a_reader_takes_only_what_is_newer_than_its_cursor() {
    let mut detector = watching(loud());
    let btc = window(&[("BTC", 100.0), ("BTC", 100.0)]);
    detector.scan(&btc, 10_000);
    let cursor = detector.head();
    let eth = window(&[("ETH", 100.0), ("ETH", 100.0)]);
    detector.scan(&eth, 20_000);

    let fresh: Vec<&str> = detector
        .since(cursor)
        .map(|row| row.coin.as_str())
        .collect();
    assert_eq!(fresh, vec!["ETH"]);
}

#[test]
fn the_ring_is_bounded() {
    let mut detector = watching(loud());
    // Each pass announces a coin that has never been seen before, so nothing is suppressed.
    for step in 0..(RING as u64 + 20) {
        let coin = format!("C{step}");
        let minute = window(&[(coin.as_str(), 100.0), (coin.as_str(), 100.0)]);
        detector.scan(&minute, 10_000 + step * 1_000);
    }
    assert_eq!(detector.since(0).count(), RING);
}

#[test]
fn a_coin_that_left_the_window_is_forgotten_once_its_gap_has_run_out() {
    let mut detector = watching(loud());
    let minute = window(&[("BTC", 100.0), ("BTC", 100.0)]);
    detector.scan(&minute, 10_000);
    assert_eq!(detector.watch.len(), 1);

    // The window is empty and the gap has long gone: nothing about that coin is worth carrying.
    let empty = Minute::new();
    detector.scan(&empty, 10_000 + REFIRE_MS * 2);
    assert!(detector.watch.is_empty(), "the watch map grows forever");
}

#[test]
fn a_quiet_window_does_not_report_a_change() {
    // The rule is only ever asked on a window that MOVED, so this is the gate that keeps a quiet
    // market from waking anybody at all.
    let mut minute = Minute::new();
    minute.push(Trade {
        coin: "BTC".to_string(),
        profit: 1.0,
        at_ms: 1_000,
    });
    assert!(minute.tick(1_000), "an arrival must report a change");
    assert!(!minute.tick(1_100), "a quiet second reported a change");
}

/// One pass cannot fill the reader's feed.
///
/// Mutation: drop the cap and a market-wide move — or a line set too low — hands the Detects panel
/// more crossings in one notification than it has seats, and the cards a CORE reported, the ones
/// somebody is trading on, are the ones evicted to make room.
#[test]
fn one_pass_announces_at_most_a_burst_and_takes_the_loudest() {
    let mut detector = watching(loud());
    let rows: Vec<(String, f64)> = (0..20)
        .map(|step| (format!("C{step:02}"), 100.0 + f64::from(step)))
        .collect();
    let window_rows: Vec<(&str, f64)> = rows
        .iter()
        .flat_map(|(coin, profit)| [(coin.as_str(), *profit), (coin.as_str(), *profit)])
        .collect();
    let minute = window(&window_rows);

    assert_eq!(detector.scan(&minute, 10_000), BURST);
    let taken: Vec<&str> = detector.since(0).map(|row| row.coin.as_str()).collect();
    // Loudest first: the biggest minutes of the twenty, and only a burst of them.
    assert_eq!(taken, vec!["C19", "C18", "C17", "C16", "C15"]);

    // The rest are not forgotten — they never fired, so they are still crossing, and the backlog
    // drains a burst at a time.
    let mut fired = BURST;
    for step in 1..4 {
        fired += detector.scan(&minute, 10_000 + step * 1_000);
    }
    assert_eq!(fired, 20, "the backlog did not drain");
    assert_eq!(
        detector.scan(&minute, 20_000),
        0,
        "a coin was announced twice"
    );
}

/// The trade count gets the same band the money does.
///
/// Mutation: compare the count against the bare line and a coin oscillating across it by one trade
/// re-arms every pass, so it is announced again on every refire gap while nothing about the market
/// has changed.
#[test]
fn the_count_line_has_a_band_too() {
    let mut detector = watching(CrowdRule {
        enabled: true,
        profit: 1.0,
        trades: 10,
    });
    let hot: Vec<(&str, f64)> = (0..10).map(|_| ("BTC", 100.0)).collect();
    let hot = window(&hot);
    assert_eq!(detector.scan(&hot, 10_000), 1);

    // Nine trades: under the line, but well inside the band, so nothing is re-armed.
    let nine: Vec<(&str, f64)> = (0..9).map(|_| ("BTC", 100.0)).collect();
    let nine = window(&nine);
    assert_eq!(detector.scan(&nine, 200_000), 0);
    assert_eq!(
        detector.scan(&hot, 300_000),
        0,
        "the count band did not hold"
    );

    // Seven is below the band, and the next crossing is a real one.
    let seven: Vec<(&str, f64)> = (0..7).map(|_| ("BTC", 100.0)).collect();
    assert_eq!(detector.scan(&window(&seven), 400_000), 0);
    assert_eq!(detector.scan(&hot, 500_000), 1);
}

/// A band of zero would be no band at all: one trade is a rule somebody may set.
#[test]
fn the_count_band_never_falls_to_zero() {
    assert_eq!(rearm_trades(1), 1);
    assert_eq!(rearm_trades(10), 8);
    assert_eq!(rearm_trades(0), 1);
}

/// Switching the rule off forgets what was ANNOUNCED as well as what was watched.
///
/// Mutation: keep the ring, and a Detects panel opened in the minute after the rule was switched
/// off is handed cards for a rule nobody is watching.
#[test]
fn switching_the_rule_off_takes_the_announcements_with_it() {
    let mut detector = watching(loud());
    let minute = window(&[("BTC", 100.0), ("BTC", 100.0)]);
    assert_eq!(detector.scan(&minute, 10_000), 1);
    assert_eq!(detector.since(0).count(), 1);

    let mut off = loud();
    off.enabled = false;
    detector.set_rule(off);
    assert_eq!(
        detector.since(0).count(),
        0,
        "a reader could still be handed a card for a rule nobody watches"
    );
}

/// A zero switches a line OFF; it is not a line at zero.
///
/// Mutation: read a zero as an ordinary threshold. `profit = 0` then means "any gain at all" and a
/// losing coin can never be announced, which is the opposite of what the field says it does.
#[test]
fn a_zero_switches_a_line_off() {
    // The money line off: the count decides, whatever the money did — including a loss.
    let mut detector = watching(CrowdRule {
        enabled: true,
        profit: 0.0,
        trades: 2,
    });
    let losing = window(&[("BTC", -50.0), ("BTC", -50.0)]);
    assert_eq!(
        detector.scan(&losing, 10_000),
        1,
        "a switched-off money line still judged the money"
    );

    // The count line off: the money decides, on a single trade.
    let mut detector = watching(CrowdRule {
        enabled: true,
        profit: 1.0,
        trades: 0,
    });
    let one = window(&[("BTC", 50.0)]);
    assert_eq!(detector.scan(&one, 10_000), 1);
}

/// Both lines at zero ask nothing — and a question nobody asked opens no connection.
///
/// Mutation: leave `armed` reading `enabled` alone. The rule then holds the trade socket open for
/// the life of the terminal to evaluate a condition that can never be true.
#[test]
fn both_lines_at_zero_ask_nothing_and_read_nothing() {
    let quiet = CrowdRule {
        enabled: true,
        profit: 0.0,
        trades: 0,
    };
    assert!(
        !quiet.armed(),
        "a rule with no lines still claimed the feed"
    );

    let mut detector = Detector::new();
    detector.set_rule(quiet);
    let minute = window(&[("BTC", 5_000.0), ("BTC", 5_000.0)]);
    assert_eq!(detector.scan(&minute, 10_000), 0);

    // One line back on and it is a rule again.
    let mut live = quiet;
    live.trades = 2;
    assert!(live.armed());
}

/// A negative money line is read as written: `> -500` is "not worse than five hundred down".
#[test]
fn a_negative_line_widens_the_rule_instead_of_inverting_it() {
    let mut detector = watching(CrowdRule {
        enabled: true,
        profit: -500.0,
        trades: 1,
    });
    // Down four hundred is above the line.
    assert_eq!(detector.scan(&window(&[("BTC", -400.0)]), 10_000), 1);
    // Down six hundred is below it.
    assert_eq!(detector.scan(&window(&[("ETH", -600.0)]), 20_000), 0);
}

/// The re-arm band sits BELOW the line on either side of zero.
///
/// Mutation: keep the old `profit * REARM`. On a negative line that lands ABOVE the line — `-500 *
/// 0.8` is `-400` — so a coin that never fell at all counts as having fallen back, and the edge the
/// module is built on stops existing.
#[test]
fn the_band_stays_below_a_negative_line() {
    assert_eq!(rearm_profit(100.0), 80.0);
    assert_eq!(rearm_profit(-500.0), -600.0);

    let mut detector = watching(CrowdRule {
        enabled: true,
        profit: -500.0,
        trades: 1,
    });
    let over = window(&[("BTC", -400.0)]);
    assert_eq!(detector.scan(&over, 10_000), 1);
    // Under the line but inside the band: not re-armed.
    assert_eq!(detector.scan(&window(&[("BTC", -550.0)]), 200_000), 0);
    assert_eq!(detector.scan(&over, 300_000), 0, "the band did not hold");
    // Past the band, and the next crossing is real.
    assert_eq!(detector.scan(&window(&[("BTC", -700.0)]), 400_000), 0);
    assert_eq!(detector.scan(&over, 500_000), 1);
}
