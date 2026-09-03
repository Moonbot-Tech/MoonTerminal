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

pub(in crate::db) mod coin_m;

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
    /// Build the positive predicate proving this rule's explicitly excluded direct market.
    ///
    /// Args:
    ///     alias: Report source alias the row is selected through.
    ///     columns: Columns the source actually carries.
    ///
    /// Returns:
    ///     An OR of the rule-owned veto markers, or `None` when the row cannot carry that proof.
    fn excluded_market_sql(
        &self,
        alias: &str,
        columns: &std::collections::HashSet<String>,
    ) -> Option<String> {
        (columns.contains("fname") && !self.excluded_markers.is_empty()).then(|| {
            self.excluded_markers
                .iter()
                .map(|marker| format!("{alias}.fname LIKE '{marker}'"))
                .collect::<Vec<_>>()
                .join(" OR ")
        })
    }

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
        let vetoes = self
            .excluded_market_sql(alias, columns)
            .map(|matches| format!("NOT ({matches})"))
            .unwrap_or_else(|| "1".to_string());
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

/// Learn which cores own COIN-M rows, from the sources a reader just opened.
///
/// Called where a connection exists, so the SQL builders below stay pure functions of the columns
/// they are given. The evidence is a scan over `fname` that no index covers, and this sits on the
/// discovery path of EVERY money query — so [`coin_m`] pays it per core rather than per call, and
/// asks the replica nothing at all once every core present has been examined.
///
/// Args:
///     conn: Open report reader.
///     sources: Physical report sources with their discovered columns.
pub(in crate::db) fn learn_coin_m_cores(
    conn: &rusqlite::Connection,
    sources: &[super::ReadSource],
) {
    coin_m::learn(conn, sources, |src| {
        DENOMINATION_RULES
            .iter()
            .filter_map(|rule| rule.guard_sql("d", &src.cols))
            .collect()
    });
}

/// Instant separating the two ways the core has written a COIN-M liquidation.
///
/// Measured on the live replica: the last row of the old shape closed 2023-08-31 14:33, the first
/// of the new one 2024-03-29 08:13, and no liquidation exists in the 211 days between them. The
/// boundary is therefore placed inside that empty gap, where no row can be misclassified by it.
const LIQUIDATION_ERA_SWITCH: i64 = 1_704_067_200; // 2024-01-01 00:00 UTC

