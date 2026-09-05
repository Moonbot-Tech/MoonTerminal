//! Moonbot's "Автостарт" page, control for control — and the second page of this window that is
//! fully LIVE.
//!
//! Every switch, box and slider here edits `moon_core::feed::AutoStartSettings` or
//! `BtcBlinkSettings`, both of which the terminal projects and `ExpertTab::add_sections`
//! carries back, so OK sends what the page shows. The compact popup draws the same fields in its
//! own compact order; this page follows Moonbot's layout instead, row for row.
//!
//! The two "Сейчас:" counters are the core's REPORT totals (`moon_core::feed::ProfitState`), not
//! balances, which is why they can disagree with the header P&L. Their Reset buttons act
//! IMMEDIATELY, outside the OK/Cancel contract: a reset is an action on the core, and staging it
//! would leave a button the user pressed doing nothing until they also pressed OK.

use gpui::*;
use moon_ui::{MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::{CoreConfig, ResetProfitKind};
use moon_core::session::CoreId;

use crate::Backend;
use crate::design;
use crate::shell::editors::EditorStore;
use crate::shell::{
    ERRORS_LEVEL_BOUNDS, PING_LEVEL_BOUNDS, fmt_hhmm, parse_hhmm, parse_num,
    resolve_core_settings_write,
};

use super::super::CoreExpertView;
use super::super::widgets::{caption, flag, hint, num, rows, slider, sound_cell};
use super::ProfitCounter;

/// See [`super::field_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    let a = &draft.auto_start;
    let b = &draft.btc_blink;
    vec![
        (
            "exp-as-work-from",
            fmt_hhmm(a.work_time_from_min),
            (|d, t| {
                if let Some(v) = parse_hhmm(t) {
                    d.auto_start.work_time_from_min = v;
                }
            }) as fn(&mut CoreConfig, &str),
        ),
        ("exp-as-work-to", fmt_hhmm(a.work_time_to_min), |d, t| {
            if let Some(v) = parse_hhmm(t) {
                d.auto_start.work_time_to_min = v;
            }
        }),
        (
            "exp-as-loss-sum",
            format!("{:.2}", a.auto_stop_loss),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.auto_stop_loss = v;
                }
            },
        ),
        ("exp-as-loss-trades", a.stop_trades.to_string(), |d, t| {
            if let Some(v) = parse_num(t) {
                d.auto_start.stop_trades = v.round() as i32;
            }
        }),
        (
            "exp-as-hours-sum",
            format!("{:.2}", a.auto_stop_hours_val),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.auto_stop_hours_val = v;
                }
            },
        ),
        ("exp-as-hours", a.stop_hours.to_string(), |d, t| {
            if let Some(v) = parse_num(t) {
                d.auto_start.stop_hours = v.round() as i32;
            }
        }),
        (
            "exp-as-hours-trades",
            a.stop_hours_trades.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.stop_hours_trades = v.round() as i32;
                }
            },
        ),
        ("exp-as-rs-hours", a.rs_hours.to_string(), |d, t| {
            if let Some(v) = parse_num(t) {
                d.auto_start.rs_hours = v.round() as i32;
            }
        }),
        (
            "exp-as-session-cap",
            a.max_session_cap.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.max_session_cap = v.round() as i32;
                }
            },
        ),
        (
            "exp-as-btc-down",
            format!("{:+.2}", a.panic_btc_delta),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.panic_btc_delta = v;
                }
            },
        ),
        (
            "exp-as-btc-up",
            format!("{:+.2}", a.panic_btc_delta_up),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.panic_btc_delta_up = v;
                }
            },
        ),
        (
            "exp-as-market-down",
            format!("{:+.2}", a.panic_market_delta),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.panic_market_delta = v;
                }
            },
        ),
        (
            "exp-as-btc-higher",
            format!("{:+.2}", a.btc_higher_then),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.btc_higher_then = v;
                }
            },
        ),
        (
            "exp-as-btc-lower",
            format!("{:+.2}", a.btc_lower_then),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.btc_lower_then = v;
                }
            },
        ),
        (
            "exp-as-market-higher",
            format!("{:+.2}", a.market_higher_then),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.market_higher_then = v;
                }
            },
        ),
        (
            "exp-as-err-restart",
            a.restart_err_time.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.restart_err_time = v.round() as i32;
                }
            },
        ),
        (
            "exp-as-ping-restart",
            a.restart_ping_time.to_string(),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.auto_start.restart_ping_time = v.round() as i32;
                }
            },
        ),
        (
            "exp-as-blink-down",
            format!("{:+.2}", b.blink_btc_delta),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.btc_blink.blink_btc_delta = v;
                }
            },
        ),
        (
            "exp-as-blink-up",
            format!("{:+.2}", b.blink_btc_delta_up),
            |d, t| {
                if let Some(v) = parse_num(t) {
                    d.btc_blink.blink_btc_delta_up = v;
                }
            },
        ),
    ]
}

