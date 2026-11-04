//! The header's self-update button and the width it claims from the manual-strategy row.
//!
//! The button sits after the header's drag spacer, before the ticker. The manual-strategy buttons
//! to its left fit themselves against a flat reservation for the rest of the header
//! (`controls::manual_strat`), and the button exists only while a version is offered — so unless
//! its CURRENT label is added to that reservation, the names grow under it (#1191). The label and
//! its reservation are derived here from one `UpdateState`, so the two cannot disagree.

use gpui::*;
use moon_ui::{MoonButton, MoonButtonVariant, MoonPalette};
use rust_i18n::t;

use crate::design;
use crate::update::{UpdateController, UpdateState};

/// Estimated non-text chrome of the update button at design-reference scale: horizontal padding
/// and border of the default-tier `MoonButton`. Conservative like the strategy row's own
/// `BTN_CHROME_W`; `tests.rs` pins it against the rendered button at every label and font delta.
const UPDATE_BTN_CHROME_W: f32 = 28.0;
/// Extra width the busy (`loading`) face spends on its spinner and the gap after it.
const UPDATE_BTN_SPINNER_W: f32 = 20.0;
/// Weight the button label is measured at: heavier than the rendered one, so the estimate errs
/// toward reserving too much rather than letting a name slide under the button.
const UPDATE_LABEL_WEIGHT: f32 = 600.0;

/// The text on the button for `state`, or `None` while no update is offered.
pub(crate) fn update_button_label(state: &UpdateState) -> Option<String> {
    if !state.visible() {
        return None;
    }
    let version = state.version().map(|v| v.to_string()).unwrap_or_default();
    Some(if state.busy() {
        t!("update.installing").to_string()
    } else if matches!(state, UpdateState::Failed { .. }) {
        t!("update.retry").to_string()
    } else if matches!(state, UpdateState::Opened(_)) {
        t!("update.opened").to_string()
    } else {
        t!("update.available", version = version).to_string()
    })
}

/// Width the update cluster takes from the header for `state`, `0.0` while it is hidden.
///
/// Counts the button at its current label, the divider after it, and the two header gaps that
/// cluster adds to the row (before the button, and between button and divider). The trailing gap
/// before the ticker is already inside the strategy row's flat reservation.
pub(crate) fn update_cluster_reserve_w(cx: &App, state: &UpdateState) -> f32 {
    let Some(label) = update_button_label(state) else {
        return 0.0;
    };
    let spinner = if state.busy() {
        design::ui_value(cx, UPDATE_BTN_SPINNER_W)
    } else {
        0.0
    };
    update_button_estimate_w(cx, &label, spinner)
        + design::ui_value(cx, design::CHROME_GAP) * 2.0
        + 1.0
}

/// Estimated rendered width of the button alone at `label`.
fn update_button_estimate_w(cx: &App, label: &str, spinner: f32) -> f32 {
    design::ui_value(cx, UPDATE_BTN_CHROME_W)
        + spinner
        + design::ui_text_width(cx, label, design::BODY_TEXT, UPDATE_LABEL_WEIGHT, false)
}

/// The update cluster: the button (tooltip-wrapped) and the divider that separates it from the
/// ticker. `None` while no update is offered, so no empty separator is drawn.
pub(crate) fn update_cluster(
    updater: &Entity<UpdateController>,
    p: MoonPalette,
    cx: &App,
) -> Option<Div> {
    let state = updater.read(cx).state();
    let label = update_button_label(&state)?;
    let tooltip = match &state {
        UpdateState::Failed { message, .. } => t!("update.failed", error = message).to_string(),
        UpdateState::Opened(_) => t!("update.opened_tooltip").to_string(),
        _ if cfg!(target_os = "macos") => t!("update.tooltip_image").to_string(),
        _ => t!("update.tooltip").to_string(),
    };
    let updater = updater.clone();
    Some(cluster(label, &state, tooltip, p, cx, move |_, _, cx| {
        UpdateController::start_install(&updater, cx);
    }))
}

/// Section holding the button and its trailing divider, exactly as the header lays it out.
fn cluster(
    label: String,
    state: &UpdateState,
    tooltip: String,
    p: MoonPalette,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    design::chrome_section(cx)
        .child(update_button(label, state, tooltip, on_click))
        .child(design::chrome_divider(cx, p))
}

/// The button itself, split out so the layout test renders exactly what the header renders.
fn update_button(
    label: String,
    state: &UpdateState,
    tooltip: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    // An offered update is the only call to action this header ever grows, and it appears
    // on a window whose every other control is `Panel` — so neutral panel chrome is
    // exactly what a user scanning the row does not stop on.
    //
    // `Green` rather than the `Blue` primary variant, and the reason is contrast, not
    // taste. Every tinted MoonUI variant paints its LABEL in the accent colour over a
    // ~10% wash of it, so the label's legibility is the palette token's own contrast
    // against this header. In the light theme `Blue` resolves to `p.accent` `#009DFF`
    // (`button.rs:866`, `tokens.rs:190`) — about 2.5:1 on `shell_high`, far under the
    // 4.5:1 floor, and this label is 10px `Micro`. `Amber` `#B97824` is no better at
    // ~3.6:1. `Green` is the one accent MoonUI gave a light-specific foreground,
    // `green_text` `#0E6E45` (`tokens.rs:186`, `success_fg` at `button.rs:868`), which
    // clears the floor at roughly 6:1 while staying the accent in the dark theme. It is
    // also the right semantic: something good is available, not something wrong.
    //
    // Every other state keeps `Panel` deliberately. Installing/Restarting already read as
    // busy through `loading(busy)`, and Failed already reads through its retry label —
    // accenting those would advertise an action that is either underway or just refused.
    let variant = if matches!(state, UpdateState::Available(_)) {
        MoonButtonVariant::Green
    } else {
        MoonButtonVariant::Panel
    };
    div()
        .id("terminal-update-tooltip")
        .debug_selector(|| "terminal-update".into())
        .flex_none()
        .tooltip(crate::panels::common::text_tooltip(tooltip))
        .child(
            MoonButton::new("terminal-update")
                .label(label)
                .variant(variant)
                .loading(state.busy())
                .on_click(on_click)
                .render(),
        )
}

#[cfg(test)]
mod tests;
