//! Settings popup for the header's manual-strategy quick-select buttons.
//!
//! One gear beside the MS cluster, in the same shape as every other gear popup in the chrome
//! (`chrome::terminal_chrome::header_gear_popover`). It owns everything about the ten buttons that
//! is not a single click: which slots are drawn, which strategy each one fires, which MoonHook that
//! strategy defers its exits to, and the two rules that decide whether the toolbar's TP and SL
//! reach a manual order at all.
//!
//! Which slots are drawn and what they fire is the terminal's own per-core state
//! (`ServerConfig::strat_slots`) — see `Backend::strat_slots`. `Pull from the core` replaces them
//! with Moonbot's own arrangement, and is the way back after any local edit. A button always says
//! what strategy it fires; there is no separate caption, because a button that can be named
//! something other than what it does is a button that places a real order on something other than
//! what it says.
//!
//! The hook column is the one control here that writes to the CORE (`Backend::set_strategy_hook`):
//! it is the strategy's own `UseHookStrategy` field, the same one the Strategies panel edits, put
//! beside the button it changes the meaning of.

use gpui::*;
use moon_core::config::MANUAL_STRAT_SLOTS;
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonDropdown, MoonMenuItem, MoonMenuSize, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use crate::backend::manual_strategy_id;
use crate::shell::Shell;
use crate::{Backend, design};

/// Width of the leading show-box column, holding the checkbox and its slot number.
///
/// FIXED, because the number is the label: `10` is wider than `9`, and a column sized to its
/// content puts the last row's pickers a few pixels right of the other nine.
const SLOT_COL_W: f32 = 42.0;
/// Trigger width of the strategy picker, which names the strategy a slot fires.
///
/// Sized against the popup's own content box rather than by eye: the row spends roughly four fifths
/// of it, and the rest goes here, because a truncated strategy name beside empty space is the one
/// thing this column can get wrong.
const STRATEGY_COL_W: f32 = 164.0;
/// Trigger width of the hook picker beside it.
const HOOK_COL_W: f32 = 130.0;
/// Width of the hotkey column, showing the terminal binding this slot answers to.
///
/// Wide enough for the longest binding it can print, `CTRL-ALT-1`: a column that truncates the
/// thing it exists to state is worse than one row narrower elsewhere.
const HOTKEY_COL_W: f32 = 78.0;
/// Dropped-down menu width for both pickers, wide enough for a name the trigger truncates.
const MENU_W: f32 = 260.0;
/// Menu height cap, so ten strategies do not push the popup past the window.
const MENU_MAX_H: f32 = 220.0;

