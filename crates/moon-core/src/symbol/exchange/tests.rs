use super::Exchange;
use moonproto::ExchangeCode;

/// The ordinals are wire values, so the oracle is moonproto's own constants rather than the
/// literals this module was written from: a renumbering upstream must fail here, and asserting
/// each `match` arm back at itself could not catch that.
#[test]
fn codes_map_to_families() {
    let cases = [
        (ExchangeCode::FBybit, Exchange::Bybit),
        (ExchangeCode::ByBit, Exchange::Bybit),
        (ExchangeCode::Binance, Exchange::Binance),
        (ExchangeCode::FBinance, Exchange::Binance),
        (ExchangeCode::QBinance, Exchange::BinanceCoinM),
        (ExchangeCode::Huobi, Exchange::Huobi),
        (ExchangeCode::Gate, Exchange::Gate),
        (ExchangeCode::FGate, Exchange::Gate),
        (ExchangeCode::BitGet, Exchange::BitGet),
        (ExchangeCode::FBitGet, Exchange::BitGet),
        (ExchangeCode::Hyper, Exchange::Hyperliquid),
        (ExchangeCode::FHyper, Exchange::Hyperliquid),
        (ExchangeCode::OKX, Exchange::Okx),
        (ExchangeCode::FOKX, Exchange::Okx),
    ];
    for (code, family) in cases {
        assert_eq!(
            Exchange::from_code(code.stable_id()),
            family,
            "{code:?} (ordinal {})",
            code.stable_id()
        );
    }
}

/// A future ordinal must fall back to shape recognition, not to some neighbour's rules.
#[test]
fn unknown_ordinals_do_not_guess() {
    assert_eq!(
        Exchange::from_code(ExchangeCode::None.stable_id()),
        Exchange::Unknown
    );
    assert_eq!(
        Exchange::from_code(ExchangeCode::FOKX.stable_id() + 1),
        Exchange::Unknown
    );
    assert_eq!(Exchange::from_code(255), Exchange::Unknown);
}
