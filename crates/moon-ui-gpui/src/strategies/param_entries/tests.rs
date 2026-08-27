//! Unit tests for the flattened full-mode parameter entries.

use std::collections::HashMap;

use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaSection};

use super::{ParamEntry, ParamLabels, flatten_params};

/// Build a schema field with only the metadata the flattened model reads.
///
/// Args:
///     name: Field name supplied by the schema fixture.
///
/// Returns:
///     An editable string field suitable for the flattened model fixture.
fn field(name: &str) -> SchemaField {
    SchemaField {
        name: name.to_string(),
        type_name: "String".to_string(),
        ui: SchemaFieldUi::Edit,
        picklist: Vec::new(),
        default: None,
    }
}

/// Extract the rendered field name together with the schema section that owns it.
///
/// Args:
///     entries: Flattened rows returned by the production model.
///
/// Returns:
///     Each field row's owning section and name, omitting headings.
fn flattened_fields(entries: &[ParamEntry]) -> Vec<(Option<usize>, String)> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            ParamEntry::Field { section, field } => Some((*section, field.name.clone())),
            ParamEntry::SectionHeader { .. } => None,
        })
        .collect()
}

/// `param_entries.rs::flatten_params` must not ignore `changed`: doing so shows unchanged fields
/// beneath a version-diff banner and makes the user read the whole strategy as a diff.
#[test]
fn version_full_mode_keeps_only_changed_fields_and_groups_orphans() {
    let sections = vec![
        SchemaSection {
            title: "Main".to_string(),
            fields: vec![field("ChangedMain"), field("UnchangedMain")],
        },
        SchemaSection {
            title: "Rules".to_string(),
            fields: vec![field("UnchangedRule"), field("ChangedRule")],
        },
    ];
    let changed = HashMap::from([
        (
            "changedmain".to_string(),
            ("ChangedMain".to_string(), "old".to_string()),
        ),
        (
            "changedrule".to_string(),
            ("ChangedRule".to_string(), "old".to_string()),
        ),
        (
            "retiredfield".to_string(),
            ("RetiredField".to_string(), "old".to_string()),
        ),
    ]);

    let flat = flatten_params(
        &sections,
        Some(&changed),
        false,
        None,
        false,
        ParamLabels {
            orphans: "Other fields",
        },
    );

    assert_eq!(
        flattened_fields(&flat.entries),
        vec![
            (Some(0), "ChangedMain".to_string()),
            (Some(1), "ChangedRule".to_string()),
            (None, "RetiredField".to_string()),
        ],
        "the independently declared version diff is the only source for surviving field rows"
    );
    assert_eq!(flat.heading_at.get(&0), Some(&0));
    assert_eq!(flat.heading_at.get(&1), Some(&2));
    assert_eq!(flat.field_count, changed.len());
}