/// Build the popup's content: ten slot rows, the two exit rules, and the two actions.
///
/// Args:
///     core: Core whose slots are being edited — the group's active trade core.
///     group: Window group this cluster belongs to, which authorizes revealing that core.
///     backend: Application state read for slots and written by every control here.
///     shell: Shell that owns the popup's open state.
///     p: Active palette.
///     cx: Application context.
///
/// Returns:
///     The popup body.
pub(super) fn slot_settings_content(
    core: CoreId,
    group: &str,
    backend: &Entity<Backend>,
    shell: &Entity<Shell>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let b = backend.read(cx);
    let slots = b.strat_slots(core).unwrap_or_default();
    // The TERMINAL's own bindings (Settings > Hotkeys > manual strategy), shown read-only: this
    // popup is where a trader looks to see which key fires which button. Changing one is a
    // different question, and the button below leads to where that is done.
    let hotkeys = b.config.hotkeys.manual_strategy.clone();
    let ignore_sell = b.ignore_strat_sell_price(core);
    let mb_logic = b.ms_mb_logic(core);
    // Pulling reads the core's OWN configuration, so it is available exactly while that
    // configuration is. `ignore_strat_sell_price` answering at all is that proof — it is projected
    // from the same snapshot — whereas local slots prove only that this terminal has its own.
    let core_known = ignore_sell.is_some();
    // Borrowed, never cloned: this popup re-renders on every repaint while it is open, and the
    // snapshot it needs is a few hundred rows each carrying its own field vector.
    let strategies = b
        .session
        .store()
        .core(core)
        .map(|data| data.strategies.as_slice())
        .unwrap_or_default();
    let manuals = super::manual_strategy_options(strategies).unwrap_or_default();
    // One list for all ten rows: it does not vary by slot, and building it per row clones every
    // strategy name ten times on each repaint the popup is open for.
    let slot_names = super::slot_strategy_names(&manuals);
    let hooks = b.hook_strategy_names(core);
    let gap = design::ui_px(cx, 6.0);

    let mut rows = v_flex().gap(gap).w_full();
    for slot in 0..MANUAL_STRAT_SLOTS {
        let current = slots.get(slot).cloned().unwrap_or_default();
        let assigned = current.strategy.trim().to_string();
        // The id this slot actually fires, through the shared resolver the button and the hotkey
        // use — a name the core no longer has resolves to nothing, and its hook picker is then
        // disabled rather than pointed at some other strategy.
        let sid = manual_strategy_id(strategies, &assigned);
        // The hook INCLUDING an edit of ours the core has not echoed yet, so the picker keeps
        // showing what was just chosen instead of snapping back for the length of a round trip.
        let hook_now = sid
            .map(|sid| b.strategy_hook_shown(core, sid))
            .unwrap_or_default();
        // Offered only where the write can actually land: the field has to be in this strategy
        // kind's schema, and the schema has to have arrived at all — moonproto refuses a whole
        // edit batch before it does, so an enabled picker would send clicks into nothing.
        let hook_writable = sid.is_some_and(|sid| {
            b.strategy_has_field(core, sid, crate::backend::FIELD_USE_HOOK_STRATEGY)
        });
        // The hook as a STRATEGY, for the button that opens it. Resolved from the SHOWN name, so a
        // hook picked a moment ago is reachable at once — the hook's own row is in the snapshot
        // whether or not the core has echoed the strategy that now points at it.
        let hook_sid = b.hook_strategy_id(core, &hook_now);
        let show_backend = backend.clone();
        rows = rows.child(
            h_flex()
                .w_full()
                .items_center()
                .gap(gap)
                .child(
                    div()
                        .w(px(design::font_w(cx, SLOT_COL_W)))
                        .flex_none()
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
                        ),
                )
                // Each field followed by the button that opens WHAT IT NAMES, and a rule between
                // the two pairs: the left one is the slot's own manual strategy, the right one the
                // hook that strategy defers to. Position is what tells the two buttons apart, so
                // neither needs a colour of its own.
                .child(strategy_picker(core, slot, &assigned, &slot_names, backend))
                .child(goto_button(
                    SharedString::from(format!("ms-slot-goto-{slot}")),
                    t!("coin_menu.strategy_goto").to_string(),
                    core,
                    group,
                    sid,
                    backend,
                    shell,
                    cx,
                ))
                .child(design::chrome_divider(cx, p))
                .child(hook_picker(
                    core,
                    slot,
                    sid.filter(|_| hook_writable),
                    &hook_now,
                    &hooks,
                    backend,
                ))
                // The hook is a strategy of its own, with its own parameters to edit — and the one
                // whose stop a hooked order actually carries. Reaching it through the manual
                // strategy first would be two hops to the numbers that apply.
                .child(goto_button(
                    SharedString::from(format!("ms-slot-hook-goto-{slot}")),
                    t!("header.ms_hook_goto_tip").to_string(),
                    core,
                    group,
                    hook_sid,
                    backend,
                    shell,
                    cx,
                ))
                .child(
                    div()
                        .w(px(design::font_w(cx, HOTKEY_COL_W)))
                        .flex_none()
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
    let logic_backend = backend.clone();
    let hotkeys_backend = backend.clone();
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
        // Moonbot's own stop rule, and the default. Local state, so it is offered whether or not
        // the core has reported anything.
        .child(
            v_flex()
                .gap(design::ui_px(cx, 2.0))
                .child(
                    MoonCheckbox::new("ms-mb-logic")
                        .label(t!("header.ms_mb_logic").to_string())
                        .checked(mb_logic)
                        .size(MoonCheckboxSize::Compact)
                        .on_change(move |checked: &bool, _w, app| {
                            let on = *checked;
                            logic_backend.update(app, |b, cx| {
                                b.set_ms_mb_logic(core, on);
                                cx.notify();
                            });
                        }),
                )
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(t!("header.ms_mb_logic_hint").to_string()),
                ),
        )
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
            h_flex()
                .w_full()
                .justify_between()
                .child(
                    MoonButton::new("ms-slots-hotkeys")
                        .size(MoonButtonSize::ToolbarCompact)
                        .variant(MoonButtonVariant::Ghost)
                        .label(t!("header.ms_slots_hotkeys").to_string())
                        .tooltip(t!("header.ms_slots_hotkeys_tip").to_string())
                        .on_click(move |_, window, app| {
                            // Owner and display come from the clicking window, as the toolbar's
                            // own launcher does: without them the window is placed on the primary
                            // display, which on a multi-monitor desk is not where the trader is.
                            crate::settings::open_on_tab(
                                hotkeys_backend.clone(),
                                Some(window.window_handle()),
                                window.display(app).map(|display| display.id()),
                                crate::settings::Tab::Hotkeys,
                                app,
                            );
                        })
                        .render(),
                )
                .child(
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
                            // The popup renders straight from the slots, so a pull needs no
                            // re-seed — only a repaint of the window that owns it.
                            if pulled {
                                pull_shell.update(app, |_, cx| cx.notify());
                            }
                        })
                        .render(),
                ),
        )
        .into_any_element()
}

