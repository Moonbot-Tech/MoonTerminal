//! Core-owned manual strategy and leverage helpers of the toolbar.

use super::*;

/// Prefer the hovered chart's core when deciding whether its core-owned manual strategy applies.
pub(crate) fn manual_strategy_core(
    active_core: Option<CoreId>,
    hovered_core: Option<CoreId>,
    hovered_belongs_to_group: bool,
) -> Option<CoreId> {
    hovered_core
        .filter(|_| hovered_belongs_to_group)
        .or(active_core)
}

/// Resolve the chart core whose manual-trading config governs the toolbar's size/exit block —
/// deliberately NOT [`crate::Backend::active_trade_core`], which never answers `None` for a group
/// with live cores (it falls through to that group's first core) and so cannot express "no chart
/// is addressed right now". With no chart in front of the user this answers `None`, and the
/// toolbar then shows group-local values with no core marker — the honest answer.
///
/// This is deliberately NOT the core an order would route to (that is
/// [`effective_manual_strategy_core`]) — goal A had two different "which core" rules in this
/// area and merging them was the bug that doc comment exists to prevent.
///
/// Priority: the hovered chart in this group, then the group's Main chart target, then the
/// remembered Classic selection. Pure and arg-taking, the same shape as [`manual_strategy_core`]
/// and for the same reason its own test exists.
pub(crate) fn chart_display_core(
    hovered: Option<CoreId>,
    hovered_in_group: bool,
    main_target: Option<CoreId>,
    remembered: Option<CoreId>,
) -> Option<CoreId> {
    hovered
        .filter(|_| hovered_in_group)
        .or(main_target)
        .or(remembered)
}

/// Resolve [`chart_display_core`] against live backend state.
///
/// Reads `hovered_chart` through [`crate::panels::ChartPanel::active_target`] rather than
/// `target_at_cursor`: the latter answers `None` the instant the pointer leaves the hovered pane,
/// even while `hovered_chart` itself is still set, which would make the gate flicker away and back
/// on ordinary pane-to-pane pointer movement. `active_target` stays with the panel regardless of
/// pane-level hover, which is also what makes a DETACHED window keep naming its core: the `Pane` is
/// owned by the `ChartEngine` inside the `ChartPanel`, and a detached window re-hosts the same
/// entity, so nothing about the toolbar's group window needs to know the chart left it.
pub(crate) fn effective_chart_display_core(
    backend: &Entity<Backend>,
    group: &str,
    cx: &App,
) -> Option<CoreId> {
    let hovered_core = backend
        .read(cx)
        .hovered_chart
        .clone()
        .and_then(|weak| weak.upgrade())
        .and_then(|chart| chart.read(cx).active_target())
        .map(|(core, _)| core);
    let b = backend.read(cx);
    chart_display_core(
        hovered_core,
        hovered_core.is_some_and(|core| b.core_belongs_to_group(group, core)),
        b.main_chart_target(group).map(|(core, _)| core),
        b.layout
            .active_trade_core_by_group
            .get(group)
            .copied()
            .filter(|&core| b.core_belongs_to_group(group, core)),
    )
}

/// Per-core "keep your own manual-trading set" switch, drawn immediately right of the order-size
/// strip.
///
/// `None` when no chart core is addressed: the switch names ONE core's generation, so a row with
/// no core to name must not draw it. It addresses the same `display_core` the strip beside it was
/// rendered from, so the switch and the numbers it governs can never describe different cores.
///
/// Flipping it on moves this core off its group's shared generation and onto its own, seeded from
/// the group so the numbers do not move under the trader's hands; flipping it off returns the row
/// to the group's, keeping the core's own set for the next time it is switched back on.
pub(super) fn own_trade_toggle(
    core: Option<CoreId>,
    on: bool,
    backend: &Entity<Backend>,
    roles: bool,
) -> Option<AnyElement> {
    let core = core?;
    let toggle_backend = backend.clone();
    let tip = if on {
        "toolbar.own_trade_on"
    } else {
        "toolbar.own_trade_off"
    };
    Some(
        h_flex()
            .id("toolbar-own-trade")
            .flex_none()
            .items_center()
            .child(
                MoonToggle::new("toolbar-own-trade-toggle")
                    .checked(on)
                    .when_some(design::chrome_toggle_tone(on, false, roles), |t, tone| {
                        t.tone(tone)
                    })
                    .on_change(move |checked: &bool, _w, app| {
                        let on = *checked;
                        toggle_backend.update(app, |b, cx| {
                            b.set_core_own_trade(core, on);
                            cx.notify();
                        });
                    }),
            )
            .tooltip(text_tooltip(SharedString::from(t!(tip).to_string())))
            .into_any_element(),
    )
}

