//! Content for the core-settings popover opened by the gear button beside the header core selector.
//! This is a pure renderer: checkboxes and buttons build their own backend closures, like
//! `controls::metric_popup_content`, while Shell owns persistent input entities for Global TP,
//! Trailing, V-Stop, and the blacklist. On open, `shell/core_settings.rs` seeds them from the active
//! core but preserves the last displayed Trailing and V-Stop values when the snapshot contains zero.
//! Subscriptions in `shell/init.rs` commit inputs on blur or Enter, commit sliders on Change, and
//! keep numeric inputs synchronized with sliders. `shell/core_settings.rs` owns popover state,
//! synchronizes the two blacklist editors, commits when the expanded editor collapses, and handles
//! Cancel All confirmation.
//!
//! Every edit here goes to the core the popup was opened over, resolved through
//! `core_settings::resolve_core_settings_write` at event time. None of these handlers may fall back
//! to `active_trade_core(group)` alone: each commits either a RETAINED editor's value or a value
//! read at RENDER time, and both belong to the core that was active then — which the stacked
//! repaint throttles allow to differ from the core active when the click lands.
//! Default Alert Strategy is the exception by construction: it captures the core rendered into its
//! own row. A missing settings snapshot produces the placeholder state, and the popup stays open —
//! it still belongs to that core.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonInput,
    MoonInputState, MoonPalette, MoonSlider, MoonSliderState, MoonTextArea, h_flex, rgba_from,
    v_flex,
};
use rust_i18n::t;

use moon_core::feed::{ClientSettingsEdit, LevManageEdit, RuntimeState};
use moon_core::session::CoreId;

use crate::panels::{icon_checkbox, popup_group, popup_title};
use crate::shell::core_settings::resolve_core_settings_write;
use crate::{Backend, design};

/// Ordinal of the Alerts strategy kind, matching MoonProto `StrategyKindId::ALERTS = 22`.
const ALERTS_KIND: u8 = 22;

/// Builds the rendered core's Default Alert Strategy row from that core's Alerts strategies. A
/// selection updates `Backend::default_alert_strategy[core]`, which is persisted in server config.
/// Enabling an alert applies this default only when the alert's existing `strategy_id` is zero.
fn def_alert_strategy_row(
    core: Option<CoreId>,
    filter_input: &Entity<MoonInputState>,
    backend: &Entity<Backend>,
    p: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    let core = core?;
    let b = backend.read(cx);
    let cur = b.alert_def_strategy(core);
    let filter = filter_input.read(cx).value().trim().to_lowercase();
    // Filter this core's Alerts strategies by the query. The em dash meaning no strategy always
    // remains first and is never filtered out.
    let mut options: Vec<(u64, String)> = vec![(0u64, "—".to_string())];
    options.extend(
        b.session
            .store()
            .core(core)?
            .strategies
            .iter()
            .filter(|s| s.kind_ordinal == ALERTS_KIND)
            .filter(|s| filter.is_empty() || s.name.to_lowercase().contains(&filter))
            .map(|s| (s.id, s.name.clone())),
    );
    // Render inline instead of using MoonDropdown because a nested menu overlay inside MoonPopover
    // is treated as an outside click and closes the popover before selection. Search and a
    // height-capped scroller keep hundreds of strategies compact, shrink for short lists, and keep
    // clicks inside the content.
    let mut list = v_flex().w_full().gap(design::ui_px(cx, 2.0));
    for (id, name) in options {
        let selected = id == cur;
        let backend2 = backend.clone();
        list = list.child(
            h_flex()
                .id(SharedString::from(format!("def-strat-{id}")))
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .px(design::ui_px(cx, 6.0))
                .py(design::ui_px(cx, 3.0))
                .rounded(design::r_button(cx))
                .cursor_pointer()
                .when(selected, |e| e.bg(rgba_from(p.accent, 0.16)))
                .hover(|e| e.bg(rgba_from(p.text, 0.06)))
                .child(
                    div()
                        .w(design::ui_px(cx, 12.0))
                        .text_color(rgb(p.accent))
                        .child(if selected { "✓" } else { "" }),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(rgb(if selected { p.text } else { p.text_soft }))
                        .child(name),
                )
                .on_click(move |_, _w, app| {
                    backend2.update(app, |bk, bcx| {
                        bk.set_alert_def_strategy(core, id);
                        bcx.notify();
                    });
                }),
        );
    }
    Some(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 4.0))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("core_settings.def_strategy").to_string()),
            )
            .child(
                MoonInput::new("core-def-strategy-filter")
                    .state(filter_input)
                    .small(),
            )
            .child(
                div()
                    .id("core-def-strategy-list")
                    .w_full()
                    .max_h(design::ui_px(cx, 150.0))
                    .overflow_y_scroll()
                    .child(list),
            )
            .into_any_element(),
    )
}