/// Open the Strategies window on one strategy — this slot's, or the hook it defers to.
///
/// The same reveal the coin menu and the tuner use (`strategies::open_goto`), carrying the same
/// robot icon the Analytics tuner's row button does, and placed directly after the field naming
/// what it opens. Disabled when there is nothing to reveal: an unassigned slot, a name this core
/// does not have, or no hook set.
///
/// Args:
///     id: Element id, unique per slot and per target.
///     tooltip: What this button reveals, already localized.
///     core: Core owning the strategy.
///     group: Window group this popup belongs to, which authorizes revealing that core.
///     target: Strategy to reveal, or `None` to render the button disabled.
///     backend: Application state the reveal request is parked on.
///     shell: Shell owning the popup's open state, closed as the reveal goes out.
///     cx: Application context, for the font-scaled square size.
///
/// Returns:
///     The button element.
#[allow(clippy::too_many_arguments)]
fn goto_button(
    id: SharedString,
    tooltip: String,
    core: CoreId,
    group: &str,
    target: Option<u64>,
    backend: &Entity<Backend>,
    shell: &Entity<Shell>,
    cx: &App,
) -> AnyElement {
    let goto_backend = backend.clone();
    let goto_shell = shell.clone();
    // THIS window's group, exactly as the Orders panel passes its own: the authority belongs to the
    // surface the click came from. Reading the process-global Auto-focus slot instead would hand
    // over whichever group last held focus, and `open_goto` would then silently refuse a core that
    // group does not name — a dead button with no window and no message. The contract test in
    // `strategies/window/tests.rs` fails on that identifier appearing in this file at all.
    let workspace_group = group.to_string();
    // The Analytics tuner's own "open this strategy" button, repeated: same robot icon, same Micro
    // square, same Soft variant. One gesture should not look like two different things in two
    // windows.
    MoonButton::new(id)
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Soft)
        .width(design::micro_control_h_value(cx))
        // No explicit icon colour: the button's own foreground already answers to the variant and
        // to the disabled state, and an explicit one would override the dimming a disabled robot
        // needs. Which strategy each button opens is said by where it sits, not by a colour.
        .leading_icon(MoonButtonIconSlot::new("icons/bot.svg"))
        .disabled(target.is_none())
        .tooltip(tooltip)
        .on_click(move |_, window, app| {
            let Some(sid) = target else { return };
            // The popup goes with the click, as the coin menu closes itself before revealing:
            // leaving it open would park it over the header while the focus moves to another
            // window, where nothing the trader does can dismiss it.
            goto_shell.update(app, |shell, cx| {
                shell.set_strat_slots_open(false);
                cx.notify();
            });
            // Owner and display come from the clicking window, like every other launcher in this
            // popup: without them the window lands on the primary display rather than this one.
            let owner_display = window.display(app).map(|display| display.id());
            crate::strategies::open_goto(
                goto_backend.clone(),
                core,
                sid,
                Some(workspace_group.clone()),
                Some(window.window_handle()),
                owner_display,
                app,
            );
        })
        .render()
        .into_any_element()
}