/// Recognize a COIN-M liquidation, which the core books without a market name.
///
/// Every one of the 72 liquidations on the three COIN-M cores carries an EMPTY `fname` — the
/// column [`DENOMINATION_RULES`] keys on — so none of them is reachable by that rule, and they
/// keep the USDT label the core wrote. Two independent facts identify them instead: the sell
/// reason, and the dated/perpetual contract shape in `coin`. Measured: that pair selects 72 rows,
/// all on the three COIN-M cores, and NOT ONE row anywhere else in the replica.
///
/// Args:
///     alias: Report source alias the row is selected through.
///     columns: Columns the source actually carries.
///
/// Returns:
///     A predicate over existing columns, or `None` when the source cannot evidence it.
fn coin_m_liquidation_guard(
    alias: &str,
    columns: &std::collections::HashSet<String>,
) -> Option<String> {
    if !columns.contains("sellreason")
        || !columns.contains("coin")
        || !columns.contains("fname")
        || !columns.contains("core_uid")
    {
        return None;
    }
    let shapes = DENOMINATION_RULES
        .iter()
        .flat_map(|rule| rule.contract_shapes)
        .map(|shape| format!("{alias}.coin GLOB '{shape}'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    // The shape and the missing name are NOT enough on their own. EVERY liquidation on every core
    // is nameless — measured: 232 of them on one Bybit core alone — and a USD-M core trades the
    // same dated contracts (106 live rows). Those two facts would therefore multiply an ordinary
    // USD-M loss by its entry price. The third fact is the CORE: only one whose other rows the
    // market rule already relabels can own a COIN-M liquidation.
    let cores = coin_m::cores();
    if cores.is_empty() {
        // Not probed yet, or no COIN-M core is connected. Correcting nothing is the safe answer:
        // it leaves the historical reading in place instead of rewriting a row on a guess.
        return None;
    }
    let core_list = cores
        .iter()
        .map(|core| core.to_string())
        .collect::<Vec<_>>()
        .join(",");
    Some(format!(
        "({alias}.sellreason = 'LIQUIDATION'
          AND COALESCE({alias}.fname, '') = ''
          AND ({shapes})
          AND {alias}.core_uid IN ({core_list}))"
    ))
}

/// Rewrite a COIN-M liquidation's money into the currency it is actually settled in.
///
/// The core has booked these rows two different ways, and NEITHER stores plain money:
///
/// - **Before 2024** the amount is the posted margin in USD: `boughtq * buyprice / lev` reproduces
///   it exactly on all 8 such rows. That already matches the USDT label the core wrote, so the
///   amount passes through untouched.
/// - **From 2024** the amount is the margin in BTC DIVIDED BY the coin's entry price — a quantity
///   in no currency at all, which is why it reads as dust (`0.00001826`). Multiplying by that same
///   price returns the margin in BTC, and [`effective_ordinal_expr`] labels the row BTC to match.
///   Verified on 61 of the 64 rows against `boughtq * contract / lev` at the period's BTC rate.
///
/// The contract size cancels out of the correction: it appears on both sides of the identity that
/// established the era, so restoring the amount needs only the row's own price. Nothing here
/// assumes $10 or $100.
///
/// Left alone, era two is catastrophic in both directions: the terminal renders 33 750 USD of real
/// liquidations as 19 cents, and MoonBot renders one 129-dollar era-one row as -8 261 862.
///
/// Args:
///     alias: Report source alias the row is selected through.
///     columns: Columns the source actually carries.
///     column: Money column to read (`profitbtc` or `spentbtc`).
///
/// Returns:
///     SQL yielding the settled amount, or the plain column when the source cannot evidence a
///     liquidation.
pub(crate) fn settled_amount_expr(
    alias: &str,
    columns: &std::collections::HashSet<String>,
    column: &str,
) -> String {
    let plain = format!("{alias}.\"{column}\"");
    if !columns.contains(column) || !columns.contains("buyprice") || !columns.contains("closedate")
    {
        return plain;
    }
    let Some(guard) = coin_m_liquidation_guard(alias, columns) else {
        return plain;
    };
    // A non-positive price cannot restore anything, so such a row keeps its stored amount rather
    // than collapsing to zero and silently deleting a loss.
    format!(
        "(CASE WHEN {guard}
                AND {alias}.closedate >= {LIQUIDATION_ERA_SWITCH}
                AND {alias}.buyprice > 0
           THEN {plain} * {alias}.buyprice
           ELSE {plain} END)"
    )
}

