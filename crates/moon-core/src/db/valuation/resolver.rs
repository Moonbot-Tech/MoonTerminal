//! Deterministic historical conversion across direct, inverse, and two-leg spot routes.

use std::collections::BTreeMap;

use super::provider::{validated_market_rate, FetchFailure, SpotCandle, SpotRateSource};
use super::{identity_rate, RateOrientation, RatePriceBasis, ResolvedRate};
use crate::db::QuoteCurrency;

#[cfg(test)]
mod tests;

/// One provider market that converts base units into quote units.
#[derive(Clone, Debug)]
struct LegRoute {
    provider: &'static str,
    symbol: String,
    orientation: RateOrientation,
    provider_rank: u8,
    orientation_rank: u8,
}

/// One oriented route observation used while comparing path candidates.
#[derive(Clone, Debug)]
struct LegObservation {
    route: LegRoute,
    candle: SpotCandle,
    rate: f64,
}

/// Resolve a historical quote into USDT without turning temporary data absence into a terminal miss.
///
/// Args:
///     source: Public closed-candle boundary.
///     quote: Known persisted quote currency.
///     requested_minute_utc: Trade minute used as the immutable cache key.
///     search_start_minute_utc: First minute not already proven empty for this rate key.
///     latest_closed_minute_utc: Current search horizon.
///     canonical_exact_prefetched: Whether Binance/Bybit one-leg exact misses are already proven.
///
/// Returns:
///     Exact-close or earliest later-open conversion, or a retryable provider outcome.
pub(crate) fn resolve_historical_rate(
    source: &dyn SpotRateSource,
    quote: QuoteCurrency,
    requested_minute_utc: i64,
    search_start_minute_utc: i64,
    latest_closed_minute_utc: i64,
    canonical_exact_prefetched: bool,
) -> Result<ResolvedRate, FetchFailure> {
    if quote == QuoteCurrency::usdt() {
        return Ok(identity_rate(
            i64::from(quote.ordinal()),
            requested_minute_utc,
        ));
    }
    if search_start_minute_utc <= requested_minute_utc {
        if let Some(rate) = resolve_paths(
            source,
            quote,
            requested_minute_utc,
            requested_minute_utc,
            requested_minute_utc,
            RatePriceBasis::ExactClose,
            canonical_exact_prefetched,
        )? {
            return Ok(rate);
        }
    }
    let successor_start = search_start_minute_utc.max(requested_minute_utc.saturating_add(60));
    if successor_start > latest_closed_minute_utc {
        return Err(FetchFailure::Missing);
    }
    resolve_paths(
        source,
        quote,
        requested_minute_utc,
        successor_start,
        latest_closed_minute_utc,
        RatePriceBasis::SuccessorOpen,
        false,
    )?
    .ok_or(FetchFailure::Missing)
}

