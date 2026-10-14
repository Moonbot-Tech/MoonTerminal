//! "General" tab of the core-settings popup, laid out like Moonbot's page of the same name:
//! exit rules on the left, risk limits and leverage on the right, every value in the row's own
//! caption with its slider underneath.
//!
//! Everything here stages into the shared draft and reaches the core on OK. The three exit rules
//! carry a real enable flag from the safe-share section (`use_g_take_profit`, `trailing_stop`,
//! `panic_if_vol_drop`) rather than the compact channel's "zero means off", so switching one off
//! keeps the level it was switched off at.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonInput, MoonPalette, MoonTextArea, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design::{self, moon, moon_alpha};
use crate::panels::popup_group;
use crate::shell::Shell;
use crate::shell::core_settings::draft::{
    TAKE_PROFIT_BOUNDS, TRAILING_BOUNDS, VSTOP_BOUNDS, parse_num,
};
use crate::shell::core_settings::{BlacklistReveal, coin_blacklist_remove_button};

use super::widgets::{
    caption, def_alert_strategy_row, flag, num, slider, sound_cell, stretch_field,
};
use super::{SettingsWidgets, TabCtx, TextEditors};

/// Editors this tab owns; see [`super::field_specs`] for the tuple's shape.
///
/// Two belong to the leverage block — the fixed-leverage target beside its checkbox and Moonbot's
/// free-form "Config" rules line — and two are the price-approach alerts' levels.
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
            // is on the way out, in `core_settings::draft::send_core_config`, which bounds every
            // value but an untouched 0 — the wire's "no fixed multiplier chosen".
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
        // Whole per cent, as the wire carries them. Unclamped for the same reason `gen-fix-lev` is:
        // the editor re-reads the draft only on a re-seed, so clamping mid-typing would leave one
        // number on screen and another in the packet.
        (
            "gen-sell-alert-lvl",
            draft.signals.sell_alert_level.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.signals.sell_alert_level = v.round() as i32;
                }
            },
            40.0,
        ),
        (
            "gen-buy-alert-lvl",
            draft.signals.buy_alert_level.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.signals.buy_alert_level = v.round() as i32;
                }
            },
            40.0,
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

