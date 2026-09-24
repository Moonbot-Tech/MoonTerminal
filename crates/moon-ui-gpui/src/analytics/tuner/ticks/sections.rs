//! The rows of the Entry/Exit grid, laid out by the strategy editor's sections — Strategy
//! settings, Stops, Sell order, SellShot, SellSpread, Delta Modifiers — with EVERY field each
//! section holds for the scope's kinds, as the Strategies window shows them. Only the knobs of
//! [`TICK_PARAMS`](moon_core::db::tuner::ticks::TICK_PARAMS) are searched; every other field is
//! drawn fixed, so what the model does not turn yet stays in sight where the user looks for it.
//! The sections the model does not have at all — SellShot and SellSpread
//! ([`ParamSection::modelled`]) — keep their fields in sight too, every one of them inactive.
//!
//! The field lists come from the live schema of each deal's strategy kind — the store's strategy
//! row gives the kind ordinal, as `strategies::logic::selected_sections` does; the `SignalType`
//! the deals carry is spelled differently from the schema's kind names (`PumpsDetection`). With
//! no connected core holding a schema, only the knobs are drawn, under their
//! [`TickParam::section`].

use std::collections::{HashMap, HashSet};

use moon_core::db::tuner::ticks::params::{ParamSection, TickParam, is_model_only, params_for};
use moon_core::feed::SchemaSection;
use moon_core::session::CoreStore;

use crate::strategies::sections::section_title_eq;

#[cfg(test)]
mod tests;

/// What a row of the grid is to the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::analytics::tuner) enum RowRole {
    /// A field the search can turn.
    Knob(&'static TickParam),
    /// A field the model reads at the strategy's value and does not turn.
    Fixed,
    /// A field the model does not take into account.
    Outside,
    /// A field of a section the model does not have at all ([`ParamSection::modelled`]): a
    /// strategy that switches the section on is not judged.
    Unmodelled,
}

/// One row: the field as the schema spells it, and its role.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics::tuner) struct GridRow {
    pub(in crate::analytics::tuner) key: String,
    pub(in crate::analytics::tuner) role: RowRole,
}

/// One section of the grid with its rows in the schema's order.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics::tuner) struct GridSection {
    pub(in crate::analytics::tuner) section: ParamSection,
    pub(in crate::analytics::tuner) rows: Vec<GridRow>,
}

impl GridSection {
    /// The knobs of the section.
    pub(in crate::analytics::tuner) fn knobs(
        &self,
    ) -> impl Iterator<Item = &'static TickParam> + '_ {
        self.rows.iter().filter_map(|r| match r.role {
            RowRole::Knob(p) => Some(p),
            _ => None,
        })
    }
}

/// The knobs the scope's kinds understand — the union over the kinds present, in descriptor
/// order.
pub(in crate::analytics::tuner) fn scope_knobs(kinds: &[String]) -> Vec<&'static TickParam> {
    moon_core::db::tuner::ticks::TICK_PARAMS
        .iter()
        .filter(|f| {
            kinds
                .iter()
                .any(|k| params_for(f.group, k).any(|g| g.key == f.key))
        })
        .collect()
}

/// The grid's sections, every one of [`ParamSection::GRID_ORDER`] in that order, empty ones
/// included.
///
/// A field goes where the first kind's schema that has it files it; a field two sections share
/// is drawn once. A knob no schema places goes under its own [`TickParam::section`].
///
/// Args:
///     kind_sections: The schema sections of each kind in the scope.
///     knobs: The knobs of the scope ([`scope_knobs`]).
pub(in crate::analytics::tuner) fn layout(
    kind_sections: &[&[SchemaSection]],
    knobs: &[&'static TickParam],
) -> Vec<GridSection> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<GridSection> = ParamSection::GRID_ORDER
        .iter()
        .map(|&section| GridSection {
            section,
            rows: Vec::new(),
        })
        .collect();
    for grid in &mut out {
        let title = grid.section.schema_title();
        for sections in kind_sections {
            for section in sections
                .iter()
                .filter(|s| section_title_eq(&s.title, title))
            {
                for field in &section.fields {
                    if seen.insert(field.name.to_ascii_lowercase()) {
                        grid.rows.push(row(&field.name, grid.section, knobs));
                    }
                }
            }
        }
    }
    for &knob in knobs {
        if seen.insert(knob.key.to_ascii_lowercase())
            && let Some(grid) = out.iter_mut().find(|g| g.section == knob.section)
        {
            grid.rows.push(GridRow {
                key: knob.key.to_string(),
                role: RowRole::Knob(knob),
            });
        }
    }
    out
}