/// Build a predicate saying whether a row's PRICES are denominated in the same currency as its
/// MONEY.
///
/// They usually are, and then a notional can be rebuilt as quantity × price. On the inverse
/// contracts [`DENOMINATION_RULES`] corrects, they are not: the market quotes in USD, `boughtq`
/// counts contracts, and the money columns settle in the base coin — so quantity × price is
/// neither the notional nor even the right currency. Being relabeled by a rule IS that signal, so
/// this compares the persisted label against the effective ordinal.
///
/// It lives here because the comparison needs the RAW column, which only this module may name; a
/// caller reaching for `basecurrency` itself would bypass every correction the module applies.
///
/// Args:
///     alias: Table alias carrying the report row.
///     columns: Discovered physical source columns.
///
/// Returns:
///     SQL predicate; the constant `1` for a source without a quote column, where nothing can have
///     been relabeled. On a core already proven to own a denomination mismatch, a still-raw
///     labeled quote is accepted only when the row carries the rule's explicit direct-market veto;
///     missing market facts therefore fail closed without rejecting a proven ordinary USDT row.
pub(crate) fn prices_share_money_quote_expr(
    alias: &str,
    columns: &std::collections::HashSet<String>,
) -> String {
    if !columns.contains("basecurrency") {
        return "1".to_string();
    }
    // `IS` rather than `=`, so a NULL on either side compares as a value: an unknown quote was not
    // relabeled either, and `=` would yield NULL and silently drop the row from a `WHEN` arm.
    let labels_match = format!(
        "COALESCE(({alias}.basecurrency) IS ({}), 0)",
        effective_ordinal_expr(alias, columns)
    );
    let known_mismatched = coin_m::cores();
    if !columns.contains("core_uid") || known_mismatched.is_empty() {
        return labels_match;
    }
    let cores = known_mismatched
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let ambiguous_labels = DENOMINATION_RULES
        .iter()
        .map(|rule| {
            let direct = rule
                .excluded_market_sql(alias, columns)
                .unwrap_or_else(|| "0".to_string());
            format!(
                "(typeof({alias}.basecurrency)='integer'
                  AND {alias}.basecurrency={}
                  AND NOT COALESCE(({direct}), 0))",
                rule.labeled.ordinal()
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "({labels_match}) AND NOT (
            typeof({alias}.core_uid)='integer' AND {alias}.core_uid IN ({cores})
            AND ({ambiguous_labels})
         )"
    )
}

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
    let mut arms = DENOMINATION_RULES
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
    // A COIN-M liquidation carries no market name, so no rule above can reach it. From 2024 its
    // amount settles in BTC once `settled_amount_expr` restores it, and the label must follow the
    // same era boundary — a row whose money is corrected but whose quote is not would be valued at
    // the BTC amount with the USDT rate. Era one keeps the USDT label, which its USD margin fits.
    // Same three facts, same era boundary and the same column guards as `settled_amount_expr`:
    // a row whose money is corrected but whose quote is not would be valued at the BTC amount with
    // the USDT rate. The persisted label must still be the one the rule expects, so a deliberately
    // USDC- or ETH-labeled row is never silently reclassified.
    if columns.contains("closedate") && columns.contains("buyprice") {
        if let Some(guard) = coin_m_liquidation_guard(alias, columns) {
            arms.push_str(&format!(
                " WHEN ({trusted}) = {labeled} AND {guard}
                   AND {alias}.closedate >= {LIQUIDATION_ERA_SWITCH} AND {alias}.buyprice > 0
                   THEN {btc}",
                labeled = QuoteCurrency::usdt().ordinal(),
                btc = QuoteCurrency::btc().ordinal(),
            ));
        }
    }
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
    /// Iterate every persisted quote currency in stable ordinal order.
    ///
    /// Returns:
    ///     The complete known quote universe used by storage and conversion routing.
    pub fn all() -> impl ExactSizeIterator<Item = Self> {
        (0_u8..=20).map(Self)
    }

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

/// One known-currency spend/profit subtotal over the CLOSED, non-Funding, positively-spent rows
/// that [`QuoteBreakdown::average_order_return`] counts.
///
/// A separate carrier from [`QuoteTotal`] rather than a widened field on it: `QuoteTotal` is
/// shared by [`QuoteBreakdown::from_groups`] and [`OpenPositions::from_groups`] through
/// `group_quotes`, so adding a spend leg there would drag a permanently-empty field through the
/// open pass and Analytics, which never accounts a spend this way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuoteSpend {
    /// Currency shared by every contributing row.
    pub currency: QuoteCurrency,
    /// Sum of settled `spentbtc` over the counted rows of this currency.
    pub spent: f64,
    /// Sum of settled `profitbtc` over the SAME counted rows.
    pub profit: f64,
    /// Counted rows contributing to both sums.
    pub orders: i64,
}

