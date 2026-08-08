//! Commands to cores (strategy, trading, and Engine actions) through the per-core `CoreCmd`
//! channel, plus read-only manager accessors (`store`, `sessions`, `market_source`, etc.).

use anyhow::{Result, anyhow};

use crate::data::OrderBookModel;
use crate::feed::{
    ClientSettingsEdit, CoreCmd, FeedWakeTx, LevManageEdit, NewStrategySpec, OrderLinePriceKind,
    OrderStopKind, OrderStopsForm, ResetProfitKind, WalletKind,
};
use crate::market::{MarketDataMode, MarketDataSource};

use super::{CoreId, CoreSession, CoreStore, SessionManager};

impl SessionManager {
    fn send_core_cmd(&self, core: CoreId, cmd: CoreCmd, action: &str) -> Result<()> {
        let Some(s) = self.sessions.iter().find(|s| s.id == core) else {
            return Err(anyhow!("ядро {core} недоступно для команды: {action}"));
        };
        s.handle
            .cmd_tx
            .send(cmd)
            .map_err(|_| anyhow!("канал команд ядра {core} закрыт: {action}"))
    }

    /// Send a strategy action from the Strategies window through the per-core command channel.
    /// This first synchronizes the changed checkboxes in `checks`, then starts or stops the
    /// checked strategies according to `start_stop`. An empty action is a no-op.
    pub fn apply_strategies(
        &self,
        core: CoreId,
        checks: Vec<(u64, bool)>,
        start_stop: Option<bool>,
    ) -> Result<()> {
        if checks.is_empty() && start_stop.is_none() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::StrategiesAction { checks, start_stop },
            "strategies action",
        )
    }

    /// Edit fields for strategies belonging to one core, with one `(id, changes)` entry per
    /// strategy. All edits for the core travel in one command because the feed patches the full
    /// snapshot and sends one `sync_local_strategies`; separate syncs would overwrite each other.
    pub fn edit_strategies(
        &self,
        core: CoreId,
        edits: Vec<(u64, Vec<(String, String)>)>,
    ) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::EditStrategyFields { edits },
            "edit strategies",
        )
    }

    /// Logically delete one core strategy by `id`. The UI enforces the "unchecked only" rule
    /// before calling this method, and retained history allows a later restore under the same ID.
    pub fn delete_strategy(&self, core: CoreId, id: u64) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::DeleteStrategy { id }, "delete strategy")
    }

    /// Request strategy deletion only while its core-wide placement snapshot is unchanged.
    ///
    /// Args:
    ///     core: Core that owns the strategy.
    ///     id: Strategy id to delete.
    ///     expected_placements: Full `(strategy id, raw folder path)` snapshot inspected by the
    ///         caller immediately before queueing deletion.
    ///
    /// Returns:
    ///     Success when the conditional intent entered the core's local command queue.
    pub fn delete_strategy_if_unchanged(
        &self,
        core: CoreId,
        id: u64,
        expected_placements: Vec<(u64, String)>,
    ) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::DeleteStrategyIfUnchanged {
                id,
                expected_placements,
            },
            "delete unchanged strategy",
        )
    }

    /// Delete an entire core folder by path, including any strategies it still contains.
    pub fn delete_folder(&self, core: CoreId, path: String) -> Result<()> {
        if path.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::DeleteFolder { path }, "delete folder")
    }

    /// Request removal of an empty folder while guarding against queued placement changes.
    ///
    /// Args:
    ///     core: Core that owns the folder.
    ///     path: Canonical folder path sent to MoonProto.
    ///     expected_placements: Full `(strategy id, raw folder path)` snapshot inspected by the
    ///         caller after the target strategy disappeared.
    ///
    /// Returns:
    ///     Success when the conditional intent entered the core's local command queue.
    pub fn delete_empty_folder(
        &self,
        core: CoreId,
        path: String,
        expected_placements: Vec<(u64, String)>,
    ) -> Result<()> {
        if path.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::DeleteEmptyFolder {
                path,
                expected_placements,
            },
            "delete empty folder",
        )
    }

    /// Create new core strategies directly or from the clipboard. The feed adds them to the full
    /// set with new IDs and one sync. Send one batch per core.
    pub fn create_strategies(&self, core: CoreId, specs: Vec<NewStrategySpec>) -> Result<()> {
        if specs.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::CreateStrategies { specs },
            "create strategies",
        )
    }

    /// Restore a deleted strategy under its previous ID so version history and order-profit joins
    /// through `strategyid` continue. Fields come from the latest `strat_db` version, and the
    /// restored strategy is unchecked.
    pub fn restore_strategy(
        &self,
        core: CoreId,
        id: u64,
        kind_ordinal: u8,
        folder_path: String,
        fields: Vec<(String, String)>,
    ) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::RestoreStrategy {
                id,
                kind_ordinal,
                folder_path,
                fields,
            },
            "restore strategy",
        )
    }

    /// Change the folder of existing core strategies to rename a folder or move strategies.
    /// Each `moves` entry is `(strategy_id, new_folder_path)`. Send one batch per core.
    pub fn move_strategies(&self, core: CoreId, moves: Vec<(u64, String)>) -> Result<()> {
        if moves.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::MoveStrategies { moves }, "move strategies")
    }

    /// Transfer an asset between wallets of one core through drag and drop in the Assets window.
    /// `qty` is in the base coin; `from` and `to` are Spot, Futures, or Quarterly wallets.
    pub fn transfer_asset(
        &self,
        core: CoreId,
        asset: String,
        qty: f64,
        from: WalletKind,
        to: WalletKind,
    ) -> Result<()> {
        if from == to || asset.is_empty() || !(qty > 0.0) {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::TransferAsset {
                asset,
                qty,
                from,
                to,
            },
            "transfer asset",
        )
    }

    /// Arm or update a chart alert, represented by a drawn object with its Alert option enabled,
    /// on a core market. `blob` is a serialized `TChartObject`; `obj_uid` is the stable object ID.
    pub fn chart_alert_upsert(
        &self,
        core: CoreId,
        market: String,
        obj_uid: u64,
        blob: Vec<u8>,
    ) -> Result<()> {
        if market.is_empty() || obj_uid == 0 || blob.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::ChartAlertUpsert {
                market,
                obj_uid,
                blob,
            },
            "chart alert upsert",
        )
    }

    /// Disarm and delete a chart alert by `obj_uid`.
    pub fn chart_alert_delete(&self, core: CoreId, market: String, obj_uid: u64) -> Result<()> {
        if market.is_empty() || obj_uid == 0 {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::ChartAlertDelete { market, obj_uid },
            "chart alert delete",
        )
    }

    /// Request a fresh list of the core's transferable assets across all wallets.
    pub fn refresh_transfer_assets(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::RefreshTransferAssets,
            "refresh transfer assets",
        )
    }

    /// Permanently convert one core's small balances, or dust, to BNB.
    pub fn convert_dust(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::ConvertDust, "convert dust")
    }

    /// Place a manual order on the core's `market` after its group exit state is confirmed.
    ///
    /// `short` selects the Long or Short position side; `strategy_id=None` maps to `StratID=0` for
    /// an order without a strategy. Non-finite or non-positive `price` or `size` values are ignored.
    pub fn place_order(
        &self,
        core: CoreId,
        market: String,
        short: bool,
        price: f64,
        size: f64,
        strategy_id: Option<u64>,
        exit: crate::config::GroupExitSettings,
    ) -> Result<()> {
        if market.is_empty()
            || !(price.is_finite() && price > 0.0)
            || !(size.is_finite() && size > 0.0)
        {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::PlaceOrder {
                market,
                short,
                price,
                size,
                strategy_id,
                exit,
            },
            "place order",
        )
    }

    /// Move or replace a core order by `uid` at a new price, as when dragging its line.
    /// A non-positive `new_price` is ignored.
    pub fn move_order(&self, core: CoreId, uid: u64, new_price: f64) -> Result<()> {
        if !(new_price > 0.0) {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::MoveOrder { uid, new_price }, "move order")
    }

    /// Cancel a core order by `uid`.
    pub fn cancel_order(&self, core: CoreId, uid: u64) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::CancelOrder { uid }, "cancel order")
    }

    /// Toggle market-level panic sell for a core from the chart button.
    pub fn panic_sell_market(&self, core: CoreId, market: String, on: bool) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::PanicSellMarket { market, on },
            "panic sell market",
        )
    }

    /// Toggle panic sell for one order by `uid`. Dragging a sell line clears this flag before
    /// `move_order`; otherwise the core's panic worker would restore the previous price. Other
    /// orders in the market remain unchanged.
    pub fn turn_order_panic_sell(&self, core: CoreId, uid: u64, on: bool) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::TurnOrderPanicSell { uid, on },
            "turn order panic sell",
        )
    }

    /// Close a core market position at market from the Market Sell button on a position row.
    pub fn market_sell_position(&self, core: CoreId, market: String) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::MarketSellPosition { market },
            "market sell position",
        )
    }

    /// Sell a core market's spot token at market from the Market Sell button on a holding row.
    /// `size` is the quantity in the base coin, usually the full balance.
    pub fn market_sell_token(&self, core: CoreId, market: String, size: f64) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::MarketSellToken { market, size },
            "market sell token",
        )
    }

    /// Cancel pending buy orders for a core market from the Cancel Buy button.
    pub fn cancel_market_buys(&self, core: CoreId, market: String) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::CancelMarketBuys { market },
            "cancel market buys",
        )
    }

    /// Join all sell orders for a market and position side from a sell-line context menu.
    pub fn join_sells(&self, core: CoreId, market: String, short: bool) -> Result<()> {
        if market.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::JoinSells { market, short }, "join sells")
    }

    /// Split the specific sell order selected from a line or row into `parts`.
    /// Values below two are ignored before the command reaches the feed.
    pub fn split_order(&self, core: CoreId, uid: u64, parts: i32) -> Result<()> {
        if parts < 2 {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::SplitOrder { uid, parts }, "split order")
    }

    /// Request a market-level split for a hotkey with no selected order.
    /// The feed sends it only when exactly one active sell order exists; an empty
    /// market or fewer than two parts is ignored here.
    pub fn split_order_for_market(&self, core: CoreId, market: String, parts: i32) -> Result<()> {
        if market.is_empty() || parts < 2 {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::SplitOrderForMarket { market, parts },
            "split order",
        )
    }

    /// Toggle the SL, TS, or VStop flag of one core order by `uid`, as triggered by a cell click in
    /// the Orders table. The feed preserves the configured stop level when re-enabling it.
    pub fn set_order_stop(
        &self,
        core: CoreId,
        uid: u64,
        kind: OrderStopKind,
        on: bool,
    ) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::SetOrderStop { uid, kind, on },
            "set order stop",
        )
    }

    /// Soft-delete (`deleted=true`) or restore (`false`) report rows on `core`, addressed by
    /// `newrecid` ranges and singles from the Report selection commands. An empty selection is a
    /// no-op. The core echoes `ReportEvent::RowsDeleted`, which flips the local replica's
    /// `deleted` flag; the Report panel refreshes from that echo, not optimistically.
    pub fn set_report_rows_deleted(
        &self,
        core: CoreId,
        deleted: bool,
        ranges: Vec<moonproto::ReportRecIdRange>,
        singles: Vec<i64>,
    ) -> Result<()> {
        if ranges.is_empty() && singles.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::SetReportRowsDeleted {
                deleted,
                ranges,
                singles,
            },
            "set report rows deleted",
        )
    }

    /// Soft-delete or restore report rows addressed by a bare list of `newrecid`s.
    ///
    /// The list is folded into ranges plus singles by [`super::compress_rec_ids`] rather than by the
    /// caller: a strategy's whole history is thousands of mostly consecutive ids, and duplicating
    /// that folding at call sites risks inconsistent normalization or ranges widened across gaps.
    /// Centralizing it keeps every caller on the exact-set invariant.
    ///
    /// Args:
    ///     core: Core owning the rows.
    ///     deleted: `true` soft-deletes, `false` restores.
    ///     rec_ids: Rec ids to address; consumed, sorted, and deduplicated.
    ///
    /// Returns:
    ///     `Ok` once the command is queued on the local session channel — never a core
    ///     acknowledgement. An empty list is a no-op.
    pub fn set_report_rows_deleted_ids(
        &self,
        core: CoreId,
        deleted: bool,
        mut rec_ids: Vec<i64>,
    ) -> Result<()> {
        let (ranges, singles) = super::rec_ranges::compress_rec_ids(&mut rec_ids);
        self.set_report_rows_deleted(core, deleted, ranges, singles)
    }

    /// Move an order's stop or take-profit line by dragging it on the chart. The absolute `price`
    /// sets SL or TS to a fixed-price stop, or sets take profit to an absolute price; the other
    /// stops are preserved. A non-positive `price` is ignored.
    pub fn move_order_stop_price(
        &self,
        core: CoreId,
        uid: u64,
        kind: OrderLinePriceKind,
        price: f64,
    ) -> Result<()> {
        if !(price > 0.0) {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::MoveOrderStopPrice { uid, kind, price },
            "move order stop price",
        )
    }

    /// Apply all SL, TS, TP, and VStop edits for an order by `uid` from the Active Order editor,
    /// including enabling, disabling, using a fixed price, or returning to the global level.
    /// Groups set to `None` remain unchanged; an empty form is a no-op.
    pub fn update_order_stops(&self, core: CoreId, uid: u64, form: OrderStopsForm) -> Result<()> {
        if form.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::UpdateOrderStopsForm { uid, form },
            "update order stops",
        )
    }

    /// Apply a targeted `ClientSettings` edit from the toolbar, such as TP, SL, or sell-preset
    /// selection. The feed patches the retained snapshot and sends it to the core in full.
    pub fn edit_client_settings(&self, core: CoreId, edit: ClientSettingsEdit) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::EditClientSettings(edit),
            "edit client settings",
        )
    }

    /// Synchronize every visible manual-exit control to one core without changing core-owned fields.
    pub fn sync_group_exit(
        &self,
        core: CoreId,
        exit: crate::config::GroupExitSettings,
    ) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::SyncGroupExit(exit),
            "sync group exit settings",
        )
    }

    /// Apply a targeted leverage-management edit, such as fixed leverage from the toolbar.
    pub fn edit_lev_manage(&self, core: CoreId, edit: LevManageEdit) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::EditLevManage(edit), "edit lev manage")
    }

    /// Toggle account hedge mode for dual-side positions. This performs a live exchange action.
    pub fn set_hedge_mode(&self, core: CoreId, on: bool) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::SetHedgeMode(on), "set hedge mode")
    }

    /// Set leverage for one core market through the Engine API. This is a live exchange action.
    pub fn set_leverage(&self, core: CoreId, market: String, leverage: i32) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::SetLeverage { market, leverage },
            "set leverage",
        )
    }

    /// Start or restart the core runtime from the core-settings popup.
    pub fn restart_now(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::RestartNow, "restart now")
    }

    /// Reset the core's session or all-time profit counter.
    pub fn reset_profit(&self, core: CoreId, kind: ResetProfitKind) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::ResetProfit(kind), "reset profit")
    }

    /// Cancel every order for the core. This is a live exchange action.
    pub fn cancel_all_orders(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::CancelAllOrders, "cancel all orders")
    }

    /// Set the core's coin-blacklist state and text.
    pub fn set_blacklist(&self, core: CoreId, on: bool, text: String) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::SetBlacklist { on, text }, "set blacklist")
    }

    /// Locally exclude blacklisted coins from the core's Active Lib market-delta calculation.
    pub fn set_exclude_blacklisted_delta(&self, core: CoreId, on: bool) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::SetExcludeBlacklistedDelta(on),
            "set exclude blacklisted delta",
        )
    }

    /// Return read-only access to account-plane state such as statuses, orders, detects, and
    /// strategies. Only the manager mutates the store.
    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    /// Live core sessions in `AppConfig::servers` order, independent of connection timing.
    ///
    /// This is raw config order; the UI applies the selected `CoreSortMode` separately.
    pub fn sessions(&self) -> &[CoreSession] {
        &self.sessions
    }

    /// Return the core account's base currency, such as `USDT` or `BTC`, once `CoreBase` has
    /// identified it. The UI uses this currency for group-USD-to-base order-size conversion.
    pub fn core_base(&self, core: CoreId) -> Option<&str> {
        self.core_base.get(&core).map(String::as_str)
    }

    pub fn feed_wake(&self) -> Option<FeedWakeTx> {
        self.feed_wake.clone()
    }

    /// Read the order-book view for `core` and `market` after resolving the core's provider.
    /// History and last price use the retained-history API instead.
    pub fn with_orderbook_view<R>(
        &self,
        core: CoreId,
        market: &str,
        f: impl FnOnce(Option<(&OrderBookModel, u64)>) -> R,
    ) -> R {
        self.market_source.with_orderbook_view(core, market, f)
    }

    pub fn market_source(&self) -> MarketDataSource {
        self.market_source.clone()
    }

    /// Switch the market-data source mode from Settings. An actual change clears the entire market
    /// plan so providers, served markets, and data are rebuilt from scratch by the next `set_open`.
    /// Clearing `last_cmd` ensures that existing cores receive fresh roles.
    pub fn set_market_mode(&mut self, mode: MarketDataMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.market_source.clear();
        self.providers.clear();
        self.core_provider.clear();
        self.wanted.clear();
        self.pending_drop.clear();
        self.last_cmd.clear();
    }
}
