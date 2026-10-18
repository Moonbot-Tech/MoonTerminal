//! Synthetic core-log lines. Tokens and prices here are fixtures, not captures.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use super::*;

/// Reading the previous label as the current price would conceal stale weighted readings.
#[test]
fn parses_weighted_readings() {
    let line = format!("{STOP_LINE} WeightedPricePrev: 0.9850 WeightedPrice: 0.9840");
    let fire = parse_stop_fire(1000, "CORE_A", &line).unwrap();
    assert_eq!(
        (fire.weighted, fire.weighted_prev),
        (Some(0.984), Some(0.985))
    );
    let only_prev = parse_stop_fire(
        1000,
        "CORE_A",
        &format!("{STOP_LINE} WeightedPricePrev: 0.9850"),
    )
    .unwrap();
    assert_eq!(
        (only_prev.weighted, only_prev.weighted_prev),
        (None, Some(0.985))
    );
}

/// Older cores must remain parseable without inventing weighted observations.
#[test]
fn fire_without_weighted_readings() {
    let fire = parse_stop_fire(1000, "CORE_A", STOP_LINE).unwrap();
    assert_eq!((fire.weighted, fire.weighted_prev), (None, None));
}

/// Marker or sign regressions would misattribute modifier and ladder level defects.
#[test]
fn parses_stop_level_events() {
    for (body, kind, level, pct) in [
        (
            "StopLoss applied (buyPrice 1.0000 stop -1.50% => 0.98500)",
            LevelKind::Applied,
            0.985,
            Some(-1.5),
        ),
        (
            "StopLoss applied (buyPrice 1.0000 stop +0.50% => 0.99500)",
            LevelKind::Applied,
            0.995,
            Some(0.5),
        ),
        (
            "Strategy (strategy <STRAT_A>) Second stop applied. Level: 0.99800000 (SecondStopLoss=-0.2000) LastAsk: 1.0100 BuyPrice:1.0000",
            LevelKind::Second,
            0.998,
            None,
        ),
        (
            "Strategy (strategy <STRAT_A>) StopLoss3 applied. Level: 1.0050",
            LevelKind::Third,
            1.005,
            None,
        ),
        (
            "Strategy (strategy <STRAT_A>) StopAboveLiq Level changed: 1.0300",
            LevelKind::AboveLiq,
            1.03,
            None,
        ),
    ] {
        let event = parse_stop_level(
            1000,
            "CORE_A",
            &format!("10:00:00 COIN_A<Short> : [3] (345) {body}"),
        )
        .unwrap();
        assert_eq!((event.kind, event.level, event.pct), (kind, level, pct));
        assert_eq!(event.task, Some(345));
        assert_eq!(event.coin.as_deref(), Some("COIN_A"));
        let events = vec![CoreEvent::Level(event)];
        let key = DealKey {
            strategy: None,
            coin: "COIN_A",
            task: Some(345),
            buy_ms: 1000,
            close_ms: 2000,
        };
        assert_eq!(match_levels(&key, &events).len(), 1);
    }
}

/// Activation fields cannot masquerade as a placement.
#[test]
fn autoactivated_is_not_a_level_event() {
    assert!(parse_stop_level(1000, "CORE_A", STOP_LINE).is_none());
}

/// Stop activation the parser tests read. Dropping `BidEMA:` must not move the other prices.
const STOP_LINE: &str = concat!(
    "10:00:01  COIN_A: [7] (345) StopLoss AutoActivated on price drop: ",
    "BID = 0.9900 ASK: 0.9910 BidEMA: 0.9920 LastPrice = 0.9905 ",
    "BuyPrice = 1.0000 (strategy <STRAT_A>); StopLoss fixed: 0.9950 AllowedDrop: BUY -1% spread: 1%",
);

/// Sell placement the parser tests read.
const SELL_LINE: &str = concat!(
    "10:00:02  COIN_A: [8] (345) Using (strategy <STRAT_A>) ",
    "SELL price: 2% = 1.0200",
);

