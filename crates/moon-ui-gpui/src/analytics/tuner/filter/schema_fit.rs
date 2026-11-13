//! What a Filters-axis save may write onto one target, given the strategy schema of the core
//! that holds it.
//!
//! A spot core's schema has no leverage or mark-price bounds (`MinLeverage`, `MaxLeverage`,
//! `MarkPriceMin`, `MarkPriceMax`), and a TopMarket strategy no mark-price bounds on any core. A
//! fit fills `Lev` / `dMark` like every other row, and before this filter the save sent them
//! anyway: the core could not store them, the whole strategy came back `Adjusted` (#1160), and
//! writing them had already cleared `IgnoreBase` — a real change on a strategy that has nothing
//! for it to switch on. The feed drops such a name too (`feed::strategies::storable_edits`); this
//! side also takes back the ignore flags the dropped bounds alone had cleared, which the feed
//! cannot tell from a flag the user asked for.

use std::collections::{HashMap, HashSet};

use moon_core::db::tuner::{FIELDS, FieldClass};
use moon_core::feed::{StrategyRow, StrategySchemaModel};

/// Lowercase names of the fields a strategy's schema shows for its kind, across every copy of it
/// a save reaches.
///
/// A copy whose core has sent no schema yet, or whose row is not in the list, cannot be judged:
/// the answer is then `None` and the save keeps every field, as before. Across several copies a
/// field shown by ANY of them is kept — the feed still leaves it out of the copies that cannot
/// store it.
///
/// Args:
///     copies: For each core the save writes this strategy to, that core's schema and the
///         strategy's row there.
///
/// Returns:
///     The shown names, or `None` when some copy cannot be judged or there is none.
pub(in crate::analytics::tuner) fn shown_fields<'a>(
    copies: impl IntoIterator<Item = (Option<&'a StrategySchemaModel>, Option<&'a StrategyRow>)>,
) -> Option<HashSet<String>> {
    let mut shown: Option<HashSet<String>> = None;
    for (schema, row) in copies {
        let ordinal = row?.kind_ordinal;
        let kind = schema?.kinds.iter().find(|k| k.ordinal == ordinal)?;
        let names = kind
            .sections
            .iter()
            .flat_map(|section| section.fields.iter())
            .map(|field| field.name.to_ascii_lowercase());
        shown.get_or_insert_with(HashSet::new).extend(names);
    }
    shown
}

/// The class of the threshold a parameter writes, `None` for anything else (a flag, a comment).
fn class_of(key: &str) -> Option<FieldClass> {
    if let Some(spec) = FIELDS
        .iter()
        .find(|spec| spec.p_min == Some(key) || spec.p_max == Some(key))
    {
        return Some(spec.class);
    }
    let slot = key
        .strip_prefix("Delta")
        .and_then(|rest| rest.split_once('_'))
        .is_some_and(|(n, part)| n.parse::<u8>().is_ok() && matches!(part, "Type" | "Min" | "Max"));
    slot.then_some(FieldClass::DeltaSlot)
}

/// Whether an ignore flag's enabling value still has a threshold behind it.
///
/// Mirrors what `wake::flag_changes` clears for the thresholds a save writes.
fn flag_needed(flag: &str, written: &[FieldClass]) -> Option<bool> {
    let any = |classes: &[FieldClass]| written.iter().any(|c| classes.contains(c));
    Some(match flag {
        "IgnoreFilters" => written.iter().any(|c| *c != FieldClass::Detect),
        "IgnoreDelta" => any(&[FieldClass::Delta, FieldClass::DeltaSlot]),
        "IgnoreVolume" => any(&[FieldClass::Volume, FieldClass::BvSv]),
        "UseBV_SV_Filter" => any(&[FieldClass::BvSv]),
        "IgnorePing" => any(&[FieldClass::Ping]),
        "IgnoreBase" => any(&[FieldClass::Base]),
        _ => return None,
    })
}

/// The value of an ignore flag that switches its group ON — what the save writes for the
/// thresholds it sets. `UseBV_SV_Filter` is an enabler, the others are ignores.
fn enabling(flag: &str) -> &'static str {
    if flag == "UseBV_SV_Filter" {
        "YES"
    } else {
        "NO"
    }
}

/// The change list one target can store.
///
/// Thresholds its schema does not show are left out. An ignore flag the save cleared only for
/// those thresholds goes with them; a flag the user staged on a subheader stays. When nothing
/// but the analyzer's Comment stamp would be left, the stamp goes too: it would record a save
/// that wrote nothing.
///
/// Args:
///     changes: The target's edits after the kind filter.
///     shown: [`shown_fields`] for the target; `None` keeps the list as it is.
///     staged: The subheader toggles the user staged, by flag name.
///
/// Returns:
///     The edits to write onto the target.
pub(in crate::analytics::tuner) fn fit_to_schema(
    changes: &[(String, String)],
    shown: Option<&HashSet<String>>,
    staged: &HashMap<&'static str, bool>,
) -> Vec<(String, String)> {
    let Some(shown) = shown else {
        return changes.to_vec();
    };
    let storable = |key: &str| class_of(key).is_none() || shown.contains(&key.to_ascii_lowercase());
    if changes.iter().all(|(key, _)| storable(key)) {
        return changes.to_vec();
    }
    let kept: Vec<(String, String)> = changes
        .iter()
        .filter(|(key, _)| storable(key))
        .cloned()
        .collect();
    let written: Vec<FieldClass> = kept.iter().filter_map(|(key, _)| class_of(key)).collect();
    let fitted: Vec<(String, String)> = kept
        .into_iter()
        .filter(|(key, value)| {
            staged.contains_key(key.as_str())
                || value != enabling(key)
                || flag_needed(key, &written).unwrap_or(true)
        })
        .collect();
    if fitted.iter().all(|(key, _)| key == "Comment") {
        return Vec::new();
    }
    fitted
}

#[cfg(test)]
mod tests;
