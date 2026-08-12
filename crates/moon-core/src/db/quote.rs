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

use crate::util::fmt::{self, DeltaSign};

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

/// One market family whose report money is denominated in a currency the core does not label it
/// with.
///
/// A rule fires on THREE facts at once — the market spelling in `fname`, the exact ordinal the core
/// wrote, and nothing else — so it can only ever move rows it was written for. Adding a venue with
/// the same habit is one entry in [`DENOMINATION_RULES`]; no SQL builder changes with it.
struct DenominationRule {
    /// SQL `LIKE` patterns for the market spelling in `fname`, any one of which may match.
    ///
    /// Deliberately NOT anchored to the `_` that separates `fname`'s `<source>_<market>_<stamp>`
    /// segments: measured on the replica, 1 476 COIN-M rows spell the market with its last letter
    /// rotated to the front — `Pump_TUSD-DO_0329_…` for market `USD-DOT_0329`, `Pump_BUSD-BN_0630_…`
    /// for `USD-BNB_0630` — and an anchor drops every one of them. [`Self::excluded_markers`] and
    /// the contract shape carry the precision the anchor would have.
    market_markers: &'static [&'static str],
    /// SQL `LIKE` patterns that VETO the rule, whatever else matched.
    ///
    /// `fname`'s first segment is a user-named strategy, so a strategy called `USD-hedge` satisfies
    /// an unanchored marker on a USD-M core. Its market segment still spells the quote in full
    /// (`USDT-ETH_0927`), and a COIN-M row never does — including the rotated spellings, where
    /// `TUSD-` and `BUSD-` contain no `USDT-`. So this veto separates the one collision the
    /// contract shape cannot: a USD-M DATED contract, whose coin has the same shape.
    excluded_markers: &'static [&'static str],
    /// SQL `GLOB` patterns for the contract shape in `coin`, any one of which may match.
    ///
    /// The second independent fact. Neither alone is proof — a USD-M core trades the same
    /// `ETH_0926` shape, and a strategy name can contain anything — but a row moves only when the
    /// market spelling and the contract shape agree and no veto fires.
    contract_shapes: &'static [&'static str],
    /// Ordinal the core writes for such a row.
    labeled: QuoteCurrency,
    /// Ordinal the row's money is actually in.
    denominated: QuoteCurrency,
}

impl DenominationRule {
    /// Build this rule's guard over one source, or `None` when the source cannot evidence it.
    ///
    /// Args:
    ///     alias: Report source alias the row is selected through.
    ///     columns: Columns the source actually carries.
    ///
    /// Returns:
    ///     A predicate naming only columns the source has, or `None` to skip the rule entirely.
    fn guard_sql(
        &self,
        alias: &str,
        columns: &std::collections::HashSet<String>,
    ) -> Option<String> {
        // A source missing either fact cannot prove the rule, so every row keeps its persisted
        // identity — the answer this module gave before any rule existed.
        if !columns.contains("fname") || !columns.contains("coin") {
            return None;
        }
        // An empty list collapses to the neutral `1`, never to `()`: a rule that states no veto —
        // or no shape — is a legitimate rule, and joining nothing into parentheses would produce
        // SQL that fails to prepare, taking every money query down with it.
        let joined = |parts: Vec<String>, separator: &str| {
            if parts.is_empty() {
                "1".to_string()
            } else {
                format!("({})", parts.join(separator))
            }
        };
        let markers = joined(
            self.market_markers
                .iter()
                .map(|marker| format!("{alias}.fname LIKE '{marker}'"))
                .collect(),
            " OR ",
        );
        let vetoes = joined(
            self.excluded_markers
                .iter()
                .map(|marker| format!("{alias}.fname NOT LIKE '{marker}'"))
                .collect(),
            " AND ",
        );
        let shapes = joined(
            self.contract_shapes
                .iter()
                .map(|shape| format!("{alias}.coin GLOB '{shape}'"))
                .collect(),
            " OR ",
        );
        Some(format!("{markers} AND {vetoes} AND {shapes}"))
    }
}

/// Every known label-versus-denomination mismatch, applied in order.
///
/// Binance COIN-M (`QBinance`, the cores reporting `Binance Quarterly`) writes its markets as
/// `USD-<COIN>` — `Pump_USD-UNI_RP_…`, `BinanceQ_USD-ETH_0926_…` — while a USD-M core writes the
/// very same dated contract as `USDT-<COIN>` (`Pump_USDT-ETH_0927_…`). The `coin` column keeps only
/// the contract part, `ETH_0926` on both, so the spelling in `fname` is the ONE per-row fact that
/// separates them; measured on the live replica, the marker selects every row of the three COIN-M
/// cores and no row of any other.
///
/// Those rows quote in USD but settle in the base coin, and MoonBot normalizes the settled amount
/// to BTC before storing it: `notional / spentbtc` holds the BTC price of its period for every one
/// of them (median 65k in 2024, 106k in 2025, 67k in 2026), and MoonBot's own report converts them
/// with the BTC rate. Left uncorrected, a −0.00041380 BTC trade values as −0.0004 USDT instead of
/// −26.33.
///
/// Measured against the live replica: the three facts together select 13 381 rows — every row of
/// the three COIN-M cores that carries a filename, and not one row of the other twenty cores.
const DENOMINATION_RULES: &[DenominationRule] = &[DenominationRule {
    market_markers: &["%USD-%"],
    excluded_markers: &["%USDT-%"],
    contract_shapes: &["*_RP", "*_[0-9][0-9][0-9][0-9]"],
    labeled: QuoteCurrency::usdt(),
    denominated: QuoteCurrency::btc(),
}];

