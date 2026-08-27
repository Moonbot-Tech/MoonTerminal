//! Flattened row model for the full-mode parameters pane: a pure pass from schema sections to the
//! sequence [`super::full_params`] draws, modelled on `analytics/profit_monitor/sections.rs`.
//! Nothing here touches `App`, `Window`, or `t!` -- captions that need localization are resolved
//! by the caller and passed in through [`ParamLabels`], the same seam
//! `analytics/profit_monitor/sections.rs::SectionLabels` uses, so this module stays testable
//! without a locale.

use std::collections::{HashMap, HashSet};

use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaSection};

/// One line of the virtualized full-mode parameters list.
///
/// No `PartialEq`: `SchemaField` (moon-core) does not implement it.
#[derive(Clone, Debug)]
pub(super) enum ParamEntry {
    /// Heading above one schema section's surviving fields, or above the trailing orphan group
    /// (`section: None`) when viewing a version diff.
    SectionHeader {
        section: Option<usize>,
        title: String,
        field_count: usize,
    },
    /// One field row, carrying the owning section so `full_params_list` can resolve it against
    /// `field_row`'s `compact` parameter.
    Field {
        section: Option<usize>,
        field: SchemaField,
    },
}

/// Localized captions the pure pass cannot produce on its own.
pub(super) struct ParamLabels<'a> {
    /// Caption for the trailing group of changed fields absent from the current kind's schema.
    pub(super) orphans: &'a str,
}

#[cfg(test)]
mod tests;

/// The flattened full-mode parameter list, including heading lookup metadata for section scrolling.
pub(super) struct FlatParams {
    pub(super) entries: Vec<ParamEntry>,
    /// Entry index of each schema section's heading, for the sections panel's table-of-contents
    /// scroll. Absent for a section dropped by [`flatten_params`] because every one of its fields
    /// was filtered out.
    pub(super) heading_at: HashMap<usize, usize>,
    /// Total field rows across every entry, for the header's "fields: N" count.
    pub(super) field_count: usize,
}

/// Flatten every schema section's surviving fields into one full-mode sequence, in schema order.
///
/// Args:
///     sections: Schema sections for the selected strategy kind, in schema order.
///     changed: When viewing a persisted snapshot with a diff, only fields present here (keyed by
///         lowercase name) survive, and a trailing orphan group is appended for names in `changed`
///         that do not appear in any section.
///     multi: Whether more than one strategy is selected, hiding `StrategyName`.
///     common: When selections differ in kind, only fields present here survive.
///     differ: Whether selected kinds differ, hiding `SignalType`.
///     labels: Localized captions.
///
/// Returns:
///     The flat entry sequence plus the section-heading index and total field count.
pub(super) fn flatten_params(
    sections: &[SchemaSection],
    changed: Option<&HashMap<String, (String, String)>>,
    multi: bool,
    common: Option<&HashSet<String>>,
    differ: bool,
    labels: ParamLabels<'_>,
) -> FlatParams {
    let mut entries = Vec::new();
    let mut heading_at = HashMap::new();
    let mut field_count = 0;
    // Dedup globally by lowercase field name, first section wins: `editor_state_id` keys the
    // retained editor state on the field NAME alone, so a name appearing in two schema sections
    // would give two rows one shared `MoonInputState` and two identical gpui element ids in one
    // list.
    let mut seen: HashSet<String> = HashSet::new();

    for (i, sec) in sections.iter().enumerate() {
        let mut fields: Vec<SchemaField> = Vec::new();
        for f in &sec.fields {
            let lname = f.name.to_lowercase();
            // Claim the name for THIS section before checking visibility: a field hidden here by
            // multi/common/differ still exists in the schema, so it must count as "seen" or it
            // would wrongly reappear below as an orphan (schema-absent) synthetic row.
            if !seen.insert(lname.clone()) {
                continue;
            }
            if multi && lname == "strategyname" {
                continue;
            }
            if let Some(c) = common {
                if !c.contains(&lname) {
                    continue;
                }
            }
            if differ && lname == "signaltype" {
                continue;
            }
            if let Some(ch) = changed {
                if !ch.contains_key(&lname) {
                    continue;
                }
            }
            fields.push(f.clone());
        }
        if fields.is_empty() {
            continue;
        }
        let entry_index = entries.len();
        heading_at.insert(i, entry_index);
        field_count += fields.len();
        entries.push(ParamEntry::SectionHeader {
            section: Some(i),
            title: sec.title.clone(),
            field_count: fields.len(),
        });
        entries.extend(fields.into_iter().map(|field| ParamEntry::Field {
            section: Some(i),
            field,
        }));
    }

    if let Some(ch) = changed {
        let extra = orphan_fields(ch, &seen);
        if !extra.is_empty() {
            entries.push(ParamEntry::SectionHeader {
                section: None,
                title: labels.orphans.to_string(),
                field_count: extra.len(),
            });
            field_count += extra.len();
            entries.extend(extra.into_iter().map(|field| ParamEntry::Field {
                section: None,
                field,
            }));
        }
    }

    FlatParams {
        entries,
        heading_at,
        field_count,
    }
}

/// Build the synthetic rows for changed fields the current kind's schema no longer declares.
///
/// The core can drop a field in an update, or the value can belong to another strategy kind:
/// without these rows the versions panel could report "(2)" changes while the pane displays
/// zero. Shared with [`super::params`]'s per-section "Все" body so both views synthesize the
/// same placeholder from the same rule.
///
/// Args:
///     changed: The version diff, keyed by LOWERCASE field name, valued `(display name, old)`.
///     seen: Lowercase names already placed under a real schema section.
///
/// Returns:
///     One placeholder field per unseen changed name, ordered by display name; empty when the
///     schema declared every changed field.
pub(super) fn orphan_fields(
    changed: &HashMap<String, (String, String)>,
    seen: &HashSet<String>,
) -> Vec<SchemaField> {
    let mut extra: Vec<&String> = changed
        .iter()
        .filter(|(lc, _)| !seen.contains(lc.as_str()))
        .map(|(_, (name, _))| name)
        .collect();
    extra.sort();
    extra
        .into_iter()
        .map(|name| SchemaField {
            name: name.clone(),
            type_name: "String".to_string(),
            ui: SchemaFieldUi::Edit,
            picklist: Vec::new(),
            default: None,
        })
        .collect()
}