/// Search one- and two-leg paths and choose the deterministic earliest result.
///
/// Args:
///     source: Public closed-candle boundary.
///     quote: Known source quote currency.
///     requested_minute_utc: Immutable trade-minute cache key.
///     start_minute_utc: First minute eligible in this search phase.
///     end_minute_utc: Last fully closed minute eligible in this search phase.
///     basis: Exact-close or successor-open price selection.
///     canonical_exact_prefetched: Whether canonical one-leg exact misses are already proven.
///
/// Returns:
///     Best deterministic path, no path in the range, or a transient provider failure.
fn resolve_paths(
    source: &dyn SpotRateSource,
    quote: QuoteCurrency,
    requested_minute_utc: i64,
    start_minute_utc: i64,
    end_minute_utc: i64,
    basis: RatePriceBasis,
    canonical_exact_prefetched: bool,
) -> Result<Option<ResolvedRate>, FetchFailure> {
    let quote_ticker = quote.ticker();
    let mut best: Option<(PathKey, ResolvedRate)> = None;
    let mut lookups = BTreeMap::new();
    for route in leg_routes(quote_ticker, "USDT") {
        if basis == RatePriceBasis::ExactClose
            && canonical_exact_prefetched
            && route.provider != "hyperliquid_spot"
        {
            continue;
        }
        if let Some(observation) = observe(
            source,
            &route,
            start_minute_utc,
            end_minute_utc,
            basis,
            &mut lookups,
        )? {
            let minute = observation.candle.open_ms.div_euclid(60_000) * 60;
            let key = PathKey::one(minute, &observation.route);
            consider(
                &mut best,
                key,
                one_leg_rate(quote, requested_minute_utc, basis, observation),
            );
        }
    }
    if best
        .as_ref()
        .is_some_and(|(key, _)| key.resolved_minute == start_minute_utc)
    {
        return Ok(best.map(|(_, rate)| rate));
    }

    for intermediate in QuoteCurrency::all()
        .filter(|currency| *currency != quote && *currency != QuoteCurrency::usdt())
    {
        let second_routes = leg_routes(intermediate.ticker(), "USDT");
        for first in leg_routes(quote_ticker, intermediate.ticker()) {
            for second in &second_routes {
                if let Some((first_observation, second_observation)) = common_observations(
                    source,
                    &first,
                    second,
                    start_minute_utc,
                    end_minute_utc,
                    basis,
                    &mut lookups,
                )? {
                    let minute = first_observation.candle.open_ms.div_euclid(60_000) * 60;
                    let key = PathKey::two(
                        minute,
                        intermediate.ordinal(),
                        &first_observation.route,
                        &second_observation.route,
                    );
                    consider(
                        &mut best,
                        key,
                        two_leg_rate(
                            quote,
                            requested_minute_utc,
                            basis,
                            first_observation,
                            second_observation,
                        )?,
                    );
                }
            }
        }
        if best.as_ref().is_some_and(|(key, _)| {
            key.resolved_minute == start_minute_utc
                && key.leg_count == 2
                && key.intermediate == intermediate.ordinal()
        }) {
            break;
        }
    }
    Ok(best.map(|(_, rate)| rate))
}