/// UTC midnight of 2026-10-04, the day in the temp-dir file name.
const DAY_MS: i64 = 1_791_072_000_000;

/// A stop line that omits one field still parses the fields it names.
///
/// Deleting the by-label read, or treating `BidEMA` as required, empties this fire.
#[test]
fn stop_fire_reads_levels_task_strategy_and_clock() {
    let fire = parse_stop_fire(1_000, "CORE_A", STOP_LINE).expect("stop line");
    assert_eq!(fire.t_ms, 1_000);
    assert_eq!(fire.core_label, "CORE_A");
    assert_eq!(fire.task, Some(345));
    assert_eq!(fire.strategy.as_deref(), Some("STRAT_A"));
    assert_eq!(fire.bid, Some(0.9900));
    assert_eq!(fire.ask, 0.9910);
    assert_eq!(fire.bid_ema, Some(0.9920));
    assert_eq!(fire.last, 0.9905);
    assert_eq!(fire.buy, Some(1.0000));
    assert_eq!(fire.coin.as_deref(), Some("COIN_A"));
    assert_eq!(fire.allowed_drop_pct, Some(-1.0));
    assert_eq!(fire.spread_pct, Some(1.0));
    assert!(!fire.trailing);
    assert_eq!(fire.take_profit, None);
    assert_eq!(fire.fixed, Some(0.9950));
    assert_eq!(fire.msg_clock_s, Some(36_001));
}

/// `BidEMA` is optional. Bid, ask, last, and buy are not.
///
/// Filling a missing `BidEMA` with zero, or shifting columns after it is removed,
/// changes these readings.
#[test]
fn stop_fire_allows_missing_bid_ema() {
    let msg = STOP_LINE.replace("BidEMA: 0.9920 ", "");
    let fire = parse_stop_fire(1_000, "CORE_A", &msg).expect("stop line without BidEMA");
    assert_eq!(fire.bid_ema, None);
    assert_eq!(fire.bid, Some(0.9900));
    assert_eq!(fire.ask, 0.9910);
    assert_eq!(fire.last, 0.9905);
    assert_eq!(fire.buy, Some(1.0000));
    assert_eq!(fire.fixed, Some(0.9950));

    let missing_bid = STOP_LINE.replace("BID = 0.9900 ", "");
    assert!(parse_stop_fire(1_000, "CORE_A", &missing_bid).is_none());
}

/// A trailing activation is a stop fire even without `StopLoss AutoActivated`.
///
/// Requiring the `StopLoss` words, or ignoring `Trailing`, drops the take or the flag.
#[test]
fn trailing_activation_sets_flag_and_take_profit() {
    let msg = concat!(
        "10:00:03  COIN_A<Short> : [9] (345) TrailingStop AutoActivated on price drop: ",
        "ASK = 0.9910 LastPrice = 0.9905; PeakPrice = 1.05 FloatTrailing: 2% ; ",
        "allowed drop > TakeProfit 1.0300 BUY +3% ; spread: 1% (strategy <STRAT_A>)",
    );
    let fire = parse_stop_fire(3_000, "CORE_A", msg).expect("trailing line");
    assert!(fire.trailing);
    assert_eq!(fire.take_profit, Some(1.0300));
    assert_eq!(fire.bid_ema, None);
    assert_eq!(fire.fixed, None);
    assert_eq!(fire.bid, None);
    assert_eq!(fire.buy, None);
    assert_eq!(fire.peak, Some(1.05));
    assert_eq!(fire.float_trailing_pct, Some(2.0));
    assert_eq!(fire.coin.as_deref(), Some("COIN_A"));
    assert_eq!(fire.msg_clock_s, Some(36_003));
}

