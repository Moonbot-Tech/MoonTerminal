//! Shared right-click context menu for a market token. It is used wherever a token is shown,
//! including chart order lines and the chart's ban lock, token or strategy cells in Orders, and token cells in Assets,
//! Report and the Screener. The menu selects applicable entries from the context available in [`CoinMenuCtx`]: a
//! strategy (`strat_id`), an order (`order_uid` and `side`), and the source panel filter's selected
//! cores.
//!
//! MoonProto has no add-one-token command for any of these lists. The PERMANENT ones are therefore
//! rewritten whole: each action reads the current list, appends the token or drops it when the coin
//! is already listed, and sends all of it, for the core-wide list via `set_blacklist` and for the
//! strategy list stored in `CoinsBlackList` via `edit_strategies`. Adding a token also enables the
//! core-wide flag; lifting one leaves that flag alone. The TEMPORARY list cannot be written that
//! way — the core adds rows to it itself — so it travels as a delta the feed merges at send time;
//! see [`blacklist`] and `moon_core::feed::CoreCmd::SetTempBlacklist`.
//!
//! Exchanges spell one coin differently (`1kBONK`, `1000BONK`, `BONK`) and a core matches its lists
//! by exact text, so an action over several cores sends each one its own spelling: see [`names`].

use std::rc::Rc;

use gpui::*;
use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonWindowExt as _};
use rust_i18n::t;

use moon_core::session::CoreId;
use moon_core::symbol::coin_list::contains as blacklist_contains;

use crate::Backend;

pub use plan::CoinMenuOrigin;
use plan::{MenuFacts, Order, ReportFacts, Slot};

/// Minimum fitted width preserving the former shared coin-menu footprint.
const MENU_MIN_WIDTH: f32 = 220.0;
/// Maximum fitted width before anomalously long dynamic labels truncate inside the viewport.
const MENU_MAX_WIDTH: f32 = 560.0;

/// Side of the order line at the clicked point. The chart distinguishes Buy from Sell by line type;
/// Buy provides Cancel, while Sell provides Join All Sells and Split actions.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Actions captured from one Report row before a later publication can change its index.
#[derive(Clone)]
pub(crate) struct ReportRowMenu {
    pub(crate) trade_window: MoonMenuItem,
    pub(crate) trade_log: Result<MoonMenuItem, &'static str>,
    pub(crate) filter: Option<MoonMenuItem>,
    /// The focused Report's shortcuts for the available chart and copy actions.
    pub(crate) chart_hint: Option<String>,
    pub(crate) copy_hint: Option<String>,
}

/// Context for the clicked token. Each caller supplies the data available at its location, and the
/// menu uses field presence to determine which entries to show.
///
/// `Clone` because opening a branch REBUILDS this menu — see [`open_coin_menu`].
#[derive(Clone)]
pub struct CoinMenuCtx {
    pub core: CoreId,
    /// Source core name used in labels and the core-blacklist entry.
    pub core_name: String,
    /// Order or token market, such as `ADAUSDT`, used for navigation, joining, and splitting.
    pub market: String,
    /// Base token, such as `ADA`, written to blacklist values.
    pub coin: String,
    /// Cores selected by the source panel's filter for the selected-cores blacklist action. That
    /// entry covers the selected cores that CARRY the coin, each in its own spelling, and appears
    /// only when more than one does. Chart order lines supply `[core]`, so it remains hidden there;
    /// the chart's ban lock supplies its group's scope.
    pub selected_cores: Vec<CoreId>,
    /// Order strategy when `strat_id != 0`, enabling the strategy section. None means manual or join.
    pub strat_id: Option<u64>,
    /// Strategy name used in labels and the scoped Report title. None falls back to the strategy ID.
    pub strat_name: Option<String>,
    /// Order UID enabling the order section. None represents a token point without an order.
    pub order_uid: Option<u64>,
    /// Group whose current workspace scope authorizes mutating actions. Group panels supply their
    /// owner; group-owned chart lines retain that group, while global windows and standalone
    /// reports supply `None`.
    pub workspace_group: Option<String>,
    /// Side for chart lines. A table supplies None, producing Edit and Cancel order actions.
    pub side: Option<OrderSide>,
    /// Order position direction used by `join_sells`.
    pub short: bool,
    pub origin: CoinMenuOrigin,
    /// Report-only durable-history scope. Other producers use the default exact-target history.
    pub history: Option<crate::backend::ChartHistoryScope>,
    /// Report-only trade actions and the filter for the current Report.
    pub(crate) report: Option<ReportRowMenu>,
    /// Screener-only copy actions over the table's captured sorted rows.
    pub(crate) screener_copy: Vec<MoonMenuItem>,
}

impl CoinMenuCtx {
    /// Return the coin as the clicked surface prints it, trimmed.
    ///
    /// A Report row carries its stored coin column in [`Self::history`]; that is what the
    /// Report's coin field matches, and it survives a market that no longer resolves, which
    /// empties [`Self::coin`]. Every other surface prints the base token itself.
    fn shown_coin(&self) -> &str {
        match &self.history {
            Some(crate::backend::ChartHistoryScope::Report { exact_coin, .. }) => exact_coin.trim(),
            _ => self.coin.trim(),
        }
    }
}

