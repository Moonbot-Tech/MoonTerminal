use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::*;
use crate::market::trade_replay::venue_caps::{KlineRoute, TradeRoute};

/// Asking records nothing: two lanes of one host may both be sent to while neither has been
/// refused, and a refusal is what starts the backoff — at the floor, doubling per refusal in a
/// row, gone once the venue answers again.
#[test]
fn only_a_refusal_starts_the_backoff_and_an_answer_ends_it() {
    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let t0 = Instant::now();
    assert_eq!(gate.claim(host, t0), Ok(()));
    assert_eq!(
        gate.claim(host, t0),
        Ok(()),
        "a second asker is not refused by the first"
    );
    gate.refuse(host, t0);
    assert_eq!(gate.claim(host, t0), Err(RETRY_MIN_S));
    assert_eq!(
        gate.claim(host, t0 + Duration::from_secs(u64::from(RETRY_MIN_S))),
        Ok(()),
        "the floor waited out"
    );
    // A second refusal in a row doubles the wait.
    gate.refuse(host, t0 + Duration::from_secs(u64::from(RETRY_MIN_S)));
    assert_eq!(
        gate.refused_for(host, t0 + Duration::from_secs(u64::from(RETRY_MIN_S))),
        Some(2 * RETRY_MIN_S)
    );
    let refused_at = t0 + Duration::from_secs(u64::from(RETRY_MIN_S));
    // An answer to a request sent before or AT the refusal proves nothing and leaves it
    // standing; one sent after it erases the history.
    gate.clear(host, refused_at - Duration::from_millis(1));
    gate.clear(host, refused_at);
    assert_eq!(gate.refused_for(host, refused_at), Some(2 * RETRY_MIN_S));
    gate.clear(host, refused_at + Duration::from_millis(1));
    assert_eq!(gate.claim(host, t0), Ok(()), "an answer erases the history");
    assert_eq!(
        gate.claim("api.gateio.ws", t0),
        Ok(()),
        "another host is another budget"
    );
}

/// Two threads pacing one host — a chart lane beside a model lane — never book slots closer
/// than the floor to each other. Judged on the BOOKED slots, which the gate sets exactly; the
/// wake-ups after them are the scheduler's and are not what the floor promises.
#[test]
fn two_lanes_of_one_host_keep_the_floor_between_them() {
    use std::sync::{Arc, Mutex};
    let gate = Arc::new(ReplayGate::new());
    let host = "fapi.binance.com";
    let floor = Duration::from_millis(40);
    let booked: Arc<Mutex<Vec<Instant>>> = Arc::new(Mutex::new(Vec::new()));
    let lanes: Vec<_> = (0..2)
        .map(|_| {
            let gate = gate.clone();
            let booked = booked.clone();
            std::thread::spawn(move || {
                for _ in 0..4 {
                    let slot = gate.pace_at(host, floor);
                    booked.lock().unwrap().push(slot);
                }
            })
        })
        .collect();
    for lane in lanes {
        lane.join().unwrap();
    }
    let mut booked = booked.lock().unwrap().clone();
    booked.sort();
    assert_eq!(booked.len(), 8);
    for pair in booked.windows(2) {
        let gap = pair[1].duration_since(pair[0]);
        assert!(
            gap >= floor,
            "two slots {gap:?} apart, under the {floor:?} floor"
        );
    }
}

/// The floor between two requests to one host holds across callers: a second caller books
/// its slot after the FIRST caller's booked slot plus the floor, even while that slot still
/// lies in the future — where `elapsed` would have read zero.
#[test]
fn the_floor_holds_across_two_callers_of_one_host() {
    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let floor = Duration::from_millis(120);
    let started = Instant::now();
    let first = gate.pace_at(host, floor);
    // Booked at once after the first: the floor from the first's slot, not from now.
    let second = gate.pace_at(host, floor);
    assert!(
        second.duration_since(first) >= floor,
        "second slot {:?} after the first",
        second.duration_since(first)
    );
    // A third caller under a smaller floor still books after the second's slot.
    let third = gate.pace_at(host, Duration::from_millis(10));
    assert!(third >= second + Duration::from_millis(10));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "no runaway wait"
    );
}

