//! What a synthetic core reports beyond its market feed: its settings and its strategies.
//!
//! Off unless `MOON_SYNTH_CORE_STATE` is set, which the UI capture build does: it photographs every
//! screen on synthetic cores, and a core that reports only ticks leaves the core settings, the
//! expert window and the Strategies window empty — a catalogue of controls nobody can see. A render
//! bench and `--fixture` measure and compare what they always did.
//!
//! **Settings** are what a fresh core holds, through the live feed's own conversions
//! (`live::default_core_settings`). **Strategies** are the ones recorded for this core's uid in
//! `strategies.sqlite`, at their latest version. Nothing here writes them back: the database's
//! writer hangs off the live feed, and `FeedMsg::Strategies` only fills the session's list.
//!
//! **The schema is derived, and that is a known difference from a real core.** It reaches the
//! terminal only over the wire, a recording holds none, and without one the editor says "no schema"
//! instead of showing a single field. So it is built from the recorded fields: one section per
//! strategy kind, in the record's order, each field's widget and type read off its value. The real
//! fields with their real values, under one heading rather than the core's own sections.
//!
//! `MOON_SYNTH_STRATEGY_SCHEMA` names a file holding the sections instead, for a run that shows the
//! editor as a core lays it out. Every kind gets them, so the file describes one kind's editor.

use std::path::Path;

use serde::Deserialize;

use super::super::{
    FeedMsg, FeedTx, SchemaField, SchemaFieldUi, SchemaKind, SchemaSection, StrategyRow,
    StrategySchemaModel,
};

/// The heading the derived schema files every field under; see the module docs.
const SECTION_TITLE: &str = "Main";

/// One editor section as `MOON_SYNTH_STRATEGY_SCHEMA`'s file writes it.
#[derive(Deserialize)]
struct WrittenSection {
    title: String,
    fields: Vec<WrittenField>,
}

/// One field as `MOON_SYNTH_STRATEGY_SCHEMA`'s file writes it.
#[derive(Deserialize)]
struct WrittenField {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
    ui: String,
    #[serde(default)]
    picklist: Vec<String>,
    #[serde(default)]
    default: Option<String>,
}

impl From<WrittenField> for SchemaField {
    fn from(field: WrittenField) -> Self {
        SchemaField {
            name: field.name,
            type_name: field.type_name,
            ui: match field.ui.as_str() {
                "Checkbox" => SchemaFieldUi::Checkbox,
                "Combo" => SchemaFieldUi::Combo,
                "Color" => SchemaFieldUi::Color,
                _ => SchemaFieldUi::Edit,
            },
            picklist: field.picklist,
            default: field.default,
        }
    }
}

/// Read the sections a file writes.
///
/// A file that cannot be read falls back to the derived schema with a line in the log: a run that
/// asked for it still shows every field, under one heading.
///
/// Args:
///     path: The file.
///
/// Returns:
///     The sections, or `None` when the file gives none.
fn written_sections(path: &Path) -> Option<Vec<SchemaSection>> {
    let read = std::fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            serde_json::from_str::<Vec<WrittenSection>>(&text).map_err(|error| error.to_string())
        });
    let sections = match read {
        Ok(sections) => sections,
        Err(error) => {
            log::warn!("synthetic feed: {} not read: {error}", path.display());
            return None;
        }
    };
    let sections: Vec<SchemaSection> = sections
        .into_iter()
        .map(|section| SchemaSection {
            title: section.title,
            fields: section.fields.into_iter().map(Into::into).collect(),
        })
        .collect();
    (!sections.is_empty()).then_some(sections)
}

/// The derived schema under the given sections.
///
/// A recorded field no section lists goes to the last one rather than off the editor: a value the
/// strategy holds must stay visible.
///
/// Args:
///     rows: The strategies.
///     written: The sections; must not be empty.
///
/// Returns:
///     One kind per ordinal, each with those sections.
fn laid_out(rows: &[StrategyRow], written: &[SchemaSection]) -> StrategySchemaModel {
    let mut schema = schema_of(rows);
    for kind in &mut schema.kinds {
        let recorded = std::mem::take(&mut kind.sections[0].fields);
        kind.sections = written.to_vec();
        for field in recorded {
            let listed = kind
                .sections
                .iter()
                .flat_map(|section| &section.fields)
                .any(|known| known.name == field.name);
            if listed {
                continue;
            }
            if let Some(last) = kind.sections.last_mut() {
                last.fields.push(field);
            }
        }
    }
    schema
}

