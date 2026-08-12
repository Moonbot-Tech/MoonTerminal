//! Current-rate valuation: an in-memory quote-to-USDT snapshot and the SQL that applies it.
//!
//! The historical path answers "what was this trade worth when it closed". This one answers "what
//! would the same quote amount be worth right now", which is a different question and never a
//! substitute: it is opt-in, it is labelled separately everywhere it renders, and it is **never
//! persisted**. A stored "current" rate is a stale rate wearing a fresh rate's label, and the
//! derived cache cannot hold it anyway — `ALGORITHM_VERSION` sits inside both primary keys and
//! every join predicate, so the two conversions could not share a row.
//!
//! Freshness is explicit rather than implied. A rate the worker fetched and could not refresh
//! stops counting as current after [`FRESHNESS_MS`], at which point its quote falls back to
//! uncovered and the total honestly splits by currency — the same degradation the historical path
//! already shows for a partially valued scope.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use super::{CoverageSql, PerRowSql};

/// How long a fetched rate keeps counting as "current".
///
/// Ten minutes survives a short provider outage without ever letting a half-hour-old number render
/// under a label that claims otherwise. The worker refreshes at half this interval, so a healthy
/// session never approaches the bound and a single failed pass cannot reach it; reaching it means
/// the provider has been unreachable across consecutive passes.
pub const FRESHNESS_MS: i64 = 600_000;

/// Which conversion a report or Analytics read applies to quote-denominated money.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ValuationMode {
    /// Convert each trade at the spot rate of the minute it closed.
    #[default]
    Historical,
    /// Convert every trade at the latest known spot rate.
    Current,
}

impl ValuationMode {
    /// Stable persistence and diagnostic code.
    ///
    /// Returns:
    ///     A machine identifier that never changes with translation.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Historical => "historical",
            Self::Current => "current",
        }
    }

    /// Pick the localization key naming this conversion, from a historical/current pair.
    ///
    /// The mapping lives here rather than as a `match` at each rendering site: a current-rate
    /// figure wearing the historical sentence is the one mislabeling this whole feature exists to
    /// avoid, and five copies of the choice is five places to miss when a key is renamed.
    ///
    /// Args:
    ///     historical: Key for the at-trade-time conversion.
    ///     current: Key for the latest-rate conversion.
    ///
    /// Returns:
    ///     The key this mode renders under.
    pub const fn key(self, historical: &'static str, current: &'static str) -> &'static str {
        match self {
            Self::Historical => historical,
            Self::Current => current,
        }
    }

    /// Decode a persisted mode code.
    ///
    /// Args:
    ///     code: Value read back from `settings.toml`.
    ///
    /// Returns:
    ///     The named mode, or `None` for an unrecognized value so the caller can keep its default.
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "historical" => Some(Self::Historical),
            "current" => Some(Self::Current),
            _ => None,
        }
    }
}

impl serde::Serialize for ValuationMode {
    /// Write the stable code rather than the variant name, so a rename cannot move the file format.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl<'de> serde::Deserialize<'de> for ValuationMode {
    /// Map an unknown code, a scalar of the wrong type, a sequence, or a map to the default.
    ///
    /// This field decides how money is CONVERTED, but it is one enum among many in
    /// `settings.toml`: rejecting the whole file over it would leave the application holding
    /// defaults for every other setting and forbidden to write them back (see
    /// `AppConfig::settings_unreadable`). Falling back to `Historical` costs the user one
    /// re-selection of an opt-in mode; failing the parse costs them their configuration.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        /// Visitor accepting every TOML form that has a safe default valuation mode.
        struct AnyScalar;

        impl<'de> serde::de::Visitor<'de> for AnyScalar {
            type Value = ValuationMode;

            /// Describe the accepted codes for serde diagnostics.
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a valuation mode (historical / current)")
            }

