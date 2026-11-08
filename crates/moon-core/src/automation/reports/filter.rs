//! The trade filter every report tool shares, and the close-reason classes.
//!
//! What SQL can narrow cheaply — cores, the window, side, emulator, exact strategy ids — goes into
//! the Report layer's own filter, whose `build_where` stays untouched. Everything else is checked
//! here, in Rust, on rows that already came back: no text a client sends ever reaches SQL.

use anyhow::{Result, bail, ensure};
use serde::Deserialize;

use super::trade::Trade;

/// Longest window one call may read; a year of trades is about 220 thousand rows.
pub const MAX_WINDOW_MS: i64 = 366 * 86_400_000;

/// The shared `filter` argument. Conditions join with AND.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradeFilter {
    /// Core ids as strings; empty means every core the client may read. Resolved by the caller,
    /// which holds the grant.
    #[serde(default)]
    pub cores: Vec<String>,
    /// Window start, UTC milliseconds. Required on the main filter.
    pub from_ms: Option<i64>,
    /// Window end, UTC milliseconds. Required on the main filter.
    pub to_ms: Option<i64>,
    /// Which time the window applies to.
    #[serde(default)]
    pub date_field: DateField,
    #[serde(default)]
    pub coins: Vec<String>,
    #[serde(default)]
    pub exclude_coins: Vec<String>,
    /// `%` and `_` wildcards, ignoring case.
    pub coin_like: Option<String>,
    /// Decimal strategy ids; `"0"` selects manual trades.
    #[serde(default)]
    pub strategy_ids: Vec<String>,
    /// The Strategies window's query syntax, matched against the current name or the name at the
    /// time of the trade.
    pub strategy_name: Option<String>,
    pub side: Option<Side>,
    pub currency: Option<String>,
    #[serde(default)]
    pub reason: Vec<Reason>,
    #[serde(default)]
    pub exclude_reason: Vec<Reason>,
    pub lev_min: Option<f64>,
    pub lev_max: Option<f64>,
    pub profit_usdt_min: Option<f64>,
    pub profit_usdt_max: Option<f64>,
    pub profit_pct_min: Option<f64>,
    pub profit_pct_max: Option<f64>,
    pub hold_sec_min: Option<f64>,
    pub hold_sec_max: Option<f64>,
    /// Bounds on entry conditions, by tuner field.
    #[serde(default)]
    pub entry: Vec<EntryBound>,
    #[serde(default)]
    pub include_emulator: bool,
}

/// Which trade time a window applies to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateField {
    #[default]
    Close,
    Buy,
}

/// Trade direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Long,
    Short,
}

/// One bound on an entry condition, with the tuner's meaning: an empty field reads as 0.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryBound {
    pub field: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

/// Why a trade closed, folded from the core's free text.
///
/// Checked on a year of a real replica (221 728 rows): eight classes cover every text, and a new
/// text from a newer core lands in `Other`, where its row count shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    AutoPriceDown,
    Stoploss,
    SellPrice,
    TrailingStop,
    Manual,
    Funding,
    Liquidation,
    Other,
}

impl Reason {
    /// The class of one stored `sellreason`.
    pub fn classify(text: &str) -> Self {
        let text = text.trim();
        if text.eq_ignore_ascii_case("Auto Price Down") {
            Reason::AutoPriceDown
        } else if text.starts_with("StopLoss") {
            Reason::Stoploss
        } else if text.eq_ignore_ascii_case("Sell Price") {
            Reason::SellPrice
        } else if text.starts_with("TrailingStop") {
            Reason::TrailingStop
        } else if ["Manual PanicSell", "Manual Sell", "Global PanicSell"]
            .iter()
            .any(|manual| text.eq_ignore_ascii_case(manual))
        {
            Reason::Manual
        } else if text.eq_ignore_ascii_case("Funding") {
            Reason::Funding
        } else if text.eq_ignore_ascii_case("LIQUIDATION") {
            Reason::Liquidation
        } else {
            Reason::Other
        }
    }

    /// The class as the answers name it.
    pub fn name(self) -> &'static str {
        match self {
            Reason::AutoPriceDown => "auto_price_down",
            Reason::Stoploss => "stoploss",
            Reason::SellPrice => "sell_price",
            Reason::TrailingStop => "trailing_stop",
            Reason::Manual => "manual",
            Reason::Funding => "funding",
            Reason::Liquidation => "liquidation",
            Reason::Other => "other",
        }
    }
}

/// The checks a filter applies after the read, prepared once.
pub struct Residual {
    coins: Vec<String>,
    exclude_coins: Vec<String>,
    coin_like: Option<Vec<char>>,
    name: Option<crate::strategy_query::StrategyQuery>,
    currency: Option<String>,
    filter: TradeFilter,
    /// Index of each entry bound's field in the trade's entry values.
    entry: Vec<(usize, Option<f64>, Option<f64>)>,
}

