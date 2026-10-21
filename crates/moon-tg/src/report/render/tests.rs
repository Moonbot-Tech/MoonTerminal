//! A trade without a USDT rate must not hide what the valued trades made.
use super::super::{Page, Row};
use super::{profit, report_html};
use moon_core::config::telegram_menu::ReportBasis;
use moon_core::config::{ReportColumn, ReportLayout, TotalPlace};
use moon_core::db::{QuoteBreakdown, SourceGroups, ValuationCoverage};
use moon_core::telegram::report::{Period, ReportRequest};

const USDT: i64 = 1;
const USDC: i64 = 8;
const RUB: i64 = 13;

/// A breakdown shaped like a Report totals read: one source group per
/// `(ordinal, native profit, orders, valued rows, USDT profit)`. Every trade spent 100 of its own
/// currency, so a part's average-order return is its profit in percent.
fn money(groups: &[(i64, f64, i64, i64, f64)]) -> QuoteBreakdown {
    let mut source = SourceGroups::default();
    let (mut eligible, mut valued, mut usdt) = (0, 0, 0.0);
    for &(ordinal, profit, orders, rows_valued, profit_usdt) in groups {
        source.profit.push((Some(ordinal), profit, orders));
        source
            .coverage
            .push((Some(ordinal), orders, rows_valued, 0, profit_usdt, 0.0, 0));
        let spent = 100.0 * orders as f64;
        let spent_usdt = if rows_valued == orders { spent } else { 0.0 };
        source.spend.push((
            Some(ordinal),
            orders,
            spent,
            profit,
            rows_valued,
            spent_usdt,
            profit_usdt,
        ));
        eligible += orders;
        valued += rows_valued;
        usdt += profit_usdt;
    }
    let complete = eligible == valued;
    let mut total = QuoteBreakdown::from_groups(source.profit.iter().copied()).with_valuation(
        ValuationCoverage {
            eligible_orders: eligible,
            valued_orders: valued,
            unavailable_orders: 0,
            usdt: complete.then_some(moon_core::db::UsdtTotal {
                profit: usdt,
                spent: None,
            }),
        },
    );
    total.source_groups = Box::new(source);
    total
}

/// The report from the field: a Binance USDT loss, eleven Hyperliquid USDC trades and one ruble
/// trade on a venue no provider quotes.
fn exchanges_page() -> Page {
    let binance = money(&[(USDT, -193.03, 1, 1, -193.03)]);
    let hyperliquid = money(&[(USDC, 212.40, 11, 11, 212.35)]);
    let ruble = money(&[(RUB, 200.0, 1, 0, 0.0)]);
    let mut request = ReportRequest::new(Period::Today, false);
    request.by_exchange = true;
    Page {
        request,
        from: 0,
        to: 1,
        zone: chrono_tz::UTC,
        total: money(&[
            (USDT, -193.03, 1, 1, -193.03),
            (USDC, 212.40, 11, 11, 212.35),
            (RUB, 200.0, 1, 0, 0.0),
        ]),
        rows: vec![
            Row::Line(None, "Binance Futures".into(), binance),
            Row::Line(None, "Hyperliquid".into(), hyperliquid),
            Row::Line(None, "Ruble venue".into(), ruble),
        ],
        pages: 1,
        drilldowns: vec![],
        scope_label: None,
        basis: ReportBasis::Close,
        layout: ReportLayout {
            columns: vec![ReportColumn::Profit, ReportColumn::Trades],
            total: TotalPlace::Bottom,
            ..Default::default()
        }
        .sanitized(),
        cores: vec![],
        caption: None,
        exchange: None,
        groups_missing: false,
    }
}

/// The ruble venue's own row states its rubles instead of "unvalued".
#[test]
fn an_unpriced_currency_row_shows_its_own_money() {
    let _locale = crate::test_locale::force("en");
    let page = exchanges_page();
    assert_eq!(profit(page.rows[2].total()), "+200.00\u{a0}RUB");
    assert_eq!(profit(page.rows[1].total()), "+212.35$");
}

/// The total splits into a USDT line over the valued venues and a ruble line, each with its own
/// trade count, where it used to read "unvalued" from end to end.
#[test]
fn the_total_splits_into_one_line_per_unpriced_currency() {
    let _locale = crate::test_locale::force("en");
    let html = report_html(&exchanges_page());
    assert!(
        html.contains(
            "<tr><th align=\"left\"><b>Total USDT</b></th><th align=\"right\"><b>+19.32$</b></th>\
             <th align=\"right\"><b>12</b></th></tr>"
        ),
        "{html}"
    );
    assert!(
        html.contains(
            "<tr><th align=\"left\"><b>Total RUB</b></th>\
             <th align=\"right\"><b>+200.00\u{a0}RUB</b></th><th align=\"right\"><b>1</b></th></tr>"
        ),
        "{html}"
    );
    // The main table only: the details keep their own per-currency accounting.
    let table = &html[..html.find("</table>").expect("main table")];
    assert!(
        !table.contains(&*crate::t!("telegram.report_unvalued")),
        "{html}"
    );
}

/// A row mixing a valued and an unpriced currency states both, never one sum of the two.
#[test]
fn a_mixed_row_states_each_part() {
    let _locale = crate::test_locale::force("en");
    let mixed = money(&[(USDT, -193.03, 1, 1, -193.03), (RUB, 200.0, 1, 0, 0.0)]);
    assert_eq!(profit(&mixed), "-193.03$; +200.00\u{a0}RUB");
    // The average column follows the same parts instead of reading "unvalued" beside them.
    let layout = ReportLayout {
        columns: vec![ReportColumn::Average],
        ..Default::default()
    }
    .sanitized();
    assert_eq!(
        super::row_html(&Row::Line(None, "Mixed".into(), mixed), false, &layout),
        "<tr><td>Mixed</td><td align=\"right\">-193.0%; +200.0%</td></tr>"
    );
}

/// A fully valued total keeps its single plain "Total" band.
#[test]
fn a_complete_total_keeps_one_band() {
    let _locale = crate::test_locale::force("en");
    let mut page = exchanges_page();
    page.rows.pop();
    page.total = money(&[
        (USDT, -193.03, 1, 1, -193.03),
        (USDC, 212.40, 11, 11, 212.35),
    ]);
    let html = report_html(&page);
    assert!(html.contains("<b>Total</b>"), "{html}");
    assert!(!html.contains("Total USDT"), "{html}");
}

/// The details' average order must not read "unavailable" for a row the table states per part.
///
/// A row in USDT and RUB (no USDT rate) splits per currency in the table — Average
/// `+10.0%; +200.0%` — so the details block of the same message states the same row's average
/// order per part too, instead of the whole-set "unavailable" (issue #1051).
#[test]
fn details_average_follows_the_table_split() {
    let _locale = crate::test_locale::force("en");
    let mixed = money(&[(USDT, 10.0, 1, 1, 10.0), (RUB, 200.0, 1, 0, 0.0)]);
    let mut page = exchanges_page();
    page.request.by_exchange = false;
    page.total = mixed.clone();
    page.rows = vec![Row::Line(Some(1), "Mixed".into(), mixed)];
    page.layout = ReportLayout {
        columns: vec![ReportColumn::Profit, ReportColumn::Average],
        total: TotalPlace::Bottom,
        ..Default::default()
    }
    .sanitized();
    let html = report_html(&page);
    assert!(html.contains("+10.0%; +200.0%"), "{html}");
    let details = &html[html.find("<details>").unwrap()..];
    assert!(
        !details.contains(&*rust_i18n::t!("telegram.report_unvalued")),
        "{details}"
    );
}
