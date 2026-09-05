//! Moonbot's "Интерфейс" page, control for control — and LIVE for the half of it the safe-share
//! snapshot carries.
//!
//! The longest page of the dialog. Its values are Moonbot's OWN window and chart appearance — which
//! panels it opens, how it paints its price line, what its order-book zones look like. None of it
//! changes this terminal, which has its own chart, its own panels and its own theme; changing what
//! the OTHER program looks like is the point.
//!
//! What is live is `moon_core::feed::InterfaceSettings` — spread across the wire's `trading`,
//! `visual`, `signals` and `ui` sections — plus the two price-approach alerts of
//! `moon_core::feed::SignalsSettings`, which Moonbot draws on this page and the compact popup draws
//! on its own. This tab names both areas; see `super::super::ExpertTab::add_sections`.
//!
//! What is left disabled is disabled for a reason that can be named per row, and no longer for the
//! want of one. Four checkbox rows have no field in the snapshot at all — the two windows Moonbot
//! opens on start, closing to the tray, encrypting its reports — and neither do the style picker,
//! the Pixel Size track, the background image or the price panel. Nor do the two buttons: "Colors
//! Setup" opens a dialog of its own and "Reset Reports form" acts on Moonbot's report window, and
//! an action is not a setting to carry.
//!
//! Three more are held back although the field IS there — "Use Leverage for TP", "Не учитывать
//! SellPrice ручной стратегии" and "Открывать выбранные вручную монеты в FullScreen". Which fields
//! those are, and why each cannot be written from here, is named once in `moon_core`'s
//! `apply_interface` rather than repeated here.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::shell::editors::EditorStore;
use crate::shell::parse_num;

use super::super::CoreExpertView;
use super::super::widgets::{
    action, caption, columns, dropdown, flag_dead, flag_live, group, num, rows, slider, sound_cell,
    stepper_live, text_line,
};

/// The write for a slider this page draws but the snapshot does not carry.
const DEAD_NUM: fn(&mut CoreConfig, f32) = |_, _| {};

/// Whole per cent, the way the wire stores every transparency on this page — and the range the
/// dead ones resemble, so a track this terminal cannot fill still moves like its neighbours.
const PCT: (f32, f32, f32) = (0.0, 100.0, 1.0);

/// The order-book BORDER track, which stops where Moonbot's own does.
///
/// Its `bGlassOpacity` declares `Max = 75` while the two tracks beside it declare none, and
/// `super::autobuy`'s rule is to widen a range only where Moonbot states none — here it states one.
/// What that buys is narrow and worth stating exactly: it stops the USER producing a value Moonbot's
/// own control cannot, which is the kind the core is most likely to clamp — and a clamped value
/// never echoes back, so the write would spend its whole retry budget before being dropped.
///
/// It does NOT bound what is sent. A slider clamps for display only (`shell::core_settings::editors`
/// stages nothing on a re-seed), so a core already holding more than 75 keeps that value: the thumb
/// sits at the end of the track while the caption beside it prints the truth, and OK sends the
/// truth. The wire's own type is 0..100, so 76..100 stays legal and simply unreachable from here.
const BORDER_PCT: (f32, f32, f32) = (0.0, 75.0, 1.0);

/// Floor for the two line widths, both drawn as a spinner rather than a track. A width is a pixel
/// count, so below zero is meaningless; there is no ceiling, because the wire states none and
/// inventing one would show a number OK does not send.
const WIDTH_FLOOR: i32 = 0;

/// Floor for the two alert levels. Zero is a legitimate value — it means "when the price has
/// reached the order's price" — and the flag beside it is what switches the alert off.
const ALERT_LEVEL_FLOOR: i32 = 0;

/// Value shown where Moonbot prints a number this terminal has not read.
const NO_VALUE: &str = "—";

/// Widest spread a keystroke may stage, in per cent.
///
/// Not Moonbot's own bound, which the protocol does not state — a sanity range, and the only one on
/// this path: `send_core_config` bounds nothing but the leverage, so without it a stray digit
/// reaches a live core as a pending-order spread of 1e300. Applied to the MAGNITUDE, because a
/// spread may legitimately be placed on either side of the price.
///
/// Out of range is REFUSED rather than clamped, which is what every other numeric box on these
/// pages does with text it cannot use: the last valid value stands. That does not make the box
/// agree with the draft while the impossible text is still in it — nothing here re-writes what was
/// typed — but it keeps the rule one rule, instead of clamping here and refusing everywhere else.
const SPREAD_LIMIT: f64 = 100.0;

