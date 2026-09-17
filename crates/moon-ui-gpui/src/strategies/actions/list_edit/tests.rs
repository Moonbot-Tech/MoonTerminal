//! Regression tests for list semantics and the action's per-strategy draft writer.

use std::collections::HashMap;

use moon_core::feed::{SchemaField, SchemaFieldUi};

use super::{ListEdit, edit_tokens, is_list_field, stage_list_values};

/// Appending raw text instead of merging keys duplicates triggers and loses conflict precedence.
#[test]
fn append_deduplicates_keys_and_replaces_time_in_place() {
    assert_eq!(
        edit_tokens("1=300 5 7=900", "5=900 9 1=600 9=20", ListEdit::Append),
        "1=600 5=900 7=900 9=20"
    );
    assert_eq!(edit_tokens("5=300", "5", ListEdit::Append), "5");
}

/// Matching whole tokens would leave timed triggers behind when removing a bare key or new time.
#[test]
fn remove_matches_keys_with_or_without_time_and_preserves_survivor_order() {
    assert_eq!(
        edit_tokens("7=900 1 5=300 8", "5 1=99", ListEdit::Remove),
        "7=900 8"
    );
    assert_eq!(edit_tokens("5 7=300", "7=900 5", ListEdit::Remove), "");
}

/// Sorting or raw concatenation changes strategy order or leaves irregular separators behind.
#[test]
fn nonempty_edits_normalize_whitespace_without_sorting() {
    assert_eq!(
        edit_tokens(" 7\t1=20\n7=30  5 ", " 9\n8 ", ListEdit::Append),
        "7=30 1=20 5 9 8"
    );
    assert_eq!(edit_tokens(" 7\t1  ", "9", ListEdit::Remove), "7 1");
    assert_eq!(edit_tokens("", "9 1", ListEdit::Append), "9 1");
}

/// Normalizing before the empty-input guard would dirty untouched strategies on a blank submit.
#[test]
fn blank_input_is_an_exact_noop() {
    for operation in [ListEdit::Append, ListEdit::Remove] {
        for entered in ["", " \t\n"] {
            assert_eq!(edit_tokens("  5\t1  ", entered, operation), "  5\t1  ");
        }
    }
}

/// Replacing each draft with the entered text, or reusing the first value, wipes individual lists.
#[test]
fn staging_append_preserves_each_strategy_list_and_unrelated_drafts() {
    let field = "TriggerKeysBL";
    let mut drafts = HashMap::from([((3, 1, field.to_string()), "80".to_string())]);
    let current = vec![
        ((1, 1), "1=300 5".to_string()),
        ((2, 1), "7=900".to_string()),
    ];
    assert_eq!(
        stage_list_values(&mut drafts, field, &current, "9", ListEdit::Append),
        2
    );
    assert_eq!(drafts[&(1, 1, field.to_string())], "1=300 5 9");
    assert_eq!(drafts[&(2, 1, field.to_string())], "7=900 9");
    assert_eq!(drafts[&(3, 1, field.to_string())], "80");
    let next = vec![
        ((1, 1), drafts[&(1, 1, field.to_string())].clone()),
        ((2, 1), drafts[&(2, 1, field.to_string())].clone()),
    ];
    assert_eq!(
        stage_list_values(&mut drafts, field, &next, "9=20", ListEdit::Remove),
        2
    );
    assert_eq!(drafts[&(1, 1, field.to_string())], "1=300 5");
    assert_eq!(drafts[&(2, 1, field.to_string())], "7=900");
}

/// Unconditionally inserting drafts would offer Apply even for a blank action or absent key.
#[test]
fn staging_noop_creates_no_drafts() {
    let mut drafts = HashMap::new();
    let current = vec![((1, 1), "5".to_string()), ((1, 2), "7".to_string())];
    assert_eq!(
        stage_list_values(&mut drafts, "TriggerKeys", &current, " ", ListEdit::Append),
        0
    );
    assert_eq!(
        stage_list_values(&mut drafts, "TriggerKeys", &current, "9", ListEdit::Remove),
        0
    );
    assert!(drafts.is_empty());
}

/// A broad string/name match would offer destructive token actions for formulas and scalar keys.
#[test]
fn list_actions_require_known_string_edit_fields() {
    let mut field = SchemaField {
        name: "TriggerKeysBL".to_string(),
        type_name: "String".to_string(),
        ui: SchemaFieldUi::Edit,
        picklist: Vec::new(),
        default: None,
    };
    for name in [
        "TriggerKeys",
        "TriggerKeysBL",
        "ClearTriggerKeys",
        "TriggerByKey",
    ] {
        field.name = name.to_string();
        assert!(is_list_field(&field), "{name}");
    }
    for name in [
        "TriggerKey",
        "TriggerKeyBuy",
        "TriggerSecondsBL",
        "CustomEMA",
        "Comment",
        "TriggerKeysUnknown",
    ] {
        field.name = name.to_string();
        assert!(!is_list_field(&field), "{name}");
    }
    field.name = "TriggerKeysBL".to_string();
    field.type_name = "Int32".to_string();
    assert!(!is_list_field(&field));
    field.type_name = "String".to_string();
    for ui in [
        SchemaFieldUi::Checkbox,
        SchemaFieldUi::Color,
        SchemaFieldUi::Combo,
    ] {
        field.ui = ui;
        assert!(!is_list_field(&field));
    }
}