/// Unscaled content width shared with the popover host.
///
/// [`core_settings_content`] applies the font scale to this value; the terminal chrome uses the
/// same scaled width when sizing the surrounding `MoonPopover`.
/// A width of 268 fits the title plus Running and Auto Detect status in EN and RU without clipping;
/// ES relies on truncation.
pub const CONTENT_W: f32 = 268.0;

/// Bounds `(minimum, maximum, step)` for checkbox-slider-input parameters. Global TP maps to
/// positive `g_take_profit`, while Trailing maps to negative `trailing_drop`. Stop Loss lives in
/// the toolbar.
pub const CORE_GTP_BOUNDS: (f32, f32, f32) = (0.5, 10.0, 0.1);
pub const CORE_TRAILING_BOUNDS: (f32, f32, f32) = (-10.0, -0.1, 0.1);
/// V-Stop bounds for negative, whole-percentage BID volume-drop levels in `vol_drop_level`.
pub const CORE_VSTOP_BOUNDS: (f32, f32, f32) = (-50.0, 0.0, 1.0);

/// Builds a `ClientSettings` checkbox for the popup's core. `edit` constructs its boolean variant.
///
/// The box the user clicks was drawn from `checked`, read at RENDER time from the core that was
/// active then, so the toggled value belongs to that core and nowhere else — hence the same
/// [`resolve_core_settings_write`] guard the retained editors use.
fn cs_checkbox(
    id: &str,
    label: String,
    checked: bool,
    backend: &Entity<Backend>,
    group: &str,
    seeded: Option<CoreId>,
    edit: fn(bool) -> ClientSettingsEdit,
) -> impl IntoElement {
    let backend = backend.clone();
    let group = group.to_string();
    MoonCheckbox::new(SharedString::from(id.to_string()))
        .label(label)
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let on = *ch;
            let b = backend.read(app);
            if let Some(core) = resolve_core_settings_write(seeded, b.active_trade_core(&group)) {
                if let Err(e) = b.session.edit_client_settings(core, edit(on)) {
                    log::warn!("core settings edit failed: {e:#}");
                }
            }
        })
}

/// Builds a `LevManage` checkbox for the popup's core. `edit` constructs its boolean variant.
/// Guarded like [`cs_checkbox`], and for the same reason.
fn lev_checkbox(
    id: &str,
    label: String,
    checked: bool,
    backend: &Entity<Backend>,
    group: &str,
    seeded: Option<CoreId>,
    edit: fn(bool) -> LevManageEdit,
) -> impl IntoElement {
    let backend = backend.clone();
    let group = group.to_string();
    MoonCheckbox::new(SharedString::from(id.to_string()))
        .label(label)
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let on = *ch;
            let b = backend.read(app);
            if let Some(core) = resolve_core_settings_write(seeded, b.active_trade_core(&group)) {
                if let Err(e) = b.session.edit_lev_manage(core, edit(on)) {
                    log::warn!("core lev edit failed: {e:#}");
                }
            }
        })
}

/// Builds a checkbox-slider-input parameter with a title above the control row. Shell subscriptions
/// commit slider and input changes, while `checkbox` controls enablement.
#[allow(clippy::too_many_arguments)]
fn param_row(
    title: String,
    checkbox: AnyElement,
    slider: &Entity<MoonSliderState>,
    slider_id: &str,
    input: &Entity<MoonInputState>,
    input_id: &str,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 3.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(title),
        )
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(checkbox)
                .child(
                    div().flex_1().child(
                        MoonSlider::new(slider)
                            .id(slider_id.to_string())
                            .height(18.0),
                    ),
                )
                .child(
                    div().w(design::font_w_px(cx, 56.0)).child(
                        MoonInput::new(SharedString::from(input_id.to_string()))
                            .state(input)
                            .small(),
                    ),
                ),
        )
}

