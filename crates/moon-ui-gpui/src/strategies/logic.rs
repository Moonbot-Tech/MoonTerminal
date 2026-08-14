//! Pure helpers for Strategies-window selection, schema intersections, field values and edits,
//! memo/formula classification, folder-tree construction, and folder counts. These functions
//! perform UI-independent calculations over `StrategiesView` and `CoreStore`; rendering methods
//! live in [`super`].

use std::collections::{HashMap, HashSet};

use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaSection, StrategyRow};
use moon_core::session::{CoreId, CoreStore};

use super::filter::PreparedFilter;
use super::rules::{Rules, Values};
use super::tree::ops::path_segments;
use super::{Key, StrategiesView};
use crate::Backend;

/// Return whether one core belongs to the singleton's effective Strategies scope.
///
/// Args:
///     workspace: Concrete Auto ids, or `None` for Classic all-core behavior.
///     core: Core to test.
///
/// Returns:
///     `true` in Classic or when Auto currently exposes the core.
pub(super) fn strategy_core_is_visible(workspace: Option<&[CoreId]>, core: CoreId) -> bool {
    workspace.is_none_or(|cores| cores.contains(&core))
}

/// Filter retained strategy keys to the effective singleton scope without mutating the source.
///
/// Args:
///     keys: Retained primary or multi-selection keys.
///     workspace: Concrete Auto ids, or `None` for Classic.
///
/// Returns:
///     Only keys whose cores are currently visible.
pub(super) fn visible_strategy_keys(
    keys: impl IntoIterator<Item = Key>,
    workspace: Option<&[CoreId]>,
) -> Vec<Key> {
    keys.into_iter()
        .filter(|(core, _)| strategy_core_is_visible(workspace, *core))
        .collect()
}

/// Resolve the retained primary selection through the effective singleton scope.
pub(super) fn selected_key(st: &StrategiesView) -> Option<Key> {
    st.selected
        .filter(|(core, _)| strategy_core_is_visible(st.workspace_cores.as_deref(), *core))
}

/// Return connected strategy-tree roots constrained to the singleton workspace when present.
pub(super) fn visible_strategy_cores(
    st: &StrategiesView,
    backend: &Backend,
) -> crate::core_order::OrderedCores {
    crate::core_order::CoreOrder::new(&backend.config)
        .from_sessions(backend.session.sessions(), |session| {
            strategy_core_is_visible(st.workspace_cores.as_deref(), session.id)
        })
}

/// Return the retained folder only while its core is visible in the effective singleton scope.
pub(super) fn selected_folder(st: &StrategiesView) -> Option<(CoreId, String)> {
    st.selected_folder
        .clone()
        .filter(|(core, _)| strategy_core_is_visible(st.workspace_cores.as_deref(), *core))
}

/// Count staged checkbox changes that the current workspace may display and apply.
pub(super) fn staged_count(st: &StrategiesView) -> usize {
    st.staged
        .keys()
        .filter(|(core, _)| strategy_core_is_visible(st.workspace_cores.as_deref(), *core))
        .count()
}

/// Count field drafts that the current workspace may display and apply.
pub(super) fn field_edit_count(st: &StrategiesView) -> usize {
    st.field_edits
        .keys()
        .filter(|(core, _, _)| strategy_core_is_visible(st.workspace_cores.as_deref(), *core))
        .count()
}

/// Find a strategy row in the core store.
pub(super) fn row(store: &CoreStore, core: CoreId, id: u64) -> Option<&StrategyRow> {
    store.core(core)?.strategies.iter().find(|s| s.id == id)
}

/// Return the primary selected strategy row.
/// When the Versions panel selects a persisted read-only snapshot, returns its synthetic row built
/// from `raw_json` so the sections and parameters panels display that snapshot instead of live mode.
pub(super) fn selected_row<'a>(
    st: &'a StrategiesView,
    store: &'a CoreStore,
) -> Option<&'a StrategyRow> {
    if let Some((_, r)) = st.version_override() {
        return Some(r);
    }
    let (core, id) = selected_key(st)?;
    row(store, core, id)
}

/// Return multi-selected keys, or the primary selection when the set is empty.
pub(super) fn selected_keys(st: &StrategiesView) -> Vec<Key> {
    let retained: Vec<Key> = if st.sel.is_empty() {
        st.selected.into_iter().collect()
    } else {
        st.sel.iter().copied().collect()
    };
    visible_strategy_keys(retained, st.workspace_cores.as_deref())
}

pub(super) fn multi_row_pairs<'a>(
    st: &'a StrategiesView,
    store: &'a CoreStore,
) -> Vec<(Key, &'a StrategyRow)> {
    // A persisted snapshot view is read-only, supports one selection, and supplies the sole row.
    if let Some((key, r)) = st.version_override() {
        return vec![(key, r)];
    }
    selected_keys(st)
        .iter()
        .filter_map(|(c, id)| row(store, *c, *id).map(|row| ((*c, *id), row)))
        .collect()
}

