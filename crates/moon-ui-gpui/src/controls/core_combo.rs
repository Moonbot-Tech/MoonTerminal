//! Shared checkbox-based core multi-selector used by the Orders, Report, Assets, Alerts, Core
//! Status, and Analytics views.
//!
//! The trigger has fixed compact geometry so an open menu stays anchored while selections change.
//! MoonUI fits the exchange-grouped menu and ellipsizes anomalously long labels inside its cap.
//! `CoreId` aliases `u64`, so the builder uses `u64` for every caller. Each caller supplies its
//! localized labels and defines the behavior of its own individual and exchange-batch callbacks.
//! Every exchange row is clickable, the unnamed section included: it has no reported exchange
//! identity, but its members are still a batch worth toggling at once.
//!
//! Above its core rows the menu carries the All row, which CLEARS the selection — empty means all
//! cores — and, unless the selector is pinned, the saved-groups block [`CoreComboExtras`] supplies.
//! Its actions sit at the very BOTTOM of the menu, past the cores.
//!
//! There is no "select all" row. An explicit full selection — the state a user removes ONE core
//! from, which is the whole reason this picker was reworked — is reached by saving the implicit-All
//! selection as a group and clicking it: `saved_group_cores` materializes every live core at save
//! time, so applying that group yields the explicit set. One deliberate setup step replaced a
//! permanent row.
//!
//! The menu is `close_on_select(false)`, because its core rows are checkboxes and a multi-select
//! menu must survive them. The two rows that open a dialog are the exception and say so themselves
//! with `MoonMenuItem::closes_menu(true)`: a menu left standing paints OVER the dialog it just
//! opened, since MoonUI defers popovers above the dialog layer. That per-row override is a MoonUI
//! affordance that avoids making all six hosts mirror the dropdown's open state and retain another
//! callback.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{App, Window};
use rust_i18n::t;

use super::core_groups::{
    applicable_count, group_dead_count, group_facts, group_is_applied, saved_group_cores,
    saves_core,
};
use super::core_quick::{exchange_state_label, group_check_state, section_core_ids};
use super::venue_label::venue_section_label;
use crate::controls::CORE_COMBO_TRIGGER_W;
use crate::controls::wrap_fit;
use crate::core_order::OrderedCores;
use moon_core::config::CoreGroup;
use moon_core::venue::CoreVenue;
use moon_ui::{MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize};

#[cfg(test)]
mod tests;

/// One exchange section as its venue and canonically ordered core rows.
pub(crate) type CoreMenuSection<'a> = (Option<&'a CoreVenue>, Vec<(u64, &'a str)>);

/// Controls when the selector's All row represents an explicit complete selection.
#[derive(Clone, Copy)]
pub(crate) enum CoreAllRowMode {
    /// Empty and complete explicit selections both check and label the All row.
    ImplicitOrComplete,
    /// Only the empty implicit selection checks and labels the All row.
    ImplicitOnly,
}

/// Everything the picker offers beyond its own rows.
///
/// A selector pinned by an Auto workspace is disabled, so it takes `None` rather than rows nobody
/// can click. Private fields keep construction on [`CoreComboExtras::new`], while
/// [`super::core_combo_extras`] is the shared assembly path that applies the pin policy and uses
/// weak view captures for retained handlers.
///
/// The groups are BORROWED from the live config for the duration of one render. Both this and
/// [`core_combo`] are called inside the same expression at every call site, and cloning 32 names
/// and member lists per host per repaint, to render rows a closed menu never shows, is exactly the
/// per-frame work this codebase forbids panels.
pub(crate) struct CoreComboExtras<'a> {
    groups: &'a [CoreGroup],
    configured: HashSet<u64>,
    on_apply_group: Rc<dyn Fn(String, bool, Vec<u64>, &mut Window, &mut App)>,
    on_save_group: Rc<dyn Fn(Vec<u64>, &mut Window, &mut App)>,
    on_manage_groups: Rc<dyn Fn(&mut Window, &mut App)>,
}

