//! Navigation, reporting and clipboard actions for the clicked ticker.

use gpui::{ClipboardItem, Entity};
use moon_ui::{MoonMenuItem, MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

use moon_core::session::CoreId;

use super::CoinMenuCtx;
use crate::Backend;

/// Open the market in Compare using the originating workspace authority.
pub(super) fn compare(ctx: &CoinMenuCtx, backend: &Entity<Backend>) -> MoonMenuItem {
    let core = ctx.core;
    let backend_cmp = backend.clone();
    let market = ctx.market.clone();
    let workspace_group = ctx.workspace_group.clone();
    MoonMenuItem::with_key("coin-compare", t!("coin_menu.compare").to_string()).on_click(
        move |_, window, app| {
            window.close_context_menu(app);
            backend_cmp.update(app, |b, bcx| {
                if b.open_compare_if_authorized(workspace_group.as_deref(), (core, market.clone()))
                {
                    bcx.notify();
                }
            });
        },
    )
}

/// The "copy the ticker" entry: the coin cell is not selectable text, so this is how a ticker
/// reaches the Report's coin field, a strategy coin list or a message.
///
/// The clipboard gets exactly the printed coin — no blacklist spelling, no market key, no
/// keyboard-layout mapping (`龙虾` stays `龙虾`). The ticker is not a secret, so the toast may
/// name it; it is deliberately not logged.
///
/// Args:
///     coin: The coin as the clicked surface prints it, trimmed and non-empty.
///
/// Returns:
///     The menu entry whose toast names the copied ticker.
pub(super) fn copy_coin_item(coin: &str) -> MoonMenuItem {
    let coin = coin.to_string();
    MoonMenuItem::with_key("coin-copy", t!("coin_menu.copy_coin").to_string()).on_click(
        move |_, window, app| {
            window.close_context_menu(app);
            app.write_to_clipboard(ClipboardItem::new_string(coin.clone()));
            window.push_notification(
                MoonNotification::success(
                    t!("coin_menu.coin_copied", coin = coin.as_str()).to_string(),
                ),
                app,
            );
        },
    )
}

/// The "every trade of this coin" entry, scoped to the clicked core.
///
/// It opens or retargets the same singleton standalone Report the strategy entry uses — all
/// time, every strategy, the clicked row's one core, the exact ticker — and leaves the report
/// the user clicked in untouched.
///
/// Args:
///     backend: Shared backend holding the singleton Report window.
///     core: Core of the clicked row; the same ticker on another core is another market.
///     coin: The coin as the clicked surface prints it, trimmed and non-empty.
///
/// Returns:
///     The menu entry labeled with the clicked ticker.
pub(super) fn coin_report_item(
    backend: &Entity<Backend>,
    core: CoreId,
    coin: &str,
) -> MoonMenuItem {
    let backend = backend.clone();
    let coin = coin.to_string();
    MoonMenuItem::with_key(
        "coin-coin-report",
        t!("coin_menu.coin_report", coin = coin.as_str()).to_string(),
    )
    .right_label(t!("coin_menu.new_report").to_string())
    .on_click(move |_, window, app| {
        window.close_context_menu(app);
        let owner = window.window_handle();
        let owner_display = window.display(app).map(|display| display.id());
        let backend = backend.clone();
        let scope = crate::panels::ReportScope::for_coin(core, &coin);
        // Deferred for the same reason as the strategy entry: a click inside the standalone
        // Report leases that window, and a nested retarget would open a second one.
        app.defer(move |app| {
            crate::panels::open_scoped_report(backend, scope, Some(owner), owner_display, app);
        });
    })
}
