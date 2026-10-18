//! Splitting a breakdown into the currencies that convert to USDT and the ones that do not.
//!
//! A unified USDT total is all or nothing: one trade in a currency with no USDT route — RUB, which
//! no provider quotes since Binance's ruble pairs stopped trading — leaves the whole total
//! unvalued and hides the dollars every other trade did convert to. The split keeps the fully
//! valued currencies together as one USDT figure and states each remaining currency in its own
//! money. The two are never added: a ruble amount has no exchange rate here to add it with.

use super::{EntrySpend, QuoteBreakdown, QuoteCurrency, TradedVolume};
use crate::db::valuation::CoverageAggregate;

/// The per-source, per-quote groups a breakdown was folded from.
///
/// Kept so a sub-scope of the breakdown — some of its currencies — is rebuilt by the same
/// constructors that built the whole, not by a second copy of their arithmetic. Each vector holds
/// exactly the tuples its constructor took: [`QuoteBreakdown::from_groups`],
/// [`TradedVolume::from_groups`], [`EntrySpend::from_groups`] and the six valuation-coverage
/// sums. Empty on a breakdown built by hand or by a reader that does not record its groups.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceGroups {
    /// `(ordinal, profit, orders)` per source and quote group.
    pub profit: Vec<(Option<i64>, f64, i64)>,
    /// `(ordinal, eligible, valued, unavailable, USDT profit, USDT spent, spent rows)`; empty when
    /// the read had no valuation projection.
    pub coverage: Vec<(Option<i64>, i64, i64, i64, f64, f64, i64)>,
    /// Traded-volume groups as [`TradedVolume::from_groups`] takes them.
    pub volume: Vec<(Option<i64>, i64, i64, f64, i64, f64)>,
    /// Entry-spend groups as [`EntrySpend::from_groups`] takes them.
    pub spend: Vec<(Option<i64>, i64, f64, f64, i64, f64, f64)>,
}

impl SourceGroups {
    /// Keep only the groups whose ordinal decodes to a currency `keep` admits.
    ///
    /// Args:
    ///     keep: Currencies of the sub-scope. An unknown ordinal is never admitted.
    ///
    /// Returns:
    ///     The same four group lists narrowed to the sub-scope, order preserved, so the sums
    ///     rebuilt from them fold in the order the whole did.
    fn restricted(&self, keep: &impl Fn(QuoteCurrency) -> bool) -> Self {
        let admit = |ordinal: Option<i64>| {
            ordinal
                .and_then(QuoteCurrency::from_report_ordinal)
                .is_some_and(keep)
        };
        Self {
            profit: self.profit.iter().copied().filter(|g| admit(g.0)).collect(),
            coverage: self
                .coverage
                .iter()
                .copied()
                .filter(|g| admit(g.0))
                .collect(),
            volume: self.volume.iter().copied().filter(|g| admit(g.0)).collect(),
            spend: self.spend.iter().copied().filter(|g| admit(g.0)).collect(),
        }
    }
}

/// One line of a breakdown whose USDT total is incomplete.
#[derive(Clone, Debug, PartialEq)]
pub enum ValuationPart {
    /// Every currency whose rows all carry a USDT value, as one breakdown whose
    /// [`QuoteBreakdown::unified_usdt`] is complete.
    Usdt(QuoteBreakdown),
    /// One currency with at least one row lacking a USDT value, stated in its own money.
    Native(QuoteCurrency, QuoteBreakdown),
    /// Rows whose currency is unknown. Only their count is safe to state: their amounts may be in
    /// several incomparable currencies.
    Unknown(i64),
}

impl QuoteBreakdown {
    /// Split this breakdown into its USDT-valued part and one native part per other currency.
    ///
    /// A currency joins the USDT part only when EVERY one of its rows is valued. A currency that
    /// is half valued — rates still being fetched for some of its trades — stays whole in its own
    /// money rather than contributing a partial dollar sum that would read like a complete one.
    ///
    /// Meant for a breakdown whose [`Self::unified_usdt`] is `None`; on a complete one it returns
    /// the whole breakdown as its single USDT part.
    ///
    /// Returns:
    ///     The USDT part first when any currency is fully valued, then native parts in ordinal
    ///     order, then the unknown-currency count when there is one.
    pub fn valuation_split(&self) -> Vec<ValuationPart> {
        let valued: Vec<QuoteCurrency> = self
            .totals
            .iter()
            .map(|total| total.currency)
            .filter(|&currency| self.currency_valued(currency))
            .collect();
        let mut parts = Vec::with_capacity(self.totals.len() + 1);
        if !valued.is_empty() {
            parts.push(ValuationPart::Usdt(
                self.restricted(|currency| valued.contains(&currency)),
            ));
        }
        for total in &self.totals {
            if !valued.contains(&total.currency) {
                parts.push(ValuationPart::Native(
                    total.currency,
                    self.restricted(|currency| currency == total.currency),
                ));
            }
        }
        if self.unknown_orders > 0 {
            parts.push(ValuationPart::Unknown(self.unknown_orders));
        }
        parts
    }

    /// Whether every row of `currency` carries a USDT value under this read's coverage.
    ///
    /// Answered from the recorded coverage groups only: a breakdown without a valuation
    /// projection, or without recorded groups, values nothing, so every currency stays native.
    fn currency_valued(&self, currency: QuoteCurrency) -> bool {
        if self.valuation.is_none() {
            return false;
        }
        let (eligible, valued, unavailable) = self
            .source_groups
            .coverage
            .iter()
            .filter(|group| group.0.and_then(QuoteCurrency::from_report_ordinal) == Some(currency))
            .fold((0, 0, 0), |(e, v, u), group| {
                (e + group.1, v + group.2, u + group.3)
            });
        eligible > 0 && eligible == valued && unavailable == 0
    }

    /// Rebuild the breakdown over the currencies `keep` admits.
    ///
    /// Args:
    ///     keep: Currencies of the sub-scope.
    ///
    /// Returns:
    ///     A breakdown built by the same constructors as the whole, over the admitted groups only.
    ///     Without recorded groups, only the profit buckets are known and only they are kept.
    ///     `counted_cores` is zero: a per-currency core count is not recorded, and zero is the
    ///     documented "not stated" value.
    fn restricted(&self, keep: impl Fn(QuoteCurrency) -> bool) -> Self {
        if self.source_groups.profit.is_empty() {
            return Self::from_groups(self.totals.iter().filter(|total| keep(total.currency)).map(
                |total| {
                    (
                        Some(i64::from(total.currency.ordinal())),
                        total.profit,
                        total.orders,
                    )
                },
            ));
        }
        let groups = self.source_groups.restricted(&keep);
        let mut out = Self::from_groups(groups.profit.iter().copied())
            .with_traded_volume(TradedVolume::from_groups(groups.volume.iter().copied()))
            .with_entry_spend(EntrySpend::from_groups(groups.spend.iter().copied()));
        if self.valuation.is_some() {
            let mut coverage = CoverageAggregate::default();
            for &(_, eligible, valued, unavailable, profit, spent, spent_rows) in &groups.coverage {
                coverage.add(eligible, valued, unavailable, profit, spent, spent_rows);
            }
            out = out.with_valuation(coverage.finish());
        }
        out.source_groups = Box::new(groups);
        out
    }
}

#[cfg(test)]
mod tests;