/// Return whether selected strategies have different kinds, which hides the SignalType editor.
pub(super) fn kinds_differ(st: &StrategiesView, store: &CoreStore) -> bool {
    let mut kind: Option<u8> = None;
    for (c, id) in selected_keys(st) {
        if let Some(r) = row(store, c, id) {
            match kind {
                None => kind = Some(r.kind_ordinal),
                Some(k) if k != r.kind_ordinal => return true,
                _ => {}
            }
        }
    }
    false
}

/// Return lowercase field names from core `core`'s schema for kind ordinal `ord`.
pub(super) fn kind_field_set(store: &CoreStore, core: CoreId, ord: u8) -> HashSet<String> {
    store
        .core(core)
        .and_then(|cd| cd.schema.as_ref())
        .and_then(|sch| sch.kinds.iter().find(|k| k.ordinal == ord))
        .map(|k| {
            k.sections
                .iter()
                .flat_map(|s| &s.fields)
                .map(|f| f.name.to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

/// Return lowercase fields present in every selected strategy kind's schema.
/// Returns `None` for a single selection, meaning no intersection restriction applies.
pub(super) fn common_fields(st: &StrategiesView, store: &CoreStore) -> Option<HashSet<String>> {
    let keys = selected_keys(st);
    if keys.len() <= 1 {
        return None;
    }
    // Selections are usually many strategies of ONE kind, and each `kind_field_set` call lowercases
    // every field name in that kind's schema. Resolve each kind once instead of once per row.
    let mut by_kind: HashMap<(CoreId, u8), HashSet<String>> = HashMap::new();
    let mut acc: Option<HashSet<String>> = None;
    for (c, id) in keys {
        let Some(r) = row(store, c, id) else { continue };
        let set = by_kind
            .entry((c, r.kind_ordinal))
            .or_insert_with(|| kind_field_set(store, c, r.kind_ordinal));
        acc = Some(match acc {
            None => set.clone(),
            Some(a) => a.intersection(set).cloned().collect(),
        });
    }
    acc
}

/// Build dependency strings for the selected strategy, keyed by lowercase field name.
/// Values come from stored fields, pending edits, or schema defaults without display normalization.
pub(super) fn selected_values(st: &StrategiesView, store: &CoreStore) -> Values {
    let mut v = Values::new();
    if let Some(row) = selected_row(st, store) {
        for (name, val) in &row.fields {
            v.insert(name.to_lowercase(), val.clone());
        }
        if let Some((core, id)) = selected_key(st) {
            for ((c, sid, name), value) in &st.field_edits {
                if *c == core && *sid == id {
                    v.insert(name.to_lowercase(), value.clone());
                }
            }
        }
        if let Some(sections) = selected_sections(st, store) {
            for sec in sections {
                for f in &sec.fields {
                    v.entry(f.name.to_lowercase())
                        .or_insert_with(|| f.default.clone().unwrap_or_default());
                }
            }
        }
    }
    v
}

/// Return whether a section has more than one dependency-active field and should remain undimmed.
pub(super) fn section_active(rules: &Rules, values: &Values, sec: &SchemaSection) -> bool {
    sec.fields
        .iter()
        .filter(|f| rules.field_active(&f.name, values))
        .count()
        > 1
}

/// Return schema sections for the selected strategy kind, or `None` without a selection or schema.
/// Resolving the kind through [`selected_row`] supports both live rows and synthetic version rows,
/// including deleted strategies that no longer exist in the store.
pub(super) fn selected_sections<'a>(
    st: &'a StrategiesView,
    store: &'a CoreStore,
) -> Option<&'a [SchemaSection]> {
    let (core, _) = selected_key(st)?;
    let cd = store.core(core)?;
    let row = selected_row(st, store)?;
    let schema = cd.schema.as_ref()?;
    let kind = schema
        .kinds
        .iter()
        .find(|k| k.ordinal == row.kind_ordinal)?;
    Some(&kind.sections)
}

pub(super) fn merged_value_for_owned(
    st: &StrategiesView,
    rows: &[(Key, StrategyRow)],
    f: &SchemaField,
) -> Option<String> {
    let mut it = rows
        .iter()
        .map(|(key, row)| edited_field_value(st, *key, row, f));
    let first = it.next()?;
    if it.all(|v| v == first) {
        Some(first)
    } else {
        None
    }
}

pub(super) fn edited_field_value(
    st: &StrategiesView,
    key: Key,
    row: &StrategyRow,
    f: &SchemaField,
) -> String {
    st.field_edits
        .get(&(key.0, key.1, f.name.clone()))
        .cloned()
        .unwrap_or_else(|| field_value(row, f))
}

/// Return a named strategy field value or its schema default.
/// A numeric edit field with neither value nor default displays `0`; Moonbot cores omit fields equal
/// to their defaults, and an empty numeric input would otherwise be ambiguous.
pub(super) fn field_value(row: &StrategyRow, f: &SchemaField) -> String {
    let v = row
        .fields
        .iter()
        .find(|(n, _)| n == &f.name)
        .map(|(_, v)| v.clone())
        .or_else(|| f.default.clone())
        .unwrap_or_default();
    if v.is_empty() && f.type_name != "String" && matches!(f.ui, SchemaFieldUi::Edit) {
        return "0".to_string();
    }
    v
}

/// Discard non-hexadecimal characters and parse the final six remaining digits as RGB.
/// Moonbot `AARRGGBB` and plain `RRGGBB` are accepted; fewer than six remaining digits, or a parse
/// failure, produces gray.
pub(super) fn parse_hex_rgb(v: &str) -> [u8; 3] {
    let hex: String = v.trim().chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() < 6 {
        return [128, 128, 128];
    }
    let tail = &hex[hex.len() - 6..];
    let n = u32::from_str_radix(tail, 16).unwrap_or(0x80_80_80);
    crate::design::u32_to_rgb(n)
}

/// Return the hexadecimal prefix before the final RGB digits, usually the `FF` alpha component.
/// The color picker preserves this prefix when replacing the color.
pub(super) fn hex_alpha_prefix(v: &str) -> String {
    let hex: String = v.trim().chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() > 6 {
        hex[..hex.len() - 6].to_string()
    } else {
        String::new()
    }
}

/// Compare displayed field values semantically across case, numeric formatting, and boolean forms.
/// This treats legacy imported `YES` like live `Yes`, and values such as `1` and `1.0` as equal.
pub(super) fn values_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    if let (Ok(x), Ok(y)) = (a.parse::<f64>(), b.parse::<f64>()) {
        return (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0);
    }
    let boolish = |s: &str| match s.to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" => Some(true),
        "no" | "false" | "off" | "" => Some(false),
        _ => None,
    };
    matches!((boolish(a), boolish(b)), (Some(x), Some(y)) if x == y)
}

