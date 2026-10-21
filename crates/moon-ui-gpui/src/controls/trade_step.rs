//! The trade clusters' price-step list beside the price scale, for the tab strip and a detached
//! chart window.
//!
//! It shows only while the target draws its trades as bubbles. A pick is for the running session:
//! the stored step is the graphics popup's, and this list never writes it — picking the stored
//! step again simply returns the tab to following it.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButtonSegment, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonPalette, MoonSize,
    MoonTooltipView,
};

/// Label of a cluster step: `x2` for a fixed step, the localized Auto for `0`.
///
/// Shared by this list and the graphics popup, so the two name a step the same way.
pub(crate) fn trade_step_label(step: u16) -> String {
    if step == 0 {
        t!("chart.graphics.trade_step_auto").to_string()
    } else {
        format!("x{step}")
    }
}

/// The session value a pick stores: `None` when it names the tab's stored step, so the tab goes
/// back to following the popup, `Some` otherwise.
pub(crate) fn session_step(picked: u16, stored: u16) -> Option<u16> {
    (picked != stored).then_some(picked)
}

/// Build the cluster step dropdown.
///
/// Args:
///     id: Element id prefix, unique per host.
///     stored: The tab's stored step from its graphics settings.
///     session: The step picked for this session, `None` while following `stored`.
///     p: Current Moon palette.
///     on_pick: Receives the new session value (see [`session_step`]).
///
/// Returns:
///     The dropdown with its tooltip.
pub(crate) fn trade_step_dropdown(
    id: &'static str,
    stored: u16,
    session: Option<u16>,
    p: MoonPalette,
    on_pick: impl Fn(Option<u16>, &mut App) + Clone + 'static,
) -> AnyElement {
    let current = session.unwrap_or(stored);
    let items = moon_chart::clusters::CLUSTER_STEPS
        .iter()
        .map(|&step| {
            let on_pick = on_pick.clone();
            MoonMenuItem::with_key(format!("{id}-{step}"), trade_step_label(step))
                .selected(step == current)
                .checked(step == current)
                .on_click(move |_, _, cx| on_pick(session_step(step, stored), cx))
        })
        .collect::<Vec<_>>();
    div()
        .id(SharedString::from(format!("{id}-tip")))
        .tooltip(|_window, cx| {
            cx.new(|_| MoonTooltipView::new(t!("chart.strip.trade_step_tip").to_string()))
                .into()
        })
        .child(
            MoonDropdown::new(SharedString::from(format!("{id}-dropdown")))
                .trigger_width_scaled(58.0)
                .trigger_variant(MoonButtonVariant::Neutral)
                .trigger_size(MoonSize::Xs.into())
                .fit_menu_width(96.0, 560.0)
                .segment(
                    MoonButtonSegment::new("⇅")
                        .color(p.text_muted)
                        .weight(400.0),
                )
                .segment(
                    MoonButtonSegment::new(trade_step_label(current))
                        .color(p.text)
                        .weight(500.0),
                )
                .items(items),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
