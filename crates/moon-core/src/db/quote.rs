//! Currency identity and safe aggregate shapes for persisted MoonBot reports.
//!
//! The report schema stores `basecurrency` as the ordinal of MoonBot's
//! `TBaseCurrency`. This module is the only place in MoonTerminal that decodes
//! that persisted contract. Historical rows must never inherit the quote from
//! a core's current configuration because a core can change quote over time.
//!
//! MoonProto owns the wire enum, but its raw-byte constructor is deliberately
//! private outside diagnostics builds. SQLite additionally needs a strict
//! storage-class check and must reject placeholders/sentinels, so this reader
//! boundary mirrors the current 0..=20 contract and pins the complete table in
//! tests instead of enabling MoonProto diagnostics in production.

use std::collections::BTreeMap;

use rusqlite::types::Value;

/// Decode a SQLite report value into an integral persisted currency ordinal.
///
/// Args:
///     value: Raw `basecurrency` value from SQLite.
///
/// Returns:
///     An integer ordinal, or `None` for every non-integer SQLite storage class.
pub(crate) fn report_ordinal_from_value(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(value) => Some(*value),
        Value::Null | Value::Real(_) | Value::Text(_) | Value::Blob(_) => None,
    }
}

/// Build the trusted quote projection and grouping suffix for one report source.
///
/// SQLite considers numeric `INTEGER 1` and malformed `REAL 1.0` equal for `GROUP BY`. Grouping
/// the raw column could therefore let an arbitrary row decide whether the complete amount is
/// trusted. The storage-class guard separates every non-integer value into the unknown bucket.
///
/// Args:
///     column: Qualified report column expression.
///     available: Whether the source actually carries that column.
///
/// Returns:
///     Safe SELECT expression and optional GROUP BY suffix using exactly the same expression.
pub(crate) fn trusted_quote_group(column: &str, available: bool) -> (String, String) {
    if !available {
        return ("NULL".to_string(), String::new());
    }
    let quote = format!("CASE WHEN typeof({column}) = 'integer' THEN {column} END");
    let group_by = format!(" GROUP BY {quote}");
    (quote, group_by)
}

/// One known quote currency decoded from a persisted report ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QuoteCurrency(u8);

impl QuoteCurrency {
    /// Decode a raw SQLite report value into a known quote currency.
    ///
    /// Args:
    ///     value: Raw `basecurrency` cell from a report row.
    ///
    /// Returns:
    ///     A known currency only when the cell is an integer trusted ordinal.
    pub fn from_report_value(value: &Value) -> Option<Self> {
        report_ordinal_from_value(value).and_then(Self::from_report_ordinal)
    }

    /// Decode a persisted MoonBot `TBaseCurrency` ordinal.
    ///
    /// Args:
    ///     ordinal: Integer stored in the report row's `basecurrency` column.
    ///
    /// Returns:
    ///     A known currency, or `None` for placeholders, empty/unknown sentinels,
    ///     negative values, and future ordinals.
    pub const fn from_report_ordinal(ordinal: i64) -> Option<Self> {
        match ordinal {
            0..=20 => Some(Self(ordinal as u8)),
            _ => None,
        }
    }

    /// Stable display ticker for this quote currency.
    ///
    /// Returns:
    ///     The persisted currency's neutral uppercase ticker.
    pub const fn ticker(self) -> &'static str {
        match self.0 {
            0 => "BTC",
            1 => "USDT",
            2 => "ETH",
            3 => "BNB",
            4 => "AUD",
            5 => "TUSD",
            6 => "BRL",
            7 => "USDH",
            8 => "USDC",
            9 => "FDUSD",
            10 => "AEUR",
            11 => "USD",
            12 => "TRX",
            13 => "RUB",
            14 => "EUR",
            15 => "HTX",
            16 => "USDD",
            17 => "IDR",
            18 => "DOGE",
            19 => "TRY",
            20 => "USDE",
            _ => "UNKNOWN",
        }
    }

    /// Decimal precision suitable for compact monetary display.
    ///
    /// Returns:
    ///     Eight places for crypto quote assets and two for fiat or stable quotes.
    pub const fn display_decimals(self) -> usize {
        match self.0 {
            0 | 2 | 3 | 12 | 15 | 18 => 8,
            _ => 2,
        }
    }
}