/// A missing or non-integer `X-MBX-USED-WEIGHT-1M` is not a stop. Exactly 75% of the futures
/// limit is the pace's own ceiling and is not a stop either; one weight over it is.
#[test]
fn a_missing_or_malformed_weight_header_does_not_stop_the_host() {
    assert_eq!(parse_u32_header(None), None);
    assert_eq!(parse_u32_header(Some("")), None);
    assert_eq!(parse_u32_header(Some("   ")), None);
    assert_eq!(parse_u32_header(Some("nope")), None);
    assert_eq!(parse_u32_header(Some("-1")), None);
    assert_eq!(parse_u32_header(Some("1800.5")), None);
    assert_eq!(
        parse_u32_header(Some("Wed, 21 Oct 2015 07:28:00 GMT")),
        None,
        "an HTTP-date Retry-After is not a second count"
    );
    assert_eq!(parse_u32_header(Some(" 1801 ")), Some(1801));

    let limit = binance_weight_limit("fapi.binance.com").expect("futures limit");
    assert_eq!(weight_budget(limit), 1_800);
    assert!(!weight_over_share(1_800, limit), "75% exactly is allowed");
    assert!(weight_over_share(1_801, limit));

    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let t0 = Instant::now();
    assert!(
        !gate.note_used_weight(host, 1_800, t0, 1_700_000_000),
        "the ceiling does not stop the host"
    );
    assert_eq!(gate.claim(host, t0), Ok(()));
    assert!(
        !gate.note_used_weight("api.gateio.ws", 9_999, t0, 1_700_000_000),
        "a venue without a Binance weight limit is not stopped by the header"
    );
}

/// Over 75%, the host waits until the next UTC minute and a second reading of the same stop
/// does not move that deadline. Another host is not involved.
#[test]
fn used_weight_over_75_percent_pauses_until_the_next_utc_minute() {
    let unix = 1_700_000_000u64;
    let into = (unix % 60) as u32;
    let delay = if into == 0 { 60 } else { 60 - into };
    assert_eq!(secs_until_next_utc_minute(unix), delay);
    assert_eq!(secs_until_next_utc_minute(unix + u64::from(delay)), 60);

    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let t0 = Instant::now();
    assert!(gate.note_used_weight(host, 1_801, t0, unix));
    assert_eq!(gate.refused_for(host, t0), Some(delay));
    assert!(
        !gate.note_used_weight(host, 2_000, t0 + Duration::from_secs(5), unix + 5),
        "a second header in the same stop is not a new stop"
    );
    assert!(
        delay > 5,
        "the fixture must leave a remainder after five seconds"
    );
    assert_eq!(
        gate.refused_for(host, t0 + Duration::from_secs(5)),
        Some(delay - 5),
        "the second header must not move the original deadline"
    );
    // A 200 for a request sent after the stop does not lift it: the weight window is the
    // IP's, and one answered page does not reset it.
    gate.clear(host, t0 + Duration::from_millis(1));
    assert_eq!(
        gate.refused_for(host, t0),
        Some(delay),
        "a success does not lift a weight stop that has not elapsed"
    );

    let boundary = ReplayGate::new();
    assert!(boundary.note_used_weight(host, 1_801, t0, unix));
    assert_eq!(
        boundary.claim(host, t0 + Duration::from_secs(u64::from(delay) - 1)),
        Err(1)
    );
    assert_eq!(
        boundary.claim(host, t0 + Duration::from_secs(u64::from(delay))),
        Ok(()),
        "the minute boundary is when sending resumes"
    );
    assert_eq!(
        boundary.claim("dapi.binance.com", t0),
        Ok(()),
        "the other futures host keeps its own budget"
    );
}

