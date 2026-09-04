//! Moonbot's "Интерфейс" page, control for control.
//!
//! The longest page of the dialog, and entirely dead here. Its values are Moonbot's OWN window and
//! chart appearance — which panels it opens, how it paints its price line, what its order-book
//! zones look like. They travel in the safe-share `visual` section, but this terminal draws none of
//! them: it has its own chart, its own panels and its own theme, and a switch here could only
//! change what the OTHER program looks like.
//!
//! It is reproduced anyway, so a trader reading the two dialogs side by side finds every row where
//! Moonbot puts it — with the values shown as an em dash, because this window has not read them.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::shell::editors::EditorStore;

use super::super::CoreExpertView;
use super::super::widgets::{
    action, caption, columns, dropdown, flag, group, num, rows, slider, stepper, text_line,
};

/// Nothing on this page reaches the draft.
const DEAD_TEXT: fn(&mut CoreConfig, &str) = |_, _| {};
const DEAD_NUM: fn(&mut CoreConfig, f32) = |_, _| {};

/// Percent ranges for the dead transparency sliders, resembling Moonbot's own.
const DEAD_PCT: (f32, f32, f32) = (0.0, 100.0, 1.0);

/// Value shown where Moonbot prints a number this terminal has not read.
const NO_VALUE: &str = "—";

/// See [`super::field_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    _draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    vec![
        ("exp-int-spread-base", String::new(), DEAD_TEXT),
        ("exp-int-spread-delta", String::new(), DEAD_TEXT),
    ]
}

/// See [`super::slider_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn slider_specs(
    _draft: &CoreConfig,
) -> Vec<(
    &'static str,
    (f32, f32, f32),
    f32,
    fn(&mut CoreConfig, f32),
    Option<&'static str>,
)> {
    vec![
        ("exp-int-pixel-size", DEAD_PCT, 0.0, DEAD_NUM, None),
        ("exp-int-zone-border", DEAD_PCT, 0.0, DEAD_NUM, None),
        ("exp-int-zone-fill", DEAD_PCT, 0.0, DEAD_NUM, None),
        ("exp-int-zone-orders", DEAD_PCT, 0.0, DEAD_NUM, None),
        ("exp-int-panic-zone", DEAD_PCT, 0.0, DEAD_NUM, None),
        ("exp-int-background", DEAD_PCT, 0.0, DEAD_NUM, None),
    ]
}

/// One dead checkbox row, which this page has three dozen of.
fn row(id: &'static str, key: &'static str, view: &Entity<CoreExpertView>) -> impl IntoElement {
    flag(id, t!(key).to_string(), false, false, view, |_, _| {})
}