/// Reading the strategy percent as the placement price breaks sell agreement.
#[test]
fn sell_price_line_parses_and_branches_as_sell_price() {
    let placed = parse_sell_placed(2_000, "CORE_A", SELL_LINE).expect("sell line");
    assert_eq!(placed.t_ms, 2_000);
    assert_eq!(placed.core_label, "CORE_A");
    assert_eq!(placed.task, Some(345));
    assert_eq!(placed.strategy.as_deref(), Some("STRAT_A"));
    assert_eq!(placed.sell, 1.0200);
    assert_eq!(placed.sell_price_field, None);
    assert_eq!(placed.sell_pct, Some(2.0));
    assert_eq!(placed.field_pct, None);
    assert_eq!(branch_of(&placed, 1.0, true, 0.01), TakeBranch::SellPrice);
}

/// Branches use the logged reference and explicit SellAdjust, never a model price.
#[test]
fn branch_without_sell_price_field() {
    let placed = parse_sell_placed(2_000, "CORE_A", SELL_LINE).expect("sell line");
    assert_eq!(branch_of(&placed, 0.9, true, 0.01), TakeBranch::AskAdjusted);
    assert_eq!(branch_of(&placed, 1.0, true, 0.01), TakeBranch::SellPrice);
    let short = parse_sell_placed(
        2_000,
        "CORE_A",
        &SELL_LINE.replace("= 1.0200", "= 0.980392"),
    )
    .expect("short");
    assert_eq!(branch_of(&short, 1.0, false, 0.01), TakeBranch::SellPrice);
    let adjusted = "10:00:02  COIN_A: [8] (345) Using (strategy <STRAT_A>) SELL price: 103 (SellAdjust: new min.Price: 101, new depth: 2%, new SellPrice: 104 (+4%)";
    let p = parse_sell_placed(2_000, "CORE_A", adjusted).expect("adjusted");
    assert_eq!(p.sell, 103.0);
    assert_eq!(p.sell_price_field, Some(104.0));
    assert_eq!(p.field_pct, Some(4.0));
    assert!(p.adjust);
    assert_eq!(branch_of(&p, 100.0, true, 0.05), TakeBranch::Adjusted);
    for (msg, predefined) in [
        (
            "10:00:02  COIN_A: [8] (345) Using (strategy <STRAT_A>) SELL price: 102",
            false,
        ),
        (
            "10:00:02  COIN_A: [8] (345) Using predefined SELL price: 102",
            true,
        ),
    ] {
        let p = parse_sell_placed(2_000, "CORE_A", msg).expect("bare");
        assert_eq!(p.sell, 102.0);
        assert_eq!(p.predefined, predefined);
        assert_eq!(branch_of(&p, 100.0, true, 0.05), TakeBranch::Unknown);
    }
}

/// Multiplying a short take instead of dividing misclassifies its logged branch.
#[test]
fn short_branch_uses_inverse_take() {
    for (sell, expected) in [
        ("90.909091", TakeBranch::SellPrice),
        ("90.0", TakeBranch::AskAdjusted),
    ] {
        let msg = format!("10:00:02 COIN_A<Short> : [8] (345) SELL price: 10% = {sell}");
        let placed = parse_sell_placed(2_000, "CORE_A", &msg).expect("short placement");
        assert_eq!(branch_of(&placed, 100.0, false, 0.00001), expected);
    }
}

/// Coin fallback must not borrow another task's stop or placement.
#[test]
fn coin_fallback_rejects_known_task_conflict() {
    let key = deal_key(Some(345), 100_000, 130_000);
    let stops = [stop_event(130_000, &STOP_LINE.replace("(345)", "(346)"))];
    let sells = [sell_event(101_000, &SELL_LINE.replace("(345)", "(346)"))];
    assert!(matches!(match_stop(&key, &stops), Match::None));
    assert!(matches!(match_sell(&key, &sells, 10), Match::None));
}

