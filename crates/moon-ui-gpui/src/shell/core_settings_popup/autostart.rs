//! "AutoStart" tab of the core-settings popup, reproducing Moonbot's settings page of the same name.
//!
//! Every control here edits the Shell-owned draft; nothing reaches the core until OK. The values
//! come from `trading.auto_start`, `trading.auto_start_2`, and `visual.blink_config` of the core's
//! safe-share configuration — see `moon_core::feed::AutoStartSettings` for the field-by-field
//! mapping.
//!
//! The "now" counters beside the two loss caps are the core's report totals
//! (`moon_core::feed::ProfitState`), not balances, which is why they can disagree with the header
//! P&L. Their Reset buttons act IMMEDIATELY, like the buttons above the tab strip: a reset is an
//! action on the core, not a setting to be staged.

use gpui::*;
use moon_ui::{MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::{CoreConfig, ResetProfitKind};
use moon_core::session::CoreId;

use crate::panels::popup_group;
use crate::shell::Shell;
use crate::shell::core_settings::draft::{
    ERRORS_LEVEL_BOUNDS, PING_LEVEL_BOUNDS, fmt_hhmm, parse_hhmm, parse_num,
};
use crate::shell::core_settings::resolve_core_settings_write;
use crate::{Backend, design};

use super::widgets::{caption, flag, num, slider};
use super::{SettingsWidgets, TabCtx};

/// The core's report counter line plus its Reset button.
///
/// The button is deliberately outside the OK/Cancel contract: `TResetProfitCommand` is an action,
/// and staging it would leave a "reset" the user pressed doing nothing until they also pressed OK.
#[allow(clippy::too_many_arguments)]
fn profit_line(
    id: &'static str,
    reset_label: String,
    profit: Option<(f64, i32)>,
    kind: ResetProfitKind,
    seeded: Option<CoreId>,
    backend: &Entity<Backend>,
    group: &str,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let text = match profit {
        Some((sum, trades)) => t!(
            "core_settings.as_now",
            sum = format!("{sum:+.2}"),
            trades = trades.to_string()
        )
        .to_string(),
        None => t!("core_settings.as_now_unknown").to_string(),
    };
    let reset = {
        let backend = backend.clone();
        let group = group.to_string();
        MoonButton::new(SharedString::from(id))
            .label(reset_label)
            .size(MoonButtonSize::Micro)
            .variant(MoonButtonVariant::Soft)
            .on_click(move |_, _w, app| {
                let b = backend.read(app);
                if let Some(core) = resolve_core_settings_write(seeded, b.active_trade_core(&group))
                    && let Err(e) = b.session.reset_profit(core, kind)
                {
                    log::warn!("reset profit failed: {e:#}");
                }
            })
            .render()
    };
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(caption(text, p, cx))
        .child(reset)
}

/// Builds the AutoStart tab's body.
///
/// Args:
///     ctx: Popup-wide addressing and palette.
///     draft: Pending tab state; every control reads and writes it.
///     profit: Core report counters, or `None` before the core publishes them.
///     widgets: Retained editors and sliders prepared by Shell for this render.
///     view: Shell entity used to stage checkbox changes.
///     cx: Application context used to read state and render controls.
///
/// Returns:
///     The tab body, without the popup chrome, the tab strip, or the OK/Cancel footer.
pub(super) fn autostart_tab(
    ctx: &TabCtx<'_>,
    draft: &CoreConfig,
    profit: Option<moon_core::feed::ProfitState>,
    widgets: &SettingsWidgets,
    view: &Entity<Shell>,
    cx: &App,
) -> AnyElement {
    let TabCtx {
        backend,
        group,
        seeded,
        p,
    } = *ctx;
    let s = &draft.auto_start;
    let blink = &draft.btc_blink;
    let gap = design::ui_px(cx, 6.0);

    // --- Enable on launch, plus the work-time window ---
    let launch = popup_group("core-as-launch", t!("core_settings.as_launch").to_string()).child(
        v_flex()
            .w_full()
            .gap(gap)
            .child(
                h_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 10.0))
                    .child(flag(
                        "as-start",
                        t!("core_settings.as_start").to_string(),
                        s.auto_start,
                        view,
                        |d, on| d.auto_start.auto_start = on,
                    ))
                    .child(flag(
                        "as-detect",
                        t!("core_settings.as_detect").to_string(),
                        s.auto_detect_on,
                        view,
                        |d, on| d.auto_start.auto_detect_on = on,
                    ))
                    .child(flag(
                        "as-strats",
                        t!("core_settings.as_strats").to_string(),
                        s.strategies_on,
                        view,
                        |d, on| d.auto_start.strategies_on = on,
                    )),
            )
            .child(flag(
                "as-remember",
                t!("core_settings.as_remember").to_string(),
                s.remember_state,
                view,
                |d, on| d.auto_start.remember_state = on,
            ))
            .child(flag(
                "as-update",
                t!("core_settings.as_update").to_string(),
                s.auto_update,
                view,
                |d, on| d.auto_start.auto_update = on,
            ))
            // Moonbot's checkbox reads "wait if open sells exist"; the wire field is its inverse.
            .child(flag(
                "as-wait-sells",
                t!("core_settings.as_wait_sells").to_string(),
                !s.dont_wait_sells,
                view,
                |d, on| d.auto_start.dont_wait_sells = !on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(flag(
                        "as-work-time",
                        t!("core_settings.as_work_time").to_string(),
                        s.work_time,
                        view,
                        |d, on| d.auto_start.work_time = on,
                    ))
                    .children(num(widgets, "as-work-from", cx))
                    .child(caption("—".to_string(), p, cx))
                    .children(num(widgets, "as-work-to", cx)),
            ),
    );

    // --- Loss cap over a trade window ---
    let loss_trades = popup_group(
        "core-as-loss-trades",
        t!("core_settings.as_loss_trades_title").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(gap)
            .child(flag(
                "as-loss-on",
                t!("core_settings.as_loss_if").to_string(),
                s.auto_stop_if_loss,
                view,
                |d, on| d.auto_start.auto_stop_if_loss = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .children(num(widgets, "as-loss-sum", cx))
                    .child(caption(t!("core_settings.as_per").to_string(), p, cx))
                    .children(num(widgets, "as-loss-trades", cx))
                    .child(caption(t!("core_settings.as_trades").to_string(), p, cx)),
            )
            .child(profit_line(
                "as-reset-session",
                t!("core_settings.as_reset_session").to_string(),
                profit.map(|p| (p.total_profit, p.total_trades)),
                ResetProfitKind::Session,
                seeded,
                backend,
                group,
                p,
                cx,
            ))
            .child(flag(
                "as-loss-panic",
                t!("core_settings.as_also_panic").to_string(),
                s.sell_if_loss,
                view,
                |d, on| d.auto_start.sell_if_loss = on,
            )),
    );

    // --- Loss cap over an hourly window, session reset ---
    let loss_hours = popup_group(
        "core-as-loss-hours",
        t!("core_settings.as_loss_hours_title").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(gap)
            .child(flag(
                "as-hours-on",
                t!("core_settings.as_loss_if").to_string(),
                s.auto_stop_if_loss_hours,
                view,
                |d, on| d.auto_start.auto_stop_if_loss_hours = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .children(num(widgets, "as-hours-sum", cx))
                    .child(caption(t!("core_settings.as_per").to_string(), p, cx))
                    .children(num(widgets, "as-hours", cx))
                    .child(caption(t!("core_settings.as_hours").to_string(), p, cx)),
            )
            .child(profit_line(
                "as-reset-all",
                t!("core_settings.as_reset_all").to_string(),
                profit.map(|p| (p.hourly_profit, p.hourly_trades)),
                ResetProfitKind::All,
                seeded,
                backend,
                group,
                p,
                cx,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(flag(
                        "as-ignore-emu",
                        t!("core_settings.as_ignore_emu").to_string(),
                        s.ignore_emulator,
                        view,
                        |d, on| d.auto_start.ignore_emulator = on,
                    ))
                    .child(caption(
                        t!("core_settings.as_and_trades_gt").to_string(),
                        p,
                        cx,
                    ))
                    .children(num(widgets, "as-hours-trades", cx)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(flag(
                        "as-reset-every",
                        t!("core_settings.as_reset_session_every").to_string(),
                        s.reset_session,
                        view,
                        |d, on| d.auto_start.reset_session = on,
                    ))
                    .children(num(widgets, "as-rs-hours", cx))
                    .child(caption(t!("core_settings.as_hours").to_string(), p, cx)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(caption(
                        t!("core_settings.as_session_cap").to_string(),
                        p,
                        cx,
                    ))
                    .children(num(widgets, "as-session-cap", cx)),
            ),
    );

    // --- Global panic sell on BTC ---
    let panic_btc = popup_group(
        "core-as-panic-btc",
        t!("core_settings.as_panic_btc_title").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(gap)
            .child(flag(
                "as-panic-btc",
                t!("core_settings.as_panic_btc").to_string(),
                s.panic_btc,
                view,
                |d, on| d.auto_start.panic_btc = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(caption(t!("core_settings.as_btc_fell").to_string(), p, cx))
                    .children(num(widgets, "as-btc-down", cx)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(caption(t!("core_settings.as_btc_rose").to_string(), p, cx))
                    .children(num(widgets, "as-btc-up", cx)),
            ),
    );

    // --- Global panic sell on the whole market, and the restart band ---
    let panic_market = popup_group(
        "core-as-panic-market",
        t!("core_settings.as_panic_market_title").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(gap)
            .child(flag(
                "as-panic-market",
                t!("core_settings.as_panic_market").to_string(),
                s.panic_market,
                view,
                |d, on| d.auto_start.panic_market = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(caption(
                        t!("core_settings.as_market_fell").to_string(),
                        p,
                        cx,
                    ))
                    .children(num(widgets, "as-market-down", cx)),
            )
            .child(flag(
                "as-restart-market",
                t!("core_settings.as_restart_if").to_string(),
                s.restart_on_market,
                view,
                |d, on| d.auto_start.restart_on_market = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 4.0))
                    .child(caption("BTC >".to_string(), p, cx))
                    .children(num(widgets, "as-btc-higher", cx))
                    .child(caption("BTC <".to_string(), p, cx))
                    .children(num(widgets, "as-btc-lower", cx))
                    .child(caption(t!("core_settings.as_market_gt").to_string(), p, cx))
                    .children(num(widgets, "as-market-higher", cx)),
            ),
    );

    // --- Error watchdog ---
    let errors = popup_group(
        "core-as-errors",
        t!("core_settings.as_errors_title").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(gap)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(flag(
                        "as-errors-on",
                        t!("core_settings.as_errors_stop").to_string(),
                        s.auto_stop_on_errors,
                        view,
                        |d, on| d.auto_start.auto_stop_on_errors = on,
                    ))
                    .child(div().flex_1().children(slider(widgets, "as-errors")))
                    .children(num(widgets, "as-errors-level", cx)),
            )
            .child(flag(
                "as-errors-panic",
                t!("core_settings.as_also_panic").to_string(),
                s.sell_all_on_errors,
                view,
                |d, on| d.auto_start.sell_all_on_errors = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(flag(
                        "as-errors-restart",
                        t!("core_settings.as_restart_after").to_string(),
                        s.restart_after_err,
                        view,
                        |d, on| d.auto_start.restart_after_err = on,
                    ))
                    .children(num(widgets, "as-errors-restart-min", cx))
                    .child(caption(t!("core_settings.as_minutes").to_string(), p, cx)),
            ),
    );

    // --- Ping watchdog ---
    let ping = popup_group(
        "core-as-ping",
        t!("core_settings.as_ping_title").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(gap)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(flag(
                        "as-ping-on",
                        t!("core_settings.as_ping_stop").to_string(),
                        s.auto_stop_on_ping,
                        view,
                        |d, on| d.auto_start.auto_stop_on_ping = on,
                    ))
                    .child(div().flex_1().children(slider(widgets, "as-ping")))
                    .children(num(widgets, "as-ping-level", cx)),
            )
            .child(flag(
                "as-ping-panic",
                t!("core_settings.as_also_panic").to_string(),
                s.sell_all_on_ping,
                view,
                |d, on| d.auto_start.sell_all_on_ping = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(flag(
                        "as-ping-restart",
                        t!("core_settings.as_restart_after").to_string(),
                        s.restart_after_ping,
                        view,
                        |d, on| d.auto_start.restart_after_ping = on,
                    ))
                    .children(num(widgets, "as-ping-restart-min", cx))
                    .child(caption(t!("core_settings.as_minutes").to_string(), p, cx)),
            ),
    );

    // --- BTC highlight and alarm (visual.blink_config) ---
    let blink_group = popup_group(
        "core-as-blink",
        t!("core_settings.as_blink_title").to_string(),
    )
    .child(
        v_flex()
            .w_full()
            .gap(gap)
            .child(flag(
                "as-blink",
                t!("core_settings.as_blink").to_string(),
                blink.blink_btc,
                view,
                |d, on| d.btc_blink.blink_btc = on,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(caption(t!("core_settings.as_btc_fell").to_string(), p, cx))
                    .children(num(widgets, "as-blink-down", cx))
                    .child(caption(t!("core_settings.as_btc_rose").to_string(), p, cx))
                    .children(num(widgets, "as-blink-up", cx)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .child(flag(
                        "as-alarm",
                        t!("core_settings.as_alarm").to_string(),
                        blink.alarm_btc,
                        view,
                        |d, on| d.btc_blink.alarm_btc = on,
                    ))
                    // A number, not a picker: the protocol carries `alarm_type` as an opaque
                    // ordinal and sends no list of sound names to label a dropdown with. A nested
                    // menu would also close this popover — see `def_alert_strategy_row`.
                    .child(caption(t!("core_settings.as_alarm_no").to_string(), p, cx))
                    .children(num(widgets, "as-alarm-type", cx)),
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
                .child(launch)
                .child(loss_trades)
                .child(panic_btc)
                .child(errors),
        )
        .child(
            v_flex()
                .flex_1()
                .gap(design::ui_px(cx, 8.0))
                .child(loss_hours)
                .child(panic_market)
                .child(ping)
                .child(blink_group),
        )
        .into_any_element()
}

/// The tab's numeric editors, in the order Shell should create them.
///
/// Kept beside the render pass on purpose: a field added to one list and forgotten in the other
/// renders without its input, and having both lists in one file makes that visible in review.
/// Each entry is `(id, current value, staging function, field width)`.
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str), f32)> {
    let s = &draft.auto_start;
    let b = &draft.btc_blink;
    vec![
        (
            "as-work-from",
            fmt_hhmm(s.work_time_from_min),
            (|d, t| {
                if let Some(v) = parse_hhmm(t) {
                    d.auto_start.work_time_from_min = v;
                }
            }) as fn(&mut CoreConfig, &str),
            52.0,
        ),
        (
            "as-work-to",
            fmt_hhmm(s.work_time_to_min),
            |d, t| {
                if let Some(v) = parse_hhmm(t) {
                    d.auto_start.work_time_to_min = v;
                }
            },
            52.0,
        ),
        (
            "as-loss-sum",
            format!("{:.2}", s.auto_stop_loss),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.auto_stop_loss = v;
                }
            },
            72.0,
        ),
        (
            "as-loss-trades",
            s.stop_trades.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.stop_trades = v as i32;
                }
            },
            48.0,
        ),
        (
            "as-hours-sum",
            format!("{:.2}", s.auto_stop_hours_val),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.auto_stop_hours_val = v;
                }
            },
            72.0,
        ),
        (
            "as-hours",
            s.stop_hours.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.stop_hours = v as i32;
                }
            },
            48.0,
        ),
        (
            "as-hours-trades",
            s.stop_hours_trades.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.stop_hours_trades = v as i32;
                }
            },
            48.0,
        ),
        (
            "as-rs-hours",
            s.rs_hours.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.rs_hours = v as i32;
                }
            },
            48.0,
        ),
        (
            "as-session-cap",
            s.max_session_cap.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.max_session_cap = v as i32;
                }
            },
            56.0,
        ),
        (
            "as-btc-down",
            format!("{:+.2}", s.panic_btc_delta),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.panic_btc_delta = v;
                }
            },
            60.0,
        ),
        (
            "as-btc-up",
            format!("{:+.2}", s.panic_btc_delta_up),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.panic_btc_delta_up = v;
                }
            },
            60.0,
        ),
        (
            "as-market-down",
            format!("{:+.2}", s.panic_market_delta),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.panic_market_delta = v;
                }
            },
            60.0,
        ),
        (
            "as-btc-higher",
            format!("{:+.2}", s.btc_higher_then),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.btc_higher_then = v;
                }
            },
            56.0,
        ),
        (
            "as-btc-lower",
            format!("{:+.2}", s.btc_lower_then),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.btc_lower_then = v;
                }
            },
            56.0,
        ),
        (
            "as-market-higher",
            format!("{:+.2}", s.market_higher_then),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.market_higher_then = v;
                }
            },
            56.0,
        ),
        (
            "as-errors-level",
            s.errors_level.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.errors_level = v as i32;
                }
            },
            48.0,
        ),
        (
            "as-errors-restart-min",
            s.restart_err_time.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.restart_err_time = v as i32;
                }
            },
            48.0,
        ),
        (
            "as-ping-level",
            s.ping_level.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.ping_level = v as i32;
                }
            },
            56.0,
        ),
        (
            "as-ping-restart-min",
            s.restart_ping_time.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.restart_ping_time = v as i32;
                }
            },
            48.0,
        ),
        (
            "as-blink-down",
            format!("{:+.2}", b.blink_btc_delta),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.btc_blink.blink_btc_delta = v;
                }
            },
            60.0,
        ),
        (
            "as-blink-up",
            format!("{:+.2}", b.blink_btc_delta_up),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.btc_blink.blink_btc_delta_up = v;
                }
            },
            60.0,
        ),
        (
            "as-alarm-type",
            b.alarm_type.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.btc_blink.alarm_type = v.clamp(0.0, 255.0) as u8;
                }
            },
            44.0,
        ),
    ]
}

/// Sliders this tab owns, as `(id, bounds, current value, staging function)`.
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
    let s = &draft.auto_start;
    vec![
        (
            "as-errors",
            ERRORS_LEVEL_BOUNDS,
            s.errors_level as f32,
            (|d, v| d.auto_start.errors_level = v.round() as i32) as fn(&mut CoreConfig, f32),
            Some("as-errors-level"),
        ),
        (
            "as-ping",
            PING_LEVEL_BOUNDS,
            s.ping_level as f32,
            |d, v| d.auto_start.ping_level = v.round() as i32,
            Some("as-ping-level"),
        ),
    ]
}
