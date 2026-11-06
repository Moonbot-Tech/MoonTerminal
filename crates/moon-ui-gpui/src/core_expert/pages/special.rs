//! Moonbot's "Специальные" page, control for control — and live where the wire reaches.
//!
//! Four collapsible sections, exactly as that dialog splits them: the engine, Remote, System and
//! the hang watchdog. `moon_core::feed::SpecialSettings` carries what is live — the engine's own
//! switches, its logging and its screenshot rules, out of `trading` and its `send_shots_config`,
//! `orders_control` and `moonbot_config` sub-records, plus `auto_start_2.show_old_listing`.
//!
//! What stays disabled, and why. The Remote block's own identity is outside the safe-share subset
//! altogether. Its channel binding and outgoing bot — `Generate PIN code`, `Reset channel`,
//! `My Channel`, `I have my own bot ID` and the token — are live on the core's service API instead
//! (#1174, see [`remote`]), with buttons of their own that OK does not carry. What neither the
//! snapshot nor that API carries stays dead: `Add @TMoonBot to your channel` (an action of Moonbot's
//! own machine), `This bot name`, `Слать отчёты о сделках` with its interval selector, and the UDP
//! command port and password.
//!
//! Six more rows, in the engine and System blocks, have no field in the snapshot at all: Custom IP
//! and Auto DNS, Extended Debug Mode, the two exports, and accepting beta versions.
//!
//! The iceberg pair is live too, although its fields belong to `GeneralSettings`, which the compact
//! popup edits as well. Both faces of the gear may write one area: this window lays only its staged
//! fields over each core's latest snapshot, so it never sends a frozen copy, and the expert
//! preference closes the popup, discarding its draft. The one way both stand open is the blacklist
//! reveal, which opens this window on a core other than the popup's; the popup's OK still sends its
//! whole General area, so that overlap is the one every live row of the General page here already
//! has, and these two rows add no new kind of it.
//!
//! And one row is held back by this window rather than by the wire: see "No trades on markets"
//! below.
//!
//! Three rows were dead until moonproto shipped a reference for every shared setting (9b616b9), and
//! each is live now on what that reference settled rather than on a resemblance:
//! "Не проверять лимиты позиции" is `free_position_check`, ticked meaning `true` — the reference
//! and Moonbot's hint describe the same effect; "Old coins as New Listing" (`cbGateOldListing`) is
//! `auto_start_2.show_old_listing`, which the reference now calls a Gate market-list option; and the
//! connection selector is `binance_connection`. `moon_core::feed::SpecialSettings` carries the
//! reasoning per field.
//!
//! The hang watchdog is not drawn from the page at all (#1173): every row of it — "Следить за
//! ордерами этого бота", the control VDS address, the two position-control flags, the exclusion
//! list and the observer table — is the core's observer API (`client.watchdogs()`), with an edit and
//! an Apply of its own, and OK does not carry it. See [`watchdog`] for the section and
//! `core_expert::watchdog` for the window's side of it.

pub(super) mod remote;
mod service;
pub(super) mod watchdog;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::shell::editors::EditorStore;
use crate::shell::parse_num;

