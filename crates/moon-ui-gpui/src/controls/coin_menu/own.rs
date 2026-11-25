//! Chart navigation and actions on the specific order under the pointer.

use gpui::Entity;
use moon_ui::{MoonMenuItem, MoonTone, MoonWindowExt as _};
use rust_i18n::t;

use moon_core::config::SPLIT_ORDER_PARTS;

use super::{CoinMenuCtx, ReportRowMenu, workspace_action_allows_cores};
use crate::Backend;

/// Open the clicked market, using the Report's captured history scope when present.
pub(super) fn open_chart(ctx: &CoinMenuCtx, backend: &Entity<Backend>) -> MoonMenuItem {
    let core = ctx.core;
    let backend_open = backend.clone();
    let market = ctx.market.clone();
    let workspace_group = ctx.workspace_group.clone();
    let history = ctx.history.clone();
    MoonMenuItem::with_key(
        "coin-open",
        t!(if history.is_some() {
            "coin_menu.view_trades"
        } else {
            "coin_menu.open"
        })
        .to_string(),
    )
    .on_click(move |_, window, app| {
        window.close_context_menu(app);
        backend_open.update(app, |b, bcx| {
            let opened = if let Some(history) = history.clone() {
                b.open_report_on_main_if_authorized(
                    workspace_group.as_deref(),
                    (core, market.clone()),
                    history,
                    false,
                )
            } else {
                b.open_on_main_if_authorized(
                    workspace_group.as_deref(),
                    (core, market.clone()),
                    false,
                )
            };
            if opened {
                bcx.notify();
            }
        });
    })
}

/// Explain why the Report row cannot currently open on a chart.
pub(super) fn chart_not_ready() -> MoonMenuItem {
    MoonMenuItem::with_key(
        "coin-open-not-ready",
        t!("coin_menu.view_trades_not_ready").to_string(),
    )
    .disabled(true)
}

/// Keep the trade-log row visible with its localized reason when the request is unavailable.
pub(super) fn trade_log(
    report: &ReportRowMenu,
    disabled_reason: Option<&'static str>,
) -> MoonMenuItem {
    match disabled_reason {
        Some(reason) => {
            MoonMenuItem::with_key("rep-row-trade-log", t!(reason).to_string()).disabled(true)
        }
        None => report.trade_log.as_ref().unwrap().clone(),
    }
}

/// Open the order editor after rechecking workspace authority.
pub(super) fn order_edit(ctx: &CoinMenuCtx, backend: &Entity<Backend>, uid: u64) -> MoonMenuItem {
    let core = ctx.core;
    let backend_e = backend.clone();
    let workspace_group = ctx.workspace_group.clone();
    MoonMenuItem::with_key("coin-order-edit", t!("chart.order_menu.edit").to_string()).on_click(
        move |_, window, app| {
            window.close_context_menu(app);
            let authorized = backend_e.update(app, |b, _| {
                workspace_action_allows_cores(b, workspace_group.as_deref(), &[core])
            });
            if !authorized {
                return;
            }
            crate::panels::open_order_edit(
                backend_e.clone(),
                workspace_group.clone(),
                core,
                uid,
                window,
                app,
            );
        },
    )
}

/// Join sells for the clicked market and position direction.
pub(super) fn join_sells(ctx: &CoinMenuCtx, backend: &Entity<Backend>) -> MoonMenuItem {
    let core = ctx.core;
    let backend_j = backend.clone();
    let market_j = ctx.market.clone();
    let short = ctx.short;
    let workspace_group = ctx.workspace_group.clone();
    MoonMenuItem::with_key(
        "coin-order-join",
        t!("chart.order_menu.join_sells").to_string(),
    )
    .on_click(move |_, window, app| {
        window.close_context_menu(app);
        backend_j.update(app, |b, _| {
            if workspace_action_allows_cores(b, workspace_group.as_deref(), &[core]) {
                let _ = b.session.join_sells(core, market_j.clone(), short);
            }
        });
    })
}

/// Send the split count captured in this row's label, even if settings change before the click.
pub(super) fn split_order(
    ctx: &CoinMenuCtx,
    backend: &Entity<Backend>,
    uid: u64,
    parts: i32,
) -> MoonMenuItem {
    let core = ctx.core;
    let key = if parts == SPLIT_ORDER_PARTS {
        "coin-order-split"
    } else {
        "coin-order-split-n"
    };
    let backend_sp = backend.clone();
    let workspace_group = ctx.workspace_group.clone();
    MoonMenuItem::with_key(key, t!("chart.order_menu.split", n = parts).to_string()).on_click(
        move |_, window, app| {
            window.close_context_menu(app);
            backend_sp.update(app, |b, _| {
                if workspace_action_allows_cores(b, workspace_group.as_deref(), &[core]) {
                    let _ = b.session.split_order(core, uid, parts);
                }
            });
        },
    )
}

/// Cancel the whole order by UID after rechecking workspace authority.
pub(super) fn cancel_order(ctx: &CoinMenuCtx, backend: &Entity<Backend>, uid: u64) -> MoonMenuItem {
    let core = ctx.core;
    let backend_c = backend.clone();
    let workspace_group = ctx.workspace_group.clone();
    MoonMenuItem::with_key(
        "coin-order-cancel",
        t!("chart.order_menu.cancel").to_string(),
    )
    .tone(MoonTone::Danger)
    .on_click(move |_, window, app| {
        window.close_context_menu(app);
        backend_c.update(app, |b, _| {
            if workspace_action_allows_cores(b, workspace_group.as_deref(), &[core]) {
                let _ = b.session.cancel_order(core, uid);
            }
        });
    })
}
