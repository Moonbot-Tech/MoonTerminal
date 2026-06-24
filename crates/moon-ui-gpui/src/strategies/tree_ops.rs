//! Чистая логика операций над деревом стратегий (создать/переименовать/копировать/
//! вставить/перенести/удалить). Без UI и без `cx` — только вычисления над `StrategyRow`
//! и схемой вида (`SchemaKind`). Результат — намерения (`NewStrategy` / списки
//! `(id, новый путь)`), которые слой диспетча превращает в команды `moon-core`.
//!
//! Папка существует только как ПРЕФИКС пути у стратегий (в данных пустой папки нет —
//! см. STRATEGIES_TREE_OPS_PLAN.md): все операции — это правка `folder_path`/набора.

use std::collections::HashSet;

use moon_core::feed::{SchemaKind, StrategyRow};

/// Имя поля, в котором moonproto хранит имя стратегии (`StrategySnapshot::strategy_name`).
pub const STRATEGY_NAME_FIELD: &str = "StrategyName";

/// Сегменты пути (`/` и `\` — разделители, пустые отброшены) — БЕЗ аллокаций. Единый
/// источник правила разбиения пути для всего окна (дерево/счётчики/раскрытие/операции).
pub fn path_segments(path: &str) -> impl Iterator<Item = &str> {
    path.split(['/', '\\']).filter(|s| !s.is_empty())
}

/// Разбить путь папки на владеемые сегменты (поверх [`path_segments`]).
pub fn split_path(path: &str) -> Vec<String> {
    path_segments(path).map(str::to_string).collect()
}

/// Собрать путь из сегментов (канонично через `/`).
pub fn join_path(parts: &[String]) -> String {
    parts.join("/")
}

/// `path` начинается с `prefix` (посегментно, регистр учитывается как в данных)?
fn starts_with(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && prefix.iter().zip(path).all(|(a, b)| a == b)
}

/// Все строки (включая вложенные) под префиксом пути.
pub fn rows_under<'a>(rows: &'a [StrategyRow], prefix: &[String]) -> Vec<&'a StrategyRow> {
    rows.iter()
        .filter(|r| starts_with(&split_path(&r.folder_path), prefix))
        .collect()
}

/// Правило удаления: ВСЕ затронутые стратегии выключены (`!checked`).
pub fn all_off(rows: &[&StrategyRow]) -> bool {
    rows.iter().all(|r| !r.checked)
}

// --- Создание -------------------------------------------------------------

/// Новая стратегия: вид + папка + поля (имя кладётся в поле `StrategyName`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewStrategy {
    pub kind_ordinal: u8,
    pub folder_path: String,
    pub fields: Vec<(String, String)>,
}

/// Дефолтные значения всех полей вида (из схемы; поля без дефолта — пустая строка).
pub fn default_fields(kind: &SchemaKind) -> Vec<(String, String)> {
    kind.sections
        .iter()
        .flat_map(|s| &s.fields)
        .map(|f| (f.name.clone(), f.default.clone().unwrap_or_default()))
        .collect()
}

/// Заменить (или добавить) значение поля по имени.
fn set_field(fields: &mut Vec<(String, String)>, name: &str, value: &str) {
    if let Some(slot) = fields.iter_mut().find(|(n, _)| n == name) {
        slot.1 = value.to_string();
    } else {
        fields.push((name.to_string(), value.to_string()));
    }
}

/// Построить новую стратегию заданного вида с дефолтами схемы и именем.
pub fn new_strategy(kind: &SchemaKind, name: &str, folder_path: &str) -> NewStrategy {
    let mut fields = default_fields(kind);
    set_field(&mut fields, STRATEGY_NAME_FIELD, name);
    NewStrategy {
        kind_ordinal: kind.ordinal,
        folder_path: folder_path.to_string(),
        fields,
    }
}

// --- Копирование / вставка ------------------------------------------------

/// Элемент буфера копирования: ИСХОДНЫЕ данные стратегии (не ссылка на ядро) +
/// относительный путь от базы копирования — чтобы вставлять в любое ядро/папку.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipItem {
    pub kind_ordinal: u8,
    /// Имя вида (для межъядерного предупреждения о несовместимости схем).
    pub kind: String,
    pub name: String,
    /// Путь относительно базы копирования (сегменты ниже базы; пусто — корень буфера).
    pub rel_path: Vec<String>,
    pub fields: Vec<(String, String)>,
}

