//! Header quiet-mode ("sleep") cluster: the work/sleep toggle and the ⚙ that opens its settings.
//!
//! It stands between the rate ticker and the clock. That position has a cost: `shell::ticker`
//! positions the ticker's popup by hand-summing everything that stands to the ticker's RIGHT, and
//! this cluster is now part of that span. So the cluster renders at an EXPLICIT
//! [`header_quiet_width`] rather than sizing to its content — measurement and layout are then the
//! same number by construction, and no font or locale change can drift them apart.
//!
//! The toggle's label is the same word in both states for the same reason: a label that changed
//! between "Work" and "Sleep" would resize the cluster on every click and slide the ticker popup
//! with it. State is shown in COLOUR instead — label and switch both turn amber while asleep,
//! which changes nothing about the geometry. Amber rather than the ordinary accent because this is
//! a "something is switched off" state: sounds the operator normally relies on are being withheld,
//! and that should read as a caution the moment the header is glanced at.
//!
//! The label is drawn here rather than handed to `MoonToggle` precisely so it can carry that
//! colour: the component paints its own label in `text_soft` with no way to override it.

use gpui::*;
use moon_ui::{MoonPopoverPlacement, MoonToggle, MoonToggleSize, h_flex};
use rust_i18n::t;

use crate::panels::popup_gear_trigger;
use crate::shell::Shell;
use crate::{Backend, design};

/// `MoonToggleSize::Compact` geometry, mirrored from MoonUI's private `ToggleMetrics` exactly as
/// `design::glyph_btn_w` mirrors its button metrics. Nothing checks this automatically — if
/// MoonUI's Compact toggle moves, this follows by hand.
///
/// Only the SWITCH is mirrored. The label beside it is drawn at the terminal's own caption step
/// (`design::t_caption`), not at the component's 9.5: this file draws that text itself, and the
/// three named steps in `design.rs` are the only sizes terminal text is allowed to take, so the
/// Font slider moves it with everything else.
const TOGGLE_TRACK_W: f32 = 28.0;
const TOGGLE_GAP: f32 = 7.0;
/// Normal weight, matching what `div()` draws with and what MoonUI gives its own toggle label.
const TOGGLE_LABEL_WEIGHT: f32 = 400.0;

/// The toggle's fixed label, drawn in both states.
fn toggle_label() -> String {
    t!("quiet.toggle").to_string()
}

/// Rendered width of the whole cluster, in the units the header lays its children out with.
///
/// The header renders the cluster AT this width and `shell::ticker` offsets its popup BY it, so the
/// two cannot disagree. Glyph advances only, like `clock::header_clock_width`.
///
/// Args:
///     cx: Application context used to measure the active typography and UI scale.
///
/// Returns:
///     Toggle (label + gap + track), the chrome gap, and the square gear button.
pub(crate) fn header_quiet_width(cx: &App) -> f32 {
    let label = design::mono_caption_text_width(cx, &toggle_label(), TOGGLE_LABEL_WEIGHT);
    label
        + design::ui_value(cx, TOGGLE_GAP)
        + design::ui_value(cx, TOGGLE_TRACK_W)
        + design::ui_value(cx, design::CHROME_GAP)
        // The gear is `popup_gear_trigger`, icon-only at Micro, so MoonUI draws it square and its
        // width IS its height.
        + design::micro_control_h_value(cx)
}

/// Build the header's quiet-mode cluster.
///
/// Args:
///     backend: Shared authority for the quiet state and its persistence.
///     shell: Header owner holding the settings popover's open state and editors.
///     open: Whether the settings popover is open.
///     content: Lazily built settings content, present only while `open`.
///     p: Active palette, supplying the amber that marks the sleeping state.
///     cx: Application context used to read state and scale metrics.
///
/// Returns:
///     The fixed-width toggle-plus-gear cluster.
pub(crate) fn header_quiet_cluster(
    backend: &Entity<Backend>,
    shell: Entity<Shell>,
    open: bool,
    content: Option<AnyElement>,
    p: moon_ui::MoonPalette,
    cx: &App,
) -> AnyElement {
    let sleeping = backend.read(cx).quiet_sleeping();
    let tooltip = if sleeping {
        t!("quiet.tip_on")
    } else {
        t!("quiet.tip_off")
    }
    .to_string();
    let toggle_backend = backend.clone();
    h_flex()
        .flex_none()
        .items_center()
        .w(px(header_quiet_width(cx)))
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .child(
            div()
                .id("header-quiet-tip")
                .flex()
                .items_center()
                .gap(design::ui_px(cx, TOGGLE_GAP))
                .tooltip(crate::panels::common::text_tooltip(tooltip))
                .child(
                    div()
                        .flex_none()
                        .font_family(design::mono())
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(design::chrome_toggle_label_color(p, sleeping, true)))
                        .child(toggle_label()),
                )
                .child(
                    MoonToggle::new("header-quiet")
                        .checked(sleeping)
                        .size(MoonToggleSize::Compact)
                        // Amber while asleep, matching the label: the switch is the larger target
                        // for the eye, so both have to carry the state or it reads as decoration.
                        .tone(design::chrome_toggle_tone(sleeping, true))
                        // The click carries `checked`, but the authority is the backend: a schedule
                        // can have the terminal asleep with nothing switched on by hand, and only
                        // `toggle_quiet` knows whether that means "sleep now" or "wake this window".
                        .on_change(move |_checked, _window, cx| {
                            toggle_backend.update(cx, |b, bcx| b.toggle_quiet(bcx));
                        }),
                ),
        )
        // `BottomEnd`: this cluster sits near the window's right edge, and a start-anchored popup
        // of this width would open past it.
        .child(crate::chrome::terminal_chrome::header_gear_popover(
            "header-quiet-gear",
            MoonPopoverPlacement::BottomEnd,
            crate::shell::quiet_popup::CONTENT_W,
            open,
            content,
            // The same gear the Detects view configurator uses, so every settings popup in the
            // terminal is opened by one control: Micro, icon-only, lit while its popup is up.
            popup_gear_trigger("header-quiet-gear", t!("quiet.gear_tip").to_string(), open),
            move |open, window, cx| {
                shell.update(cx, |s, cx| s.set_quiet_settings_open(open, window, cx));
            },
        ))
        .into_any_element()
}