/// Opens the shared token context menu at window-coordinate `pos`, usually `event.position`.
///
/// Opens nothing when the context yields no entry at all — an empty popup reads as a bug.
pub fn open_coin_menu(
    ctx: CoinMenuCtx,
    backend: Entity<Backend>,
    pos: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let items = build_items(ctx, &backend, cx);
    if items.is_empty() {
        return;
    }
    open_menu_items("coin-context-menu", pos, items, window, cx);
}

/// Place single-row and multi-row menus in the same fitted Root-owned overlay.
fn open_menu_items(
    menu_id: &'static str,
    position: Point<Pixels>,
    items: Vec<MoonMenuItem>,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_fitted_moon_context_menu(
        cx,
        menu_id,
        position,
        items,
        MENU_MIN_WIDTH,
        MENU_MAX_WIDTH,
    );
}

/// Capture the current row facts, plan its groups, and realize each slot with the shared backend.
fn build_items(ctx: CoinMenuCtx, backend: &Entity<Backend>, cx: &App) -> Vec<MoonMenuItem> {
    let _stall = crate::diag::stall::span(crate::diag::stall::StallKind::Open, "coin-menu");
    let b = backend.read(cx);
    let shown_coin = ctx.shown_coin().to_string();
    let facts = MenuFacts {
        surface: ctx.origin,
        has_market: !ctx.market.is_empty(),
        coin_shown: !shown_coin.is_empty(),
        strategy: ctx.strat_id,
        order: ctx.order_uid.map(|uid| match ctx.side {
            Some(OrderSide::Sell) => Order::Sell(uid),
            Some(OrderSide::Buy) => Order::Buy(uid),
            None => Order::Table(uid),
        }),
        // Use the settings draft when present, matching the split hotkey.
        split_n: b
            .preview
            .as_ref()
            .unwrap_or(&b.config)
            .hotkeys
            .split_n_parts(),
        report: ctx.report.as_ref().map(|report| ReportFacts {
            trade_window: &report.trade_window,
            trade_log: report,
            filter_coin: report.filter.as_ref(),
        }),
        screener_copy: !ctx.screener_copy.is_empty(),
        blacklist: !ctx.coin.is_empty(),
    };

    let mut items = Vec::new();
    for slot in plan::plan(&facts) {
        let emphasized = slot.emphasized();
        let item = match slot {
            Slot::TradeWindow(item) | Slot::FilterHere(item) => item.clone(),
            Slot::ShowOnChart | Slot::OpenOnChart => own::open_chart(&ctx, backend),
            Slot::NotReady => own::chart_not_ready(),
            Slot::TradeLog(report) => own::trade_log(report),
            Slot::OrderEdit(uid) => own::order_edit(&ctx, backend, uid),
            Slot::JoinSells => own::join_sells(&ctx, backend),
            Slot::Split(uid, parts) => own::split_order(&ctx, backend, uid, parts),
            Slot::Cancel(uid) => own::cancel_order(&ctx, backend, uid),
            Slot::ScreenerCopy => {
                items.extend(ctx.screener_copy.iter().cloned());
                continue;
            }
            Slot::CoinReport => coin::coin_report_item(backend, ctx.core, &shown_coin),
            Slot::Compare => coin::compare(&ctx, backend),
            Slot::CopyTicker => {
                let hint = ctx
                    .report
                    .as_ref()
                    .and_then(|report| report.copy_hint.clone());
                with_hint(coin::copy_coin_item(&shown_coin), hint)
            }
            Slot::StrategyGoto(sid) => strategy::goto(&ctx, backend, sid),
            Slot::StrategyReport(sid) => strategy::report(&ctx, backend, sid),
            Slot::Blacklist => {
                // Resolve once so the menu and its dialog use the same spelling for each core.
                let names = Rc::new(names::CoinNames::resolve(b, &ctx));
                blacklist::blacklist_item(&ctx, b, backend, &names)
            }
            Slot::Sep => MoonMenuItem::separator(),
        };
        items.push(if emphasized {
            item.emphasized(true)
        } else {
            item
        });
    }
    items
}

/// Validate every captured mutation target against one current workspace authority snapshot.
///
/// Args:
///     backend: Current terminal and workspace state.
///     group: Group-owned authority, or `None` for an intentionally unscoped caller.
///     cores: Complete target set for the pending action.
///
/// Returns:
///     `true` only when every target remains allowed, preventing partial multi-core writes.
fn workspace_action_allows_cores(backend: &Backend, group: Option<&str>, cores: &[CoreId]) -> bool {
    cores
        .iter()
        .all(|core| backend.workspace_action_allows_core(group, *core))
}

// Blacklist read and write helpers.