fn clip_with_base(rows: &[&StrategyRow], base: &[String]) -> Vec<ClipItem> {
    rows.iter()
        .map(|r| {
            let path = split_path(&r.folder_path);
            let rel = path.get(base.len()..).unwrap_or(&[]).to_vec();
            ClipItem {
                kind_ordinal: r.kind_ordinal,
                kind: r.kind.clone(),
                name: r.name.clone(),
                rel_path: rel,
                fields: r.fields.clone(),
            }
        })
        .collect()
}

/// Снять выбранные стратегии в буфер ПЛОСКО: `rel_path` пуст у всех → при вставке копии
/// падают ПРЯМО в целевую папку (исходные пути не сохраняются — мультивыбор может быть из
/// разных папок, и пользователь ждёт копии там, куда вставляет, а не по старым путям).
pub fn copy_rows(rows: &[&StrategyRow]) -> Vec<ClipItem> {
    rows.iter()
        .map(|r| ClipItem {
            kind_ordinal: r.kind_ordinal,
            kind: r.kind.clone(),
            name: r.name.clone(),
            rel_path: Vec::new(),
            fields: r.fields.clone(),
        })
        .collect()
}

/// Снять ПАПКУ в буфер; относительный путь — от РОДИТЕЛЯ папки (имя папки сохраняется
/// при вставке, как в проводнике).
pub fn copy_folder(rows: &[StrategyRow], folder_prefix: &[String]) -> Vec<ClipItem> {
    let under = rows_under(rows, folder_prefix);
    let parent_len = folder_prefix.len().saturating_sub(1);
    clip_with_base(&under, &folder_prefix[..parent_len])
}

/// Уникальное имя в наборе занятых: `name` → `name (copy)` → `name (2)` → …
pub fn unique_name(taken: &HashSet<String>, desired: &str) -> String {
    if !taken.contains(desired) {
        return desired.to_string();
    }
    let with_copy = format!("{desired} (copy)");
    if !taken.contains(&with_copy) {
        return with_copy;
    }
    for n in 2.. {
        let cand = format!("{desired} ({n})");
        if !taken.contains(&cand) {
            return cand;
        }
    }
    unreachable!()
}

/// План вставки буфера в целевую папку: для каждого элемента — новая стратегия с
/// уникальным именем (коллизии и внутри самой пачки). `taken_names` — имена, уже
/// занятые в целевом наборе (любой папки целевого ядра — имена в MoonBot глобальны).
pub fn paste_plan(
    clip: &[ClipItem],
    target: &[String],
    taken_names: &HashSet<String>,
) -> Vec<NewStrategy> {
    let mut taken = taken_names.clone();
    let mut out = Vec::with_capacity(clip.len());
    for item in clip {
        let name = unique_name(&taken, &item.name);
        taken.insert(name.clone());
        let mut full = target.to_vec();
        full.extend(item.rel_path.iter().cloned());
        let mut fields = item.fields.clone();
        set_field(&mut fields, STRATEGY_NAME_FIELD, &name);
        out.push(NewStrategy {
            kind_ordinal: item.kind_ordinal,
            folder_path: join_path(&full),
            fields,
        });
    }
    out
}

// --- Переименование / перенос (правка folder_path существующих) -----------

/// Переименование папки: для строк под `old_prefix` вернуть `(id, новый folder_path)`,
/// заменив последний сегмент `old_prefix` на `new_name`. Прочие строки не трогаем.
pub fn rename_folder(
    rows: &[StrategyRow],
    old_prefix: &[String],
    new_name: &str,
) -> Vec<(u64, String)> {
    if old_prefix.is_empty() {
        return Vec::new();
    }
    let idx = old_prefix.len() - 1;
    rows.iter()
        .filter_map(|r| {
            let path = split_path(&r.folder_path);
            if !starts_with(&path, old_prefix) {
                return None;
            }
            let mut np = path.clone();
            np[idx] = new_name.to_string();
            Some((r.id, join_path(&np)))
        })
        .collect()
}

/// Перенос ПАПКИ под нового родителя (drag&drop): имя папки сохраняется, поддерево
/// ребейзится в `target_parent + имя + хвост`. Возвращает `(id, новый folder_path)`.
/// No-op, если цель — сама папка или её потомок (защита от зацикливания).
pub fn move_folder(
    rows: &[StrategyRow],
    folder_path: &[String],
    target_parent: &[String],
) -> Vec<(u64, String)> {
    if folder_path.is_empty() || starts_with(target_parent, folder_path) {
        return Vec::new();
    }
    let name = folder_path[folder_path.len() - 1].clone();
    rows_under(rows, folder_path)
        .iter()
        .map(|r| {
            let path = split_path(&r.folder_path);
            let rel = path.get(folder_path.len()..).unwrap_or(&[]).to_vec();
            let mut np = target_parent.to_vec();
            np.push(name.clone());
            np.extend(rel);
            (r.id, join_path(&np))
        })
        .collect()
}

