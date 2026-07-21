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
pub fn set_field(fields: &mut Vec<(String, String)>, name: &str, value: &str) {
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
    // Тип стратегии в Moonbot = поле `SignalType` (kind-байт снапшота сервер
    // пересобирает из него при sync, см. feed/live/commands.rs). Без явной
    // записи созданная «Volumes» возвращалась с дефолтным SignalType схемы
    // (Drops) — выбранный в диалоге вид игнорировался.
    set_field(&mut fields, "SignalType", &kind.name);
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

/// База имени без суффиксов копий: срезает ХВОСТОВЫЕ « (copy)» / « (N)», сколько бы
/// их ни накопилось («S (copy) (copy)», «S (copy) (2)» → «S»). Имя целиком из
/// суффиксов не трогаем.
fn base_name(name: &str) -> &str {
    let mut s = name.trim_end();
    loop {
        let Some(open) = s.rfind(" (") else { break };
        let Some(inner) = s[open + 2..].strip_suffix(')') else {
            break;
        };
        let is_copy_suffix =
            inner == "copy" || (!inner.is_empty() && inner.bytes().all(|b| b.is_ascii_digit()));
        if !is_copy_suffix {
            break;
        }
        let head = s[..open].trim_end();
        if head.is_empty() {
            break;
        }
        s = head;
    }
    s
}

/// Уникальное имя в наборе занятых: единый формат «База (N)» с наименьшим свободным
/// N от 2. Старые суффиксы срезаются (см. [`base_name`]) — копия копии больше не
/// плодит «S (copy) (copy)», а разнобой «(copy)»/«(2)» сведён к одному виду.
pub fn unique_name(taken: &HashSet<String>, desired: &str) -> String {
    if !taken.contains(desired) {
        return desired.to_string();
    }
    let base = base_name(desired);
    for n in 2.. {
        let cand = format!("{base} ({n})");
        if !taken.contains(&cand) {
            return cand;
        }
    }
    unreachable!()
}

/// План вставки буфера в целевую папку: для каждого элемента — новая стратегия с
/// уникальным именем (коллизии и внутри самой пачки). `taken_names` — имена, уже
/// занятые в целевом наборе (любой папки целевого ядра — имена в Moonbot глобальны).
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

// --- Текстовый буфер (блокнот / обмен между пользователями) ----------------

/// Экранирование значения поля для строчного формата: `\` → `\\`, перевод строки → `\n`.
fn escape_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for ch in v.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            other => out.push(other),
        }
    }
    out
}

fn unescape_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(ch) = it.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Сериализация буфера в ТЕКСТ (кладётся в системный буфер обмена вместе с внутренним):
/// блок `[Strategy]` на стратегию, `Ключ=Значение` построчно. Самодостаточен для
/// обратной вставки ([`clip_from_text`]) в любом ядре/экземпляре терминала — так папку
/// со стратегиями можно выкинуть в блокнот и переслать целиком.
pub fn clip_to_text(clip: &[ClipItem]) -> String {
    let mut out = String::new();
    for item in clip {
        out.push_str("[Strategy]\n");
        out.push_str(&format!("Kind={}\n", escape_value(&item.kind)));
        out.push_str(&format!("KindOrdinal={}\n", item.kind_ordinal));
        if !item.rel_path.is_empty() {
            out.push_str(&format!(
                "Path={}\n",
                escape_value(&join_path(&item.rel_path))
            ));
        }
        out.push_str(&format!("Name={}\n", escape_value(&item.name)));
        for (n, v) in &item.fields {
            out.push_str(&format!("{n}={}\n", escape_value(v)));
        }
        out.push('\n');
    }
    out
}

/// Разбор текста [`clip_to_text`]. Не наш формат / битые блоки → `None`.
pub fn clip_from_text(text: &str) -> Option<Vec<ClipItem>> {
    let mut out: Vec<ClipItem> = Vec::new();
    let mut cur: Option<ClipItem> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim() == "[Strategy]" {
            if let Some(item) = cur.take() {
                out.push(item);
            }
            cur = Some(ClipItem {
                kind_ordinal: 0,
                kind: String::new(),
                name: String::new(),
                rel_path: Vec::new(),
                fields: Vec::new(),
            });
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let item = cur.as_mut()?; // содержимое до первого блока — не наш формат
        let (key, value) = line.split_once('=')?;
        let value = unescape_value(value);
        match key {
            "Kind" => item.kind = value,
            "KindOrdinal" => item.kind_ordinal = value.parse().ok()?,
            "Path" => item.rel_path = split_path(&value),
            "Name" => item.name = value,
            _ => item.fields.push((key.to_string(), value)),
        }
    }
    if let Some(item) = cur.take() {
        out.push(item);
    }
    (!out.is_empty() && out.iter().all(|i| !i.name.is_empty())).then_some(out)
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
mod tests;
