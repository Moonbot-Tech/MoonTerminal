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
//! `MOON_SYNTH_STRATEGY_SCHEMA` names a file holding a core's own editor instead, one per strategy
//! kind, for a run that shows the editor as a core lays it out. Every kind the file names is sent,
//! as a live core sends all of them; a recorded field the file does not list for its kind goes to
//! that kind's last section rather than off the editor.
//!
//! **Manual-strategy buttons** are described by the core, as a live core describes them in
//! `trading.manual_strats_names` and `manual_strats_config`: a core named in
//! `MOON_SYNTH_MANUAL_BUTTONS` (comma-separated uids) puts its Manual-kind strategies on its
//! quick-select buttons, in the record's order, and shows them. Any other core shows none, so a run
//! can photograph both faces of the header: the buttons, and the picker that stands in for them.

use std::path::Path;

use serde::Deserialize;

use super::super::{
    CoreConfig, FeedMsg, FeedTx, SchemaField, SchemaFieldUi, SchemaKind, SchemaSection,
    StrategyRow, StrategySchemaModel,
};

/// The heading the derived schema files every field under; see the module docs.
const SECTION_TITLE: &str = "Main";

/// Environment variable naming the cores that show their manual strategies on buttons.
const MANUAL_BUTTONS_ENV: &str = "MOON_SYNTH_MANUAL_BUTTONS";

/// `MOON_SYNTH_STRATEGY_SCHEMA`'s file: one editor per strategy kind.
#[derive(Deserialize)]
struct WrittenSchema {
    kinds: Vec<WrittenKind>,
}

/// One strategy kind's editor as the file writes it.
#[derive(Deserialize)]
struct WrittenKind {
    ordinal: u8,
    name: String,
    sections: Vec<WrittenSection>,
}

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

/// Read the editors a file writes, one per strategy kind.
///
/// A file that cannot be read falls back to the derived schema with a line in the log: a run that
/// asked for it still shows every field, under one heading.
///
/// Args:
///     path: The file.
///
/// Returns:
///     The kinds, named as the live feed names them, or `None` when the file gives none.
fn written_kinds(path: &Path) -> Option<Vec<SchemaKind>> {
    let read = std::fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            serde_json::from_str::<WrittenSchema>(&text).map_err(|error| error.to_string())
        });
    let written = match read {
        Ok(written) => written,
        Err(error) => {
            log::warn!("synthetic feed: {} not read: {error}", path.display());
            return None;
        }
    };
    let kinds: Vec<SchemaKind> = written
        .kinds
        .into_iter()
        .map(|kind| SchemaKind {
            ordinal: kind.ordinal,
            name: super::super::strategies::schema_kind_name(kind.ordinal, &kind.name),
            sections: kind
                .sections
                .into_iter()
                .map(|section| SchemaSection {
                    title: section.title,
                    fields: section.fields.into_iter().map(Into::into).collect(),
                })
                .collect(),
        })
        .collect();
    (!kinds.is_empty()).then_some(kinds)
}

/// The written editors, with every recorded field kept.
///
/// A recorded field its kind's editor does not list goes to that editor's last section rather than
/// off it: a value the strategy holds must stay visible. A recorded kind the file does not name keeps
/// its derived editor.
///
/// Args:
///     rows: The strategies.
///     written: The editors the file writes; must not be empty.
///
/// Returns:
///     Every written kind, then any recorded kind the file lacks.
fn laid_out(rows: &[StrategyRow], written: &[SchemaKind]) -> StrategySchemaModel {
    let mut kinds = written.to_vec();
    for derived in schema_of(rows).kinds {
        let Some(kind) = kinds
            .iter_mut()
            .find(|kind| kind.ordinal == derived.ordinal)
        else {
            kinds.push(derived);
            continue;
        };
        for field in derived
            .sections
            .into_iter()
            .flat_map(|section| section.fields)
        {
            let listed = kind
                .sections
                .iter()
                .flat_map(|section| &section.fields)
                .any(|known| known.name == field.name);
            if listed {
                continue;
            }
            if kind.sections.is_empty() {
                kind.sections.push(SchemaSection {
                    title: SECTION_TITLE.to_string(),
                    fields: Vec::new(),
                });
            }
            if let Some(last) = kind.sections.last_mut() {
                last.fields.push(field);
            }
        }
    }
    StrategySchemaModel { kinds }
}

/// Whether `MOON_SYNTH_MANUAL_BUTTONS` names this core.
fn shows_manual_buttons(core_uid: u64) -> bool {
    std::env::var(MANUAL_BUTTONS_ENV).is_ok_and(|list| {
        list.split(',')
            .any(|uid| uid.trim().parse::<u64>().is_ok_and(|uid| uid == core_uid))
    })
}

/// Put a core's Manual-kind strategies on its quick-select buttons, as Moonbot's own config would.
///
/// The first ten in the record's order, each shown; the slots past them stay empty and hidden. A core
/// with no Manual-kind strategy is left as it is: it has nothing a button could fire.
///
/// Args:
///     config: The core's config, as sent.
///     rows: The core's strategies.
fn put_manual_on_buttons(config: &mut CoreConfig, rows: &[StrategyRow]) {
    let manual = moonproto::StrategyKind::MANUAL.ordinal();
    let names: Vec<&str> = rows
        .iter()
        .filter(|row| row.kind_ordinal == manual)
        .map(|row| row.name.as_str())
        .take(config.manual.strat_names.len())
        .collect();
    if names.is_empty() {
        return;
    }
    config.manual.strat_buttons.use_buttons = true;
    for (slot, name) in names.into_iter().enumerate() {
        config.manual.strat_names[slot] = name.to_string();
        config.manual.strat_buttons.show_button[slot] = true;
    }
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
    let rows = recorded_strategies(core_uid);
    log::info!(
        "synthetic feed: core {core_uid}, {} strategies from the record",
        rows.len()
    );
    let (settings, mut config) = super::super::live::default_core_settings();
    if shows_manual_buttons(core_uid) {
        put_manual_on_buttons(&mut config, &rows);
    }
    let _ = tx.send(FeedMsg::ClientSettings(settings));
    let _ = tx.send(FeedMsg::CoreConfig {
        config,
        from_full_snapshot: true,
    });
    let first = rows
        .first()
        .map(|row| (row.id, row.name.clone()))
        .unwrap_or_default();
    let written = std::env::var_os("MOON_SYNTH_STRATEGY_SCHEMA")
        .and_then(|path| written_kinds(Path::new(&path)));
    let schema = match &written {
        Some(kinds) => laid_out(&rows, kinds),
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