impl<'a> CoreComboExtras<'a> {
    /// Assemble the affordances.
    ///
    /// Args:
    ///     groups: The saved groups, borrowed from the live config in their persisted order.
    ///     configured: Every uid in `AppConfig.servers` — the authority on a missing member.
    ///     on_apply_group: Applies a group click, given its NAME, whether the click was additive,
    ///         and the selectable ids. By name, not position: the saved list is application state
    ///         and a management modal in another window can reorder it under an open menu.
    ///     on_save_group: Opens the save modal for the member uids a new group would store.
    ///     on_manage_groups: Opens the management modal.
    ///
    /// Returns:
    ///     The affordances, ready to hand to [`core_combo`].
    pub(crate) fn new(
        groups: &'a [CoreGroup],
        configured: HashSet<u64>,
        on_apply_group: Rc<dyn Fn(String, bool, Vec<u64>, &mut Window, &mut App)>,
        on_save_group: Rc<dyn Fn(Vec<u64>, &mut Window, &mut App)>,
        on_manage_groups: Rc<dyn Fn(&mut Window, &mut App)>,
    ) -> Self {
        Self {
            groups,
            configured,
            on_apply_group,
            on_save_group,
            on_manage_groups,
        }
    }
}

/// Group canonically ordered cores by the venue each is connected to.
///
/// The common ordering helper owns unknown-first and caption ordering. This adapter maps its stable
/// source indices back to the core ids and display names consumed by menus.
///
/// Args:
///     cores: Core ids and display names in canonical order.
///     venues: What each identified core is connected to, keyed by core id.
///
/// Returns:
///     Exchange sections whose member order matches `cores`.
pub(crate) fn core_menu_sections<'a>(
    cores: &'a [(u64, String)],
    venues: &'a HashMap<u64, CoreVenue>,
) -> Vec<CoreMenuSection<'a>> {
    crate::core_order::exchange_sections(
        cores
            .iter()
            .enumerate()
            .map(|(index, (core, _))| (index, venues.get(core))),
    )
    .into_iter()
    .map(|(venue, members)| {
        (
            venue,
            members
                .into_iter()
                .map(|index| (cores[index].0, cores[index].1.as_str()))
                .collect(),
        )
    })
    .collect()
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
/// This is the `ImplicitOrComplete` display rule shared by Orders, Report, Assets, Alerts, and Core
/// Status. Analytics uses `ImplicitOnly`, where a complete explicit selection remains visible as
/// selected cores rather than being relabeled All.
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

/// Icon standing for "cores" once the selector drops the word.
///
/// The same glyph everywhere the compact selector appears, for the reason the toolbar's launchers
/// keep theirs: at this width the icon IS the label, so a second glyph for the same concept would
/// read as a different control.
const CORE_COMPACT_ICON: &str = "icons/cpu.svg";

/// Put a core selector into its compact form: the icon, a short label, and no room for a word.
///
/// Applied to the dropdown [`core_combo`] already built, so the menu, its callbacks and its
/// selection rules are untouched by the row's width — only the trigger yields. The caller keeps the
/// full summary reachable as a tooltip; nothing here removes information without a way back to it.
///
/// The width is FITTED by the component rather than imposed: this trigger also carries a pinned
/// workspace's core name, which is arbitrary user text, and only the component's own fitting
/// ellipsizes it and appends the caret exactly once. The BOUNDS are the row's shared ones
/// ([`wrap_fit::COMPACT_MIN_W`]), so this lands on the same width as a selector beside it that
/// resolves its own trigger content through `wrap_fit::compact_trigger_width`.
///
/// Args:
///     cx: Application context used to resolve the row's shared floor at the active font step.
///     combo: The selector as [`core_combo`] built it.
///     label: Compact trigger text — a short All word, a count, or a pinned scope's own name.
///     all_word: The short All word this row's selectors are floored on, so this trigger holds the
///         same width as the ones beside it and does not change width as the selection moves.
///
/// Returns:
///     The same selector wearing its compact trigger.
pub(crate) fn compact_core_trigger(
    cx: &App,
    combo: MoonDropdown,
    label: String,
    all_word: &str,
) -> MoonDropdown {
    combo
        .label(label)
        .trigger_icon(CORE_COMPACT_ICON)
        .fit_trigger_width(
            wrap_fit::compact_design_floor(cx, all_word),
            wrap_fit::COMPACT_MAX_W,
        )
}

