//! What an applied action changed, in the owner's change journal.
//!
//! Built from the preview the action was approved against, so the journal states the same
//! before/after the AI client saw. Only changed values are kept: a strategy's or a settings
//! snapshot's untouched fields stay out of the store.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::Action;
use crate::station_api::mcp::{AuditChange, CoreAccess};

/// Most fields one journal entry keeps; the rest are counted. Bounds the owner's status reply,
/// which shares one control-channel frame with everything else on the page.
const MAX_CHANGES: usize = 16;
/// Longest value kept, in characters. A strategy's text field can hold a whole list.
const MAX_VALUE_CHARS: usize = 120;

/// The journal's account of one action.
#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    /// What the action targeted: a strategy's name, a market, an order.
    pub subject: Option<String>,
    pub changes: Vec<AuditChange>,
    /// Changed fields beyond [`MAX_CHANGES`].
    pub omitted: u32,
}

/// Summarize an action from the preview it was approved against.
///
/// Args:
///     action: The action being applied.
///     preview: The preview retained for it.
///
/// Returns:
///     The subject and the bounded list of changed values.
pub fn summarize(action: &Action, preview: &Value) -> Summary {
    let before = &preview["before"];
    let after = &preview["after"];
    let (subject, changes) = match action {
        Action::StrategyEdit {
            apply_to_orders, ..
        } => {
            let mut changes = strategy_diff(before, after);
            // Not a strategy value, but it rewrites live orders: the entry must not read as a
            // settings change alone.
            if *apply_to_orders {
                changes.push(change("apply_to_orders", &Value::Null, &json!(true)));
            }
            (
                strategy_name(after).or_else(|| strategy_name(before)),
                changes,
            )
        }
        Action::StrategyCreate { name, fields, .. } => (
            Some(name.clone()),
            fields
                .iter()
                .map(|(field, requested)| {
                    let saved = &after["fields"][field];
                    change(
                        field,
                        &Value::Null,
                        if saved.is_null() { requested } else { saved },
                    )
                })
                .collect(),
        ),
        Action::StrategyDelete { .. } => (strategy_name(before), Vec::new()),
        Action::SettingsPatch { fields } => (
            None,
            fields
                .keys()
                .filter(|field| before[field.as_str()] != after[field.as_str()])
                .map(|field| change(field, &before[field.as_str()], &after[field.as_str()]))
                .collect(),
        ),
        Action::Trading { running } => (
            None,
            vec![change(
                "strategies_running",
                &before["strategies_running"],
                &json!(running),
            )],
        ),
        Action::Autodetect { enabled } => (
            None,
            vec![change("autodetect", &before["autodetect"], &json!(enabled))],
        ),
        Action::HedgeMode { enabled } => (
            None,
            vec![change("hedge_mode", &before["hedge_mode"], &json!(enabled))],
        ),
        Action::RuntimeRestart => (
            None,
            vec![change(
                "runtime_started",
                &before["runtime_started"],
                &json!(true),
            )],
        ),
        Action::OrderPlace {
            market,
            short,
            price,
            strategy_id,
            planned_sell_price,
            ..
        } => {
            let mut changes = vec![
                change(
                    "side",
                    &Value::Null,
                    &json!(if *short { "SHORT" } else { "LONG" }),
                ),
                change("price", &Value::Null, &json!(price)),
                // The preview's amount, not the command's: a selected strategy can override it.
                change(
                    "balance_amount",
                    &Value::Null,
                    &json!(format!(
                        "{} {}",
                        text(&preview["initial_order_balance_amount"]).unwrap_or_default(),
                        text(&preview["balance_currency"]).unwrap_or_default(),
                    )),
                ),
            ];
            if let Some(sell) = planned_sell_price {
                changes.push(change("planned_sell_price", &Value::Null, &json!(sell)));
            }
            if let Some(id) = strategy_id {
                changes.push(change("strategy_id", &Value::Null, &json!(id.to_string())));
            }
            (Some(market.clone()), changes)
        }
        Action::OrderCancel { uid } => (Some(order_subject(before, *uid)), Vec::new()),
        // Which of an order's prices moves depends on its phase, so no "before" is claimed.
        Action::OrderMove { uid, price } => (
            Some(order_subject(before, *uid)),
            vec![change("price", &Value::Null, &json!(price))],
        ),
        Action::PanicSell { uid, enabled } => (
            Some(order_subject(before, *uid)),
            vec![change("panic_sell", &before["panic_sell"], &json!(enabled))],
        ),
        Action::PositionClose {
            market,
            aggressive_limit,
        } => (
            Some(market.clone()),
            vec![change(
                "aggressive_limit",
                &Value::Null,
                &json!(aggressive_limit),
            )],
        ),
        Action::Leverage { market, leverage } => (
            Some(market.clone()),
            vec![change("leverage", &before["leverage"], &json!(leverage))],
        ),
    };
    bounded(subject, changes)
}