/// The blacklist rows inside the compact popup's risk-limits group.
///
/// The token strip sits above the editor. The remove button sits on the label row, before the
/// expand control. Both appear only while `reveal`'s coin is still in `list_text`.
///
/// Args:
///     blacklist_flag: The «Чёрный список» checkbox.
///     exclude_flag: The delta-filter checkbox.
///     expand: The ellipsis that switches the editor between one line and a text area.
///     editor: The blacklist field that is on screen.
///     list_text: Draft blacklist text the strip is painted from.
///     reveal: Highlight, when a reveal is showing.
///     on_remove: Writes the seeded core and rewrites the draft.
///     cx: Application context for type scale and colour.
///
/// Returns:
///     The rows the risk-limits group hosts.
#[allow(clippy::too_many_arguments)]
pub(super) fn compact_blacklist_rows(
    blacklist_flag: impl IntoElement,
    exclude_flag: impl IntoElement,
    expand: impl IntoElement,
    editor: impl IntoElement,
    list_text: &str,
    reveal: Option<BlacklistReveal<'_>>,
    on_remove: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let marked =
        reveal.is_some_and(|item| moon_core::symbol::coin_list::contains(list_text, item.coin));
    let error = reveal.and_then(|item| item.error.map(str::to_string));
    let coin = reveal.map(|item| item.coin.to_string()).unwrap_or_default();
    let gap = design::ui_px(cx, 6.0);
    let remove = marked.then(|| {
        div()
            .id("bl-compact-remove")
            .debug_selector(|| "bl-compact-remove".into())
            .flex_none()
            .child(coin_blacklist_remove_button(
                "core-bl-remove",
                &coin,
                on_remove,
            ))
    });
    let strip = marked.then(|| {
        div().id("bl-compact-token").w_full().min_w_0().child(
            crate::controls::coin_token::coin_token_strip(
                list_text,
                &coin,
                design::t_body(cx),
                // The blacklist sits in one of two columns, so the strip gets half the width.
                Some(design::font_w_px(cx, super::CONTENT_W / 2.0 - 60.0)),
                cx,
            ),
        )
    });
    v_flex()
        .id("bl-compact-rows")
        .debug_selector(|| "bl-compact-rows".into())
        .w_full()
        .gap(gap)
        .child(
            h_flex()
                .id("bl-compact-label")
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 10.0))
                .child(blacklist_flag)
                .child(exclude_flag)
                .child(div().flex_1())
                .children(remove)
                .child(expand),
        )
        .children(error.map(|text| {
            div()
                .id("bl-compact-error")
                .w_full()
                .text_size(design::t_caption(cx))
                .text_color(moon(design::danger_color(MoonPalette::active(cx))))
                .child(text)
        }))
        .children(strip)
        .child(div().id("bl-compact-editor").w_full().child(editor))
        .into_any_element()
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
///     bl_reveal: Coin highlighted on this tab, when a reveal is showing.
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
    bl_reveal: Option<BlacklistReveal<'_>>,
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
            .size(design::INPUT_SIZE)
            .into_any_element()
    };
    let alpha = bl_reveal.map(|item| item.alpha).unwrap_or(0.0);
    let weak = view.downgrade();
    let risks_rows = compact_blacklist_rows(
        flag(
            "core-bl",
            t!("core_settings.blacklist").to_string(),
            g.blacklist_on,
            view,
            |d, on| d.general.blacklist_on = on,
        ),
        flag(
            "core-bl-exclude",
            t!("core_settings.exclude_delta").to_string(),
            g.exclude_blacklisted_from_deltas,
            view,
            |d, on| d.general.exclude_blacklisted_from_deltas = on,
        ),
        bl_expand_btn,
        bl_field,
        &g.blacklist_text,
        bl_reveal,
        move |window, app| {
            let _ = weak.update(app, |this, cx| {
                this.remove_highlighted_core_coin(window, cx)
            });
        },
        cx,
    );
    let risks = popup_group(
        "core-frame-risks",
        t!("core_settings.frame_risks").to_string(),
    )
    .child(
        div()
            .id("core-frame-risks-body")
            .w_full()
            .when(alpha > 0.0, |body| {
                body.rounded(design::ui_px(cx, 4.0))
                    .border(px(1.5))
                    .border_color(moon_alpha(p.amber, alpha))
            })
            .child(risks_rows),
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
    .child(
        v_flex()
            .w_full()
            .gap(gap)
            .children(def_alert_strategy_row(core, backend, p, cx)),
    );

    // --- Moonbot's two price-approach alerts, laid out as its own page has them: the level inside
    // the checkbox caption AND in an editor beside it, with the sound picker under each caption.
    //
    // The sound plays HERE rather than on the core; see `backend::alert_sound`.
    let sig = &draft.signals;
    let sounds = popup_group(
        "core-frame-alert-sounds",
        t!("core_settings.frame_alert_sounds").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(gap)
            .child(flag(
                "gen-sell-alert-on",
                t!(
                    "core_settings.sell_alert_line",
                    v = sig.sell_alert_level.to_string()
                )
                .to_string(),
                sig.play_sell_alert,
                view,
                |d, on| d.signals.play_sell_alert = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .child(sound_cell(
                        "gen-sell-alert-sound",
                        sig.signal_sound_2,
                        view,
                        |d, v| d.signals.signal_sound_2 = v,
                        p,
                        cx,
                    ))
                    .child(div().flex_1())
                    .children(num(widgets, "gen-sell-alert-lvl", cx)),
            )
            .child(flag(
                "gen-buy-alert-on",
                t!(
                    "core_settings.buy_alert_line",
                    v = sig.buy_alert_level.to_string()
                )
                .to_string(),
                sig.play_buy_alert,
                view,
                |d, on| d.signals.play_buy_alert = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .child(sound_cell(
                        "gen-buy-alert-sound",
                        sig.buy_signal_sound,
                        view,
                        |d, v| d.signals.buy_signal_sound = v,
                        p,
                        cx,
                    ))
                    .child(div().flex_1())
                    .children(num(widgets, "gen-buy-alert-lvl", cx)),
            ),
    );

    h_flex()
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 10.0))
        .child(
            v_flex()
                .flex_1()
                .gap(design::ui_px(cx, 8.0))
                .child(stops)
                .child(sounds),
        )
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

#[cfg(test)]
mod tests;
