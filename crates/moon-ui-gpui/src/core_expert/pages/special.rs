//! Moonbot's "Специальные" page, control for control — and live where the wire reaches.
//!
//! Four collapsible sections, exactly as that dialog splits them: the engine, Remote, System and
//! the hang watchdog. `moon_core::feed::SpecialSettings` carries what is live — the engine's own
//! switches, its logging and its screenshot rules, out of `trading` and its `send_shots_config`
//! sub-record.
//!
//! What stays disabled, and why. The Remote block's own identity — the bot token, its PIN and the
//! UDP password, and the control VDS address the watchdog below it carries — is outside the
//! safe-share subset altogether, which is why what IS live inside Remote is only the screenshot
//! rules and the multi-command switch. The iceberg pair belongs to `GeneralSettings`, which the
//! compact popup edits: one wire field belongs to one area, or a write from either surface would
//! put the other's frozen copy back. A few rows have no wire field that means what their caption
//! says — "Не проверять лимиты позиции" is the clearest, whose nearest neighbour
//! `trading.free_position_check` is documented as CLOSING orphaned positions and travels on the
//! compact `ClientSettings` route besides. And one row is held back by this window rather than by
//! the wire: see "No trades on markets" below.
//!
//! Four fields of the `trading.orders_control` block behind these rows stay out.
//!
//! `sign_orders` is marked on the wire as a mirror of `ClientSettingsCommand::sign_orders`, so it
//! travels on the compact channel too and writing it here would set two routes fighting over one
//! field. `min_price` and `max_time` are this core's OWN watchdog thresholds — the snapshot carries
//! no follower list, so the "Price, %" and "Time, s" columns Moonbot shows in its table are a
//! watcher's view of other bots, not these — and no row of the worker-bot block sets them here.
//!
//! And `h_pos_control` ("hanging-position detection"), which has no caption of its own: the single
//! switch in the worker-bot block says "следить за ОРДЕРАМИ", which is `orders_control.active`, and
//! binding one checkbox to two flags would turn a feature on and off that nobody named. The cost is
//! stated rather than hidden: "Report to Telegram" and "AutoSell" below it act only while hanging-
//! position detection is on in the core, and this page cannot set it.
//!
//! The follower table at the bottom is drawn as an empty frame with its columns: the terminal has
//! no rows to put in it, and inventing any would state a fleet this window has not read.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::shell::editors::EditorStore;
use crate::shell::parse_num;

use super::super::CoreExpertView;
use super::super::widgets::{
    action, caption, dropdown, field, flag, hint, link, list_box, num, slider, text_block,
};

/// The writes of the rows this page draws but the snapshot does not carry.
const DEAD_TEXT: fn(&mut CoreConfig, &str) = |_, _| {};
const DEAD_NUM: fn(&mut CoreConfig, f32) = |_, _| {};

/// Ranges of the live sliders. Wider than Moonbot's own where the protocol states none, because the
/// seeded value is clamped into them for display: too narrow a range would show a thumb that
/// disagrees with the number this page would send.
const PCT: (f32, f32, f32) = (0.0, 100.0, 1.0);
const LOG_LEVEL: (f32, f32, f32) = (0.0, 5.0, 1.0);
const LOG_DAYS: (f32, f32, f32) = (0.0, 365.0, 1.0);
const CHART_MINUTES: (f32, f32, f32) = (0.0, 1440.0, 1.0);
/// The iceberg slice is a FRACTION of the order — the wire's own default is 0.1 — and the only one
/// of these it carries as a float. A percentage range would pin every real value at the far left.
const ICEBERG_STEP: (f32, f32, f32) = (0.0, 1.0, 0.01);

/// Print an amount so that reading it back yields the same number — Rust's default `f64` formatting
/// is the shortest text that round-trips, and the box parses whatever it shows.
fn fmt_amount(v: f64) -> String {
    format!("{v}")
}

/// Stage a whole count, or refuse the text.
///
/// Refused rather than clamped, the rule the boxes on THIS page follow: clamping would leave the
/// typed text on screen while OK carried a different number. (The AutoStart page still saturates
/// through `as i32`; this is the newer rule, not a universal one.)
///
/// `floor` is zero everywhere it is used, and deliberately not more: the wire's own defaults include
/// a `price_scale` of 0, so a floor picked from what "a trader can have meant" would refuse a value
/// the core itself ships with. What is refused is a negative count and a number outside `i32` —
/// neither is a setting, and `send_core_config` bounds nothing but the leverage on the way out.
fn stage_count(t: &str, floor: i32) -> Option<i32> {
    let v = parse_num(t)?.round();
    // Out of range is refused rather than saturated: `as` would turn a typo into `i32::MAX`, which
    // the core stores without objection.
    (v >= f64::from(floor) && v <= f64::from(i32::MAX)).then_some(v as i32)
}