            /// Parse a code, mapping an unknown one to the default.
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(ValuationMode::from_code(v).unwrap_or_default())
            }

            /// Map a signed integer to the default.
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(ValuationMode::default())
            }

            /// Map an unsigned integer to the default.
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(ValuationMode::default())
            }

            /// Map a floating-point value to the default.
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(ValuationMode::default())
            }

            /// Map a Boolean to the default.
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(ValuationMode::default())
            }

            /// Map unit/null to the default.
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(ValuationMode::default())
            }

            /// Consume an entire sequence, keeping deserialization synchronized, and default.
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(ValuationMode::default())
            }

            /// Consume an entire map, keeping deserialization synchronized, and default.
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                while map
                    .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(ValuationMode::default())
            }
        }

        d.deserialize_any(AnyScalar)
    }
}

/// One quote currency's latest known USDT price.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentRate {
    /// USDT paid for one unit of the quote currency.
    pub rate_usdt: f64,
    /// Provider that answered.
    pub provider: String,
    /// Exchange symbol the price came from.
    pub symbol: String,
    /// When this rate was fetched, for the freshness cutoff.
    pub fetched_at_ms: i64,
}

impl CurrentRate {
    /// Whether two rates would render the same figure and the same provenance.
    ///
    /// `fetched_at_ms` is excluded on purpose: it moves on every refresh and drives only the
    /// freshness cutoff, so including it would make every re-fetch look like a new number. Exact
    /// bit equality rather than a tolerance — a threshold here would decide, silently and for
    /// everyone, how wrong a displayed total is allowed to be.
    ///
    /// Args:
    ///     other: Rate to compare against.
    ///
    /// Returns:
    ///     `true` when price, provider, and symbol are all identical.
    pub(super) fn renders_same(&self, other: &Self) -> bool {
        self.rate_usdt.to_bits() == other.rate_usdt.to_bits()
            && self.provider == other.provider
            && self.symbol == other.symbol
    }

    /// Whether this rate still counts as current.
    ///
    /// The one statement of the cutoff. Three callers ask it — the projection builder, the
    /// worker's expiry sweep, and its change detection — and a rule copied into each of them is a
    /// rule that can be changed in one and not the others.
    ///
    /// Args:
    ///     now_ms: Instant to judge against, in Unix milliseconds.
    ///
    /// Returns:
    ///     `true` while the fetch is younger than [`FRESHNESS_MS`].
    pub(super) fn is_fresh(&self, now_ms: i64) -> bool {
        now_ms.saturating_sub(self.fetched_at_ms) < FRESHNESS_MS
    }
}

/// The worker's published current-rate snapshot.
///
/// Two maps rather than one: a quote with no rate yet is still being worked on, while a quote whose
/// routes were all classified as permanently absent on the latest attempt is unavailable until a
/// later pass resolves one. Collapsing them would either report a permanent gap as perpetual
/// progress or flash "unavailable" while the first refresh pass is still running.
#[derive(Clone, Debug, Default)]
pub struct CurrentRates {
    /// Latest price per quote ordinal; USDT itself is never stored, it is an identity.
    rates: BTreeMap<i64, CurrentRate>,
    /// Quote ordinals whose direct and inverse routes were all last classified as permanently
    /// absent.
    missing: BTreeSet<i64>,
}

impl CurrentRates {
    /// Build a snapshot from the worker's accumulated results.
    ///
    /// Args:
    ///     rates: Latest price per quote ordinal.
    ///     missing: Ordinals whose routes are permanently absent.
    ///
    /// Returns:
    ///     A snapshot ready to publish.
    pub(super) fn new(rates: BTreeMap<i64, CurrentRate>, missing: BTreeSet<i64>) -> Self {
        Self { rates, missing }
    }

    /// Borrow the rates still inside the freshness window.
    ///
    /// Args:
    ///     now_ms: Current wall clock in Unix milliseconds.
    ///
    /// Returns:
    ///     Ordinal and rate pairs a projection may apply.
    pub(super) fn fresh(&self, now_ms: i64) -> impl Iterator<Item = (i64, &CurrentRate)> {
        self.rates
            .iter()
            .filter(move |(_, rate)| rate.is_fresh(now_ms))
            .map(|(ordinal, rate)| (*ordinal, rate))
    }

    /// Borrow the permanently unroutable quote ordinals.
    ///
    /// Returns:
    ///     Ordinals currently classified as permanently unroutable.
    pub(super) fn missing(&self) -> impl Iterator<Item = i64> + '_ {
        self.missing.iter().copied()
    }
}

