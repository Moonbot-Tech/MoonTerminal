//! Actions scoped to the strategy referenced by the clicked row.

use gpui::Entity;
use moon_ui::{MoonMenuItem, MoonWindowExt as _};
use rust_i18n::t;

use super::CoinMenuCtx;
use crate::Backend;

/// Reveal the clicked strategy within the originating workspace scope.
pub(super) fn goto(ctx: &CoinMenuCtx, backend: &Entity<Backend>, sid: u64) -> MoonMenuItem {
    let core = ctx.core;
    let backend_g = backend.clone();
    let workspace_group = ctx.workspace_group.clone();
    MoonMenuItem::with_key("coin-strat-goto", t!("coin_menu.strategy_goto").to_string()).on_click(
        move |_, window, app| {
            window.close_context_menu(app);
            let owner_display = window.display(app).map(|d| d.id());
            crate::strategies::open_goto(
                backend_g.clone(),
                core,
                sid,
                workspace_group.clone(),
                Some(window.window_handle()),
                owner_display,
                app,
            );
        },
    )
}

/// Open a standalone Report scoped to the clicked strategy.
pub(super) fn report(ctx: &CoinMenuCtx, backend: &Entity<Backend>, sid: u64) -> MoonMenuItem {
    let core = ctx.core;
    let backend_report = backend.clone();
    let strategy_name = ctx.strat_name.clone().unwrap_or_else(|| sid.to_string());
    MoonMenuItem::with_key(
        "coin-strat-report",
        t!("coin_menu.strategy_report").to_string(),
    )
    .right_label(t!("coin_menu.new_report").to_string())
    .on_click(move |_, window, app| {
        window.close_context_menu(app);
        let owner = window.window_handle();
        let owner_display = window.display(app).map(|display| display.id());
        let backend = backend_report.clone();
        let scope = crate::panels::ReportScope::for_strategy(
            moon_core::db::ReportStrategyKey {
                core_uid: core,
                strategy_id: sid as i64,
            },
            strategy_name.clone(),
        );
        // Deferred: clicked inside the standalone Report itself, this window is leased
        // for the click, and the retarget's `handle.update` would fail and open a second
        // Report instead of reusing the singleton.
        app.defer(move |app| {
            crate::panels::open_scoped_report(backend, scope, Some(owner), owner_display, app);
        });
    })
}