use super::super::CoreExpertView;
use super::super::widgets::{
    action, caption, choice_live, dropdown, field, flag_dead, flag_live, hint, link, num, slider,
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
/// The iceberg threshold is a PER CENT of price step, not the fraction of the order the wire's doc
/// claims: Moonbot formats the row as "Ставить Iceberg если шаг цены < %s%%". Its default is 0.1,
/// meaning a tenth of a per cent, and this range — the only float among these — suits that reading
/// as it stood.
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

/// Range of the one slider left dead, resembling Moonbot's on a control that writes nothing.
const DEAD_MINUTES: (f32, f32, f32) = (0.0, 240.0, 1.0);

/// Value shown where Moonbot prints a number this terminal has not read.
const NO_VALUE: &str = "—";

/// Highest REST connection method the wire documents: 0 is Moonbot's "Original", 1..=3 its
/// "Method 2" to "Method 4".
const CONNECTION_METHODS: u8 = 3;

/// The connection selector's items, in Moonbot's order.
///
/// Built per render rather than held: four short labels, and the selector only borrows them.
fn connection_options() -> Vec<(u8, SharedString, SharedString)> {
    (0..=CONNECTION_METHODS)
        .map(|ordinal| {
            let label = if ordinal == 0 {
                t!("core_expert.sp_connection_original").to_string()
            } else {
                t!("core_expert.sp_connection_method", n = ordinal + 1).to_string()
            };
            (
                ordinal,
                SharedString::from(format!("exp-sp-connection-{ordinal}")),
                SharedString::from(label),
            )
        })
        .collect()
}

/// The core's connection method as the selector's ordinal.
///
/// A value no item names — a negative, or one past `u8` — shows as `#255` rather than being
/// clamped onto a real method: `choice_live` prints an unknown ordinal raw, and staging nothing
/// until an item is picked keeps the core's own value on the wire.
fn connection_ordinal(value: i32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

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
        // Facet skin: the bar is a raised control standing off the page.
        .when(design::is_facet(cx), |this| {
            design::facet_surface(this, moon_ui::MoonFacetKind::Raised, cx)
        })
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
///
/// `wd` and `rm` are the hang watchdog's and the Remote block's own contexts, read by the window
/// only while that section is open.
#[allow(clippy::too_many_arguments)]
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    draft: &CoreConfig,
    open: SpecialSection,
    wd: Option<&watchdog::WatchdogCtx<'_>>,
    rm: Option<&remote::RemoteCtx<'_>>,
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
                            .child(flag_live(
                                "exp-sp-iceberg-buys",
                                "core_expert.sp_iceberg_buys",
                                draft.general.buy_iceberg,
                                view,
                                |d, on| d.general.buy_iceberg = on,
                            ))
                            .child(flag_live(
                                "exp-sp-no-pos-limit",
                                "core_expert.sp_no_position_limit",
                                sp.free_position_check,
                                view,
                                |d, on| d.special.free_position_check = on,
                            ))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(design::ui_px(cx, 8.0))
                                    .child(flag_live(
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
                                    .child(flag_live(
                                        "exp-sp-quant",
                                        "core_expert.sp_quantitative",
                                        sp.futures_rules,
                                        view,
                                        |d, on| d.special.futures_rules = on,
                                    ))
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
                                    .child(flag_live(
                                        "exp-sp-auto-lev",
                                        "core_expert.sp_auto_leverage",
                                        sp.auto_lower_lev,
                                        view,
                                        |d, on| d.special.auto_lower_lev = on,
                                    ))
                                    .child(flag_live(
                                        "exp-sp-close-zero",
                                        "core_expert.sp_auto_close_zero",
                                        sp.auto_close_zero_pos,
                                        view,
                                        |d, on| d.special.auto_close_zero_pos = on,
                                    )),
                            )
                            .child(flag_live(
                                "exp-sp-ws-api",
                                "core_expert.sp_websocket_api",
                                sp.use_websocket_api,
                                view,
                                |d, on| d.special.use_websocket_api = on,
                            ))
                            // Moonbot's own neighbour of the WebSocket row (`cbHLFastIOC`).
                            .child(flag_live(
                                "exp-sp-hl-fast-ioc",
                                "core_expert.sp_hl_fast_ioc",
                                sp.use_hl_fast_ioc,
                                view,
                                |d, on| d.special.use_hl_fast_ioc = on,
                            )),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(flag_live(
                                "exp-sp-iceberg-sells",
                                "core_expert.sp_iceberg_sells",
                                draft.general.sell_iceberg,
                                view,
                                |d, on| d.general.sell_iceberg = on,
                            ))
                            .child(flag_live(
                                "exp-sp-book-ticker",
                                "core_expert.sp_book_ticker",
                                sp.use_book_ticker,
                                view,
                                |d, on| d.special.use_book_ticker = on,
                            ))
                            .child(flag_live(
                                "exp-sp-random-pct",
                                "core_expert.sp_random_percent",
                                sp.random_price,
                                view,
                                |d, on| d.special.random_price = on,
                            ))
                            .child(flag_live(
                                "exp-sp-weighted",
                                "core_expert.sp_weighted_mavg",
                                sp.m_avg_use_vol_weight,
                                view,
                                |d, on| d.special.m_avg_use_vol_weight = on,
                            ))
                            .child(flag_live(
                                "exp-sp-reduce",
                                "core_expert.sp_auto_reduce",
                                sp.auto_reduce_order,
                                view,
                                |d, on| d.special.auto_reduce_order = on,
                            ))
                            .child(flag_live(
                                "exp-sp-old-coins",
                                "core_expert.sp_old_as_new",
                                sp.show_old_listing,
                                view,
                                |d, on| d.special.show_old_listing = on,
                            ))
                            .child(flag_live(
                                "exp-sp-margin-borrow",
                                "core_expert.sp_margin_borrow",
                                sp.auto_margin_borrow,
                                view,
                                |d, on| d.special.auto_margin_borrow = on,
                            ))
                            .child(flag_live(
                                "exp-sp-margin-transfer",
                                "core_expert.sp_margin_transfer",
                                sp.auto_margin_transfer,
                                view,
                                |d, on| d.special.auto_margin_transfer = on,
                            )),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(gap)
                            .child(flag_live(
                                "exp-sp-correct-price",
                                "core_expert.sp_correct_price",
                                sp.correct_order_price,
                                view,
                                |d, on| d.special.correct_order_price = on,
                            ))
                            .child(flag_live(
                                "exp-sp-liq-control",
                                "core_expert.sp_liquidation_control",
                                sp.liq_control,
                                view,
                                |d, on| d.special.liq_control = on,
                            ))
                            .child(flag_live(
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
                            .child(choice_live(
                                "exp-sp-connection",
                                &connection_options(),
                                connection_ordinal(sp.binance_connection),
                                true,
                                view,
                                cx,
                                |d, v| d.special.binance_connection = i32::from(v),
                            )),
                    )
                    .child(
                        v_flex()
                            .gap(design::ui_px(cx, 2.0))
                            .child(flag_dead(
                                "exp-sp-custom-ip",
                                "core_expert.sp_custom_ip",
                                view,
                            ))
                            .child(flag_dead(
                                "exp-sp-auto-dns",
                                "core_expert.sp_auto_dns",
                                view,
                            )),
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
                    .child(dropdown("exp-sp-auth", NO_VALUE.to_string(), false, cx)),
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
            .children(rm.map(|rm| remote::notices(rm, p, cx)).unwrap_or_default())
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(design::ui_px(cx, 10.0))
                    // Not in the protocol: Moonbot's button opens Telegram on its own machine.
                    .child(action(
                        "exp-sp-add-bot",
                        "Add @TMoonBot to your channel".to_string(),
                        false,
                    ))
                    .children(
                        rm.map(|rm| remote::channel_actions(view, rm, p, cx))
                            .unwrap_or_default(),
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
                            .children(rm.map(|rm| remote::own_bot(view, rm, cx))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(design::ui_px(cx, 2.0))
                            .child(hint(t!("core_expert.sp_type_pin").to_string(), p, cx))
                            .children(rm.map(|rm| remote::pin(rm, p, cx))),
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
                    .child(flag_dead(
                        "exp-sp-send-reports",
                        "core_expert.sp_send_trade_reports",
                        view,
                    ))
                    .child(dropdown(
                        "exp-sp-system-reports",
                        NO_VALUE.to_string(),
                        false,
                        cx,
                    ))
                    .child(flag_live(
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
                    .child(flag_live(
                        "exp-sp-send-shots",
                        "core_expert.sp_send_shots",
                        sp.send_shots,
                        view,
                        |d, on| d.special.send_shots = on,
                    ))
                    .child(flag_live(
                        "exp-sp-send-public",
                        "core_expert.sp_send_public",
                        sp.send_public,
                        view,
                        |d, on| d.special.send_public = on,
                    ))
                    .child(flag_live(
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
                    .child(flag_dead(
                        "exp-sp-debug",
                        "core_expert.sp_extended_debug",
                        view,
                    ))
                    .child(flag_dead(
                        "exp-sp-market-export",
                        "core_expert.sp_market_export",
                        view,
                    ))
                    .child(flag_dead(
                        "exp-sp-udp-export",
                        "core_expert.sp_udp_export",
                        view,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(flag_live(
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
                    .child(flag_live(
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
                    .child(flag_dead("exp-sp-beta", "core_expert.sp_accept_beta", view)),
            )
    };

    // --- "Защита от зависаний" -----------------------------------------------------------------
    // Drawn from the core's observer snapshot rather than from the page: see [`watchdog`]. Absent
    // only on a frame built before the window read the block's context, which draws it empty.
    let watchdog_section = || match wd {
        Some(wd) => watchdog::body(view, wd, p, cx),
        None => div().into_any_element(),
    };

    // Only the open section is BUILT, not merely hidden: Moonbot shows one at a time, and a page
    // that assembled all four every frame would pay for three nobody is looking at.
    let open_body: AnyElement = match open {
        SpecialSection::Engine => engine().into_any_element(),
        SpecialSection::Remote => remote().into_any_element(),
        SpecialSection::System => system().into_any_element(),
        SpecialSection::Watchdog => watchdog_section(),
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
