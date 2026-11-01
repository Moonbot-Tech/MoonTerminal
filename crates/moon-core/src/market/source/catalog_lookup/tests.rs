use super::spellings_on;
use crate::symbol::Exchange;

/// A coin delisted after the trade is spelled in the core's ACCOUNT currency, not its quote
/// setting: a BTC-quoted setting must not turn a USDT core's tape name into `...BTC`.
#[test]
fn an_unlisted_coin_is_spelled_in_the_account_currency() {
    assert_eq!(
        spellings_on(Exchange::Bybit, "ETHBTC", Some("USDT".into()), "SPORTFUN"),
        vec!["SPORTFUNUSDT".to_string()]
    );
    assert_eq!(
        spellings_on(Exchange::Gate, "BTCUSDT", Some("USDT".into()), "SPORTFUN"),
        vec!["SPORTFUN_USDT".to_string()]
    );
}

/// Without a stated account currency the quote setting decides, and a stored value that is
/// already a market of that quote is its own only spelling — never `SPORTFUNUSDTUSDT`.
#[test]
fn the_setting_decides_without_an_account_currency_and_a_full_market_stays() {
    assert_eq!(
        spellings_on(Exchange::Bybit, "BTCUSDT", None, "SPORTFUN"),
        vec!["SPORTFUNUSDT".to_string()]
    );
    assert_eq!(
        spellings_on(
            Exchange::Bybit,
            "BTCUSDT",
            Some("USDT".into()),
            "SPORTFUNUSDT"
        ),
        vec!["SPORTFUNUSDT".to_string()]
    );
}
