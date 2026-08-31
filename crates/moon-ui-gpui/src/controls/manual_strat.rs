//! Header toggle and picker for Moonbot manual strategies.
//!
//! State lives in the core's `ClientSettings.use_manual_strategy` and `manual_strategy_id` fields.
//! Toggle and picker changes send `ClientSettingsEdit::ManualStrategy`; the process-lifetime local
//! override exposed by `Backend::manual_strat_state` provides immediate feedback and continues to
//! take precedence over ClientSettings snapshots until replaced. Core echoes and command failures do
//! not reconcile it. When enabled, the core derives sell and stop behavior for manual orders from
//! the strategy fields, so the toolbar's TP, S slots, and SL do not apply to new orders and are
//! disabled. `effective_strat_id` in moon-core already routes manual orders through the selected
//! strategy.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonInputState, MoonMenuItem,
    MoonMenuSize, MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu, MoonSelectorPill,
    MoonSelectorSegment, MoonToggle, MoonToggleLabelSide, MoonToggleSize, h_flex,
};

use moon_core::config::MANUAL_STRAT_SLOTS;
use moon_core::feed::{ClientSettingsEdit, StrategyRow, StrategySchemaModel};
use moon_core::session::CoreId;

use crate::backend::MANUAL_STRATEGY_KIND;
use crate::shell::Shell;
use crate::{Backend, design};

mod fit;
mod settings;
use fit::{LabelMode, SlotWidths, resolve_strat_fit};

/// Pill height shared with the header's core selector; label width is capped separately.
const PILL_H: f32 = 26.0;
/// Gap between two adjacent quick-strategy buttons in the header cluster.
const BTN_GAP: f32 = 4.0;
/// Estimated non-text chrome (padding, border) of one `ToolbarCompact` button carrying one
/// segment, at design-reference scale — run through `design::ui_value` before use, like every
/// other estimate below. MoonUI computes the real value from its own metrics; this is a
/// conservative estimate for the fit ladder, pending an on-screen check.
const BTN_CHROME_W: f32 = 20.0;
/// Content width of the quick-select settings popup, in font-scaled pixels.
const SLOT_SETTINGS_W: f32 = 380.0;
/// Estimated rendered width of the settings gear, at design-reference scale, for the fit ladder —
/// same reason as [`BTN_CHROME_W`]: `MoonButton` sizes its icon-only form itself.
const SLOT_GEAR_W: f32 = 26.0;
const BTN_NAME_TEXT_SIZE: f32 = 11.0;
const BTN_TEXT_WEIGHT: f32 = 500.0;
/// Estimated rendered width of the "MS" toggle (track plus label), at design-reference scale, for
/// the same reason as [`BTN_CHROME_W`].
const MS_TOGGLE_W: f32 = 70.0;
/// Estimated non-text chrome of the picker pill at design-reference scale: leading dot, padding,
/// and border.
const PILL_CHROME_W: f32 = 40.0;
/// Reduced picker-pill label cap used once the button row has already dropped to zero buttons.
const REDUCED_PILL_MAX_W: f32 = 120.0;
/// Conservative reservation for the rest of the header (brand, workspace toggle, core selector and
/// its gear, balance, the strategy-parameter summary, the trailing spacer's minimum, and the
/// ticker/quiet/clock/window-control cluster once visible) — sections this cluster does not own
/// and cannot measure without touching them, the same reasoning `design::ticker_visible` uses for
/// its own flat threshold rather than a live remainder. Needs an on-screen check. At
/// design-reference scale like every other estimate below.
const HEADER_OTHER_SECTIONS_W: f32 = 760.0;

/// One quick-select button slot: `(slot index, strategy name, resolved strategy id, caption,
/// numeric fallback caption)`.
///
/// No hotkey segment: a button shows its CAPTION alone — the trader's own, or the strategy's name
/// when they set none. The binding belongs to the slot rather than to the label, and is stated in
/// the settings popup, where there is room for it.
type StratButtonSlot = (usize, String, Option<u64>, String, String);

