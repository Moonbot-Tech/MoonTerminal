//! Historical successor-route regression tests.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;

/// Deterministic neutral market fixture keyed by provider, symbol, and minute.
struct FixtureSource {
    candles: BTreeMap<(&'static str, &'static str, i64), SpotCandle>,
    calls: Mutex<usize>,
}

impl FixtureSource {
    /// Build a source from explicitly authored market observations.
    ///
    /// Args:
    ///     candles: Provider, symbol, minute, open, and close tuples.
    ///
    /// Returns:
    ///     Deterministic source with an empty request counter.
    fn new(candles: &[(&'static str, &'static str, i64, f64, f64)]) -> Self {
        Self {
            candles: candles
                .iter()
                .map(|(provider, symbol, minute, open, close)| {
                    (
                        (*provider, *symbol, *minute),
                        SpotCandle {
                            open_ms: minute * 1_000,
                            close_ms: minute * 1_000 + 59_999,
                            open: *open,
                            close: *close,
                        },
                    )
                })
                .collect(),
            calls: Mutex::new(0),
        }
    }

    /// Return the number of provider-boundary requests made by the resolver.
    fn call_count(&self) -> usize {
        *self.calls.lock().expect("read request counter")
    }
}

impl SpotRateSource for FixtureSource {
    /// Return every fixture candle inside the requested inclusive range.
    fn candles(
        &self,
        provider: &'static str,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<Vec<SpotCandle>, FetchFailure> {
        *self.calls.lock().expect("lock request counter") += 1;
        let candles = self
            .candles
            .iter()
            .filter(|((candidate_provider, candidate_symbol, minute), _)| {
                *candidate_provider == provider
                    && *candidate_symbol == symbol
                    && (start_minute_utc..=end_minute_utc).contains(minute)
            })
            .map(|(_, candle)| *candle)
            .collect::<Vec<_>>();
        if candles.is_empty() {
            Err(FetchFailure::Missing)
        } else {
            Ok(candles)
        }
    }
}

/// Breakage: restoring a 24-hour horizon rejects a valid later observation and leaves a known
/// quote permanently outside historical totals. The successor must use its open, not its close.
#[test]
fn successor_search_has_no_twenty_four_hour_cap_and_uses_open() {
    let requested = 1_700_000_040;
    let successor = requested + 48 * 60 * 60;
    let source = FixtureSource::new(&[("binance_spot", "USDCUSDT", successor, 1.001, 9.999)]);

    let rate = resolve_historical_rate(
        &source,
        QuoteCurrency::from_report_ordinal(8).expect("USDC quote"),
        requested,
        requested,
        successor,
        false,
    )
    .expect("resolve later retained candle");

    assert_eq!(rate.minute_utc, requested);
    assert_eq!(rate.resolved_minute_utc, successor);
    assert_eq!(rate.price_basis, RatePriceBasis::SuccessorOpen);
    assert_eq!(rate.rate_usdt, 1.001);
}

/// Breakage: deriving the cache key from a resumed search start stores the result under the last
/// checked horizon instead of the trade minute, leaving the original report row pending forever.
#[test]
fn resumed_successor_search_keeps_the_original_requested_minute() {
    let requested = 1_700_000_040;
    let already_searched = requested + 60;
    let successor = already_searched + 60;
    let source = FixtureSource::new(&[
        ("binance_spot", "USDCUSDT", already_searched, 1.001, 1.002),
        ("binance_spot", "USDCUSDT", successor, 1.003, 1.004),
    ]);

    let rate = resolve_historical_rate(
        &source,
        QuoteCurrency::from_report_ordinal(8).expect("USDC quote"),
        requested,
        successor,
        successor,
        false,
    )
    .expect("resume after the proven-empty horizon");

    assert_eq!(rate.minute_utc, requested);
    assert_eq!(rate.resolved_minute_utc, successor);
    assert_eq!(rate.rate_usdt, 1.003);
}

/// Breakage: limiting routing to quote/USDT pairs cannot price the reported USDH server row even
/// though USDH/USDC and USDC/USDT share the first later closed minute.
#[test]
fn usd_hyperliquid_route_composes_with_usdc_at_one_successor_minute() {
    let requested = 1_700_000_040;
    let successor = requested + 60;
    let source = FixtureSource::new(&[
        ("hyperliquid_spot", "USDH/USDC", successor, 0.998, 7.0),
        ("binance_spot", "USDCUSDT", successor, 1.001, 8.0),
    ]);

    let rate = resolve_historical_rate(
        &source,
        QuoteCurrency::from_report_ordinal(7).expect("USDH quote"),
        requested,
        requested,
        successor,
        false,
    )
    .expect("resolve USDH through USDC");

    assert_eq!(rate.minute_utc, requested);
    assert_eq!(rate.resolved_minute_utc, successor);
    assert_eq!(rate.rate_usdt, 0.998 * 1.001);
    assert_eq!(rate.provider, "hyperliquid_spot");
    assert_eq!(rate.symbol, "USDH/USDC");
    assert_eq!(rate.leg2_provider.as_deref(), Some("binance_spot"));
    assert_eq!(rate.leg2_symbol.as_deref(), Some("USDCUSDT"));
}

