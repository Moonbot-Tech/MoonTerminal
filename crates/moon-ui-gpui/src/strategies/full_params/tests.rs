//! Pins for locating a schema field inside a full-mode flatten and in the schema sections.

use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaSection};

use super::super::Key;
use super::super::param_entries::{ParamLabels, flatten_params};
use super::{
    FieldRevealDecision, field_reveal_decision, resolve_field_target, resolve_owning_section,
};

fn field(name: &str) -> SchemaField {
    SchemaField {
        name: name.to_string(),
        type_name: "String".to_string(),
        ui: SchemaFieldUi::Edit,
        picklist: Vec::new(),
        default: None,
    }
}

fn labels() -> ParamLabels<'static> {
    ParamLabels {
        orphans: "other",
        section_title: &|raw| raw.to_string(),
    }
}

/// `resolve_field_target` returns the first section that holds the name, and the row's entry index.
#[test]
fn a_field_in_a_later_section_resolves_there() {
    let sections = vec![
        SchemaSection {
            title: "Main".to_string(),
            fields: vec![field("StrategyName")],
        },
        SchemaSection {
            title: "Lists".to_string(),
            fields: vec![field("CoinsBlackList")],
        },
    ];
    let flat = flatten_params(&sections, None, false, None, false, labels());
    assert_eq!(
        resolve_field_target(&flat, "CoinsBlackList"),
        Some((Some(1), 3))
    );
    assert_eq!(
        resolve_field_target(&flat, "StrategyName"),
        Some((Some(0), 1))
    );
}

/// A name the flatten does not contain is not a scroll target.
#[test]
fn a_missing_field_resolves_to_nothing() {
    let sections = vec![SchemaSection {
        title: "Main".to_string(),
        fields: vec![field("StrategyName")],
    }];
    let flat = flatten_params(&sections, None, false, None, false, labels());
    assert_eq!(resolve_field_target(&flat, "CoinsBlackList"), None);
}

/// A changed name the schema no longer declares lands in the orphan group.
#[test]
fn an_orphan_field_has_no_section() {
    let sections = vec![SchemaSection {
        title: "Main".to_string(),
        fields: vec![field("StrategyName")],
    }];
    let mut changed = std::collections::HashMap::new();
    changed.insert(
        "coinsblacklist".to_string(),
        ("CoinsBlackList".to_string(), "XALPHA".to_string()),
    );
    let flat = flatten_params(&sections, Some(&changed), false, None, false, labels());
    let (section, ix) = resolve_field_target(&flat, "CoinsBlackList").expect("orphan row");
    assert_eq!(section, None);
    assert!(ix > 0);
}

/// A field the changed-only flatten drops still names the section that declares it.
///
/// Per-section reveal walks the schema by name. Routing it through `flatten_params` with the
/// version diff would drop the request: the field is absent from the flatten, so the pane would
/// stay on the wrong section.
#[test]
fn a_field_hidden_by_the_changed_filter_still_names_its_section() {
    let sections = vec![
        SchemaSection {
            title: "Main".to_string(),
            fields: vec![field("StrategyName")],
        },
        SchemaSection {
            title: "Lists".to_string(),
            fields: vec![field("CoinsBlackList")],
        },
        SchemaSection {
            title: "Again".to_string(),
            fields: vec![field("CoinsBlackList")],
        },
    ];
    let mut changed = std::collections::HashMap::new();
    changed.insert(
        "strategyname".to_string(),
        ("StrategyName".to_string(), "UNIT_01".to_string()),
    );
    let flat = flatten_params(&sections, Some(&changed), false, None, false, labels());
    assert_eq!(
        resolve_field_target(&flat, "CoinsBlackList"),
        None,
        "the changed-only flatten hides a field the diff does not name"
    );
    assert_eq!(resolve_owning_section(&sections, "CoinsBlackList"), Some(1));
    assert_eq!(resolve_owning_section(&sections, "StrategyName"), Some(0));
    assert_eq!(resolve_owning_section(&sections, "Missing"), None);
}

/// A pending field reveal is consumed only when the selected strategy is the one it named.
#[test]
fn field_reveal_decision_matches_the_selected_strategy() {
    let target: Key = (7, 11);
    assert_eq!(
        field_reveal_decision(Some(target), target),
        FieldRevealDecision::Consume
    );
    assert_eq!(
        field_reveal_decision(Some((7, 12)), target),
        FieldRevealDecision::Drop
    );
    assert_eq!(
        field_reveal_decision(None, target),
        FieldRevealDecision::Wait
    );
}
