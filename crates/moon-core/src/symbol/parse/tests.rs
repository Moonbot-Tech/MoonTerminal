//! Every name here is a REAL market taken from the connected cores' catalogs (6328 distinct names
//! across ten exchange connections), not an invented example. A case that never occurs on any
//! exchange would only lock in a guess.

use super::super::exchange::Exchange;
use super::{market_in_text, split_market};

/// `(market, base, quote, contract)` under the exchange's own rules AND under shape recognition:
/// a log line never knows the exchange, so both paths must land on the same coin.
#[track_caller]
fn both(ex: Exchange, market: &str, base: &str, quote: &str, contract: Option<&str>) {
    let known = split_market(market, ex);
    assert_eq!(
        (known.base, known.quote, known.contract),
        (base, quote, contract),
        "{market} under {ex:?}"
    );
    let guessed = split_market(market, Exchange::Unknown);
    assert_eq!(
        (guessed.base, guessed.quote, guessed.contract),
        (base, quote, contract),
        "{market} by shape"
    );
}

#[test]
fn okx_dashed() {
    both(Exchange::Okx, "BEAT-USDT-SWAP", "BEAT", "USDT", None);
    both(Exchange::Okx, "0G-USDT-SWAP", "0G", "USDT", None);
    both(Exchange::Okx, "A-USDT-SWAP", "A", "USDT", None);
    both(Exchange::Okx, "BTC-USDT", "BTC", "USDT", None);
    // Delivery futures keep the expiry: two expiries are two instruments.
    both(
        Exchange::Okx,
        "BTC-USDT-250808",
        "BTC",
        "USDT",
        Some("250808"),
    );
    // Inverse contracts quote in USD and settle in the coin.
    both(Exchange::Okx, "BTC-USD-SWAP", "BTC", "USD", None);
}

/// An option chain is not traded here, but it must not silently lose its tail either.
#[test]
fn okx_option_keeps_its_tail() {
    let p = split_market("BTC-USDT-250228-50000-C", Exchange::Okx);
    assert_eq!((p.base, p.quote), ("BTC", "USDT"));
    assert_eq!(p.contract, Some("250228-50000-C"));
}

#[test]
fn binance_concat_and_dated() {
    both(Exchange::Binance, "BTCUSDT", "BTC", "USDT", None);
    both(
        Exchange::Binance,
        "1000000BABYDOGEUSDT",
        "1000000BABYDOGE",
        "USDT",
        None,
    );
    both(Exchange::Binance, "0GUSDC", "0G", "USDC", None);
    both(
        Exchange::Binance,
        "BTCUSDT_260925",
        "BTC",
        "USDT",
        Some("260925"),
    );
}

/// Binance COIN-M: the perpetual is `_PERP`, the quarterly a date, and the quote is USD.
#[test]
fn binance_coin_m() {
    both(Exchange::BinanceCoinM, "AAVEUSD_PERP", "AAVE", "USD", None);
    both(
        Exchange::BinanceCoinM,
        "BNBUSD_260925",
        "BNB",
        "USD",
        Some("260925"),
    );
}

#[test]
fn bybit_concat_perp_and_dated() {
    both(Exchange::Bybit, "0GUSDT", "0G", "USDT", None);
    both(
        Exchange::Bybit,
        "BTCUSDT-07AUG26",
        "BTC",
        "USDT",
        Some("07AUG26"),
    );
    both(
        Exchange::Bybit,
        "ETHUSDT-25JUN27",
        "ETH",
        "USDT",
        Some("25JUN27"),
    );
}

/// A USDC perpetual keeps its `PERP`: the core reports the coin as `AAVEPERP`, so stripping it
/// here would write a blacklist token the core does not associate with this market.
#[test]
fn bybit_usdc_perp_keeps_perp() {
    both(Exchange::Bybit, "AAVEPERP", "AAVEPERP", "", None);
    both(Exchange::Bybit, "1000BONKPERP", "1000BONKPERP", "", None);
}

#[test]
fn gate_underscore() {
    both(Exchange::Gate, "0G_USDT", "0G", "USDT", None);
    both(Exchange::Gate, "4_USDT", "4", "USDT", None);
    both(Exchange::Gate, "ETH_BTC", "ETH", "BTC", None);
    // A coin whose own name ends in a quote, and one that is literally called SWAP.
    both(Exchange::Gate, "CRVUSD_USDT", "CRVUSD", "USDT", None);
    both(Exchange::Gate, "SWAP_USDT", "SWAP", "USDT", None);
    both(Exchange::Gate, "EZSWAP_USDT", "EZSWAP", "USDT", None);
}

