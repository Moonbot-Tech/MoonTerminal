//! The exit fields a strategy can switch on that the model does not have — what a search and a
//! write must warn about: the search ran without them, and a strategy that keeps them on will not
//! exit the way the columns counted.
//!
//! The list is the exit side of the strategy window — Stops, Sell order, SellShot, SellSpread,
//! Delta Modifiers — less the fields the model reads ([`super::params::param_keys`]) and less
//! those that do not change WHERE or WHEN the position is sold:
//!
//! - the panic sell's execution — `AllowedDrop`, `AllowedDrop3`, `StopLossSpread`,
//!   `StopSpreadAdd1mDelta`, `TrailingSpread`: the verdict judges a stop by its decision, and a
//!   variant that keeps the trade's stop takes the fact's own sale (`record::StopAnchor`);
//! - the liquidation guards and the grid's fixed stop — `DontSellBelowLiq`, `StopAboveLiq`,
//!   `StopLossFixed`: not taken into account at all (the developer's call, 2026-09-24);
//! - `UseMarketOrder` — how a stop's sale goes, the book's again: the verdict tells a market stop
//!   by the fact's own reason (`StopLoss Market Sell`), and a variant keeping the stop takes the
//!   fact's sale;
//! - the entry — `SellEMACheckEnter` (checks the EMA filter before the BUY), `BuyModifier`,
//!   `DetectModifier`;
//! - the EMA sell — `SellByCustomEMA` and its `SellEMADelay`: unused, and left out of the list
//!   (the developer's call, 2026-09-24);
//! - what acts only by hand or only in another kind — `SplitPiece` (a chart menu item),
//!   `UseMarketStop`/`MarketStopLevel` (Manual), `SellPriceAbsolute`/`SellFromAssets`/
//!   `SellQuantity` (NewListing): none of them is a kind the tuner runs;
//! - the fields of a listed switch (`SecondStopLoss`, `BV_SV_Ratio`, `SellEMADelay`, …): the
//!   switch stands for them.
//!
//! "Switched on" is the Strategies window's own reading: the field's dependency rule holds
//! (`assets/param_deps.toml`, [`FieldDeps`]) and its value differs from the core's default — the
//! live schema's, else the one written here, which is the site's (`moonbot.pro`, the Sell order
//! and Stops tabs) or, where the site is silent, the value the live strategies leave out (24.09).
//!
//! The same reading decides which rows the tuner's grid draws ([`fields_in_use`]): a field no
//! strategy of the scope switches on is left out of it. There the whole kind's schema fills what
//! a dump leaves out, as the Strategies window fills its values (`strategies::logic::
//! selected_values`). The Delta Modifiers tab is read as a whole on top of that: its sum acts
//! only through a modifier that applies it, so a coefficient with no modifier, or a modifier with
//! no coefficient, uses nothing (the core developer via LinKvo, 2026-09-24,
//! `STRATEGY_FORMULAS/sell-common.md`).

use std::collections::HashMap;

use super::exit::UnmodelledRule;
use super::params::ParamSection;
use crate::feed::SchemaSection;
use crate::feed::strategy_deps::{FieldDeps, Values, as_bool};

/// A further condition a watched field only acts under, beyond its dependency rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Also {
    Nothing,
    /// `UseScalpingMode` acts only while `SellPrice` is under 1 % (the site's Sell order tab).
    SellPriceUnderOne,
    /// SellShot keeps the sell at a distance only when there is one (`SellShotDistance` ≠ 0) —
    /// as `params::unmodelled_rule` reads it.
    SellShotDistance,
}

/// One exit field the model does not have.
#[derive(Clone, Copy, Debug)]
struct Watched {
    key: &'static str,
    section: ParamSection,
    /// The core's default when no live schema says it.
    default: &'static str,
    also: Also,
    /// The rule of the model it stands for, when it takes the trade out of the verdict rather
    /// than only warning.
    rule: Option<UnmodelledRule>,
}

const fn watched(key: &'static str, section: ParamSection, default: &'static str) -> Watched {
    Watched {
        key,
        section,
        default,
        also: Also::Nothing,
        rule: None,
    }
}

