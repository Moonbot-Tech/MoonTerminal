//! The rows of the Entry/Exit grid, laid out by the strategy editor's sections — Strategy
//! settings, Stops, Sell order, SellShot, SellSpread, Delta Modifiers — with the fields each
//! section holds for the scope's kinds, as the Strategies window shows them. Only the knobs of
//! [`TICK_PARAMS`](moon_core::db::tuner::ticks::TICK_PARAMS) are searched, and they are always
//! drawn; every other field is drawn fixed, and only while a strategy of the scope switches it
//! on (`unmodelled::fields_in_use`: its rule holds and it is off its default), so what the model
//! does not turn yet stays in sight where the user looks for it, and what no strategy uses does
//! not crowd it. The sections the model does not have at all — SellShot and SellSpread
//! ([`ParamSection::modelled`]) — follow the same rule, every row of them inactive; a section
//! left without a row is not drawn.
//!
//! The field lists come from the live schema of each deal's strategy kind — the store's strategy
//! row gives the kind ordinal, as `strategies::logic::selected_sections` does; the `SignalType`
//! the deals carry is spelled differently from the schema's kind names (`PumpsDetection`). With
//! no connected core holding a schema, only the knobs are drawn, under their
//! [`TickParam::section`].

use std::collections::{HashMap, HashSet};

use moon_core::db::tuner::ticks::Deal;
use moon_core::db::tuner::ticks::params::{ParamSection, TickParam, is_model_only, params_for};
use moon_core::db::tuner::ticks::unmodelled::fields_in_use;
use moon_core::feed::SchemaSection;
use moon_core::feed::strategy_deps::FieldDeps;
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

/// The grid's rows for the deals' kinds, by section: the fields the live schema files under
/// each of the kinds that a strategy of the scope switches on, every knob, the knobs no schema
/// places under their own section.
///
/// Args:
///     store: The connected cores, for their schemas and strategy lists.
///     deals: The scope's deals.
///     strategies: Every strategy of the scope with its values, by `(strategy_id, core_uid)` —
///         the deals' own and the selected ones. One whose kind the store cannot tell — no core
///         given, or its core no longer lists it (renamed or deleted since the trade) — is read
///         against every kind of the scope, so a field it switches on is not lost.
///     deps: The fields' dependency rules, read for this load.
pub(in crate::analytics::tuner) fn grid_for<'a>(
    store: &CoreStore,
    deals: &[Deal],
    strategies: impl IntoIterator<Item = ((i64, Option<u64>), &'a HashMap<String, String>)>,
    deps: &FieldDeps,
) -> Vec<GridSection> {
    let mut kinds: Vec<String> = Vec::new();
    for deal in deals {
        if !kinds.contains(&deal.kind) {
            kinds.push(deal.kind.clone());
        }
    }
    let knobs = scope_knobs(&kinds);
    let schema = scope_schema(store, deals.iter().map(|d| (d.strategy_id, d.core_uid)));
    let mut in_use: HashSet<String> = HashSet::new();
    for ((sid, core), values) in strategies {
        match core.and_then(|core| strategy_schema(store, sid, core)) {
            Some(sections) => in_use.extend(fields_in_use(sections, values, deps)),
            None => {
                for sections in &schema {
                    in_use.extend(fields_in_use(sections, values, deps));
                }
            }
        }
    }
    layout(&schema, &knobs, &in_use)
}

/// The grid's sections, every one of [`ParamSection::GRID_ORDER`] in that order, empty ones
/// included — the grid drops those once the scope is known.
///
/// A field goes where the first kind's schema that has it files it; a field two sections share
/// is drawn once. A knob no schema places goes under its own [`TickParam::section`]. A field
/// that is not a knob is drawn only when `in_use` holds it.
///
/// Args:
///     kind_sections: The schema sections of each kind in the scope.
///     knobs: The knobs of the scope ([`scope_knobs`]).
///     in_use: The fields some strategy of the scope switches on, lowercase
///         (`unmodelled::fields_in_use`).
pub(in crate::analytics::tuner) fn layout(
    kind_sections: &[&[SchemaSection]],
    knobs: &[&'static TickParam],
    in_use: &HashSet<String>,
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
                    let key = field.name.to_ascii_lowercase();
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    let row = row(&field.name, grid.section, knobs);
                    if matches!(row.role, RowRole::Knob(_)) || in_use.contains(&key) {
                        grid.rows.push(row);
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

/// The schema sections of one strategy's kind, when its core is connected with a schema and
/// still lists the strategy.
fn strategy_schema(store: &CoreStore, strategy: i64, core: u64) -> Option<&[SchemaSection]> {
    let data = store.core(core)?;
    let schema = data.schema.as_ref()?;
    let row = data
        .strategies
        .iter()
        .find(|row| same_strategy(row.id, strategy))?;
    schema
        .kinds
        .iter()
        .find(|k| k.ordinal == row.kind_ordinal)
        .map(|k| k.sections.as_slice())
}

/// Every field name any kind of any connected core's schema holds — the keys the "now" column
/// reads beside the models' own, so a fixed row shows the strategy's value, and the ones the
/// grid's rows are chosen by ([`grid_for`]): a field's rule may read a field of a section the grid
/// does not draw (`HODLmode`), and one left unread would stand at its default.
pub(in crate::analytics::tuner) fn schema_keys(store: &CoreStore) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    for (_, core) in store.cores() {
        let Some(schema) = core.schema.as_ref() else {
            continue;
        };
        for kind in &schema.kinds {
            for section in &kind.sections {
                seen.extend(section.fields.iter().map(|f| f.name.as_str()));
            }
        }
    }
    seen.into_iter().map(str::to_string).collect()
}

/// The number knobs every connected core's schema types as an integer (`Int32`, `Int64`) —
/// wherever it lists them at all: a typed range on such a field is cut to whole numbers
/// (`params::range::resolve`). A field one schema types otherwise is not among them.
pub(in crate::analytics::tuner) fn integer_keys(store: &CoreStore) -> HashSet<&'static str> {
    let mut integer: HashSet<&'static str> = HashSet::new();
    let mut other: HashSet<&'static str> = HashSet::new();
    let knob = |name: &str| {
        moon_core::db::tuner::ticks::TICK_PARAMS
            .iter()
            .find(|f| f.key.eq_ignore_ascii_case(name))
            .map(|f| f.key)
    };
    for (_, core) in store.cores() {
        let Some(schema) = core.schema.as_ref() else {
            continue;
        };
        for field in schema
            .kinds
            .iter()
            .flat_map(|k| &k.sections)
            .flat_map(|s| &s.fields)
        {
            let Some(key) = knob(&field.name) else {
                continue;
            };
            if field.type_name.starts_with("Int") {
                integer.insert(key);
            } else {
                other.insert(key);
            }
        }
    }
    integer.retain(|key| !other.contains(key));
    integer
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
