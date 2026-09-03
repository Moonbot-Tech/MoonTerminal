//! Header toggle and picker for Moonbot manual strategies.
//!
//! The mode is TERMINAL state, stored per core in `ServerConfig::manual_strategy` and never sent:
//! an order names its strategy explicitly, so the core's own `use_manual_strategy` switch stays
//! where its user left it and two terminals on one core can work on different strategies. A core
//! that has never been set here is seeded once from its own snapshot
//! (`Backend::tick_manual_strat_seed`), which is what carries an upgrade over.
//!
//! With a strategy selected, the core derives the sell from that strategy unless its "ignore the
//! strategy's sell price" checkbox is on, so the toolbar's TP and S slots do not apply to new
//! orders and are disabled. The stop follows the core's own "Moonbot logic" switch
//! (`ManualStratState::mb_logic`, on by default): with it on the strategy owns the stop and the
//! toolbar only reports it; with it off the visible stop is written to the order after placement.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonMenuItem, MoonMenuSize,
    MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu, MoonSelectorPill,
    MoonSelectorSegment, MoonToggle, MoonToggleLabelSide, MoonToggleSize, h_flex,
};

use moon_core::config::MANUAL_STRAT_SLOTS;
use moon_core::feed::{StrategyRow, StrategySchemaModel};
use moon_core::session::CoreId;

