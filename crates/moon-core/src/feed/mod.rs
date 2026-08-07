//! Feed: the boundary between a core and the UI. One backend thread per core sends `FeedMsg`
//! to the UI. The UI never calls moonproto directly. Live mode is the only mode.

pub(crate) mod assets;
mod core_label;
pub mod live;
pub mod news;
pub mod news_marks;
mod order_edit;
mod strategies;
pub mod synth;
mod trade;
pub mod types;

pub use core_label::{clear_core_name, core_label, set_core_name};
pub use news::{NewsItem, NewsSnapshot};
pub use types::*;

use std::sync::mpsc::{Receiver, SendError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use moonproto::MoonClient;

use crate::config::ServerConfig;
use crate::db::ReportTx;
use crate::market::SharedMarketStore;

pub type FeedRx = Receiver<FeedMsg>;
pub type FeedWakeTx = Sender<()>;

/// Timestamp indicating that any Assets view rendered recently, in Unix milliseconds.
/// The UI stamps it from every `AssetsView::render`; feed threads use it to choose the cadence for
/// a full `build_assets`: 1 Hz while at least one view is active, every 5 seconds otherwise. A
/// snapshot is still always needed because the header balance and leverage metric read
/// `assets.global`/`assets.leverage`. The marker is self-healing: when all views are closed or
/// hidden, renders stop, the timestamp ages, and the cadence slows without separate close signals.
/// It is shared across group, tool, and detached Assets views because any recent render requests
/// the fast cadence for all feed threads.
static ASSETS_VIEW_RENDER_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

fn unix_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Record a render from any Assets view, as called by the UI.
///
/// A visible view renders at least once per second through the panel's `RenderGate`, so the shared
/// marker remains fresh while any Assets view is on screen.
pub fn note_assets_view_render() {
    ASSETS_VIEW_RENDER_MS.store(unix_ms_now(), std::sync::atomic::Ordering::Relaxed);
}

/// Return whether any Assets view rendered within roughly the last three seconds.
pub(crate) fn assets_view_active() -> bool {
    unix_ms_now() - ASSETS_VIEW_RENDER_MS.load(std::sync::atomic::Ordering::Relaxed) < 3_000
}

/// The client slot's contents: the live client and how many have been installed here.
#[derive(Default)]
struct MoonClientSlot {
    client: Option<Arc<MoonClient>>,
    /// Counts INSTALLS into this slot, so a replaced client is distinguishable from the same one.
    ///
    /// Every reconnect builds a fresh `MoonClient` in `live::run` and drops it into this slot, and
    /// a fresh client means fresh retained history: empty rings, no subscriptions yet. Nothing else
    /// in the terminal reports that — `MarketDataSource`'s provider generation is bumped only when
    /// a core is added or removed, not when its connection drops and comes back. A consumer that
    /// cached anything derived from the client's retained state compares this number to know it
    /// must start over.
    epoch: u64,
}

#[derive(Clone, Default)]
pub struct SharedMoonClient {
    inner: Arc<RwLock<MoonClientSlot>>,
}

impl SharedMoonClient {
    pub(crate) fn set(&self, client: Option<Arc<MoonClient>>) {
        let mut slot = self.inner.write().expect("moon client slot poisoned");
        if client.is_some() {
            slot.epoch = slot.epoch.wrapping_add(1);
        }
        slot.client = client;
    }

    pub fn get(&self) -> Option<Arc<MoonClient>> {
        self.get_with_epoch().map(|(client, _)| client)
    }

    /// The live client together with the epoch it was installed under.
    ///
    /// Read as a pair under ONE guard on purpose: taken separately, a reconnect landing between
    /// the two reads would pair a fresh client with a stale epoch, and whatever the caller keyed
    /// on that epoch would never be redone for the client that actually needs it.
    pub fn get_with_epoch(&self) -> Option<(Arc<MoonClient>, u64)> {
        let slot = self.inner.read().expect("moon client slot poisoned");
        slot.client.clone().map(|client| (client, slot.epoch))
    }
}

#[derive(Clone)]
pub struct FeedTx {
    data: Sender<FeedMsg>,
    wake: Option<FeedWakeTx>,
}

impl FeedTx {
    fn new(data: Sender<FeedMsg>, wake: Option<FeedWakeTx>) -> Self {
        Self { data, wake }
    }

    pub fn send(&self, msg: FeedMsg) -> Result<(), SendError<FeedMsg>> {
        self.data.send(msg)?;
        if let Some(wake) = &self.wake {
            let _ = wake.send(());
        }
        Ok(())
    }
}

/// Specification for a new strategy created or pasted by the UI.
///
/// `fields` contains UI-formatted strings, as in `EditStrategyFields`, and the strategy name is
/// stored in the `"StrategyName"` field. The feed converts the strings to `FieldValue` entries
/// using the kind schema and assigns a new `strategy_id`.
#[derive(Debug, Clone)]
pub struct NewStrategySpec {
    pub kind_ordinal: u8,
    pub folder_path: String,
    pub fields: Vec<(String, String)>,
    /// `(core, strategy id)` this strategy should sit immediately after; `None` appends.
    ///
    /// A copy belongs beside its source, not at the bottom of a list that can run to a hundred
    /// strategies. A PLACEMENT REQUEST, not a durable guarantee: moonproto keeps the snapshot
    /// order it is handed and an echo never reorders an id it already knows, so the position
    /// holds for the session — but a core reconnect or a terminal restart rebuilds the list in
    /// the server's own wire order, which is the Delphi core's and outside this terminal's say.
    ///
    /// CORE-QUALIFIED on purpose. Strategy ids are small per-core sequences, so an id borrowed
    /// from another core almost certainly EXISTS on the destination — it just belongs to an
    /// unrelated strategy, and the copy would land silently beside that one. Carrying the core
    /// lets the drain drop a foreign anchor where `server.id` is known, instead of asking every
    /// producer to remember the rule.
    pub insert_after: Option<(u64, u64)>,
}

/// Order stop flag toggled by clicking a cell in the Orders table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderStopKind {
    /// Stop loss (`SL`).
    StopLoss,
    /// Trailing stop (`TS`).
    Trailing,
    /// VStop.
    VStop,
}