/// A spread a keystroke may stage, or nothing.
fn parse_spread(t: &str) -> Option<f64> {
    parse_num(t).filter(|v| v.abs() <= SPREAD_LIMIT)
}

/// Print a spread so that reading it back yields the same number.
///
/// Rust's default `f64` formatting is the shortest text that round-trips, which matters here
/// because the first keystroke in the box parses whatever is displayed back into the draft: a fixed
/// four decimals would quietly rewrite the core's 0.12345 as 0.1235.
fn fmt_spread(v: f64) -> String {
    format!("{v}")
}

/// See [`super::field_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    let i = &draft.interface;
    vec![
        (
            "exp-int-spread-base",
            fmt_spread(i.pending_orders_spread),
            (|d, t| {
                if let Some(v) = parse_spread(t) {
                    d.interface.pending_orders_spread = v;
                }
            }) as fn(&mut CoreConfig, &str),
        ),
        (
            "exp-int-spread-delta",
            fmt_spread(i.pending_orders_spread_h_delta),
            |d, t| {
                if let Some(v) = parse_spread(t) {
                    d.interface.pending_orders_spread_h_delta = v;
                }
            },
        ),
    ]
}

/// See [`super::slider_specs`].
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
    let i = &draft.interface;
    vec![
        ("exp-int-pixel-size", PCT, 0.0, DEAD_NUM, None),
        (
            "exp-int-zone-border",
            BORDER_PCT,
            i.glass_opacity as f32,
            |d, v| d.interface.glass_opacity = v.round() as i32,
            None,
        ),
        (
            "exp-int-zone-fill",
            PCT,
            i.book_cumulative_opacity as f32,
            |d, v| d.interface.book_cumulative_opacity = v.round() as i32,
            None,
        ),
        (
            "exp-int-zone-orders",
            PCT,
            i.book_orders_opacity as f32,
            |d, v| d.interface.book_orders_opacity = v.round() as i32,
            None,
        ),
        (
            "exp-int-panic-zone",
            PCT,
            i.panic_sell_opacity as f32,
            |d, v| d.interface.panic_sell_opacity = v.round() as i32,
            None,
        ),
        ("exp-int-background", PCT, 0.0, DEAD_NUM, None),
    ]
}