/// Every exit field a strategy can switch on that the model does not have, in the strategy
/// window's order.
const WATCHED: &[Watched] = &[
    // Stops. The ladder is modelled (`exit::stops::ladder`); `DontSellBelowLiq`, `StopAboveLiq`
    // and `StopLossFixed` are left out on purpose (the developer's call, 2026-09-24).
    // A stop on the ratio of buy to sell volume, over trades the tape has but a rule it does not.
    watched("UseBV_SV_Stop", ParamSection::Stops, "NO"),
    // Stops taken from a Telegram signal.
    watched("UseSignalStops", ParamSection::Stops, "NO"),
    // A panic sell on a delisting message.
    watched("PanicSellDelisted", ParamSection::Stops, "NO"),
    // Sell order.
    // NO places no sell at all.
    Watched {
        rule: Some(UnmodelledRule::NoAutoSell),
        ..watched("AutoSell", ParamSection::SellOrder, "YES")
    },
    // PriceDown counted to `PriceDownAllowedDrop`, not to the buy (the core developer's answer
    // 10 and the site); the model counts to the buy, which is NO.
    watched("PriceDownToAllowedDrop", ParamSection::SellOrder, "NO"),
    // The sell placed under the ASK book's walls, up to 2 % — the book, which the tape has not.
    Watched {
        also: Also::SellPriceUnderOne,
        ..watched("UseScalpingMode", ParamSection::SellOrder, "NO")
    },
    // Sells when the filters' bounds are left, seconds after the buy; 0 is off.
    watched("SellByFilters", ParamSection::SellOrder, "0"),
    // SellShot.
    Watched {
        also: Also::SellShotDistance,
        rule: Some(UnmodelledRule::SellShot),
        ..watched("IgnoreSellShot", ParamSection::SellShot, "YES")
    },
    // SellSpread.
    Watched {
        rule: Some(UnmodelledRule::SellSpread),
        ..watched("IgnoreSellSpread", ParamSection::SellSpread, "YES")
    },
];

/// The Delta Modifiers tab's fields that APPLY its sum `Σ Pn·Dn`, lowercase — the sell's, the
/// stop's, and the buy's and the detect's for the kinds whose schema shows them; every other field
/// of the tab but `MaxModifier` is a term of the sum.
const DELTA_APPLIERS: &[&str] = &[
    "sellmodifier",
    "stoplossmodifier",
    "buymodifier",
    "detectmodifier",
];

/// The fields the dependency rules of [`WATCHED`] and its extra conditions read, spelled as the
/// strategy dump spells them — a read by key is case-sensitive, and [`FieldDeps`] hands the
/// names back lowercase. A unit test holds this against the bundled rules.
const CONDITION_KEYS: &[&str] = &[
    "HODLmode",
    "AutoSell",
    "UseStopLoss",
    "PriceDownTimer",
    "PriceDownRelative",
    "SellPrice",
    "SellShotDistance",
];

/// One field of one strategy that switches on exit behaviour the model does not have.
#[derive(Clone, Debug, PartialEq)]
pub struct UnmodelledField {
    /// The field, as the strategy window names it.
    pub key: &'static str,
    /// The strategy's value.
    pub value: String,
    /// The strategy window's section the field sits in.
    pub section: ParamSection,
    /// The model's rule it stands for, when the model does not judge a trade under it at all;
    /// `None` for a field it only warns about.
    pub rule: Option<UnmodelledRule>,
}

/// Every field a read must fetch for [`unmodelled_fields`] to answer.
pub fn watched_keys() -> Vec<String> {
    WATCHED
        .iter()
        .map(|w| w.key)
        .chain(CONDITION_KEYS.iter().copied())
        .map(str::to_string)
        .collect()
}

/// The exit fields `values` switches on that the model does not have, in the strategy window's
/// order.
///
/// Args:
///     values: The strategy's values by field name, as `strategy_current_values` reads them — a
///         field left at its default is absent.
///     defaults: The live schema's numeric defaults, lowercase names (`strategy_field_defaults`);
///         empty without a connected core.
///     deps: The fields' dependency rules.
pub fn unmodelled_fields(
    values: &HashMap<String, String>,
    defaults: &HashMap<String, f64>,
    deps: &FieldDeps,
) -> Vec<UnmodelledField> {
    // The Strategies window's view of the strategy: every stored field, lowercase, the schema's
    // default filling a condition field the dump leaves out — absent means "not this kind's".
    let mut effective: Values = values
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
        .collect();
    for key in CONDITION_KEYS {
        let lower = key.to_ascii_lowercase();
        if let Some(default) = defaults.get(&lower) {
            effective
                .entry(lower)
                .or_insert_with(|| default.to_string());
        }
    }
    WATCHED
        .iter()
        .filter_map(|w| {
            let value = values.get(w.key)?;
            let default = defaults
                .get(&w.key.to_ascii_lowercase())
                .map(f64::to_string)
                .unwrap_or_else(|| w.default.to_string());
            let on = switched_on(w.key, value, &default, &effective, deps)
                && also_holds(w.also, values, defaults);
            on.then(|| UnmodelledField {
                key: w.key,
                value: value.clone(),
                section: w.section,
                rule: w.rule,
            })
        })
        .collect()
}

