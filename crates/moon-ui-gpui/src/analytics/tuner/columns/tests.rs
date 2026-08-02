//! Regression coverage for unit-bearing Tuning metric values.

use super::{COL_AVG_ORDER, COL_PROFIT};
use crate::analytics::set_pnl_unit;
use moon_core::db::analytics::GroupStat;
use moon_core::db::{ProfitUnit, QuoteCurrency, QuoteScope};

/// Removing the exact ticker from `columns.rs:tuner_profit_text` must fail the USDC assertion;
/// otherwise Tuning shows ambiguous or historically hard-coded money beside its percentage column.
#[test]
fn profit_value_carries_the_active_lens_unit() {
    let mut group = GroupStat {
        profit: 12.34,
        quote: QuoteScope::Single(
            QuoteCurrency::from_report_ordinal(8).expect("USDC report ordinal"),
        ),
        ..GroupStat::default()
    };

    set_pnl_unit(Some(ProfitUnit::Quote(
        QuoteCurrency::from_report_ordinal(8).expect("USDC report ordinal"),
    )));
    assert_eq!((COL_PROFIT.text)(&group), "+12.34 USDC");

    set_pnl_unit(Some(ProfitUnit::Percent));
    assert_eq!((COL_PROFIT.text)(&group), "+12.34%");

    set_pnl_unit(Some(ProfitUnit::Quote(
        QuoteCurrency::from_report_ordinal(8).expect("USDC report ordinal"),
    )));
    group.profit = f64::NAN;
    assert_eq!((COL_PROFIT.text)(&group), "—");
}

/// Removing the ticker from `columns.rs:tuner_avg_order_text` must fail the finite assertion;
/// otherwise Avg order again renders ambiguous unitless money.
#[test]
fn average_order_value_carries_the_exact_quote_unit() {
    let mut group = GroupStat {
        avg_order: 19_983.48,
        quote: QuoteScope::Single(
            QuoteCurrency::from_report_ordinal(8).expect("USDC report ordinal"),
        ),
        ..GroupStat::default()
    };

    assert_eq!((COL_AVG_ORDER.text)(&group), "19 983.48 USDC");

    group.avg_order = f64::NAN;
    assert_eq!((COL_AVG_ORDER.text)(&group), "—");
}

/// Mixed-quote raw money must never be formatted as a scalar amount.
///
/// Removing the `QuoteScope` guard in `columns.rs` makes either cell expose an invalid sum.
#[test]
fn mixed_quote_money_stays_unavailable() {
    let group = GroupStat {
        profit: 12.34,
        avg_order: 100.0,
        quote: QuoteScope::Mixed,
        ..GroupStat::default()
    };
    set_pnl_unit(None);

    assert_eq!((COL_PROFIT.text)(&group), "—");
    assert_eq!((COL_AVG_ORDER.text)(&group), "—");
}

/// `columns.rs:quote_amount` must not return stable/fiat quotes to `usd_grouped`; that formatter
/// trims trailing zeros, so equal-currency Profit and Avg order cells drift between one and two
/// fractional positions. Crypto must still retain meaningful precision instead of rounding to cents.
#[test]
fn quote_money_keeps_its_currency_precision_and_grouping() {
    let usdt = QuoteCurrency::usdt();
    let mut group = GroupStat {
        profit: 1_842.0,
        avg_order: 1_842.5,
        quote: QuoteScope::Single(usdt),
        ..GroupStat::default()
    };
    set_pnl_unit(Some(ProfitUnit::Quote(usdt)));

    assert_eq!((COL_PROFIT.text)(&group), "+1 842.00 USDT");
    assert_eq!((COL_AVG_ORDER.text)(&group), "1 842.50 USDT");

    let btc = QuoteCurrency::from_report_ordinal(0).expect("BTC report ordinal");
    group.profit = 0.12345678;
    group.quote = QuoteScope::Single(btc);
    set_pnl_unit(Some(ProfitUnit::Quote(btc)));
    assert_eq!((COL_PROFIT.text)(&group), "+0.12345678 BTC");
}