/// Per-quote spend/profit subtotals feeding [`QuoteBreakdown::average_order_return`], carrying its
/// OWN unified USDT leg rather than reading [`UsdtTotal::spent`].
///
/// [`UsdtTotal::spent`] rests on [`super::valuation::SourcePredicates::spent_value`], which tests
/// only that `spentbtc` is numeric — no positive-spend guard, no Funding exclusion. Averaging over
/// it would count a WIDER row set than the native arm while still reporting a complete `counted`,
/// a dishonest denominator. [`TradedVolume`] already carries its own `usdt` leg for the identical
/// reason; this follows that precedent instead of borrowing the valuation cache's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntrySpend {
    /// Per known quote, over COUNTED rows only, sorted by persisted currency ordinal.
    pub totals: Vec<QuoteSpend>,
    /// Every counted row in the scope, including rows whose quote identity is unknown.
    pub counted_orders: i64,
    /// Counted rows carrying an active-mode USDT rate.
    pub valued_orders: i64,
    /// Sigma spent converted at the active-mode rate, over valued counted rows.
    pub usdt_spent: f64,
    /// Sigma profit converted at the same rate, over the same rows.
    pub usdt_profit: f64,
}

impl EntrySpend {
    /// Build per-quote spend/profit subtotals, and the unified USDT leg, from physical-source
    /// quote groups.
    ///
    /// Args:
    ///     groups: `(ordinal, counted, Sigma spent, Sigma profit, valued, Sigma USDT spent, Sigma
    ///         USDT profit)` aggregates, one row per physical source and quote group.
    ///
    /// Returns:
    ///     Ordinal-sorted known buckets plus the scope-wide counted/valued/USDT tallies. A KNOWN
    ///     ordinal folds into a [`QuoteSpend`] bucket; an UNKNOWN one still contributes to
    ///     [`Self::counted_orders`] but to no bucket. Such a row can carry no per-quote rate, so it
    ///     can never be valued either, and [`Self::unified`] fails closed on it by construction.
    pub(crate) fn from_groups(
        groups: impl IntoIterator<Item = (Option<i64>, i64, f64, f64, i64, f64, f64)>,
    ) -> Self {
        let mut known: BTreeMap<QuoteCurrency, (i64, f64, f64)> = BTreeMap::new();
        let mut out = Self::default();
        for (ordinal, counted, spent, profit, valued, usdt_spent, usdt_profit) in groups {
            if counted == 0 {
                continue;
            }
            out.counted_orders += counted;
            out.valued_orders += valued;
            out.usdt_spent += usdt_spent;
            out.usdt_profit += usdt_profit;
            if let Some(currency) = ordinal.and_then(QuoteCurrency::from_report_ordinal) {
                let bucket = known.entry(currency).or_default();
                bucket.0 += counted;
                bucket.1 += spent;
                bucket.2 += profit;
            }
        }
        out.totals = known
            .into_iter()
            .map(|(currency, (orders, spent, profit))| QuoteSpend {
                currency,
                spent,
                profit,
                orders,
            })
            .collect();
        out
    }

    /// The unified USDT pair, available ONLY over a completely valued counted scope.
    ///
    /// Mirrors [`TradedVolume::from_groups`]'s completeness rule: a partial unified figure is
    /// never published, because a mixed scope has no second bucket to fall back on and a partial
    /// sum would read exactly like a complete one.
    ///
    /// Returns:
    ///     `(Sigma spent, Sigma profit)` in USDT, or `None` for an empty or incompletely-valued
    ///     scope.
    pub fn unified(&self) -> Option<(f64, f64)> {
        (self.counted_orders > 0 && self.valued_orders == self.counted_orders)
            .then_some((self.usdt_spent, self.usdt_profit))
    }
}

