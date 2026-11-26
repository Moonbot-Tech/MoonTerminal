//! Commands to cores (strategy, trading, and Engine actions) through the per-core `CoreCmd`
//! channel, plus read-only manager accessors (`store`, `sessions`, `market_source`, etc.).

use std::collections::HashMap;
use std::time::{Duration, Instant};

mod temp_bans;
pub(super) use temp_bans::TempBanIntents;

/// How long a blacklist this process sent may stand in for the core's echo of it; well past the
/// round trip. Past it the store is trusted even if the echo never came.
const BLACKLIST_ECHO_WAIT: Duration = Duration::from_secs(15);

/// The latest accepted list and the snapshots that can still precede its echo.
#[derive(Clone, Debug)]
pub(super) struct SentBlacklist {
    before: Vec<(Instant, BlacklistValue)>,
    sent: BlacklistValue,
    at: Instant,
    epoch: Instant,
    generation: u64,
    token_generations: HashMap<String, u64>,
    flag_generation: u64,
}

/// The switch and text travel together. Strategy lists are always enabled.
type BlacklistValue = (bool, String);

/// The exact inserted tokens and their shared inverse for a bulk click.
type BlacklistBatchUndo<F> = (Vec<String>, F);

impl SentBlacklist {
    /// Recognize both the initial snapshot and an intermediate echo of our writes.
    fn pending(&self, in_store: &BlacklistValue) -> bool {
        self.at.elapsed() < BLACKLIST_ECHO_WAIT
            && self.sent != *in_store
            && self
                .before
                .iter()
                .any(|(at, value)| at.elapsed() < BLACKLIST_ECHO_WAIT && value == in_store)
    }

    /// Use the newest accepted edit while its predecessor is still published.
    fn effective<'a>(&'a self, in_store: &'a BlacklistValue) -> &'a BlacklistValue {
        if self.pending(in_store) {
            &self.sent
        } else {
            in_store
        }
    }

    /// Register a write and invalidate ownership only where it changed a token or the switch.
    fn record(
        previous: Option<Self>,
        in_store: BlacklistValue,
        sent: BlacklistValue,
        strategy: bool,
        writes_flag: bool,
    ) -> Self {
        let now = Instant::now();
        let Some(mut entry) = previous.filter(|entry| entry.at.elapsed() < BLACKLIST_ECHO_WAIT)
        else {
            return Self {
                before: vec![(now, in_store)],
                sent,
                at: now,
                epoch: now,
                generation: 1,
                token_generations: HashMap::new(),
                flag_generation: 1,
            };
        };
        entry.observe(&in_store, strategy);
        let generation = entry.generation + 1;
        let flag_generation = if writes_flag || entry.sent.0 != sent.0 {
            generation
        } else {
            entry.flag_generation
        };
        entry.retain_token_owners(&sent.1, strategy);
        let before = if entry.pending(&in_store) {
            entry
                .before
                .retain(|(at, _)| at.elapsed() < BLACKLIST_ECHO_WAIT);
            entry.before.push((now, entry.sent));
            entry.before
        } else {
            vec![(now, in_store)]
        };
        Self {
            before,
            sent,
            at: now,
            epoch: entry.epoch,
            generation,
            token_generations: entry.token_generations,
            flag_generation,
        }
    }

    /// Once the latest write echoes, a later snapshot owns its own state.
    fn observe(&mut self, published: &BlacklistValue, strategy: bool) {
        if !self.pending(published) {
            self.before.clear();
            if *published != self.sent {
                self.generation += 1;
                if published.0 != self.sent.0 {
                    self.flag_generation = self.generation;
                }
                self.retain_token_owners(&published.1, strategy);
                self.sent = published.clone();
            }
        }
    }

    /// A token that disappears has lost its original owner even if it is later added again.
    fn retain_token_owners(&mut self, next: &str, strategy: bool) {
        let contains = if strategy {
            crate::symbol::coin_list::strategy_contains
        } else {
            crate::symbol::coin_list::contains
        };
        self.token_generations
            .retain(|coin, _| contains(next, coin));
    }
}

/// The list mutation sent by one click, with ownership per inserted token.
struct BlacklistAdd {
    coins: Vec<String>,
    before: String,
    after: String,
    enabled_flag: bool,
    generation: u64,
    at: Instant,
    epoch: Instant,
}

impl BlacklistAdd {
    /// Restore exact bytes only when doing so cannot overwrite another pending write.
    fn revert(
        &self,
        entry: &SentBlacklist,
        in_store: &BlacklistValue,
        strategy: bool,
    ) -> BlacklistValue {
        let current = entry.effective(in_store);
        let owned = |coin: &String| {
            entry.token_generations.get(&coin.to_ascii_uppercase()) == Some(&self.generation)
        };
        if self.epoch != entry.epoch || self.at.elapsed() >= BLACKLIST_ECHO_WAIT {
            return current.clone();
        }
        let text = if current.1 == self.after
            && (!entry.pending(in_store) || entry.generation == self.generation)
            && self.coins.iter().all(owned)
        {
            self.before.clone()
        } else {
            self.coins
                .iter()
                .filter(|coin| owned(coin))
                .fold(current.1.clone(), |text, coin| {
                    if strategy {
                        crate::symbol::coin_list::strategy_remove(&text, coin)
                    } else {
                        crate::symbol::coin_list::remove(&text, coin)
                    }
                })
        };
        let disable =
            self.enabled_flag && entry.flag_generation <= self.generation && text == self.before;
        (current.0 && !disable, text)
    }
}

/// Compute a token edit and report whether it inserted a new entry.
fn blacklist_token_edit(
    (enabled, base): &BlacklistValue,
    coin: &str,
    lift: bool,
    strategy: bool,
) -> (BlacklistValue, bool) {
    let after = if strategy {
        crate::symbol::coin_list::strategy_edit(base, coin, lift)
    } else {
        crate::symbol::coin_list::edit(base, coin, lift)
    };
    let inserted = !lift && after != *base;
    ((*enabled || !lift, after), inserted)
}

use anyhow::{Result, anyhow};