/// The message is the fourth field and keeps its own tabs. Three fields are not a line.
///
/// A split on every tab, or a clock that requires the fraction, drops one of these.
#[test]
fn split_tsv_keeps_tabs_in_the_message() {
    assert_eq!(
        split_tsv("10:00:01.250\tINFO\tCORE_A\tmsg with\ttab"),
        Some((36_001_250, "msg with\ttab"))
    );
    assert_eq!(
        split_tsv("10:00:01\tINFO\tCORE_A\tmsg"),
        Some((36_001_000, "msg"))
    );
    assert_eq!(split_tsv("10:00:01.250\tINFO\tCORE_A"), None);
    assert_eq!(split_tsv("nope\tINFO\tCORE_A\tmsg"), None);
}

/// The file name is the UTC day and the core label. The terminal log and other names are not.
///
/// Parsing the day in local time, or keeping `*_app.log`, attaches fires to the wrong day or core.
#[test]
fn date_of_file_accepts_core_label_and_skips_app() {
    assert_eq!(
        date_of_file("2026-10-04_CORE_A.log"),
        Some((DAY_MS, "CORE_A"))
    );
    assert_eq!(date_of_file("2026-10-04_app.log"), None);
    assert_eq!(date_of_file("notes.txt"), None);
}

/// A task reused hours after this trade is outside both matching routes.
#[test]
fn matcher_reuse_guard() {
    let close_ms = 1_700_000_000_000;
    let buy_ms = close_ms - 30_000;
    let key = deal_key(Some(345), buy_ms, close_ms);
    assert_eq!(label_of_core("CORE A"), "CORE_A");

    let late = stop_event(close_ms + 2 * 3_600_000, &with_coin(STOP_LINE));
    assert!(matches!(match_stop(&key, &[late]), Match::None));
}

/// Repeated fires in the trade-wide coin window select the nearest close.
#[test]
fn stop_coin_route_nearest_close() {
    let close_ms = 1_700_000_000_000;
    let buy_ms = close_ms - 100_000;
    let word = stop_event(close_ms, &with_coin(STOP_LINE));
    let also = stop_event(close_ms + 1_000, &with_coin(STOP_LINE));
    let coin_key = deal_key(None, buy_ms, close_ms);
    assert!(matches!(
        match_stop(&coin_key, &[word, also]),
        Match::CoinTime(fire) if fire.t_ms == close_ms
    ));
    let earlier = stop_event(close_ms - 20_000, STOP_LINE);
    assert!(matches!(
        match_stop(&coin_key, &[earlier]),
        Match::CoinTime(_)
    ));
}

/// Only the header coin is matched; strategy words or message suffixes cannot supply it.
#[test]
fn coin_route_checks_token_and_strategy() {
    let close_ms = 1_700_000_000_000;
    let coin_key = deal_key(None, close_ms - 30_000, close_ms);
    let glued = stop_event(
        close_ms + 1_000,
        &format!("{} COIN_A", STOP_LINE.replace("COIN_A:", "XCOIN_AY:")),
    );
    let only = stop_event(close_ms, &with_coin(STOP_LINE));
    match match_stop(&coin_key, &[glued, only]) {
        Match::CoinTime(fire) => assert_eq!(fire.t_ms, close_ms),
        other => panic!("expected one coin hit, got {other:?}"),
    }

    let other_strategy = STOP_LINE.replace("(strategy <STRAT_A>)", "(strategy <STRAT_B>)");
    let mismatched = stop_event(close_ms, &with_coin(&other_strategy));
    assert!(matches!(match_stop(&coin_key, &[mismatched]), Match::None));
    let short = stop_event(close_ms, &STOP_LINE.replace("COIN_A:", "coin_a<Short> :"));
    assert!(matches!(
        match_stop(&coin_key, &[short]),
        Match::CoinTime(_)
    ));
    let contract_key = DealKey {
        coin: "COIN_A_0626",
        ..coin_key
    };
    let plain = stop_event(close_ms, STOP_LINE);
    assert!(matches!(
        match_stop(&contract_key, &[plain]),
        Match::CoinTime(_)
    ));
}

