//! Canonical provider routing and parser regression tests.

use std::sync::Mutex;

use super::*;

/// Deterministic route script recording every provider/symbol request.
struct FakeSource {
    calls: Mutex<Vec<(String, String, i64, i64)>>,
    answers: Mutex<Vec<Result<Vec<SpotCandle>, FetchFailure>>>,
}

impl FakeSource {
    /// Build a fake whose answers are consumed in request order.
    ///
    /// Args:
    ///     answers: Scripted route results in expected request order.
    ///
    /// Returns:
    ///     Deterministic source with an empty request log.
    fn new(answers: Vec<Result<Vec<SpotCandle>, FetchFailure>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            answers: Mutex::new(answers.into_iter().rev().collect()),
        }
    }
}

impl SpotRateSource for FakeSource {
    /// Record one route and return its scripted response.
    ///
    /// Args:
    ///     provider: Canonical provider identifier.
    ///     symbol: Provider-native spot symbol.
    ///     start_minute_utc: First requested UTC minute.
    ///     end_minute_utc: Last requested UTC minute.
    ///
    /// Returns:
    ///     Next scripted candle result.
    ///
    /// Panics:
    ///     Panics when the test performs more requests than it scripted.
    fn candles(
        &self,
        provider: &'static str,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<Vec<SpotCandle>, FetchFailure> {
        self.calls.lock().expect("lock fake calls").push((
            provider.to_string(),
            symbol.to_string(),
            start_minute_utc,
            end_minute_utc,
        ));
        self.answers
            .lock()
            .expect("lock fake answers")
            .pop()
            .expect("script one answer per route")
    }
}

/// Removing inverse routing from `provider::resolve_rate` would leave a quote that trades only as
/// `USDTQUOTE` uncovered; the second Binance route must invert its close.
#[test]
fn inverse_binance_route_is_used_after_direct_absence() {
    let minute = 1_700_000_040;
    let source = FakeSource::new(vec![
        Err(FetchFailure::Missing),
        Ok(vec![SpotCandle {
            open_ms: minute * 1_000,
            close_ms: minute * 1_000 + 59_999,
            close: 0.000_025,
        }]),
    ]);
    let rate = resolve_rate(&source, 3, "BNB", minute).expect("resolve inverse quote");
    assert_eq!(rate.provider, "binance_spot");
    assert_eq!(rate.symbol, "USDTBNB");
    assert_eq!(rate.orientation, RateOrientation::Inverse);
    assert_eq!(rate.rate_usdt, 40_000.0);
    assert_eq!(
        source.calls.into_inner().expect("take fake calls"),
        vec![
            (
                "binance_spot".to_string(),
                "BNBUSDT".to_string(),
                minute,
                minute,
            ),
            (
                "binance_spot".to_string(),
                "USDTBNB".to_string(),
                minute,
                minute,
            ),
        ]
    );
}

/// Treating a transient Binance outage as symbol absence would silently switch the benchmark and
/// possibly cache a false permanent miss; transient errors must stop canonical fallback.
#[test]
fn transient_primary_failure_does_not_fall_through() {
    let source = FakeSource::new(vec![Err(FetchFailure::Transient("timeout".to_string()))]);
    let result = resolve_rate(&source, 0, "BTC", 1_700_000_040);
    assert_eq!(result, Err(FetchFailure::Transient("timeout".to_string())));
    assert_eq!(source.calls.into_inner().expect("take fake calls").len(), 1);
}

/// Parsing the wrong Binance array slot would value every trade from volume or high price; the
/// fixture independently places the intended close at index four and close time at index six.
#[test]
fn binance_parser_reads_exact_close_and_times() {
    let minute = 1_700_000_040;
    let value = serde_json::json!([[
        minute * 1_000,
        "10.0",
        "99.0",
        "1.0",
        "42.5",
        "123.0",
        minute * 1_000 + 59_999,
        "999.0"
    ]]);
    let candle = parse_binance(&value, minute, minute)
        .expect("parse Binance candle")
        .into_iter()
        .next()
        .expect("one Binance candle");
    assert_eq!(candle.open_ms, minute * 1_000);
    assert_eq!(candle.close_ms, minute * 1_000 + 59_999);
    assert_eq!(candle.close, 42.5);
}

/// Bybit returns string fields under `result.list`; flattening it as a Binance array would mark a
/// valid fallback candle missing and prevent mixed-quote coverage from completing.
#[test]
fn bybit_parser_reads_enveloped_string_fields() {
    let minute = 1_700_000_040;
    let value = serde_json::json!({
        "retCode": 0,
        "result": {
            "list": [[
                (minute * 1_000).to_string(), "10.0", "12.0", "9.0", "11.25", "123"
            ]]
        }
    });
    let candle = parse_bybit(&value, minute, minute)
        .expect("parse Bybit candle")
        .into_iter()
        .next()
        .expect("one Bybit candle");
    assert_eq!(candle.open_ms, minute * 1_000);
    assert_eq!(candle.close_ms, minute * 1_000 + 59_999);
    assert_eq!(candle.close, 11.25);
}

/// Replacing `resolve_rate_batch` with a per-trade loop would issue one request per historical
/// order; two requested minutes in one provider window must share one canonical route call.
#[test]
fn batch_resolution_fetches_a_minute_window_once() {
    let first = 1_700_000_040;
    let second = first + 60;
    let source = FakeSource::new(vec![Ok(vec![
        SpotCandle {
            open_ms: first * 1_000,
            close_ms: first * 1_000 + 59_999,
            close: 42_000.0,
        },
        SpotCandle {
            open_ms: second * 1_000,
            close_ms: second * 1_000 + 59_999,
            close: 42_100.0,
        },
    ])]);

    let batch = resolve_rate_batch(&source, 0, "BTC", &[first, second]);

    assert_eq!(batch.ready.len(), 2);
    assert!(batch.missing.is_empty());
    assert!(batch.transient.is_none());
    assert_eq!(
        source.calls.into_inner().expect("take fake calls"),
        vec![(
            "binance_spot".to_string(),
            "BTCUSDT".to_string(),
            first,
            second,
        )]
    );
}

/// Replacing Binance's structured `-1121` check with a blanket 4xx rule would permanently cache
/// endpoint or parameter failures as missing markets; only the exchange's invalid-symbol response
/// may advance canonical fallback.
#[test]
fn only_structured_binance_invalid_symbol_is_permanent() {
    let invalid_symbol = serde_json::json!({"code": -1121, "msg": "Invalid symbol."});
    assert_eq!(
        classify_binance_status(400, &invalid_symbol),
        Err(FetchFailure::Missing)
    );
    assert!(matches!(
        classify_binance_status(404, &serde_json::json!({"code": -1})),
        Err(FetchFailure::Transient(_))
    ));
}

/// Treating Bybit's generic `10001` parameter failure as an absent market would permanently cache
/// a client/API-contract bug, while missing `10029` would stop before the inverse fallback route.
#[test]
fn only_bybit_invalid_symbol_is_permanent() {
    let invalid_symbol = serde_json::json!({"retCode": 10029, "retMsg": "symbol invalid"});
    assert_eq!(
        parse_bybit(&invalid_symbol, 1_700_000_040, 1_700_000_040),
        Err(FetchFailure::Missing)
    );
    assert!(matches!(
        parse_bybit(
            &serde_json::json!({"retCode": 10001, "retMsg": "request parameter error"}),
            1_700_000_040,
            1_700_000_040,
        ),
        Err(FetchFailure::Transient(_))
    ));
}