/// The one published snapshot, replaced whole by the worker and read by every query.
///
/// A process-wide cell rather than a field threaded through every reader: the worker is a
/// singleton, the readers reach SQL through `&Connection` alone, and the alternative — carrying an
/// `Arc` inside `ReportFilter` and `Query` — would put a live handle inside two value types that
/// are cloned and compared for equality all over the UI.
static CURRENT_RATES: RwLock<Option<Arc<CurrentRates>>> = RwLock::new(None);

/// Publish a new snapshot, replacing the previous one whole.
///
/// Args:
///     rates: Replacement snapshot after a completed refresh pass or stale-rate retirement.
pub(super) fn publish_current_rates(rates: CurrentRates) {
    if let Ok(mut slot) = CURRENT_RATES.write() {
        *slot = Some(Arc::new(rates));
    }
}

thread_local! {
    /// One snapshot AND the instant it is judged fresh at, held for a multi-statement read.
    ///
    /// The clock is pinned with the snapshot, not left to each statement: freshness is a function
    /// of both, so a batch straddling the cutoff would otherwise include a rate in its first
    /// statement and exclude it from the next while holding the very same snapshot.
    static PINNED_RATES: std::cell::RefCell<Option<(Arc<CurrentRates>, i64)>> =
        const { std::cell::RefCell::new(None) };
}

/// Guard holding one rate snapshot and its freshness clock fixed for a whole read batch.
///
/// The rate half of what `db::read_snapshot` does for the rows: a Report batch runs `query_reports`
/// and then `query_totals`, and Analytics runs a preflight and then its scan — each of them a
/// separate statement, each building its own SQL. Without a pin, a worker publication landing
/// between two of those calls embeds different rates in one visible answer, and the rows stop
/// summing to the footer below them.
///
/// Not `Send`: it pins the thread it was taken on, which is the thread the batch runs on.
pub struct RatePin {
    /// Whatever was pinned before, restored on drop so nesting is harmless.
    previous: Option<(Arc<CurrentRates>, i64)>,
    /// Keeps the guard thread-bound, matching the thread-local it controls.
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for RatePin {
    /// Restore the enclosing pin, or clear it at the outermost level.
    fn drop(&mut self) {
        let previous = self.previous.take();
        PINNED_RATES.with(|slot| *slot.borrow_mut() = previous);
    }
}

/// Fix one snapshot and freshness clock for every current-rate projection built on this thread
/// until the guard drops.
///
/// Returns:
///     A guard that restores the previous pin when dropped.
pub fn pin_current_rates() -> RatePin {
    let pinned = (published_rates(), crate::util::now_unix_ms_i64());
    let previous = PINNED_RATES.with(|slot| slot.borrow_mut().replace(pinned));
    RatePin {
        previous,
        _not_send: std::marker::PhantomData,
    }
}

/// Read whatever the worker last published, ignoring any pin.
///
/// Returns:
///     The latest snapshot, or an empty one before the worker's first publication.
fn published_rates() -> Arc<CurrentRates> {
    CURRENT_RATES
        .read()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_default()
}

/// Read the snapshot and the instant every projection on this thread must agree on.
///
/// Returns:
///     The pinned pair while a [`RatePin`] is held, otherwise the latest published snapshot and
///     the current clock.
pub(super) fn current_rates_at() -> (Arc<CurrentRates>, i64) {
    PINNED_RATES
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| (published_rates(), crate::util::now_unix_ms_i64()))
}

/// Format one rate as a SQL literal that round-trips exactly.
///
/// `{:?}` on `f64` emits Rust's shortest round-trip form, which SQLite parses back to the same
/// double. A fixed-precision format here would silently round every BTC rate and the totals would
/// drift by thousands.
///
/// Args:
///     rate: Price to embed.
///
/// Returns:
///     A SQL numeric literal.
fn rate_literal(rate: f64) -> String {
    format!("{rate:?}")
}

