//! Persisted quote-currency decoding and safe-breakdown regression tests.

use super::*;

/// Every persisted MoonBot quote ordinal keeps its exact ticker identity.
///
/// Reordering or mistyping any arm in `QuoteCurrency::from_report_ordinal` must fail this
/// independently transcribed contract table, otherwise historical PnL is labeled as another
/// asset.
#[test]
fn persisted_ordinals_keep_distinct_quote_identities() {
    let expected = [
        "BTC", "USDT", "ETH", "BNB", "AUD", "TUSD", "BRL", "USDH", "USDC", "FDUSD", "AEUR", "USD",
        "TRX", "RUB", "EUR", "HTX", "USDD", "IDR", "DOGE", "TRY", "USDE",
    ];
    let actual = (0..=20)
        .map(|ordinal| {
            QuoteCurrency::from_report_ordinal(ordinal)
                .expect("known persisted ordinal")
                .ticker()
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert!(QuoteCurrency::from_report_ordinal(21).is_none());
    assert!(QuoteCurrency::from_report_ordinal(25).is_none());
    assert!(QuoteCurrency::from_report_ordinal(26).is_none());
    assert!(QuoteCurrency::from_report_ordinal(255).is_none());
    assert_eq!(
        QuoteCurrency::from_report_value(&Value::Real(8.0)),
        None,
        "non-integer SQLite storage must not inherit a currency"
    );
}

/// Plausible regression: removing the currency key from `QuoteBreakdown::from_groups` must fail
/// the bucket assertions, otherwise USDT and USDC are silently added into one false total.
#[test]
fn breakdown_merges_only_identical_known_quotes() {
    let totals = QuoteBreakdown::from_groups([
        (Some(1), 10.0, 2),
        (Some(8), 3.0, 1),
        (Some(1), -2.0, 1),
        (None, 9_999.0, 4),
        (Some(26), 8_888.0, 5),
    ]);

    assert_eq!(totals.orders, 13);
    assert_eq!(totals.unknown_orders, 9);
    assert_eq!(totals.totals.len(), 2);
    assert_eq!(totals.totals[0].currency.ticker(), "USDT");
    assert_eq!(totals.totals[0].profit, 8.0);
    assert_eq!(totals.totals[0].orders, 3);
    assert_eq!(totals.totals[1].currency.ticker(), "USDC");
    assert_eq!(totals.totals[1].profit, 3.0);
    assert_eq!(totals.scope(), QuoteScope::Unknown);
}