/// Retrieve one route observation using the price basis required by the path phase.
///
/// Args:
///     source: Public closed-candle boundary.
///     route: Provider market and orientation to observe.
///     start_minute_utc: First eligible candle-open minute.
///     end_minute_utc: Last eligible candle-open minute.
///     basis: Exact-close or successor-open price selection.
///     lookups: Resolution-local cache of completed provider lookups.
///
/// Returns:
///     Oriented observation, route/range absence, or a transient provider failure.
fn observe(
    source: &dyn SpotRateSource,
    route: &LegRoute,
    start_minute_utc: i64,
    end_minute_utc: i64,
    basis: RatePriceBasis,
    lookups: &mut BTreeMap<LookupKey, Option<LegObservation>>,
) -> Result<Option<LegObservation>, FetchFailure> {
    let key = LookupKey {
        provider: route.provider,
        symbol: route.symbol.clone(),
        start_minute_utc,
        end_minute_utc,
        successor: basis == RatePriceBasis::SuccessorOpen,
    };
    if let Some(cached) = lookups.get(&key) {
        return Ok(cached.clone());
    }
    let candle = match basis {
        RatePriceBasis::ExactClose => source
            .candles(
                route.provider,
                &route.symbol,
                start_minute_utc,
                start_minute_utc,
            )
            .and_then(|candles| {
                candles
                    .into_iter()
                    .find(|candle| candle.open_ms.div_euclid(1_000) == start_minute_utc)
                    .ok_or(FetchFailure::Missing)
            }),
        RatePriceBasis::SuccessorOpen => source.next_closed_candle(
            route.provider,
            &route.symbol,
            start_minute_utc,
            end_minute_utc,
        ),
    };
    let candle = match candle {
        Ok(candle) => candle,
        Err(FetchFailure::Missing) => {
            lookups.insert(key, None);
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let raw = match basis {
        RatePriceBasis::ExactClose => candle.close,
        RatePriceBasis::SuccessorOpen => candle.open,
    };
    let rate = validated_market_rate(raw, route.orientation).map_err(FetchFailure::Transient)?;
    let result = Some(LegObservation {
        route: route.clone(),
        candle,
        rate,
    });
    lookups.insert(key, result.clone());
    Ok(result)
}

/// Find the first minute at which both legs have a closed observation.
///
/// Args:
///     source: Public closed-candle boundary.
///     first: First conversion leg.
///     second: Second conversion leg.
///     start_minute_utc: First eligible common minute.
///     end_minute_utc: Last eligible common minute.
///     basis: Exact-close or successor-open price selection.
///     lookups: Resolution-local cache shared by every path comparison.
///
/// Returns:
///     Two observations at one minute, no common minute, or a transient provider failure.
fn common_observations(
    source: &dyn SpotRateSource,
    first: &LegRoute,
    second: &LegRoute,
    start_minute_utc: i64,
    end_minute_utc: i64,
    basis: RatePriceBasis,
    lookups: &mut BTreeMap<LookupKey, Option<LegObservation>>,
) -> Result<Option<(LegObservation, LegObservation)>, FetchFailure> {
    if basis == RatePriceBasis::ExactClose {
        let Some(first) = observe(
            source,
            first,
            start_minute_utc,
            end_minute_utc,
            basis,
            lookups,
        )?
        else {
            return Ok(None);
        };
        let Some(second) = observe(
            source,
            second,
            start_minute_utc,
            end_minute_utc,
            basis,
            lookups,
        )?
        else {
            return Ok(None);
        };
        return Ok(Some((first, second)));
    }
    let mut cursor = start_minute_utc;
    let Some(first_observation) = observe(source, first, cursor, end_minute_utc, basis, lookups)?
    else {
        return Ok(None);
    };
    let Some(second_observation) = observe(source, second, cursor, end_minute_utc, basis, lookups)?
    else {
        return Ok(None);
    };
    let mut left = Some(first_observation);
    let mut right = Some(second_observation);
    loop {
        let (Some(left_value), Some(right_value)) = (&left, &right) else {
            return Ok(None);
        };
        let left_minute = left_value.candle.open_ms.div_euclid(60_000) * 60;
        let right_minute = right_value.candle.open_ms.div_euclid(60_000) * 60;
        if left_minute == right_minute {
            return Ok(left.zip(right));
        }
        cursor = left_minute.max(right_minute);
        if cursor > end_minute_utc {
            return Ok(None);
        }
        if left_minute < right_minute {
            left = observe(source, first, cursor, end_minute_utc, basis, lookups)?;
        } else {
            right = observe(source, second, cursor, end_minute_utc, basis, lookups)?;
        }
    }
}

/// Cache identity for one provider lookup inside a single historical resolution.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct LookupKey {
    provider: &'static str,
    symbol: String,
    start_minute_utc: i64,
    end_minute_utc: i64,
    successor: bool,
}

/// Build every provider and orientation route for one neutral conversion leg.
///
/// Args:
///     base: Neutral base ticker.
///     quote: Neutral quote ticker.
///
/// Returns:
///     Direct and inverse Binance, Bybit, and Hyperliquid routes in stable rank order.
fn leg_routes(base: &str, quote: &str) -> Vec<LegRoute> {
    let mut routes = Vec::with_capacity(6);
    for (provider_rank, provider) in ["binance_spot", "bybit_spot", "hyperliquid_spot"]
        .into_iter()
        .enumerate()
    {
        let direct_symbol = if provider == "hyperliquid_spot" {
            format!("{base}/{quote}")
        } else {
            format!("{base}{quote}")
        };
        let inverse_symbol = if provider == "hyperliquid_spot" {
            format!("{quote}/{base}")
        } else {
            format!("{quote}{base}")
        };
        routes.push(LegRoute {
            provider,
            symbol: direct_symbol,
            orientation: RateOrientation::Direct,
            provider_rank: provider_rank as u8,
            orientation_rank: 0,
        });
        routes.push(LegRoute {
            provider,
            symbol: inverse_symbol,
            orientation: RateOrientation::Inverse,
            provider_rank: provider_rank as u8,
            orientation_rank: 1,
        });
    }
    routes
}

/// Convert one observation into a persisted single-leg result.
///
/// Args:
///     quote: Original trade quote currency.
///     requested_minute_utc: Immutable trade-minute cache key.
///     basis: Price basis used for the observation.
///     observation: Validated oriented market observation.
///
/// Returns:
///     Provenance-rich single-leg conversion.
fn one_leg_rate(
    quote: QuoteCurrency,
    requested_minute_utc: i64,
    basis: RatePriceBasis,
    observation: LegObservation,
) -> ResolvedRate {
    let resolved_minute_utc = observation.candle.open_ms.div_euclid(60_000) * 60;
    ResolvedRate {
        quote_ordinal: i64::from(quote.ordinal()),
        minute_utc: requested_minute_utc,
        resolved_minute_utc,
        rate_usdt: observation.rate,
        provider: observation.route.provider.to_string(),
        symbol: observation.route.symbol,
        orientation: observation.route.orientation,
        price_basis: basis,
        candle_open_ms: observation.candle.open_ms,
        candle_close_ms: observation.candle.close_ms,
        leg2_provider: None,
        leg2_symbol: None,
        leg2_orientation: None,
        leg1_rate: observation.rate,
        leg2_rate: None,
    }
}

/// Convert two common-minute observations into one persisted result.
///
/// Args:
///     quote: Original trade quote currency.
///     requested_minute_utc: Immutable trade-minute cache key.
///     basis: Price basis shared by both observations.
///     first: First validated conversion leg.
///     second: Second validated conversion leg at the same minute.
///
/// Returns:
///     Provenance-rich two-leg conversion or a transient invalid-product failure.
fn two_leg_rate(
    quote: QuoteCurrency,
    requested_minute_utc: i64,
    basis: RatePriceBasis,
    first: LegObservation,
    second: LegObservation,
) -> Result<ResolvedRate, FetchFailure> {
    let rate_usdt = first.rate * second.rate;
    if !rate_usdt.is_finite() || rate_usdt <= 0.0 {
        return Err(FetchFailure::Transient(format!(
            "invalid two-leg rate {rate_usdt}"
        )));
    }
    let resolved_minute_utc = first.candle.open_ms.div_euclid(60_000) * 60;
    Ok(ResolvedRate {
        quote_ordinal: i64::from(quote.ordinal()),
        minute_utc: requested_minute_utc,
        resolved_minute_utc,
        rate_usdt,
        provider: first.route.provider.to_string(),
        symbol: first.route.symbol,
        orientation: first.route.orientation,
        price_basis: basis,
        candle_open_ms: first.candle.open_ms,
        candle_close_ms: first.candle.close_ms.max(second.candle.close_ms),
        leg2_provider: Some(second.route.provider.to_string()),
        leg2_symbol: Some(second.route.symbol),
        leg2_orientation: Some(second.route.orientation),
        leg1_rate: first.rate,
        leg2_rate: Some(second.rate),
    })
}

/// Stable path key used to choose a result independently of response ordering.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PathKey {
    resolved_minute: i64,
    leg_count: u8,
    intermediate: u8,
    leg1_provider: u8,
    leg1_orientation: u8,
    leg2_provider: u8,
    leg2_orientation: u8,
    leg1_symbol: String,
    leg2_symbol: String,
}