/// Price set by dragging an order's stop or take-profit line on the chart.
///
/// Volume-based VStop and the pending condition are excluded because they have no price level
/// that can be dragged with the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderLinePriceKind {
    /// Stop loss (`SL`) at a fixed price.
    StopLoss,
    /// Trailing stop (`TS`) at a fixed price.
    Trailing,
    /// Take profit at an absolute price.
    TakeProfit,
}

/// Edit to one stop group, either SL or TS, from the order editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StopGroupEdit {
    /// Target state: whether the stop is enabled.
    pub on: bool,
    /// `true` uses `price` as a fixed absolute price; `false` uses the global or percentage mode
    /// with a level from the wire, strategy, or `ClientSettings` default.
    pub fixed: bool,
    /// Absolute fixed-stop price, used when `fixed` is set.
    pub price: f64,
}

/// Take-profit edit from the order editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TakeProfitEdit {
    pub on: bool,
    /// Absolute take-profit price, used when `on` is set.
    pub price: f64,
}

/// VStop edit from the order editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VStopEdit {
    pub on: bool,
    /// `true` treats `level` as a fixed price; `false` treats it as a percentage level.
    pub fixed: bool,
    pub level: f64,
    /// Trigger volume threshold (`Vol <`).
    pub vol: f64,
}

/// Order-stop edits from the Active Order editor.
///
/// `None` for a group means the user did not change it, so the feed preserves its effective
/// state. See `feed::order_edit`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OrderStopsForm {
    pub sl: Option<StopGroupEdit>,
    pub ts: Option<StopGroupEdit>,
    pub tp: Option<TakeProfitEdit>,
    pub vstop: Option<VStopEdit>,
}