/// See [`super::slider_specs`].
///
/// Neither mirrors a box, because Moonbot draws none here: the level is printed in the caption of
/// the checkbox that owns the watchdog, and the track beside it is the only way to set it.
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
    let a = &draft.auto_start;
    vec![
        (
            "exp-as-errors",
            ERRORS_LEVEL_BOUNDS,
            a.errors_level as f32,
            (|d, v| d.auto_start.errors_level = v.round() as i32) as fn(&mut CoreConfig, f32),
            None,
        ),
        (
            "exp-as-ping",
            PING_LEVEL_BOUNDS,
            a.ping_level as f32,
            |d, v| d.auto_start.ping_level = v.round() as i32,
            None,
        ),
    ]
}

/// How far Moonbot indents a line that qualifies the checkbox above it.
const SUBROW_INDENT: f32 = 18.0;

/// Moonbot's watchdog track is SHORT and starts about halfway across the dialog — not at the end of
/// its caption, and not spanning the rest of the row: measured off that window, the caption zone
/// runs to ~46% of the content width and the track covers ~17% of it.
///
/// Fractions rather than pixels, so the pair keeps Moonbot's proportion at any window size; and the
/// caption column is a MINIMUM, so a longer translation pushes the track right instead of being
/// clipped by it.
const WATCHDOG_CAPTION_W: f32 = 0.46;
const WATCHDOG_TRACK_W: f32 = 0.17;

/// The core's report counter line plus its Reset button.
#[allow(clippy::too_many_arguments)]
fn profit_line(
    id: &'static str,
    reset_label: String,
    profit: ProfitCounter,
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
    let backend = backend.clone();
    let group = group.to_string();
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(hint(text, p, cx))
        .child(
            MoonButton::new(SharedString::from(id))
                .label(reset_label)
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Soft)
                .on_click(move |_, _w, app| {
                    let b = backend.read(app);
                    // The same hazard OK answers with its banner: the core can move between the
                    // render that drew this button and the click that pressed it. Silence would be
                    // indistinguishable from a counter that reset and had nothing to show for it.
                    let Some(core) =
                        resolve_core_settings_write(seeded, b.active_trade_core(&group))
                    else {
                        log::warn!(
                            "reset profit ignored: the active core moved since the page was seeded"
                        );
                        return;
                    };
                    if let Err(e) = b.session.reset_profit(core, kind) {
                        log::warn!("reset profit failed: {e:#}");
                    }
                })
                .render(),
        )
}