/// `Retry-After` is the venue's own number: a 418 longer than 600 s is not clamped, and a
/// 429 shorter than 30 s is not raised to the curve floor. A later shorter header does not
/// cut a ban that is still standing.
#[test]
fn retry_after_outranks_the_backoff_and_is_not_clamped() {
    let gate = ReplayGate::new();
    let host = "dapi.binance.com";
    let t0 = Instant::now();
    gate.refuse(host, t0);
    assert_eq!(
        gate.refused_for(host, t0),
        Some(RETRY_MIN_S),
        "no Retry-After: the curve's floor"
    );
    gate.honour_retry_after(host, t0, 7_200);
    assert_eq!(gate.refused_for(host, t0), Some(7_200));
    assert_eq!(
        gate.claim(host, t0 + Duration::from_secs(u64::from(RETRY_MAX_S))),
        Err(7_200 - RETRY_MAX_S),
        "the 600 s ceiling does not shorten a 418"
    );
    gate.honour_retry_after(host, t0, 5);
    assert_eq!(
        gate.refused_for(host, t0),
        Some(7_200),
        "a short 429 must not cut a standing 418"
    );

    let short = ReplayGate::new();
    short.honour_retry_after(host, t0, 5);
    assert_eq!(
        short.refused_for(host, t0),
        Some(5),
        "a short 429 is not raised to the 30 s floor"
    );
    assert_eq!(short.claim(host, t0 + Duration::from_secs(5)), Ok(()));
}

/// A 418's Retry-After stays when the other lane then records a short 429.
///
/// `honour_retry_after` replacing the stored attempt lets that 429 cut a multi-hour ban
/// down to a few seconds, and the next page goes out while the IP is still banned.
#[test]
fn a_short_retry_after_leaves_a_longer_ban_standing() {
    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let t0 = Instant::now();
    gate.honour_retry_after(host, t0, 7_200);
    gate.honour_retry_after(host, t0 + Duration::from_secs(1), 5);
    assert_eq!(gate.refused_for(host, t0), Some(7_200));
    assert_eq!(
        gate.claim(host, t0 + Duration::from_secs(5)),
        Err(7_200 - 5),
        "five seconds of the ban have elapsed, not the short 429"
    );
}

/// A transient error's 30 s curve does not replace a weight stop that ends later.
///
/// `refuse` inserting a fresh curve lets a non-429 failure on the other lane open the
/// host before the UTC minute resets, and the next page is sent into a weight window
/// Binance is already counting past 75%.
#[test]
fn a_transient_refusal_leaves_a_weight_stop_standing() {
    let unix = 1_700_000_000u64;
    let remain = secs_until_next_utc_minute(unix);
    assert!(
        remain > RETRY_MIN_S,
        "the fixture minute must outlast the curve floor"
    );
    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let t0 = Instant::now();
    assert!(gate.note_used_weight(host, 1_801, t0, unix));
    gate.refuse(host, t0);
    assert_eq!(gate.refused_for(host, t0), Some(remain));
    assert_eq!(
        gate.claim(host, t0 + Duration::from_secs(u64::from(RETRY_MIN_S))),
        Err(remain - RETRY_MIN_S),
        "the curve floor has elapsed and the weight stop has not"
    );
}

/// A weight stop does not cut a curve that already runs past the minute boundary.
///
/// `note_used_weight` writing its minute remainder over a longer curve lets the host
/// resume when the UTC minute ends, while the backoff the failures earned is still
/// unelapsed, and the next page hits the same limit again.
#[test]
fn a_weight_stop_leaves_a_longer_curve_standing() {
    let unix = 1_700_000_000u64;
    let remain = secs_until_next_utc_minute(unix);
    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let t0 = Instant::now();
    // 30 s, then 60 s, then 120 s: past any minute remainder.
    gate.refuse(host, t0);
    gate.refuse(host, t0);
    gate.refuse(host, t0);
    assert_eq!(gate.refused_for(host, t0), Some(120));
    assert!(
        !gate.note_used_weight(host, 1_801, t0, unix),
        "a minute remainder of {remain} s must not replace a 120 s curve"
    );
    assert_eq!(gate.refused_for(host, t0), Some(120));
}

