//! Shared checkbox-based core multi-selector used by the Orders, Report, Assets, Core Status, and
//! Analytics views.
//!
//! The trigger has fixed compact geometry so an open menu stays anchored while selections change.
//! The exchange-grouped menu has one stable width, and anomalously long labels ellipsize inside it.
//! `CoreId` aliases `u64`, so the builder uses `u64` for every caller. Each caller supplies its
//! localized labels and defines the behavior of its own `toggle_core` callback.

use std::collections::{HashMap, HashSet};

use gpui::App;
use rust_i18n::t;

use crate::core_order::OrderedCores;
use crate::design;
use moon_ui::{MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize};

/// One exchange section as its reported name and canonically ordered core rows.
pub(crate) type CoreMenuSection<'a> = (Option<&'a str>, Vec<(u64, &'a str)>);

/// Group canonically ordered cores by their reported exchange names.
///
/// The common ordering helper owns unknown-first and alphabetical exchange ordering. This adapter
/// maps its stable source indices back to the core ids and display names consumed by menus.
///
/// Args:
///     cores: Core ids and display names in canonical order.
///     exchange_names: Reported display exchange names keyed by core id.
///
/// Returns:
///     Exchange sections whose member order matches `cores`.
pub(crate) fn core_menu_sections<'a>(
    cores: &'a [(u64, String)],
    exchange_names: &'a HashMap<u64, String>,
) -> Vec<CoreMenuSection<'a>> {
    crate::core_order::exchange_sections(cores.iter().enumerate().map(|(index, (core, _))| {
        (
            index,
            exchange_names.get(core).map(|exchange| exchange.as_str()),
        )
    }))
    .into_iter()
    .map(|(exchange, members)| {
        (
            exchange,
            members
                .into_iter()
                .map(|index| (cores[index].0, cores[index].1.as_str()))
                .collect(),
        )
    })
    .collect()
}

/// Fit a menu label to an explicit width with a caller-supplied measurement function.
///
/// Keeping this pure separates the boundary behavior from GPUI font lookup so the production
/// renderer and deterministic regression test exercise the same truncation decision.
///
/// Args:
///     label: Full label from server configuration or exchange discovery.
///     max_w: Maximum rendered width available to the label.
///     measure: Function returning rendered width for arbitrary label fragments.
///
/// Returns:
///     The original label when it fits, otherwise a prefix ending in an ellipsis.
fn fit_core_menu_label(label: &str, max_w: f32, measure: impl Fn(&str) -> f32) -> String {
    design::fit_text(label, max_w, measure).0
}

/// Fit a core-menu label using the compact MoonUI menu's actual font and chrome metrics.
///
/// Args:
///     cx: Application context used for text measurement.
///     label: Full label from server configuration or exchange discovery.
///     menu_w: Rendered fixed menu width.
///
/// Returns:
///     A left-start label that fits the checked-item text column.
pub(crate) fn core_menu_label(cx: &App, label: &str, menu_w: f32) -> String {
    fit_core_menu_label(label, design::menu_item_label_width(cx, menu_w), |text| {
        design::ui_text_width(cx, text, 9.5, 600.0, true)
    })
}

/// Toggle the All row against the membership of the currently available cores.
///
/// Args:
///     selected: Mutable selected-core set using empty as the implicit All representation.
///     available: Current core ids in the selector's scope.
///
/// Returns:
///     Nothing; `selected` is cleared when it already contains every available core, or replaced
///     with `available` otherwise.
pub(crate) fn toggle_all_core_selection(selected: &mut HashSet<u64>, available: HashSet<u64>) {
    if !available.is_empty() && available.iter().all(|core| selected.contains(core)) {
        selected.clear();
    } else {
        *selected = available;
    }
}

/// Normalize a multi-core selection for consumers where an empty vector means no filter.
///
/// Args:
///     available: Current core ids in the consumer's scope.
///     selected: Currently selected core ids.
///
/// Returns:
///     An empty vector when selection is implicit or contains every available core; otherwise the
///     explicit selected ids, including stale ids that must not silently broaden the result.
pub(crate) fn normalized_core_filter_ids(
    available: impl IntoIterator<Item = u64>,
    selected: &HashSet<u64>,
) -> Vec<u64> {
    let available: Vec<u64> = available.into_iter().collect();
    let all_selected =
        !available.is_empty() && available.iter().all(|core| selected.contains(core));
    if selected.is_empty() || all_selected {
        Vec::new()
    } else {
        selected.iter().copied().collect()
    }
}