/// Resolve the trigger summary without exposing a sole core's variable-length name.
///
/// Also the summary a HOST needs outside the trigger — a compact label, or the tooltip that
/// recovers what a compact trigger dropped. Passing short labels yields the compact form; passing
/// the full ones yields exactly what the trigger says. Both come from these same rules, never from
/// a second reading of the set: a label that disagreed with the menu beneath it about what "all"
/// means would be worse than no compaction at all.
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
///     The localized trigger summary without its caret, whether the All row is selected, and the
///     count behind that summary — so a host wanting a second wording (a tooltip beside a compact
///     trigger) formats it from this pass rather than walking the selection again.
pub(crate) fn core_selection_summary(
    cores: &[(u64, String)],
    selected: &HashSet<u64>,
    all_row_mode: CoreAllRowMode,
    all_label: &str,
    cores_n: &impl Fn(usize) -> String,
) -> CoreSummary {
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
    CoreSummary {
        label,
        all_on,
        selected: selected_available,
    }
}

/// One resolution of a core selection, as the trigger and any label beside it should read it.
pub(crate) struct CoreSummary {
    /// The localized summary text, without the caret the trigger appends itself.
    pub(crate) label: String,
    /// Whether this selection reads as the All row.
    pub(crate) all_on: bool,
    /// How many of the selector's own cores are selected — the number `label` may be built from.
    pub(crate) selected: usize,
}

/// Append the saved-groups block above the exchange sections: a heading and one row per group.
///
/// Only the heading and the group rows: the two management ACTIONS live at the bottom of the menu,
/// in [`group_actions_block`], because every row here delays the core list the picker exists for.
///
/// A group whose members this consumer cannot select is DISABLED rather than hidden: hiding it
/// would suggest the group was deleted, while a greyed row correctly says "not here".
///
/// Its trailing count is what the CLICK produces — the members this consumer can actually select —
/// not the group's own size, for the reason any acting row states its own effect: an
/// action row's number is a promise about the action. A group of ten reading `10` in a panel scoped
/// to three of them would enable a row that then selects three. The missing count beside it
/// distinguishes cores absent from the configuration from those merely outside this panel's scope.
///
/// Save is gated on the selection resolving to at least one CONFIGURED core, since a group of
/// nothing cannot be applied later. Manage is never gated: a group whose members all died must
/// stay reachable to be renamed or deleted.
///
/// Args:
///     menu: The menu being built.
///     id: Dropdown id, the prefix of the generated keys.
///     selected: Currently selected core ids, which decide whether a group reads as applied.
///     selectable: Every core this consumer can select, shared with each row's handler.
///     cores_n: The consumer's own localized core-count formatter.
///     extras: The consumer's groups and handlers.
///
/// Returns:
///     The menu with the block appended.
fn groups_block(
    menu: MoonDropdown,
    id: &'static str,
    selected: &HashSet<u64>,
    selectable: &Rc<[u64]>,
    cores_n: &impl Fn(usize) -> String,
    extras: &CoreComboExtras,
) -> MoonDropdown {
    let mut menu = menu;
    if !extras.groups.is_empty() {
        menu = menu.item(MoonMenuItem::label(
            t!("common.core_pick.groups_heading").to_string(),
        ));
    }
    let pickable: HashSet<u64> = selectable.iter().copied().collect();
    for (index, group) in extras.groups.iter().enumerate() {
        let applicable = applicable_count(&group.cores, &pickable);
        let on_apply = extras.on_apply_group.clone();
        let payload = selectable.clone();
        let name = group.name.clone();
        // `cores_n` and NOT a group-specific string: this row sits directly under a trigger that
        // renders the very same fact through that formatter, and two wordings for one count in one
        // menu read as two different numbers.
        let trailing = group_facts(
            cores_n(applicable),
            group_dead_count(&group.cores, &extras.configured),
        );
        // A checkbox row (`with_key`), not the `action_label` the exchange headings use, because a
        // group row genuinely HAS state: the tick says the current selection IS this group. An
        // exchange heading cannot say that — it TOGGLES, so it has no stable "on" — which is why
        // it keeps a `3/8` count instead. Ticking an already-ticked group is inert, so the tick
        // never contradicts what the click does.
        let applied = group_is_applied(&group.cores, &pickable, selected);
        menu = menu.item(
            MoonMenuItem::with_key(format!("{id}-cg-{index}"), group.name.clone())
                .checked(applied)
                .selected(applied)
                .right_label(trailing)
                .disabled(applicable == 0)
                .on_click(move |event, window, app| {
                    on_apply(
                        name.clone(),
                        event.modifiers().secondary(),
                        payload.to_vec(),
                        window,
                        app,
                    );
                }),
        );
    }

    menu
}

