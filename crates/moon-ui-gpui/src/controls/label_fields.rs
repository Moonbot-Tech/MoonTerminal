//! The chart-caption catalogue as a menu: every field a caption can print, in sections.
//!
//! Shared because two places offer the same list and must not drift: the labels popup, where a pick
//! creates a whole module, and the module editor, where a pick adds one caption to the module being
//! edited. What differs is what happens to the pick, which is the callback — never the list.

use gpui::{App, SharedString, Window};
use moon_core::config::{ChartLabelField, ChartLabelGroup};
use moon_ui::MoonMenuItem;
use rust_i18n::t;

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