/// Task matches choose the fire nearest close instead of the first in file order.
#[test]
fn stop_task_route_nearest_close() {
    let close_ms = 1_700_000_000_000;
    let key = deal_key(Some(345), close_ms - 30_000, close_ms);
    let nearer = stop_event(close_ms + 1_000, STOP_LINE);
    let farther = stop_event(close_ms - 50_000, STOP_LINE);
    match match_stop(&key, &[farther, nearer]) {
        Match::Task(fire) => assert_eq!(fire.t_ms, close_ms + 1_000),
        other => panic!("expected the stop nearest the close, got {other:?}"),
    }
}

/// Both sell routes keep the first placement and ignore SellAdjust replacements.
#[test]
fn sell_routes_keep_first_placement() {
    let sell_buy = 5_000_000_000_000;
    let sell_key = deal_key(Some(345), sell_buy, sell_buy + 30_000);
    let early = sell_event(sell_buy + 2_000, SELL_LINE);
    let late_sell = sell_event(sell_buy + 20_000, SELL_LINE);
    match match_sell(&sell_key, &[late_sell, early], 30) {
        Match::Task(sell) => assert_eq!(sell.t_ms, sell_buy + 2_000),
        other => panic!("expected the earliest sell after the buy, got {other:?}"),
    }

    let sell_coin = deal_key(None, sell_buy, sell_buy + 30_000);
    let placed_glued = sell_event(
        sell_buy + 2_000,
        &format!("{} COIN_A", SELL_LINE.replace("COIN_A:", "XCOIN_AY:")),
    );
    let placed = sell_event(sell_buy + 1_000, &with_coin(SELL_LINE));
    match match_sell(&sell_coin, &[placed_glued, placed], 30) {
        Match::CoinTime(sell) => assert_eq!(sell.t_ms, sell_buy + 1_000),
        other => panic!("expected one sell coin hit, got {other:?}"),
    }
    let first = sell_event(sell_buy + 1_000, &with_coin(SELL_LINE));
    let second = sell_event(sell_buy + 3_000, &with_coin(SELL_LINE));
    assert!(matches!(
        match_sell(&sell_coin, &[first, second], 30),
        Match::CoinTime(sell) if sell.t_ms == sell_buy + 1_000
    ));
    for task in [Some(345), None] {
        let key = deal_key(task, sell_buy, sell_buy + 30_000);
        let events = [
            sell_event(sell_buy - 1_000, SELL_LINE),
            sell_event(
                sell_buy - 2_000,
                &format!("{SELL_LINE} (SellAdjust: new SellPrice: 2 (+2%))"),
            ),
        ];
        assert!(
            matches!(match_sell(&key, &events, 10), Match::Task(sell) | Match::CoinTime(sell) if sell.t_ms == sell_buy - 1_000)
        );
    }
}

/// Different tasks in the same coin window must remain ambiguous.
#[test]
fn sell_coin_route_competing_tasks() {
    let key = deal_key(None, 100_000, 130_000);
    let events = [
        sell_event(101_000, SELL_LINE),
        sell_event(102_000, &SELL_LINE.replace("(345)", "(346)")),
    ];
    assert!(matches!(match_sell(&key, &events, 10), Match::Ambiguous(2)));
}

/// A reused task with conflicting strategy names is not a safe first-placement match.
#[test]
fn sell_task_route_competing_strategies() {
    let key = deal_key(Some(345), 100_000, 130_000);
    let events = [
        sell_event(101_000, SELL_LINE),
        sell_event(102_000, &SELL_LINE.replace("<STRAT_A>", "<STRAT_B>")),
    ];
    assert!(matches!(match_sell(&key, &events, 10), Match::Ambiguous(2)));
}

/// A side tag and extra header spacing are not part of the coin token.
#[test]
fn coin_of_reads_header_token() {
    assert_eq!(coin_of(STOP_LINE), Some("COIN_A"));
    assert_eq!(
        coin_of("10:00:01  COIN_A<Short> : [7] (345) text"),
        Some("COIN_A")
    );
    assert_eq!(
        coin_of("10:00:01  COIN_A<Long> : [7] (345) text"),
        Some("COIN_A")
    );
    assert_eq!(coin_of("no clock COIN_A: [7]"), None);
    assert_eq!(coin_of("10:00:01 no counter COIN_A"), None);
}