/// A success does not erase a 418 or a weight stop that has not elapsed.
///
/// `clear` removing every kind once `asked_at` is later lets one lane's 200, for a
/// request sent after the stop was recorded, lift the other lane's ban or open the
/// host during the hot minute.
#[test]
fn a_success_leaves_an_unelapsed_ban_or_weight_stop() {
    let host = "fapi.binance.com";
    let t0 = Instant::now();
    let unix = 1_700_000_000u64;
    let remain = secs_until_next_utc_minute(unix);

    let ban = ReplayGate::new();
    ban.honour_retry_after(host, t0, 7_200);
    ban.clear(host, t0 + Duration::from_millis(1));
    assert_eq!(ban.refused_for(host, t0), Some(7_200));

    let stopped = ReplayGate::new();
    assert!(stopped.note_used_weight(host, 1_801, t0, unix));
    stopped.clear(host, t0 + Duration::from_millis(1));
    assert_eq!(stopped.refused_for(host, t0), Some(remain));

    // The curve is the opposite: an answer to a request sent after the refusal still
    // ends it. That contract is `only_a_refusal_starts_the_backoff_and_an_answer_ends_it`.
    let curve = ReplayGate::new();
    curve.refuse(host, t0);
    curve.clear(host, t0 + Duration::from_millis(1));
    assert_eq!(curve.claim(host, t0), Ok(()));
}

/// A short Retry-After folded under a longer curve still stands after a success.
///
/// `merge_wait` keeping only the curve kind lets `clear` delete the entry, so a 200
/// for a request sent after the curve opens the host while that 429 is still in force.
#[test]
fn a_shorter_retry_after_under_a_curve_survives_clear() {
    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let t0 = Instant::now();
    gate.refuse(host, t0);
    gate.honour_retry_after(host, t0 + Duration::from_secs(2), 5);
    assert_eq!(
        gate.refused_for(host, t0),
        Some(RETRY_MIN_S),
        "the 30 s curve is the deadline until a success"
    );
    gate.clear(host, t0 + Duration::from_millis(1_500));
    assert!(
        gate.claim(host, t0 + Duration::from_secs(6)).is_err(),
        "the 429 runs until t0+7, and a success must not open the host at t0+6"
    );
    assert_eq!(
        gate.claim(host, t0 + Duration::from_secs(9)),
        Ok(()),
        "the curve's remaining backoff was cleared; only the 429 was left"
    );
}

/// A weight stop folded under a longer curve still stands after a success.
///
/// `note_used_weight` dropping a merge that did not extend the deadline lets `clear`
/// delete the curve and open the host before the UTC minute resets.
#[test]
fn a_weight_stop_under_a_longer_curve_survives_clear() {
    let unix = 1_700_000_000u64;
    let remain = secs_until_next_utc_minute(unix);
    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let t0 = Instant::now();
    gate.refuse(host, t0);
    gate.refuse(host, t0);
    gate.refuse(host, t0);
    assert_eq!(gate.refused_for(host, t0), Some(120));
    assert!(!gate.note_used_weight(host, 1_801, t0, unix));
    gate.clear(host, t0 + Duration::from_millis(1));
    assert!(
        gate.claim(host, t0 + Duration::from_secs(10)).is_err(),
        "the weight stop outlasts the success"
    );
    assert_eq!(
        gate.claim(host, t0 + Duration::from_secs(u64::from(remain) + 2)),
        Ok(()),
        "the 120 s curve does not outlast the minute it was covering"
    );
}