/// Realized profit stated as a percentage of the average order over a single-core Report scope.
///
/// Denominated the same way the head footer figure is: a native currency where [`QuoteBreakdown`]
/// carries exactly one, or a unified USDT total where the scope is mixed, the head figure is
/// itself complete ([`QuoteBreakdown::unified_usdt`] is `Some`), and [`EntrySpend::unified`] is
/// complete over the COUNTED rows. The unified arm can still be PARTIAL — [`Self::excluded`] can
/// be positive even there, because Funding, non-positive-spend, and unknown-quote rows shrink the
/// counted scope below [`QuoteBreakdown::orders`] independently of whether every counted row was
/// valued.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AverageOrderReturn {
    /// `100 * Sigma profit / avg_order`, over the counted rows.
    pub pct: f64,
    /// `Sigma spent / counted`, denominated in [`Self::currency`].
    pub avg_order: f64,
    /// Currency both sums are denominated in.
    pub currency: QuoteCurrency,
    /// Rows contributing to both sums.
    pub counted: i64,
    /// Rows this scope's total row count could not account for, derived as `orders - counted`,
    /// never carried as a second field that could disagree with the first.
    pub excluded: i64,
    /// Whether [`Self::pct`] and [`Self::avg_order`] are the unified USDT conversion rather than a
    /// native currency.
    pub unified: bool,
}

/// Whether an average order size is a figure worth dividing a profit by.
///
/// A positive test alone is NOT enough, and the gap is not theoretical: replica ingestion stores
/// an unvalidated float straight into SQLite as `REAL`, and a `SUM` over a large scope can
/// overflow, so `spent` can arrive as positive infinity. Infinity passes any `> 0.0` check, and
/// the percentage taken from it is `100 * profit / inf` — a FINITE zero. The footer would then
/// state a confident `+0.0%` beside an average rendered `inf`, and the trailing `is_finite` check
/// on the ratio cannot see it, because by then the ratio looks perfectly ordinary. Rejecting the
/// AVERAGE is the only place that catches it.
///
/// Args:
///     avg_order: Candidate average order size.
///
/// Returns:
///     Whether the value is finite and strictly positive. NaN fails both halves.
fn usable_average(avg_order: f64) -> bool {
    avg_order.is_finite() && avg_order > 0.0
}

/// One known-currency traded-volume bucket over an exact Report scope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuoteVolume {
    /// Currency shared by every eligible trade in this bucket.
    pub currency: QuoteCurrency,
    /// Unsigned entry-plus-exit notional over the RECONSTRUCTED trades of this bucket only.
    ///
    /// Never a withheld figure: the physical query already sums nothing but reconstructed rows, so
    /// this is a dimensionally sound subtotal whatever the rest of the bucket did. It is the WHOLE
    /// bucket only while [`Self::reconstructed`] equals [`Self::orders`]; a reader that presents it
    /// without checking that pair states a partial sum as if it were the complete filter total.
    pub amount: f64,
    /// Closed non-Funding trades assigned to this currency.
    pub orders: i64,
    /// Trades of this bucket whose two price legs reconstructed in native money.
    ///
    /// Kept beside [`Self::orders`] rather than collapsed into an optional amount, because one
    /// unprovable row out of a thousand used to blank a bucket entirely — and a single-currency
    /// scope has no second bucket to fall back on.
    pub reconstructed: i64,
}

/// Two-sided traded volume for one exact Report filter, complete or partial.
///
/// Only [`Self::usdt`] is complete-only. The native buckets always carry their money and state
/// their own completeness through [`QuoteVolume::reconstructed`], because a single-currency scope
/// has no second bucket to fall back on: withholding the amount there left the footer with nothing
/// to say over one unprovable row in a thousand.
///
/// This carrier is deliberately independent of [`ValuationCoverage`]: open and Funding rows are
/// valid Report/profit rows but are not eligible volume rows, so profit coverage cannot decide
/// whether a volume total is complete.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradedVolume {
    /// Known quote buckets sorted by persisted currency ordinal.
    pub totals: Vec<QuoteVolume>,
    /// Eligible trades whose quote identity is unknown.
    pub unknown_orders: i64,
    /// Closed non-Funding trades in the exact Report scope.
    pub eligible_orders: i64,
    /// Eligible trades whose two price legs can be reconstructed in native money.
    pub reconstructed_orders: i64,
    /// Reconstructed trades carrying an active-mode USDT rate.
    pub valued_orders: i64,
    /// Unified unsigned USDT notional, available only for a completely valued known scope.
    pub usdt: Option<f64>,
}