/// Build the page.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    draft: &CoreConfig,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 4.0);
    let i = &draft.interface;
    let sig = &draft.signals;

    // --- Left: Moonbot's main window ---------------------------------------------------------
    let main_window = group("exp-int-main", t!("core_expert.int_main_frame").to_string()).child(
        rows(cx)
            .gap(gap)
            .child(flag_live(
                "exp-int-buy-enter",
                "core_expert.int_buy_on_enter",
                i.buy_on_enter,
                view,
                |d, on| d.interface.buy_on_enter = on,
            ))
            .child(flag_dead(
                "exp-int-log-window",
                "core_expert.int_log_window",
                view,
            ))
            .child(flag_dead(
                "exp-int-orders-window",
                "core_expert.int_orders_window",
                view,
            ))
            .child(flag_dead(
                "exp-int-to-tray",
                "core_expert.int_to_tray",
                view,
            ))
            .child(flag_live(
                "exp-int-restore-on-signal",
                "core_expert.int_restore_on_signal",
                i.auto_show_on_signal,
                view,
                |d, on| d.interface.auto_show_on_signal = on,
            ))
            .child(flag_live(
                "exp-int-hide-forum",
                "core_expert.int_hide_forum",
                i.hide_forum_label,
                view,
                |d, on| d.interface.hide_forum_label = on,
            ))
            .child(flag_live(
                "exp-int-scroll-charts",
                "core_expert.int_scroll_charts",
                i.scrolling_charts,
                view,
                |d, on| d.interface.scrolling_charts = on,
            ))
            .child(flag_live(
                "exp-int-open-all",
                "core_expert.int_open_all_charts",
                i.startup_load_charts,
                view,
                |d, on| d.interface.startup_load_charts = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(flag_dead(
                        "exp-int-lev-for-tp",
                        "core_expert.int_lev_for_tp",
                        view,
                    ))
                    .child(flag_live(
                        "exp-int-ask-on-exit",
                        "core_expert.int_ask_on_exit",
                        i.confirm_close,
                        view,
                        |d, on| d.interface.confirm_close = on,
                    )),
            )
            .child(flag_dead(
                "exp-int-ignore-ms-sell",
                "core_expert.int_ignore_ms_sell",
                view,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(flag_dead(
                        "exp-int-encrypt-reports",
                        "core_expert.int_encrypt_reports",
                        view,
                    ))
                    .child(flag_live(
                        "exp-int-detect-buttons",
                        "core_expert.int_detect_buttons",
                        i.show_detects_tool,
                        view,
                        |d, on| d.interface.show_detects_tool = on,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(flag_live(
                        "exp-int-hide-buy",
                        "core_expert.int_hide_buy",
                        i.hide_buy_button,
                        view,
                        |d, on| d.interface.hide_buy_button = on,
                    ))
                    .child(flag_live(
                        "exp-int-hide-demo",
                        "core_expert.int_hide_demo",
                        i.hide_demo_button,
                        view,
                        |d, on| d.interface.hide_demo_button = on,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(flag_live(
                        "exp-int-hide-bonus",
                        "core_expert.int_hide_bonus",
                        i.hide_cashback_button,
                        view,
                        |d, on| d.interface.hide_cashback_button = on,
                    ))
                    .child(flag_live(
                        "exp-int-hide-candy",
                        "core_expert.int_hide_candy",
                        i.hide_cashback_info,
                        view,
                        |d, on| d.interface.hide_cashback_info = on,
                    )),
            )
            // Moonbot's three alert sounds, each a switch over a picker and — for the last two — a
            // level. These are the `signals` section the compact popup draws on its own General
            // tab; Moonbot keeps them here, so this page owns them.
            .child(flag_live(
                "exp-int-net-sound",
                "core_expert.int_network_sound",
                i.play_signal_sound,
                view,
                |d, on| d.interface.play_signal_sound = on,
            ))
            // The picker stays dead: the only unclaimed preset field in the section is
            // `signals.signal_sound`, whose own wire doc assigns it to incoming SIGNAL
            // notifications rather than to this connectivity alert.
            .child(dropdown(
                "exp-int-net-sound-pick",
                NO_VALUE.to_string(),
                false,
            ))
            .child(flag_live(
                "exp-int-sell-sound",
                "core_expert.int_sell_sound",
                sig.play_sell_alert,
                view,
                |d, on| d.signals.play_sell_alert = on,
            ))
            .child(
                h_flex()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    // Live whichever way the alert flag stands, like the level beside it and
                    // like the compact popup's copy of this row: the flag switches the ALERT, not
                    // the ability to choose the sound it will use. Gating one half of the pair made
                    // the two faces of one gear disagree about the same row.
                    .child(sound_cell(
                        "exp-int-sell-sound-pick",
                        sig.signal_sound_2,
                        view,
                        |d, v| d.signals.signal_sound_2 = v,
                        p,
                        cx,
                    ))
                    .child(stepper_live(
                        "exp-int-sell-sound-level",
                        sig.sell_alert_level,
                        ALERT_LEVEL_FLOOR,
                        view,
                        |d, v| d.signals.sell_alert_level = v,
                    )),
            )
            .child(flag_live(
                "exp-int-buy-sound",
                "core_expert.int_buy_sound",
                sig.play_buy_alert,
                view,
                |d, on| d.signals.play_buy_alert = on,
            ))
            .child(
                h_flex()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    // Live whichever way the alert flag stands, like the level beside it and
                    // like the compact popup's copy of this row: the flag switches the ALERT, not
                    // the ability to choose the sound it will use. Gating one half of the pair made
                    // the two faces of one gear disagree about the same row.
                    .child(sound_cell(
                        "exp-int-buy-sound-pick",
                        sig.buy_signal_sound,
                        view,
                        |d, v| d.signals.buy_signal_sound = v,
                        p,
                        cx,
                    ))
                    .child(stepper_live(
                        "exp-int-buy-sound-level",
                        sig.buy_alert_level,
                        ALERT_LEVEL_FLOOR,
                        view,
                        |d, v| d.signals.buy_alert_level = v,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(design::ui_px(cx, 12.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!("core_expert.int_style").to_string(),
                                false,
                                p,
                                cx,
                            ))
                            .child(dropdown("exp-int-style", NO_VALUE.to_string(), false)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!("core_expert.int_icons").to_string(),
                                false,
                                p,
                                cx,
                            ))
                            // Shown, not chosen: the protocol carries the index but no table to
                            // name the variants with, and a menu of numbers would say less than the
                            // number already does.
                            .child(dropdown(
                                "exp-int-icons",
                                i.icon_selection.to_string(),
                                false,
                            )),
                    ),
            )
            .child(action(
                "exp-int-reset-reports",
                t!("core_expert.int_reset_reports").to_string(),
                false,
            )),
    );

    // --- Right: Moonbot's own market charts ---------------------------------------------------
    let charts = group(
        "exp-int-charts",
        t!("core_expert.int_charts_frame").to_string(),
    )
    .child(
        rows(cx)
            .gap(gap)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .child(action(
                        "exp-int-colors",
                        t!("core_expert.int_colors_setup").to_string(),
                        false,
                    ))
                    .child(caption(
                        t!("core_expert.int_price_width").to_string(),
                        true,
                        p,
                        cx,
                    ))
                    .child(stepper_live(
                        "exp-int-price-width",
                        i.price_line_width,
                        WIDTH_FLOOR,
                        view,
                        |d, v| d.interface.price_line_width = v,
                    )),
            )
            .child(caption(
                t!("core_expert.int_pixel_size").to_string(),
                false,
                p,
                cx,
            ))
            .children(slider(store, "exp-int-pixel-size", false))
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(design::ui_px(cx, 12.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(flag_live(
                                "exp-int-hints",
                                "core_expert.int_chart_hints",
                                i.show_market_captions,
                                view,
                                |d, on| d.interface.show_market_captions = on,
                            ))
                            .child(flag_live(
                                "exp-int-profit-usd",
                                "core_expert.int_profit_in_usd",
                                i.show_usd_on_charts,
                                view,
                                |d, on| d.interface.show_usd_on_charts = on,
                            ))
                            .child(flag_live(
                                "exp-int-label-iceberg",
                                "core_expert.int_label_iceberg",
                                i.show_iceberg,
                                view,
                                |d, on| d.interface.show_iceberg = on,
                            )),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(flag_live(
                                "exp-int-label-orders",
                                "core_expert.int_label_orders",
                                i.show_orders_captions,
                                view,
                                |d, on| d.interface.show_orders_captions = on,
                            ))
                            .child(div().pl(design::ui_px(cx, 18.0)).child(flag_live(
                                "exp-int-under-line",
                                "core_expert.int_under_line",
                                i.orders_captions_lower,
                                view,
                                |d, on| d.interface.orders_captions_lower = on,
                            )))
                            .child(dropdown("exp-int-price-panel", NO_VALUE.to_string(), false)),
                    ),
            )
            .child(caption(
                t!("core_expert.int_zone_transparency").to_string(),
                true,
                p,
                cx,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(design::ui_px(cx, 10.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!(
                                    "core_expert.int_zone_border",
                                    v = i.glass_opacity.to_string()
                                )
                                .to_string(),
                                true,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-int-zone-border", true)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!(
                                    "core_expert.int_zone_fill",
                                    v = i.book_cumulative_opacity.to_string()
                                )
                                .to_string(),
                                true,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-int-zone-fill", true)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!(
                                    "core_expert.int_zone_orders",
                                    v = i.book_orders_opacity.to_string()
                                )
                                .to_string(),
                                true,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-int-zone-orders", true))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(design::ui_px(cx, 6.0))
                                    .child(caption(
                                        t!("core_expert.int_line_width").to_string(),
                                        true,
                                        p,
                                        cx,
                                    ))
                                    .child(stepper_live(
                                        "exp-int-line-width",
                                        i.book_orders_width,
                                        WIDTH_FLOOR,
                                        view,
                                        |d, v| d.interface.book_orders_width = v,
                                    )),
                            ),
                    ),
            )
            .child(caption(
                t!(
                    "core_expert.int_panic_zone",
                    v = i.panic_sell_opacity.to_string()
                )
                .to_string(),
                true,
                p,
                cx,
            ))
            .children(slider(store, "exp-int-panic-zone", true))
            .child(text_line(
                t!("core_expert.int_background", v = NO_VALUE).to_string(),
                p.accent,
                false,
                cx,
            ))
            .children(slider(store, "exp-int-background", false))
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(design::ui_px(cx, 12.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(flag_live(
                                "exp-int-dbl-panic",
                                "core_expert.int_double_click_panic",
                                i.dbl_click_panic_sell,
                                view,
                                |d, on| d.interface.dbl_click_panic_sell = on,
                            ))
                            .child(flag_live(
                                "exp-int-zones",
                                "core_expert.int_control_zones",
                                i.chart_split_zones,
                                view,
                                |d, on| d.interface.chart_split_zones = on,
                            ))
                            .child(flag_live(
                                "exp-int-new-on-top",
                                "core_expert.int_new_charts_on_top",
                                i.new_markets_on_top,
                                view,
                                |d, on| d.interface.new_markets_on_top = on,
                            ))
                            .child(flag_live(
                                "exp-int-update-title",
                                "core_expert.int_update_title",
                                i.use_last_detect_caption,
                                view,
                                |d, on| d.interface.use_last_detect_caption = on,
                            ))
                            .child(flag_live(
                                "exp-int-hide-right",
                                "core_expert.int_hide_right_panel",
                                i.hide_right_chart_panel,
                                view,
                                |d, on| d.interface.hide_right_chart_panel = on,
                            ))
                            .child(flag_live(
                                "exp-int-server-charts",
                                "core_expert.int_server_charts",
                                i.auto_request_charts,
                                view,
                                |d, on| d.interface.auto_request_charts = on,
                            ))
                            .child(flag_live(
                                "exp-int-pending-price",
                                "core_expert.int_pending_price",
                                i.pending_buy_price,
                                view,
                                |d, on| d.interface.pending_buy_price = on,
                            ))
                            .child(flag_dead(
                                "exp-int-manual-fullscreen",
                                "core_expert.int_manual_fullscreen",
                                view,
                            )),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(flag_live(
                                "exp-int-one-fullscreen",
                                "core_expert.int_one_fullscreen",
                                i.full_screen_prevent_signals,
                                view,
                                |d, on| d.interface.full_screen_prevent_signals = on,
                            ))
                            .child(flag_live(
                                "exp-int-stop-line",
                                "core_expert.int_stop_line",
                                i.draw_stop,
                                view,
                                |d, on| d.interface.draw_stop = on,
                            ))
                            .child(flag_live(
                                "exp-int-compact",
                                "core_expert.int_compact",
                                i.new_markets_max_scale,
                                view,
                                |d, on| d.interface.new_markets_max_scale = on,
                            ))
                            .child(flag_live(
                                "exp-int-info-right",
                                "core_expert.int_info_right",
                                !i.left_chart_info,
                                view,
                                |d, on| d.interface.left_chart_info = !on,
                            ))
                            .child(flag_live(
                                "exp-int-hide-pnl",
                                "core_expert.int_hide_pnl",
                                i.hide_pnl,
                                view,
                                |d, on| d.interface.hide_pnl = on,
                            ))
                            .child(flag_live(
                                "exp-int-scale-tool",
                                "core_expert.int_scale_tool",
                                i.scale_tool,
                                view,
                                |d, on| d.interface.scale_tool = on,
                            ))
                            .child(flag_live(
                                "exp-int-buttons-memory",
                                "core_expert.int_buttons_memory",
                                i.remember_chart_buttons,
                                view,
                                |d, on| d.interface.remember_chart_buttons = on,
                            )),
                    ),
            )
            .child(
                group(
                    "exp-int-spread",
                    t!("core_expert.int_spread_frame").to_string(),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap(design::ui_px(cx, 6.0))
                        .children(num(store, "exp-int-spread-base", 64.0, true, cx))
                        .child(caption(
                            t!("core_expert.int_spread_hdelta").to_string(),
                            true,
                            p,
                            cx,
                        ))
                        .children(num(store, "exp-int-spread-delta", 64.0, true, cx)),
                ),
            ),
    );

    v_flex()
        .w_full()
        .child(columns(main_window, charts, cx))
        .into_any_element()
}
