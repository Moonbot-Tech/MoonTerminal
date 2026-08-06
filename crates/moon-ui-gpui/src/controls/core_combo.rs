//! Shared checkbox-based core multi-selector used by the Orders, Report, Assets, Core Status, and
//! Analytics views.
//!
//! The trigger has fixed compact geometry so an open menu stays anchored while selections change.
//! MoonUI fits the exchange-grouped menu and ellipsizes anomalously long labels inside its cap.
//! `CoreId` aliases `u64`, so the builder uses `u64` for every caller. Each caller supplies its
//! localized labels and defines the behavior of its own individual and exchange-batch callbacks.
//! Known exchange rows are clickable in every multi-select consumer; the unknown section remains a
//! non-interactive label because it has no reported exchange identity.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::App;
use rust_i18n::t;

use super::exchange_label::exchange_display_name;
use crate::controls::CORE_COMBO_TRIGGER_W;
use crate::core_order::OrderedCores;
use moon_ui::{MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize};

/// One exchange section as its reported name and canonically ordered core rows.
pub(crate) type CoreMenuSection<'a> = (Option<&'a str>, Vec<(u64, &'a str)>);

/// Controls when the selector's All row represents an explicit complete selection.
#[derive(Clone, Copy)]
pub(crate) enum CoreAllRowMode {
    /// Empty and complete explicit selections both check and label the All row.
    ImplicitOrComplete,
    /// Only the empty implicit selection checks and labels the All row.
    ImplicitOnly,
}

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

/// Toggle every currently available core from one exchange in an explicit selection.
///
/// An empty selection represents All before this action, so the first click inserts the exchange
/// ids and narrows it to that exchange. When every available exchange member is explicitly
/// selected, the next click removes them. Cores selected from other exchanges remain unchanged,
/// and render-time ids that are no longer available are ignored.
///
/// Args:
///     selected: Mutable selected-core set using empty as the implicit All representation.
///     available: Current core ids in the consumer's selector scope.
///     exchange: Core ids captured from one rendered exchange section.
///
/// Returns:
///     Whether at least one available exchange core changed selection state.
pub(crate) fn toggle_exchange_cores(
    selected: &mut HashSet<u64>,
    available: &HashSet<u64>,
    exchange: impl IntoIterator<Item = u64>,
) -> bool {
    let exchange: HashSet<u64> = exchange
        .into_iter()
        .filter(|core| available.contains(core))
        .collect();
    if exchange.is_empty() {
        return false;
    }

    if exchange.iter().all(|core| selected.contains(core)) {
        selected.retain(|core| !exchange.contains(core));
    } else {
        selected.extend(exchange);
    }
    true
}

/// Whether a selection reads as "all cores": empty (implicit) or holding every available one.
///
/// This is the legacy `ImplicitOrComplete` display rule shared by Orders, Report, Assets, and Core
/// Status. Analytics deliberately uses `ImplicitOnly`, where a complete explicit selection remains
/// visible as selected cores rather than being relabeled All.
///
/// Args:
///     available: Current core ids in the consumer's scope.
///     selected: Currently selected core ids.
///
/// Returns:
///     Whether the selection represents all cores, implicitly or by containing every available id.
fn core_selection_is_all(
    available: impl IntoIterator<Item = u64>,
    selected: &HashSet<u64>,
) -> bool {
    if selected.is_empty() {
        return true;
    }
    // An empty scope is NOT "all": there is nothing for the selection to cover, and a stale id
    // must keep reading as a partial selection rather than as the complete one.
    let mut available = available.into_iter().peekable();
    available.peek().is_some() && available.all(|core| selected.contains(&core))
}

/// Resolve the trigger summary without exposing a sole core's variable-length name.
///
/// `ImplicitOrComplete` preserves the existing All representation for empty selections and those
/// containing every available core. `ImplicitOnly` reserves All for the empty set. Every other
/// selection uses the localized count of available selected cores, including one core, so changes
/// cannot replace a compact summary with arbitrary user text. Stale ids do not make an equal-sized
/// partial selection look complete.
///
/// The Analytics tab bar does name a sole selected core, but OUTSIDE this trigger: a muted,
/// width-bounded, truncating label in the row's flex slack, which cannot push the fixed-width
/// combos off the row. The trigger itself still never shows the name.
///
/// Args:
///     cores: Available core ids and names.
///     selected: Currently selected core ids.
///     all_row_mode: Whether a complete explicit selection also represents the All row.
///     all_label: Localized All label.
///     cores_n: Localized core-count formatter.
///
/// Returns:
///     The localized trigger summary without its caret and whether the All row is selected.
fn selection_summary(
    cores: &[(u64, String)],
    selected: &HashSet<u64>,
    all_row_mode: CoreAllRowMode,
    all_label: &str,
    cores_n: &impl Fn(usize) -> String,
) -> (String, bool) {
    let selected_available = cores
        .iter()
        .filter(|(core, _)| selected.contains(core))
        .count();
    let all_on = match all_row_mode {
        CoreAllRowMode::ImplicitOrComplete => {
            core_selection_is_all(cores.iter().map(|(core, _)| *core), selected)
        }
        CoreAllRowMode::ImplicitOnly => selected.is_empty(),
    };
    let label = if all_on {
        all_label.to_string()
    } else {
        cores_n(selected_available)
    };
    (label, all_on)
}

/// Copy every core id from one rendered exchange section in member order.
///
/// Args:
///     members: Canonically ordered core ids and display names from one exchange section.
///
/// Returns:
///     Every section member id in the same order.
fn section_core_ids(members: &[(u64, &str)]) -> Vec<u64> {
    members.iter().map(|(core, _)| *core).collect()
}

/// Build a checkbox-based core multi-selector with clickable known-exchange rows.
///
/// The trigger shows `all_label` according to `all_row_mode`; every other selection shows
/// `cores_n(N)` for the available selected cores, including one core. Fixed trigger geometry keeps
/// the open menu anchored while selections change. MoonUI fits the menu and truncates labels
/// against its own row geometry. The All item calls
/// `on_toggle(None, app)`, while a core item calls `on_toggle(Some(id), app)`. Each known exchange
/// row submits all of its member ids to `on_toggle_exchange`; the unknown section remains a label.
///
/// Args:
///     id: Dropdown id and prefix for generated item keys.
///     cores: Ordered core ids and display names.
///     exchange_names: Reported display exchange names keyed by core id.
///     selected: Currently selected core ids.
///     all_row_mode: Whether a complete explicit selection also represents the All row.
///     all_label: Localized label for the All item and every state that activates it.
///     cores_n: Localized formatter for every partial selection count.
///     min_menu_w: Caller-specific lower bound for the fitted menu width.
///     on_toggle: Callback receiving `None` for All or `Some(id)` for one core.
///     on_toggle_exchange: Callback receiving every core id in the clicked exchange section.
///
/// Returns:
///     The configured multi-select dropdown.
pub(crate) fn core_combo<F, G>(
    id: &'static str,
    cores: &OrderedCores,
    exchange_names: &HashMap<u64, String>,
    selected: &HashSet<u64>,
    all_row_mode: CoreAllRowMode,
    all_label: String,
    cores_n: impl Fn(usize) -> String,
    min_menu_w: f32,
    on_toggle: F,
    on_toggle_exchange: G,
) -> MoonDropdown
where
    F: Fn(Option<u64>, &mut App) + Clone + 'static,
    G: Fn(Vec<u64>, &mut App) + 'static,
{
    let on_toggle_exchange: Rc<dyn Fn(Vec<u64>, &mut App)> = Rc::new(on_toggle_exchange);
    let (cur, all_on) = selection_summary(cores, selected, all_row_mode, &all_label, &cores_n);
    let unknown_exchange = t!("common.exchange_unknown").to_string();
    let sections = core_menu_sections(cores, exchange_names);
    let toggle_all = on_toggle.clone();
    let mut menu = MoonDropdown::new(id)
        .label(cur)
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Action)
        .trigger_width_scaled(CORE_COMBO_TRIGGER_W)
        .fit_menu_width(min_menu_w, 560.0)
        .menu_max_height_ui(360.0)
        .menu_size(MoonMenuSize::Compact)
        .close_on_select(false)
        .item(
            // The All item delegates clearing or selecting every core to the caller.
            MoonMenuItem::with_key(format!("{id}-all"), all_label)
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| toggle_all(None, app)),
        );
    if !sections.is_empty() {
        menu = menu.item(MoonMenuItem::separator());
    }
    for (section_index, (exchange, members)) in sections.into_iter().enumerate() {
        let exchange_label = exchange
            .map(exchange_display_name)
            .unwrap_or_else(|| unknown_exchange.clone());
        if exchange.is_some() {
            let exchange_cores = section_core_ids(&members);
            let on_toggle_exchange = on_toggle_exchange.clone();
            menu = menu.item(
                MoonMenuItem::action_label(
                    format!("{id}-exchange-{section_index}"),
                    exchange_label,
                )
                .on_click(move |_, _, app| {
                    on_toggle_exchange(exchange_cores.clone(), app);
                }),
            );
        } else {
            menu = menu.item(MoonMenuItem::label(exchange_label));
        }
        for (core, name) in members {
            let on = selected.contains(&core);
            let on_toggle = on_toggle.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("{id}-core-{core}"), name)
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
