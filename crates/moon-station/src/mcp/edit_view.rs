//! What an AI client is shown of a strategy edit: names and the fields that differ.
//!
//! The Station keeps every listed strategy's whole before/after snapshot and compares them again
//! at apply time; the client never sends them back, so it has no use for them. A whole strategy is
//! about 9 KB, mostly coin lists, and the preview carried it twice: a client that re-reads its
//! conversation on every request paid for that hundreds of times over (#1458).

use std::collections::BTreeSet;

use moon_core::automation::Action;
use serde_json::{Map, Value, json};

/// A string value longer than this is a list for display: shown as its size and its difference,
/// not whole, unless the client asks for whole values.
pub(super) const LONG_VALUE: usize = 100;

/// Most items a list difference names on each side; the rest are in the counts.
const LIST_ITEMS: usize = 20;

/// The part of a preview the client reads; the retained preview stays whole.
///
/// Args:
///     action: The action previewed.
///     preview: The preview as retained for apply.
///     full_lists: Show long values whole.
///
/// Returns:
///     For a strategy edit, each changed strategy's id, name and differing fields, and the ids
///     left out because they already hold the values. Any other action's preview unchanged.
pub(super) fn client_view(action: &Action, preview: &Value, full_lists: bool) -> Value {
    if !matches!(action, Action::StrategyEdit { .. }) {
        return preview.clone();
    }
    let mut changed = Vec::new();
    let mut unchanged = Vec::new();
    for strategy in preview["strategies"].as_array().into_iter().flatten() {
        let name = name_of(&strategy["after"]).or_else(|| name_of(&strategy["before"]));
        if strategy["unchanged"] == true {
            unchanged.push(json!({"id":strategy["id"],"name":name}));
            continue;
        }
        changed.push(json!({"id":strategy["id"],"name":name,
            "changes":differences(&strategy["before"], &strategy["after"], full_lists)}));
    }
    let mut view = json!({"strategies_changed":changed.len(),"strategies":changed,
        "apply_to_orders":preview["apply_to_orders"],"limits":preview["limits"],
        "snapshots":"The Station keeps each strategy's whole snapshot and compares it again at apply: any of them changing refuses the whole edit. Read a strategy with strategy_read naming its fields."});
    if !unchanged.is_empty() {
        view["unchanged"] = json!(unchanged);
        view["unchanged_means"] =
            json!("already holds these values; left out of the submission, nothing is sent for it");
    }
    view
}

/// A strategy's name from its preview row.
fn name_of(row: &Value) -> Option<&str> {
    row["fields"]["StrategyName"].as_str()
}

/// Every attribute and field whose value differs between two rows of one strategy.
///
/// Revision stamps are left out: they move on every edit and say nothing about it. A field the
/// core did not send is `null`, as `strategy_read` reports it.
fn differences(before: &Value, after: &Value, full_lists: bool) -> Map<String, Value> {
    let mut out = Map::new();
    for key in ["kind", "checked", "path"] {
        if before[key] != after[key] {
            out.insert(key.into(), pair(&before[key], &after[key], full_lists));
        }
    }
    let names: BTreeSet<&String> = [&before["fields"], &after["fields"]]
        .into_iter()
        .filter_map(Value::as_object)
        .flat_map(Map::keys)
        .collect();
    for name in names {
        let (was, is) = (&before["fields"][name], &after["fields"][name]);
        if was != is {
            out.insert(name.clone(), pair(was, is, full_lists));
        }
    }
    out
}

/// One field's before and after; a long list as its size and what entered and left it.
fn pair(was: &Value, is: &Value, full_lists: bool) -> Value {
    let long = |value: &Value| value.as_str().is_some_and(|text| text.len() > LONG_VALUE);
    if full_lists || !(long(was) || long(is)) {
        return json!({"before":was,"after":is});
    }
    let old = list_items(was.as_str().unwrap_or_default());
    let new = list_items(is.as_str().unwrap_or_default());
    let added: Vec<_> = new.difference(&old).collect();
    let removed: Vec<_> = old.difference(&new).collect();
    json!({"before":{"items":old.len()},"after":{"items":new.len()},
        "added":added.iter().take(LIST_ITEMS).collect::<Vec<_>>(),"added_count":added.len(),
        "removed":removed.iter().take(LIST_ITEMS).collect::<Vec<_>>(),"removed_count":removed.len(),
        "whole_value":"pass full_lists=true to action_preview to see it"})
}

/// The items of a comma-separated list, trimmed, upper-cased and unique — the way
/// `strategy_compare` counts a coin list, so both tools report one list the same way.
pub(super) fn list_items(value: &str) -> BTreeSet<String> {
    value
        .split(',')
        .map(|item| item.trim().to_ascii_uppercase())
        .filter(|item| !item.is_empty())
        .collect()
}

#[cfg(test)]
mod tests;