/// A schema field's row: a knob of the scope, a field the model reads, or one it does not —
/// and every field of a section the model does not have, whatever the field.
fn row(name: &str, section: ParamSection, knobs: &[&'static TickParam]) -> GridRow {
    let role = match knobs.iter().find(|k| k.key.eq_ignore_ascii_case(name)) {
        _ if !section.modelled() => RowRole::Unmodelled,
        Some(&knob) => RowRole::Knob(knob),
        None if is_model_only(name) => RowRole::Fixed,
        None => RowRole::Outside,
    };
    let key = match role {
        RowRole::Knob(knob) => knob.key.to_string(),
        _ => name.to_string(),
    };
    GridRow { key, role }
}

/// The schema sections of each distinct kind the strategies of `pairs` are, by
/// `(strategy_id, core_uid)`. A strategy on a core without a schema, or no longer in the core's
/// list, gives nothing.
pub(in crate::analytics::tuner) fn scope_schema(
    store: &CoreStore,
    pairs: impl IntoIterator<Item = (i64, u64)>,
) -> Vec<&[SchemaSection]> {
    let mut by_core: HashMap<u64, HashSet<i64>> = HashMap::new();
    for (strategy, core) in pairs {
        by_core.entry(core).or_default().insert(strategy);
    }
    let mut kinds: HashSet<(u64, u8)> = HashSet::new();
    let mut out = Vec::new();
    for (core, strategies) in by_core {
        let Some(data) = store.core(core) else {
            continue;
        };
        let Some(schema) = data.schema.as_ref() else {
            continue;
        };
        for row in &data.strategies {
            if !strategies.iter().any(|&id| same_strategy(row.id, id)) {
                continue;
            }
            if !kinds.insert((core, row.kind_ordinal)) {
                continue;
            }
            if let Some(kind) = schema.kinds.iter().find(|k| k.ordinal == row.kind_ordinal) {
                out.push(kind.sections.as_slice());
            }
        }
    }
    out
}

/// Every field name the grid's sections hold in any kind of any connected core's schema — the
/// keys the "now" column reads beside the models' own, so a fixed row shows the strategy's value.
pub(in crate::analytics::tuner) fn schema_keys(store: &CoreStore) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    for (_, core) in store.cores() {
        let Some(schema) = core.schema.as_ref() else {
            continue;
        };
        for kind in &schema.kinds {
            for section in kind.sections.iter().filter(|s| {
                ParamSection::GRID_ORDER
                    .iter()
                    .any(|g| section_title_eq(&s.title, g.schema_title()))
            }) {
                seen.extend(section.fields.iter().map(|f| f.name.as_str()));
            }
        }
    }
    seen.into_iter().map(str::to_string).collect()
}

/// A signature of the schemas the store holds: which cores have one, at which revision. It moves
/// whenever a core's schema arrives, changes or goes, and is independent of the order the store
/// lists its cores in.
pub(in crate::analytics::tuner) fn schema_signature(store: &CoreStore) -> u64 {
    store
        .cores()
        .filter(|(_, core)| core.schema.is_some())
        .map(|(id, core)| {
            (id ^ core.schema_rev.rotate_left(32)).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        })
        .fold(0u64, u64::wrapping_add)
}

/// Whether a store strategy row (`id` as the core sends it) is the report's `strategyid`.
///
/// The report keeps the core's `u64` in a signed column, so an id past `i64::MAX` reads back
/// negative: the bits are the same, the value is not (as `tuner/mod.rs` maps `live_id`).
fn same_strategy(row_id: u64, strategy_id: i64) -> bool {
    row_id == strategy_id as u64
}