/// Header "manual strategy" cluster: the MS toggle, the picker pill, and a summary of the
/// selected strategy's parameters.
///
/// `None` when the group has no active trade core or that core has no Manual-kind strategies. The
/// caller owns the separator that precedes the cluster and must drop it on `None`, otherwise the
/// header keeps a rule with nothing behind it.
///
/// `chrome_width` feeds the priority-ordered narrow-window clip (`fit::resolve_strat_fit`) that
/// governs the ten quick-select buttons and the parameter summary this cluster adds.
///
/// The buttons are drawn only while manual-strategy mode is ENABLED: with MS off they select
/// nothing a new order would use, and their width is what makes the rest of the row shift every
/// time the balance beside them changes width.
///
/// Each button carries two gestures, matching Moonbot: left click fires the slot's strategy and
/// right click assigns which strategy it fires. Everything else about the buttons — captions,
/// per-slot visibility, and the core's own sell-price flag — lives in the gear popup beside them
/// (`settings::slot_settings_content`).
#[allow(clippy::too_many_arguments)]
pub fn manual_strategy_controls(
    group: &str,
    backend: &Entity<Backend>,
    shell: &Entity<Shell>,
    slot_menu: Option<(CoreId, usize)>,
    slots_open: bool,
    label_inputs: &[Entity<MoonInputState>],
    chrome_width: f32,
    p: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    let b = backend.read(cx);
    let core = b.active_trade_core(group)?;
    let core_data = b.session.store().core(core)?;
    let manuals = manual_strategy_options(&core_data.strategies)?;
    let (on, id) = b.manual_strat_state(core);
    let sel_row = core_data.strategies.iter().find(|s| s.id == id && id != 0);
    let schema = core_data.schema.as_ref();
    // Show the Moonbot-style Buy/Sell/SL/TS summary only while the mode is enabled.
    let summary = (on && sel_row.is_some())
        .then(|| sel_row.map(|r| strat_summary(r, schema)))
        .flatten();

    // The pill shows the selected strategy, the localized none marker when no id is selected, or
    // `?` when an id exists but the selected strategy was deleted. The untruncated candidate is
    // resolved first because the fit ladder below decides which cap it renders at.
    let full_pill_text: String = match (sel_row, id) {
        (Some(r), _) => r.name.clone(),
        (None, 0) => t!("header.ms_none").to_string(),
        (None, _) => "?".to_string(),
    };

    // Quick-select buttons. The SLOTS come from this terminal (`Backend::strat_slots`), which falls
    // back to the core's own `manual_strats_names` until a button is assigned here; the core's
    // config still decides which slots are ON SCREEN (`use_buttons` / `show_button[i]`), plus any
    // slot this terminal has assigned — that one must be reachable even if the core never showed
    // it, or an assignment could not be undone.
    let slots = b.strat_slots(core);
    let buttons: Option<Vec<StratButtonSlot>> = (on && slots.is_some()).then(|| {
        let slots = slots.as_deref().unwrap_or_default();
        (0..MANUAL_STRAT_SLOTS)
            .filter(|&i| slots.get(i).is_some_and(|slot| slot.show))
            .map(|i| {
                let slot = slots.get(i).cloned().unwrap_or_default();
                let strategy = slot.strategy.trim().to_string();
                // Match the slot's STRATEGY NAME against this snapshot's Manual-kind strategies
                // rather than its ordinal position: the slot is the core's slot `i` while an
                // ordinal match would address the ix-th Manual-kind strategy in snapshot order —
                // the two can disagree, and a button that fires the wrong strategy places a real
                // order silently.
                let sid = manuals
                    .iter()
                    .find(|(_, name)| *name == strategy)
                    .map(|(sid, _)| *sid);
                let numeric_label = (i + 1).to_string();
                let caption = slot_caption(&slot.label, &strategy, &numeric_label);
                (i, strategy, sid, caption, numeric_label)
            })
            .collect::<Vec<_>>()
    });

    // Resolve the narrow-window clip once, from real measured widths, before building either the
    // button row or the pill text below — the two share one decision.
    let fit = buttons.as_deref().map(|slots| {
        let btn_chrome_w = design::ui_value(cx, BTN_CHROME_W);
        let ms_toggle_w = design::ui_value(cx, MS_TOGGLE_W);
        let pill_chrome_w = design::ui_value(cx, PILL_CHROME_W);
        let header_other_sections_w = design::ui_value(cx, HEADER_OTHER_SECTIONS_W);
        let widths: Vec<SlotWidths> = slots
            .iter()
            .map(|(_, _, _, name_label, numeric_label)| {
                let name_w = design::ui_text_width(
                    cx,
                    name_label,
                    BTN_NAME_TEXT_SIZE,
                    BTN_TEXT_WEIGHT,
                    true,
                );
                let numeric_w = design::ui_text_width(
                    cx,
                    numeric_label,
                    BTN_NAME_TEXT_SIZE,
                    BTN_TEXT_WEIGHT,
                    true,
                );
                SlotWidths {
                    name_only: btn_chrome_w + name_w,
                    number_only: btn_chrome_w + numeric_w,
                }
            })
            .collect();
        let pill_w_full = pill_chrome_w
            + design::ui_text_width(
                cx,
                &design::fit_label(
                    cx,
                    &full_pill_text,
                    design::font_w(cx, design::HEADER_LABEL_MAX_W),
                ),
                10.5,
                500.0,
                true,
            );
        // The parameter summary sits at the END of this cluster and is measured like the buttons
        // are: it is real width the header spends, and leaving it out of the base is what let it
        // run under the readouts to its right instead of making the buttons yield first.
        let summary_w = summary
            .as_deref()
            .map(|text| {
                design::ui_value(cx, design::CHROME_GAP)
                    + design::ui_text_width(cx, text, design::base_text(cx) - 2.0, 400.0, true)
            })
            .unwrap_or(0.0);
        // The gear is permanent chrome in this cluster, so it belongs in the base like the toggle
        // and the pill: budgeting the row without it proves a fit at a width the row then overflows.
        let base = header_other_sections_w
            + ms_toggle_w
            + design::ui_value(cx, design::CHROME_GAP) * 3.0
            + design::ui_value(cx, SLOT_GEAR_W)
            + pill_w_full
            + summary_w;
        // Which rendered slot is the active one, so the ladder measures it the way it draws it.
        let selected_slot = on
            .then(|| {
                slots
                    .iter()
                    .position(|(_, _, sid, _, _)| sid.is_some() && *sid == Some(id))
            })
            .flatten();
        resolve_strat_fit(
            chrome_width,
            design::ui_value(cx, BTN_GAP),
            &widths,
            selected_slot,
            base,
        )
    });

    // Capped like the core selector beside it: a strategy name is arbitrary user text and this
    // pill sizes to its content, so an uncapped one pushes the header's right cluster off-window.
    // The cap narrows once the fit ladder has already dropped every quick-select button.
    let pill_cap = if fit.is_some_and(|f| f.pill_reduced) {
        design::font_w(cx, REDUCED_PILL_MAX_W)
    } else {
        design::font_w(cx, design::HEADER_LABEL_MAX_W)
    };
    let display = design::fit_label(cx, &full_pill_text, pill_cap);
    let dot_color = if on && sel_row.is_some() {
        design::positive_color(p)
    } else if on {
        // Signal an enabled strategy id that did not resolve by using the danger color.
        design::danger_color(p)
    } else {
        p.text_muted
    };

    let mut items = Vec::with_capacity(manuals.len());
    for (sid, name) in &manuals {
        let sid = *sid;
        let backend = backend.clone();
        items.push(
            MoonMenuItem::with_key(format!("ms-{sid}"), name.clone())
                .selected(id == sid)
                .checked(id == sid)
                // Selecting a strategy also enables the mode, matching the Moonbot menu.
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        send_manual(b, core, true, sid);
                        bcx.notify();
                    });
                }),
        );
    }

    let toggle_backend = backend.clone();
    let mut row = h_flex()
        .min_w_0()
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .child(
            MoonToggle::new("header-ms-toggle")
                .label("MS")
                .label_side(MoonToggleLabelSide::Left)
                .checked(on)
                .size(MoonToggleSize::Compact)
                // The mode cannot be enabled until the picker selects a strategy.
                .disabled(id == 0)
                .on_change(move |ch: &bool, _w, app| {
                    let v = *ch;
                    toggle_backend.update(app, |b, bcx| {
                        let (_, cur_id) = b.manual_strat_state(core);
                        if cur_id == 0 {
                            return;
                        }
                        // Disabling preserves the id so the next toggle restores the same strategy.
                        send_manual(b, core, v, cur_id);
                        bcx.notify();
                    });
                }),
        )
        // The gear sits with the switch it belongs to and exists only while MS is on: with the
        // mode off there are no buttons on the row, so settings for them would configure nothing
        // visible.
        .children(on.then(|| {
            let gear_shell = shell.clone();
            let content = slots_open.then(|| {
                settings::slot_settings_content(core, backend, shell, label_inputs, p, cx)
            });
            crate::chrome::terminal_chrome::header_gear_popover(
                "ms-slots",
                MoonPopoverPlacement::BottomStart,
                SLOT_SETTINGS_W,
                slots_open,
                content,
                MoonButton::new("ms-slots-gear")
                    .size(MoonButtonSize::ToolbarCompact)
                    .variant(MoonButtonVariant::Ghost)
                    .icon("icons/settings.svg")
                    .tooltip(t!("header.ms_slots_gear").to_string())
                    .render(),
                move |open, _, app| {
                    gear_shell.update(app, |shell, cx| {
                        shell.set_strat_slots_open(open, cx);
                        cx.notify();
                    });
                },
            )
        }))
        .child({
            MoonPopover::new("header-ms-selector")
                .placement(MoonPopoverPlacement::BottomStart)
                .fit_content()
                .close_on_content_click(true)
                .trigger(
                    MoonSelectorPill::new("header-ms-pill")
                        .height(PILL_H)
                        .radius(PILL_H / 2.0)
                        .leading_dot(dot_color)
                        .segment(
                            MoonSelectorSegment::new(display)
                                .color(if on { p.text } else { p.text_soft })
                                .weight(500.0),
                        )
                        .render(),
                )
                .content(
                    MoonPopupMenu::new("header-ms-menu")
                        .fit_width(200.0, 560.0)
                        .size(MoonMenuSize::Compact)
                        .items(items)
                        .render(),
                )
        });
    // Ten quick-select buttons: the fit ladder resolved above decides the label mode and how many
    // slots to render; `0` renders none, leaving the toggle and the pill as the last two standing.
    //
    // Three gestures per button, matching Moonbot: left click fires the slot, right click assigns
    // which strategy it fires, and a double click renames it.
    if let (Some(slots), Some(fit)) = (buttons, fit)
        && fit.visible_count > 0
    {
        let mut btn_row = h_flex().flex_none().gap(design::ui_px(cx, BTN_GAP));
        for (i, strategy, sid, caption, numeric_label) in slots.into_iter().take(fit.visible_count)
        {
            let selected = on && sid.is_some() && sid == Some(id);
            // The SELECTED slot keeps its caption even at the number-only clip level: the row's
            // job is to say which strategy a new order will use, and a lone digit does not.
            let label = match fit.label_mode {
                LabelMode::NumberOnly if !selected => numeric_label,
                _ => caption.clone(),
            };
            // An explicit width, so the caption sits inside the button's own padding instead of
            // against its border. Measured through the same helper the fit ladder used, so the
            // budget and the rendered button cannot disagree.
            let label_w =
                design::ui_text_width(cx, &label, BTN_NAME_TEXT_SIZE, BTN_TEXT_WEIGHT, true);
            let btn_w = design::ui_value(cx, BTN_CHROME_W) + label_w;

            let mut btn = MoonButton::new(SharedString::from(format!("ms-btn-{i}")))
                .size(MoonButtonSize::ToolbarCompact)
                // The active slot carries the accent variant rather than only `selected`: on a
                // Panel button the selected state is a few percent of background and reads as
                // nothing on a row of ten.
                .variant(if selected {
                    MoonButtonVariant::Blue
                } else {
                    MoonButtonVariant::Panel
                })
                .width(btn_w)
                .selected(selected)
                .segment(MoonButtonSegment::new(label.clone()).weight(BTN_TEXT_WEIGHT));
            if sid.is_none() && !strategy.is_empty() {
                // Assigned to a name this core's snapshot does not have: say so rather than firing
                // something else. Still renameable and re-assignable, hence not disabled.
                btn = btn.tooltip(
                    t!("hotkeys.ms_button_unresolved", name = strategy.as_str()).to_string(),
                );
            } else if sid.is_none() {
                btn = btn.tooltip(t!("header.ms_slot_empty").to_string());
            }
            let click_backend = backend.clone();
            let btn = btn.on_click(move |_, _, app| {
                let Some(sid) = sid else { return };
                click_backend.update(app, |b, bcx| {
                    send_manual(b, core, true, sid);
                    bcx.notify();
                });
            });

            // Right click opens the assign menu for THIS slot. `MoonPopover` opens from a click on
            // its trigger, so the open state is driven from `Shell` instead and the wrapper only
            // has to catch the right button.
            let menu_shell = shell.clone();
            let close_shell = shell.clone();
            let clear_backend = backend.clone();
            let clear_shell = shell.clone();
            let mut menu_items = Vec::with_capacity(manuals.len() + 1);
            menu_items.push(
                MoonMenuItem::with_key(
                    format!("ms-slot-{i}-clear"),
                    t!("header.ms_slot_clear").to_string(),
                )
                .selected(strategy.is_empty())
                .on_click(move |_, _, app| {
                    clear_backend.update(app, |b, bcx| {
                        b.set_strat_slot_strategy(core, i, String::new());
                        bcx.notify();
                    });
                    clear_shell.update(app, |shell, cx| {
                        shell.close_strat_slot_menu();
                        cx.notify();
                    });
                }),
            );
            for (_, name) in &manuals {
                let assigned = *name == strategy;
                let assign_name = name.clone();
                let assign_backend = backend.clone();
                let assign_shell = shell.clone();
                menu_items.push(
                    MoonMenuItem::with_key(format!("ms-slot-{i}-{name}"), name.clone())
                        .selected(assigned)
                        .checked(assigned)
                        .on_click(move |_, _, app| {
                            let assign_name = assign_name.clone();
                            assign_backend.update(app, |b, bcx| {
                                b.set_strat_slot_strategy(core, i, assign_name);
                                bcx.notify();
                            });
                            assign_shell.update(app, |shell, cx| {
                                shell.close_strat_slot_menu();
                                cx.notify();
                            });
                        }),
                );
            }
            // The menu hangs off a ZERO-SIZE anchor beside the button rather than wrapping it.
            // `MoonPopover` toggles itself on a left mouse-down on its trigger, so making the
            // button the trigger would open this menu on the very click that fires the strategy —
            // and swallow that click's mouse-down. An anchor nothing can click leaves the open
            // state entirely to the right-click handler below.
            btn_row = btn_row.child(
                div()
                    .id(SharedString::from(format!("ms-btn-wrap-{i}")))
                    .flex_none()
                    .relative()
                    .on_mouse_down(MouseButton::Right, move |_, _, app| {
                        menu_shell.update(app, |shell, cx| {
                            shell.open_strat_slot_menu(core, i);
                            cx.notify();
                        });
                    })
                    .child(btn.render())
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .bottom_0()
                            .w(px(0.0))
                            .h(px(0.0))
                            .child(
                                MoonPopover::new(SharedString::from(format!("ms-slot-menu-{i}")))
                                    .placement(MoonPopoverPlacement::BottomStart)
                                    .fit_content()
                                    .close_on_content_click(true)
                                    .open(slot_menu == Some((core, i)))
                                    .on_open_change(move |open, _, app| {
                                        if !open {
                                            close_shell.update(app, |shell, cx| {
                                                shell.close_strat_slot_menu();
                                                cx.notify();
                                            });
                                        }
                                    })
                                    .trigger(div().w(px(0.0)).h(px(0.0)))
                                    .content(
                                        MoonPopupMenu::new(SharedString::from(format!(
                                            "ms-slot-list-{i}"
                                        )))
                                        .fit_width(160.0, 420.0)
                                        .size(MoonMenuSize::Compact)
                                        .items(menu_items)
                                        .render(),
                                    ),
                            ),
                    ),
            );
        }
        row = row.child(btn_row);
    }
    if let Some(summary) = summary {
        row = row.child(
            div()
                // The longest thing in the left half of the header; truncating it here keeps a
                // long strategy summary from pushing the right-hand readouts off a narrow window.
                .min_w_0()
                .truncate()
                .text_size(design::t_caption(cx))
                .font_family(design::mono())
                .text_color(rgb(p.text_soft))
                .child(summary),
        );
    }
    Some(row.into_any_element())
}