/// The strategy this slot fires, as a picker over the core's Manual-kind strategies.
///
/// The same assignment the button's own right-click menu makes, and through the same setter — this
/// is where a trader who has not discovered that gesture finds it, with every slot visible at once.
///
/// Args:
///     core: Core whose slot is being assigned.
///     slot: Zero-based quick-select slot.
///     assigned: Strategy name currently stored for the slot; empty for none.
///     names: Distinct Manual-strategy names this slot can be assigned, in snapshot order.
///     backend: Application state the selection is written to.
///
/// Returns:
///     The picker element.
fn strategy_picker(
    core: CoreId,
    slot: usize,
    assigned: &str,
    names: &[String],
    backend: &Entity<Backend>,
) -> AnyElement {
    let none_label = t!("header.ms_slot_unassigned").to_string();
    let mut items = Vec::with_capacity(names.len() + 1);
    let clear_backend = backend.clone();
    items.push(
        MoonMenuItem::with_key(format!("ms-slot-{slot}-strat-none"), none_label.clone())
            .selected(assigned.is_empty())
            .checked(assigned.is_empty())
            .on_click(move |_, _, app| {
                clear_backend.update(app, |b, cx| {
                    b.set_strat_slot_strategy(core, slot, String::new());
                    cx.notify();
                });
            }),
    );
    // Keyed by POSITION: the key identifies the row to the menu, and a name is arbitrary user text
    // that two strategies can share.
    for (ix, name) in names.iter().enumerate() {
        let selected = name == assigned;
        let value = name.clone();
        let pick_backend = backend.clone();
        items.push(
            MoonMenuItem::with_key(format!("ms-slot-{slot}-strat-{ix}"), name.clone())
                .selected(selected)
                .checked(selected)
                .on_click(move |_, _, app| {
                    let value = value.clone();
                    pick_backend.update(app, |b, cx| {
                        b.set_strat_slot_strategy(core, slot, value);
                        cx.notify();
                    });
                }),
        );
    }
    MoonDropdown::new(SharedString::from(format!("ms-slot-strat-{slot}")))
        .label(if assigned.is_empty() {
            none_label
        } else {
            assigned.to_string()
        })
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::ToolbarCompact)
        .trigger_width_scaled(STRATEGY_COL_W)
        .menu_width_scaled(MENU_W)
        .menu_size(MoonMenuSize::Compact)
        .menu_max_height_ui(MENU_MAX_H)
        .items(items)
        .into_any_element()
}

/// The MoonHook strategy this slot's strategy defers its exits to.
///
/// Writes to the CORE, unlike everything else in this row: it is the strategy's own
/// `UseHookStrategy`. Disabled for a slot with no strategy this core can name — there would be no
/// strategy to write it to.
///
/// Args:
///     core: Core the field edit is sent to.
///     slot: Zero-based quick-select slot, for the control id.
///     sid: Strategy this slot fires, or `None` when it names none this core has or the field
///          cannot be written to it — both leave the picker disabled.
///     current: Hook that strategy holds right now, unconfirmed edits included; empty for none.
///     hooks: MoonHook strategy names this core offers.
///     backend: Application state that sends the field edit.
///
/// Returns:
///     The picker element.
fn hook_picker(
    core: CoreId,
    slot: usize,
    sid: Option<u64>,
    current: &str,
    hooks: &[String],
    backend: &Entity<Backend>,
) -> AnyElement {
    let mut items = Vec::with_capacity(hooks.len() + 1);
    // The empty first item is the picklist moonproto itself builds for this field: clearing the
    // hook is a value, not the absence of one.
    for (ix, name) in std::iter::once(&String::new())
        .chain(hooks.iter())
        .enumerate()
    {
        let selected = name.as_str() == current;
        let value = name.clone();
        let label = if name.is_empty() {
            t!("header.ms_hook_none").to_string()
        } else {
            name.clone()
        };
        let hook_backend = backend.clone();
        items.push(
            MoonMenuItem::with_key(format!("ms-slot-{slot}-hook-{ix}"), label)
                .selected(selected)
                .checked(selected)
                .on_click(move |_, _, app| {
                    let Some(sid) = sid else { return };
                    let value = value.clone();
                    hook_backend.update(app, |b, cx| {
                        b.set_strategy_hook(core, sid, value);
                        cx.notify();
                    });
                }),
        );
    }
    MoonDropdown::new(SharedString::from(format!("ms-slot-hook-{slot}")))
        .label(if current.is_empty() {
            t!("header.ms_hook_none").to_string()
        } else {
            current.to_string()
        })
        .trigger_caret(true)
        // Amber wherever a hook is set, matching the quick-select button's own mark. The two answer
        // different questions for the length of a core round trip — this control shows what was
        // just chosen, the button shows what the core is applying — and that is deliberate: a
        // picker that ignored the click until the echo lands reads as a control that does nothing.
        .trigger_variant(if current.is_empty() {
            MoonButtonVariant::Soft
        } else {
            MoonButtonVariant::Amber
        })
        .trigger_size(MoonButtonSize::ToolbarCompact)
        .trigger_width_scaled(HOOK_COL_W)
        .menu_width_scaled(MENU_W)
        .menu_size(MoonMenuSize::Compact)
        .menu_max_height_ui(MENU_MAX_H)
        .disabled(sid.is_none())
        .items(items)
        .into_any_element()
}