use crate::backend::{MANUAL_STRATEGY_KIND, manual_strategy_id};
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
///
const BTN_CHROME_W: f32 = 20.0;
/// Padding between a quick-select button and the dashed hook frame around it, at design-reference
/// scale. Non-zero so the frame reads as a mark ON the button rather than as a second border
/// welded to its own.
///
/// Deliberately NOT folded into [`BTN_CHROME_W`]: that one is the BUTTON's own chrome and is also
/// what `.width()` hands the button, so widening it would fatten every button instead of the frame
/// around it. The frame is the wrapper's, and only the fit ladder adds it.
///
/// ONE, not two, and the height is what fixes it: a `ToolbarCompact` button is 26 + font delta tall
/// inside a 32 + font delta header strip, so the frame has exactly 6px to spend on both sides
/// together — `2 * (1 pad + 2 border)`. At two the buttons would overhang the strip they sit in.
const HOOK_FRAME_PAD: f32 = 1.0;
/// Border width of that frame, in raw pixels — `border_2()` is not font-scaled, so the ladder must
/// not scale it either.
///
/// TWO, not one: the dash pattern is a fixed multiple of the border width, so a 1px dashed border
/// is a 2px dash against a 1px gap and reads as solid at a glance — which would leave the mark
/// saying the same thing as an ordinary border.
const HOOK_FRAME_BORDER: f32 = 2.0;
/// Content width of the quick-select settings popup, in font-scaled pixels.
///
/// Sized for the widest row it holds: the show box, the strategy picker, the hook picker, the two
/// reveal buttons and the hotkey column — about 484 of it at design-reference scale, leaving the
/// same headroom the row had before those columns joined it.
const SLOT_SETTINGS_W: f32 = 540.0;
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
/// Conservative reservation for the rest of the header (brand, the workspace-mode dropdown, core
/// selector and its gear, balance, the strategy-parameter summary, the trailing spacer's minimum,
/// and the ticker/quiet/clock/window-control cluster once visible) — sections this cluster does
/// not own and cannot measure without touching them, the same reasoning `design::ticker_visible`
/// uses for its own flat threshold rather than a live remainder. Needs an on-screen check. At
/// design-reference scale like every other estimate below.
///
/// The workspace section grew when its compact toggle became a mode-naming dropdown
/// (`chrome/terminal_chrome.rs::workspace_mode_selector`, fixed trigger); this reservation moved
/// with it, because a budget that under-reserves lets THIS cluster claim room the header has
/// already spent.
const HEADER_OTHER_SECTIONS_W: f32 = 778.0;

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
/// right click assigns which strategy it fires. Everything else lives in the gear popup beside them
/// (`settings::slot_settings_content`): per-slot visibility, which strategy each slot fires, that
/// strategy's hook, the two exit rules, and a link into the Strategies window.
#[allow(clippy::too_many_arguments)]
pub fn manual_strategy_controls(
    group: &str,
    backend: &Entity<Backend>,
    shell: &Entity<Shell>,
    slot_menu: Option<(CoreId, usize)>,
    slots_open: bool,
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

    // The pill shows the selected strategy, `?` when one is selected but does not resolve, and the
    // localized none marker when nothing is selected at all. A name that resolves to nothing is NOT
    // an empty selection — the strategy was renamed, deleted, or has not arrived, and an order in
    // that state is refused rather than placed, so the two must not look alike. The untruncated
    // candidate is resolved first because the fit ladder below decides which cap it renders at.
    let full_pill_text: String = match sel_row {
        Some(r) => r.name.clone(),
        None if b.selected_manual_strategy_name(core).is_some() => "?".to_string(),
        None => t!("header.ms_none").to_string(),
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
                // order silently. Through the shared resolver, so the button and the hotkey for the
                // same slot cannot reach different strategies.
                let sid = manual_strategy_id(&core_data.strategies, &strategy);
                let numeric_label = (i + 1).to_string();
                // The button IS its strategy; an unassigned slot falls back to its own number so
                // the empty slot stays clickable and identifiable.
                let caption = if strategy.is_empty() {
                    numeric_label.clone()
                } else {
                    strategy.clone()
                };
                (i, strategy, sid, caption, numeric_label)
            })
            .collect::<Vec<_>>()
    });

    // Resolve the narrow-window clip once, from real measured widths, before building either the
    // button row or the pill text below — the two share one decision.
    let fit = buttons.as_deref().map(|slots| {
        let btn_chrome_w = design::ui_value(cx, BTN_CHROME_W);
        // The dashed hook frame is real width every slot spends, hooked or not — the row draws it
        // on all ten so its geometry cannot depend on which strategies carry a hook. Budgeting the
        // button alone proves a fit the row then overflows by six pixels a button.
        let frame_w = (design::ui_value(cx, HOOK_FRAME_PAD) + HOOK_FRAME_BORDER) * 2.0;
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
                    name_only: btn_chrome_w + frame_w + name_w,
                    number_only: btn_chrome_w + frame_w + numeric_w,
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
    // Only ASSIGNED slots replace the picker. A row of empty slots chooses nothing — clicking one
    // is a no-op — so hiding the dropdown behind it would leave no way to pick a strategy at all.
    // The picker also stays whenever the selection resolves to nothing, because its `?` and its
    // danger dot are the only thing on screen saying why every order is being refused.
    let assigned_shown = match (buttons.as_deref(), fit) {
        (Some(slots), Some(fit)) => slots
            .iter()
            .take(fit.visible_count)
            .any(|(_, _, sid, _, _)| sid.is_some()),
        _ => false,
    };
    // The selected strategy has to be visible SOMEWHERE. A button carries it only if one of the
    // rendered slots fires it; otherwise the picker is the only thing naming the selection, and
    // hiding it would leave the trader with no highlighted button, no name, and no way to pick a
    // strategy that is not in a slot.
    let selection_on_a_button = match (buttons.as_deref(), fit) {
        (Some(slots), Some(fit)) => slots
            .iter()
            .take(fit.visible_count)
            .any(|(_, _, sid, _, _)| sid.is_some() && *sid == Some(id)),
        _ => false,
    };
    let buttons_shown =
        assigned_shown && selection_on_a_button && b.manual_strat_unresolved(core).is_none();
    let pill_cap = if fit.is_some_and(|f| f.pill_reduced) {
        design::font_w(cx, REDUCED_PILL_MAX_W)
    } else {
        design::font_w(cx, design::HEADER_LABEL_MAX_W)
    };
    let display = design::fit_label(cx, &full_pill_text, pill_cap);
    let dot_color = if on && sel_row.is_some() {
        design::positive_color(p)
    } else if on && b.selected_manual_strategy_name(core).is_some() {
        // A selection that NAMES a strategy but does not resolve: the danger colour, because every
        // order on this core is being refused. The mode being on with nothing selected is not that
        // — it is an ordinary manual order — and painting it red would cry wolf.
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
                        set_manual(b, core, true, sid);
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
                // Never disabled, including with nothing selected: the picker and the gear appear
                // only while the mode is ON, so a switch that refused to turn on without a strategy
                // would hide the one control able to choose one. The mode with no selection places
                // ordinary manual orders — `manual_strat_active` answers `None` — which is exactly
                // what it did before this switch existed.
                .on_change(move |ch: &bool, _w, app| {
                    let v = *ch;
                    toggle_backend.update(app, |b, bcx| {
                        let (_, cur_id) = b.manual_strat_state(core);
                        // Disabling preserves the id so the next toggle restores the same strategy.
                        set_manual(b, core, v, cur_id);
                        bcx.notify();
                    });
                }),
        )
        // The gear sits with the switch it belongs to and exists only while MS is on: with the
        // mode off there are no buttons on the row, so settings for them would configure nothing
        // visible.
        .children(on.then(|| {
            let gear_shell = shell.clone();
            let content = slots_open
                .then(|| settings::slot_settings_content(core, group, backend, shell, p, cx));
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
                        shell.set_strat_slots_open(open);
                        cx.notify();
                    });
                },
            )
        }))
        // The picker exists to choose a strategy, and only the enabled mode uses one: with MS off
        // nothing it selects reaches an order, so it leaves the row entirely rather than sitting
        // there naming a strategy nothing will fire. With the mode on it yields to the quick-select
        // buttons — the same choice, one click away — and returns the moment none are shown.
        .children((on && !buttons_shown).then(|| {
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
        }));
    // Ten quick-select buttons: the fit ladder resolved above decides the label mode and how many
    // slots to render; `0` renders none, leaving the toggle and the pill as the last two standing.
    //
    // Two gestures per button, matching Moonbot: left click fires the slot, right click assigns
    // which strategy it fires.
    if let (Some(slots), Some(fit)) = (buttons, fit)
        && fit.visible_count > 0
    {
        // One list for all ten assign menus, built before the loop: it does not vary by slot, and
        // rebuilding it per button clones every strategy name ten times on every header repaint.
        let slot_names = slot_strategy_names(&manuals);
        let mut btn_row = h_flex().flex_none().gap(design::ui_px(cx, BTN_GAP));
        for (i, strategy, sid, caption, numeric_label) in slots.into_iter().take(fit.visible_count)
        {
            let selected = on && sid.is_some() && sid == Some(id);
            // What this slot's strategy defers its exits to, resolved for the buttons the fit
            // ladder actually draws rather than for all ten: it changes what the button MEANS —
            // both exits then come from that hook — so a drawn button states it, and a clipped one
            // costs nothing to say nothing about. Read off the snapshot already borrowed here,
            // which is the same reader the backend uses.
            let hook = sid
                .and_then(|sid| core_data.strategies.iter().find(|row| row.id == sid))
                .map(|row| crate::backend::hook_of(row, schema))
                .unwrap_or_default();
            let hooked = !hook.is_empty();
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
                // nothing on a row of ten. Fill is therefore the SELECTION and nothing else — the
                // hook is marked by the dashed frame around the button instead, so a mark and a
                // selection can never be mistaken for each other.
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
                // something else. Still re-assignable by right click, hence not disabled.
                btn = btn.tooltip(
                    t!("hotkeys.ms_button_unresolved", name = strategy.as_str()).to_string(),
                );
            } else if sid.is_none() {
                btn = btn.tooltip(t!("header.ms_slot_empty").to_string());
            } else if hooked {
                // The same sentence the parameter summary uses for the same fact, from the same
                // key: two wordings for "these exits come from that hook" would drift apart.
                btn = btn.tooltip(t!("header.ms_summary_hook", name = hook.as_str()).to_string());
            }
            let click_backend = backend.clone();
            let btn = btn.on_click(move |_, _, app| {
                let Some(sid) = sid else { return };
                click_backend.update(app, |b, bcx| {
                    set_manual(b, core, true, sid);
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
            let mut menu_items = Vec::with_capacity(slot_names.len() + 1);
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
            for (ix, name) in slot_names.iter().enumerate() {
                let assigned = *name == strategy;
                let assign_name = name.clone();
                let assign_backend = backend.clone();
                let assign_shell = shell.clone();
                menu_items.push(
                    // Keyed by POSITION: the key identifies the row to the menu, and a name is
                    // arbitrary user text that two strategies can share.
                    MoonMenuItem::with_key(format!("ms-slot-{i}-strat-{ix}"), name.clone())
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
                    // The hook mark: a dashed amber frame around the button, drawn on the wrapper
                    // the right-click menu already needs. `MoonButton` offers no dashed border of
                    // its own — variant and `.outline()` are its only border knobs — so this is
                    // the framework's own `border_dashed`, in the palette's amber and the shared
                    // button radius, rather than a bespoke widget or a hand-mixed colour.
                    //
                    // Drawn on EVERY button, transparent where there is no hook: a frame that only
                    // some buttons carry would make the row's spacing depend on which strategies
                    // are hooked, and the row would visibly re-flow as a hook is set or cleared.
                    .p(px(design::ui_value(cx, HOOK_FRAME_PAD)))
                    // The button's own radius plus the padding between them, so the frame's corners
                    // stay concentric with the ones they enclose instead of cutting inside them.
                    .rounded(design::r_button(cx) + px(design::ui_value(cx, HOOK_FRAME_PAD)))
                    .border_2()
                    .border_dashed()
                    .border_color(if hooked {
                        design::moon(p.amber)
                    } else {
                        gpui::transparent_black()
                    })
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

/// The distinct strategy NAMES a quick-select slot can be assigned.
///
/// A slot stores a name (`StratSlot::strategy`) and fires whatever that name resolves to, first
/// match wins. Two strategies sharing a name are therefore one choice however they are listed:
/// offering both would let the second be picked and the first be assigned, with both rows then
/// drawn as selected. The header's own picker is unaffected — it selects an id and pins it.
fn slot_strategy_names(manuals: &[(u64, String)]) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(manuals.len());
    for (_, name) in manuals {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

/// Collect Manual-kind strategy ids and names in their snapshot order.
///
/// Duplicates are kept: the header's own picker selects by ID and pins it, so two strategies
/// sharing a name are two real, separately selectable choices there. A SLOT is different — it
/// stores the name — and its pickers go through [`slot_strategy_names`] instead.
///
/// Args:
///     strategies: Current strategy snapshot for the active trade core.
///
/// Returns:
///     Ordered picker options, or `None` when the snapshot has no Manual-kind strategies.
fn manual_strategy_options(strategies: &[StrategyRow]) -> Option<Vec<(u64, String)>> {
    let options: Vec<_> = strategies
        .iter()
        // The same two rules `manual_strategy_id` resolves by: a zero id is the sentinel for
        // "nothing selected" everywhere here, and the name is compared trimmed. An untrimmed option
        // is stored untrimmed and then never compares equal to the slot that fires it, so the
        // picker shows no selection for a button that works.
        .filter(|strategy| strategy.kind_ordinal == MANUAL_STRATEGY_KIND && strategy.id != 0)
        .map(|strategy| (strategy.id, strategy.name.trim().to_string()))
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
        Some(strategy) => b
            .session
            .store()
            .core(core)
            .and_then(|cd| manual_strategy_id(&cd.strategies, &strategy)),
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
            set_manual(b, core, true, sid);
            true
        }
        None => false,
    }
}

/// Store this core's manual-strategy mode. Nothing leaves the terminal.
///
/// Args:
///     b: Backend whose per-core configuration is updated.
///     core: Target core.
///     on: Whether manual-strategy mode should be enabled.
///     id: Selected strategy id, retained by name even when the mode is disabled.
fn set_manual(b: &mut Backend, core: CoreId, on: bool, id: u64) {
    // The id actually STORED, which is not always the one clicked: an id the snapshot cannot name
    // keeps the previous selection rather than clearing it, and the exits must be seeded from what
    // the order will really use.
    let id = b.set_manual_strat(core, on, id);
    // Selecting a strategy SEEDS the visible take profit and stop from it, once — the helper keeps
    // the per-strategy overlay it already has. From then on the screen is authoritative (the order
    // carries what the trader sees, not what the strategy holds), but starting from the strategy's
    // own values is what makes the first order after a selection do what the strategy says it will.
    b.seed_exit_from_strategy(core, id);
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
    let field = |name: &str| crate::backend::strat_field_value(row, schema, name);
    // A strategy that hands its exits to a MoonHook has none of its own to show: the core
    // substitutes the hook at order time and takes both the sell price and the stop from it, so
    // printing this strategy's Sell and SL here would name numbers no order will ever use. Say
    // which hook instead — that is the thing whose values apply.
    if let Some(hook) = field(crate::backend::FIELD_USE_HOOK_STRATEGY).map(|v| v.trim().to_string())
        && !hook.is_empty()
    {
        return t!("header.ms_summary_hook", name = hook).to_string();
    }
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