/// Resolve what one quick-select button says, in the order a trader expects to see it.
///
/// The trader's own caption wins; failing that the assigned strategy names the button, as it does
/// in Moonbot; failing that the slot shows its own number, which is what makes an unassigned slot
/// visible enough to right-click.
///
/// Args:
///     label: Caption stored for this slot, possibly blank.
///     strategy: Strategy name assigned to this slot, possibly blank.
///     ordinal: The slot's 1-based number, used when it has neither.
///
/// Returns:
///     The caption to render.
fn slot_caption(label: &str, strategy: &str, ordinal: &str) -> String {
    match (label.trim(), strategy.trim()) {
        ("", "") => ordinal.to_string(),
        ("", name) => name.to_string(),
        (label, _) => label.to_string(),
    }
}

/// Collect Manual-kind strategy ids and names in their snapshot order.
///
/// Args:
///     strategies: Current strategy snapshot for the active trade core.
///
/// Returns:
///     Ordered picker options, or `None` when the snapshot has no Manual-kind strategies.
fn manual_strategy_options(strategies: &[StrategyRow]) -> Option<Vec<(u64, String)>> {
    let options: Vec<_> = strategies
        .iter()
        .filter(|strategy| strategy.kind_ordinal == MANUAL_STRATEGY_KIND)
        .map(|strategy| (strategy.id, strategy.name.clone()))
        .collect();
    (!options.is_empty()).then_some(options)
}