pub(super) fn is_on(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on")
}

pub(super) fn is_memo_field(f: &SchemaField, value: &str) -> bool {
    // Memo and multiline formula editors apply only to string fields. Numeric fields such as Int,
    // Double, and Single always use a single-line input even when their name contains `ema` (for
    // example, `trailingEma`); otherwise an ordinary numeric field expands like a formula editor.
    if f.type_name != "String" {
        return false;
    }
    if value.contains('\n') || value.chars().count() > 44 {
        return true;
    }
    is_formula_field(&f.name)
        || matches!(f.ui, SchemaFieldUi::Edit)
            && value
                .chars()
                .any(|ch| matches!(ch, '<' | '>' | '(' | ')' | '&' | '|'))
}

pub(super) fn is_formula_field(field: &str) -> bool {
    let name = field.to_ascii_lowercase();
    name.contains("custom")
        || name.contains("formula")
        || name.contains("ema")
        || name.contains("condition")
        || name.contains("filter")
}

pub(super) fn formula_snippets() -> [(&'static str, &'static str, &'static str); 10] {
    [
        ("EMA(t,i)", "token EMA", "EMA(60s, 1)"),
        ("BTC(t,i)", "BTC market EMA", "BTC(60s, 1)"),
        ("MIN(t,i)", "min price change", "MIN(15m, 1)"),
        ("MAX(t,i)", "max price change", "MAX(15m, 1)"),
        ("MAvg(t,i)", "avg of all EMAs", "MAvg(5m, 1)"),
        ("Avg(t,i)", "price average", "Avg(5m, 1)"),
        ("Vol(t,i)", "volume indicator", "Vol(5m, 1)"),
        ("Arb(ex)", "arb spread", "Arb(GateS)"),
        ("EMA short", "EMA(60s,1)<{v}", "EMA(60s, 1) < "),
        (
            "Multi-TF",
            "MIN(15m,1)<{v} AND MIN(5m,1)<{v}",
            "MIN(15m, 1) <  AND MIN(5m, 1) < ",
        ),
    ]
}

pub(super) fn field_id(field: &str) -> String {
    field
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .to_ascii_lowercase()
}

pub(super) fn editor_state_id(keys: &[Key], field: &str) -> String {
    let mut key_parts: Vec<String> = keys
        .iter()
        .map(|(core, id)| format!("{core}-{id}"))
        .collect();
    key_parts.sort();
    format!("{}:{}", field_id(field), key_parts.join(","))
}

pub(super) fn append_snippet(current: &str, snippet: &str) -> String {
    if current.trim().is_empty() {
        snippet.to_string()
    } else if current.ends_with(' ') || current.ends_with('\n') {
        format!("{current}{snippet}")
    } else {
        format!("{current} {snippet}")
    }
}

/// Toggle a key's membership in a set, such as an expanded/collapsed state set.
pub(super) fn toggle<T: std::cmp::Eq + std::hash::Hash>(set: &mut HashSet<T>, key: T) {
    if !set.remove(&key) {
        set.insert(key);
    }
}

/// Return strategy kinds present in the tree as ordinal/name pairs sorted by name.
pub(super) fn kinds_present(cores: &[(CoreId, String)], store: &CoreStore) -> Vec<(u8, String)> {
    let mut map: std::collections::BTreeMap<u8, String> = std::collections::BTreeMap::new();
    for (c, _) in cores {
        if let Some(cd) = store.core(*c) {
            for r in &cd.strategies {
                map.entry(r.kind_ordinal).or_insert_with(|| r.kind.clone());
            }
        }
    }
    let mut v: Vec<(u8, String)> = map.into_iter().collect();
    v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    v
}

/// Folder-tree node containing named child folders and strategies directly in this folder.
#[derive(Default)]
pub(super) struct FolderNode<'a> {
    pub(super) children: std::collections::BTreeMap<String, FolderNode<'a>>,
    pub(super) strategies: Vec<&'a StrategyRow>,
}

