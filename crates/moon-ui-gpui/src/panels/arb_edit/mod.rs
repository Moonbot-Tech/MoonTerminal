//! The arbitrage roster window: which venues the chart's arbitrage column lists, in what order,
//! under what name and in what colour.
//!
//! Its own window rather than a section of the caption editor, because what it edits is not a
//! caption: the roster is GLOBAL — one answer for every chart and every tab — while the module that
//! prints it is per tab like every other caption module. Two lifetimes, two places to set them.
//!
//! Everything here is the terminal's own. The core reports a numeric platform code and a price; the
//! name, the colour, the order and the visibility exist only in `arb_view.toml`, which is also why
//! a Hyperliquid deployer can be renamed at all — it reaches us as an index and nothing else.
//!
//! Writes go straight through rather than waiting for an OK: the file is small, and a roster that
//! only landed at the end would leave the user picking colours against a chart that shows the old
//! ones. Closing the window is therefore not a cancel, and the footer offers a RESET instead.

use std::rc::Rc;

use gpui::*;
use moon_core::config::ArbViewCfg;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInputState, MoonPalette, MoonWindowExt as _,
    h_flex,
};
use rust_i18n::t;

use crate::design::{self, moon};

mod body;

#[cfg(test)]
mod tests;

use body::dialog_body;

/// Working state of the open window.
pub struct ArbEditState {
    /// The roster as it stands. Edited in place and published on every change.
    cfg: ArbViewCfg,
    /// Which venue's name is being typed, if any.
    ///
    /// ONE input for the whole list: a dozen live `MoonInputState` entities would be a dozen focus
    /// targets for one column of names, and only one of them can be typed into anyway.
    editing: Option<usize>,
    name_input: Entity<MoonInputState>,
    /// Hands the edited roster back: the backend saves it and every chart redraws.
    on_change: Rc<dyn Fn(ArbViewCfg, &mut App)>,
    /// Run when the window goes away, whichever way: the ✕, the overlay, Escape.
    ///
    /// Separate from [`Self::on_change`], which fires on every click: the caller uses this to bring
    /// back the popup it closed, and doing that per edit would paint the popover over this window.
    on_dismiss: Rc<dyn Fn(&mut App)>,
}

impl ArbEditState {
    /// Apply an edit and publish it in one step.
    ///
    /// Both halves always happen together: a change that reached this state but not the backend
    /// would show in the window and nowhere else, which is the bug a live-writing window invites.
    fn write(state: &Entity<Self>, cx: &mut App, f: impl FnOnce(&mut ArbViewCfg)) {
        let (cfg, on_change) = state.update(cx, |s, cx| {
            f(&mut s.cfg);
            s.cfg.sanitize();
            cx.notify();
            (s.cfg.clone(), s.on_change.clone())
        });
        on_change(cfg, cx);
    }

    /// Commit whatever is in the name field to the venue it was opened on.
    ///
    /// Read at the END of the edit rather than on every keystroke: a name is sanitized on write —
    /// trimmed and cut — and doing that under the cursor would fight the person typing.
    fn commit_name(state: &Entity<Self>, cx: &mut App) {
        let Some((ix, text)) = state.read(cx).editing.map(|ix| {
            (
                ix,
                state.read(cx).name_input.read(cx).value().trim().to_string(),
            )
        }) else {
            return;
        };
        Self::write(state, cx, |cfg| {
            if let Some(venue) = cfg.venues.get_mut(ix) {
                venue.name = text;
            }
        });
        state.update(cx, |s, cx| {
            s.editing = None;
            cx.notify();
        });
    }
}

/// Open the arbitrage roster window.
///
/// Args:
///     cfg: The roster as it stands now.
///     on_change: Applied with the edited roster on every change; saves and republishes it.
pub(crate) fn open_arb_edit(
    cfg: ArbViewCfg,
    window: &mut Window,
    cx: &mut App,
    on_change: impl Fn(ArbViewCfg, &mut App) + 'static,
    on_dismiss: impl Fn(&mut App) + 'static,
) {
    let name_input =
        cx.new(|cx| MoonInputState::new(window, cx).placeholder(t!("arb.name_hint").to_string()));
    let state = cx.new(|_| ArbEditState {
        cfg,
        editing: None,
        name_input,
        on_change: Rc::new(on_change),
        on_dismiss: Rc::new(on_dismiss),
    });

    window.open_unique_moon_dialog("chart-arb-edit", cx, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let content_state = state.clone();
        let footer_state = state.clone();
        let dismiss_state = state.clone();
        let dismiss = state.read(cx).on_dismiss.clone();
        dialog
            // Wide enough for a venue line: two order buttons, an eye, a name, a swatch row.
            .w(px(520.0))
            .close_button(true)
            .overlay(true)
            .overlay_closable(true)
            .bg(moon(p.shell_high))
            .border_color(moon(p.border))
            .rounded(design::r_container(cx))
            .text_color(moon(p.text))
            .header(
                div()
                    .w_full()
                    .py_2()
                    .border_b_1()
                    .border_color(moon(p.border))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("arb.title").to_string()),
            )
            // Closing is not a cancel — every edit is already live — but a name still being TYPED
            // has not been written yet, and dropping it on the ✕ would lose exactly the edit the
            // user was in the middle of. Committed here, then the caller gets its popup back.
            .on_cancel(move |_, _, cx: &mut App| {
                ArbEditState::commit_name(&dismiss_state, cx);
                dismiss(cx);
                true
            })
            .content(move |content, _window, cx| content.child(dialog_body(&content_state, cx)))
            .footer(dialog_footer(footer_state, p))
    });
}

/// The roster's own footer: what the window is, and the way back to the shipped list.
///
/// No OK and no Cancel, and that is the point — every edit above is already live. A Cancel here
/// would have to undo writes the chart has already drawn.
fn dialog_footer(state: Entity<ArbEditState>, p: MoonPalette) -> AnyElement {
    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_color(moon(p.text_muted))
                .child(t!("arb.hint").to_string()),
        )
        .child(
            MoonButton::new("arb-reset")
                .ghost()
                .size(MoonButtonSize::Micro)
                .label(t!("arb.reset").to_string())
                .variant(MoonButtonVariant::Ghost)
                .on_click(move |_, _w, cx: &mut App| {
                    ArbEditState::write(&state, cx, |cfg| *cfg = ArbViewCfg::default());
                })
                .render(),
        )
        .into_any_element()
}