impl TradedVolume {
    /// Build per-quote volume subtotals from physical-source quote groups.
    ///
    /// Args:
    ///     groups: `(ordinal, eligible, reconstructed, native sum, valued, USDT sum)` aggregates.
    ///
    /// Returns:
    ///     Per-quote native subtotals with their own reconstruction counts, plus independently
    ///     complete unified USDT coverage.
    pub(crate) fn from_groups(
        groups: impl IntoIterator<Item = (Option<i64>, i64, i64, f64, i64, f64)>,
    ) -> Self {
        let mut known: BTreeMap<QuoteCurrency, (i64, i64, f64)> = BTreeMap::new();
        let mut out = Self::default();
        let mut usdt_sum = 0.0;
        for (ordinal, eligible, reconstructed, native, valued, usdt) in groups {
            if eligible == 0 {
                continue;
            }
            out.eligible_orders += eligible;
            out.reconstructed_orders += reconstructed;
            out.valued_orders += valued;
            usdt_sum += usdt;
            match ordinal.and_then(QuoteCurrency::from_report_ordinal) {
                Some(currency) => {
                    let bucket = known.entry(currency).or_default();
                    bucket.0 += eligible;
                    bucket.1 += reconstructed;
                    bucket.2 += native;
                }
                None => out.unknown_orders += eligible,
            }
        }
        out.totals = known
            .into_iter()
            .map(|(currency, (orders, reconstructed, native))| QuoteVolume {
                currency,
                amount: native,
                orders,
                reconstructed,
            })
            .collect();
        out.usdt = (out.eligible_orders > 0
            && out.unknown_orders == 0
            && out.reconstructed_orders == out.eligible_orders
            && out.valued_orders == out.eligible_orders)
            .then_some(usdt_sum);
        out
    }