/// Build the page.
#[allow(clippy::too_many_arguments)]
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    draft: &CoreConfig,
    backend: &Entity<Backend>,
    group_name: &str,
    seeded: Option<CoreId>,
    profit: (ProfitCounter, ProfitCounter),
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let a = &draft.auto_start;
    let b = &draft.btc_blink;
    let gap = design::ui_px(cx, 6.0);
    let (window_profit, hourly_profit) = profit;

    // --- What the core turns on when it starts ---------------------------------------------------
    let startup = rows(cx)
        .gap(gap)
        .child(caption(
            t!("core_expert.as_enable_on_start").to_string(),
            true,
            p,
            cx,
        ))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 18.0))
                .child(flag(
                    "exp-as-start",
                    t!("core_settings.as_start").to_string(),
                    a.auto_start,
                    true,
                    view,
                    |d, on| d.auto_start.auto_start = on,
                ))
                .child(flag(
                    "exp-as-detect",
                    t!("core_settings.as_detect").to_string(),
                    a.auto_detect_on,
                    true,
                    view,
                    |d, on| d.auto_start.auto_detect_on = on,
                ))
                .child(flag(
                    "exp-as-strats",
                    t!("core_settings.as_strats").to_string(),
                    a.strategies_on,
                    true,
                    view,
                    |d, on| d.auto_start.strategies_on = on,
                )),
        )
        .child(flag(
            "exp-as-remember",
            t!("core_settings.as_remember").to_string(),
            a.remember_state,
            true,
            view,
            |d, on| d.auto_start.remember_state = on,
        ))
        .child(flag(
            "exp-as-update",
            t!("core_settings.as_update").to_string(),
            a.auto_update,
            true,
            view,
            |d, on| d.auto_start.auto_update = on,
        ));

    let work_time = rows(cx)
        .gap(gap)
        .child(flag(
            "exp-as-work-time",
            t!("core_settings.as_work_time").to_string(),
            a.work_time,
            true,
            view,
            |d, on| d.auto_start.work_time = on,
        ))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .children(num(store, "exp-as-work-from", 64.0, true, cx))
                .child(caption("—".to_string(), true, p, cx))
                .children(num(store, "exp-as-work-to", 64.0, true, cx)),
        )
        .child(flag(
            "exp-as-wait-sells",
            t!("core_settings.as_wait_sells").to_string(),
            !a.dont_wait_sells,
            true,
            view,
            |d, on| d.auto_start.dont_wait_sells = !on,
        ));

    // --- The two loss caps, each with its own report counter -------------------------------------
    let loss_window = rows(cx)
        .gap(gap)
        .child(flag(
            "exp-as-loss-on",
            t!("core_settings.as_loss_if").to_string(),
            a.auto_stop_if_loss,
            true,
            view,
            |d, on| d.auto_start.auto_stop_if_loss = on,
        ))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .children(num(store, "exp-as-loss-sum", 96.0, true, cx))
                .child(caption(t!("core_settings.as_per").to_string(), true, p, cx))
                .children(num(store, "exp-as-loss-trades", 56.0, true, cx))
                .child(caption(
                    t!("core_settings.as_trades").to_string(),
                    true,
                    p,
                    cx,
                )),
        )
        .child(profit_line(
            "exp-as-reset-window",
            t!("core_settings.as_reset_session").to_string(),
            window_profit,
            ResetProfitKind::Session,
            seeded,
            backend,
            group_name,
            p,
            cx,
        ))
        .child(flag(
            "exp-as-loss-panic",
            t!("core_settings.as_also_panic").to_string(),
            a.sell_if_loss,
            true,
            view,
            |d, on| d.auto_start.sell_if_loss = on,
        ));

    let loss_hours = rows(cx)
        .gap(gap)
        .child(flag(
            "exp-as-hours-on",
            t!("core_settings.as_loss_if").to_string(),
            a.auto_stop_if_loss_hours,
            true,
            view,
            |d, on| d.auto_start.auto_stop_if_loss_hours = on,
        ))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .children(num(store, "exp-as-hours-sum", 96.0, true, cx))
                .child(caption(t!("core_settings.as_per").to_string(), true, p, cx))
                .children(num(store, "exp-as-hours", 56.0, true, cx))
                .child(caption(
                    t!("core_settings.as_hours").to_string(),
                    true,
                    p,
                    cx,
                )),
        )
        .child(profit_line(
            "exp-as-reset-hours",
            t!("core_settings.as_reset_all").to_string(),
            hourly_profit,
            ResetProfitKind::All,
            seeded,
            backend,
            group_name,
            p,
            cx,
        ))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(flag(
                    "exp-as-reset-every",
                    t!("core_settings.as_reset_session_every").to_string(),
                    a.reset_session,
                    true,
                    view,
                    |d, on| d.auto_start.reset_session = on,
                ))
                .children(num(store, "exp-as-rs-hours", 56.0, true, cx))
                .child(caption(
                    t!("core_settings.as_hours").to_string(),
                    true,
                    p,
                    cx,
                )),
        )
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(caption(
                    t!("core_settings.as_session_cap").to_string(),
                    true,
                    p,
                    cx,
                ))
                .children(num(store, "exp-as-session-cap", 64.0, true, cx)),
        );

    let emulator = rows(cx)
        .gap(gap)
        .child(flag(
            "exp-as-ignore-emu",
            t!("core_settings.as_ignore_emu").to_string(),
            a.ignore_emulator,
            true,
            view,
            |d, on| d.auto_start.ignore_emulator = on,
        ))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(caption(
                    t!("core_settings.as_and_trades_gt").to_string(),
                    true,
                    p,
                    cx,
                ))
                .children(num(store, "exp-as-hours-trades", 56.0, true, cx)),
        );

    // --- The market watchdogs --------------------------------------------------------------------
    let panic_btc = rows(cx)
        .gap(gap)
        .child(flag(
            "exp-as-panic-btc",
            t!("core_settings.as_panic_btc").to_string(),
            a.panic_btc,
            true,
            view,
            |d, on| d.auto_start.panic_btc = on,
        ))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .pl(design::ui_px(cx, SUBROW_INDENT))
                .child(caption(
                    t!("core_settings.as_btc_fell").to_string(),
                    true,
                    p,
                    cx,
                ))
                .children(num(store, "exp-as-btc-down", 72.0, true, cx)),
        )
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .pl(design::ui_px(cx, SUBROW_INDENT))
                .child(caption(
                    t!("core_settings.as_btc_rose").to_string(),
                    true,
                    p,
                    cx,
                ))
                .children(num(store, "exp-as-btc-up", 72.0, true, cx)),
        );

    let panic_market = rows(cx)
        .gap(gap)
        .child(flag(
            "exp-as-panic-market",
            t!("core_settings.as_panic_market").to_string(),
            a.panic_market,
            true,
            view,
            |d, on| d.auto_start.panic_market = on,
        ))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .pl(design::ui_px(cx, SUBROW_INDENT))
                .child(caption(
                    t!("core_settings.as_market_fell").to_string(),
                    true,
                    p,
                    cx,
                ))
                .children(num(store, "exp-as-market-down", 72.0, true, cx)),
        );

    let restart_market = rows(cx).gap(gap).child(
        h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(flag(
                "exp-as-restart-market",
                t!("core_settings.as_restart_if").to_string(),
                a.restart_on_market,
                true,
                view,
                |d, on| d.auto_start.restart_on_market = on,
            ))
            .child(caption(
                t!("core_expert.as_btc_delta_over").to_string(),
                true,
                p,
                cx,
            ))
            .children(num(store, "exp-as-btc-lower", 68.0, true, cx))
            .child(caption(
                t!("core_expert.as_btc_delta_under").to_string(),
                true,
                p,
                cx,
            ))
            .children(num(store, "exp-as-btc-higher", 68.0, true, cx))
            .child(caption(
                t!("core_settings.as_market_gt").to_string(),
                true,
                p,
                cx,
            ))
            .children(num(store, "exp-as-market-higher", 68.0, true, cx)),
    );

    // --- The two health watchdogs, each with its own restart timer --------------------------------
    let errors = rows(cx)
        .gap(gap)
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 12.0))
                .child(
                    div()
                        .flex_none()
                        .min_w(relative(WATCHDOG_CAPTION_W))
                        .child(flag(
                            "exp-as-errors-on",
                            format!("{} {}", t!("core_settings.as_errors_stop"), a.errors_level),
                            a.auto_stop_on_errors,
                            true,
                            view,
                            |d, on| d.auto_start.auto_stop_on_errors = on,
                        )),
                )
                .child(
                    div()
                        .flex_none()
                        .w(relative(WATCHDOG_TRACK_W))
                        .children(slider(store, "exp-as-errors", true)),
                ),
        )
        .child(div().pl(design::ui_px(cx, SUBROW_INDENT)).child(flag(
            "exp-as-errors-panic",
            t!("core_settings.as_also_panic").to_string(),
            a.sell_all_on_errors,
            true,
            view,
            |d, on| d.auto_start.sell_all_on_errors = on,
        )))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .pl(design::ui_px(cx, SUBROW_INDENT))
                .child(flag(
                    "exp-as-err-restart-on",
                    t!(
                        "core_expert.as_restart_after_min",
                        v = a.restart_err_time.to_string()
                    )
                    .to_string(),
                    a.restart_after_err,
                    true,
                    view,
                    |d, on| d.auto_start.restart_after_err = on,
                ))
                .children(num(store, "exp-as-err-restart", 56.0, true, cx))
                .child(hint(
                    t!("core_expert.as_not_recommended").to_string(),
                    p,
                    cx,
                )),
        );

    let ping = rows(cx)
        .gap(gap)
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 12.0))
                .child(
                    div()
                        .flex_none()
                        .min_w(relative(WATCHDOG_CAPTION_W))
                        .child(flag(
                            "exp-as-ping-on",
                            format!("{} {} ms", t!("core_settings.as_ping_stop"), a.ping_level),
                            a.auto_stop_on_ping,
                            true,
                            view,
                            |d, on| d.auto_start.auto_stop_on_ping = on,
                        )),
                )
                .child(
                    div()
                        .flex_none()
                        .w(relative(WATCHDOG_TRACK_W))
                        .children(slider(store, "exp-as-ping", true)),
                ),
        )
        .child(div().pl(design::ui_px(cx, SUBROW_INDENT)).child(flag(
            "exp-as-ping-panic",
            t!("core_settings.as_also_panic").to_string(),
            a.sell_all_on_ping,
            true,
            view,
            |d, on| d.auto_start.sell_all_on_ping = on,
        )))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .pl(design::ui_px(cx, SUBROW_INDENT))
                .child(flag(
                    "exp-as-ping-restart-on",
                    t!(
                        "core_expert.as_restart_after_min",
                        v = a.restart_ping_time.to_string()
                    )
                    .to_string(),
                    a.restart_after_ping,
                    true,
                    view,
                    |d, on| d.auto_start.restart_after_ping = on,
                ))
                .children(num(store, "exp-as-ping-restart", 56.0, true, cx))
                .child(hint(
                    t!("core_expert.as_not_recommended").to_string(),
                    p,
                    cx,
                )),
        );

    // --- The BTC blink and its alarm, which live in `visual.blink_config` -------------------------
    let blink = rows(cx)
        .gap(gap)
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap(design::ui_px(cx, 12.0))
                .child(div().flex_1().min_w_0().child(flag(
                    "exp-as-blink",
                    t!("core_expert.as_blink_btc").to_string(),
                    b.blink_btc,
                    true,
                    view,
                    |d, on| d.btc_blink.blink_btc = on,
                )))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(design::ui_px(cx, 4.0))
                        .child(flag(
                            "exp-as-alarm",
                            t!("core_expert.as_alarm_btc").to_string(),
                            b.alarm_btc,
                            true,
                            view,
                            |d, on| d.btc_blink.alarm_btc = on,
                        ))
                        .child(sound_cell(
                            "exp-as-alarm-sound",
                            i32::from(b.alarm_type),
                            view,
                            |d, v| d.btc_blink.alarm_type = v.clamp(0, i32::from(u8::MAX)) as u8,
                            p,
                            cx,
                        )),
                ),
        )
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .pl(design::ui_px(cx, SUBROW_INDENT))
                .child(caption(
                    t!("core_settings.as_btc_fell").to_string(),
                    true,
                    p,
                    cx,
                ))
                .children(num(store, "exp-as-blink-down", 72.0, true, cx))
                .child(caption(
                    t!("core_settings.as_btc_rose").to_string(),
                    true,
                    p,
                    cx,
                ))
                .children(num(store, "exp-as-blink-up", 72.0, true, cx)),
        );

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        // Moonbot's head: what starts with the core on the left, its working hours beside it.
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap(design::ui_px(cx, 16.0))
                .child(div().flex_1().min_w_0().child(startup))
                .child(div().flex_1().min_w_0().child(work_time)),
        )
        // The two loss caps side by side, with the emulator switch in Moonbot's narrower third
        // column.
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap(design::ui_px(cx, 16.0))
                .child(div().flex_1().min_w_0().child(loss_window))
                .child(div().flex_1().min_w_0().child(loss_hours))
                .child(
                    div()
                        .w(design::ui_px(cx, 200.0))
                        .flex_none()
                        .child(emulator),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap(design::ui_px(cx, 16.0))
                .child(div().flex_1().min_w_0().child(panic_btc))
                .child(div().flex_1().min_w_0().child(panic_market)),
        )
        .child(restart_market)
        // The two watchdogs: Moonbot puts the track on the SAME line as the caption — short, and
        // starting about halfway across, see `WATCHDOG_TRACK_W` — with its two sub-rows indented
        // underneath.
        .child(errors)
        .child(ping)
        .child(blink)
        .into_any_element()
}