/// Send this core's settings and strategies, once, as a live core does when it connects.
///
/// A send fails only when the terminal is shutting the feed down, and the feed's own loop notices
/// that on its next tick.
///
/// Args:
///     core_uid: The core these belong to, as the record files them.
///     tx: The feed's channel to the terminal.
///
/// Returns:
///     The id and name of this core's first strategy, for whatever else it reports to name; zero and
///     empty when the record holds none.
pub(super) fn send_core_state(core_uid: u64, tx: &FeedTx) -> (u64, String) {
    let (settings, config) = super::super::live::default_core_settings();
    let _ = tx.send(FeedMsg::ClientSettings(settings));
    let _ = tx.send(FeedMsg::CoreConfig {
        config,
        from_full_snapshot: true,
    });
    let rows = recorded_strategies(core_uid);
    log::info!(
        "synthetic feed: core {core_uid}, {} strategies from the record",
        rows.len()
    );
    let first = rows
        .first()
        .map(|row| (row.id, row.name.clone()))
        .unwrap_or_default();
    let written = std::env::var_os("MOON_SYNTH_STRATEGY_SCHEMA")
        .and_then(|path| written_sections(Path::new(&path)));
    let schema = match &written {
        Some(sections) => laid_out(&rows, sections),
        None => schema_of(&rows),
    };
    let _ = tx.send(FeedMsg::StrategySchema(schema));
    let _ = tx.send(FeedMsg::Strategies(rows));
    first
}

/// The strategies recorded for one core, at their latest versions.
///
/// A strategy with no readable version is left out rather than shown with no fields: an empty row
/// looks exactly like a broken read.
fn recorded_strategies(core_uid: u64) -> Vec<StrategyRow> {
    crate::strat_db::stats::live_heads(core_uid)
        .into_iter()
        .filter_map(|(head, checked)| {
            let fields = crate::strat_db::stats::latest_version_fields(core_uid, head.strategy_id)?;
            Some(StrategyRow {
                id: head.strategy_id as u64,
                name: head.name,
                kind: head.kind,
                kind_ordinal: head.kind_ordinal,
                folder_path: head.folder_path,
                checked,
                is_short: head.is_short,
                fields,
            })
        })
        .collect()
}

/// A schema covering the given strategies: one kind per ordinal, each with every field its
/// strategies carry, first seen first.
fn schema_of(rows: &[StrategyRow]) -> StrategySchemaModel {
    let mut kinds: Vec<SchemaKind> = Vec::new();
    for row in rows {
        let at = kinds
            .iter()
            .position(|kind| kind.ordinal == row.kind_ordinal)
            .unwrap_or_else(|| {
                kinds.push(SchemaKind {
                    ordinal: row.kind_ordinal,
                    name: row.kind.clone(),
                    sections: vec![SchemaSection {
                        title: SECTION_TITLE.to_string(),
                        fields: Vec::new(),
                    }],
                });
                kinds.len() - 1
            });
        let fields = &mut kinds[at].sections[0].fields;
        for (name, value) in &row.fields {
            if !fields.iter().any(|field| field.name == *name) {
                fields.push(field_of(name, value));
            }
        }
    }
    StrategySchemaModel { kinds }
}

/// One field of the derived schema, its type read off the value it holds.
///
/// The value is already in `fmt_field`'s form, so a boolean reads `Yes`/`No`. A text field that
/// happens to hold one of those comes out as a checkbox; the picture is what this serves, and
/// nothing a capture run does writes it back to a core. Every number is a `Double`: one value cannot
/// tell an integer field from a fractional one that happens to hold `5`, and only `Double` accepts
/// both, whatever the size.
fn field_of(name: &str, value: &str) -> SchemaField {
    let (type_name, ui) = if matches!(value, "Yes" | "No") {
        ("Bool", SchemaFieldUi::Checkbox)
    } else if value.parse::<f64>().is_ok() {
        ("Double", SchemaFieldUi::Edit)
    } else {
        ("String", SchemaFieldUi::Edit)
    };
    SchemaField {
        name: name.to_string(),
        type_name: type_name.to_string(),
        ui,
        picklist: Vec::new(),
        default: None,
    }
}

#[cfg(test)]
mod tests;