/// Append the two group-management actions at the very BOTTOM of the menu.
///
/// Separate from [`groups_block`], and deliberately far from it. Every row above the core list
/// pushes the cores themselves further down, and this picker's list runs to fifty-odd rows: with
/// the actions up top, creating a single group turned the head of the menu into five rows of
/// chrome before the first core. Saving and managing are rare, deliberate acts, so they pay the
/// scroll; applying a saved group is the frequent one and stays where the eye lands.
///
/// Both rows take the menu down with them: MoonUI defers popovers above the dialog layer, so a
/// menu left standing paints opaquely over the modal it just opened, and the first click into that
/// modal both dismisses the menu and yanks focus back out of the dialog.
///
/// Save is gated on the selection resolving to at least one CONFIGURED core, since a group of
/// nothing cannot be applied later. Manage is never gated: a group whose members all died must
/// stay reachable to be renamed or deleted.
///
/// Args:
///     menu: The menu being built.
///     id: Dropdown id, the prefix of the generated keys.
///     selected: Currently selected core ids.
///     selectable: Every core this consumer can select.
///     extras: The consumer's groups and handlers.
///
/// Returns:
///     The menu with a separator and the two action rows appended.
fn group_actions_block(
    menu: MoonDropdown,
    id: &'static str,
    selected: &HashSet<u64>,
    selectable: &Rc<[u64]>,
    extras: &CoreComboExtras,
) -> MoonDropdown {
    let mut menu = menu.item(MoonMenuItem::separator());
    let can_save = selectable
        .iter()
        .any(|core| saves_core(selected, &extras.configured, *core));
    let on_save = extras.on_save_group.clone();
    let save_selectable = selectable.clone();
    let save_configured = extras.configured.clone();
    let save_selected = selected.clone();
    menu = menu.item(
        MoonMenuItem::action_label(
            format!("{id}-cg-save"),
            t!("common.core_pick.save_as_group").to_string(),
        )
        .disabled(!can_save)
        .closes_menu(true)
        .on_click(move |_, window, app| {
            on_save(
                saved_group_cores(&save_selected, &save_selectable, &save_configured),
                window,
                app,
            );
        }),
    );
    if !extras.groups.is_empty() {
        let on_manage = extras.on_manage_groups.clone();
        menu = menu.item(
            MoonMenuItem::action_label(
                format!("{id}-cg-manage"),
                t!("common.core_pick.manage_groups").to_string(),
            )
            .closes_menu(true)
            .on_click(move |_, window, app| {
                on_manage(window, app);
            }),
        );
    }
    menu
}

