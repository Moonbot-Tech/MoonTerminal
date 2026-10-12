//! Header quiet-mode ("sleep") cluster: the labelled sleep button and the ⚙ that opens its
//! settings.
//!
//! It stands right of the rate ticker, so `shell::ticker` positions the ticker's popup with this
//! cluster inside the span it sums (`design::HeaderWidths::right_of_ticker`). The cluster
//! therefore renders at an EXPLICIT [`header_quiet_width`] rather than sizing to its content —
//! measurement and layout are the same number by construction.
//!
//! The button names the CURRENT state: a bell and "Sounds" while awake, a moon and "Sleep" while
//! asleep, accent-filled so the withheld sounds read at a glance. The label changes width between
//! the two states; that is safe because the header budget, and with it the ticker popup offset,
//! is measured from the current state on every render. The label is the first thing the header
//! sheds (level 1); the glyph keeps the state and the tooltip keeps the words.

use gpui::*;
use moon_ui::{MoonButton, MoonButtonIconSlot, MoonButtonVariant, MoonPopoverPlacement, h_flex};
use rust_i18n::t;

use crate::controls::launch::{LAUNCHER_PAD_X, LAUNCHER_TEXT_WEIGHT};
use crate::panels::common::popup_gear_trigger;
use crate::shell::Shell;
use crate::{Backend, design};

/// The button's label for the current state.
fn button_label(sleeping: bool) -> String {
    if sleeping {
        t!("quiet.toggle").to_string()
    } else {
        t!("quiet.label_off").to_string()
    }
}

/// The button's glyph for the current state.
fn button_icon(sleeping: bool) -> &'static str {
    if sleeping {
        "icons/moon.svg"
    } else {
        "icons/bell.svg"
    }
}

/// Rendered width of the label part of the sleep button: what the header saves when it drops it.
pub(crate) fn header_quiet_label_width(cx: &App, sleeping: bool) -> f32 {
    header_quiet_width(cx, sleeping, true) - header_quiet_width(cx, sleeping, false)
}

/// The width of the trailing gear and the gap before it, which every cluster width carries.
fn gear_span(cx: &App) -> f32 {
    // The gear is `popup_gear_trigger`, icon-only at the control tier, so MoonUI draws it square
    // and its width IS its height.
    design::ui_value(cx, design::CHROME_GAP) + design::action_control_h_value(cx)
}

/// Rendered width of the whole cluster, in the units the header lays its children out with.
///
/// The header renders the cluster AT this width and `shell::ticker` offsets its popup BY it (through
/// `HeaderWidths`), so the two cannot disagree. The labelled button is the launcher geometry
/// (`controls::launch::launcher_label_width`): glyph, gap, both insets, the two border pixels and
/// the label in the UI family the cluster renders it in.
///
/// Args:
///     cx: Application context used to measure the active typography and UI scale.
///     sleeping: Whether the terminal is asleep, which picks the label.
///     label: Whether the label is drawn at the current header level.
///
/// Returns:
///     Button, the chrome gap, and the square gear button.
pub(crate) fn header_quiet_width(cx: &App, sleeping: bool, label: bool) -> f32 {
    let button = if label {
        crate::controls::launch::labeled_button_width(
            cx,
            &button_label(sleeping),
            LAUNCHER_TEXT_WEIGHT,
            true,
        )
    } else {
        design::glyph_btn_w(cx)
    };
    button + gear_span(cx)
}

/// What the cluster acts on; `None` renders it inert with the same geometry.
pub(crate) struct QuietActions {
    pub backend: Entity<Backend>,
    pub shell: Entity<Shell>,
    /// Whether the settings popover is open.
    pub open: bool,
    /// Lazily built settings content, present only while `open`.
    pub content: Option<AnyElement>,
}

/// Build the header's quiet-mode cluster.
///
/// Args:
///     sleeping: Whether the terminal is asleep.
///     label: Whether the button's label is drawn at the current header level.
///     actions: Backend and Shell the button and gear act on; `None` renders them inert.
///     p: Active palette.
///     cx: Application context used to scale metrics.
///
/// Returns:
///     The fixed-width button-plus-gear cluster.
pub(crate) fn header_quiet_cluster(
    sleeping: bool,
    label: bool,
    actions: Option<QuietActions>,
    p: moon_ui::MoonPalette,
    cx: &App,
) -> AnyElement {
    let tooltip = if sleeping {
        t!("quiet.tip_on")
    } else {
        t!("quiet.tip_off")
    }
    .to_string();
    let text = button_label(sleeping);
    // Accent-filled while asleep: sounds the operator relies on are being withheld, and the state
    // must read from across the screen. Awake it is ordinary chrome.
    let variant = if sleeping {
        MoonButtonVariant::Blue
    } else {
        MoonButtonVariant::Soft
    };
    let fg = if sleeping { p.accent } else { p.text_soft };
    let mut button = MoonButton::new("header-sleep")
        .width(header_quiet_width(cx, sleeping, label) - gear_span(cx))
        .variant(variant)
        .selected(sleeping)
        .leading_icon(MoonButtonIconSlot::new(button_icon(sleeping)).color(fg))
        .tooltip(tooltip)
        // The label reads in the UI family the width above measures; the section below sets it.
        .mono(false);
    if label {
        button = button
            .padding_x(LAUNCHER_PAD_X)
            .text_segment(text, fg, LAUNCHER_TEXT_WEIGHT);
    }
    let (gear_open, content, gear_shell) = match actions {
        Some(QuietActions {
            backend,
            shell,
            open,
            content,
        }) => {
            // The click carries no state; the authority is the backend: a schedule can have the
            // terminal asleep with nothing switched on by hand, and only `toggle_quiet` knows
            // whether that means "sleep now" or "wake this window".
            button = button.on_click(move |_, _window, cx| {
                backend.update(cx, |b, bcx| b.toggle_quiet(bcx));
            });
            (open, content, Some(shell))
        }
        None => (false, None, None),
    };
    let gear = popup_gear_trigger(
        "header-quiet-gear",
        t!("quiet.gear_tip").to_string(),
        gear_open,
    );
    h_flex()
        .id("header-quiet")
        .debug_selector(|| "header-quiet".into())
        .flex_none()
        .items_center()
        .w(px(header_quiet_width(cx, sleeping, label)))
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .font_family(design::ui_font())
        .child(button.render())
        // `BottomEnd`: this cluster sits near the window's right edge, and a start-anchored popup
        // of this width would open past it.
        .child(match gear_shell {
            Some(shell) => crate::chrome::terminal_chrome::header_gear_popover(
                "header-quiet-gear",
                MoonPopoverPlacement::BottomEnd,
                crate::shell::quiet_popup::CONTENT_W,
                gear_open,
                content,
                gear,
                move |open, window, cx| {
                    shell.update(cx, |s, cx| s.set_quiet_settings_open(open, window, cx));
                },
            )
            .into_any_element(),
            None => gear.into_any_element(),
        })
        .into_any_element()
}