/// Escape one provenance string for embedding as a SQL text literal.
///
/// Args:
///     text: Provider or symbol text.
///
/// Returns:
///     The text with embedded quotes doubled.
fn text_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Build current-rate valuation SQL for one physical report source.
///
/// The shape is deliberately [`CoverageSql`], identical to the historical builder, so that every
/// downstream consumer — the aggregate columns, the Report projection, the sort expression and the
/// three per-row columns — stays untouched and cannot treat the two modes differently by accident.
///
/// It joins nothing. The rates live in memory, so they enter as literals in a `CASE` over the quote
/// ordinal. Literals rather than bound parameters because the produced string is reused as a `FROM`
/// fragment by callers that bind only their own date range: there is no placeholder budget to spend.
///
/// Args:
///     alias: Qualified report-row alias used by the caller.
///     columns: Discovered physical source columns.
///     rates: Published snapshot.
///     now_ms: Current wall clock, for the freshness cutoff.
///
/// Returns:
///     Coverage fragments carrying no joins at all.
pub(crate) fn current_rate_sql(
    alias: &str,
    columns: &std::collections::HashSet<String>,
    rates: &CurrentRates,
    now_ms: i64,
) -> CoverageSql {
    let has_quote = columns.contains("basecurrency");
    let has_profit = columns.contains("profitbtc");
    // The same guards the historical builder applies, from the one place that states them.
    let super::SourcePredicates {
        quote_known,
        numeric_profit,
        spent_value,
    } = super::source_predicates(alias, columns);

    // Without a quote column there is nothing to switch on, and naming it in an unreachable arm
    // would still fail at prepare time — so the arms are not even built.
    let (rate_case, source_case) = if has_quote {
        // Identity USDT is always convertible and needs no provider, so its arm is emitted whether
        // or not the worker has fetched anything. Without it, a USDT-only scope would remain blank
        // for no reason.
        let mut rate_arms = String::from(" WHEN 1 THEN 1.0");
        let mut source_arms = String::from(" WHEN 1 THEN 'identity'");
        for (ordinal, rate) in rates.fresh(now_ms) {
            if ordinal == 1 || !rate.rate_usdt.is_finite() || rate.rate_usdt <= 0.0 {
                continue;
            }
            rate_arms.push_str(&format!(
                " WHEN {ordinal} THEN {}",
                rate_literal(rate.rate_usdt)
            ));
            source_arms.push_str(&format!(
                " WHEN {ordinal} THEN {}",
                text_literal(&format!("current {} {}", rate.provider, rate.symbol))
            ));
        }
        // Switched on the EFFECTIVE ordinal, like every other quote reference: a COIN-M row is
        // denominated in BTC, so it must pick the BTC arm rather than the identity one.
        let quote = super::super::quote::effective_ordinal_expr(alias, columns);
        (
            format!("(CASE ({quote}){rate_arms} END)"),
            format!("(CASE ({quote}){source_arms} END)"),
        )
    } else {
        ("NULL".to_string(), "NULL".to_string())
    };

    let eligible = format!("({quote_known})");
    let valued = format!("({eligible} AND {numeric_profit} AND {rate_case} IS NOT NULL)");
    let unroutable: Vec<String> = rates.missing().map(|ordinal| ordinal.to_string()).collect();
    // Absent from both maps means "not fetched yet", which is progress, not a permanent gap. Only a
    // quote whose every route came back permanently missing counts as unavailable.
    let unavailable = if has_quote && !unroutable.is_empty() {
        format!(
            "({eligible} AND {numeric_profit} AND {rate_case} IS NULL
              AND ({quote}) IN ({}))",
            unroutable.join(","),
            quote = super::super::quote::effective_ordinal_expr(alias, columns)
        )
    } else {
        "0".to_string()
    };
    let profit_usdt = if has_profit {
        format!("CASE WHEN {valued} THEN {alias}.profitbtc * {rate_case} END")
    } else {
        "NULL".to_string()
    };
    let spent_usdt = format!("CASE WHEN {valued} THEN ({spent_value}) * {rate_case} END");
    CoverageSql {
        joins: String::new(),
        per_row: PerRowSql {
            joins: String::new(),
            rate: format!("CASE WHEN {valued} THEN {rate_case} END"),
            source: format!("CASE WHEN {valued} THEN {source_case} END"),
        },
        eligible,
        valued,
        unavailable,
        profit_usdt,
        spent_usdt,
    }
}

#[cfg(test)]
mod tests;