#[cfg(test)]
mod tests;

/// Fire quick-select SLOT `ix` and enable manual-strategy mode.
///
/// This performs the same update as clicking button `ix` in the header, and deliberately addresses
/// the same thing that button does: the slot. Resolving `ix` as "the ix-th Manual-kind strategy in
/// snapshot order" instead — as this did before slots were assignable — makes the hotkey and the
/// button with the same number fire DIFFERENT strategies the moment a trader assigns a slot by
/// hand, which places a real order on the wrong strategy with no visible cause.
///
/// Args:
///     b: Backend used to read the slot, resolve the strategy, and send the settings edit.
///     core: Core whose manual strategy should be selected.
///     ix: Zero-based quick-select slot.
///
/// Returns:
///     `true` when that slot names a strategy this core actually has; `false` to let the hotkey
///     propagate otherwise.
pub(crate) fn select_manual_strategy(b: &mut Backend, core: CoreId, ix: usize) -> bool {
    let slot_strategy = b
        .strat_slots(core)
        .and_then(|slots| slots.get(ix).map(|slot| slot.strategy.trim().to_string()))
        .filter(|strategy| !strategy.is_empty());
    let sid = match slot_strategy {
        Some(strategy) => b.session.store().core(core).and_then(|cd| {
            cd.strategies
                .iter()
                .find(|s| s.kind_ordinal == MANUAL_STRATEGY_KIND && s.name == strategy)
                .map(|s| s.id)
        }),
        // No slot to go by — the core has assigned none and this terminal owns none either, which
        // is every core that has not reported its config yet. Fall back to the ordinal reading
        // this used before slots existed, so the hotkeys keep working on an unconfigured core;
        // once slots exist they are authoritative, and an empty slot fires nothing on purpose.
        None if !b.core_owns_strat_trade_slots(core) => {
            b.session.store().core(core).and_then(|cd| {
                cd.strategies
                    .iter()
                    .filter(|s| s.kind_ordinal == MANUAL_STRATEGY_KIND)
                    .nth(ix)
                    .map(|s| s.id)
            })
        }
        None => None,
    };
    match sid {
        Some(sid) => {
            send_manual(b, core, true, sid);
            true
        }
        None => false,
    }
}