impl OrderStopsForm {
    /// Return whether the form contains no edits and therefore need not be sent.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Commands from the coordinator to a core backend that define the core's market role.
#[derive(Debug, Clone)]
pub enum CoreCmd {
    /// Desired market role of the core as full state, not a delta.
    ///
    /// With `provider=true`, the core retains every exchange trade through
    /// `subscribe_all_trades`, reads trades and history for each entry in `markets`, and subscribes
    /// to order books only for `orderbook_markets`. With `provider=false`, it has no market
    /// subscriptions and emits only account-plane data such as orders, detects, and strategies.
    SetMarket {
        provider: bool,
        markets: Vec<String>,
        /// Subset of `markets` that need an order book because at least one window enables it.
        /// Order-book subscriptions are retained only for these markets.
        orderbook_markets: Vec<String>,
    },
    /// Action on the core's strategies. It first synchronizes changed checkboxes by calling
    /// `set_checked` for each pair and `send_checked_delta`, then optionally sends start checked
    /// with `start_stop=Some(true)` or stop checked with `Some(false)`.
    /// `checks` contains only changed checkboxes; `start_stop=None` only synchronizes them.
    StrategiesAction {
        checks: Vec<(u64, bool)>,
        start_stop: Option<bool>,
    },
    /// Field edits for strategies belonging to one core, with one `(id, changes)` map from field
    /// name to string per strategy. All edits for the core must travel in one command: the feed
    /// clones the full snapshot, patches every listed strategy, and sends one
    /// `sync_local_strategies`. Separate commands are unsafe because each sync replaces the full
    /// set, so the second sync would overwrite the first edit.
    EditStrategyFields {
        edits: Vec<(u64, Vec<(String, String)>)>,
    },
    /// Logically delete one core strategy by `id` using `TStratDelete` with `folder_path=""`. Its
    /// history remains in `strat_db`, and the UI can restore it under the same ID. The UI enforces
    /// that only unchecked strategies can be deleted before sending the command.
    DeleteStrategy { id: u64 },
    /// Delete an entire folder by path using `TStratDelete` with `strategy_id=0`.
    /// The server removes an empty folder; the UI guarantees that contained strategies are moved
    /// or deleted first.
    DeleteFolder { path: String },
    /// Create new strategies, including clipboard pastes within or between cores. The feed adds
    /// each strategy to the full set through `StrategySnapshot::new`, assigns `max + 1` from the
    /// target core, converts string fields through the schema, sets `last_date=now`, and sends one
    /// `sync_local_strategies` per core.
    CreateStrategies { specs: Vec<NewStrategySpec> },
    /// Restore a deleted strategy under its previous ID, preserving version history and
    /// order-profit joins through `strategyid`. The feed inserts a snapshot carrying that ID into
    /// the full set. `sync_local_strategies` sends the client ID unchanged, and the core accepts
    /// arbitrary unique IDs just as it accepts `max + 1` on normal creation. Fields come from the
    /// latest `strat_db` version.
    RestoreStrategy {
        id: u64,
        kind_ordinal: u8,
        folder_path: String,
        fields: Vec<(String, String)>,
    },
    /// Move existing strategies or rename their folder. Each `moves` entry contains
    /// `(strategy_id, new_folder_path)`. The feed patches `path` for the listed strategies in the
    /// full set, advances `last_date`, and sends one `sync_local_strategies`.
    MoveStrategies { moves: Vec<(u64, String)> },
    /// Transfer an asset between wallets of one core through drag and drop in the Assets tree.
    /// `from` and `to` are Spot, Futures, or Quarterly wallets; `qty` is in the base coin.
    TransferAsset {
        asset: String,
        qty: f64,
        from: WalletKind,
        to: WalletKind,
    },
    /// Request a fresh list of the core's transferable assets across all wallets.
    RefreshTransferAssets,
    /// Permanently convert one core's small balances, or dust, to BNB.
    ConvertDust,
    /// Place a manual order from the main screen on the core's `market`. `short` is the position
    /// side, mirroring `is_short`; `size` is in the base coin; `strategy_id=None` maps to
    /// `StratID=0` for an order without a strategy. This becomes moonproto `new_order`; see
    /// `feed::trade`.
    PlaceOrder {
        market: String,
        short: bool,
        price: f64,
        size: f64,
        strategy_id: Option<u64>,
        /// Group-local exit settings that must be confirmed before this order is released.
        exit: crate::config::GroupExitSettings,
    },
    /// Move or replace an existing core order by `uid` at a new price, as when dragging its line.
    /// This becomes moonproto `orders().move_order`.
    MoveOrder { uid: u64, new_price: f64 },
    /// Cancel a core order by `uid` through moonproto `orders().cancel`.
    CancelOrder { uid: u64 },
    /// Toggle the SL, TS, or VStop flag of one core order by `uid`, as triggered by a cell click in
    /// the Orders table. The feed reads the retained order snapshot and flips the requested flag
    /// while preserving its level, spread, and percentage or fixed mode. It sends
    /// `orders().update_stops` for SL/TS or `orders().update_vstop` for VStop. The runtime itself
    /// sends only when the result differs from its live order model.
    SetOrderStop {
        uid: u64,
        kind: OrderStopKind,
        on: bool,
    },
    /// Move an order's stop or take-profit line by dragging it on the chart. The feed reads the
    /// retained order snapshot, sets SL/TS to a fixed price or sets take profit to an absolute
    /// price, preserves the other stops, and sends `orders().update_stops`.
    MoveOrderStopPrice {
        uid: u64,
        kind: OrderLinePriceKind,
        price: f64,
    },
    /// Apply all SL, TS, TP, and VStop edits from the Active Order editor at once, including
    /// enabling, disabling, using a fixed price, or returning to the global level. SL, TS, and TP
    /// are combined into one `update_stops`, with untouched groups preserved effectively as in
    /// `SetOrderStop`; VStop uses a separate `update_vstop`. See `feed::order_edit`.
    UpdateOrderStopsForm { uid: u64, form: OrderStopsForm },
    /// Soft-delete (`deleted=true`) or restore (`false`) report rows on this core, addressed by
    /// `newrecid` ranges and singles from the Report selection commands. The core commits it to
    /// its report database and echoes `ReportEvent::RowsDeleted` to all subscribers, which flips
    /// the local replica's `deleted` flag; nothing is applied locally until that echo. Soft, not
    /// physical: a `deleted=false` batch restores the rows.
    SetReportRowsDeleted {
        deleted: bool,
        ranges: Vec<moonproto::ReportRecIdRange>,
        singles: Vec<i64>,
    },
    /// Targeted `ClientSettings` edit from the toolbar, such as TP, SL, or sell-preset selection.
    /// The feed patches the retained settings snapshot through its helper and sends it in full
    /// with `settings().send`.
    EditClientSettings(ClientSettingsEdit),
    /// Synchronize complete visible group exit settings through the per-core serializer.
    SyncGroupExit(crate::config::GroupExitSettings),
    /// Targeted leverage-management edit. The feed patches the retained snapshot and sends it
    /// through `settings().manage_leverage`.
    EditLevManage(LevManageEdit),
    /// Toggle account hedge mode for dual-side positions. This performs a live exchange action
    /// through the Engine API's `account().set_hedge_mode`.
    SetHedgeMode(bool),
    /// Set leverage for one market from the toolbar's Lev Apply button. This performs a live
    /// exchange action through the Engine API's `account().set_leverage`; the new value returns in
    /// the market's balance push as `leverage_x` and updates the leverage map in assets.
    SetLeverage { market: String, leverage: i32 },
    /// Toggle market-level panic sell from the chart button. This is a live action translated to
    /// moonproto `orders().switch_panic_sell_by_market(market, on)`, which toggles panic sell on
    /// the market's orders.
    PanicSellMarket { market: String, on: bool },
    /// Toggle panic sell for one order by `uid` through `TTurnPanicSellCommand`. A sell-line drag
    /// clears this order's panic flag before `move_order`; otherwise the core's panic worker would
    /// restore the previous price. See `AllowedDrop`. This is a live action.
    TurnOrderPanicSell { uid: u64, on: bool },
    /// Close a market position at market from the Market Sell button on an Assets position row.
    /// moonproto `trade().close_position(ClosePositionParams{market, market_sell:true})`
    /// (`TDoClosePositionCommand`). This performs a live exchange action.
    MarketSellPosition { market: String },
    /// Sell a market's spot token at market from the Market Sell button on an Assets holding row.
    /// This uses moonproto `trade().sell_order(SellOrderParams{market, price:0=market, size})`
    /// (`TDoSellOrderCommand`) and performs a live exchange action.
    MarketSellToken { market: String, size: f64 },
    /// Cancel pending buy orders for a market from the Cancel Buy button. The feed reads the
    /// retained snapshot, selects the market's pre-fill buy-phase orders in `OS_None` or `BuySet`,
    /// and sends `orders().cancel(uid)` for each. This is a live action.
    CancelMarketBuys { market: String },
    /// Join all sell orders for a market and position side from a sell-line context menu.
    /// This is a live action translated to moonproto `trade().join_orders(market, side)`.
    JoinSells { market: String, short: bool },
    /// Split the specific sell order selected from a line or row into `parts`.
    /// Protocol v4 addresses this live trading action by the server order `uid`.
    SplitOrder { uid: u64, parts: i32 },
    /// Split by market for a hotkey with no selected order.
    /// The feed sends this live trading action only when exactly one active sell
    /// order can be resolved; zero or multiple candidates are a no-op.
    SplitOrderForMarket { market: String, parts: i32 },
    /// Start or restart the core runtime from the core-settings popup through moonproto
    /// `settings().restart_now()`. It starts the market runtime, leaves passive mode, and starts
    /// checked strategies. The protocol has no stop operation.
    RestartNow,
    /// Reset the core's session or all-time profit counter through moonproto
    /// `settings().reset_profit`.
    ResetProfit(ResetProfitKind),
    /// Cancel every order for the core from the confirmed core-settings action. This performs a
    /// live exchange action through moonproto `account().cancel_all_orders()`.
    CancelAllOrders,
    /// Set the coin blacklist state and text. The feed patches
    /// `use_coins_black_list`/`coins_black_list_text` in the retained settings snapshot and sends
    /// it in full.
    SetBlacklist { on: bool, text: String },
    /// Locally exclude blacklisted coins from the Active Lib market-delta calculation. This is
    /// not a wire settings field; it uses moonproto
    /// `settings().set_exclude_blacklisted_markets_from_exchange_delta`.
    SetExcludeBlacklistedDelta(bool),
    /// Arm or update a chart alert, represented by a drawn object with its Alert option enabled,
    /// on a core market. `blob` is a serialized `TChartObject`; see `alert_blob`. This calls
    /// moonproto `chart_alerts().upsert(market, obj_uid, blob)`. The server is authoritative and
    /// returns `Event::ChartAlert::Upserted`.
    ChartAlertUpsert {
        market: String,
        obj_uid: u64,
        blob: Vec<u8>,
    },
    /// Disarm and delete a chart alert by `obj_uid` through moonproto `chart_alerts().delete`.
    ChartAlertDelete { market: String, obj_uid: u64 },
}

/// Complete market-role assignment published independently of the bounded command backlog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::feed) struct MarketRoleAssignment {
    pub(in crate::feed) provider: bool,
    pub(in crate::feed) markets: Vec<String>,
    pub(in crate::feed) orderbook_markets: Vec<String>,
}

