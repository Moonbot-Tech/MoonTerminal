//! Table persistence helpers for `layout.toml`.
//!
//! After choosing a stable persistence ID, a table restores widths when it creates state:
//! `state.column_widths = table_persist::saved(backend, &id);`
//! It then calls `table_persist::persist(&backend, &id, &state, cx)` from the state observer or
//! another change gate. Context-sensitive panels derive `id` with [`ctx_id`]; the base portion
//! normally matches the ID passed to `MoonDataTable::new`, while the `:dock`/`:win` suffix keeps
//! layouts separate.
//!
//! Widths live in `layout.table_column_widths`. Mutations set `layout_dirty`, and the shared
//! debounced/quit save path writes the layout. Visible-column sets use the parallel [`visible`]
//! and [`set_visible`] helpers; user-selected sorts use [`saved_sort`] and [`set_sort`]. Supporting
//! another table's columns or sort requires no table-specific storage code here.
//!
//! The module also owns the SIBLING per-context preferences of those tables — [`report_filters`]
//! and [`set_report_filters`] — because they are keyed by the same [`ctx_id`] and written under the
//! same compare-then-mark-dirty contract. Keeping every writer of that contract in one file is the
//! point; a panel that reaches into `layout` directly is the drift this prevents.

use std::collections::HashMap;

use gpui::{AnyElement, App, Entity, IntoElement, SharedString};
use moon_core::config::{ReportFilterPrefs, TableSortPreference};
use moon_ui::{MoonButton, MoonButtonSize, MoonDataTableState};

use crate::Backend;

/// Returns a context-qualified storage ID for a table and its sibling preferences.
///
/// A docked tab uses `base:dock`; a detached or separately opened window uses `base:win`.
/// Separate keys let a narrow tab and a wide window retain different layouts and related choices.
pub fn ctx_id(base: &str, detached: bool) -> String {
    format!("{base}:{}", if detached { "win" } else { "dock" })
}

/// Recover the table id shared by every context of one [`ctx_id`].
///
/// The inverse of the function above, kept beside it so a caller holding one context id can reach
/// its siblings without re-spelling the base literal or re-deriving the separator.
///
/// Args:
///     ctx_id: Any context-qualified table id.
///
/// Returns:
///     Everything before the final colon, or `None` when the input has no colon. The context
///     suffix is intentionally not validated so callers can migrate all sibling contexts.
pub fn base_of(ctx_id: &str) -> Option<&str> {
    ctx_id.rsplit_once(':').map(|(base, _)| base)
}

/// Builds a toolbar button that resets every column width in `state` to automatic fill.
///
/// This is the button equivalent of Shift+double-clicking a divider. `id` must uniquely identify
/// the button element. Persistence occurs when the state observer subsequently calls [`persist`].
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

/// Returns stored column widths for `id` to seed `MoonDataTableState::column_widths`.
///
/// Returns an empty map when the layout contains no entry, including after a full reset.
pub fn saved(backend: &Backend, id: &str) -> HashMap<String, f32> {
    backend
        .layout
        .table_column_widths
        .get(id)
        .cloned()
        .unwrap_or_default()
}

/// Resets all table column widths to automatic fill.
///
/// Clears `column_widths` in state and notifies observers. A panel observer then removes the
/// layout entry through [`persist`]. Does nothing, including no notification, when already empty.
pub fn reset(state: &Entity<MoonDataTableState>, cx: &mut App) {
    state.update(cx, |s, c| {
        if !s.column_widths.is_empty() {
            s.column_widths.clear();
            c.notify();
        }
    });
}

/// Returns the stored SET of visible-column keys for `id` in canonical order.
///
/// Panels use it while being created or detached. `None` means there is no stored entry, so the
/// table keeps its default, usually all columns visible. Pass the same context-qualified ID used
/// for widths so docked (`:dock`) and windowed (`:win`) field sets remain separate.
pub fn visible(backend: &Backend, id: &str) -> Option<Vec<String>> {
    backend.layout.table_visible_columns.get(id).cloned()
}

/// Stores the SET of visible-column keys for `id` in canonical order.
///
/// Callers invoke this after a visibility toggle. Updates the in-memory layout and marks
/// `layout_dirty` only when the list differs. This function does not reject an empty list; callers
/// must keep at least one column visible to avoid persisting an unusable empty table.
pub fn set_visible(backend: &Entity<Backend>, id: &str, keys: Vec<String>, cx: &mut App) {
    backend.update(cx, |b, _| {
        if b.layout.table_visible_columns.get(id) != Some(&keys) {
            b.layout.table_visible_columns.insert(id.to_string(), keys);
            b.layout_dirty = true;
        }
    });
}

/// Return the stored sort preference for a context-qualified table id.
///
/// Panels validate the returned column key against their current descriptors before applying it.
/// `None` means the table must retain its historical default.
pub fn saved_sort(backend: &Backend, id: &str) -> Option<TableSortPreference> {
    backend.layout.table_sorts.get(id).cloned()
}

/// Store or clear one table's sort preference under the compare-then-mark-dirty contract.
///
/// Passing `None` removes the entry so panels can represent their historical default without an
/// unnecessary serialized value. Repeating the current value performs no write and does not arm
/// the layout saver.
pub fn set_sort(
    backend: &Entity<Backend>,
    id: &str,
    preference: Option<TableSortPreference>,
    cx: &mut App,
) {
    backend.update(cx, |b, _| {
        if update_sort_preferences(&mut b.layout.table_sorts, id, preference) {
            b.layout_dirty = true;
        }
    });
}

/// Apply one optional sort value to the shared map and report whether it changed.
///
/// Kept pure so insert, direction change, no-op, and reset semantics have a direct regression test
/// without constructing a GPUI application context.
fn update_sort_preferences(
    preferences: &mut HashMap<String, TableSortPreference>,
    id: &str,
    next: Option<TableSortPreference>,
) -> bool {
    match next {
        Some(next) if preferences.get(id) != Some(&next) => {
            preferences.insert(id.to_string(), next);
            true
        }
        Some(_) => false,
        None => preferences.remove(id).is_some(),
    }
}

/// Returns the stored Report toolbar filters for `id`, borrowed from the live layout.
///
/// `None` means nothing was ever stored for that host context, which leaves the panel's own
/// defaults standing. Pass the same context-qualified id used for widths.
pub fn report_filters<'a>(backend: &'a Backend, id: &str) -> Option<&'a ReportFilterPrefs> {
    backend.layout.report_filters.get(id)
}

/// Stores the Report toolbar filters for `id` when they differ from what is already there.
///
/// The same compare-then-mark-dirty rule as [`set_visible`]: an unchanged set writes nothing and
/// leaves `layout_dirty` alone, so a no-op toolbar click cannot schedule a layout flush.
pub fn set_report_filters(
    backend: &Entity<Backend>,
    id: &str,
    prefs: ReportFilterPrefs,
    cx: &mut App,
) {
    backend.update(cx, |b, _| {
        if b.layout.report_filters.get(id) != Some(&prefs) {
            b.layout.report_filters.insert(id.to_string(), prefs);
            b.layout_dirty = true;
        }
    });
}

/// Stores the table's current column widths for `id` when they change.
///
/// Intended for a render/change observer: it only compares maps unless an update is needed.
/// A changed non-empty map replaces the in-memory layout entry and marks `layout_dirty`. An empty
/// map removes an existing entry and marks it dirty, preserving a full reset so the next panel
/// instance opens with automatic widths. If both current state and layout entry are empty, this is
/// a no-op.
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

#[cfg(test)]
mod tests;