/// Builds core-settings popover content. Shell owns the Global TP, Trailing, V-Stop, blacklist, and
/// strategy-filter inputs. `shell/core_settings.rs` seeds them on open without overwriting retained
/// Trailing or V-Stop values when the core reports zero, and synchronizes the two blacklist editors;
/// collapsing the expanded editor commits its text. Subscriptions in `shell/init.rs` commit numeric
/// and blacklist inputs on blur or Enter, commit sliders on Change, and synchronize slider values
/// into numeric inputs. `cancel_confirm` is the Cancel All Orders confirmation stage. The
/// Shell-provided `on_cancel_all` callback arms confirmation on the first click and sends the command
/// on the confirmed second click.
///
/// Args:
///     gtp_slider: Retained Global TP slider state.
///     trailing_slider: Retained Trailing slider state.
///     vstop_slider: Retained V-Stop slider state.
///     gtp_input: Retained Global TP numeric input.
///     trailing_input: Retained Trailing numeric input.
///     vstop_input: Retained V-Stop numeric input.
///     blacklist_input: Collapsed single-line blacklist editor.
///     blacklist_area: Expanded multiline blacklist editor.
///     def_strategy_input: Default strategy filter input.
///     blacklist_expanded: Whether to render the multiline blacklist editor.
///     cancel_confirm: Whether Cancel All Orders is awaiting confirmation.
///     seeded: Core the retained editors were seeded from; the only core they may write to.
///     backend: Shared terminal backend.
///     group: Active window group whose trade core is edited.
///     p: Active Moon palette.
///     cx: Application context used to read state and render controls.
///     on_cancel_all: Callback for the staged Cancel All Orders action.
///     on_toggle_blacklist: Callback that toggles the blacklist editor mode.
///
/// Returns:
///     The complete core-settings popover content.
#[allow(clippy::too_many_arguments)]
pub fn core_settings_content(
    gtp_slider: &Entity<MoonSliderState>,
    trailing_slider: &Entity<MoonSliderState>,
    vstop_slider: &Entity<MoonSliderState>,
    gtp_input: &Entity<MoonInputState>,
    trailing_input: &Entity<MoonInputState>,
    vstop_input: &Entity<MoonInputState>,
    blacklist_input: &Entity<MoonInputState>,
    blacklist_area: &Entity<MoonInputState>,
    def_strategy_input: &Entity<MoonInputState>,
    blacklist_expanded: bool,
    cancel_confirm: bool,
    seeded: Option<CoreId>,
    backend: &Entity<Backend>,
    group: &str,
    p: MoonPalette,
    cx: &App,
    on_cancel_all: impl Fn(&mut App) + 'static,
    on_toggle_blacklist: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let b = backend.read(cx);
    let core = b.active_trade_core(group);
    let cd = core.and_then(|c| b.session.store().core(c));
    let cs = cd.and_then(|d| d.client_settings.clone());
    let lm = cd.and_then(|d| d.lev_manage.clone());
    // Show labeled Running and Auto Detect dots for the active core. They moved from the header,
    // where the unlabeled dots were cramped beside the gear button.
    let rt = cd.and_then(|d| d.runtime_state);

    // Chrome is MoonPopover's; see `popover_contents_do_not_paint_a_second_surface`.
    let root = v_flex()
        .id("core-settings-popup")
        .w(design::font_w_px(cx, CONTENT_W))
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(popup_title(t!("core_settings.title"), p, cx))
                .child(runtime_status(rt, p, cx)),
        );

    // Show a placeholder when the core or its settings snapshot is unavailable.
    let Some(cs) = cs else {
        return root
            .child(
                div()
                    .text_color(rgb(p.text_muted))
                    .child(t!("core_settings.no_core").to_string()),
            )
            .into_any_element();
    };

    // Header: restart and emulator controls.
    let restart_btn = {
        let backend = backend.clone();
        let group = group.to_string();
        // Keep the localized label clean; MoonButton owns its scaled content inset.
        MoonButton::new("core-restart")
            .label(t!("core_settings.restart").to_string())
            .size(MoonButtonSize::Action)
            .variant(MoonButtonVariant::Blue)
            .padding_x(7.0)
            .on_click(move |_, _w, app| {
                let b = backend.read(app);
                // Guarded like Cancel All beside it: the popup's two destructive actions must not
                // disagree about which core they act on.
                if let Some(core) = resolve_core_settings_write(seeded, b.active_trade_core(&group))
                {
                    if let Err(e) = b.session.restart_now(core) {
                        log::warn!("restart_now failed: {e:#}");
                    }
                }
            })
            .render()
    };
    let emu_check = cs_checkbox(
        "core-emu",
        t!("core_settings.emu").to_string(),
        cs.emu_mode,
        backend,
        group,
        seeded,
        ClientSettingsEdit::EmuMode,
    );
    let header_row = v_flex()
        .w_full()
        .gap(design::ui_px(cx, 6.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(restart_btn)
                .child(div().flex_1())
                .child(emu_check),
        )
        // Display a prominent warning banner while emulator mode is enabled.
        .when(cs.emu_mode, |this| {
            this.child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 6.0))
                    .py(design::ui_px(cx, 3.0))
                    .rounded(design::r_button(cx))
                    .bg(rgba_from(p.amber, 0.18))
                    .border_1()
                    .border_color(rgb(p.amber))
                    .text_color(rgb(p.amber))
                    .text_size(design::t_caption(cx))
                    .child(t!("core_settings.emu_on").to_string()),
            )
        });

    // Behavior Defaults group. Stop Loss and Panic moved to the toolbar beside the SL button.
    // Global TP combines the `use_g_take_profit` flag with the `g_take_profit` value.
    let gtp_cb = {
        let backend = backend.clone();
        let group = group.to_string();
        let pct = cs.global_take_profit_pct;
        icon_checkbox(
            "core-gtp-cb",
            t!("core_settings.toggle_tip").to_string(),
            cs.use_global_take_profit,
            move |ch, _w, app| {
                let on = *ch;
                let b = backend.read(app);
                // `pct` was read at render time from the then-active core; it may only go back to
                // that same core.
                if let Some(core) = resolve_core_settings_write(seeded, b.active_trade_core(&group))
                {
                    if let Err(e) = b.session.edit_client_settings(
                        core,
                        ClientSettingsEdit::GlobalTakeProfit { on, pct },
                    ) {
                        log::warn!("global tp toggle failed: {e:#}");
                    }
                }
            },
        )
    };
    // Trailing has no wire-level enable flag, so a nonzero value means enabled. Disabling sends zero;
    // enabling uses the current slider value or -1.0 by default. The slider and input edit the value.
    let trailing_cb = {
        let backend = backend.clone();
        let group = group.to_string();
        let cur = cs.trailing_drop_pct;
        let slider = trailing_slider.clone();
        icon_checkbox(
            "core-trailing-cb",
            t!("core_settings.toggle_tip").to_string(),
            cur.abs() > 1e-6,
            move |ch, _w, app| {
                let on = *ch;
                let val = if on {
                    if cur.abs() > 1e-6 {
                        cur
                    } else {
                        let s = slider.read(app).value().end();
                        if s.abs() > 1e-6 { s } else { -1.0 }
                    }
                } else {
                    0.0
                };
                let b = backend.read(app);
                // `val` can come from the retained slider, which holds the SEEDED core's value —
                // so this write is addressed the same way the slider's own commits are.
                if let Some(core) = resolve_core_settings_write(seeded, b.active_trade_core(&group))
                {
                    if let Err(e) = b
                        .session
                        .edit_client_settings(core, ClientSettingsEdit::TrailingDrop(val))
                    {
                        log::warn!("trailing toggle failed: {e:#}");
                    }
                }
            },
        )
    };
    // V-Stop also has no wire-level enable flag, so a nonzero value means enabled. The slider and
    // input edit a whole percentage; enabling from zero uses the slider value or -2 by default.
    let vstop_cb = {
        let backend = backend.clone();
        let group = group.to_string();
        let cur = cs.vol_drop_level;
        let slider = vstop_slider.clone();
        icon_checkbox(
            "core-vstop-cb",
            t!("core_settings.toggle_tip").to_string(),
            cur != 0,
            move |ch, _w, app| {
                let on = *ch;
                let n = if on {
                    if cur != 0 {
                        cur
                    } else {
                        let s = slider.read(app).value().end().round() as i32;
                        if s != 0 { s } else { -2 }
                    }
                } else {
                    0
                };
                let b = backend.read(app);
                // `n` can come from the retained slider — the seeded core's value; see the trailing
                // toggle above.
                if let Some(core) = resolve_core_settings_write(seeded, b.active_trade_core(&group))
                {
                    if let Err(e) = b
                        .session
                        .edit_client_settings(core, ClientSettingsEdit::VolDropLevel(n))
                    {
                        log::warn!("vstop toggle failed: {e:#}");
                    }
                }
            },
        )
    };
    let defaults = popup_group(
        "core-frame-defaults",
        t!("core_settings.frame_defaults").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .child(param_row(
                t!("core_settings.global_tp").to_string(),
                gtp_cb,
                gtp_slider,
                "core-gtp-slider",
                gtp_input,
                "core-gtp-input",
                p,
                cx,
            ))
            .child(param_row(
                t!("core_settings.trailing").to_string(),
                trailing_cb,
                trailing_slider,
                "core-trailing-slider",
                trailing_input,
                "core-trailing-input",
                p,
                cx,
            ))
            .child(param_row(
                t!("core_settings.vstop").to_string(),
                vstop_cb,
                vstop_slider,
                "core-vstop-slider",
                vstop_input,
                "core-vstop-input",
                p,
                cx,
            ))
            .child(cs_checkbox(
                "core-buy-iceberg",
                t!("core_settings.buy_iceberg").to_string(),
                cs.buy_iceberg,
                backend,
                group,
                seeded,
                ClientSettingsEdit::BuyIceberg,
            ))
            .child(cs_checkbox(
                "core-sell-iceberg",
                t!("core_settings.sell_iceberg").to_string(),
                cs.sell_iceberg,
                backend,
                group,
                seeded,
                ClientSettingsEdit::SellIceberg,
            )),
    );

    // Risk Limits group: token blacklist. It combines the `use_coins_black_list` flag and
    // `coins_black_list_text` through CoreCmd::SetBlacklist with a local Exclude from Deltas option.
    // The latter has no core read-back, so Backend retains its state.
    let bl_check = {
        let backend = backend.clone();
        let group = group.to_string();
        MoonCheckbox::new("core-bl")
            .label(t!("core_settings.blacklist").to_string())
            .checked(cs.use_blacklist)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let on = *ch;
                let b = backend.read(app);
                if let Some(core) = resolve_core_settings_write(seeded, b.active_trade_core(&group))
                {
                    let text = b
                        .session
                        .store()
                        .core(core)
                        .and_then(|d| d.client_settings.as_ref())
                        .map(|s| s.blacklist_text.clone())
                        .unwrap_or_default();
                    if let Err(e) = b.session.set_blacklist(core, on, text) {
                        log::warn!("blacklist toggle failed: {e:#}");
                    }
                }
            })
    };
    let exclude_on = core.map(|c| b.exclude_bl_delta(c)).unwrap_or(false);
    let exclude_check = {
        let backend = backend.clone();
        let group = group.to_string();
        MoonCheckbox::new("core-bl-exclude")
            .label(t!("core_settings.exclude_delta").to_string())
            .checked(exclude_on)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let on = *ch;
                let active = backend.read(app).active_trade_core(&group);
                if let Some(core) = resolve_core_settings_write(seeded, active) {
                    backend.update(app, |bk, _| bk.set_exclude_bl_delta(core, on));
                    if let Err(e) = backend
                        .read(app)
                        .session
                        .set_exclude_blacklisted_delta(core, on)
                    {
                        log::warn!("exclude delta failed: {e:#}");
                    }
                }
            })
    };
    // The ellipsis button expands or collapses the token-list field. Collapsed mode hides the long
    // tail in one line; expanded mode uses a fixed-height scrolling editor without growing the popover.
    let bl_expand_btn = MoonButton::new("core-bl-expand")
        .label("…")
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Soft)
        .selected(blacklist_expanded)
        .on_click(move |_, w, app| on_toggle_blacklist(w, app))
        .render();
    // Collapsed mode uses a single-line MoonInput so the hidden tail cannot stretch the field.
    // Expanded mode uses a separate multiline `blacklist_area` state. Sharing state is impossible:
    // MoonTextArea permanently switches it to multiline, after which the collapsed input renders as
    // a narrow strip. Shell synchronizes the two states when the ellipsis is toggled, and both have
    // Blur/Enter commit subscriptions. `submit_on_enter` commits instead of inserting a newline
    // because the token list is logically one line. The text area uses only the default Normal
    // height, about three scrolling lines, because moonui does not re-export
    // `MoonTextAreaSize::Custom`.
    let bl_field: AnyElement = if blacklist_expanded {
        MoonTextArea::new("core-bl-area")
            .state(blacklist_area)
            .submit_on_enter(true)
            .mono(true)
            .into_any_element()
    } else {
        MoonInput::new("core-bl-text")
            .state(blacklist_input)
            .small()
            .into_any_element()
    };
    let risks = popup_group(
        "core-frame-risks",
        t!("core_settings.frame_risks").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 6.0))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(bl_check)
                    .child(div().flex_1())
                    .child(bl_expand_btn),
            )
            .child(div().w_full().child(bl_field))
            .child(exclude_check),
    );

    // Leverage and Margin group backed by LevManage.
    let (lev_max, lev_up, lev_iso, lev_cross, lev_tlg) = lm
        .as_ref()
        .map(|l| {
            (
                l.auto_max_order,
                l.auto_lev_up,
                l.auto_isolated,
                l.auto_cross,
                l.tlg_report,
            )
        })
        .unwrap_or((false, false, false, false, false));
    let leverage = popup_group(
        "core-frame-leverage",
        t!("core_settings.frame_leverage").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 6.0))
            .child(lev_checkbox(
                "core-auto-max",
                t!("core_settings.auto_max_order").to_string(),
                lev_max,
                backend,
                group,
                seeded,
                LevManageEdit::AutoMaxOrder,
            ))
            .child(lev_checkbox(
                "core-auto-levup",
                t!("core_settings.auto_lev_up").to_string(),
                lev_up,
                backend,
                group,
                seeded,
                LevManageEdit::AutoLevUp,
            ))
            .child(lev_checkbox(
                "core-isolated",
                t!("core_settings.isolated").to_string(),
                lev_iso,
                backend,
                group,
                seeded,
                LevManageEdit::AutoIsolated,
            ))
            .child(lev_checkbox(
                "core-cross",
                t!("core_settings.cross").to_string(),
                lev_cross,
                backend,
                group,
                seeded,
                LevManageEdit::AutoCross,
            ))
            .child(lev_checkbox(
                "core-tlg",
                t!("core_settings.tlg_report").to_string(),
                lev_tlg,
                backend,
                group,
                seeded,
                LevManageEdit::TlgReport,
            )),
    );

    // Actions group. Reset Session and Reset All are omitted because TResetProfitCommand resets
    // server-side RepForm counters that are not transmitted to the client, making the result
    // invisible in the terminal. Restore these actions when MoonProto exposes those counters.
    let cancel_all = MoonButton::new("core-cancel-all")
        .label(if cancel_confirm {
            t!("core_settings.cancel_all_confirm").to_string()
        } else {
            t!("core_settings.cancel_all").to_string()
        })
        .size(MoonButtonSize::Action)
        .variant(MoonButtonVariant::Danger)
        .selected(cancel_confirm)
        .full_width()
        .on_click(move |_, _w, app| on_cancel_all(app))
        .render();
    let actions = popup_group(
        "core-frame-actions",
        t!("core_settings.frame_actions").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 6.0))
            .children(def_alert_strategy_row(
                core,
                def_strategy_input,
                backend,
                p,
                cx,
            ))
            .child(cancel_all),
    );

    root.child(header_row)
        .child(defaults)
        .child(risks)
        .child(leverage)
        .child(actions)
        .into_any_element()
}

