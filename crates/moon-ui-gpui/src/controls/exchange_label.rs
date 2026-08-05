//! Shared presentation policy for exchange names reported by MoonBot cores.

use rust_i18n::t;

/// Add the localized spot suffix when a reported exchange name has no market-type suffix.
///
/// Args:
///     exchange: Exchange name reported by a core.
///     spot: Localized spot-market suffix.
///
/// Returns:
///     Display-only exchange name with exactly one market-type suffix.
pub(crate) fn exchange_display_name_with_spot(exchange: &str, spot: &str) -> String {
    let exchange = exchange.trim();
    if exchange.is_empty() {
        return String::new();
    }
    let market_type = exchange.split_whitespace().next_back().unwrap_or_default();
    if market_type.eq_ignore_ascii_case("spot") || market_type.eq_ignore_ascii_case("futures") {
        exchange.to_string()
    } else {
        format!("{exchange} {spot}")
    }
}

/// Format one reported exchange name through the application's localized display policy.
///
/// Args:
///     exchange: Exchange name reported by a core.
///
/// Returns:
///     Display-only exchange name with an explicit Spot or Futures market type.
pub(crate) fn exchange_display_name(exchange: &str) -> String {
    exchange_display_name_with_spot(exchange, t!("common.exchange_spot").as_ref())
}

#[cfg(test)]
mod tests;