/// Перенос выбранных стратегий ПЛОСКО в целевую папку: каждая → прямо в `target`
/// (исходные пути не сохраняются; мультивыбор может быть из разных папок).
pub fn move_to(rows: &[&StrategyRow], target: &[String]) -> Vec<(u64, String)> {
    let path = join_path(target);
    rows.iter().map(|r| (r.id, path.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaKind, SchemaSection, StrategyRow};

    fn row(id: u64, name: &str, path: &str, checked: bool) -> StrategyRow {
        StrategyRow {
            id,
            name: name.to_string(),
            kind: "Long".to_string(),
            kind_ordinal: 1,
            folder_path: path.to_string(),
            checked,
            is_short: false,
            fields: vec![
                (STRATEGY_NAME_FIELD.to_string(), name.to_string()),
                ("Amount".to_string(), "10".to_string()),
            ],
        }
    }

    fn field(name: &str, default: Option<&str>) -> SchemaField {
        SchemaField {
            name: name.to_string(),
            type_name: "String".to_string(),
            ui: SchemaFieldUi::Edit,
            picklist: vec![],
            default: default.map(str::to_string),
        }
    }

    fn kind() -> SchemaKind {
        SchemaKind {
            ordinal: 1,
            name: "Long".to_string(),
            sections: vec![SchemaSection {
                title: "main".to_string(),
                fields: vec![
                    field(STRATEGY_NAME_FIELD, None),
                    field("Amount", Some("100")),
                    field("Spread", Some("0.5")),
                ],
            }],
        }
    }

    #[test]
    fn split_and_join_roundtrip() {
        assert_eq!(split_path("a/b\\c"), vec!["a", "b", "c"]);
        assert_eq!(split_path("/a//b/"), vec!["a", "b"]);
        assert_eq!(split_path(""), Vec::<String>::new());
        assert_eq!(join_path(&split_path("a/b")), "a/b");
    }

    #[test]
    fn rows_under_includes_nested() {
        let rows = vec![
            row(1, "s1", "a/b", false),
            row(2, "s2", "a/b/c", false),
            row(3, "s3", "a/x", false),
        ];
        let under = rows_under(&rows, &split_path("a/b"));
        let ids: Vec<u64> = under.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn all_off_rule() {
        let on = row(1, "s1", "a", true);
        let off = row(2, "s2", "a", false);
        assert!(all_off(&[&off]));
        assert!(!all_off(&[&off, &on]));
        assert!(all_off(&[]));
    }

    #[test]
    fn new_strategy_uses_defaults_and_name() {
        let ns = new_strategy(&kind(), "My Strat", "folder/x");
        assert_eq!(ns.kind_ordinal, 1);
        assert_eq!(ns.folder_path, "folder/x");
        // имя — в поле StrategyName
        let name = ns.fields.iter().find(|(n, _)| n == STRATEGY_NAME_FIELD);
        assert_eq!(
            name,
            Some(&(STRATEGY_NAME_FIELD.to_string(), "My Strat".to_string()))
        );
        // дефолты схемы
        let amount = ns.fields.iter().find(|(n, _)| n == "Amount");
        assert_eq!(amount.map(|(_, v)| v.as_str()), Some("100"));
        let spread = ns.fields.iter().find(|(n, _)| n == "Spread");
        assert_eq!(spread.map(|(_, v)| v.as_str()), Some("0.5"));
    }

    #[test]
    fn unique_name_suffixing() {
        let mut taken = HashSet::new();
        assert_eq!(unique_name(&taken, "S"), "S");
        taken.insert("S".to_string());
        assert_eq!(unique_name(&taken, "S"), "S (copy)");
        taken.insert("S (copy)".to_string());
        assert_eq!(unique_name(&taken, "S"), "S (2)");
        taken.insert("S (2)".to_string());
        assert_eq!(unique_name(&taken, "S"), "S (3)");
    }

    #[test]
    fn copy_rows_flattens_to_target() {
        // Мультивыбор из РАЗНЫХ папок: rel_path пуст у всех → вставка кладёт всех в цель.
        let rows = vec![
            row(1, "a", "grpA/p1", false),
            row(2, "b", "grpB/sub/p2", false),
        ];
        let refs: Vec<&StrategyRow> = rows.iter().collect();
        let clip = copy_rows(&refs);
        assert!(clip.iter().all(|c| c.rel_path.is_empty()));
        // и paste_plan кладёт обе ПРЯМО в целевую папку
        let plan = paste_plan(&clip, &split_path("dest"), &HashSet::new());
        assert!(plan.iter().all(|n| n.folder_path == "dest"));
    }

    #[test]
    fn copy_folder_keeps_folder_name() {
        let rows = vec![
            row(1, "a", "parent/fld", false),
            row(2, "b", "parent/fld/sub", false),
            row(3, "c", "other", false),
        ];
        let clip = copy_folder(&rows, &split_path("parent/fld"));
        // относительно родителя parent → сохраняется сегмент fld
        let rels: Vec<Vec<String>> = clip.iter().map(|c| c.rel_path.clone()).collect();
        assert!(rels.contains(&split_path("fld")));
        assert!(rels.contains(&split_path("fld/sub")));
        assert_eq!(clip.len(), 2);
    }

    #[test]
    fn paste_plan_rebases_and_dedups() {
        let clip = vec![
            ClipItem {
                kind_ordinal: 1,
                kind: "Long".to_string(),
                name: "S".to_string(),
                rel_path: vec![],
                fields: vec![(STRATEGY_NAME_FIELD.to_string(), "S".to_string())],
            },
            ClipItem {
                kind_ordinal: 1,
                kind: "Long".to_string(),
                name: "S".to_string(),
                rel_path: split_path("sub"),
                fields: vec![(STRATEGY_NAME_FIELD.to_string(), "S".to_string())],
            },
        ];
        let mut taken = HashSet::new();
        taken.insert("S".to_string());
        let plan = paste_plan(&clip, &split_path("dest"), &taken);
        assert_eq!(plan[0].folder_path, "dest");
        assert_eq!(plan[1].folder_path, "dest/sub");
        // оба имени уникализированы, не пересекаются между собой
        let n0 = plan[0]
            .fields
            .iter()
            .find(|(n, _)| n == STRATEGY_NAME_FIELD)
            .unwrap()
            .1
            .clone();
        let n1 = plan[1]
            .fields
            .iter()
            .find(|(n, _)| n == STRATEGY_NAME_FIELD)
            .unwrap()
            .1
            .clone();
        assert_eq!(n0, "S (copy)");
        assert_eq!(n1, "S (2)");
        assert_ne!(n0, n1);
    }

    #[test]
    fn rename_folder_rewrites_matching_only() {
        let rows = vec![
            row(1, "a", "a/old", false),
            row(2, "b", "a/old/sub", false),
            row(3, "c", "a/keep", false),
        ];
        let edits = rename_folder(&rows, &split_path("a/old"), "new");
        assert_eq!(edits.len(), 2);
        assert!(edits.contains(&(1, "a/new".to_string())));
        assert!(edits.contains(&(2, "a/new/sub".to_string())));
    }

    #[test]
    fn move_folder_keeps_name_and_guards_self() {
        let rows = vec![
            row(1, "a", "src/fld", false),
            row(2, "b", "src/fld/sub", false),
            row(3, "c", "other", false),
        ];
        let edits = tree_ops_move_folder(&rows, "src/fld", "dest");
        assert!(edits.contains(&(1, "dest/fld".to_string())));
        assert!(edits.contains(&(2, "dest/fld/sub".to_string())));
        assert_eq!(edits.len(), 2);
        // в самого себя/потомка — no-op
        assert!(tree_ops_move_folder(&rows, "src/fld", "src/fld/sub").is_empty());
    }

    fn tree_ops_move_folder(
        rows: &[StrategyRow],
        folder: &str,
        target: &str,
    ) -> Vec<(u64, String)> {
        move_folder(rows, &split_path(folder), &split_path(target))
    }

    #[test]
    fn move_to_flattens_to_target() {
        // Перенос мультивыбора из разных папок → каждая прямо в целевую папку.
        let rows = vec![
            row(1, "a", "src/p1", false),
            row(2, "b", "other/grp/p2", false),
        ];
        let refs: Vec<&StrategyRow> = rows.iter().collect();
        let edits = move_to(&refs, &split_path("dest"));
        assert!(edits.contains(&(1, "dest".to_string())));
        assert!(edits.contains(&(2, "dest".to_string())));
    }
}