/// Build a nested tree from strategy paths, split through [`path_segments`].
pub(super) fn build_node<'a>(it: impl Iterator<Item = &'a StrategyRow>) -> FolderNode<'a> {
    let mut root = FolderNode::default();
    for r in it {
        let mut node = &mut root;
        for part in path_segments(&r.folder_path) {
            node = node.children.entry(part.to_string()).or_default();
        }
        node.strategies.push(r);
    }
    root
}

/// Ensure that a path exists for empty UI folders that contain no strategies yet.
pub(super) fn ensure_folder(root: &mut FolderNode, parts: &[String]) {
    let mut node = root;
    for part in parts {
        node = node.children.entry(part.clone()).or_default();
    }
}

/// Active and total strategy counts for every folder prefix of one core.
///
/// The accumulator keeps tree construction linear in the number of strategies: each strategy
/// contributes once to every prefix of its path, including the folder that directly contains it.
///
/// The key is the slash-joined prefix — the same string [`super::tree::moon`] uses for folder ids
/// and for the `expanded_folders` set — so these counts inherit the tree's own path aliasing and
/// introduce none of their own.
#[derive(Default)]
pub(super) struct FolderCounts {
    by_path: HashMap<String, (usize, usize)>,
    root: (usize, usize),
    /// Set for a core whose folders are not built, so no per-folder entry is worth allocating.
    totals_only: bool,
}

impl FolderCounts {
    /// Counts only the core total, for a collapsed core whose folder rows are never built.
    /// [`Self::for_path`] reports nothing in this mode and must not be consulted.
    pub(super) fn totals_only() -> Self {
        Self {
            totals_only: true,
            ..Self::default()
        }
    }

    /// Adds one strategy to its own folder and to every ancestor folder.
    ///
    /// The filter gate lives here rather than at the call site so a caller cannot accidentally
    /// count rows the captions must exclude. Only kind and direction gate a count; search text hides
    /// rows from the tree without changing any folder's `active/total`.
    pub(super) fn add(&mut self, row: &StrategyRow, filter: &PreparedFilter) {
        if !filter.counts(row) {
            return;
        }
        let hit = usize::from(row.checked);
        self.root.0 += hit;
        self.root.1 += 1;
        if self.totals_only {
            return;
        }
        let mut key = String::new();
        for seg in path_segments(&row.folder_path) {
            if !key.is_empty() {
                key.push('/');
            }
            key.push_str(seg);
            // Look up before inserting so a repeat visit to a known folder does not clone the key.
            if let Some(e) = self.by_path.get_mut(&key) {
                e.0 += hit;
                e.1 += 1;
            } else {
                self.by_path.insert(key.clone(), (hit, 1));
            }
        }
    }

    /// Returns `(active, total)` at or below a folder, or `(0, 0)` for a folder holding no
    /// strategies — the case of a UI-only folder created before its first strategy.
    pub(super) fn for_path(&self, path: &str) -> (usize, usize) {
        self.by_path.get(path).copied().unwrap_or((0, 0))
    }

    /// Returns `(active, total)` over every counted strategy, which is the core's own caption.
    pub(super) fn root(&self) -> (usize, usize) {
        self.root
    }
}

#[cfg(test)]
mod tests;
