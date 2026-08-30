//! "General" tab of the core-settings popup, laid out like Moonbot's page of the same name:
//! exit rules on the left, risk limits and leverage on the right, every value in the row's own
//! caption with its slider underneath.
//!
//! Everything here stages into the shared draft and reaches the core on OK. The three exit rules
//! carry a real enable flag from the safe-share section (`use_g_take_profit`, `trailing_stop`,
//! `panic_if_vol_drop`) rather than the compact channel's "zero means off", so switching one off
//! keeps the level it was switched off at.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonTextArea, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::panels::popup_group;
use crate::shell::Shell;
use crate::shell::core_settings::draft::{
    TAKE_PROFIT_BOUNDS, TRAILING_BOUNDS, VSTOP_BOUNDS, parse_num,
};

use super::widgets::{caption, def_alert_strategy_row, flag, num, slider, stretch_field};
use super::{SettingsWidgets, TabCtx, TextEditors};

/// Editors this tab owns; see [`super::field_specs`] for the tuple's shape.
///
/// Both belong to the leverage block: the fixed-leverage target beside its checkbox, and Moonbot's
/// free-form "Config" rules line.
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str), f32)> {
    let l = &draft.leverage;
    vec![
        (
            "gen-fix-lev",
            l.fix_lev.to_string(),
            // Deliberately NOT clamped here: the editor only re-reads the draft on a re-seed, so
            // clamping mid-typing would leave 150 on screen and 125 in the packet. The single clamp
            // is on the way out, in `Shell::commit_core_draft`.
            (|d, t| {
                if let Some(v) = parse_num(t) {
                    d.leverage.fix_lev = v.round() as i32;
                }
            }) as fn(&mut CoreConfig, &str),
            48.0,
        ),
        (
            "gen-lev-config",
            l.lev_control.clone(),
            |d, t| d.leverage.lev_control = t.to_string(),
            0.0,
        ),
    ]
}

/// Sliders this tab owns; see [`super::slider_specs`] for the tuple's shape.
///
/// None of them mirrors into a numeric editor: each row prints its value in its own caption, the
/// way the Moonbot page does.
#[allow(clippy::type_complexity)]
pub(super) fn slider_specs(
    draft: &CoreConfig,
) -> Vec<(
    &'static str,
    (f32, f32, f32),
    f32,
    fn(&mut CoreConfig, f32),
    Option<&'static str>,
)> {
    let g = &draft.general;
    vec![
        (
            "gen-tp",
            TAKE_PROFIT_BOUNDS,
            g.take_profit_pct as f32,
            (|d, v| d.general.take_profit_pct = f64::from(v)) as fn(&mut CoreConfig, f32),
            None,
        ),
        (
            "gen-trailing",
            TRAILING_BOUNDS,
            g.trailing_pct,
            |d, v| d.general.trailing_pct = v,
            None,
        ),
        (
            "gen-vstop",
            VSTOP_BOUNDS,
            g.vol_drop_level as f32,
            |d, v| d.general.vol_drop_level = v.round() as i32,
            None,
        ),
    ]
}

