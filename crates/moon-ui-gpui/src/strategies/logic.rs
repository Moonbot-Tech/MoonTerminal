//! Чистые помощники окна «Стратегии» (порт `strategies/mod.rs`): выбор/мультивыбор,
//! пересечение схем, значения полей и их правки, признаки полей (memo/формула) и
//! построение дерева папок. Без UI и без `cx` — только вычисления над `StrategiesView`
//! и `CoreStore`; рендер-методы живут в [`super`].

use std::collections::HashSet;

use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaSection, StrategyRow};
use moon_core::session::{CoreId, CoreStore};

use super::filter::StrategyFilter;
use super::rules::{Rules, Values};
use super::tree_ops::path_segments;
use super::{Key, StrategiesView};

/// Поиск строки стратегии в store.
pub(super) fn row(store: &CoreStore, core: CoreId, id: u64) -> Option<&StrategyRow> {
    store.core(core)?.strategies.iter().find(|s| s.id == id)
}

/// Выбранная строка стратегии (по `selected`).
pub(super) fn selected_row<'a>(
    st: &StrategiesView,
    store: &'a CoreStore,
) -> Option<&'a StrategyRow> {
    let (core, id) = st.selected?;
    row(store, core, id)
}

/// Ключи выбранных стратегий (мультивыбор) или первичная, если выбор пуст.
pub(super) fn selected_keys(st: &StrategiesView) -> Vec<Key> {
    if st.sel.is_empty() {
        st.selected.into_iter().collect()
    } else {
        st.sel.iter().copied().collect()
    }
}

pub(super) fn multi_row_pairs<'a>(
    st: &StrategiesView,
    store: &'a CoreStore,
) -> Vec<(Key, &'a StrategyRow)> {
    selected_keys(st)
        .iter()
        .filter_map(|(c, id)| row(store, *c, *id).map(|row| ((*c, *id), row)))
        .collect()
}

/// У выбранных РАЗНЫЕ виды стратегий? (тогда SignalType менять нельзя — скрываем).
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

/// Имена полей (lowercase) в схеме ядра `core` для вида `ord`.
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

/// Поля (lowercase), которые есть у ВСЕХ выбранных стратегий (пересечение схем их
/// видов). None — выбрана одна (ограничения нет, показываем всё).
pub(super) fn common_fields(st: &StrategiesView, store: &CoreStore) -> Option<HashSet<String>> {
    let keys = selected_keys(st);
    if keys.len() <= 1 {
        return None;
    }
    let mut acc: Option<HashSet<String>> = None;
    for (c, id) in keys {
        let Some(r) = row(store, c, id) else { continue };
        let set = kind_field_set(store, c, r.kind_ordinal);
        acc = Some(match acc {
            None => set,
            Some(a) => a.intersection(&set).cloned().collect(),
        });
    }
    acc
}

/// Значения полей выбранной стратегии: имя(lowercase) → значение(как есть) — для
/// вычисления зависимостей (depends_on). Несохранённые ядром поля добираем дефолтами.
pub(super) fn selected_values(st: &StrategiesView, store: &CoreStore) -> Values {
    let mut v = Values::new();
    if let Some(row) = selected_row(st, store) {
        for (name, val) in &row.fields {
            v.insert(name.to_lowercase(), val.clone());
        }
        if let Some((core, id)) = st.selected {
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

/// Раздел АКТИВЕН (не затемнён), если в нём осталось БОЛЬШЕ ОДНОГО активного поля.
pub(super) fn section_active(rules: &Rules, values: &Values, sec: &SchemaSection) -> bool {
    sec.fields
        .iter()
        .filter(|f| rules.field_active(&f.name, values))
        .count()
        > 1
}

/// Секции схемы для выбранной стратегии (по её виду). None — нет выбора/схемы.
pub(super) fn selected_sections<'a>(
    st: &StrategiesView,
    store: &'a CoreStore,
) -> Option<&'a [SchemaSection]> {
    let (core, id) = st.selected?;
    let cd = store.core(core)?;
    let row = cd.strategies.iter().find(|s| s.id == id)?;
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

/// Значение поля стратегии (по имени) или дефолт схемы.
pub(super) fn field_value(row: &StrategyRow, f: &SchemaField) -> String {
    row.fields
        .iter()
        .find(|(n, _)| n == &f.name)
        .map(|(_, v)| v.clone())
        .or_else(|| f.default.clone())
        .unwrap_or_default()
}

pub(super) fn is_on(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on")
}

pub(super) fn is_memo_field(f: &SchemaField, value: &str) -> bool {
    // Memo (многострочный редактор формул) — ТОЛЬКО для строковых полей. Числовые
    // (Int/Double/Single/…) всегда однострочный инпут, даже если в имени есть «ema»
    // (напр. trailingEma) — иначе числовое поле растягивается как формула и текст течёт.
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

/// Переключает наличие ключа в множестве (раскрыт/свёрнут).
pub(super) fn toggle<T: std::cmp::Eq + std::hash::Hash>(set: &mut HashSet<T>, key: T) {
    if !set.remove(&key) {
        set.insert(key);
    }
}

/// Виды стратегий, присутствующие в дереве: (ordinal, имя), отсортировано по имени.
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

/// Узел дерева папок: подпапки (по имени) + стратегии прямо в этой папке.
#[derive(Default)]
pub(super) struct FolderNode<'a> {
    pub(super) children: std::collections::BTreeMap<String, FolderNode<'a>>,
    pub(super) strategies: Vec<&'a StrategyRow>,
}

/// Строит вложенное дерево из путей стратегий (`/` и `\` — разделители).
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

/// Гарантирует существование узла по пути (для пустых UI-папок без стратегий).
pub(super) fn ensure_folder(root: &mut FolderNode, parts: &[String]) {
    let mut node = root;
    for part in parts {
        node = node.children.entry(part.clone()).or_default();
    }
}

/// Активных/всего (по фильтру типа/L/S) во всех стратегиях под путём `prefix`.
pub(super) fn folder_counts(
    strategies: &[StrategyRow],
    filter: &StrategyFilter,
    prefix: &[String],
) -> (usize, usize) {
    let mut active = 0;
    let mut total = 0;
    for r in strategies {
        if !filter.counts(r) {
            continue;
        }
        let parts: Vec<&str> = path_segments(&r.folder_path).collect();
        if parts.len() >= prefix.len()
            && prefix
                .iter()
                .zip(parts.iter())
                .all(|(a, b)| a.as_str() == *b)
        {
            total += 1;
            if r.checked {
                active += 1;
            }
        }
    }
    (active, total)
}