impl PathKey {
    /// Build a single-leg ordering key.
    ///
    /// Args:
    ///     resolved_minute: Actual observed UTC minute.
    ///     route: Selected provider market.
    ///
    /// Returns:
    ///     Complete deterministic key for a one-leg candidate.
    fn one(resolved_minute: i64, route: &LegRoute) -> Self {
        Self {
            resolved_minute,
            leg_count: 1,
            intermediate: 0,
            leg1_provider: route.provider_rank,
            leg1_orientation: route.orientation_rank,
            leg2_provider: 0,
            leg2_orientation: 0,
            leg1_symbol: route.symbol.clone(),
            leg2_symbol: String::new(),
        }
    }

    /// Build a two-leg ordering key.
    ///
    /// Args:
    ///     resolved_minute: Common actual UTC minute.
    ///     intermediate: Stable ordinal of the intermediate currency.
    ///     first: First provider market.
    ///     second: Second provider market.
    ///
    /// Returns:
    ///     Complete deterministic key for a two-leg candidate.
    fn two(resolved_minute: i64, intermediate: u8, first: &LegRoute, second: &LegRoute) -> Self {
        Self {
            resolved_minute,
            leg_count: 2,
            intermediate,
            leg1_provider: first.provider_rank,
            leg1_orientation: first.orientation_rank,
            leg2_provider: second.provider_rank,
            leg2_orientation: second.orientation_rank,
            leg1_symbol: first.symbol.clone(),
            leg2_symbol: second.symbol.clone(),
        }
    }
}

/// Retain one candidate only when its complete deterministic key is superior.
///
/// Args:
///     best: Current best key and conversion.
///     key: New candidate key.
///     rate: New candidate conversion.
fn consider(best: &mut Option<(PathKey, ResolvedRate)>, key: PathKey, rate: ResolvedRate) {
    if best.as_ref().is_none_or(|(current, _)| key < *current) {
        *best = Some((key, rate));
    }
}
