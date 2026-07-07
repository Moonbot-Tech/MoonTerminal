//! Универсальное сохранение ширин колонок таблиц (`MoonDataTable`) в `layout.toml`.
//!
//! Любая таблица подключается двумя строками:
//! - при создании state — засеять сохранённые ширины из layout:
//!   `state.column_widths = table_persist::saved(backend, "orders-table");`
//! - в `render` (или observe-гейте) — `table_persist::persist(&backend, "orders-table", &state, cx);`
//!
//! Ключ — стабильный id таблицы (тот же, что в `MoonDataTable::new(id, …)`). Ширины идут в
//! `layout.table_column_widths` и persist'ятся общим механизмом (`layout_dirty`). Новая таблица
//! получает сохранение бесплатно — достаточно этих двух вызовов, ничего в этот модуль добавлять
//! не нужно.

use std::collections::HashMap;

use gpui::{AnyElement, App, Entity, IntoElement, SharedString};
use moon_ui::{MoonButton, MoonButtonSize, MoonDataTableState};

use crate::Backend;

/// Id хранилища ширин/полей с КОНТЕКСТОМ: `base:dock` (докнутая вкладка) либо `base:win`
/// (раскрыто в отдельном окне/открепление). Разные контексты → разные сохранённые раскладки:
/// узкой вкладке и широкому окну удобны разные ширины/наборы колонок.
pub fn ctx_id(base: &str, detached: bool) -> String {
    format!("{base}:{}", if detached { "win" } else { "dock" })
}

/// Кнопка «⤢ авто» для dock-тулбара панели: полный сброс ширин колонок таблицы `state` к
/// автофилу. Дубль к Shift+дабл-клику по разделителю. `id` — уникальный id элемента кнопки.
pub fn reset_button(id: &'static str, state: &Entity<MoonDataTableState>) -> AnyElement {
    let state = state.clone();
    MoonButton::new(SharedString::from(id))
        .ghost()
        .size(MoonButtonSize::Action)
        .label("⤢")
        .tooltip(rust_i18n::t!("tables.reset_widths").to_string())
        .on_click(move |_, _window, app| reset(&state, app))
        .render()
        .into_any_element()
}

/// Сохранённые ширины колонок таблицы `id` — для засева `MoonDataTableState.column_widths` при
/// создании панели. Пусто, если таблицу ещё не ресайзили.
pub fn saved(backend: &Backend, id: &str) -> HashMap<String, f32> {
    backend
        .layout
        .table_column_widths
        .get(id)
        .cloned()
        .unwrap_or_default()
}

/// Полный сброс ширин колонок таблицы к автофилу (кнопка «⤢ авто» / Shift+дабл-клик по
/// разделителю). Чистит `column_widths` в state; observe панели дальше уберёт запись из layout
/// через [`persist`]. No-op, если и так пусто.
pub fn reset(state: &Entity<MoonDataTableState>, cx: &mut App) {
    state.update(cx, |s, c| {
        if !s.column_widths.is_empty() {
            s.column_widths.clear();
            c.notify();
        }
    });
}

/// Сохранённый НАБОР видимых колонок таблицы `id` (ключи в каноничном порядке) — для
/// восстановления при создании/откреплении панели. `None` = ничего не сохранено (дефолт таблицы,
/// обычно «все видимы»). Часть единого дескриптора полей: наборы различаются на контекст
/// (`:dock`/`:win`), поэтому `id` — тот же `ctx_id`, что и у ширин.
pub fn visible(backend: &Backend, id: &str) -> Option<Vec<String>> {
    backend.layout.table_visible_columns.get(id).cloned()
}

/// Персистит НАБОР видимых колонок таблицы `id` (ключи в каноничном порядке). Зовётся при каждом
/// тогле видимости колонки. В layout пишет только при отличии, помечает `layout_dirty`. Пустой
/// список сюда слать НЕЛЬЗЯ (таблица без колонок) — вызывающий не даёт погасить последнюю.
pub fn set_visible(backend: &Entity<Backend>, id: &str, keys: Vec<String>, cx: &mut App) {
    backend.update(cx, |b, _| {
        if b.layout.table_visible_columns.get(id) != Some(&keys) {
            b.layout.table_visible_columns.insert(id.to_string(), keys);
            b.layout_dirty = true;
        }
    });
}

/// Персистит текущие ширины колонок таблицы `id`, если они изменились. Зовётся в `render`/observe:
/// дёшево (сравнение мапы), в layout пишет только при отличии и помечает `layout_dirty`. Пустая
/// мапа (полный сброс к автофилу) — УДАЛЯЕТ запись из layout, чтобы сброс сохранялся и таблица
/// открылась авто-ширинами.
pub fn persist(
    backend: &Entity<Backend>,
    id: &str,
    state: &Entity<MoonDataTableState>,
    cx: &mut App,
) {
    let cur = state.read(cx).column_widths.clone();
    backend.update(cx, |b, _| {
        let existing = b.layout.table_column_widths.get(id);
        if cur.is_empty() {
            if existing.is_some() {
                b.layout.table_column_widths.remove(id);
                b.layout_dirty = true;
            }
        } else if existing != Some(&cur) {
            b.layout.table_column_widths.insert(id.to_string(), cur);
            b.layout_dirty = true;
        }
    });
}
