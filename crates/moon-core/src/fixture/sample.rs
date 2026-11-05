//! The market catalog of a capture run.
//!
//! A real catalog comes off a MoonProto client's snapshot, and a synthetic core has no client. So
//! everything that asks "which markets are there" gets nothing on it: the coin search lists no coin,
//! a report row resolves to no market and its trade window cannot open, the Screener has no row. A
//! capture run photographs those screens for a reference, and an empty one documents nothing.
//!
//! This answers only when a chart bench is installed AND the run asked for a core's full state
//! (`MOON_SYNTH_CORE_STATE`), and only where the real source has found no client. A live core is
//! never reached by it, and neither is a bench measuring what it always has.
//!
//! The figures are invented, like everything a synthetic core reports.

use crate::market::ScreenerRow;
use crate::session::CoreId;

/// A market of the sample catalog, beside the one the bench charts.
struct Sample {
    coin: &'static str,
    /// Price, or `None` for the charted market, whose price the bench knows.
    price: Option<f64>,
    vol_24h: f64,
    /// Change over 24 hours, 1 hour and 1 minute, in percent.
    deltas: (f64, f64, f64),
    funding_pct: f64,
    max_leverage: i32,
}

/// The markets, in the order a screener with no sort shows them.
const MARKETS: [Sample; 6] = [
    Sample {
        coin: "ACE",
        price: None,
        vol_24h: 18_400_000.0,
        deltas: (4.2, 0.8, 0.1),
        funding_pct: 0.01,
        max_leverage: 25,
    },
    Sample {
        coin: "AKE",
        price: Some(0.000_31),
        vol_24h: 6_200_000.0,
        deltas: (-3.1, -0.4, 0.0),
        funding_pct: 0.005,
        max_leverage: 20,
    },
    Sample {
        coin: "BANK",
        price: Some(0.052),
        vol_24h: 9_700_000.0,
        deltas: (12.6, 2.3, 0.4),
        funding_pct: -0.012,
        max_leverage: 25,
    },
    Sample {
        coin: "ESPORTS",
        price: Some(0.118),
        vol_24h: 4_100_000.0,
        deltas: (-1.4, 0.2, -0.1),
        funding_pct: 0.01,
        max_leverage: 20,
    },
    Sample {
        coin: "LAB",
        price: Some(0.84),
        vol_24h: 2_900_000.0,
        deltas: (7.9, -1.1, 0.2),
        funding_pct: 0.018,
        max_leverage: 12,
    },
    Sample {
        coin: "XAN",
        price: Some(0.026),
        vol_24h: 1_300_000.0,
        deltas: (-6.5, 0.6, 0.0),
        funding_pct: -0.004,
        max_leverage: 10,
    },
];

/// Quote every sample market is in, which is the one a synthetic core reports.
const QUOTE: &str = "USDT";

/// Whether this process answers from the sample catalog.
fn active() -> bool {
    super::active().is_some() && std::env::var_os("MOON_SYNTH_CORE_STATE").is_some()
}

/// The markets matching a coin-search query, by prefix first and then anywhere in the name.
///
/// Args:
///     query: What was typed.
///     limit: Most names to return.
///
/// Returns:
///     Market names; nothing outside a capture run or for a blank query.
pub fn search(query: &str, limit: usize) -> Vec<String> {
    let query = query.trim().to_ascii_uppercase();
    if !active() || query.is_empty() {
        return Vec::new();
    }
    let names = || {
        MARKETS
            .iter()
            .map(|sample| format!("{}{QUOTE}", sample.coin))
    };
    let prefix = names().filter(|name| name.starts_with(&query));
    let inside = names().filter(|name| !name.starts_with(&query) && name.contains(&query));
    prefix.chain(inside).take(limit).collect()
}

/// The market a report row's coin trades on.
///
/// Args:
///     coin: The coin, or the full market name.
///
/// Returns:
///     The market when the catalog holds it; `None` outside a capture run.
pub fn resolve(coin: &str) -> Option<String> {
    if !active() {
        return None;
    }
    let coin = coin.trim().to_ascii_uppercase();
    let coin = coin.strip_suffix(QUOTE).unwrap_or(&coin);
    MARKETS
        .iter()
        .find(|sample| sample.coin == coin)
        .map(|sample| format!("{}{QUOTE}", sample.coin))
}

/// The Screener's market half: one row per sample market.
///
/// Args:
///     provider: The market-data provider the rows are read from.
///
/// Returns:
///     The rows; nothing outside a capture run.
pub fn screener_rows(provider: CoreId) -> Vec<ScreenerRow> {
    if !active() {
        return Vec::new();
    }
    let charted = super::active().map_or(1.0, |bench| f64::from(bench.last_price()));
    MARKETS
        .iter()
        .map(|sample| {
            let ask = sample.price.unwrap_or(charted);
            let (d_24h, d_1h, d_1m) = sample.deltas;
            ScreenerRow {
                provider,
                core: provider,
                market: format!("{}{QUOTE}", sample.coin),
                coin: sample.coin.to_string(),
                vol_24h: sample.vol_24h,
                vol_1h: sample.vol_24h / 18.0,
                vol_1m: sample.vol_24h / 1_100.0,
                vol_3m: sample.vol_24h / 400.0,
                vol_5m: sample.vol_24h / 240.0,
                ask,
                high_1h: ask * 1.012,
                max_order: 250_000.0,
                d_24h,
                d_3h: d_24h / 2.0,
                d_1h,
                d_15m: d_1h / 2.0,
                d_1m,
                d_72h: d_24h * 1.5,
                funding_pct: sample.funding_pct,
                mark_delta_pct: Some(0.02),
                price_step: ask / 5_000.0,
                max_leverage: sample.max_leverage,
                leverage_x: 10,
                isolated: Some(false),
                core_pnl: 0.0,
                session: None,
                pos_size: 0.0,
                orders: 0,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