/// Builds the General tab's body.
///
/// Args:
///     ctx: Popup-wide addressing and palette.
///     draft: Staged settings; every control reads and writes it.
///     widgets: Sliders prepared by Shell for this render.
///     editors: Retained blacklist and strategy-filter editors.
///     view: Shell entity used to stage changes.
///     blacklist_expanded: Whether to render the multiline blacklist editor.
///     cx: Application context used to read state and render controls.
///     on_toggle_blacklist: Callback that toggles the blacklist editor mode.
///
/// Returns:
///     The tab body, without the popup chrome, the tab strip, or the OK/Cancel footer.
#[allow(clippy::too_many_arguments)]
pub(super) fn general_tab(
    ctx: &TabCtx<'_>,
    draft: &CoreConfig,
    widgets: &SettingsWidgets,
    editors: &TextEditors<'_>,
    view: &Entity<Shell>,
    blacklist_expanded: bool,
    cx: &App,
    on_toggle_blacklist: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let TabCtx {
        backend, group, p, ..
    } = *ctx;
    let core = backend.read(cx).active_trade_core(group);
    let g = &draft.general;
    let gap = design::ui_px(cx, 6.0);

    // --- Exit rules: value in the caption, slider underneath, as on the Moonbot page ---
    let stops = popup_group(
        "core-gen-stops",
        t!("core_settings.gen_stops_frame").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .child(flag(
                "gen-tp-on",
                t!(
                    "core_settings.gen_tp_line",
                    v = format!("{:.2}", g.take_profit_pct)
                )
                .to_string(),
                g.take_profit_on,
                view,
                |d, on| d.general.take_profit_on = on,
            ))
            .children(slider(widgets, "gen-tp"))
            .child(flag(
                "gen-trailing-on",
                t!(
                    "core_settings.gen_trailing_line",
                    v = format!("{:.2}", g.trailing_pct)
                )
                .to_string(),
                g.trailing_on,
                view,
                |d, on| d.general.trailing_on = on,
            ))
            .children(slider(widgets, "gen-trailing"))
            .child(flag(
                "gen-vstop-on",
                t!(
                    "core_settings.gen_vstop_line",
                    v = g.vol_drop_level.to_string()
                )
                .to_string(),
                g.vstop_on,
                view,
                |d, on| d.general.vstop_on = on,
            ))
            .children(slider(widgets, "gen-vstop"))
            .child(flag(
                "gen-buy-iceberg",
                t!("core_settings.buy_iceberg").to_string(),
                g.buy_iceberg,
                view,
                |d, on| d.general.buy_iceberg = on,
            ))
            .child(flag(
                "gen-sell-iceberg",
                t!("core_settings.sell_iceberg").to_string(),
                g.sell_iceberg,
                view,
                |d, on| d.general.sell_iceberg = on,
            )),
    );

    // --- Risk limits: the coin blacklist and whether it also filters the deltas ---
    // The ellipsis button expands or collapses the token-list field. Collapsed mode hides the long
    // tail in one line; expanded mode uses a fixed-height scrolling editor without growing the
    // popover.
    let bl_expand_btn = MoonButton::new("core-bl-expand")
        .label("…")
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Soft)
        .selected(blacklist_expanded)
        .on_click(move |_, w, app| on_toggle_blacklist(w, app))
        .render();
    // Collapsed mode uses a single-line MoonInput so the hidden tail cannot stretch the field.
    // Expanded mode uses a separate multiline state. Sharing one state is impossible: MoonTextArea
    // permanently switches it to multiline, after which the collapsed input renders as a narrow
    // strip. Shell synchronizes the two when the ellipsis is toggled, and both stage on Blur/Enter.
    // `submit_on_enter` stages instead of inserting a newline because the token list is logically
    // one line. The text area uses only the default Normal height, about three scrolling lines,
    // because moonui does not re-export `MoonTextAreaSize::Custom`.
    let bl_field: AnyElement = if blacklist_expanded {
        MoonTextArea::new("core-bl-area")
            .state(editors.area)
            .submit_on_enter(true)
            .mono(true)
            .into_any_element()
    } else {
        MoonInput::new("core-bl-text")
            .state(editors.input)
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
            .gap(gap)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .child(flag(
                        "core-bl",
                        t!("core_settings.blacklist").to_string(),
                        g.blacklist_on,
                        view,
                        |d, on| d.general.blacklist_on = on,
                    ))
                    .child(flag(
                        "core-bl-exclude",
                        t!("core_settings.exclude_delta").to_string(),
                        g.exclude_blacklisted_from_deltas,
                        view,
                        |d, on| d.general.exclude_blacklisted_from_deltas = on,
                    ))
                    .child(div().flex_1())
                    .child(bl_expand_btn),
            )
            .child(div().w_full().child(bl_field)),
    );

    // --- Leverage and margin, straight from `trading.auto_manage_lev` ---
    //
    // These five used to travel over the `LevManage` command, whose snapshot the core never sends
    // and the protocol cannot request — so every click was dropped before it reached the wire. The
    // safe-share section carries the same fields and does arrive.
    let l = &draft.leverage;
    // Two columns and one row per pair, matching Moonbot's own leverage panel. Its four buttons
    // (Set Leverage to / Set Max Leverage / Make ALL Isolated / Make ALL Cross) have no counterpart
    // here on purpose: the Engine API exposes only per-market `SetLeverage` and
    // `ChangePositionType`, so those are Moonbot walking its own market table — there is no
    // parameter to send that would make the core do it.
    let leverage = popup_group(
        "core-frame-leverage",
        t!("core_settings.frame_leverage").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(gap)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    // Isolated and cross are mutually exclusive in Moonbot, so each clears the
                    // other here rather than sending a packet that claims both.
                    .child(div().flex_1().child(flag(
                        "core-isolated",
                        t!("core_settings.isolated").to_string(),
                        l.auto_isolated,
                        view,
                        |d, on| {
                            d.leverage.auto_isolated = on;
                            if on {
                                d.leverage.auto_cross = false;
                            }
                        },
                    )))
                    .child(
                        h_flex()
                            .flex_1()
                            .items_center()
                            .gap(design::ui_px(cx, 6.0))
                            .child(flag(
                                "core-auto-fix-lev",
                                t!("core_settings.auto_fix_lev").to_string(),
                                l.auto_fix_lev,
                                view,
                                |d, on| d.leverage.auto_fix_lev = on,
                            ))
                            .children(num(widgets, "gen-fix-lev", cx)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .child(div().flex_1().child(flag(
                        "core-cross",
                        t!("core_settings.cross").to_string(),
                        l.auto_cross,
                        view,
                        |d, on| {
                            d.leverage.auto_cross = on;
                            if on {
                                d.leverage.auto_isolated = false;
                            }
                        },
                    )))
                    .child(div().flex_1().child(flag(
                        "core-tlg",
                        t!("core_settings.tlg_report").to_string(),
                        l.tlg_report,
                        view,
                        |d, on| d.leverage.tlg_report = on,
                    ))),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .child(div().flex_1().child(flag(
                        "core-auto-max",
                        t!("core_settings.auto_max_order").to_string(),
                        l.auto_max_order,
                        view,
                        |d, on| d.leverage.auto_max_order = on,
                    )))
                    .child(div().flex_1().child(flag(
                        "core-auto-levup",
                        t!("core_settings.auto_lev_up").to_string(),
                        l.auto_lev_up,
                        view,
                        |d, on| d.leverage.auto_lev_up = on,
                    ))),
            )
            // Moonbot's "Config" line: free-form leverage rules such as `250 BTC`, parsed by the
            // core. Carried through the same OK as everything else on this tab.
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .child(caption(t!("core_settings.lev_config").to_string(), p, cx))
                    .children(stretch_field(widgets, "gen-lev-config")),
            ),
    );

    // Default Alert Strategy is terminal-local state persisted in the server config, not a core
    // setting, so it applies on selection and is deliberately outside the OK contract.
    let alerts = popup_group(
        "core-frame-actions",
        t!("core_settings.frame_actions").to_string(),
    )
    .child(v_flex().w_full().gap(gap).children(def_alert_strategy_row(
        core,
        editors.def_strategy,
        backend,
        p,
        cx,
    )));

    h_flex()
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 10.0))
        .child(v_flex().flex_1().gap(design::ui_px(cx, 8.0)).child(stops))
        .child(
            v_flex()
                .flex_1()
                .gap(design::ui_px(cx, 8.0))
                .child(risks)
                .child(leverage)
                .child(alerts),
        )
        .into_any_element()
}