/// Caption naming one coarse [`CoreConfigArea`] the core rejected. `moon-core` cannot localize, so
/// the mapping lives here like every other caption of a `moon-core` enum in this module.
fn area_caption(area: CoreConfigArea) -> String {
    let key = match area {
        CoreConfigArea::AutoBuy => "toolbar.core_config_area_auto_buy",
        CoreConfigArea::FavMarkets => "toolbar.core_config_area_fav_markets",
        CoreConfigArea::AutoStart => "toolbar.core_config_area_auto_start",
        CoreConfigArea::BtcBlink => "toolbar.core_config_area_btc_blink",
        // Both halves of Moonbot's "Основные" page answer to its name. They are two areas
        // because two surfaces draw different parts of it, which is a fact about this terminal's
        // internals; a trader reading a rejection knows one page. The join below de-duplicates, so
        // an OK that failed on both does not print the name twice.
        CoreConfigArea::General | CoreConfigArea::OrderRules => "toolbar.core_config_area_general",
        CoreConfigArea::Gestures => "toolbar.core_config_area_gestures",
        CoreConfigArea::Interface => "toolbar.core_config_area_interface",
        CoreConfigArea::Leverage => "toolbar.core_config_area_leverage",
        CoreConfigArea::Manual => "toolbar.core_config_area_manual",
        CoreConfigArea::Signals => "toolbar.core_config_area_signals",
        CoreConfigArea::Special => "toolbar.core_config_area_special",
        CoreConfigArea::Telegram => "toolbar.core_config_area_telegram",
    };
    t!(key).to_string()
}

/// Caption naming a [`CoreConfigRejection::Areas`] a core's one retained edit carries, or `None`
/// while nothing is rejected.
///
/// Two readers of `CoreData::core_config_edit`: the toolbar's per-cell tip here, and the expert
/// core-settings window's banner. The gear popup writes AutoStart, BtcBlink, General and Leverage
/// through the shared-config sequence, and a core that refuses one of them resolves the edit as
/// `NotApplied` — without this the popup would close exactly as it does on success and the refusal
/// would never reach the screen; the expert window's Apply keeps the page open over values the
/// core may have refused, which is why it draws the same caption.
pub(crate) fn core_config_rejection_caption(
    mismatches: Option<&CoreConfigRejection>,
) -> Option<SharedString> {
    let Some(CoreConfigRejection::Areas(areas)) = mismatches else {
        return None;
    };
    if areas.is_empty() {
        return None;
    }
    let mut captions: Vec<String> = Vec::with_capacity(areas.len());
    for caption in areas.iter().map(|&area| area_caption(area)) {
        // Two areas can share one caption when they are two halves of one Moonbot page.
        if !captions.contains(&caption) {
            captions.push(caption);
        }
    }
    let areas = captions.join(", ");
    Some(SharedString::from(
        t!("toolbar.core_config_areas_rejected", areas = areas).to_string(),
    ))
}

/// Resolve the core whose manual-strategy state governs the toolbar and any open metric popup.
pub(crate) fn effective_manual_strategy_core(
    backend: &Entity<Backend>,
    group: &str,
    cx: &App,
) -> Option<CoreId> {
    let hovered_core = backend
        .read(cx)
        .hovered_chart
        .clone()
        .and_then(|weak| weak.upgrade())
        .and_then(|chart| chart.read(cx).target_at_cursor())
        .map(|(core, _)| core);
    let backend = backend.read(cx);
    manual_strategy_core(
        backend.active_trade_core(group),
        hovered_core,
        hovered_core.is_some_and(|core| backend.core_belongs_to_group(group, core)),
    )
}