/// Build a checkbox-based core multi-selector with clickable exchange rows.
///
/// The trigger shows `all_label` according to `all_row_mode`; every other selection shows
/// `cores_n(N)` for the available selected cores, including one core. Fixed trigger geometry keeps
/// the open menu anchored while selections change. MoonUI fits the menu and truncates labels
/// against its own row geometry. The All item passes `None` to the toggle callback and means "clear
/// the filter"; a core item passes its id. Each exchange row submits its member ids to the exchange
/// callback.
///
/// Args:
///     id: Dropdown id and prefix for generated item keys.
///     cores: Ordered core ids and display names — everything this consumer can select.
///     venues: What each identified core is connected to, keyed by core id.
///     selected: Currently selected core ids.
///     all_row_mode: Whether a complete explicit selection also represents the All row.
///     all_label: Localized label for the All item and every state that activates it.
///     cores_n: Localized formatter for every partial selection count.
///     min_menu_w: Caller-specific lower bound for the fitted menu width.
///     extras: The saved-groups block and its bottom actions, or `None` for a pinned selector
///         that takes no input.
///     on_toggle: Callback receiving `None` for All or `Some(id)` for one core.
///     on_toggle_exchange: Callback receiving the core ids of the clicked exchange section.
///
/// Returns:
///     The configured multi-select dropdown.
#[allow(clippy::too_many_arguments)]
pub(crate) fn core_combo<F, G>(
    id: &'static str,
    cores: &OrderedCores,
    venues: &HashMap<u64, CoreVenue>,
    selected: &HashSet<u64>,
    all_row_mode: CoreAllRowMode,
    all_label: String,
    cores_n: impl Fn(usize) -> String,
    min_menu_w: f32,
    extras: Option<CoreComboExtras<'_>>,
    on_toggle: F,
    on_toggle_exchange: G,
) -> MoonDropdown
where
    F: Fn(Option<u64>, &mut App) + Clone + 'static,
    G: Fn(Vec<u64>, &mut App) + 'static,
{
    let on_toggle_exchange: Rc<dyn Fn(Vec<u64>, &mut App)> = Rc::new(on_toggle_exchange);
    let summary = core_selection_summary(cores, selected, all_row_mode, &all_label, &cores_n);
    let (cur, all_on) = (summary.label, summary.all_on);
    let sections = core_menu_sections(cores, venues);
    let toggle_all = on_toggle.clone();
    let mut menu = MoonDropdown::new(id)
        .label(cur)
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Action)
        .trigger_width_scaled(CORE_COMBO_TRIGGER_W)
        .fit_menu_width(min_menu_w, 560.0)
        .menu_max_height_ui(520.0)
        .menu_size(MoonMenuSize::Compact)
        .close_on_select(false)
        .item(
            // The All item clears the filter in every consumer: empty means all cores.
            MoonMenuItem::with_key(format!("{id}-all"), all_label)
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| toggle_all(None, app)),
        );
    // The selectable ids are shared by every row handler below, so they are built once here and
    // handed down rather than re-collected per block.
    let selectable: Option<Rc<[u64]>> = extras
        .is_some()
        .then(|| cores.iter().map(|(core, _)| *core).collect());
    if let (Some(extras), Some(selectable)) = (extras.as_ref(), selectable.as_ref()) {
        menu = groups_block(menu, id, selected, selectable, &cores_n, extras);
    }
    if !sections.is_empty() {
        menu = menu.item(MoonMenuItem::separator());
    }
    for (section_index, (venue, members)) in sections.into_iter().enumerate() {
        if members.is_empty() {
            continue;
        }
        let exchange_label = venue_section_label(venue);
        let exchange_cores: Vec<u64> = section_core_ids(&members);
        let trailing = exchange_state_label(group_check_state(&exchange_cores, selected));
        let on_section = on_toggle_exchange.clone();
        let mut row =
            MoonMenuItem::action_label(format!("{id}-exchange-{section_index}"), exchange_label)
                .on_click(move |_, _, app| {
                    on_section(exchange_cores.clone(), app);
                });
        if let Some(trailing) = trailing {
            row = row.right_label(trailing);
        }
        menu = menu.item(row);
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
    if let (Some(extras), Some(selectable)) = (extras.as_ref(), selectable.as_ref()) {
        menu = group_actions_block(menu, id, selected, selectable, extras);
    }
    menu
}