use crate::account::AccountIdentity;
use crate::config::{MoveGestureCommand, MoveKind};
use crate::data::OrderBookModel;
use crate::feed::{
    ClientSettingsEdit, CoreCmd, CoreConfig, FeedWakeTx, FieldMask, NewStrategySpec,
    OrderLinePriceKind, OrderStopKind, OrderStopsForm, ResetProfitKind, TelegramCmd, UpdateTarget,
    WalletKind, WatchdogCmd,
};
use crate::market::{MarketDataMode, MarketDataSource};
use crate::venue::{AccountMergeKey, CoreVenue};

use super::{CoreId, CoreSession, CoreStore, SessionManager};

/// Longest test-diagnostic text the core will keep.
///
/// The core's signal buffer retains at most 200 characters in a legacy encoding — a protocol fact,
/// stated once here rather than trusted to each caller. Text past it is dropped by the core, so a
/// silent clamp on this side is the honest translation of a limit we do not own.
const TEST_PROBLEM_MAX_CHARS: usize = 200;

/// Whether a manual order's own inputs can become a live command at all.
///
/// Shared by the immediate and the pending entry points because a tightened rule that reached only
/// one of them would leave the other placing what the first refuses. Says nothing about SIZE units
/// or about the price being sane for the market — those belong to the callers that know them.
fn order_inputs_ok(market: &str, price: f64, size: f64) -> bool {
    !market.is_empty() && price.is_finite() && price > 0.0 && size.is_finite() && size > 0.0
}

