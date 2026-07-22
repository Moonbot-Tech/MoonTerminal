//! Shared checkbox-based core multi-selector used by the Orders, Report, Assets, Core Status, and
//! Analytics views.
//!
//! The trigger and menu are content-sized through `design::dropdown_content_widths`, with their old
//! fixed widths retained as lower bounds. The trigger grows to its compact-toolbar cap and then
//! ellipsizes the label; the menu grows for its longest item up to the shared upper bound. `CoreId`
//! aliases `u64`, so the builder uses `u64` for every caller. Each caller supplies its localized
//! labels and defines the behavior of its own `toggle_core` callback.

use std::collections::HashSet;

use gpui::App;

use crate::core_order::OrderedCores;
use crate::design;
use moon_ui::{MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize};

/// Build a checkbox-based core multi-selector whose menu stays open across item clicks.
///
/// The trigger shows `all_label` for an empty selection or when the nonempty `cores` and `selected`
/// collections have equal lengths. This all-selected check is cardinality-only and does not verify
/// that selected ids belong to `cores`, so an equal-sized set containing unknown ids is also treated
/// as full. Otherwise the trigger shows the sole selected core's name when present in `cores`, or
/// `cores_n(N)`. It grows to the internal cap before its label is ellipsized; the menu grows for its
/// longest item up to the shared cap. The All item calls `on_toggle(None, app)`, while a core item
/// calls `on_toggle(Some(id), app)`.
///
/// Args:
///     cx: Application context used for text and layout measurements.
///     id: Dropdown id and prefix for the `{id}-all` and `{id}-{i}` item keys.
///     cores: Ordered core ids and display names.
///     selected: Currently selected core ids.
///     all_label: Localized label for the empty or cardinality-matched selection and the All item.
///     cores_n: Localized formatter for a selection count or an unresolved sole id.
///     min_menu_w: Lower bound for menu width, not a fixed width.
///     on_toggle: Callback receiving `None` for All or `Some(id)` for one core.
///
/// Returns:
///     The configured multi-select dropdown.
pub(crate) fn core_combo<F>(
    cx: &App,
    id: &'static str,
    cores: &OrderedCores,
    selected: &HashSet<u64>,
    all_label: String,
    cores_n: impl Fn(usize) -> String,
    min_menu_w: f32,
    on_toggle: F,
) -> MoonDropdown
where
    F: Fn(Option<u64>, &mut App) + Clone + 'static,
{
    // The current all-selected state compares cardinality only; it does not validate membership of
    // `selected` in `cores`.
    let all_selected = !cores.is_empty() && selected.len() == cores.len();
    let all_on = selected.is_empty() || all_selected;
    let cur = match selected.len() {
        0 => all_label.clone(),
        _ if all_selected => all_label.clone(),
        1 => {
            let sel = *selected.iter().next().unwrap();
            cores
                .iter()
                .find(|(c, _)| *c == sel)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| cores_n(1))
        }
        n => cores_n(n),
    };
    // Share the content-width calculation with `screener::source_combo`: size the trigger for the
    // current selection between `CORES_TRIGGER_MIN_W` and the compact-toolbar cap, and size the menu
    // for its longest item between `min_menu_w` and the shared cap.
    let (trigger_label, trigger_w, menu_w) = design::dropdown_content_widths(
        cx,
        &cur,
        std::iter::once(all_label.as_str()).chain(cores.iter().map(|(_, n)| n.as_str())),
        design::CORES_TRIGGER_MIN_W,
        min_menu_w,
    );
    let toggle_all = on_toggle.clone();
    let mut menu = MoonDropdown::new(id)
        .label(trigger_label)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Action)
        .trigger_width(trigger_w)
        .menu_width(menu_w)
        .menu_max_height(360.0)
        .menu_size(MoonMenuSize::Compact)
        .close_on_select(false)
        .item(
            // The All item delegates clearing or selecting every core to the caller.
            MoonMenuItem::with_key(format!("{id}-all"), all_label)
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| toggle_all(None, app)),
        );
    for (i, (core, name)) in cores.iter().enumerate() {
        let core = *core;
        let on = selected.contains(&core);
        let on_toggle = on_toggle.clone();
        menu = menu.item(
            MoonMenuItem::with_key(format!("{id}-{i}"), name.clone())
                .checked(on)
                .selected(on)
                .on_click(move |_, _, app| on_toggle(Some(core), app)),
        );
    }
    menu
}