/// Gate lists coins with non-ASCII names; the cut must stay on a character boundary.
#[test]
fn non_ascii_base() {
    both(Exchange::Gate, "龙虾_USDT", "龙虾", "USDT", None);
    both(Exchange::Binance, "币安人生USDT", "币安人生", "USDT", None);
}

#[test]
fn hyperliquid_shapes() {
    both(Exchange::Hyperliquid, "KAITO", "KAITO", "", None);
    both(Exchange::Hyperliquid, "0G", "0G", "", None);
    // Spot pairs and HIP-3 names are decided before the family rules, so both paths agree.
    both(Exchange::Hyperliquid, "PURR/USDC", "PURR", "USDC", None);
    let hip3 = split_market("xyz:AAPL", Exchange::Hyperliquid);
    assert_eq!((hip3.dex, hip3.base, hip3.quote), (Some("xyz"), "AAPL", ""));
}

/// A spot index carries no coin at all. It must come back untouched so callers can see that and
/// resolve it through the catalog's `market_name_mb_classic` instead of displaying a guess.
#[test]
fn hyperliquid_index_is_not_a_coin() {
    both(Exchange::Hyperliquid, "@206", "@206", "", None);
    both(Exchange::Hyperliquid, "@1", "@1", "", None);
}

#[test]
fn bitget_concat() {
    both(Exchange::BitGet, "PYUSDUSDT", "PYUSD", "USDT", None);
    both(Exchange::BitGet, "TUSDUSDT", "TUSD", "USDT", None);
}

/// Shapes that must NOT be read as a contract tail, or a real coin would be truncated.
#[test]
fn tails_only_when_unambiguous() {
    // Four digits are not a `YYMMDD`, and an unknown word is not a date.
    let p = split_market("FOOUSDT_2", Exchange::Unknown);
    assert_eq!((p.base, p.contract), ("FOOUSDT_2", None));
    let p = split_market("FOOUSDT_BAR", Exchange::Unknown);
    assert_eq!((p.base, p.contract), ("FOOUSDT_BAR", None));
    // `-SWAP` needs a quote in front of it to be OKX's perpetual marker.
    let p = split_market("EZSWAP", Exchange::Unknown);
    assert_eq!((p.base, p.quote), ("EZSWAP", ""));
}

/// The free-text entrypoint the Log panel scans with. The coin, or `None` for a plain word.
use market_in_text as in_text;

#[test]
fn text_tokens_that_are_markets() {
    // The ordinary reading wins over the quote-first one. `BTC` is itself a quote currency, so
    // reading `BTC-USDT` quote-first would report the coin as USDT.
    assert_eq!(in_text("BTC-USDT"), Some("BTC"));
    assert_eq!(in_text("BTC_USDT"), Some("BTC"));
    assert_eq!(in_text("BNB_USDT"), Some("BNB"));
    assert_eq!(in_text("ETH-USDT-SWAP"), Some("ETH"));
    assert_eq!(in_text("SPKUSDT"), Some("SPK"));
    // Quote first, as core logs also write it — including when the COIN is itself a quote
    // currency, which is the ambiguous case the leading stablecoin resolves.
    assert_eq!(in_text("USDT-SPK"), Some("SPK"));
    assert_eq!(in_text("USDT_SPK"), Some("SPK"));
    assert_eq!(in_text("USDT-BTC"), Some("BTC"));
    assert_eq!(in_text("USDC_ETH"), Some("ETH"));
    // A USDC perpetual keeps its tail, as the core's own coin token does.
    assert_eq!(in_text("AAVEPERP"), Some("AAVEPERP"));
    assert_eq!(in_text("1000BONKPERP"), Some("1000BONKPERP"));
    // The report's contract spelling, whose four-digit tail no catalog name uses.
    assert_eq!(in_text("BTC_0626"), Some("BTC"));
    assert_eq!(in_text("ETH-240628"), Some("ETH"));
}

#[test]
fn text_tokens_that_are_not_markets() {
    // Ordinary log words, and a date that must not read as a ticker.
    assert_eq!(in_text("REJECTED"), None);
    assert_eq!(in_text("2026-06-30"), None);
    assert_eq!(in_text("BUY"), None);
    assert_eq!(in_text("1234"), None);
    // Too short to be a market even though it is a real coin; a bare ticker is matched elsewhere,
    // against coins already seen in the buffer.
    assert_eq!(in_text("SPK"), None);
}

/// A name that is nothing but separators must not produce a phantom coin or panic.
#[test]
fn degenerate_names() {
    for name in ["", "-", "-USDT", "BTC-", ":", "_USDT", "@"] {
        let p = split_market(name, Exchange::Unknown);
        assert!(
            p.base.len() <= name.len(),
            "{name} produced a longer base {}",
            p.base
        );
    }
}