/// Resolve the trigger summary without exposing a sole core's variable-length name.
///
/// Empty selections and selections containing every available core retain the existing All
/// representation. Every partial selection uses the localized count of available selected cores,
/// including one core, so selection changes cannot replace a compact summary with arbitrary user
/// text. Stale ids do not make an equal-sized partial selection look complete.
///
/// Args:
///     cores: Available core ids and names.
///     selected: Currently selected core ids.
///     all_label: Localized All label.
///     cores_n: Localized core-count formatter.
///
/// Returns:
///     The localized trigger summary without its caret and whether the All row is selected.
fn selection_summary(
    cores: &[(u64, String)],
    selected: &HashSet<u64>,
    all_label: &str,
    cores_n: &impl Fn(usize) -> String,
) -> (String, bool) {
    let selected_available = cores
        .iter()
        .filter(|(core, _)| selected.contains(core))
        .count();
    let all_selected = !cores.is_empty() && selected_available == cores.len();
    let all_on = selected.is_empty() || all_selected;
    let label = if all_on {
        all_label.to_string()
    } else {
        cores_n(selected_available)
    };
    (label, all_on)
}

/// Build a checkbox-based core multi-selector whose menu stays open across item clicks.
///
/// The trigger shows `all_label` for an empty selection or one containing every available core.
/// Every partial selection shows `cores_n(N)` for the available selected cores, including one core.
/// Fixed trigger and menu geometry keep the open menu anchored while selections and label lengths
/// change. Labels beyond the shared text budget end in an ellipsis. The All item calls
/// `on_toggle(None, app)`, while a core item calls `on_toggle(Some(id), app)`.
///
/// Args:
///     cx: Application context used for text and layout measurements.
///     id: Dropdown id and prefix for the `{id}-all` and `{id}-core-{core}` item keys.
///     cores: Ordered core ids and display names.
///     exchange_names: Reported display exchange names keyed by core id.
///     selected: Currently selected core ids.
///     all_label: Localized label for an empty or complete selection and the All item.
///     cores_n: Localized formatter for every partial selection count.
///     min_menu_w: Caller-specific lower bound for the shared fixed menu width.
///     on_toggle: Callback receiving `None` for All or `Some(id)` for one core.
///
/// Returns:
///     The configured multi-select dropdown.
pub(crate) fn core_combo<F>(
    cx: &App,
    id: &'static str,
    cores: &OrderedCores,
    exchange_names: &HashMap<u64, String>,
    selected: &HashSet<u64>,
    all_label: String,
    cores_n: impl Fn(usize) -> String,
    min_menu_w: f32,
    on_toggle: F,
) -> MoonDropdown
where
    F: Fn(Option<u64>, &mut App) + Clone + 'static,
{
    let (cur, all_on) = selection_summary(cores, selected, &all_label, &cores_n);
    let unknown_exchange = t!("common.exchange_unknown").to_string();
    let sections = core_menu_sections(cores, exchange_names);
    let (trigger_label, trigger_w) =
        design::fixed_dropdown_trigger(cx, &cur, design::CORES_TRIGGER_MIN_W);
    let menu_w = design::core_menu_width(cx, min_menu_w);
    let toggle_all = on_toggle.clone();
    let mut menu = MoonDropdown::new(id)
        .label(trigger_label)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Action)
        .trigger_width(trigger_w)
        .menu_width(menu_w)
        .menu_max_height(design::ui_value(cx, 360.0))
        .menu_size(MoonMenuSize::Compact)
        .close_on_select(false)
        .item(
            // The All item delegates clearing or selecting every core to the caller.
            MoonMenuItem::with_key(format!("{id}-all"), core_menu_label(cx, &all_label, menu_w))
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| toggle_all(None, app)),
        );
    if !sections.is_empty() {
        menu = menu.item(MoonMenuItem::separator());
    }
    for (exchange, members) in sections {
        let exchange = exchange.unwrap_or(unknown_exchange.as_str());
        menu = menu.item(MoonMenuItem::label(core_menu_label(cx, exchange, menu_w)));
        for (core, name) in members {
            let on = selected.contains(&core);
            let on_toggle = on_toggle.clone();
            menu = menu.item(
                MoonMenuItem::with_key(
                    format!("{id}-core-{core}"),
                    core_menu_label(cx, name, menu_w),
                )
                .checked(on)
                .selected(on)
                .on_click(move |_, _, app| on_toggle(Some(core), app)),
            );
        }
    }
    menu
}

#[cfg(test)]
mod tests;