impl Residual {
    /// Prepare the post-read checks of `filter`.
    ///
    /// Args:
    ///     filter: The client's filter.
    ///     entry_fields: The entry columns the read projects, in the order trades carry them.
    ///
    /// Errors:
    ///     An entry bound on a field that is not a tuner field.
    pub fn new(filter: &TradeFilter, entry_fields: &[&'static str]) -> Result<Self> {
        let entry = filter
            .entry
            .iter()
            .map(|bound| {
                let Some(index) = entry_fields.iter().position(|f| *f == bound.field) else {
                    bail!("entry field {} is not a tuner field", bound.field);
                };
                Ok((index, bound.min, bound.max))
            })
            .collect::<Result<_>>()?;
        let fold = |coins: &[String]| -> Vec<String> {
            coins
                .iter()
                .map(|c| crate::symbol::coin_match_key(c))
                .collect()
        };
        let name = filter
            .strategy_name
            .as_deref()
            .map(crate::strategy_query::StrategyQuery::parse)
            .filter(|query| !query.is_empty());
        Ok(Self {
            coins: fold(&filter.coins),
            exclude_coins: fold(&filter.exclude_coins),
            coin_like: filter
                .coin_like
                .as_deref()
                .map(|p| p.to_ascii_uppercase().chars().collect()),
            name,
            currency: filter.currency.as_ref().map(|c| c.to_ascii_uppercase()),
            filter: filter.clone(),
            entry,
        })
    }

    /// Whether the trade passes every post-read condition.
    pub fn keeps(&self, trade: &Trade) -> bool {
        let f = &self.filter;
        let within = |value: Option<f64>, min: Option<f64>, max: Option<f64>| {
            if min.is_none() && max.is_none() {
                return true;
            }
            value.is_some_and(|v| min.is_none_or(|m| v >= m) && max.is_none_or(|m| v <= m))
        };
        (self.coins.is_empty() || self.coins.iter().any(|c| *c == *trade.coin_key))
            && !self.exclude_coins.iter().any(|c| *c == *trade.coin_key)
            && self
                .coin_like
                .as_ref()
                .is_none_or(|p| like(p, &trade.coin.to_ascii_uppercase()))
            && self.name.as_ref().is_none_or(|query| {
                query.matches(&trade.name)
                    || trade
                        .name_at_trade
                        .as_deref()
                        .is_some_and(|n| query.matches(n))
            })
            && self
                .currency
                .as_deref()
                .is_none_or(|c| trade.currency == Some(c))
            && (f.reason.is_empty() || f.reason.contains(&trade.reason))
            && !f.exclude_reason.contains(&trade.reason)
            && within(Some(trade.lev), f.lev_min, f.lev_max)
            && within(trade.profit_usdt, f.profit_usdt_min, f.profit_usdt_max)
            && within(trade.profit_pct, f.profit_pct_min, f.profit_pct_max)
            && within(
                trade.hold_sec.map(|s| s as f64),
                f.hold_sec_min,
                f.hold_sec_max,
            )
            && self.entry.iter().all(|(index, min, max)| {
                // The tuner's meaning: a condition the core did not record reads as 0.
                let value = trade.entry.get(*index).copied().flatten().unwrap_or(0.0);
                within(Some(value), *min, *max)
            })
    }
}

/// SQL `LIKE` over characters: `%` any run, `_` one character. Both sides already upper-cased.
fn like(pattern: &[char], text: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '_' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '%' {
            star = Some(p);
            p += 1;
            mark = t;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '%')
}

/// The window of a main filter, checked.
///
/// Errors:
///     A missing bound, an empty or reversed window, or one longer than [`MAX_WINDOW_MS`].
pub fn window(filter: &TradeFilter) -> Result<(i64, i64)> {
    let (Some(from), Some(to)) = (filter.from_ms, filter.to_ms) else {
        bail!("filter.from_ms and filter.to_ms are required");
    };
    ensure!(
        from > 0 && to > from && to - from <= MAX_WINDOW_MS,
        "the window must be positive and at most 366 days"
    );
    Ok((from, to))
}

/// Strategy ids as the report stores them: a decimal `u64`, kept bit for bit as `i64`.
///
/// An id above 2^63 is stored negative, so parsing it straight as `i64` would select nothing.
///
/// Errors:
///     A value that is not a decimal unsigned integer.
pub fn strategy_ids(filter: &TradeFilter) -> Result<Vec<i64>> {
    filter
        .strategy_ids
        .iter()
        .map(|id| {
            id.trim()
                .parse::<u64>()
                .map(|id| id as i64)
                .map_err(|_| anyhow::anyhow!("strategy id {id} is not a decimal unsigned integer"))
        })
        .collect()
}

#[cfg(test)]
mod tests;
