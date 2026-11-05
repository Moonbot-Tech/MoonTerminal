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

use super::super::{
    FeedMsg, FeedTx, SchemaField, SchemaFieldUi, SchemaKind, SchemaSection, StrategyRow,
    StrategySchemaModel,
};

/// The heading the derived schema files every field under; see the module docs.
const SECTION_TITLE: &str = "Main";

/// Send this core's settings and strategies, once, as a live core does when it connects.
///
/// A send fails only when the terminal is shutting the feed down, and the feed's own loop notices
/// that on its next tick.
///
/// Args:
///     core_uid: The core these belong to, as the record files them.
///     tx: The feed's channel to the terminal.
pub(super) fn send_core_state(core_uid: u64, tx: &FeedTx) {
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
    let _ = tx.send(FeedMsg::StrategySchema(schema_of(&rows)));
    let _ = tx.send(FeedMsg::Strategies(rows));
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