impl SessionManager {
    /// Queue a typed command without exposing the transport handle to other application layers.
    pub(crate) fn send_core_cmd(&self, core: CoreId, cmd: CoreCmd, action: &str) -> Result<()> {
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

    /// Start or stop one core's GLOBAL strategy engine — Moonbot's own Start/Stop, which the core
    /// reports back as `strategies_running`.
    ///
    /// A named operation rather than an [`Self::apply_strategies`] call at each site: this one
    /// carries NO checkbox edits of its own, so a run control cannot rewrite which strategies the
    /// core has selected as a side effect of pressing Start. What it cannot prevent is the
    /// protocol's own shape — `TStratStartStopCommandV2` always ships MoonProto's outstanding
    /// checked delta alongside the action (moonproto `events/local_strats.rs`), exactly as
    /// Moonbot's own Start does. That delta is whatever the Strategies window already changed and
    /// the core has not echoed yet; nothing here produces one.
    ///
    /// The command is an intent — the core answers with a new runtime state, which is what any
    /// indicator must follow.
    ///
    /// Args:
    ///     core: Core to command.
    ///     on: Whether to start (`true`) or stop (`false`) trading.
    ///
    /// Returns:
    ///     Whether the command reached the core's channel.
    pub(crate) fn set_trading(&self, core: CoreId, on: bool) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::StrategiesAction {
                checks: Vec::new(),
                start_stop: Some(on),
            },
            "set trading",
        )
    }

    /// Start or stop trading on a whole scope — a saved group, an exchange row, a selection.
    ///
    /// One command per core, because that is what the protocol offers; the loop lives here so the
    /// windows that act on a group cannot each invent their own error handling. A core whose
    /// command channel is gone is skipped and named in the log rather than aborting the scope:
    /// with one core of six down, the user asked for the other five to stop.
    ///
    /// Returns WHICH cores accepted it, not how many. A caller that shows "waiting for the core"
    /// must arm exactly the cores it reached — arming the rest leaves controls waiting on an
    /// answer to a command nobody sent.
    ///
    /// Args:
    ///     cores: Cores to command.
    ///     on: Whether to start (`true`) or stop (`false`) trading.
    ///
    /// Crate-private: every caller outside `session` goes through [`Self::dispatch_run`], which
    /// keeps a command away from a core that is not connected.
    ///
    /// Returns:
    ///     The cores whose command channel accepted the intent, in the order given.
    pub(crate) fn set_trading_many(&self, cores: &[CoreId], on: bool) -> Vec<CoreId> {
        self.send_many(cores, &format!("set trading {on}"), |core| {
            self.set_trading(core, on)
        })
    }

    /// Send one scope command per core and report which channels accepted it.
    ///
    /// The shared half of every "act on a whole scope" operation: a core whose command channel is
    /// gone is skipped and named in the log rather than aborting the scope, and the caller gets
    /// back exactly the cores it may now show as waiting.
    ///
    /// Args:
    ///     cores: Cores to command, in the order the caller wants them tried.
    ///     action: Log label for a core that could not be reached.
    ///     send: The per-core command.
    ///
    /// Returns:
    ///     The cores whose command channel accepted the intent, in the order given.
    fn send_many(
        &self,
        cores: &[CoreId],
        action: &str,
        send: impl Fn(CoreId) -> Result<()>,
    ) -> Vec<CoreId> {
        let mut sent = Vec::with_capacity(cores.len());
        for core in cores {
            match send(*core) {
                Ok(()) => sent.push(*core),
                Err(error) => log::warn!("{action} on core {core} failed: {error:#}"),
            }
        }
        sent
    }

    /// Edit fields for strategies belonging to one core, with one `(id, changes)` entry per
    /// strategy. All edits for the core travel in one command because the feed patches the full
    /// snapshot and sends one `sync_local_strategies`; separate syncs would overwrite each other.
    pub fn edit_strategies(
        &self,
        core: CoreId,
        edits: Vec<(u64, Vec<(String, String)>)>,
    ) -> Result<()> {
        self.edit_strategies_with_order_refresh(core, edits, false)
    }

    /// Send one edit batch, optionally requesting a standing MoonShot BUY refresh with it.
    ///
    /// `apply_to_orders` belongs only to this submission. Queue acceptance and subsequent edit
    /// confirmation concern settings, not order cancellation or replacement.
    pub fn edit_strategies_with_order_refresh(
        &self,
        core: CoreId,
        edits: Vec<(u64, Vec<(String, String)>)>,
        apply_to_orders: bool,
    ) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        let lists: Vec<_> = edits
            .iter()
            .filter_map(|(id, fields)| {
                fields
                    .iter()
                    .rfind(|(name, _)| name == crate::feed::FIELD_COINS_BLACK_LIST)
                    .map(|(_, text)| ((core, Some(*id)), text.clone()))
            })
            .collect();
        if lists.is_empty() {
            return self.send_strategy_fields(core, edits, apply_to_orders);
        }
        let mut sent = self
            .blacklists_sent
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let snapshots: Vec<_> = lists
            .into_iter()
            .map(|(key, text)| Ok((key, self.blacklist_snapshot(key)?, text)))
            .collect::<Result<_>>()?;
        self.send_strategy_fields(core, edits, apply_to_orders)?;
        for (key, in_store, text) in snapshots {
            let entry =
                SentBlacklist::record(sent.remove(&key), in_store, (true, text), true, false);
            sent.insert(key, entry);
        }
        Ok(())
    }

    /// Send strategy fields after the caller has coordinated any blacklist tracking.
    fn send_strategy_fields(
        &self,
        core: CoreId,
        edits: Vec<(u64, Vec<(String, String)>)>,
        apply_to_orders: bool,
    ) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::EditStrategyFields {
                edits,
                apply_to_orders,
            },
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

    /// Delete an EMPTY core folder by path.
    ///
    /// `TStratDelete(0, path)` removes a folder only when it holds no strategy; the core answers a
    /// populated folder with a log line and no event. A caller removing a populated folder deletes
    /// its rows first, through this same per-core queue, so the folder command lands after them.
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
    ///
    /// Args:
    ///     core: Core owning the strategies.
    ///     moves: `(strategy id, new folder path)` per strategy. Send one batch per core.
    ///     rebase: The folder subtree this move renames or reparents, as `(old path, new path)`.
    ///         A rename or a folder drag must pass one, or the emptied old path stays behind on the
    ///         core as a folder of its own — see [`CoreCmd::MoveStrategies`].
    ///
    /// Returns:
    ///     Success once the intent entered that core's command queue. A call with neither moves nor
    ///     a rebase asks for nothing and is dropped here.
    pub fn move_strategies(
        &self,
        core: CoreId,
        moves: Vec<(u64, String)>,
        rebase: Option<(String, String)>,
    ) -> Result<()> {
        // A folder holding no strategy has no moves to carry it, and its rename is still a real
        // edit — so emptiness alone is not the test.
        if moves.is_empty() && rebase.is_none() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::MoveStrategies { moves, rebase },
            "move strategies",
        )
    }

    /// Create one folder on a core, holding nothing.
    ///
    /// Args:
    ///     core: Core to create it on.
    ///     path: Canonical folder path.
    ///
    /// Returns:
    ///     Success once the intent entered that core's command queue. Whether the core can hold an
    ///     empty folder at all is decided on the feed thread, which logs what it declines; callers
    ///     read `CoreFolders::editable` to decide what to PROMISE the operator, not to gate this.
    pub fn add_core_folder(&self, core: CoreId, path: String) -> Result<()> {
        if path.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::AddFolder { path }, "add folder")
    }

    /// Remove one folder and its whole subtree from a core's tree.
    ///
    /// Args:
    ///     core: Core to remove it from.
    ///     path: Canonical folder path.
    ///
    /// Returns:
    ///     Success once the intent entered that core's command queue. Strategies inside are NOT
    ///     deleted by this — the core keeps a folder that still holds one — so a caller removing a
    ///     populated folder deletes its rows first.
    pub fn remove_core_folder(&self, core: CoreId, path: String) -> Result<()> {
        if path.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::RemoveFolder { path }, "remove folder")
    }

    /// Rearrange a core's strategy list into `order`, the complete desired id sequence.
    ///
    /// The core confirms by echoing a full strategy snapshot in that sequence; nothing here waits
    /// for it. See [`CoreCmd::ReorderStrategies`] for what the feed does with a list that raced
    /// with a create or a delete.
    ///
    /// Args:
    ///     core: Core whose list is being rearranged.
    ///     order: Every strategy id of that core, in the desired order.
    ///
    /// Returns:
    ///     Success once the intent entered that core's command queue.
    ///
    /// Deliberately without a "too short to matter" early return: one that answered `Ok(())`
    /// without queueing anything would hand the caller a success for a command that does not
    /// exist, and the caller draws an unconfirmed arrangement on the strength of it. Whether an
    /// order says anything is the feed's decision, taken against the list it actually holds.
    pub fn reorder_strategies(&self, core: CoreId, order: Vec<u64>) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::ReorderStrategies { order },
            "reorder strategies",
        )
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

    /// Ask the core for strategy-filter chart text on `market`, or clear the relay.
    ///
    /// One market per core: the protocol replaces the previous request. Empty `market` or
    /// `need_filters=false` turns the overlay off for this client.
    pub fn set_chart_text(&self, core: CoreId, market: String, need_filters: bool) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::SetChartText {
                market,
                need_filters,
            },
            "set chart text",
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
    /// `short` selects the Long or Short position side; `strategy_id=None` maps to `StratID=0`,
    /// which the core reads as "substitute my own manual strategy if that mode is on" rather than
    /// as "no strategy" — there is no way to spell a deliberate absence on the wire. Non-finite or
    /// non-positive `price` or `size` values are ignored.
    pub fn place_order(
        &self,
        core: CoreId,
        market: String,
        short: bool,
        price: f64,
        size: f64,
        strategy_id: Option<u64>,
        exit: crate::config::GroupExitSettings,
        planned_sell: f64,
        sync_exit: bool,
    ) -> Result<()> {
        if !order_inputs_ok(&market, price, size) {
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
                planned_sell,
                sync_exit,
            },
            "place order",
        )
    }

    /// Place a pending order on the core's `market` after its group exit state is confirmed.
    ///
    /// `trigger_price` is the CONDITION the core watches, not the entry price: it prices the order
    /// itself when the trigger fires, applying its own pending spread. Nothing derived from the
    /// trigger is sent, which is why there is no `planned_sell` here — the exits come from the
    /// confirmed `exit` generation or from `strategy_id`, both computed by the core from the real
    /// entry. `strategy_id=None` creates a BARE pending: the core does not substitute its own manual
    /// strategy the way it may for [`Self::place_order`]. Non-finite or non-positive `trigger_price`
    /// or `size` values are ignored.
    #[allow(clippy::too_many_arguments)]
    pub fn place_pending_order(
        &self,
        core: CoreId,
        market: String,
        short: bool,
        trigger_price: f64,
        size: f64,
        strategy_id: Option<u64>,
        exit: crate::config::GroupExitSettings,
        sync_exit: bool,
    ) -> Result<()> {
        if !order_inputs_ok(&market, trigger_price, size) {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::PlacePendingOrder {
                market,
                short,
                trigger_price,
                size,
                strategy_id,
                exit,
                sync_exit,
            },
            "place pending order",
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

    /// Force the cancellation of a stuck core order by `uid` (MoonBot 7.73 N2+).
    ///
    /// Not a stronger [`Self::cancel_order`]: after three unconfirmed checks the core stops
    /// tracking the order and keeps cancelling it in the background, so it may stay open on the
    /// exchange. Callers ask the user first and offer it only where
    /// `CoreFeatures::force_cancel_gate` returns `Offered`. `Ok` means the command was queued for
    /// the core, not that anything was cancelled.
    pub fn force_cancel_order(&self, core: CoreId, uid: u64) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::ForceCancelOrder { uid },
            "force cancel order",
        )
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

    /// Re-place the closing order of a core market position from the Order button on a position
    /// row: the core cancels its own sells on the market and places a limit close for the whole
    /// position. Same command as [`Self::market_sell_position`] without the market flag.
    pub fn limit_close_position(&self, core: CoreId, market: String) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::LimitClosePosition { market },
            "limit close position",
        )
    }

    /// Sell a core market's spot token at market from the Market Sell button on a holding row.
    ///
    /// `qty` is the coin quantity, usually the full balance, and `price` the market's own last
    /// price: the wire carries the order size in the account's balance currency, so the feed
    /// multiplies the two. Passing a price from another market silently resizes the order.
    pub fn market_sell_token(
        &self,
        core: CoreId,
        market: String,
        qty: f64,
        price: f64,
    ) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::MarketSellToken { market, qty, price },
            "market sell token",
        )
    }

    /// Sell a core market's spot token with a limit order at `price`, from OK in the Assets
    /// holding row's `Order` dialog. `qty` is the coin quantity; both must be finite and positive
    /// or the feed sends nothing.
    pub fn limit_sell_token(
        &self,
        core: CoreId,
        market: String,
        qty: f64,
        price: f64,
    ) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::LimitSellToken { market, qty, price },
            "limit sell token",
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

    /// Shift a market's live orders of one side by `percent` of their price.
    ///
    /// `percent` is WHOLE percent and signed: positive moves up, negative down. Zero, a non-finite
    /// value, an empty market or a magnitude no sane hotkey can mean sends nothing — this is a bulk
    /// move of live orders, and the number reaches the core unchecked otherwise.
    pub fn shift_orders_percent(
        &self,
        core: CoreId,
        market: String,
        sell: bool,
        percent: f64,
    ) -> Result<()> {
        if market.is_empty() || !percent.is_finite() || percent == 0.0 || percent.abs() > 100.0 {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::ShiftOrdersPercent {
                market,
                sell,
                percent,
            },
            "shift orders percent",
        )
    }

    /// Move a market's orders onto a clicked price — Moonbot's Move Open / Move TP.
    ///
    /// One press can be bound to several move rows; `moves` is all of them, in the order they
    /// answer, and the feed sends the first whose leg holds an order to move (see
    /// `CoreCmd::MoveOrdersToPrice`). The core picks the orders and lays them out according to the
    /// chosen row's kind; nothing here computes a destination for an individual order. A market
    /// with no name, a price that is not a finite positive number, or a list left empty once its
    /// inert `MoveKind::None` rows are dropped sends nothing — a bulk move is the one command on
    /// this list that can rewrite every order of a side at once, so a malformed one is dropped here
    /// rather than handed to the core to interpret.
    ///
    /// Args:
    ///     core: Core the market belongs to.
    ///     market: Market whose orders are addressed.
    ///     price: Destination price, taken from where the pointer was.
    ///     moves: The rows the press claimed, each with its half of the book, Moonbot's
    ///         "Move kind" and the position side, in dispatch order.
    ///     hedge_short: Whether the caller reads the market's position as short. Used only for a
    ///         row bound to both sides on a market that holds its leg on both.
    pub fn move_orders_to_price(
        &self,
        core: CoreId,
        market: String,
        price: f64,
        moves: Vec<MoveGestureCommand>,
        hedge_short: bool,
    ) -> Result<()> {
        let moves: Vec<MoveGestureCommand> = moves
            .into_iter()
            .filter(|command| command.kind != MoveKind::None)
            .collect();
        if market.is_empty() || !price.is_finite() || price <= 0.0 || moves.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::MoveOrdersToPrice {
                market,
                price,
                moves,
                hedge_short,
            },
            "move orders to price",
        )
    }

    /// Spread a market's sell orders across a price zone — Moonbot's "sells to rectangle".
    ///
    /// The two prices arrive in whatever order the rectangle was drawn and are ordered here, so the
    /// wire always carries min then max. A degenerate zone — one price, a non-finite or
    /// non-positive bound, or an empty market — sends nothing: the core would spread every sell
    /// onto a single level, which no drag of a rectangle can have meant.
    pub fn sells_to_zone(
        &self,
        core: CoreId,
        market: String,
        a: f64,
        b: f64,
        short: bool,
    ) -> Result<()> {
        let (min_price, max_price) = if a <= b { (a, b) } else { (b, a) };
        if market.is_empty()
            || !(min_price.is_finite() && max_price.is_finite())
            || min_price <= 0.0
            || max_price <= min_price
        {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::SellsToZone {
                market,
                min_price,
                max_price,
                short,
            },
            "sells to zone",
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

    /// Ask `core` for the archived order traces of one closed report row.
    ///
    /// The answer lands in `CoreData::report_traces` under the same `report_uid`, observed through
    /// `report_traces_rev`; a core that never answers is turned into a failed outcome by MoonProto's
    /// own timeout. Sending to a core that is not connected fails here, like every other command.
    ///
    /// Args:
    ///     core: Core that recorded the trade.
    ///     report_uid: The row's `ReportUID` — never `newrecid` or an order uid.
    ///
    /// Returns:
    ///     `Ok` once the command is queued on the local session channel.
    pub fn request_report_traces(&self, core: CoreId, report_uid: i64) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::RequestReportTraces { report_uid },
            "request report traces",
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

    /// Write a core's safe-share configuration, restricted to the areas `touched` names.
    ///
    /// The feed rebuilds the packet from the core's freshest retained snapshot and waits for its
    /// echo before sending another, so a caller may issue this per OK press or per toolbar edit
    /// without tracking the core's own state. `config` is still the COMPLETE projection — `touched`
    /// only says which of its fields this edit actually asked to change; see
    /// [`crate::feed::FieldMask`]. The gear popup passes [`FieldMask::RENDERED_SECTIONS`] so an OK
    /// can never reach the manual block, checkbox on or off.
    pub fn edit_core_config(
        &self,
        core: CoreId,
        config: CoreConfig,
        touched: FieldMask,
    ) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::EditCoreConfig { config, touched },
            "edit core config",
        )
    }

    /// Mark one market in the core's favourites, or take it out.
    ///
    /// Absolute, never a toggle: the state is decided where the trader pressed it, and this write
    /// may reach the core several round trips later — see `feed::fav_markets_set`. The list is
    /// rebuilt from the core's own snapshot at send time, so a market MoonBot marked in between
    /// survives.
    ///
    /// Args:
    ///     core: Core that holds the list.
    ///     market: Market as that core spells it.
    ///     on: Whether it must be listed afterwards.
    pub fn set_fav_market(&self, core: CoreId, market: String, on: bool) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::SetFavMarket { market, on }, "set fav market")
    }

    /// Request a fresh safe-share configuration read directly from the core, bypassing the
    /// runtime's own background poll. The answer arrives as the usual `FeedMsg::CoreConfig`
    /// full-snapshot echo.
    pub fn refresh_shared_config(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::RefreshSharedConfig, "refresh shared config")
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

    /// Ask a core to install a release or a named beta/test build.
    ///
    /// This is an INTENT with no acknowledgement: the command channel accepting it proves only
    /// that it was sent, never that the update happened. The caller observes completion through
    /// the core's reported version and its connection-episode counter, never through this
    /// function's `Ok`.
    pub fn update_core_version(&self, core: CoreId, target: UpdateTarget) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::UpdateVersion { target }, "update version")
    }

    /// Ask one core to publish a TEST diagnostic, proving the confirmed-diagnostics channel works.
    ///
    /// The text is clamped HERE rather than at the call site: the core's signal buffer keeps at
    /// most 200 characters in a legacy encoding, and that is a fact about the protocol, not about
    /// whichever button happens to send it. Non-ASCII is rejected for the same reason — the buffer
    /// is not UTF-8, so a Cyrillic caption would arrive as mojibake and the row would look like a
    /// bug in this terminal.
    ///
    /// The test LEAVES A ROW on the core until [`Self::clear_core_problems`] removes it. Callers
    /// that offer one button must offer the other.
    ///
    /// Args:
    ///     core: Core to command.
    ///     text: Short ASCII label identifying who asked.
    ///
    /// Returns:
    ///     Whether the command reached the core's channel; `Err` for text the wire cannot carry.
    pub fn test_core_problem(&self, core: CoreId, text: &str) -> Result<()> {
        // Printable ASCII only. `is_ascii()` alone admits NUL, ESC and CR/LF, which land in a
        // legacy-encoded buffer on the core and come back as a row nobody can read — the same class
        // of defect the INBOUND projection filters, applied on the way out.
        let text = text.trim();
        if text.is_empty() || !text.chars().all(|c| c.is_ascii() && !c.is_control()) {
            return Err(anyhow!(
                "тестовая диагностика ядра {core}: нужен непустой печатный ASCII-текст"
            ));
        }
        let text: String = text.chars().take(TEST_PROBLEM_MAX_CHARS).collect();
        self.send_core_cmd(core, CoreCmd::TestProblem { text }, "test problem")
    }

    /// Clear every confirmed diagnostic and pending hypothesis on one core.
    ///
    /// IRREVERSIBLE and fleet-visible: the facts are dropped for every terminal watching that core,
    /// and no cause is fixed by dropping them. A caller must confirm with the operator first, and
    /// must not clear its own rows optimistically — the core's next full list is the answer.
    ///
    /// Args:
    ///     core: Core whose diagnostics are dropped.
    ///
    /// Returns:
    ///     Whether the command reached the core's channel.
    pub fn clear_core_problems(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::ClearProblems, "clear problems")
    }

    /// Clear the diagnostics of a whole scope — every core a panel currently covers.
    ///
    /// One command per core, like [`Self::set_auto_detect_many`], because that is what the protocol
    /// offers. Far more destructive than the single-core form by exactly the number of cores given,
    /// so a caller must state that number to the operator before asking.
    ///
    /// Args:
    ///     cores: Cores whose diagnostics are dropped.
    ///
    /// Returns:
    ///     The cores whose command channel accepted the intent, in the order given.
    pub fn clear_core_problems_many(&self, cores: &[CoreId]) -> Vec<CoreId> {
        self.send_many(cores, "clear problems", |core| {
            self.clear_core_problems(core)
        })
    }

    /// Re-read one core's diagnostics from the library's retained snapshot.
    ///
    /// Nothing is sent to the core and nothing on the core changes: the protocol has no request for
    /// a diagnostics list at all, so this only re-publishes what already arrived. See
    /// [`CoreCmd::RefreshProblems`] for the whole reasoning.
    ///
    /// Args:
    ///     core: Core whose retained list is re-published.
    ///
    /// Returns:
    ///     Whether the command reached the core's channel.
    pub fn refresh_core_problems(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::RefreshProblems, "refresh problems")
    }

    /// Re-read the diagnostics of a whole scope. See [`Self::refresh_core_problems`].
    ///
    /// Args:
    ///     cores: Cores whose retained lists are re-published.
    ///
    /// Returns:
    ///     The cores whose command channel accepted the intent, in the order given.
    pub fn refresh_core_problems_many(&self, cores: &[CoreId]) -> Vec<CoreId> {
        self.send_many(cores, "refresh problems", |core| {
            self.refresh_core_problems(core)
        })
    }

    /// Send one intent for the core's built-in Telegram reader or its outgoing bot.
    ///
    /// A successful send means queued, never completed. The reader and login intents are answered
    /// with a new snapshot (`SettingsEvent::TelegramUpdated`) and carry no result ticket. The
    /// outgoing-bot and channel actions (`SetOutgoingBot`, `GenerateChannelPin`, `ResetChannel`)
    /// are tracked like the observer actions: the feed reports their request id and the core's
    /// verdict on `CoreData::service.telegram`, and they are dropped rather than delivered late
    /// across a lost link.
    ///
    /// Args:
    ///     core: Core to command.
    ///     cmd: The typed intent, one variant per MoonTelegram method.
    ///
    /// Returns:
    ///     Whether the command reached the core's channel.
    pub fn telegram_cmd(&self, core: CoreId, cmd: TelegramCmd) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::Telegram(cmd), "telegram")
    }

    /// Send one action to the core's observers ("Защита от зависаний").
    ///
    /// A successful send means queued. The feed then reports the request id it left as
    /// (`FeedMsg::ServiceRequest`) and the core's verdict on it (`FeedMsg::ServiceResult`), both
    /// tracked on `CoreData::service`; the resulting state arrives as a new snapshot. Every action
    /// but a refresh is dropped rather than delivered late across a lost link.
    ///
    /// Args:
    ///     core: Core to command — one core, because a row and its keys belong to it alone.
    ///     cmd: The typed action.
    ///
    /// Returns:
    ///     Whether the command reached the core's channel.
    pub fn watchdog_cmd(&self, core: CoreId, cmd: WatchdogCmd) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::Watchdog(cmd), "watchdog")
    }

    /// Turn one core's AutoDetect on or off — Moonbot's passive mode, inverted.
    ///
    /// The command is an intent: the core answers with a new runtime state, which is what any
    /// indicator must follow. Unlike [`Self::restart_now`] it is reversible and starts nothing.
    ///
    /// Args:
    ///     core: Core to command.
    ///     on: Whether detection should be active (`true`) or the core should go passive.
    ///
    /// Returns:
    ///     Whether the command reached the core's channel.
    pub(crate) fn set_auto_detect(&self, core: CoreId, on: bool) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::SetAutoDetect(on), "set auto detect")
    }

    /// Turn AutoDetect on or off across a whole scope — a saved group, an exchange row, a window.
    ///
    /// One command per core, like [`Self::set_trading_many`], and for the same reason: that is
    /// what the protocol offers. A core whose command channel is gone is skipped and named in the
    /// log rather than aborting the scope.
    ///
    /// Args:
    ///     cores: Cores to command.
    ///     on: Whether detection should be active.
    ///
    /// Crate-private, like [`Self::set_trading_many`]: callers use [`Self::dispatch_run`].
    ///
    /// Returns:
    ///     The cores whose command channel accepted the intent, in the order given.
    pub(crate) fn set_auto_detect_many(&self, cores: &[CoreId], on: bool) -> Vec<CoreId> {
        self.send_many(cores, &format!("set auto detect {on}"), |core| {
            self.set_auto_detect(core, on)
        })
    }

    /// Reset the core's session or all-time profit counter.
    pub fn reset_profit(&self, core: CoreId, kind: ResetProfitKind) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::ResetProfit(kind), "reset profit")
    }

    /// Cancel every order for the core. This is a live exchange action.
    ///
    /// Refused for a core that is not connected, for the reason the run switches are gated
    /// (`run_dispatch`): the command channel outlives a disconnect, so a queued cancel would fire
    /// whenever the core comes back and wipe the orders it holds by then.
    pub fn cancel_all_orders(&self, core: CoreId) -> Result<()> {
        if !self.core_run_state(core).online {
            return Err(anyhow!(
                "core {core} is not connected: cancel all orders not sent"
            ));
        }
        self.send_core_cmd(core, CoreCmd::CancelAllOrders, "cancel all orders")
    }

    /// Ask the core to cancel its entry orders on the exchange (MoonBot 7.73 N1+, MoonProto UI/63).
    ///
    /// One command, whatever the local order snapshot holds: the core fetches the open orders from
    /// the exchange itself and applies its own exchange-specific entry/exit filter, so an empty
    /// snapshot must not suppress it. Core-only pending orders are outside it — that is
    /// `cancel_all_buys`' business, which this never falls back to.
    ///
    /// Refused, with nothing queued, for a core that is not connected (the command channel
    /// outlives a disconnect, see [`Self::cancel_all_orders`]) and for a core that has not proved
    /// the N1 build through `CoreFeatures::exchange_buys_cancel`: an older core drops the request
    /// without a word, so sending it there would report a cancellation that never happens.
    ///
    /// Args:
    ///     core: The core whose exchange account the request addresses.
    ///
    /// Returns:
    ///     `Ok` once the command is queued for the core — not that any order was cancelled.
    pub fn cancel_exchange_entries(&self, core: CoreId) -> Result<()> {
        if !self.core_run_state(core).online {
            return Err(anyhow!(
                "core {core} is not connected: exchange entry cancellation not sent"
            ));
        }
        let supported = self
            .store()
            .core(core)
            .is_some_and(|data| data.features.exchange_buys_cancel);
        if !supported {
            return Err(anyhow!(
                "core {core} has not reported MoonBot 7.73 N1+: exchange entry cancellation not sent"
            ));
        }
        self.send_core_cmd(
            core,
            CoreCmd::CancelAllExchangeBuys,
            "cancel exchange entries",
        )
    }

    /// Set the core's coin-blacklist state and text.
    pub fn set_blacklist(&self, core: CoreId, on: bool, text: String) -> Result<()> {
        let key = (core, None);
        let mut sent = self
            .blacklists_sent
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let next = (on, text);
        let in_store = self
            .blacklist_snapshot(key)
            .unwrap_or_else(|_| next.clone());
        self.send_blacklist_edit(key, &next)?;
        let entry = SentBlacklist::record(sent.remove(&key), in_store, next, false, true);
        sent.insert(key, entry);
        Ok(())
    }

    /// Put one coin on the core's own blacklist, or with `lift` take it off.
    ///
    /// Adding switches the list on, so the entry affects trading: the command carries the flag and
    /// the text together. Lifting drops only that token and leaves the switch as it is — turning
    /// the whole feature off is a bigger statement than un-listing one coin. A lift that changes
    /// nothing sends nothing.
    ///
    /// Args:
    ///     core: Core whose list is rewritten: from the settings it last sent, or from what this
    ///         process sent while the core has not echoed it.
    ///     coin: The core's spelling of the coin ([`crate::symbol::coin_list`]).
    ///     lift: Take the coin off instead of putting it on.
    ///
    /// Returns:
    ///     Whether a command was sent.
    ///
    /// Errors:
    ///     An empty coin; a core that has not sent its settings yet; the core's command channel
    ///     refused it.
    pub fn write_core_blacklist(&self, core: CoreId, coin: &str, lift: bool) -> Result<bool> {
        self.write_blacklist_tokens((core, None), &[coin], lift, false)
            .map(|(sent, _)| sent)
    }

    /// Add one coin and return its inverse only if the effective list gained the token.
    /// A flag-only write follows the usual add behavior without producing an undo action.
    pub fn add_to_blacklist(
        &self,
        core: CoreId,
        strategy: Option<u64>,
        coin: &str,
    ) -> Result<Option<impl FnOnce(&Self) -> Result<bool> + use<>>> {
        let key = (core, strategy);
        let (_, add) = self.write_blacklist_tokens(key, &[coin], false, false)?;
        Ok(add.map(move |add| move |session: &Self| session.revert_blacklist_add(key, &add)))
    }

    /// Add the tokens from one bulk click to one core list and return one shared inverse.
    /// A bulk click that only enables a disabled list can reverse that switch as well.
    pub fn add_many_to_blacklist(
        &self,
        core: CoreId,
        coins: &[&str],
    ) -> Result<Option<BlacklistBatchUndo<impl FnOnce(&Self) -> Result<bool> + use<>>>> {
        let key = (core, None);
        let (_, add) = self.write_blacklist_tokens(key, coins, false, true)?;
        Ok(add.map(move |add| {
            let affected = if add.coins.is_empty() {
                coins.iter().map(|coin| coin.to_string()).collect()
            } else {
                add.coins.clone()
            };
            (affected, move |session: &Self| {
                session.revert_blacklist_add(key, &add)
            })
        }))
    }

    /// Read the last published list, keeping the core-wide and strategy defaults distinct.
    fn blacklist_snapshot(
        &self,
        (core, strategy): (CoreId, Option<u64>),
    ) -> Result<(bool, String)> {
        let data = self.store().core(core);
        if let Some(strategy) = strategy {
            let current = data
                .and_then(|data| data.strategies.iter().find(|s| s.id == strategy))
                .and_then(|s| {
                    s.fields
                        .iter()
                        .find(|(name, _)| name == crate::feed::FIELD_COINS_BLACK_LIST)
                        .map(|(_, value)| value.clone())
                })
                .unwrap_or_default();
            return Ok((true, current));
        }
        // Without the core's own settings there is no list to edit: rewriting it from nothing
        // would replace every coin already on it with this one.
        data.and_then(|data| data.client_settings.as_ref())
            .map(|cs| (cs.use_blacklist, cs.blacklist_text.clone()))
            .ok_or_else(|| {
                anyhow!("core {core} has not sent its settings yet: blacklist not written")
            })
    }

    /// Edit from the same echo-aware base for both regular writes and tracked adds.
    fn write_blacklist_tokens(
        &self,
        key: (CoreId, Option<u64>),
        coins: &[&str],
        lift: bool,
        track_flag_only: bool,
    ) -> Result<(bool, Option<BlacklistAdd>)> {
        if coins.iter().any(|coin| coin.trim().is_empty()) {
            return Err(anyhow!("an empty coin is not a blacklist entry"));
        }
        let mut sent = self
            .blacklists_sent
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let in_store = self.blacklist_snapshot(key)?;
        let base = sent
            .get(&key)
            .map_or(&in_store, |entry| entry.effective(&in_store));
        let mut next = base.clone();
        let mut inserted = Vec::new();
        for coin in coins {
            let (edited, added) = blacklist_token_edit(&next, coin, lift, key.1.is_some());
            next = edited;
            if added {
                inserted.push(coin.to_string());
            }
        }
        if next == *base {
            return Ok((false, None));
        }
        let enabled_flag = !base.0 && next.0;
        let before = base.1.clone();
        self.send_blacklist_edit(key, &next)?;
        let mut entry =
            SentBlacklist::record(sent.remove(&key), in_store, next, key.1.is_some(), false);
        let add = (!inserted.is_empty() || (track_flag_only && enabled_flag)).then(|| {
            for coin in &inserted {
                entry
                    .token_generations
                    .insert(coin.to_ascii_uppercase(), entry.generation);
            }
            BlacklistAdd {
                coins: inserted,
                before,
                after: entry.sent.1.clone(),
                enabled_flag,
                generation: entry.generation,
                at: entry.at,
                epoch: entry.epoch,
            }
        });
        sent.insert(key, entry);
        Ok((true, add))
    }

    /// Reverse a recorded insertion through the current effective list.
    fn revert_blacklist_add(&self, key: (CoreId, Option<u64>), add: &BlacklistAdd) -> Result<bool> {
        let mut sent = self
            .blacklists_sent
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let Some(entry) = sent.get_mut(&key) else {
            return Ok(false);
        };
        let in_store = self.blacklist_snapshot(key)?;
        entry.observe(&in_store, key.1.is_some());
        let restored = add.revert(entry, &in_store, key.1.is_some());
        if restored == *entry.effective(&in_store) {
            return Ok(false);
        }
        self.send_blacklist_edit(key, &restored)?;
        let entry = SentBlacklist::record(
            sent.remove(&key),
            in_store,
            restored,
            key.1.is_some(),
            false,
        );
        sent.insert(key, entry);
        Ok(true)
    }

    /// Reconcile each published list so a later external edit cannot look like an old echo.
    pub(super) fn observe_blacklist_echo(&self, core: CoreId, strategies: bool) {
        let mut sent = self
            .blacklists_sent
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        sent.retain(|_, entry| entry.at.elapsed() < BLACKLIST_ECHO_WAIT);
        for (&key, entry) in &mut *sent {
            if key.0 != core || key.1.is_some() != strategies {
                continue;
            }
            if let Ok(published) = self.blacklist_snapshot(key) {
                entry.observe(&published, strategies);
            }
        }
    }

    /// Submit an edit while its caller holds the list tracker through queue acceptance.
    fn send_blacklist_edit(&self, key: (CoreId, Option<u64>), next: &BlacklistValue) -> Result<()> {
        let (core, strategy) = key;
        if let Some(strategy) = strategy {
            self.send_strategy_fields(
                core,
                vec![(
                    strategy,
                    vec![(
                        crate::feed::FIELD_COINS_BLACK_LIST.to_string(),
                        next.1.clone(),
                    )],
                )],
                false,
            )
        } else {
            let (on, text) = next.clone();
            self.send_core_cmd(core, CoreCmd::SetBlacklist { on, text }, "set blacklist")
        }
    }

    /// Put one coin on a strategy's `CoinsBlackList`, or with `lift` take it off, through the
    /// shared field editor.
    ///
    /// The core omits a field equal to its schema default, so a strategy that sends no list has
    /// an empty one and the first token starts it. An edit that changes nothing sends nothing.
    /// The field is split like every strategy list (`ANC GALA` is two coins), not like the
    /// comma-only core-wide list ([`crate::symbol::coin_list::strategy_edit`]).
    ///
    /// Args:
    ///     core: Core that holds the strategy.
    ///     strategy: Its id.
    ///     coin: The core's spelling of the coin.
    ///     lift: Take the coin off instead of putting it on.
    ///
    /// Returns:
    ///     Whether an edit was sent.
    ///
    /// Errors:
    ///     An empty coin; the core's command channel refused it.
    pub fn write_strategy_blacklist(
        &self,
        core: CoreId,
        strategy: u64,
        coin: &str,
        lift: bool,
    ) -> Result<bool> {
        self.write_blacklist_tokens((core, Some(strategy)), &[coin], lift, false)
            .map(|(sent, _)| sent)
    }

    /// Whether a strategy's kind has a `CoinsBlackList` at all, by the core's schema: an edit of a
    /// field its kind lacks is silently ignored by the core, so no surface offers one.
    pub fn strategy_has_blacklist(&self, core: CoreId, strategy: u64) -> bool {
        let Some(data) = self.store().core(core) else {
            return false;
        };
        let Some(row) = data.strategies.iter().find(|s| s.id == strategy) else {
            return false;
        };
        let Some(schema) = data.schema.as_ref() else {
            return false;
        };
        schema
            .kinds
            .iter()
            .find(|kind| kind.ordinal == row.kind_ordinal)
            .is_some_and(|kind| {
                kind.sections.iter().any(|section| {
                    section
                        .fields
                        .iter()
                        .any(|field| field.name == crate::feed::FIELD_COINS_BLACK_LIST)
                })
            })
    }

    /// Locally exclude blacklisted coins from the core's Active Lib market-delta calculation.
    pub fn set_exclude_blacklisted_delta(&self, core: CoreId, on: bool) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::SetExcludeBlacklistedDelta(on),
            "set exclude blacklisted delta",
        )
    }

    /// Locally derive the short retained deltas from raw trade extrema — the client half of the
    /// wire's `trading.deltas_by_trades`.
    pub fn set_deltas_by_trades(&self, core: CoreId, on: bool) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::SetDeltasByTrades(on), "set deltas by trades")
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

    /// Return what each identified core is connected to, keyed by core.
    ///
    /// THE answer for every consumer that groups cores by exchange, captions one, or draws its
    /// logo: the platform code decides all three through [`crate::venue`], so a picker, the
    /// Profit Monitor and the workspace rail cannot disagree about which venue a core is on.
    ///
    /// Borrowed, not rebuilt: a core's venue arrives once per connection and changes only when the
    /// connection does, while these consumers ask on their render path. A core absent from the map
    /// has not completed `BaseCheck`; one that is present but not
    /// [`CoreVenue::is_nameable`] is identified without being nameable, and belongs in the caller's
    /// "not identified" group.
    ///
    /// Returns:
    ///     One entry per core that reported an identity.
    pub fn core_venues(&self) -> &HashMap<CoreId, CoreVenue> {
        &self.core_venue
    }

    /// Return every account identity `core` stated in its own `AuthCheck` answer; which one names
    /// the account is decided per brand by `venue::merge_key`.
    ///
    /// Returns:
    ///     The identity, or `None` before the answer arrived or when the core stated none.
    pub fn account_identity(&self, core: CoreId) -> Option<AccountIdentity> {
        self.market_source.account_identity(core)
    }

    /// Return which exchange account and wallet the core's balance is, or `None` when unknown.
    ///
    /// The key every balance total folds shared accounts by ([`super::balances`]), decided per
    /// brand by `venue::merge_key` from the core's venue and its stated account identity.
    pub fn account_merge_key(&self, core: CoreId) -> Option<AccountMergeKey> {
        let venue = self.core_venue.get(&core).and_then(|v| v.resolved());
        crate::venue::merge_key(venue, self.account_identity(core))
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
        self.wanted_orderbook.clear();
        self.pending_ob_drop.clear();
        self.last_cmd.clear();
    }
}

#[cfg(test)]
mod tests;