/// Build the EFFECTIVE quote ordinal of one report row.
///
/// THE one place a row's currency is decided. Two corrections ride on the persisted `basecurrency`,
/// and both must apply wherever that row's money is read, or two surfaces disagree about what one
/// number means:
///
/// * SQLite considers numeric `INTEGER 1` and malformed `REAL 1.0` equal for `GROUP BY`, so the
///   storage-class guard separates every non-integer value into the unknown bucket.
/// * A row whose market family denominates elsewhere than its label says is moved by
///   [`DENOMINATION_RULES`]. Only an exact labeled ordinal is rewritten, so a currency the core
///   named deliberately — USDC, ETH, a fiat quote — passes through whatever its market is called.
///
/// The correction reads the ROW, never the core's current configuration: a core can change venue,
/// and a historical row must keep the identity it was written with.
///
/// Args:
///     alias: Report source alias the row is selected through.
///     columns: Columns the source actually carries.
///
/// Returns:
///     SQL expression yielding a trusted ordinal, or NULL for a row whose identity is unknown.
pub(crate) fn effective_ordinal_expr(
    alias: &str,
    columns: &std::collections::HashSet<String>,
) -> String {
    if !columns.contains("basecurrency") {
        return "NULL".to_string();
    }
    let raw = format!("{alias}.basecurrency");
    let trusted = format!("CASE WHEN typeof({raw}) = 'integer' THEN {raw} END");
    let arms = DENOMINATION_RULES
        .iter()
        .filter_map(|rule| {
            let guard = rule.guard_sql(alias, columns)?;
            Some(format!(
                " WHEN ({trusted}) = {labeled} AND {guard} THEN {denominated}",
                labeled = rule.labeled.ordinal(),
                denominated = rule.denominated.ordinal(),
            ))
        })
        .collect::<String>();
    if arms.is_empty() {
        return trusted;
    }
    format!("CASE{arms} ELSE ({trusted}) END")
}

/// Build the trusted quote projection and grouping suffix for one report source.
///
/// Args:
///     alias: Report source alias the row is selected through.
///     columns: Columns the source actually carries.
///
/// Returns:
///     Safe SELECT expression and optional GROUP BY suffix using exactly the same expression.
pub(crate) fn trusted_quote_group(
    alias: &str,
    columns: &std::collections::HashSet<String>,
) -> (String, String) {
    if !columns.contains("basecurrency") {
        return ("NULL".to_string(), String::new());
    }
    let quote = effective_ordinal_expr(alias, columns);
    let group_by = format!(" GROUP BY {quote}");
    (quote, group_by)
}

/// One known quote currency decoded from a persisted report ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QuoteCurrency(u8);

impl QuoteCurrency {
    /// Exact USDT quote identity used for fully converted mixed scopes.
    ///
    /// Returns:
    ///     Persisted USDT currency identity.
    pub const fn usdt() -> Self {
        Self(1)
    }

    /// Exact BTC quote identity, which every COIN-M report row is denominated in.
    ///
    /// Returns:
    ///     Persisted BTC currency identity.
    pub const fn btc() -> Self {
        Self(0)
    }

    /// Persisted ordinal behind this identity.
    ///
    /// Exposed because the effective-quote SQL has to embed the ordinals as literals; nothing else
    /// should need to unwrap the identity back into a number.
    ///
    /// Returns:
    ///     The `TBaseCurrency` ordinal.
    pub const fn ordinal(self) -> u8 {
        self.0
    }

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

impl QuoteTotal {
    /// Signed compact amount followed by this bucket's exact ticker, plus the sign that text shows.
    ///
    /// One home for every surface that prints a quote total — the Report footer and the Analytics
    /// quote split both read it. Two copies of the precision rule drift, and then the same figure
    /// renders differently depending on which window the user happens to be looking at.
    ///
    /// Returns:
    ///     `"+12.5 USDT"` and its [`DeltaSign`], classified from the rounded amount.
    pub fn signed_display(self) -> (String, DeltaSign) {
        let (amount, sign) = fmt::signed_amount(self.profit, self.currency.display_decimals());
        (format!("{amount} {}", self.currency.ticker()), sign)
    }
}

/// Complete unified USDT aggregate available only after every eligible row is valued.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsdtTotal {
    /// Historical USDT profit over the complete known-currency scope.
    pub profit: f64,
    /// Historical USDT spend when every source row supplied a numeric spend.
    pub spent: Option<f64>,
}

/// Historical valuation progress for one exact report filter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ValuationCoverage {
    /// Rows with a known persisted quote currency.
    pub eligible_orders: i64,
    /// Eligible rows whose current inputs have a matching prepared valuation.
    pub valued_orders: i64,
    /// Eligible rows whose canonical direct/inverse routes are permanently absent.
    pub unavailable_orders: i64,
    /// Complete USDT aggregate; never contains a partial sum.
    pub usdt: Option<UsdtTotal>,
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
    /// Optional historical USDT coverage from the attached valuation cache.
    pub valuation: Option<ValuationCoverage>,
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

    /// Attach historical valuation coverage computed over the exact same read snapshot.
    ///
    /// Args:
    ///     coverage: Eligible, valued, unavailable, and complete-only USDT aggregate.
    ///
    /// Returns:
    ///     This native breakdown carrying the supplied coverage.
    pub fn with_valuation(mut self, coverage: ValuationCoverage) -> Self {
        self.valuation = Some(coverage);
        self
    }

    /// Return a complete unified USDT total only when no row has unknown quote identity.
    ///
    /// Returns:
    ///     Complete historical USDT money, or `None` for partial/unknown scopes.
    pub fn unified_usdt(&self) -> Option<UsdtTotal> {
        (self.unknown_orders == 0)
            .then_some(self.valuation.and_then(|coverage| coverage.usdt))
            .flatten()
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
