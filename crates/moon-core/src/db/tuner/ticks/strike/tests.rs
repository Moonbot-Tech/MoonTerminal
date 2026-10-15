use super::*;

/// A synthetic comment in the shape the core writes.
const LONG: &str = " MoonStrike USDT-XALPHA <(strategy <UNIT_01>)> [Long]  DetectTime: 12:00:00  TradeTime: 12:00:01  Latency: 4  LastBID: 0.0100  TradePrice: 0.0100  Depth: 4.00  Vol: 1000.00$ ";
const SHORT: &str = " MoonStrike USDT-XALPHA <(strategy <UNIT_01>)> [Short]  DetectTime: 12:00:00  TradeTime: 12:00:01  Latency: 4  LastBID: 0.0500  TradePrice: 0.0500  Depth: 2.00  Vol: 1000.00$ ";

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * b.abs().max(1.0)
}

#[test]
fn parses_long_comment() {
    let d = parse_strike_detect(LONG).expect("strike detect");
    assert_eq!(d.depth_pct, 4.0);
    assert_eq!(d.trade_price, 0.01);
}

#[test]
fn parses_short_comment() {
    let d = parse_strike_detect(SHORT).expect("strike detect");
    assert_eq!(d.depth_pct, 2.0);
    assert_eq!(d.trade_price, 0.05);
}

/// The strike writes its depth bare — the hook's per-cent guard must not apply here.
#[test]
fn depth_without_percent_is_read() {
    let d = parse_strike_detect("TradePrice: 1.5 Depth: 3.25 Vol: 1$").expect("detect");
    assert_eq!(d.depth_pct, 3.25);
}

#[test]
fn missing_depth_is_none() {
    assert_eq!(
        parse_strike_detect("MoonStrike TradePrice: 0.0100 Vol: 1$"),
        None
    );
    assert_eq!(parse_strike_detect("TradePrice: 0.0100 Depth: 0"), None);
    assert_eq!(parse_strike_detect(""), None);
}

#[test]
fn missing_trade_price_is_none() {
    assert_eq!(
        parse_strike_detect("MoonStrike LastBID: 0.01 Depth: 4.00"),
        None
    );
}

#[test]
fn long_take_level_from_trade_price() {
    let d = StrikeDetect {
        depth_pct: 4.0,
        trade_price: 100.0,
    };
    // top 104, low 100 (fill above it), half way: 102.
    assert!(close(strike_take_level(d, 100.5, 50.0, true), 102.0));
}

#[test]
fn short_take_level_from_trade_price() {
    let d = StrikeDetect {
        depth_pct: 4.0,
        trade_price: 100.0,
    };
    // top 96, low 100 (fill below it), a quarter of the way: 99.
    assert!(close(strike_take_level(d, 99.5, 25.0, false), 99.0));
}

#[test]
fn long_fill_below_trade_price_uses_fill() {
    let d = StrikeDetect {
        depth_pct: 4.0,
        trade_price: 100.0,
    };
    // low = fill 98, top 104: half way is 101.
    assert!(close(strike_take_level(d, 98.0, 50.0, true), 101.0));
}

#[test]
fn short_fill_above_trade_price_uses_fill() {
    let d = StrikeDetect {
        depth_pct: 4.0,
        trade_price: 100.0,
    };
    // low = fill 102, top 96: half way is 99.
    assert!(close(strike_take_level(d, 102.0, 50.0, false), 99.0));
}

#[test]
fn depth_label_is_not_matched_inside_another_label() {
    let d = parse_strike_detect("MaxDepth: 9.00 TradePrice: 1.0 Depth: 4.00").expect("detect");
    assert_eq!(d.depth_pct, 4.0);
    assert_eq!(parse_strike_detect("MaxDepth: 9.00 TradePrice: 1.0"), None);
}