/// Returns the current core-wide blacklist as `(enabled, list text)`. A missing settings snapshot
/// yields `(false, "")`.
fn core_blacklist(b: &Backend, core: CoreId) -> (bool, String) {
    b.session
        .store()
        .core(core)
        .and_then(|cd| cd.client_settings.as_ref())
        .map(|cs| (cs.use_blacklist, cs.blacklist_text.clone()))
        .unwrap_or((false, String::new()))
}

/// Writes one token onto or off a core-wide blacklist.
///
/// Adding enables the list so the new entry affects trading: the command sends the flag and text
/// together. Lifting drops only that token and leaves the enable flag unchanged, because turning
/// the whole feature off is a bigger statement than un-listing one coin. A lift that would not
/// change the text is skipped so a checked row cannot pay a no-op settings write.
///
/// Args:
///     b: Session used to read the current list and send `set_blacklist`.
///     core: Core whose list is rewritten.
///     coin: Token to insert or drop, compared case-insensitively.
///     lift: When true, drop `coin`; when false, append it and enable the list.
///
/// Returns:
///     Whether a command was sent. `Ok(false)` is a click that changed nothing.
///
/// Errors:
///     The session refused the write. The warning is logged here, and the caller still sees it.
pub(crate) fn write_core_blacklist(
    b: &Backend,
    core: CoreId,
    coin: &str,
    lift: bool,
) -> anyhow::Result<bool> {
    let result = b.session.write_core_blacklist(core, coin, lift);
    if let Err(err) = &result {
        let action = if lift { "lift" } else { "add" };
        let prep = if lift { "from" } else { "to" };
        log::warn!(
            "coin_menu: {action} {coin} {prep} core {} blacklist failed: {err:#}",
            moon_core::feed::core_label(core)
        );
    }
    result
}

/// Returns the strategy's read-only `CoinsBlackList` snapshot. The server omits fields equal to the
/// schema default, so a missing value is treated as empty and the first token starts a new list.
fn strategy_blacklist(b: &Backend, core: CoreId, sid: u64) -> String {
    b.session
        .store()
        .core(core)
        .and_then(|data| data.strategies.iter().find(|row| row.id == sid))
        .map(|row| places::coins_black_list(row).to_string())
        .unwrap_or_default()
}

/// Writes one token onto or off the strategy's `CoinsBlackList` through the shared field editor.
///
/// The coin menu has no window of its own, so it cannot report a non-clean outcome directly: a
/// successful submission registers a watch instead, and `Shell::drain_strategy_edit_toasts` is
/// the only place that later becomes a toast. A click that would not change the text is skipped.
///
/// Args:
///     b: Backend that sends the field edit and registers the outcome watch.
///     core: Core that owns the strategy.
///     sid: Strategy whose `CoinsBlackList` field is rewritten.
///     coin: Token to insert or drop, compared case-insensitively.
///     lift: When true, drop `coin`; when false, append it.
///
/// Returns:
///     Whether an edit was sent. `Ok(true)` also registers the strategy-edit watch.
///
/// Errors:
///     The session refused the edit. The warning is logged here, and the caller still sees it.
pub(crate) fn write_strategy_blacklist(
    b: &mut Backend,
    core: CoreId,
    sid: u64,
    coin: &str,
    lift: bool,
) -> anyhow::Result<bool> {
    match b.session.write_strategy_blacklist(core, sid, coin, lift) {
        Ok(true) => {
            b.watch_strategy_edit(core, sid, coin.to_string());
            Ok(true)
        }
        Ok(false) => Ok(false),
        Err(err) => {
            let action = if lift { "lift" } else { "add" };
            log::warn!(
                "coin_menu: {action} {coin} {} strategy {sid}@{core} blacklist failed: {err:#}",
                if lift { "from" } else { "to" }
            );
            Err(err)
        }
    }
}

mod blacklist;
mod coin;
pub(crate) mod core_ref;
pub(crate) mod multi;
mod names;
mod own;
mod places;
mod places_dialog;
mod plan;
mod state;
mod strategy;
mod undo;

/// The "where is this coin blacklisted" row.
///
/// The count is the menu's trailing slot. That slot follows the menu font; it is not a
/// per-item monospace run. The dialog is handed the same per-core spellings the count was made
/// with, so the number in the menu and the rows it opens cannot disagree.
fn where_listed_item(
    backend: &Entity<Backend>,
    names: &Rc<names::CoinNames>,
    count: usize,
) -> MoonMenuItem {
    let backend = backend.clone();
    let names = names.clone();
    MoonMenuItem::with_key("coin-bl-where", t!("coin_menu.where_listed").to_string())
        .right_label(count.to_string())
        .on_click(move |_, window, cx| {
            window.close_context_menu(cx);
            places_dialog::open(backend.clone(), (*names).clone(), window, cx);
        })
}

pub(crate) use coin::copy_ticker;
pub(crate) use own::open_chart_target;

/// Attach the muted hint only when a shortcut is available for the row.
pub(crate) fn with_hint(item: MoonMenuItem, hint: Option<String>) -> MoonMenuItem {
    match hint {
        Some(hint) => item.right_label(hint),
        None => item,
    }
}

#[cfg(test)]
mod tests;
