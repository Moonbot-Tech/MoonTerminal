//! The chart-caption catalogue as a menu: every field a caption can print, in sections.
//!
//! Shared because two places offer the same list and must not drift: the labels popup, where a pick
//! creates a whole module, and the module editor, where a pick adds one caption to the module being
//! edited. What differs is what happens to the pick, which is the callback — never the list.

use gpui::{App, SharedString, Window};
use moon_core::config::{ChartLabelField, ChartLabelGroup, ChartLabelRow};
use moon_ui::MoonMenuItem;
use rust_i18n::t;

/// What a module is CALLED, in the reader's language.
///
/// One rule in one place, because three surfaces ask it and must not drift: the chart, when the
/// module prints its own name; the popup's module line; and the editor's title and preview. The
/// user's own name wins, a preset's name is looked up every time it is printed — which is what
/// makes it follow a live language switch — and a module that has neither has no title at all.
pub(crate) fn row_title(row: &ChartLabelRow) -> Option<String> {
    if !row.name.is_empty() {
        return Some(row.name.clone());
    }
    row.title_key().map(|key| t!(key).to_string())
}

/// What the module LIST calls this module: its title, or the captions it prints.
///
/// A module is not required to have a title — most never get one — so the list falls back to
/// naming it by what it does. An empty module with no title says so rather than showing a blank
/// line.
pub(crate) fn row_display_name(row: &ChartLabelRow) -> String {
    if let Some(title) = row_title(row) {
        return title;
    }
    let used = row.used_parts();
    if used == 0 {
        return t!("chart_labels.row_empty").to_string();
    }
    let mut out = String::new();
    for part in &row.parts[..used.min(2)] {
        if !out.is_empty() {
            out.push_str(" · ");
        }
        out.push_str(&t!(part.field.locale_key()));
    }
    if used > 2 {
        out.push_str(" · …");
    }
    out
}

/// Build the catalogue, sectioned by where a field's value comes from.
///
/// Args:
///     id_prefix: Element-id prefix, so two menus alive at once keep distinct identities.
///     is_added: Whether a field is already configured somewhere, for its check mark.
///     on_pick: Receives the chosen field.
pub(crate) fn field_menu_items(
    id_prefix: &str,
    is_added: impl Fn(ChartLabelField) -> bool,
    on_pick: impl Fn(ChartLabelField, &mut Window, &mut App) + Clone + 'static,
) -> Vec<MoonMenuItem> {
    let mut items = Vec::new();
    for (n, group) in ChartLabelGroup::ALL.into_iter().enumerate() {
        if n > 0 {
            items.push(MoonMenuItem::separator());
        }
        for field in ChartLabelField::ALL
            .into_iter()
            .filter(|f| f.group() == group)
        {
            let on_pick = on_pick.clone();
            // Checked rather than disabled: the same figure on two modules is a legitimate layout —
            // the coin over the strip and the coin over the plot — and the mark is there to answer
            // "did I already add this?", not to forbid it.
            items.push(
                MoonMenuItem::with_key(
                    SharedString::from(format!("{id_prefix}-{field:?}")),
                    SharedString::from(t!(field.locale_key()).to_string()),
                )
                .checked(is_added(field))
                .on_click(move |_, window: &mut Window, app: &mut App| on_pick(field, window, app)),
            );
        }
    }
    items
}