/// One exact known-currency total and the rows contributing to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuoteTotal {
    /// Currency shared by every contributing row.
    pub currency: QuoteCurrency,
    /// Sum of raw `profitbtc` values in `currency`.
    pub profit: f64,
    /// Number of contributing rows.
    pub orders: i64,
}

/// Safe raw-money totals split by quote currency.
///
/// Unknown rows retain only their count. Their amounts are deliberately not
/// exposed because those rows may contain several incomparable currencies.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuoteBreakdown {
    /// Known totals sorted by persisted currency ordinal.
    pub totals: Vec<QuoteTotal>,
    /// Rows whose currency is absent, invalid, placeholder, or unknown.
    pub unknown_orders: i64,
    /// Complete row count, including unknown-currency rows.
    pub orders: i64,
}

impl QuoteBreakdown {
    /// Build a breakdown from grouped `(ordinal, profit, orders)` inputs.
    ///
    /// Args:
    ///     groups: Source aggregates. `None` represents NULL or a missing column.
    ///
    /// Returns:
    ///     Known quote buckets plus unknown and complete row counts.
    pub fn from_groups(groups: impl IntoIterator<Item = (Option<i64>, f64, i64)>) -> Self {
        let mut known: BTreeMap<QuoteCurrency, (f64, i64)> = BTreeMap::new();
        let mut out = Self::default();
        for (ordinal, profit, orders) in groups {
            out.orders += orders;
            match ordinal.and_then(QuoteCurrency::from_report_ordinal) {
                Some(currency) => {
                    let bucket = known.entry(currency).or_default();
                    bucket.0 += profit;
                    bucket.1 += orders;
                }
                None => out.unknown_orders += orders,
            }
        }
        out.totals = known
            .into_iter()
            .map(|(currency, (profit, orders))| QuoteTotal {
                currency,
                profit,
                orders,
            })
            .collect();
        out
    }

    /// Classify whether raw-money values are comparable as one scalar.
    ///
    /// Returns:
    ///     Empty, one exact currency, mixed known currencies, or unknown identity.
    pub fn scope(&self) -> QuoteScope {
        if self.orders == 0 {
            QuoteScope::Empty
        } else if self.unknown_orders > 0 {
            QuoteScope::Unknown
        } else if self.totals.len() == 1 {
            QuoteScope::Single(self.totals[0].currency)
        } else {
            QuoteScope::Mixed
        }
    }
}

/// Comparability of raw quote-denominated money in one report scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuoteScope {
    /// No rows exist, so there is no unit to label and no invalid sum.
    #[default]
    Empty,
    /// Every row has one known quote currency.
    Single(QuoteCurrency),
    /// Rows use more than one known quote currency.
    Mixed,
    /// At least one row has no trustworthy quote identity.
    Unknown,
}

/// Unit carried by a comparable Analytics payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfitUnit {
    /// Per-trade return on spent capital.
    Percent,
    /// Raw money in one exact quote currency.
    Quote(QuoteCurrency),
}

/// Type-level boundary between comparable analytics and split raw totals.
#[derive(Clone, Debug)]
pub enum ProfitScope<T> {
    /// Scalar data whose values share one explicit unit.
    Comparable { unit: ProfitUnit, data: T },
    /// A legitimate empty query with no currency to infer.
    Empty(T),
    /// Raw money cannot be compared; only safe split/count totals are present.
    Split(QuoteBreakdown),
}

impl<T> ProfitScope<T> {
    /// Borrow comparable or empty data, excluding split-only raw scopes.
    ///
    /// Returns:
    ///     The scalar payload, or `None` when only split totals are safe.
    pub fn data(&self) -> Option<&T> {
        match self {
            Self::Comparable { data, .. } | Self::Empty(data) => Some(data),
            Self::Split(_) => None,
        }
    }

    /// Borrow split totals when raw-money comparison is unavailable.
    ///
    /// Returns:
    ///     Split quote totals, or `None` for comparable and empty scopes.
    pub fn split(&self) -> Option<&QuoteBreakdown> {
        match self {
            Self::Split(totals) => Some(totals),
            Self::Comparable { .. } | Self::Empty(_) => None,
        }
    }
}

#[cfg(test)]
mod tests;