/// A used-weight stop extends a shorter wait to the minute boundary and does not cut a
/// longer curve or ban down to the seconds left in the minute.
#[test]
fn a_weight_stop_extends_a_shorter_wait_and_does_not_shorten_a_longer_one() {
    let unix = 1_700_000_000u64;
    let remain = secs_until_next_utc_minute(unix);
    assert!(remain > 10, "the fixture minute must leave more than 10 s");
    let almost_due = unix + u64::from(remain - 10);
    assert_eq!(secs_until_next_utc_minute(almost_due), 10);

    let host = "fapi.binance.com";
    let t0 = Instant::now();
    let curve = ReplayGate::new();
    curve.refuse(host, t0);
    assert_eq!(curve.refused_for(host, t0), Some(RETRY_MIN_S));
    assert!(
        !curve.note_used_weight(host, 1_801, t0, almost_due),
        "10 s left in the minute must not replace a 30 s curve"
    );
    assert_eq!(curve.refused_for(host, t0), Some(RETRY_MIN_S));

    let short = ReplayGate::new();
    short.honour_retry_after(host, t0, 5);
    assert!(
        short.note_used_weight(host, 1_801, t0, unix),
        "a 5 s Retry-After is shorter than the minute remainder"
    );
    assert_eq!(short.refused_for(host, t0), Some(remain));

    let ban = ReplayGate::new();
    ban.honour_retry_after(host, t0, 7_200);
    assert!(!ban.note_used_weight(host, 1_801, t0, unix));
    assert_eq!(ban.refused_for(host, t0), Some(7_200));
}

/// Futures aggTrades at the 670 ms floor spend at most 1 800 weight a minute, and a 1 500-row
/// kline on the same host cannot push any 60 s window over that budget.
#[test]
fn the_pace_keeps_agg_trades_and_klines_under_1800_a_minute() {
    let limit = binance_weight_limit("fapi.binance.com").expect("futures limit");
    let budget = weight_budget(limit);
    assert_eq!(budget, 1_800);
    let floor = TradeRoute::BinanceUsdMAggTrades.page_interval();
    assert!(floor >= Duration::from_millis(667));
    assert_eq!(floor, Duration::from_millis(670));
    let trade_weight = TradeRoute::BinanceUsdMAggTrades.request_weight();
    let kline_weight = KlineRoute::BinanceUsdM.request_weight();
    assert_eq!(trade_weight, 20);
    assert_eq!(kline_weight, 10);
    assert_eq!(
        crate::market::trade_replay::venue_caps::futures_kline_weight(1_500),
        10
    );
    assert_eq!(
        crate::market::trade_replay::venue_caps::futures_kline_weight(1_000),
        5
    );
    assert_eq!(
        crate::market::trade_replay::venue_caps::futures_kline_weight(100),
        2
    );
    // Spot's documented weights at the default 100 ms floor already sit under 75% of 6 000,
    // which is why that host joins the same ledger without a longer floor.
    let spot_budget = weight_budget(binance_weight_limit("data-api.binance.vision").unwrap());
    assert_eq!(spot_budget, 4_500);
    let spot_flat_out = 600 * TradeRoute::BinanceSpotAggTrades.request_weight()
        + 600 * KlineRoute::BinanceSpot.request_weight();
    assert!(
        spot_flat_out <= spot_budget,
        "spot at the 100 ms floor spends {spot_flat_out}, budget {spot_budget}"
    );

    let mut weights = VecDeque::new();
    let mut last = None;
    let start = Instant::now();
    let mut now = start;
    for _ in 0..90 {
        let slot = book_slot(last, &mut weights, now, floor, trade_weight, budget);
        last = Some(slot);
        now = slot;
    }
    let elapsed = now.saturating_duration_since(start);
    assert!(
        elapsed < Duration::from_secs(60),
        "90 pages at 670 ms fit in a minute, took {elapsed:?}"
    );
    let spent: u32 = weights.iter().map(|(_, weight)| *weight).sum();
    assert!(spent <= budget, "aggTrades booked {spent}");

    let kline_at = book_slot(
        last,
        &mut weights,
        now,
        Duration::from_millis(100),
        kline_weight,
        budget,
    );
    assert!(
        kline_at > now,
        "a full minute of aggTrades leaves no room for a weight-10 kline"
    );
    let window_start = kline_at
        .checked_sub(Duration::from_secs(60))
        .unwrap_or(kline_at);
    let in_window: u32 = weights
        .iter()
        .filter(|(at, _)| *at > window_start && *at <= kline_at)
        .map(|(_, weight)| *weight)
        .sum();
    assert!(
        in_window <= budget,
        "window holding the kline spent {in_window}"
    );
}
