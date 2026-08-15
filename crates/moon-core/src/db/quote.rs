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

/// One known-currency traded-volume bucket over an exact Report scope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuoteVolume {
    /// Currency shared by every eligible trade in this bucket.
    pub currency: QuoteCurrency,
    /// Unsigned entry-plus-exit notional, withheld when any eligible row is unprovable.
    pub amount: Option<f64>,
    /// Closed non-Funding trades assigned to this currency.
    pub orders: i64,
}

/// Complete-only two-sided traded volume for one exact Report filter.
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
    /// Build complete-only volume from physical-source quote groups.
    ///
    /// Args:
    ///     groups: `(ordinal, eligible, reconstructed, native sum, valued, USDT sum)` aggregates.
    ///
    /// Returns:
    ///     Per-quote native buckets plus independently complete unified USDT coverage.
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
                amount: (orders == reconstructed).then_some(native),
                orders,
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
    /// Complete-only two-sided volume computed over this same filter and snapshot.
    pub traded_volume: TradedVolume,
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
