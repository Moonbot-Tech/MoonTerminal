//! Moonbot's "Специальные" page, control for control — four collapsible sections, exactly as that
//! dialog splits them: the engine, Remote, System and the hang watchdog.
//!
//! Nothing here is live. Most of it IS on the wire (`trading.orders_control`, the engine flags,
//! `trading.moonbot_config`), but `moon_core::feed::CoreConfig` projects none of it and
//! `ExpertTab::add_sections` would not carry it back. The Remote block and the hang watchdog
//! go further: safe-share excludes them outright, since they carry the bot token, the UDP password
//! and the control VDS address.
//!
//! The follower table at the bottom is drawn as an empty frame with its columns: the terminal has
//! no rows to put in it, and inventing any would state a fleet this window has not read.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::shell::editors::EditorStore;

use super::super::CoreExpertView;
use super::super::widgets::{
    action, caption, dropdown, field, flag, hint, link, list_box, num, slider, text_block,
};

/// Nothing on this page reaches the draft.
const DEAD_TEXT: fn(&mut CoreConfig, &str) = |_, _| {};
const DEAD_NUM: fn(&mut CoreConfig, f32) = |_, _| {};

/// Ranges that resemble Moonbot's on controls that write nothing.
const DEAD_PCT: (f32, f32, f32) = (0.0, 100.0, 1.0);
const DEAD_LEVEL: (f32, f32, f32) = (0.0, 5.0, 1.0);
const DEAD_MINUTES: (f32, f32, f32) = (0.0, 240.0, 1.0);
const DEAD_DAYS: (f32, f32, f32) = (0.0, 60.0, 1.0);

/// Value shown where Moonbot prints a number this terminal has not read.
const NO_VALUE: &str = "—";

/// Moonbot's four sections on this page, in its own order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SpecialSection {
    #[default]
    Engine,
    Remote,
    System,
    Watchdog,
}

impl SpecialSection {
    pub(crate) const ALL: [SpecialSection; 4] =
        [Self::Engine, Self::Remote, Self::System, Self::Watchdog];

    /// Localized header, in Moonbot's own wording.
    fn title(self) -> String {
        match self {
            Self::Engine => t!("core_expert.sp_engine").to_string(),
            Self::Remote => "Remote".to_string(),
            Self::System => "System".to_string(),
            Self::Watchdog => t!("core_expert.sp_watchdog").to_string(),
        }
    }
}

/// See [`super::field_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    _draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    vec![
        ("exp-sp-bnb-min", String::new(), DEAD_TEXT),
        ("exp-sp-bnb-buy", String::new(), DEAD_TEXT),
        ("exp-sp-api-ip", String::new(), DEAD_TEXT),
        ("exp-sp-stream-ip", String::new(), DEAD_TEXT),
        ("exp-sp-no-trades", String::new(), DEAD_TEXT),
        ("exp-sp-bot-token", String::new(), DEAD_TEXT),
        ("exp-sp-pin", String::new(), DEAD_TEXT),
        ("exp-sp-bot-name", String::new(), DEAD_TEXT),
        ("exp-sp-profit-usd", String::new(), DEAD_TEXT),
        ("exp-sp-profit-pct", String::new(), DEAD_TEXT),
        ("exp-sp-profit-hour", String::new(), DEAD_TEXT),
        ("exp-sp-time-axis", String::new(), DEAD_TEXT),
        ("exp-sp-price-axis", String::new(), DEAD_TEXT),
        ("exp-sp-udp-port", String::new(), DEAD_TEXT),
        ("exp-sp-udp-pass", String::new(), DEAD_TEXT),
        ("exp-sp-max-orders", String::new(), DEAD_TEXT),
        ("exp-sp-listen-port", String::new(), DEAD_TEXT),
        ("exp-sp-vds-ip", String::new(), DEAD_TEXT),
        ("exp-sp-skip-balances", String::new(), DEAD_TEXT),
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
        ("exp-sp-iceberg-step", DEAD_PCT, 0.0, DEAD_NUM, None),
        ("exp-sp-sell-x2", DEAD_PCT, 0.0, DEAD_NUM, None),
        ("exp-sp-log-level", DEAD_LEVEL, 0.0, DEAD_NUM, None),
        ("exp-sp-log-days", DEAD_DAYS, 0.0, DEAD_NUM, None),
        ("exp-sp-chart-idle", DEAD_MINUTES, 0.0, DEAD_NUM, None),
        ("exp-sp-chart-report", DEAD_MINUTES, 0.0, DEAD_NUM, None),
    ]
}

