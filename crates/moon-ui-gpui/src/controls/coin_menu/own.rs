//! Chart navigation and actions on the specific order under the pointer.

use gpui::{App, Entity};
use moon_ui::{MoonMenuItem, MoonTone, MoonWindowExt as _};
use rust_i18n::t;

use moon_core::config::SPLIT_ORDER_PARTS;

use super::{CoinMenuCtx, ReportRowMenu, with_hint, workspace_action_allows_cores};
use crate::Backend;

/// Open the clicked market, using the Report's captured history scope when present.
pub(super) fn open_chart(ctx: &CoinMenuCtx, backend: &Entity<Backend>) -> MoonMenuItem {
    let hint = ctx
        .report
        .as_ref()
        .and_then(|report| report.chart_hint.clone());
    let backend = backend.clone();
    let ctx = ctx.clone();
    let item = MoonMenuItem::with_key(
        "coin-open",
        t!(if ctx.history.is_some() {
            "coin_menu.view_trades"
        } else {
            "coin_menu.open"
        })
        .to_string(),
    )
    .on_click(move |_, window, app| {
        window.close_context_menu(app);
        open_chart_target(&ctx, &backend, app);
    });
    with_hint(item, hint)
}

/// Run the menu's chart navigation with the captured workspace and published history filter.
pub(crate) fn open_chart_target(
    ctx: &CoinMenuCtx,
    backend: &Entity<Backend>,
    app: &mut App,
) -> bool {
    backend.update(app, |b, bcx| {
        let target = (ctx.core, ctx.market.clone());
        let group = ctx.workspace_group.as_deref();
        let opened = if let Some(history) = ctx.history.clone() {
            b.open_report_on_main_if_authorized(group, target, history, false)
        } else {
            b.open_on_main_if_authorized(group, target, false)
        };
        if opened {
            bcx.notify();
        }
        opened
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
pub(super) fn trade_log(report: &ReportRowMenu) -> MoonMenuItem {
    match &report.trade_log {
        Err(reason) => {
            MoonMenuItem::with_key("rep-row-trade-log", t!(*reason).to_string()).disabled(true)
        }
        Ok(item) => item.clone(),
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