/// Build the page.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 4.0);

    // --- Left: Moonbot's main window ---------------------------------------------------------
    let main_window = group("exp-int-main", t!("core_expert.int_main_frame").to_string()).child(
        rows(cx)
            .gap(gap)
            .child(row(
                "exp-int-buy-enter",
                "core_expert.int_buy_on_enter",
                view,
            ))
            .child(row(
                "exp-int-log-window",
                "core_expert.int_log_window",
                view,
            ))
            .child(row(
                "exp-int-orders-window",
                "core_expert.int_orders_window",
                view,
            ))
            .child(row("exp-int-to-tray", "core_expert.int_to_tray", view))
            .child(row(
                "exp-int-restore-on-signal",
                "core_expert.int_restore_on_signal",
                view,
            ))
            .child(row(
                "exp-int-hide-forum",
                "core_expert.int_hide_forum",
                view,
            ))
            .child(row(
                "exp-int-scroll-charts",
                "core_expert.int_scroll_charts",
                view,
            ))
            .child(row(
                "exp-int-open-all",
                "core_expert.int_open_all_charts",
                view,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(row(
                        "exp-int-lev-for-tp",
                        "core_expert.int_lev_for_tp",
                        view,
                    ))
                    .child(row(
                        "exp-int-ask-on-exit",
                        "core_expert.int_ask_on_exit",
                        view,
                    )),
            )
            .child(row(
                "exp-int-ignore-ms-sell",
                "core_expert.int_ignore_ms_sell",
                view,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(row(
                        "exp-int-encrypt-reports",
                        "core_expert.int_encrypt_reports",
                        view,
                    ))
                    .child(row(
                        "exp-int-detect-buttons",
                        "core_expert.int_detect_buttons",
                        view,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(row("exp-int-hide-buy", "core_expert.int_hide_buy", view))
                    .child(row("exp-int-hide-demo", "core_expert.int_hide_demo", view)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(row(
                        "exp-int-hide-bonus",
                        "core_expert.int_hide_bonus",
                        view,
                    ))
                    .child(row(
                        "exp-int-hide-candy",
                        "core_expert.int_hide_candy",
                        view,
                    )),
            )
            // Moonbot's three alert sounds, each a switch over a picker and — for the last two — a
            // level. The terminal has its own price-approach alerts; these belong to that window.
            .child(row(
                "exp-int-net-sound",
                "core_expert.int_network_sound",
                view,
            ))
            .child(dropdown(
                "exp-int-net-sound-pick",
                NO_VALUE.to_string(),
                false,
            ))
            .child(row(
                "exp-int-sell-sound",
                "core_expert.int_sell_sound",
                view,
            ))
            .child(
                h_flex()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .child(dropdown(
                        "exp-int-sell-sound-pick",
                        NO_VALUE.to_string(),
                        false,
                    ))
                    .child(stepper("exp-int-sell-sound-level", 0.0, false)),
            )
            .child(row("exp-int-buy-sound", "core_expert.int_buy_sound", view))
            .child(
                h_flex()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .child(dropdown(
                        "exp-int-buy-sound-pick",
                        NO_VALUE.to_string(),
                        false,
                    ))
                    .child(stepper("exp-int-buy-sound-level", 0.0, false)),
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
                            .child(dropdown("exp-int-icons", NO_VALUE.to_string(), false)),
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
                        false,
                        p,
                        cx,
                    ))
                    .child(stepper("exp-int-price-width", 0.0, false)),
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
                            .child(row("exp-int-hints", "core_expert.int_chart_hints", view))
                            .child(row(
                                "exp-int-profit-usd",
                                "core_expert.int_profit_in_usd",
                                view,
                            ))
                            .child(row(
                                "exp-int-label-iceberg",
                                "core_expert.int_label_iceberg",
                                view,
                            )),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(row(
                                "exp-int-label-orders",
                                "core_expert.int_label_orders",
                                view,
                            ))
                            .child(div().pl(design::ui_px(cx, 18.0)).child(row(
                                "exp-int-under-line",
                                "core_expert.int_under_line",
                                view,
                            )))
                            .child(dropdown("exp-int-price-panel", NO_VALUE.to_string(), false)),
                    ),
            )
            .child(caption(
                t!("core_expert.int_zone_transparency").to_string(),
                false,
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
                                t!("core_expert.int_zone_border", v = NO_VALUE).to_string(),
                                false,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-int-zone-border", false)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!("core_expert.int_zone_fill", v = NO_VALUE).to_string(),
                                false,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-int-zone-fill", false)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!("core_expert.int_zone_orders", v = NO_VALUE).to_string(),
                                false,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-int-zone-orders", false))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(design::ui_px(cx, 6.0))
                                    .child(caption(
                                        t!("core_expert.int_line_width").to_string(),
                                        false,
                                        p,
                                        cx,
                                    ))
                                    .child(stepper("exp-int-line-width", 0.0, false)),
                            ),
                    ),
            )
            .child(caption(
                t!("core_expert.int_panic_zone", v = NO_VALUE).to_string(),
                false,
                p,
                cx,
            ))
            .children(slider(store, "exp-int-panic-zone", false))
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
                            .child(row(
                                "exp-int-dbl-panic",
                                "core_expert.int_double_click_panic",
                                view,
                            ))
                            .child(row("exp-int-zones", "core_expert.int_control_zones", view))
                            .child(row(
                                "exp-int-new-on-top",
                                "core_expert.int_new_charts_on_top",
                                view,
                            ))
                            .child(row(
                                "exp-int-update-title",
                                "core_expert.int_update_title",
                                view,
                            ))
                            .child(row(
                                "exp-int-hide-right",
                                "core_expert.int_hide_right_panel",
                                view,
                            ))
                            .child(row(
                                "exp-int-server-charts",
                                "core_expert.int_server_charts",
                                view,
                            ))
                            .child(row(
                                "exp-int-pending-price",
                                "core_expert.int_pending_price",
                                view,
                            ))
                            .child(row(
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
                            .child(row(
                                "exp-int-one-fullscreen",
                                "core_expert.int_one_fullscreen",
                                view,
                            ))
                            .child(row("exp-int-stop-line", "core_expert.int_stop_line", view))
                            .child(row("exp-int-compact", "core_expert.int_compact", view))
                            .child(row(
                                "exp-int-info-right",
                                "core_expert.int_info_right",
                                view,
                            ))
                            .child(row("exp-int-hide-pnl", "core_expert.int_hide_pnl", view))
                            .child(row(
                                "exp-int-scale-tool",
                                "core_expert.int_scale_tool",
                                view,
                            ))
                            .child(row(
                                "exp-int-buttons-memory",
                                "core_expert.int_buttons_memory",
                                view,
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
                        .children(num(store, "exp-int-spread-base", 64.0, false, cx))
                        .child(caption(
                            t!("core_expert.int_spread_hdelta").to_string(),
                            false,
                            p,
                            cx,
                        ))
                        .children(num(store, "exp-int-spread-delta", 64.0, false, cx)),
                ),
            ),
    );

    v_flex()
        .w_full()
        .child(columns(main_window, charts, cx))
        .into_any_element()
}
