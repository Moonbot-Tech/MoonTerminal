//! The change journal states what an action changed, and nothing beyond it.

use std::collections::BTreeMap;

use serde_json::json;

use super::*;

fn on_core(core: u64, scopes: &[crate::station_api::mcp::Scope]) -> CoreAccess {
    CoreAccess {
        core,
        scopes: scopes.to_vec(),
    }
}

/// Widening a client's reach is the journal's most serious line: it must name each core that
/// moved, say what it moved from, and stay silent about the cores that did not.
#[test]
fn a_change_of_access_lists_only_the_cores_that_moved() {
    use crate::station_api::mcp::Scope::{Read, Trading};
    let before = [
        on_core(1, &[Read]),
        on_core(2, &[Read]),
        on_core(3, &[Read]),
    ];
    let after = [
        on_core(1, &[Read]),
        on_core(2, &[Read, Trading]),
        on_core(4, &[Read]),
    ];
    let summary = access_change(&before, &after);
    assert_eq!(
        summary.changes,
        vec![
            AuditChange {
                field: "#2".into(),
                before: Some("read".into()),
                after: Some("read, trading".into()),
            },
            AuditChange {
                field: "#3".into(),
                before: Some("read".into()),
                after: None,
            },
            AuditChange {
                field: "#4".into(),
                before: None,
                after: Some("read".into()),
            },
        ]
    );
}

fn edit() -> Action {
    Action::StrategyEdit {
        id: 5,
        fields: BTreeMap::new(),
        checked: None,
        path: None,
        apply_to_orders: false,
    }
}

/// The preview carries the whole strategy, trading parameters included. A journal built from it
/// without a diff would store them all; the revision fields would also read as a change.
#[test]
fn a_strategy_edit_keeps_only_what_it_changed() {
    let preview = json!({
        "before": {"version": 1, "last_date": 10, "checked": false, "path": "a",
            "fields": {"StrategyName": "Night", "OrderSize": 10, "StopLoss": 2.5}},
        "after": {"version": 2, "last_date": 11, "checked": true, "path": "a",
            "fields": {"StrategyName": "Night", "OrderSize": 10, "StopLoss": 3.0}},
    });
    let summary = summarize(&edit(), &preview);
    assert_eq!(summary.subject.as_deref(), Some("Night"));
    assert_eq!(
        summary.changes,
        vec![
            AuditChange {
                field: "checked".into(),
                before: Some("false".into()),
                after: Some("true".into()),
            },
            AuditChange {
                field: "StopLoss".into(),
                before: Some("2.5".into()),
                after: Some("3.0".into()),
            },
        ]
    );
}

/// An edit that also rewrites live orders is a heavier action than its field list shows.
#[test]
fn an_edit_applied_to_live_orders_says_so() {
    let action = Action::StrategyEdit {
        id: 5,
        fields: BTreeMap::new(),
        checked: None,
        path: None,
        apply_to_orders: true,
    };
    let summary = summarize(&action, &json!({"before": {}, "after": {}}));
    assert_eq!(summary.changes[0].field, "apply_to_orders");
}

/// A selected strategy can override the command's amount: the journal must state the amount the
/// order is really placed with, or it misreports the money at stake.
#[test]
fn a_new_order_records_the_effective_amount_not_the_requested_one() {
    let action = Action::OrderPlace {
        market: "BTCUSDT".into(),
        short: true,
        price: 100.5,
        balance_amount: 10.0,
        strategy_id: Some(7),
        use_market_stop: false,
        planned_sell_price: None,
    };
    let preview = json!({"initial_order_balance_amount": 250.0, "balance_currency": "USDT"});
    let summary = summarize(&action, &preview);
    assert_eq!(summary.subject.as_deref(), Some("BTCUSDT"));
    let amount = summary
        .changes
        .iter()
        .find(|change| change.field == "balance_amount")
        .expect("amount");
    assert_eq!(amount.after.as_deref(), Some("250.0 USDT"));
    assert!(
        summary
            .changes
            .iter()
            .any(|change| change.field == "side" && change.after.as_deref() == Some("SHORT"))
    );
}

/// A settings patch naming a value the setting already holds changes nothing.
#[test]
fn a_settings_patch_lists_only_values_that_differ() {
    let action = Action::SettingsPatch {
        fields: BTreeMap::from([
            ("signals.play".to_string(), json!(true)),
            ("signals.same".to_string(), json!(1)),
        ]),
    };
    let preview = json!({
        "before": {"signals.play": false, "signals.same": 1},
        "after": {"signals.play": true, "signals.same": 1},
    });
    let summary = summarize(&action, &preview);
    assert_eq!(summary.changes.len(), 1);
    assert_eq!(summary.changes[0].field, "signals.play");
}

/// Each entry shares one bounded control-channel frame with the rest of the page.
#[test]
fn a_large_edit_is_counted_past_the_bound() {
    let before: serde_json::Map<_, _> = (0..20).map(|i| (format!("f{i:02}"), json!(0))).collect();
    let after: serde_json::Map<_, _> = (0..20).map(|i| (format!("f{i:02}"), json!(1))).collect();
    let preview = json!({"before": {"fields": before}, "after": {"fields": after}});
    let summary = summarize(&edit(), &preview);
    assert_eq!(summary.changes.len(), MAX_CHANGES);
    assert_eq!(summary.omitted, 4);
}

/// Cutting by bytes would panic inside a Cyrillic character, on the Station's owner thread.
#[test]
fn a_long_value_is_clipped_on_a_character_boundary() {
    let long = "я".repeat(MAX_VALUE_CHARS + 5);
    let clipped = text(&json!(long)).expect("text");
    assert_eq!(clipped.chars().count(), MAX_VALUE_CHARS + 1);
    assert!(clipped.ends_with('…'));
}

/// A nonfinite core number arrives as JSON null: it must not read as a word "null".
#[test]
fn a_missing_value_has_no_text() {
    assert_eq!(text(&Value::Null), None);
}

/// The preview's "before" is the factual state: a panic-sell line reads as a real transition.
#[test]
fn an_order_action_names_the_market_and_the_order() {
    let action = Action::PanicSell {
        uid: 42,
        enabled: true,
    };
    let preview = json!({"before": {"market": "ETHUSDT", "panic_sell": false}});
    let summary = summarize(&action, &preview);
    assert_eq!(summary.subject.as_deref(), Some("ETHUSDT #42"));
    assert_eq!(summary.changes[0].before.as_deref(), Some("false"));
    assert_eq!(summary.changes[0].after.as_deref(), Some("true"));
}
