//! Settings popup for the header's manual-strategy quick-select buttons.
//!
//! One gear beside the MS cluster, in the same shape as every other gear popup in the chrome
//! (`chrome::terminal_chrome::header_gear_popover`). It owns everything about the ten buttons that
//! is not a single click: which slots are drawn, what each one is called, and the one CORE-side
//! flag that decides whether the toolbar's TP/S apply at all while a manual strategy is active.
//!
//! Slot captions and visibility are the terminal's own per-core state (`ServerConfig::strat_slots`)
//! — see `Backend::strat_slots`. `Pull from the core` replaces them with Moonbot's own arrangement,
//! and is the way back after any local edit.

use gpui::*;
use moon_core::config::MANUAL_STRAT_SLOTS;
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonInput,
    MoonInputState, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use crate::shell::Shell;
use crate::{Backend, design};

/// Width of the caption field, wide enough for the strategy names Moonbot ships with.
const LABEL_INPUT_W: f32 = 150.0;
/// Width of the "fires" column, which names the strategy a slot is assigned to.
const STRATEGY_COL_W: f32 = 150.0;
/// Width of the hotkey column, showing the terminal binding this slot answers to.
const HOTKEY_COL_W: f32 = 78.0;

/// Build the popup's content: ten slot rows, the core's sell-price flag, and the pull action.
///
/// Args:
///     core: Core whose slots are being edited — the group's active trade core.
///     backend: Application state read for slots and written by every control here.
///     shell: Shell that owns the popup's open state and its caption fields.
///     inputs: One caption field per slot, seeded when the popup opened.
///     p: Active palette.
///     cx: Application context.
///
/// Returns:
///     The popup body.
pub(super) fn slot_settings_content(
    core: CoreId,
    backend: &Entity<Backend>,
    shell: &Entity<Shell>,
    inputs: &[Entity<MoonInputState>],
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let b = backend.read(cx);
    let slots = b.strat_slots(core).unwrap_or_default();
    // The TERMINAL's own bindings (Settings > Hotkeys > manual strategy), shown read-only: this
    // popup is where a trader looks to see which key fires which button, and sending them back to
    // the hotkeys tab to find out is the friction it exists to remove.
    let hotkeys = b.config.hotkeys.manual_strategy.clone();
    let ignore_sell = b.ignore_strat_sell_price(core);
    // Pulling reads the core's OWN configuration, so it is available exactly while that
    // configuration is. `ignore_strat_sell_price` answering at all is that proof — it is projected
    // from the same snapshot — whereas local slots prove only that this terminal has its own.
    let core_known = ignore_sell.is_some();
    let gap = design::ui_px(cx, 6.0);

    let mut rows = v_flex().gap(gap).w_full();
    for slot in 0..MANUAL_STRAT_SLOTS {
        let current = slots.get(slot).cloned().unwrap_or_default();
        let show_backend = backend.clone();
        let strategy_label = if current.strategy.trim().is_empty() {
            t!("header.ms_slot_unassigned").to_string()
        } else {
            current.strategy.clone()
        };
        rows = rows.child(
            h_flex()
                .w_full()
                .items_center()
                .gap(gap)
                .child(
                    MoonCheckbox::new(SharedString::from(format!("ms-slot-show-{slot}")))
                        .label(format!("{}", slot + 1))
                        .checked(current.show)
                        .size(MoonCheckboxSize::Compact)
                        .on_change(move |checked: &bool, _w, app| {
                            let show = *checked;
                            show_backend.update(app, |b, cx| {
                                b.set_strat_slot_show(core, slot, show);
                                cx.notify();
                            });
                        }),
                )
                .children(inputs.get(slot).map(|input| {
                    div().w(px(design::font_w(cx, LABEL_INPUT_W))).child(
                        MoonInput::new(SharedString::from(format!("ms-slot-label-{slot}")))
                            .state(input)
                            .small(),
                    )
                }))
                .child(
                    div()
                        .w(px(design::font_w(cx, STRATEGY_COL_W)))
                        .min_w_0()
                        .truncate()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(if current.strategy.trim().is_empty() {
                            p.text_muted
                        } else {
                            p.text_soft
                        }))
                        .child(strategy_label),
                )
                .child(
                    div()
                        .w(px(design::font_w(cx, HOTKEY_COL_W)))
                        .min_w_0()
                        .truncate()
                        .font_family(design::mono())
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(match hotkeys.get(slot).map(|key| key.trim()) {
                            Some(key) if !key.is_empty() => key.to_uppercase(),
                            _ => "—".to_string(),
                        }),
                ),
        );
    }

    let pull_backend = backend.clone();
    let pull_shell = shell.clone();
    let sell_backend = backend.clone();
    v_flex()
        .id("ms-slots-popup")
        .w_full()
        .gap(design::ui_px(cx, 10.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("header.ms_slots_hint").to_string()),
        )
        .child(rows)
        // The core's own flag, not a local one: while it is OFF the core applies a manual
        // strategy's own sell price and the toolbar's TP/S do not reach a manual order at all.
        .children(ignore_sell.map(|on| {
            MoonCheckbox::new("ms-ignore-strat-sell")
                .label(t!("header.ms_ignore_strat_sell").to_string())
                .checked(on)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |checked: &bool, _w, app| {
                    let on = *checked;
                    sell_backend.update(app, |b, cx| {
                        b.set_ignore_strat_sell_price(core, on);
                        cx.notify();
                    });
                })
        }))
        .child(
            h_flex().w_full().justify_end().child(
                MoonButton::new("ms-slots-pull")
                    .size(MoonButtonSize::ToolbarCompact)
                    .variant(MoonButtonVariant::Soft)
                    .label(t!("header.ms_slots_pull").to_string())
                    .disabled(!core_known)
                    .tooltip(t!("header.ms_slots_pull_tip").to_string())
                    .on_click(move |_, _, app| {
                        let pulled = pull_backend.update(app, |b, cx| {
                            let pulled = b.pull_strat_slots_from_core(core);
                            if pulled {
                                if let Err(error) = b.config.save() {
                                    log::warn!("save pulled strategy slots failed: {error}");
                                } else {
                                    b.config_dirty = false;
                                }
                                cx.notify();
                            }
                            pulled
                        });
                        // Re-seed the caption fields from what was just pulled; without this the
                        // fields keep showing the captions the pull replaced.
                        if pulled {
                            pull_shell.update(app, |shell, cx| {
                                shell.set_strat_slots_open(true, cx);
                                cx.notify();
                            });
                        }
                    })
                    .render(),
            ),
        )
        .into_any_element()
}