/// Store the process-lifetime local override and send a manual-strategy edit to the core.
///
/// The override remains authoritative until replaced or process exit; neither a core echo nor a
/// command failure reconciles it. Send failures are logged.
///
/// Args:
///     b: Backend whose local state and session are updated.
///     core: Target core.
///     on: Whether manual-strategy mode should be enabled.
///     id: Selected strategy id, retained even when the mode is disabled.
fn send_manual(b: &mut Backend, core: CoreId, on: bool, id: u64) {
    let changed = b.manual_strat_state(core).1 != id;
    b.set_manual_strat_local(core, on, id);
    if changed {
        // Selecting a strategy SEEDS the visible take profit and stop from it, once. From then on
        // the screen is authoritative — the order carries what the trader sees, not what the
        // strategy holds — but starting from the strategy's own values is what makes the first
        // order after a selection do what the strategy says it will.
        b.seed_exit_from_strategy(core, id);
    }
    if let Err(e) = b
        .session
        .edit_client_settings(core, ClientSettingsEdit::ManualStrategy { on, id })
    {
        log::warn!("manual strategy edit failed: {e:#}");
    }
}

/// Build a Moonbot-style `Buy +0.00% Sell +0.50% SL ON TS OFF` parameter summary.
///
/// Values absent from the strategy snapshot fall back to defaults from its schema kind. Missing
/// Buy or Sell fields fall back to zero; SL and TS are always included.
///
/// Args:
///     row: Selected strategy snapshot.
///     schema: Optional schema providing defaults omitted from the snapshot.
///
/// Returns:
///     The four-part Buy, Sell, SL, and TS summary.
fn strat_summary(row: &StrategyRow, schema: Option<&StrategySchemaModel>) -> String {
    let field = |name: &str| strat_field(row, schema, name);
    let mut parts: Vec<String> = Vec::with_capacity(4);
    // Always show Buy and Sell, as Moonbot does. A default-valued field may be absent from both the
    // snapshot and some schema sections; zero then means the current price, matching the core's
    // `signal price +0%` diagnostic.
    let buy = field("BuyPrice").unwrap_or_else(|| "0".to_string());
    parts.push(format!("Buy {}", fmt_pct(&buy)));
    let sell = field("SellPrice").unwrap_or_else(|| "0".to_string());
    parts.push(format!("Sell {}", fmt_pct(&sell)));
    parts.push(format!("SL {}", on_off(field("UseStopLoss"))));
    parts.push(format!("TS {}", on_off(field("UseTrailing"))));
    parts.join(" · ")
}