/// One checkbox row the snapshot carries, staging into the page.
fn live(
    id: &'static str,
    key: &'static str,
    checked: bool,
    view: &Entity<CoreExpertView>,
    set: fn(&mut CoreConfig, bool),
) -> impl IntoElement {
    flag(id, t!(key).to_string(), checked, true, view, set)
}

/// Range of the one slider left dead, resembling Moonbot's on a control that writes nothing.
const DEAD_MINUTES: (f32, f32, f32) = (0.0, 240.0, 1.0);

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
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    let sp = &draft.special;
    vec![
        (
            "exp-sp-bnb-min",
            fmt_amount(sp.auto_buy_bnb_level),
            (|d, t| {
                if let Some(v) = parse_num(t) {
                    d.special.auto_buy_bnb_level = v;
                }
            }) as fn(&mut CoreConfig, &str),
        ),
        (
            "exp-sp-bnb-buy",
            fmt_amount(sp.auto_buy_bnb_volume),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.special.auto_buy_bnb_volume = v;
                }
            },
        ),
        ("exp-sp-api-ip", String::new(), DEAD_TEXT),
        ("exp-sp-stream-ip", String::new(), DEAD_TEXT),
        (
            "exp-sp-no-trades",
            sp.no_trades_markets_text.clone(),
            |d, t| d.special.no_trades_markets_text = t.to_string(),
        ),
        ("exp-sp-bot-token", String::new(), DEAD_TEXT),
        ("exp-sp-pin", String::new(), DEAD_TEXT),
        ("exp-sp-bot-name", String::new(), DEAD_TEXT),
        ("exp-sp-profit-usd", sp.profit_abs.to_string(), |d, t| {
            if let Some(v) = stage_count(t, 0) {
                d.special.profit_abs = v;
            }
        }),
        ("exp-sp-profit-pct", sp.profit_pers.to_string(), |d, t| {
            if let Some(v) = stage_count(t, 0) {
                d.special.profit_pers = v;
            }
        }),
        (
            "exp-sp-profit-hour",
            sp.profit_session.to_string(),
            |d, t| {
                if let Some(v) = stage_count(t, 0) {
                    d.special.profit_session = v;
                }
            },
        ),
        ("exp-sp-time-axis", sp.time_scale.to_string(), |d, t| {
            if let Some(v) = stage_count(t, 0) {
                d.special.time_scale = v;
            }
        }),
        ("exp-sp-price-axis", sp.price_scale.to_string(), |d, t| {
            if let Some(v) = stage_count(t, 0) {
                d.special.price_scale = v;
            }
        }),
        ("exp-sp-udp-port", String::new(), DEAD_TEXT),
        ("exp-sp-udp-pass", String::new(), DEAD_TEXT),
        ("exp-sp-max-orders", sp.max_orders.to_string(), |d, t| {
            if let Some(v) = stage_count(t, 0) {
                d.special.max_orders = v;
            }
        }),
        ("exp-sp-listen-port", String::new(), DEAD_TEXT),
        ("exp-sp-vds-ip", String::new(), DEAD_TEXT),
        (
            "exp-sp-skip-balances",
            sp.h_pos_black_list_text.clone(),
            |d, t| {
                // Stripped, not refused: this box holds a one-line list, and moonui's single-line
                // paste drops a newline but keeps a lone carriage return — which would reach the
                // core inside a ticker name.
                d.special.h_pos_black_list_text = t.replace(['\r', '\n'], String::new().as_str());
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
    let sp = &draft.special;
    vec![
        (
            "exp-sp-iceberg-step",
            ICEBERG_STEP,
            sp.iceberg_step as f32,
            // Rounded to the track's own step: 0.01 has no exact `f32`, so the raw widening
            // would put 0.07000000029802322 in the caption and on the wire for a thumb the trader
            // dropped on 0.07.
            (|d, v| d.special.iceberg_step = (f64::from(v) * 100.0).round() / 100.0)
                as fn(&mut CoreConfig, f32),
            None,
        ),
        (
            "exp-sp-sell-x2",
            PCT,
            sp.sell_x2_level as f32,
            |d, v| d.special.sell_x2_level = v.round() as i32,
            None,
        ),
        (
            "exp-sp-log-level",
            LOG_LEVEL,
            sp.log_level as f32,
            |d, v| d.special.log_level = v.round() as i32,
            None,
        ),
        (
            "exp-sp-log-days",
            LOG_DAYS,
            sp.auto_delete_logs as f32,
            |d, v| d.special.auto_delete_logs = v.round() as i32,
            None,
        ),
        (
            "exp-sp-chart-idle",
            CHART_MINUTES,
            sp.chart_clean_up_time as f32,
            |d, v| d.special.chart_clean_up_time = v.round() as i32,
            None,
        ),
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
    draft: &CoreConfig,
    open: SpecialSection,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 6.0);
    let sp = &draft.special;

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
                                    .child(live(
                                        "exp-sp-replacing",
                                        "core_expert.sp_ignore_replacing",
                                        sp.ignore_replacing_bug,
                                        view,
                                        |d, on| d.special.ignore_replacing_bug = on,
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
                                    .child(live(
                                        "exp-sp-auto-lev",
                                        "core_expert.sp_auto_leverage",
                                        sp.auto_lower_lev,
                                        view,
                                        |d, on| d.special.auto_lower_lev = on,
                                    ))
                                    .child(live(
                                        "exp-sp-close-zero",
                                        "core_expert.sp_auto_close_zero",
                                        sp.auto_close_zero_pos,
                                        view,
                                        |d, on| d.special.auto_close_zero_pos = on,
                                    )),
                            )
                            .child(live(
                                "exp-sp-ws-api",
                                "core_expert.sp_websocket_api",
                                sp.use_websocket_api,
                                view,
                                |d, on| d.special.use_websocket_api = on,
                            )),
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
                            .child(live(
                                "exp-sp-book-ticker",
                                "core_expert.sp_book_ticker",
                                sp.use_book_ticker,
                                view,
                                |d, on| d.special.use_book_ticker = on,
                            ))
                            .child(live(
                                "exp-sp-random-pct",
                                "core_expert.sp_random_percent",
                                sp.random_price,
                                view,
                                |d, on| d.special.random_price = on,
                            ))
                            .child(live(
                                "exp-sp-weighted",
                                "core_expert.sp_weighted_mavg",
                                sp.m_avg_use_vol_weight,
                                view,
                                |d, on| d.special.m_avg_use_vol_weight = on,
                            ))
                            .child(live(
                                "exp-sp-reduce",
                                "core_expert.sp_auto_reduce",
                                sp.auto_reduce_order,
                                view,
                                |d, on| d.special.auto_reduce_order = on,
                            ))
                            .child(row("exp-sp-old-coins", "core_expert.sp_old_as_new", view)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(live(
                                "exp-sp-correct-price",
                                "core_expert.sp_correct_price",
                                sp.correct_order_price,
                                view,
                                |d, on| d.special.correct_order_price = on,
                            ))
                            .child(live(
                                "exp-sp-liq-control",
                                "core_expert.sp_liquidation_control",
                                sp.liq_control,
                                view,
                                |d, on| d.special.liq_control = on,
                            ))
                            .child(live(
                                "exp-sp-bnb",
                                "core_expert.sp_auto_buy_bnb",
                                sp.auto_buy_bnb,
                                view,
                                |d, on| d.special.auto_buy_bnb = on,
                            ))
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
                                            .children(num(store, "exp-sp-bnb-min", 88.0, true, cx)),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(design::ui_px(cx, 2.0))
                                            .child(hint(
                                                t!("core_expert.sp_bnb_buy").to_string(),
                                                p,
                                                cx,
                                            ))
                                            .children(num(store, "exp-sp-bnb-buy", 88.0, true, cx)),
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
                                t!(
                                    "core_expert.sp_iceberg_step",
                                    v = fmt_amount(sp.iceberg_step)
                                )
                                .to_string(),
                                true,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-iceberg-step", true)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!("core_expert.sp_sell_x2", v = sp.sell_x2_level.to_string())
                                    .to_string(),
                                true,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-sell-x2", true)),
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
                    // Shown, not edited, and not shown WHOLE: the wire holds this list one ticker
                    // per line, and a single-line box neither renders the second line nor could
                    // keep it — an edit here would replace the core's whole list with whatever
                    // fitted on one line. The value still round-trips untouched; giving this row a
                    // multi-line control is what would make it editable.
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
                    .child(live(
                        "exp-sp-multiline",
                        "core_expert.sp_multiline_commands",
                        sp.multi_commands,
                        view,
                        |d, on| d.special.multi_commands = on,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(live(
                        "exp-sp-send-shots",
                        "core_expert.sp_send_shots",
                        sp.send_shots,
                        view,
                        |d, on| d.special.send_shots = on,
                    ))
                    .child(live(
                        "exp-sp-send-public",
                        "core_expert.sp_send_public",
                        sp.send_public,
                        view,
                        |d, on| d.special.send_public = on,
                    ))
                    .child(live(
                        "exp-sp-send-negative",
                        "core_expert.sp_send_negative",
                        sp.send_negative,
                        view,
                        |d, on| d.special.send_negative = on,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(caption(
                        t!("core_expert.sp_if_profit_usd").to_string(),
                        true,
                        p,
                        cx,
                    ))
                    .children(num(store, "exp-sp-profit-usd", 72.0, true, cx))
                    .child(caption(
                        t!("core_expert.sp_or_profit_pct").to_string(),
                        true,
                        p,
                        cx,
                    ))
                    .children(num(store, "exp-sp-profit-pct", 64.0, true, cx))
                    .child(caption(
                        t!("core_expert.sp_or_profit_hour").to_string(),
                        true,
                        p,
                        cx,
                    ))
                    .children(num(store, "exp-sp-profit-hour", 72.0, true, cx)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(caption(
                        t!("core_expert.sp_time_axis").to_string(),
                        true,
                        p,
                        cx,
                    ))
                    .children(num(store, "exp-sp-time-axis", 72.0, true, cx))
                    .child(caption(
                        t!("core_expert.sp_price_axis").to_string(),
                        true,
                        p,
                        cx,
                    ))
                    .children(num(store, "exp-sp-price-axis", 72.0, true, cx))
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
                                t!("core_expert.sp_log_level", v = sp.log_level.to_string())
                                    .to_string(),
                                true,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-log-level", true))
                            .child(caption(
                                t!(
                                    "core_expert.sp_chart_idle",
                                    v = sp.chart_clean_up_time.to_string()
                                )
                                .to_string(),
                                true,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-chart-idle", true)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(caption(
                                t!(
                                    "core_expert.sp_log_days",
                                    v = sp.auto_delete_logs.to_string()
                                )
                                .to_string(),
                                true,
                                p,
                                cx,
                            ))
                            .children(slider(store, "exp-sp-log-days", true))
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
                    .child(live(
                        "exp-sp-unlimited",
                        "core_expert.sp_unlimited_orders",
                        sp.unlimited_orders,
                        view,
                        |d, on| d.special.unlimited_orders = on,
                    ))
                    .child(caption("Max Orders".to_string(), true, p, cx))
                    .children(num(store, "exp-sp-max-orders", 64.0, true, cx))
                    .child(caption("Listen UDP port".to_string(), false, p, cx))
                    .children(num(store, "exp-sp-listen-port", 72.0, false, cx)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 16.0))
                    .child(live(
                        "exp-sp-no-protection",
                        "core_expert.sp_turn_off_protection",
                        sp.ignore_protection > 0,
                        view,
                        // A LEVEL under a checkbox: turning it off is unambiguous, turning it on
                        // must not overwrite a level the core already holds — only supply one when
                        // there is none. A negative reads as protection ON, because the wire states
                        // a meaning for zero and for a bypass level, not for less than zero.
                        |d, on| {
                            d.special.ignore_protection = match (on, d.special.ignore_protection) {
                                (false, _) => 0,
                                (true, held) if held > 0 => held,
                                (true, _) => 1,
                            };
                        },
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
                    .child(live(
                        "exp-sp-watch-orders",
                        "core_expert.sp_watch_orders",
                        sp.orders_control_active,
                        view,
                        |d, on| d.special.orders_control_active = on,
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
                    .child(live(
                        "exp-sp-report-tg",
                        "core_expert.sp_report_to_telegram",
                        sp.h_pos_report,
                        view,
                        |d, on| d.special.h_pos_report = on,
                    ))
                    .child(live(
                        "exp-sp-autosell",
                        "core_expert.sp_autosell",
                        sp.h_pos_auto_sell,
                        view,
                        |d, on| d.special.h_pos_auto_sell = on,
                    ))
                    .child(caption(
                        t!("core_expert.sp_skip_balances").to_string(),
                        true,
                        p,
                        cx,
                    ))
                    // Editable only while the value really is ONE line. Moonbot's own dialog
                    // holds this list comma-separated on a single line and its documentation says
                    // so, which is why this box may edit it at all — but a core that somehow holds
                    // a newline here would have the rest of its list eaten by a control that cannot
                    // render it, so such a value is shown and not touched.
                    .child(div().flex_1().min_w_0().children(field(
                        store,
                        "exp-sp-skip-balances",
                        !sp.h_pos_black_list_text.contains(['\r', '\n']),
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