/// One core file yields its stop and its sell, in event order. The terminal log is not a core.
///
/// Sorting by file order, or opening `*_app.log`, reports the wrong fire or an extra label.
/// A reader that rejects a non-UTF-8 byte stops before the stop line that follows it.
#[test]
fn read_core_logs_skips_app_and_sorts_events() {
    let tmp = TempDir::new();
    let core_path = tmp.0.join("2026-10-04_CORE_A.log");
    let mut core = File::create(&core_path).expect("core log");
    writeln!(core, "10:00:02.000\tINFO\tCORE_A\t{SELL_LINE}").expect("sell line");
    core.write_all(b"10:00:00.000\tINFO\tCORE_A\tnoise \xff\n")
        .expect("non-utf8 line");
    writeln!(core, "10:00:01.000\tINFO\tCORE_A\t{STOP_LINE}").expect("stop line");
    drop(core);
    fs::write(
        tmp.0.join("2026-10-04_app.log"),
        format!("10:00:01.000\tINFO\tapp\t{STOP_LINE}\n"),
    )
    .expect("app log");
    fs::write(tmp.0.join("notes.txt"), STOP_LINE).expect("notes");

    let (events, files) = read_core_logs(&tmp.0, DAY_MS, DAY_MS).expect("read logs");
    assert_eq!(files, 1);
    assert_eq!(
        events.keys().map(String::as_str).collect::<Vec<_>>(),
        ["CORE_A"]
    );
    let label = events.get("CORE_A").expect("core label");
    assert_eq!(label.len(), 2);
    match &label[0] {
        CoreEvent::Stop(fire) => {
            assert_eq!(fire.core_label, "CORE_A");
            assert_eq!(fire.task, Some(345));
            assert_eq!(fire.t_ms, DAY_MS + 36_001_000);
        }
        CoreEvent::Sell(_) | CoreEvent::Level(_) => {
            panic!("stop was written second and must sort first")
        }
    }
    match &label[1] {
        CoreEvent::Sell(sell) => {
            assert_eq!(sell.core_label, "CORE_A");
            assert_eq!(sell.t_ms, DAY_MS + 36_002_000);
        }
        CoreEvent::Stop(_) | CoreEvent::Level(_) => {
            panic!("sell must follow the stop after sorting")
        }
    }
    assert!(
        read_core_logs(&tmp.0, DAY_MS + 86_400_000, DAY_MS + 86_400_000)
            .expect("next day")
            .0
            .is_empty()
    );
}

/// Deal key whose file label comes from a core name that contains a space.
fn deal_key(task: Option<i64>, buy_ms: i64, close_ms: i64) -> DealKey<'static> {
    DealKey {
        strategy: Some("STRAT_A"),
        coin: "COIN_A",
        task,
        buy_ms,
        close_ms,
    }
}

/// Append the fixture coin as its own word.
fn with_coin(msg: &str) -> String {
    format!("{msg} COIN_A")
}

/// A parsed stop, or a panic naming the fixture that failed.
fn stop_event(t_ms: i64, msg: &str) -> CoreEvent {
    CoreEvent::Stop(parse_stop_fire(t_ms, "CORE_A", msg).expect("stop fixture"))
}

/// A parsed sell, or a panic naming the fixture that failed.
fn sell_event(t_ms: i64, msg: &str) -> CoreEvent {
    CoreEvent::Sell(parse_sell_placed(t_ms, "CORE_A", msg).expect("sell fixture"))
}

/// A directory under the OS temp dir, removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    /// Create a private directory for one read of synthetic logs.
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("core-log-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    /// A failed assertion must not leave the synthetic log directory behind.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