/// Resolve a strategy field from the snapshot, then its schema-kind default.
///
/// Args:
///     row: Strategy snapshot whose explicit fields take priority.
///     schema: Optional schema containing defaults by strategy kind.
///     name: Exact field name to resolve.
///
/// Returns:
///     The explicit or default value, or `None` when neither is available.
fn strat_field(
    row: &StrategyRow,
    schema: Option<&StrategySchemaModel>,
    name: &str,
) -> Option<String> {
    if let Some((_, v)) = row.fields.iter().find(|(n, _)| n == name) {
        return Some(v.clone());
    }
    schema?
        .kinds
        .iter()
        .find(|k| k.ordinal == row.kind_ordinal)?
        .sections
        .iter()
        .flat_map(|s| s.fields.iter())
        .find(|f| f.name == name)?
        .default
        .clone()
}

/// Format a numeric percentage field with an explicit non-zero sign (`"0.5"` -> `"+0.50%"`).
///
/// A value rounding to zero prints `"0.00%"`. Non-numeric or non-finite input is returned unchanged.
fn fmt_pct(v: &str) -> String {
    let raw = v.trim();
    match raw.parse::<f64>() {
        Ok(f) => moon_core::util::fmt::signed_pct(f, 2)
            .map(|(text, _)| text)
            .unwrap_or_else(|| v.to_string()),
        Err(_) => v.to_string(),
    }
}

/// Convert a `Yes` or `No` field to `ON` or `OFF`, treating missing or other values as `OFF`.
///
/// Args:
///     v: Optional strategy field value.
///
/// Returns:
///     `ON` only for case-insensitive `Yes`; otherwise `OFF`.
fn on_off(v: Option<String>) -> &'static str {
    match v {
        Some(s) if s.trim().eq_ignore_ascii_case("yes") => "ON",
        _ => "OFF",
    }
}