    /// Classify whether native volume is comparable as one scalar.
    ///
    /// Returns:
    ///     Empty, one exact currency, mixed known currencies, or unknown identity.
    pub fn scope(&self) -> QuoteScope {
        if self.eligible_orders == 0 {
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

/// Historical valuation progress for one exact report filter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ValuationCoverage {
    /// Rows with a known persisted quote currency.
    pub eligible_orders: i64,
    /// Eligible rows whose current inputs have a matching prepared valuation.
    pub valued_orders: i64,
    /// Eligible rows proven unroutable by the active valuation mode.
    pub unavailable_orders: i64,
    /// Complete USDT aggregate; never contains a partial sum.
    pub usdt: Option<UsdtTotal>,
}

/// Bucket `(ordinal, profit, orders)` aggregates by known quote currency.
///
/// The single grouping authority behind both [`QuoteBreakdown::from_groups`] and
/// [`OpenPositions::from_groups`]: realized and unrealized money are different facts, but the way
/// rows fold into currencies is the same one, and two copies of it would eventually disagree about
/// what an unknown ordinal means.
///
/// Args:
///     groups: Source aggregates. `None` represents NULL or a missing column.
///
/// Returns:
///     Ordinal-sorted known buckets, the unknown-currency row count, and the complete row count.
fn group_quotes(
    groups: impl IntoIterator<Item = (Option<i64>, f64, i64)>,
) -> (Vec<QuoteTotal>, i64, i64) {
    let mut known: BTreeMap<QuoteCurrency, (f64, i64)> = BTreeMap::new();
    let mut unknown_orders = 0;
    let mut all_orders = 0;
    for (ordinal, profit, orders) in groups {
        all_orders += orders;
        match ordinal.and_then(QuoteCurrency::from_report_ordinal) {
            Some(currency) => {
                let bucket = known.entry(currency).or_default();
                bucket.0 += profit;
                bucket.1 += orders;
            }
            None => unknown_orders += orders,
        }
    }
    let totals = known
        .into_iter()
        .map(|(currency, (profit, orders))| QuoteTotal {
            currency,
            profit,
            orders,
        })
        .collect();
    (totals, unknown_orders, all_orders)
}

/// Still-running positions and the money they are showing right now.
///
/// Every figure here is UNREALIZED: it moves with the market and becomes a fact only when the
/// position closes. It is a separate type from [`QuoteBreakdown`] for exactly that reason — the
/// two must never be summed — and the surfaces that state it are required to say so rather than
/// letting it sit beside realized profit in the same visual weight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenPositions {
    /// Floating money per known currency.
    pub totals: Vec<QuoteTotal>,
    /// Open rows whose currency is absent, invalid, placeholder, or unknown.
    pub unknown_orders: i64,
    /// Complete open-row count, including unknown-currency rows.
    pub orders: i64,
}

impl OpenPositions {
    /// Build a floating tally from grouped `(ordinal, profit, orders)` inputs.
    ///
    /// Args:
    ///     groups: Source aggregates over OPEN rows only. `None` represents NULL or a missing
    ///         column.
    ///
    /// Returns:
    ///     Known quote buckets plus unknown and complete open-row counts.
    pub fn from_groups(groups: impl IntoIterator<Item = (Option<i64>, f64, i64)>) -> Self {
        let (totals, unknown_orders, orders) = group_quotes(groups);
        Self {
            totals,
            unknown_orders,
            orders,
        }
    }
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
    /// Two-sided volume computed over this same filter and snapshot; [`TradedVolume`] states its
    /// own completeness rather than withholding a native amount.
    pub traded_volume: TradedVolume,
    /// Per-quote spend/profit subtotals over the counted rows of this same filter and snapshot,
    /// feeding [`Self::average_order_return`].
    pub entry_spend: EntrySpend,
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
        let (totals, unknown_orders, orders) = group_quotes(groups);
        Self {
            totals,
            unknown_orders,
            orders,
            ..Self::default()
        }
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

    /// Attach two-sided traded volume computed over the same filter and read snapshot.
    ///
    /// Args:
    ///     volume: Independent closed non-Funding native and active-mode valuation totals.
    ///
    /// Returns:
    ///     This profit breakdown carrying the supplied traded volume.
    pub(crate) fn with_traded_volume(mut self, volume: TradedVolume) -> Self {
        self.traded_volume = volume;
        self
    }