/// Latest successfully queued market-role assignment shared by every command sender clone.
///
/// The mutex serializes the command-channel send with snapshot publication. The consumer holds the
/// same mutex while adopting and applying the snapshot, so it cannot apply an older provider role
/// after a newer account-only assignment has already been accepted.
#[derive(Clone, Default)]
pub(in crate::feed) struct LatestMarketRole {
    inner: Arc<Mutex<Option<MarketRoleAssignment>>>,
}

impl LatestMarketRole {
    /// Locks the latest assignment, recovering its data if an unrelated panic poisoned the mutex.
    pub(in crate::feed) fn lock(&self) -> MutexGuard<'_, Option<MarketRoleAssignment>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Command sender that wakes the live loop and publishes market roles outside the bounded backlog.
#[derive(Clone)]
pub struct CoreCmdTx {
    data: Sender<CoreCmd>,
    wake: Sender<()>,
    latest_market_role: LatestMarketRole,
}

impl CoreCmdTx {
    /// Creates a sender backed by one command queue, wake channel, and market-role snapshot.
    fn new(data: Sender<CoreCmd>, wake: Sender<()>, latest_market_role: LatestMarketRole) -> Self {
        Self {
            data,
            wake,
            latest_market_role,
        }
    }

    /// Queues a command and wakes the live loop.
    ///
    /// A market-role snapshot is published only after the matching queue send succeeds. Holding the
    /// snapshot lock across both operations gives cloned senders and the consumer one order.
    pub fn send(&self, cmd: CoreCmd) -> Result<(), SendError<CoreCmd>> {
        let assignment = match &cmd {
            CoreCmd::SetMarket {
                provider,
                markets,
                orderbook_markets,
            } => Some(MarketRoleAssignment {
                provider: *provider,
                markets: markets.clone(),
                orderbook_markets: orderbook_markets.clone(),
            }),
            _ => None,
        };
        if let Some(assignment) = assignment {
            let mut latest = self.latest_market_role.lock();
            self.data.send(cmd)?;
            *latest = Some(assignment);
        } else {
            self.data.send(cmd)?;
        }
        let _ = self.wake.send(());
        Ok(())
    }
}

impl Drop for CoreCmdTx {
    fn drop(&mut self) {
        let _ = self.wake.send(());
    }
}

/// Backend-thread handle whose drop closes the channels and terminates the thread.
pub struct FeedHandle {
    pub rx: FeedRx,
    pub cmd_tx: CoreCmdTx,
    pub client: SharedMoonClient,
    _join: std::thread::JoinHandle<()>,
}

/// Initial backoff step and its cap between initial connection attempts.
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Minimum `live::run` lifetime that marks a connection as stable and resets backoff.
///
/// A rare disconnect after a long run should not classify the host as repeatedly failing.
const STABLE_AFTER: Duration = Duration::from_secs(60);

/// Apply a random multiplier in the 0.75..1.25 range for 25% jitter.
///
/// This spreads synchronized reconnects across many cores so a failed host does not make hundreds
/// of cores retry in lockstep.
fn jittered(d: Duration) -> Duration {
    let mut b = [0u8; 8];
    let _ = getrandom::getrandom(&mut b);
    let frac = (u64::from_le_bytes(b) % 1000) as f64 / 1000.0; // 0.0..1.0
    d.mul_f64(0.75 + frac * 0.5)
}

/// Start the live backend for one core, keeping a connection at all times and subscribing on
/// command. `reports` is the channel to the SQLite writer; `None` means the database is unavailable
/// and reports are not written.
pub fn spawn(
    server: ServerConfig,
    chart_memory_percent: u16,
    reports: Option<ReportTx>,
    wake: Option<FeedWakeTx>,
    market: Option<SharedMarketStore>,
) -> FeedHandle {
    let (data_tx, rx) = std::sync::mpsc::channel();
    let tx = FeedTx::new(data_tx, wake);
    let (cmd_data_tx, cmd_rx) = std::sync::mpsc::channel::<CoreCmd>();
    let (run_wake_tx, run_wake_rx) = std::sync::mpsc::channel::<()>();
    let latest_market_role = LatestMarketRole::default();
    let cmd_tx = CoreCmdTx::new(cmd_data_tx, run_wake_tx.clone(), latest_market_role.clone());
    let client = SharedMoonClient::default();
    let thread_client = client.clone();
    let join = std::thread::Builder::new()
        .name(format!("feed-{}", server.id))
        .spawn(move || {
            // Reconnect at the application layer when `live::run` fails, including an initial
            // connection failure. Moonproto can reconnect only after a successful connection.
            // Retry with exponential backoff and jitter; exit normally when `Ok` reports that the
            // coordinator/UI has gone away. A synthetic benchmark core runs `synth::run` without
            // networking or reconnects.
            if server.synthetic {
                let _ = synth::run(&server, &tx, &cmd_rx, market.as_ref());
                return;
            }
            let mut backoff = BACKOFF_MIN;
            let mut client_settings_sequence = live::ClientSettingsSequence::new();
            let mut market_role = live::MarketRoleState::default();
            loop {
                let started = Instant::now();
                client_settings_sequence.prepare_reconnect();
                match live::run(
                    &server,
                    chart_memory_percent,
                    &tx,
                    &cmd_rx,
                    &run_wake_tx,
                    &run_wake_rx,
                    reports.as_ref(),
                    thread_client.clone(),
                    &mut client_settings_sequence,
                    &mut market_role,
                    &latest_market_role,
                ) {
                    Ok(()) => break,
                    Err(e) => {
                        // A long-lived connection is not a repeatedly failing host, so reset its
                        // backoff before reconnecting.
                        if started.elapsed() >= STABLE_AFTER {
                            backoff = BACKOFF_MIN;
                        }
                        let wait = jittered(backoff);
                        log::error!(
                            "live backend «{}» упал: {e:#}; реконнект через {:?}",
                            server.name,
                            wait
                        );
                        if tx
                            .send(FeedMsg::Status(ConnStatus::Failed(format!(
                                "{e} · переподключение…"
                            ))))
                            .is_err()
                        {
                            break; // The UI is closed.
                        }
                        match run_wake_rx.recv_timeout(wait) {
                            Ok(()) => while run_wake_rx.try_recv().is_ok() {},
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                        backoff = (backoff * 2).min(BACKOFF_MAX);
                    }
                }
            }
        })
        .expect("spawn feed thread");
    FeedHandle {
        rx,
        cmd_tx,
        client,
        _join: join,
    }
}