/// The fields of `sections` — one strategy kind's schema — that the strategy holding `values`
/// switches on, lowercase, in the schema's order: the field's rule holds and its value is not
/// the schema's default.
///
/// Args:
///     sections: The strategy kind's live schema, every section of it: a rule may read a field
///         of any section.
///     values: The strategy's values by field name, as `strategy_current_values` reads them — a
///         field left at its default is absent, and reads the schema's default here, else
///         nothing, as the Strategies window reads it.
///     deps: The fields' dependency rules.
pub fn fields_in_use(
    sections: &[SchemaSection],
    values: &HashMap<String, String>,
    deps: &FieldDeps,
) -> Vec<String> {
    let fields = || sections.iter().flat_map(|s| s.fields.iter());
    let mut effective: Values = values
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
        .collect();
    for field in fields() {
        effective
            .entry(field.name.to_ascii_lowercase())
            .or_insert_with(|| field.default.clone().unwrap_or_default());
    }
    let mut out: Vec<String> = Vec::new();
    for field in fields() {
        let key = field.name.to_ascii_lowercase();
        let default = field.default.as_deref().unwrap_or_default();
        let on = effective
            .get(&key)
            .is_some_and(|value| switched_on(&field.name, value, default, &effective, deps));
        if on && !out.contains(&key) {
            out.push(key);
        }
    }
    drop_silent_delta_tab(sections, &mut out);
    out
}

/// Take the Delta Modifiers tab out of `in_use` unless it acts: a term of the sum in use AND a
/// modifier that applies it in use. The tab is the section holding `SellModifier`.
fn drop_silent_delta_tab(sections: &[SchemaSection], in_use: &mut Vec<String>) {
    let Some(tab) = sections.iter().find(|s| {
        s.fields
            .iter()
            .any(|f| f.name.eq_ignore_ascii_case("SellModifier"))
    }) else {
        return;
    };
    let tab: Vec<String> = tab
        .fields
        .iter()
        .map(|f| f.name.to_ascii_lowercase())
        .collect();
    let applied = in_use.iter().any(|k| DELTA_APPLIERS.contains(&k.as_str()));
    let summed = in_use
        .iter()
        .any(|k| tab.contains(k) && !DELTA_APPLIERS.contains(&k.as_str()) && k != "maxmodifier");
    if !(applied && summed) {
        in_use.retain(|k| !tab.contains(k));
    }
}

/// Whether a strategy switches a field on, as the Strategies window reads it: the field's rule
/// holds on the strategy's values and its value is not the default.
///
/// Args:
///     key: The field.
///     value: The strategy's value of it.
///     default: The core's default of it.
///     effective: The strategy's values as the rules read them, lowercase.
///     deps: The fields' dependency rules.
fn switched_on(
    key: &str,
    value: &str,
    default: &str,
    effective: &Values,
    deps: &FieldDeps,
) -> bool {
    !same_value(value, default) && deps.field_active(key, effective)
}

/// Whether the extra condition of a watched field holds.
fn also_holds(
    also: Also,
    values: &HashMap<String, String>,
    defaults: &HashMap<String, f64>,
) -> bool {
    let num = |key: &str, fallback: f64| {
        values
            .get(key)
            .and_then(|v| number(v))
            .or_else(|| defaults.get(&key.to_ascii_lowercase()).copied())
            .unwrap_or(fallback)
    };
    match also {
        Also::Nothing => true,
        // The model's own fallback for `SellPrice` (`ExitParams::default`).
        Also::SellPriceUnderOne => num("SellPrice", 1.0) < 1.0,
        Also::SellShotDistance => num("SellShotDistance", 0.0) != 0.0,
    }
}

/// Whether two spellings of a field's value are the same value: as booleans when both are one
/// (`YES`, `1`, `True`…), as numbers when both are (`0.0` and `0`), else as text.
pub(super) fn same_value(a: &str, b: &str) -> bool {
    if let (Some(x), Some(y)) = (as_bool(a), as_bool(b)) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (number(a), number(b)) {
        return (x - y).abs() < 1e-9;
    }
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// A strategy number: `1.5`, `1,5`, `1.5%`.
fn number(s: &str) -> Option<f64> {
    s.trim()
        .trim_end_matches('%')
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests;