    /// Attach entry-spend subtotals computed over the same filter and read snapshot.
    ///
    /// Args:
    ///     spend: Per-quote counted spend/profit subtotals feeding [`Self::average_order_return`].
    ///
    /// Returns:
    ///     This profit breakdown carrying the supplied entry spend.
    pub(crate) fn with_entry_spend(mut self, spend: EntrySpend) -> Self {
        self.entry_spend = spend;
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

    /// State realized profit as a percentage of the average order over the counted rows, in
    /// exactly the currency [`Self::scope`] would promote as the head figure.
    ///
    /// Driven by [`Self::scope`], [`Self::unified_usdt`] and [`EntrySpend::unified`] so the ratio
    /// can never be denominated differently from the figure it qualifies. `u` below is
    /// `self.unified_usdt()`, and `e` is `self.entry_spend`:
    ///
    /// ```text
    /// scope()      condition                                     pct arm
    /// Empty        —                                              absent
    /// Single(c)    e.totals has bucket c with orders > 0          native
    /// Single(c)    otherwise                                      absent
    /// Mixed        u.is_some() AND e.unified() == Some((s, p))    unified, over USDT
    /// Mixed        otherwise                                      absent
    /// Unknown      exactly one bucket in SELF.totals with orders>0 native (excluded incl. unknowns)
    /// Unknown      otherwise                                      absent
    /// ```
    ///
    /// The `Unknown` row keys on `self.totals` — the PROFIT buckets — and not on `e.totals`, and
    /// the difference is load-bearing rather than incidental: `self.totals` is what
    /// `footer_facts` promotes into the never-clipped head, so keying on it is what guarantees
    /// the percentage is denominated in the currency the row's own money is stated in. Reading
    /// `e.totals` here instead would let the two disagree the moment a bucket has profit rows but
    /// no COUNTED ones, or the reverse.
    ///
    /// The unified arm never reads [`UsdtTotal::spent`] — see [`EntrySpend`]'s own doc for why —
    /// and it CAN still be partial: [`AverageOrderReturn::excluded`] can be positive there too,
    /// because Funding, non-positive-spend, and unknown-quote rows shrink the counted scope below
    /// [`Self::orders`] independently of whether every counted row was valued.
    ///
    /// The house average-order definition (`analytics::groups`, `analytics::profit_monitor`) is
    /// over rows with a POSITIVE numeric settled spend; the SQL leg that fills
    /// [`Self::entry_spend`] already applies that filter, together with the Funding exclusion the
    /// traded-volume leg uses. `excluded` is always `Self::orders - counted`, so it aggregates
    /// unknown-quote rows, non-numeric/non-positive spend, non-numeric profit, and Funding rows
    /// without a second field that could disagree with the first.
    ///
    /// Worked example: profit 3496.52 over 5 rows with spend
    /// 28742+34160+29884+28717+28717 = 150220 -> `avg_order == 30044.0`,
    /// `pct == 100.0 * 3496.52 / 30044.0` (rounds to `+11.6%`), `counted == 5`, `excluded == 0`.
    ///
    /// Returns:
    ///     `None` for an empty, mixed-incomplete, mixed-unknown, or multi/zero-known unknown
    ///     scope, for zero counted rows, or for a non-finite ratio.
    pub fn average_order_return(&self) -> Option<AverageOrderReturn> {
        match self.scope() {
            QuoteScope::Empty => None,
            QuoteScope::Single(currency) => self.native_average_order_return(currency),
            QuoteScope::Mixed => {
                self.unified_usdt()?;
                let (spent, profit) = self.entry_spend.unified()?;
                // `unified()` returns `Some` only over a positive counted scope, so the division
                // below cannot divide by zero and needs no guard of its own here.
                let counted = self.entry_spend.counted_orders;
                let avg_order = spent / counted as f64;
                if !usable_average(avg_order) {
                    return None;
                }
                let pct = 100.0 * profit / avg_order;
                pct.is_finite().then_some(AverageOrderReturn {
                    pct,
                    avg_order,
                    currency: QuoteCurrency::usdt(),
                    counted,
                    excluded: self.orders - counted,
                    unified: true,
                })
            }
            QuoteScope::Unknown => {
                if self.totals.len() != 1 {
                    return None;
                }
                self.native_average_order_return(self.totals[0].currency)
            }
        }
    }

    /// Build the native-currency arm of [`Self::average_order_return`] for one exact currency.
    ///
    /// Args:
    ///     currency: Currency [`Self::scope`] promoted as the head figure.
    ///
    /// Returns:
    ///     `None` when no counted bucket exists for `currency`, when its spend sums to zero or
    ///     less, or when the resulting ratio is non-finite.
    fn native_average_order_return(&self, currency: QuoteCurrency) -> Option<AverageOrderReturn> {
        let bucket = self
            .entry_spend
            .totals
            .iter()
            .find(|bucket| bucket.currency == currency)?;
        if bucket.orders <= 0 {
            return None;
        }
        let avg_order = bucket.spent / bucket.orders as f64;
        if !usable_average(avg_order) {
            return None;
        }
        let pct = 100.0 * bucket.profit / avg_order;
        pct.is_finite().then_some(AverageOrderReturn {
            pct,
            avg_order,
            currency,
            counted: bucket.orders,
            excluded: self.orders - bucket.orders,
            unified: false,
        })
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
