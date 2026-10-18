use super::*;
use crate::db::ValuationCoverage;

const USDT: i64 = 1;
const USDC: i64 = 8;
const RUB: i64 = 13;

/// One source group: `(ordinal, native profit, orders, valued rows, USDT profit of those rows)`.
type Group = (Option<i64>, f64, i64, i64, f64);

/// Build a breakdown the way the Report totals read does: per-group sums kept in
/// [`SourceGroups`], whole-scope coverage folded from the same groups.
fn breakdown(groups: &[Group]) -> QuoteBreakdown {
    let mut source = SourceGroups::default();
    let mut coverage = CoverageAggregate::default();
    for &(ordinal, profit, orders, valued, usdt) in groups {
        let known = ordinal
            .and_then(QuoteCurrency::from_report_ordinal)
            .is_some();
        let eligible = if known { orders } else { 0 };
        source.profit.push((ordinal, profit, orders));
        source
            .coverage
            .push((ordinal, eligible, valued, 0, usdt, 0.0, 0));
        source.volume.push((ordinal, 0, 0, 0.0, 0, 0.0));
        source.spend.push((ordinal, 0, 0.0, 0.0, 0, 0.0, 0.0));
        coverage.add(eligible, valued, 0, usdt, 0.0, 0);
    }
    let mut out = QuoteBreakdown::from_groups(source.profit.iter().copied())
        .with_valuation(coverage.finish());
    out.source_groups = Box::new(source);
    out
}

fn currency(ordinal: i64) -> QuoteCurrency {
    QuoteCurrency::from_report_ordinal(ordinal).expect("known ordinal")
}

/// The case the split exists for: dollars and USDC convert, a ruble trade cannot. The valued
/// currencies must come back as ONE complete USDT figure and the ruble trade in rubles, instead
/// of the whole total collapsing to "unvalued".
#[test]
fn rubles_split_off_and_the_rest_stays_one_usdt_total() {
    let total = breakdown(&[
        (Some(USDT), -193.03, 1, 1, -193.03),
        (Some(USDC), 212.40, 11, 11, 212.35),
        (Some(RUB), 200.0, 1, 0, 0.0),
    ]);
    assert!(total.unified_usdt().is_none());
    let parts = total.valuation_split();
    assert_eq!(parts.len(), 2, "{parts:?}");
    let ValuationPart::Usdt(usdt) = &parts[0] else {
        panic!("first part must be USDT: {parts:?}");
    };
    let unified = usdt.unified_usdt().expect("the valued part is complete");
    assert!((unified.profit - 19.32).abs() < 1e-9, "{}", unified.profit);
    assert_eq!(usdt.orders, 12);
    let ValuationPart::Native(quote, rub) = &parts[1] else {
        panic!("second part must be native: {parts:?}");
    };
    assert_eq!(*quote, currency(RUB));
    assert_eq!(rub.orders, 1);
    assert_eq!(rub.totals.len(), 1);
    assert!((rub.totals[0].profit - 200.0).abs() < 1e-9);
    assert!(rub.unified_usdt().is_none());
}

/// A currency with only SOME rows valued stays whole in its own money: adding the valued rows'
/// dollars to the USDT line would print a partial sum that reads like a complete one.
#[test]
fn a_half_valued_currency_stays_native_and_out_of_the_usdt_line() {
    let total = breakdown(&[(Some(USDT), 10.0, 2, 2, 10.0), (Some(USDC), 5.0, 3, 1, 1.0)]);
    let parts = total.valuation_split();
    let [
        ValuationPart::Usdt(usdt),
        ValuationPart::Native(quote, usdc),
    ] = parts.as_slice()
    else {
        panic!("{parts:?}");
    };
    assert_eq!(usdt.unified_usdt().map(|value| value.profit), Some(10.0));
    assert_eq!(usdt.orders, 2);
    assert_eq!(*quote, currency(USDC));
    assert_eq!(usdc.orders, 3);
}

/// Rows of an unknown currency are stated by count alone, after every known currency.
#[test]
fn unknown_currency_rows_come_last_as_a_count() {
    let total = breakdown(&[(Some(USDT), 1.0, 1, 1, 1.0), (None, 7.0, 4, 0, 0.0)]);
    let parts = total.valuation_split();
    assert!(
        matches!(
            parts.as_slice(),
            [ValuationPart::Usdt(_), ValuationPart::Unknown(4)]
        ),
        "{parts:?}"
    );
}

/// Without a valuation projection nothing is converted, so even USDT is stated natively —
/// never a dollar figure the read did not produce.
#[test]
fn no_projection_leaves_every_currency_native() {
    let mut total = breakdown(&[(Some(USDT), 3.0, 1, 1, 3.0), (Some(RUB), 50.0, 1, 0, 0.0)]);
    total.valuation = None;
    let parts = total.valuation_split();
    assert!(
        matches!(
            parts.as_slice(),
            [ValuationPart::Native(a, _), ValuationPart::Native(b, _)]
                if *a == currency(USDT) && *b == currency(RUB)
        ),
        "{parts:?}"
    );
}

/// A hand-built breakdown records no groups; it still splits by its profit buckets.
#[test]
fn a_breakdown_without_groups_splits_by_its_buckets() {
    let total =
        QuoteBreakdown::from_groups([(Some(RUB), 9.0, 2)]).with_valuation(ValuationCoverage {
            eligible_orders: 2,
            ..Default::default()
        });
    let parts = total.valuation_split();
    let [ValuationPart::Native(quote, rub)] = parts.as_slice() else {
        panic!("{parts:?}");
    };
    assert_eq!(*quote, currency(RUB));
    assert_eq!((rub.orders, rub.totals[0].profit), (2, 9.0));
}
