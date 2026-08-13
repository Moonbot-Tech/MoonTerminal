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

/// Route-aware source with a newer fallback candle than its preferred Binance candle.
struct RoutePrioritySource {
    /// Every provider and symbol requested, in canonical order.
    calls: Mutex<Vec<(String, String)>>,
    /// Closed minute available on the preferred Binance direct route.
    preferred_minute: i64,
    /// Newer closed minute available only on the fallback Bybit direct route.
    fallback_minute: i64,
}

impl RoutePrioritySource {
    /// Build a route-aware source for the canonical-priority regression.
    ///
    /// Args:
    ///     preferred_minute: Closed minute returned by Binance direct.
    ///     fallback_minute: Newer closed minute returned by Bybit direct.
    ///
    /// Returns:
    ///     Source with an empty request log and permanent misses on inverse routes.
    fn new(preferred_minute: i64, fallback_minute: i64) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            preferred_minute,
            fallback_minute,
        }
    }
}

impl SpotRateSource for RoutePrioritySource {
    /// Return route-specific candles so provider ordering cannot hide behind response ordering.
    ///
    /// Args:
    ///     provider: Canonical provider identifier.
    ///     symbol: Provider-native spot symbol.
    ///     _start_minute_utc: First requested UTC minute.
    ///     _end_minute_utc: Last requested UTC minute.
    ///
    /// Returns:
    ///     Binance and Bybit direct candles, or permanent absence for every inverse route.
    fn candles(
        &self,
        provider: &'static str,
        symbol: &str,
        _start_minute_utc: i64,
        _end_minute_utc: i64,
    ) -> Result<Vec<SpotCandle>, FetchFailure> {
        self.calls
            .lock()
            .expect("lock priority calls")
            .push((provider.to_string(), symbol.to_string()));
        let (minute, close) = match (provider, symbol) {
            ("binance_spot", "USDCUSDT") => (self.preferred_minute, 1.01),
            ("bybit_spot", "USDCUSDT") => (self.fallback_minute, 1.02),
            _ => return Err(FetchFailure::Missing),
        };
        Ok(vec![SpotCandle {
            open_ms: minute * 1_000,
            close_ms: minute * 1_000 + 59_999,
            close,
        }])
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
    assert_eq!(
        result,
        Err(FetchFailure::Transient(
            "binance_spot BTCUSDT: timeout".to_string()
        ))
    );
    assert_eq!(source.calls.into_inner().expect("take fake calls").len(), 1);
}

/// Breakage: replacing `resolve_latest_rate`'s `max_by_key(open_ms)` with response-order selection
/// would choose an older Bybit-style reverse-ordered candle, keeping Analytics on a stale rate even
/// though a newer closed price was returned by the preferred direct route.
#[test]
fn current_resolution_uses_the_newest_candle_on_the_first_available_route() {
    let newest = 1_700_000_040;
    let older = newest - 120;
    let source = FakeSource::new(vec![Ok(vec![
        SpotCandle {
            open_ms: newest * 1_000,
            close_ms: newest * 1_000 + 59_999,
            close: 1.01,
        },
        SpotCandle {
            open_ms: older * 1_000,
            close_ms: older * 1_000 + 59_999,
            close: 0.99,
        },
    ])]);

    let rate = resolve_latest_rate(&source, 8, "USDC", older, newest)
        .expect("resolve latest current rate");

    assert_eq!(rate.minute_utc, newest);
    assert_eq!(rate.rate_usdt, 1.01);
    assert_eq!(rate.provider, "binance_spot");
    assert_eq!(rate.symbol, "USDCUSDT");
    assert_eq!(source.calls.into_inner().expect("take fake calls").len(), 1);
}

/// Breakage: moving Bybit ahead of Binance in `canonical_routes` would let its newer fallback
/// candle override an available preferred route, changing the current PnL source and value.
#[test]
fn current_resolution_keeps_canonical_priority_over_cross_route_recency() {
    let preferred_minute = 1_700_000_040;
    let fallback_minute = preferred_minute + 60;
    let source = RoutePrioritySource::new(preferred_minute, fallback_minute);

    let rate = resolve_latest_rate(&source, 8, "USDC", preferred_minute - 60, fallback_minute)
        .expect("resolve preferred current route");

    assert_eq!(rate.minute_utc, preferred_minute);
    assert_eq!(rate.rate_usdt, 1.01);
    assert_eq!(rate.provider, "binance_spot");
    assert_eq!(rate.symbol, "USDCUSDT");
    assert_eq!(
        source.calls.into_inner().expect("take priority calls"),
        vec![("binance_spot".to_string(), "USDCUSDT".to_string())]
    );
}

/// Breakage: routing historical `resolve_rate` through the current lookback resolver would borrow
/// a neighboring candle, changing the USDT value of trades whose exact close minute had no price.
#[test]
fn historical_resolution_still_requires_the_exact_minute() {
    let requested = 1_700_000_040;
    let neighbor = requested - 60;
    let source = FakeSource::new(vec![
        Ok(vec![SpotCandle {
            open_ms: neighbor * 1_000,
            close_ms: neighbor * 1_000 + 59_999,
            close: 1.01,
        }]),
        Err(FetchFailure::Missing),
        Err(FetchFailure::Missing),
        Err(FetchFailure::Missing),
    ]);

    assert_eq!(
        resolve_rate(&source, 8, "USDC", requested),
        Err(FetchFailure::Missing)
    );
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

/// Breakage: returning a response-wide transient or accepting a non-one-minute row in
/// `provider::parse_binance` would either stall historical reconciliation at the same batch or
/// value a trade from the wrong candle; valid siblings must survive while the exact fallback wins.
#[test]
fn binance_duration_outlier_keeps_valid_siblings_and_exact_fallback() {
    let first = 1_700_000_040;
    let second = first + 60;
    let value = serde_json::json!([
        [
            first * 1_000,
            "10.0",
            "11.0",
            "9.0",
            "42.0",
            "123.0",
            first * 1_000 + 59_999
        ],
        [
            second * 1_000,
            "10.0",
            "11.0",
            "9.0",
            "99.0",
            "123.0",
            second * 1_000 + 60_999
        ]
    ]);
    let binance = parse_binance(&value, first, second).expect("retain exact Binance sibling");
    let source = FakeSource::new(vec![
        Ok(binance),
        Err(FetchFailure::Missing),
        Ok(vec![SpotCandle {
            open_ms: second * 1_000,
            close_ms: second * 1_000 + 59_999,
            close: 43.0,
        }]),
    ]);

    let batch = resolve_rate_batch(&source, 0, "BTC", &[first, second]);

    assert!(batch.missing.is_empty());
    assert!(batch.transient.is_none());
    assert_eq!(batch.ready.len(), 2);
    let first_rate = batch
        .ready
        .iter()
        .find(|rate| rate.minute_utc == first)
        .expect("valid Binance minute");
    assert_eq!(first_rate.provider, "binance_spot");
    assert_eq!(first_rate.symbol, "BTCUSDT");
    assert_eq!(first_rate.rate_usdt, 42.0);
    let second_rate = batch
        .ready
        .iter()
        .find(|rate| rate.minute_utc == second)
        .expect("exact fallback minute");
    assert_eq!(second_rate.provider, "bybit_spot");
    assert_eq!(second_rate.symbol, "BTCUSDT");
    assert_eq!(second_rate.rate_usdt, 43.0);
    assert_eq!(
        source.calls.into_inner().expect("take fake calls"),
        vec![
            (
                "binance_spot".to_string(),
                "BTCUSDT".to_string(),
                first,
                second,
            ),
            (
                "binance_spot".to_string(),
                "USDTBTC".to_string(),
                first,
                second,
            ),
            (
                "bybit_spot".to_string(),
                "BTCUSDT".to_string(),
                first,
                second,
            ),
        ]
    );
}

/// Breakage: skipping a Binance row before proving an integer close time would hide structural
/// response corruption as permanent absence, so reconciliation would stop retrying bad data.
#[test]
fn binance_missing_close_time_remains_transient() {
    let minute = 1_700_000_040;
    let value = serde_json::json!([[minute * 1_000, "10.0", "11.0", "9.0", "42.0", "123.0"]]);

    assert_eq!(
        parse_binance(&value, minute, minute),
        Err(FetchFailure::Transient(
            "binance candle has no integer close time".to_string()
        ))
    );
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

/// Breakage: removing the message-qualified `10001` arm would reproduce the observed endless
/// `current_rates/provider` retry, while matching every `10001` would hide real client/API bugs as
/// absent symbols. Missing `retMsg` must remain retryable for the same reason.
#[test]
fn only_bybit_invalid_or_observed_unsupported_symbols_are_permanent() {
    let invalid_symbol = serde_json::json!({"retCode": 10029, "retMsg": "symbol invalid"});
    assert_eq!(
        parse_bybit(&invalid_symbol, 1_700_000_040, 1_700_000_040),
        Err(FetchFailure::Missing)
    );
    let unsupported = serde_json::json!({
        "retCode": 10001,
        "retMsg": "  Not supported symbols \r\n"
    });
    assert_eq!(
        parse_bybit(&unsupported, 1_700_000_040, 1_700_000_040),
        Err(FetchFailure::Missing)
    );
    assert_eq!(
        parse_bybit(
            &serde_json::json!({"retCode": 10001, "retMsg": "request parameter error"}),
            1_700_000_040,
            1_700_000_040,
        ),
        Err(FetchFailure::Transient(
            "bybit retCode 10001: request parameter error".to_string()
        ))
    );
    assert_eq!(
        parse_bybit(
            &serde_json::json!({"retCode": 10001}),
            1_700_000_040,
            1_700_000_040,
        ),
        Err(FetchFailure::Transient(
            "bybit retCode 10001: missing retMsg".to_string()
        ))
    );
}

/// Breakage: returning a parser's raw transient from `resolve_rate_batch` would again reduce the
/// user-facing stall reason to `retCode 10001`, hiding which provider and symbol must be checked;
/// falling through after it could also cache a false permanent miss.
#[test]
fn transient_bybit_fault_names_the_route_code_and_message() {
    let minute = 1_700_000_040;
    let failure = parse_bybit(
        &serde_json::json!({"retCode": 10001, "retMsg": "request parameter error"}),
        minute,
        minute,
    )
    .expect_err("generic parameter failure stays transient");
    let source = FakeSource::new(vec![
        Err(FetchFailure::Missing),
        Err(FetchFailure::Missing),
        Err(failure),
    ]);

    assert_eq!(
        resolve_rate(&source, 8, "USDC", minute),
        Err(FetchFailure::Transient(
            "bybit_spot USDCUSDT: bybit retCode 10001: request parameter error".to_string()
        ))
    );
    assert_eq!(
        source.calls.into_inner().expect("take fake calls").len(),
        3,
        "a transient direct Bybit route must stop before inverse fallback"
    );
}