/// Breakage: special-casing USDH/USDC fixes one screenshot but leaves the same sparse-pair failure
/// on every other exchange and quote. Route generation must use the complete currency universe.
#[test]
fn two_leg_generation_is_not_specific_to_usdh_or_hyperliquid() {
    let requested = 1_700_000_040;
    let successor = requested + 60;
    let source = FixtureSource::new(&[
        ("binance_spot", "EURUSDC", successor, 1.08, 7.0),
        ("bybit_spot", "USDCUSDT", successor, 1.001, 8.0),
    ]);

    let rate = resolve_historical_rate(
        &source,
        QuoteCurrency::from_report_ordinal(14).expect("EUR quote"),
        requested,
        requested,
        successor,
        false,
    )
    .expect("resolve generic two-leg path");

    assert_eq!(rate.symbol, "EURUSDC");
    assert_eq!(rate.leg2_symbol.as_deref(), Some("USDCUSDT"));
    assert_eq!(rate.rate_usdt, 1.08 * 1.001);
}

/// Breakage: advancing a lagging leg by one minute instead of jumping to the other leg's observed
/// minute issues one synchronous provider request per dense candle across a large listing gap.
#[test]
fn common_minute_merge_jumps_directly_to_the_later_leg() {
    let start = 1_700_000_040;
    let common = start + 30 * 24 * 60 * 60;
    let source = FixtureSource::new(&[
        ("hyperliquid_spot", "USDH/USDC", start, 0.998, 0.999),
        ("hyperliquid_spot", "USDH/USDC", common, 0.997, 0.998),
        ("binance_spot", "USDCUSDT", common, 1.001, 1.002),
    ]);
    let first = leg_routes("USDH", "USDC")
        .into_iter()
        .find(|route| {
            route.provider == "hyperliquid_spot" && route.orientation == RateOrientation::Direct
        })
        .expect("Hyperliquid first leg");
    let second = leg_routes("USDC", "USDT")
        .into_iter()
        .find(|route| {
            route.provider == "binance_spot" && route.orientation == RateOrientation::Direct
        })
        .expect("Binance second leg");
    let mut lookups = BTreeMap::new();

    let (left, right) = common_observations(
        &source,
        &first,
        &second,
        start,
        common,
        RatePriceBasis::SuccessorOpen,
        &mut lookups,
    )
    .expect("merge provider observations")
    .expect("find common minute");

    assert_eq!(left.candle.open_ms, common * 1_000);
    assert_eq!(right.candle.open_ms, common * 1_000);
    assert_eq!(source.call_count(), 3);
}

/// Reusing one route inside the two-leg Cartesian comparison must reuse its batch-local result;
/// otherwise duplicate trades and route pairs multiply public exchange traffic.
#[test]
fn repeated_route_lookup_hits_the_batch_local_cache() {
    let minute = 1_700_000_040;
    let source = FixtureSource::new(&[("binance_spot", "EURUSDC", minute, 1.08, 1.09)]);
    let route = leg_routes("EUR", "USDC").remove(0);
    let mut lookups = BTreeMap::new();

    let first = observe(
        &source,
        &route,
        minute,
        minute,
        RatePriceBasis::ExactClose,
        &mut lookups,
    )
    .expect("first lookup")
    .expect("first observation");
    let second = observe(
        &source,
        &route,
        minute,
        minute,
        RatePriceBasis::ExactClose,
        &mut lookups,
    )
    .expect("cached lookup")
    .expect("cached observation");

    assert_eq!(first.rate, second.rate);
    assert_eq!(source.call_count(), 1);
}

/// Breakage: routing USDT through public markets adds avoidable failure and can move an identity
/// amount away from one. The identity path must make no provider request.
#[test]
fn usdt_identity_never_touches_the_provider() {
    let requested = 1_700_000_040;
    let source = FixtureSource::new(&[]);

    let rate = resolve_historical_rate(
        &source,
        QuoteCurrency::usdt(),
        requested,
        requested,
        requested,
        false,
    )
    .expect("resolve identity");

    assert_eq!(rate.rate_usdt, 1.0);
    assert_eq!(source.call_count(), 0);
}