/// What the owner changed in a client's permissions: one change per core whose permissions differ,
/// so widening a client's reach is on record like anything that client does with it.
///
/// Args:
///     before: The grant's permissions until now.
///     after: The grant's permissions from now on.
///
/// Returns:
///     The changed cores, as `#uid` fields, bounded like any other entry.
pub fn access_change(before: &[CoreAccess], after: &[CoreAccess]) -> Summary {
    let scopes_on = |list: &[CoreAccess], core: u64| {
        list.iter().find(|entry| entry.core == core).map(|entry| {
            entry
                .scopes
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
    };
    let cores: BTreeSet<u64> = before.iter().chain(after).map(|entry| entry.core).collect();
    let changes = cores
        .into_iter()
        .filter_map(|core| {
            let was = scopes_on(before, core);
            let is = scopes_on(after, core);
            (was != is).then(|| AuditChange {
                field: format!("#{core}"),
                before: was,
                after: is,
            })
        })
        .collect();
    bounded(None, changes)
}

/// Keep the first [`MAX_CHANGES`] changes and count the rest.
fn bounded(subject: Option<String>, mut changes: Vec<AuditChange>) -> Summary {
    let omitted = changes.len().saturating_sub(MAX_CHANGES);
    changes.truncate(MAX_CHANGES);
    Summary {
        subject: subject.map(|name| clipped(&name)),
        changes,
        omitted: u32::try_from(omitted).unwrap_or(u32::MAX),
    }
}

/// A strategy's name from its preview row.
fn strategy_name(row: &Value) -> Option<String> {
    row["fields"]["StrategyName"].as_str().map(str::to_owned)
}

/// The fields a strategy edit changes, plus its enabled flag and folder. Revision numbers move on
/// every edit and say nothing to the owner, so they stay out.
fn strategy_diff(before: &Value, after: &Value) -> Vec<AuditChange> {
    let mut changes = Vec::new();
    for field in ["checked", "path"] {
        if before[field] != after[field] {
            changes.push(change(field, &before[field], &after[field]));
        }
    }
    let old = before["fields"].as_object();
    let new = after["fields"].as_object();
    let names = old
        .into_iter()
        .chain(new)
        .flat_map(|fields| fields.keys())
        .collect::<std::collections::BTreeSet<_>>();
    for name in names {
        let was = &before["fields"][name.as_str()];
        let is = &after["fields"][name.as_str()];
        if was != is {
            changes.push(change(name, was, is));
        }
    }
    changes
}

/// An order as the journal names it, from the preview's factual order row.
fn order_subject(order: &Value, uid: u64) -> String {
    match order["market"].as_str() {
        Some(market) => format!("{market} #{uid}"),
        None => format!("#{uid}"),
    }
}

/// One journal line.
fn change(field: &str, before: &Value, after: &Value) -> AuditChange {
    AuditChange {
        field: field.to_owned(),
        before: text(before),
        after: text(after),
    }
}

/// A value as the owner reads it: a JSON null, which also stands for a nonfinite core number, has
/// no text.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(clipped(text)),
        other => Some(clipped(&other.to_string())),
    }
}

/// Cut a value at [`MAX_VALUE_CHARS`] characters, never inside a UTF-8 sequence.
fn clipped(text: &str) -> String {
    match text.char_indices().nth(MAX_VALUE_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests;