/// Builds labeled Running (`is_started`) and Auto Detect (`auto_detect_active`) status dots for the
/// active core. Enabled states are green; inactive Auto Detect on a running core is amber; other
/// inactive states are gray. These moved from unlabeled dots beside the header gear button.
fn runtime_status(rt: Option<RuntimeState>, p: MoonPalette, cx: &App) -> impl IntoElement {
    let ok = if p.is_light() { p.green_text } else { p.green };
    let started = rt.map(|r| r.is_started).unwrap_or(false);
    let auto = rt.map(|r| r.auto_detect_active).unwrap_or(false);
    let started_color = if started { ok } else { p.text_muted };
    let auto_color = if auto {
        ok
    } else if started {
        p.amber
    } else {
        p.text_muted
    };
    let labeled = |color: u32, label: String, cx: &App| {
        h_flex()
            .items_center()
            .gap(design::ui_px(cx, 4.0))
            .child(design::status_dot(color, cx))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_soft))
                    .child(label),
            )
    };
    h_flex()
        .items_center()
        .gap(design::ui_px(cx, 10.0))
        .child(labeled(
            started_color,
            t!("core_settings.runtime_started").to_string(),
            cx,
        ))
        .child(labeled(
            auto_color,
            t!("core_settings.runtime_auto").to_string(),
            cx,
        ))
}