/// One dead checkbox row, which this page has three dozen of.
fn row(id: &'static str, key: &'static str, view: &Entity<CoreExpertView>) -> impl IntoElement {
    flag(id, t!(key).to_string(), false, false, view, |_, _| {})
}

/// A section header that opens and closes its body, as Moonbot's own bar does.
fn header(
    section: SpecialSection,
    open: bool,
    view: &Entity<CoreExpertView>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let view = view.clone();
    h_flex()
        .id(SharedString::from(format!("exp-sp-head-{section:?}")))
        .w_full()
        .items_center()
        .justify_center()
        .relative()
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 4.0))
        .rounded(design::r_button(cx))
        .bg(rgb(p.shell_high))
        .border_1()
        .border_color(rgb(p.border))
        .cursor_pointer()
        .on_click(move |_, _window, app| {
            view.update(app, |this, cx| this.set_special_section(section, cx));
        })
        .child(div().absolute().left(design::ui_px(cx, 8.0)).child(caption(
            if open {
                "˄".to_string()
            } else {
                "˅".to_string()
            },
            false,
            p,
            cx,
        )))
        .child(caption(section.title(), true, p, cx))
}

/// Build the page.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    open: SpecialSection,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 6.0);

    // --- "Настройки движка" ------------------------------------------------------------------
    let engine = || {
        v_flex()
            .w_full()
            .gap(gap)
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(design::ui_px(cx, 16.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(row(
                                "exp-sp-iceberg-buys",
                                "core_expert.sp_iceberg_buys",
                                view,
                            ))
                            .child(row(
                                "exp-sp-no-pos-limit",
                                "core_expert.sp_no_position_limit",
                                view,
                            ))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(design::ui_px(cx, 8.0))
                                    .child(row(
                                        "exp-sp-replacing",
                                        "core_expert.sp_ignore_replacing",
                                        view,
                                    ))
                                    .child(link(
                                        "exp-sp-help-1",
                                        t!("core_expert.sp_help").to_string(),
                                        false,
                                    )),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(design::ui_px(cx, 8.0))
                                    .child(row("exp-sp-quant", "core_expert.sp_quantitative", view))
                                    .child(link(
                                        "exp-sp-help-2",
                                        t!("core_expert.sp_help").to_string(),
                                        false,
                                    )),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(design::ui_px(cx, 12.0))
                                    .child(row(
                                        "exp-sp-auto-lev",
                                        "core_expert.sp_auto_leverage",
                                        view,
                                    ))
                                    .child(row(
                                        "exp-sp-close-zero",
                                        "core_expert.sp_auto_close_zero",
                                        view,
                                    )),
                            )
                            .child(row("exp-sp-ws-api", "core_expert.sp_websocket_api", view)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(row(
                                "exp-sp-iceberg-sells",
                                "core_expert.sp_iceberg_sells",
                                view,
                            ))
                            .child(row(
                                "exp-sp-book-ticker",
                                "core_expert.sp_book_ticker",
                                view,
                            ))
                            .child(row(
                                "exp-sp-random-pct",
                                "core_expert.sp_random_percent",
                                view,
                            ))
                            .child(row("exp-sp-weighted", "core_expert.sp_weighted_mavg", view))
                            .child(row("exp-sp-reduce", "core_expert.sp_auto_reduce", view))
                            .child(row("exp-sp-old-coins", "core_expert.sp_old_as_new", view)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(row(
                                "exp-sp-correct-price",
                                "core_expert.sp_correct_price",
                                view,
                            ))
                            .child(row(
                                "exp-sp-liq-control",
                                "core_expert.sp_liquidation_control",
                                view,
                            ))
                            .child(row("exp-sp-bnb", "core_expert.sp_auto_buy_bnb", view))
                            .child(
                                h_flex()
                                    .items_start()
                                    .gap(design::ui_px(cx, 10.0))
                                    .child(
                                        v_flex()
                                            .gap(design::ui_px(cx, 2.0))
                                            .child(hint(
                                                t!("core_expert.sp_bnb_min").to_string(),
                                                p,
                                                cx,
                                            ))
                                            .children(num(
                                                store,
                                                "exp-sp-bnb-min",
                                                88.0,
                                                false,
                                                cx,
                                            )),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(design::ui_px(cx, 2.0))
                                            .child(hint(
                                                t!("core_expert.sp_bnb_buy").to_string(),
                                                p,
                                                cx,
                                            ))
                                            .children(num(
                                                store,
                                                "exp-sp-bnb-buy",
                                                88.0,
                                                false,
                                                cx,
                                            )),
                                    ),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(design::ui_px(cx, 16.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!("core_expert.sp_iceberg_step", v = NO_VALUE).to_string(),
                                false,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-iceberg-step", false)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!("core_expert.sp_sell_x2", v = NO_VALUE).to_string(),
                                false,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-sell-x2", false)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_end()
                    .gap(design::ui_px(cx, 10.0))
                    .child(
                        v_flex()
                            .gap(design::ui_px(cx, 2.0))
                            .child(hint(t!("core_expert.sp_connection").to_string(), p, cx))
                            .child(dropdown("exp-sp-connection", NO_VALUE.to_string(), false)),
                    )
                    .child(
                        v_flex()
                            .gap(design::ui_px(cx, 2.0))
                            .child(row("exp-sp-custom-ip", "core_expert.sp_custom_ip", view))
                            .child(row("exp-sp-auto-dns", "core_expert.sp_auto_dns", view)),
                    )
                    .child(
                        v_flex()
                            .gap(design::ui_px(cx, 2.0))
                            .child(hint("api.binance.com IP".to_string(), p, cx))
                            .children(num(store, "exp-sp-api-ip", 128.0, false, cx)),
                    )
                    .child(
                        v_flex()
                            .gap(design::ui_px(cx, 2.0))
                            .child(hint("stream.binance.com IP".to_string(), p, cx))
                            .children(num(store, "exp-sp-stream-ip", 128.0, false, cx)),
                    )
                    .child(dropdown("exp-sp-auth", NO_VALUE.to_string(), false)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(caption(
                        t!("core_expert.sp_connection_state", v = NO_VALUE).to_string(),
                        false,
                        p,
                        cx,
                    ))
                    .child(link(
                        "exp-sp-help-3",
                        t!("core_expert.sp_help_caps").to_string(),
                        false,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(caption(
                        t!("core_expert.sp_no_trades").to_string(),
                        false,
                        p,
                        cx,
                    ))
                    .child(div().flex_1().min_w_0().children(field(
                        store,
                        "exp-sp-no-trades",
                        false,
                    ))),
            )
    };

    // --- "Remote" -----------------------------------------------------------------------------
    let remote = || {
        v_flex()
            .w_full()
            .gap(gap)
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(design::ui_px(cx, 10.0))
                    .child(action(
                        "exp-sp-add-bot",
                        "Add @TMoonBot to your channel".to_string(),
                        false,
                    ))
                    .child(action(
                        "exp-sp-gen-pin",
                        "Generate PIN code".to_string(),
                        false,
                    ))
                    .child(action(
                        "exp-sp-reset-channel",
                        "Reset channel".to_string(),
                        false,
                    ))
                    .child(
                        v_flex()
                            .gap(design::ui_px(cx, 2.0))
                            .child(hint(t!("core_expert.sp_my_channel").to_string(), p, cx))
                            .child(caption(NO_VALUE.to_string(), false, p, cx)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(design::ui_px(cx, 10.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(design::ui_px(cx, 2.0))
                            .child(row("exp-sp-own-bot", "core_expert.sp_own_bot_id", view))
                            .children(field(store, "exp-sp-bot-token", false)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(design::ui_px(cx, 2.0))
                            .child(hint(t!("core_expert.sp_type_pin").to_string(), p, cx))
                            .children(field(store, "exp-sp-pin", false)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(design::ui_px(cx, 2.0))
                            .child(hint(t!("core_expert.sp_this_bot_name").to_string(), p, cx))
                            .children(field(store, "exp-sp-bot-name", false)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .child(row(
                        "exp-sp-send-reports",
                        "core_expert.sp_send_trade_reports",
                        view,
                    ))
                    .child(dropdown(
                        "exp-sp-system-reports",
                        NO_VALUE.to_string(),
                        false,
                    ))
                    .child(row(
                        "exp-sp-multiline",
                        "core_expert.sp_multiline_commands",
                        view,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(row("exp-sp-send-shots", "core_expert.sp_send_shots", view))
                    .child(row(
                        "exp-sp-send-public",
                        "core_expert.sp_send_public",
                        view,
                    ))
                    .child(row(
                        "exp-sp-send-negative",
                        "core_expert.sp_send_negative",
                        view,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(caption(
                        t!("core_expert.sp_if_profit_usd").to_string(),
                        false,
                        p,
                        cx,
                    ))
                    .children(num(store, "exp-sp-profit-usd", 72.0, false, cx))
                    .child(caption(
                        t!("core_expert.sp_or_profit_pct").to_string(),
                        false,
                        p,
                        cx,
                    ))
                    .children(num(store, "exp-sp-profit-pct", 64.0, false, cx))
                    .child(caption(
                        t!("core_expert.sp_or_profit_hour").to_string(),
                        false,
                        p,
                        cx,
                    ))
                    .children(num(store, "exp-sp-profit-hour", 72.0, false, cx)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(caption(
                        t!("core_expert.sp_time_axis").to_string(),
                        false,
                        p,
                        cx,
                    ))
                    .children(num(store, "exp-sp-time-axis", 72.0, false, cx))
                    .child(caption(
                        t!("core_expert.sp_price_axis").to_string(),
                        false,
                        p,
                        cx,
                    ))
                    .children(num(store, "exp-sp-price-axis", 72.0, false, cx))
                    .child(div().flex_1())
                    .child(
                        v_flex()
                            .gap(design::ui_px(cx, 2.0))
                            .child(hint("UDP Commands Port / Pass".to_string(), p, cx))
                            .child(
                                h_flex()
                                    .gap(design::ui_px(cx, 6.0))
                                    .children(num(store, "exp-sp-udp-port", 72.0, false, cx))
                                    .children(num(store, "exp-sp-udp-pass", 120.0, false, cx)),
                            ),
                    ),
            )
    };

    // --- "System" -----------------------------------------------------------------------------
    let system = || {
        v_flex()
            .w_full()
            .gap(gap)
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(design::ui_px(cx, 16.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!("core_expert.sp_log_level", v = NO_VALUE).to_string(),
                                false,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-log-level", false))
                            .child(caption(
                                t!("core_expert.sp_chart_idle", v = NO_VALUE).to_string(),
                                false,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-chart-idle", false)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!("core_expert.sp_log_days", v = NO_VALUE).to_string(),
                                false,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-log-days", false))
                            .child(caption(
                                t!("core_expert.sp_chart_report", v = NO_VALUE).to_string(),
                                false,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-chart-report", false)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 16.0))
                    .child(row("exp-sp-debug", "core_expert.sp_extended_debug", view))
                    .child(row(
                        "exp-sp-market-export",
                        "core_expert.sp_market_export",
                        view,
                    ))
                    .child(row("exp-sp-udp-export", "core_expert.sp_udp_export", view)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(row(
                        "exp-sp-unlimited",
                        "core_expert.sp_unlimited_orders",
                        view,
                    ))
                    .child(caption("Max Orders".to_string(), false, p, cx))
                    .children(num(store, "exp-sp-max-orders", 64.0, false, cx))
                    .child(caption("Listen UDP port".to_string(), false, p, cx))
                    .children(num(store, "exp-sp-listen-port", 72.0, false, cx)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 16.0))
                    .child(row(
                        "exp-sp-no-protection",
                        "core_expert.sp_turn_off_protection",
                        view,
                    ))
                    .child(row("exp-sp-beta", "core_expert.sp_accept_beta", view)),
            )
    };

    // --- "Защита от зависаний" -----------------------------------------------------------------
    let watchdog = || {
        v_flex()
            .w_full()
            .gap(gap)
            .child(caption(
                t!("core_expert.sp_worker_bot").to_string(),
                false,
                p,
                cx,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .child(row(
                        "exp-sp-watch-orders",
                        "core_expert.sp_watch_orders",
                        view,
                    ))
                    .child(caption("Control VDS IP".to_string(), false, p, cx))
                    .children(num(store, "exp-sp-vds-ip", 128.0, false, cx))
                    .child(div().flex_1())
                    .child(link(
                        "exp-sp-watchdog-help",
                        t!("core_expert.sp_help").to_string(),
                        false,
                    )),
            )
            .child(caption(
                t!("core_expert.sp_follower_bot").to_string(),
                false,
                p,
                cx,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .child(row(
                        "exp-sp-report-tg",
                        "core_expert.sp_report_to_telegram",
                        view,
                    ))
                    .child(row("exp-sp-autosell", "core_expert.sp_autosell", view))
                    .child(caption(
                        t!("core_expert.sp_skip_balances").to_string(),
                        false,
                        p,
                        cx,
                    ))
                    .child(div().flex_1().min_w_0().children(field(
                        store,
                        "exp-sp-skip-balances",
                        false,
                    ))),
            )
            .child(text_block(
                t!("core_expert.sp_open_udp_port").to_string(),
                p.text_soft,
                false,
                cx,
            ))
            .child(list_box(
                "exp-sp-followers",
                Vec::new(),
                t!("core_expert.sp_followers_empty").to_string(),
                p,
                cx,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(action("exp-sp-add-follower", "+".to_string(), false))
                    .child(action("exp-sp-del-follower", "−".to_string(), false))
                    .child(action(
                        "exp-sp-cancel-buys",
                        "Cancel buys".to_string(),
                        false,
                    ))
                    .child(div().flex_1())
                    .child(caption(
                        t!("core_expert.sp_udp_server", v = NO_VALUE).to_string(),
                        false,
                        p,
                        cx,
                    ))
                    .child(action("exp-sp-apply", "Apply".to_string(), false)),
            )
    };

    // Only the open section is BUILT, not merely hidden: Moonbot shows one at a time, and a page
    // that assembled all four every frame would pay for three nobody is looking at.
    let open_body: AnyElement = match open {
        SpecialSection::Engine => engine().into_any_element(),
        SpecialSection::Remote => remote().into_any_element(),
        SpecialSection::System => system().into_any_element(),
        SpecialSection::Watchdog => watchdog().into_any_element(),
    };
    let mut open_body = Some(open_body);

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 6.0))
        .children(SpecialSection::ALL.into_iter().map(|section| {
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(header(section, section == open, view, p, cx))
                .children((section == open).then(|| open_body.take()).flatten())
        }))
        .into_any_element()
}
