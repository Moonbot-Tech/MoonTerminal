//! Which exchange a market name comes from, and therefore which naming rules apply to it.
//!
//! Every exchange spells the same pair differently — `BTCUSDT` on Binance, `BTC_USDT` on Gate,
//! `BTC-USDT-SWAP` on OKX — so the parser needs to know whose name it is looking at. This enum is
//! the dispatch key; [`super::parse`] holds the rules themselves.

/// Exchange family for market-name parsing.
///
/// Spot and futures connections to the same venue share a variant when they share a naming scheme,
/// and split when they do not: Binance USD-M spells `BTCUSDT` while its COIN-M quarterly core
/// spells `BTCUSD_PERP`, so those are two variants rather than one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Exchange {
    /// The exchange is not known at this call site — for example a log line or a report row read
    /// before any core is connected. Parsing falls back to recognizing the shape of the name.
    #[default]
    Unknown,
    /// Binance spot and USD-M futures: `BTCUSDT`, `BTCUSDT_260925`.
    Binance,
    /// Binance COIN-M quarterly (`QBinance`): `BTCUSD_PERP`, `BNBUSD_260925`.
    BinanceCoinM,
    /// Bybit spot and futures: `BTCUSDT`, `BTCPERP`, `BTCUSDT-07AUG26`.
    Bybit,
    /// Gate spot and futures: `BTC_USDT`, `ETH_BTC`.
    Gate,
    /// BitGet spot and futures: `BTCUSDT`.
    BitGet,
    /// Huobi: concatenated, as Binance.
    Huobi,
    /// Hyperliquid: bare coin `BTC`, HIP-3 `xyz:BIRD`, spot index `@206`, spot pair `PURR/USDC`.
    Hyperliquid,
    /// OKX spot and futures: `BTC-USDT`, `BTC-USDT-SWAP`, `BTC-USDT-250808`.
    Okx,
}

impl Exchange {
    /// Maps MoonProto's `ExchangeCode` ordinal onto a naming family.
    ///
    /// The ordinal is resolved through [`crate::venue`], the one table that answers everything a
    /// platform code decides — brand, market kind, naming rules, order-book kind — so a venue
    /// cannot be spelled by one rule here and drawn by another elsewhere. An unknown future ordinal
    /// falls back to [`Exchange::Unknown`], which parses by shape instead of guessing a scheme.
    pub const fn from_code(code: u8) -> Self {
        match crate::venue::venue(code) {
            Some(venue) => venue.naming(),
            None => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests;
