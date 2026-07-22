//! Per-core account data: status, orders, detects, and strategies. Each core has its own state.
//! Market data such as price ticks and order books is shared per exchange and lives separately in
//! `crate::market::MarketStore`, deduplicated through a provider core; it never enters this store.
//!
//! Revision counters replace dirty flags: each panel decides when to reload its data, which matters
//! when one core is displayed in multiple panels.

use std::collections::{HashMap, VecDeque};

use crate::applog::LogLine;
use crate::feed::{
    AssetsSnapshot, ChartAlertUpdate, ClientSettings, ConnStatus, DetectRow, EngineActionResult,
    FeedMsg, LevManageState, LicenseState, OrderRow, RuntimeState, StrategyRow,
    StrategySchemaModel, TransferAssetsSnapshot,
};
use crate::session::order_lines::OrderLineStore;
use crate::util::now_unix_ms_i64;

/// Maximum number of recent detects retained in memory for each core.
const MAX_DETECTS: usize = 2000;

/// Maximum number of recent server-log lines retained per core for live viewing and search.
/// Older history remains in `logs/<date>_<core>.log` files.
const MAX_LOG: usize = 5000;

/// Maximum number of undelivered Engine action toasts queued while no window is active.
/// The active window's shell consumes the queue.
const MAX_ENGINE_ACTIONS: usize = 64;

pub type CoreId = u64;

/// The store's best available trust classification for a core's USD balance figures.
///
/// The classification lives here, next to the inputs it reads, because the raw numbers
/// alone cannot be rendered honestly: missing pricing can produce a finite zero or partial sum,
/// and a retained snapshot survives a reconnect. Every consumer of `assets.global` must agree
/// about that, so they all go through [`CoreData::balance_state`] instead of re-deriving the rule
/// from `status`/`assets_rev`/`usd_rate_known` on their own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BalanceState {
    /// A snapshot exists, the connection is ready, no stale marker remains, and the USD
    /// valuation is valid. See [`CoreData::assets_stale`] for the freshness limit.
    Live,
    /// The connection is not ready, or it became ready but still awaits a fresh snapshot.
    /// Retained figures may be shown only with an explicit stale marker.
    Stale,
    /// No snapshot has arrived: the balance is UNKNOWN, not zero.
    Awaiting,
    /// A snapshot exists but its free/total USD valuation is incomplete or non-finite.
    /// The figures must render as unavailable rather than as a zero or partial balance.
    Unpriced,
}

impl BalanceState {
    /// Whether there is a usable number to render and to sum.
    pub fn has_value(self) -> bool {
        matches!(self, BalanceState::Live | BalanceState::Stale)
    }

    /// Whether the store classifies the number as current enough to show without a stale marker.
    ///
    /// This is the companion to [`Self::has_value`]: one asks whether there is a figure, the
    /// other whether the available freshness signals classify it as live. The known limit on
    /// [`CoreData::assets_stale`] still applies.
    pub fn is_current(self) -> bool {
        matches!(self, BalanceState::Live)
    }

    /// Stable small integer for hashing this state into a render signature.
    ///
    /// Exists so consumers do not invent their own numbering: the exhaustive match keeps a new
    /// variant a compile error here rather than a silently unhashed state somewhere downstream.
    pub fn code(self) -> u64 {
        match self {
            BalanceState::Live => 1,
            BalanceState::Stale => 2,
            BalanceState::Awaiting => 3,
            BalanceState::Unpriced => 4,
        }
    }
}

pub struct CoreData {
    pub status: ConnStatus,
    /// Latest combined core order rows across all markets.
    ///
    /// A live batch starts from the open-order snapshot and can briefly include captured terminal
    /// event rows that disappeared before the application drained the feed queue.
    pub orders: Vec<OrderRow>,
    /// Retained chart order-line store, including history and up to 5000 closed orders per core.
    pub order_lines: OrderLineStore,
    /// Recent core detects, trimmed as a ring buffer to `MAX_DETECTS`.
    pub detects: VecDeque<DetectRow>,
    /// Latest core strategy snapshot for the Strategies window.
    pub strategies: Vec<StrategyRow>,
    /// Core strategy schema with sections and per-kind fields, or `None` until it arrives.
    pub schema: Option<StrategySchemaModel>,
    /// Latest core assets and positions snapshot for the Assets window.
    pub assets: AssetsSnapshot,
    /// Whether a non-`Ready` status occurred after the latest assets message. The snapshot and
    /// `assets_rev` remain retained across reconnect, so returning to `Ready` cannot establish
    /// freshness by itself.
    ///
    /// KNOWN LIMIT: the marker is cleared by the next `FeedMsg::Assets`, and the live feed emits
    /// those on ANY domain event by REBUILDING the retained snapshot — so arrival proves the core
    /// is talking, not that the balances behind it are current. After a reconnect the figures can
    /// therefore read as `Live` while still being pre-outage. The feed requests a balance refresh
    /// on reconnect, but a failed request or missing response leaves the window unbounded. Closing
    /// it properly needs a connection generation / balance revision carried on the payload, so
    /// this flag can be cleared only by data proven to be current.
    pub assets_stale: bool,
    /// Core transfer assets by wallet for the transfer tree. Empty until requested.
    pub transfer_assets: TransferAssetsSnapshot,
    /// Core license, Free/PRO, and MoonCredits state, or `None` until the core responds.
    pub license: Option<LicenseState>,
    /// Core client-settings snapshot, including TP, SL, sell, and iceberg settings, or `None` until
    /// it arrives.
    pub client_settings: Option<ClientSettings>,
    /// Core leverage-management snapshot, or `None` until it arrives.
    pub lev_manage: Option<LevManageState>,
    /// Core runtime and passive-mode state, or `None` until it arrives.
    pub runtime_state: Option<RuntimeState>,
    /// Account hedge mode for dual-side positions, or `None` until the core responds.
    pub hedge_mode: Option<bool>,
    /// Unshown Engine action results for toasts. The active window's shell drains them through
    /// [`CoreData::take_engine_actions`].
    engine_actions: VecDeque<EngineActionResult>,
    /// Authoritative core chart alerts keyed by `(market, obj_uid)`, with opaque
    /// `TChartObject.Save()` blobs. The server owns the set; after reconnect, the feed requests a
    /// snapshot delivered through the same `Upserted` updates that overwrite entries by key. The
    /// blob is retained for re-upserts when toggling alerts and for format round-tripping.
    pub chart_alerts: HashMap<(String, u64), Vec<u8>>,
    /// Recent core server-log lines, trimmed as a ring buffer to `MAX_LOG`.
    pub log: VecDeque<LogLine>,
    /// Raw server-log lines with terminal receipt times for diagnostics and FireTest measurements.
    /// The UI continues to read the formatted `log`.
    pub server_log_raw: VecDeque<crate::feed::CoreLogLine>,
    /// Latest typed core resource telemetry from protocol-v4 `Event::KernelHealth`.
    /// The Core Status table observes it through `sys_rev`.
    pub sys: crate::feed::CoreSysStatus,
    /// Advances for every new combined order-row batch and gates the Orders table.
    pub orders_table_rev: u64,
    /// Advances only when chart order-line geometry or state changes.
    pub order_lines_rev: u64,
    /// Local time of the latest `order_lines_rev` increment.
    pub order_lines_rev_ms: i64,
    pub detects_rev: u64,
    pub strategies_rev: u64,
    pub schema_rev: u64,
    pub assets_rev: u64,
    pub transfer_rev: u64,
    pub license_rev: u64,
    pub client_settings_rev: u64,
    pub lev_manage_rev: u64,
    pub runtime_state_rev: u64,
    pub hedge_mode_rev: u64,
    pub log_rev: u64,
    pub chart_alerts_rev: u64,
    /// Advances only when typed `KernelHealth` metric values change, gating the
    /// Core Status table without repainting for receipt-time-only updates.
    pub sys_rev: u64,
}

impl CoreData {
    /// Create an empty per-core store in the connecting state.
    pub fn new() -> Self {
        Self {
            status: ConnStatus::Connecting,
            orders: Vec::new(),
            order_lines: OrderLineStore::default(),
            detects: VecDeque::new(),
            strategies: Vec::new(),
            schema: None,
            assets: AssetsSnapshot::default(),
            transfer_assets: TransferAssetsSnapshot::default(),
            license: None,
            client_settings: None,
            lev_manage: None,
            runtime_state: None,
            hedge_mode: None,
            engine_actions: VecDeque::new(),
            chart_alerts: HashMap::new(),
            log: VecDeque::new(),
            server_log_raw: VecDeque::new(),
            sys: crate::feed::CoreSysStatus::default(),
            orders_table_rev: 0,
            order_lines_rev: 0,
            order_lines_rev_ms: 0,
            detects_rev: 0,
            strategies_rev: 0,
            schema_rev: 0,
            assets_stale: false,
            assets_rev: 0,
            transfer_rev: 0,
            license_rev: 0,
            client_settings_rev: 0,
            lev_manage_rev: 0,
            runtime_state_rev: 0,
            hedge_mode_rev: 0,
            log_rev: 0,
            chart_alerts_rev: 0,
            sys_rev: 0,
        }
    }

    /// Return the latest `max` core log lines from oldest to newest for the Log panel.
    pub fn log_snapshot(&self, max: usize) -> Vec<LogLine> {
        let start = self.log.len().saturating_sub(max);
        self.log.iter().skip(start).cloned().collect()
    }

    /// Drain queued Engine action results for the active window's shell.
    ///
    /// There is a single consumer, so each toast is shown exactly once.
    pub fn take_engine_actions(&mut self) -> Vec<EngineActionResult> {
        self.engine_actions.drain(..).collect()
    }

    /// Return the latest raw server-log lines from oldest to newest for diagnostic measurements.
    pub fn raw_server_log_snapshot(&self, max: usize) -> Vec<crate::feed::CoreLogLine> {
        let start = self.server_log_raw.len().saturating_sub(max);
        self.server_log_raw.iter().skip(start).cloned().collect()
    }

    /// Apply an account-plane message to this core.
    ///
    /// The coordinator routes `Identity`, `CoreBase`, and `MarketDataChanged` without applying them
    /// to `CoreData`.
    pub fn apply(&mut self, msg: FeedMsg) {
        match msg {
            FeedMsg::Status(s) => {
                // Any non-Ready status marks the retained snapshot stale, so a reconnect cannot
                // promote pre-outage figures on the strength of the status alone. What clears the
                // marker is documented on `assets_stale` — including what it does NOT prove.
                if !matches!(s, ConnStatus::Ready) {
                    self.assets_stale = true;
                }
                self.status = s;
            }
            FeedMsg::Orders(orders) => {
                // Update the retained line store (traces, nodes, and closures) from the fresh
                // combined row batch before moving it into the table list. Separate revisions gate
                // the table and chart: every batch matters to the table, while only render-affecting
                // order-line changes matter to the chart.
                let changed = self.order_lines.update(&orders);
                self.orders = orders;
                self.orders_table_rev = self.orders_table_rev.wrapping_add(1);
                if changed {
                    self.order_lines_rev = self.order_lines_rev.wrapping_add(1);
                    self.order_lines_rev_ms = now_unix_ms_i64();
                }
            }
            FeedMsg::OrderLines(orders) => {
                let changed = self.order_lines.update(&orders);
                if changed {
                    self.order_lines_rev = self.order_lines_rev.wrapping_add(1);
                    self.order_lines_rev_ms = now_unix_ms_i64();
                }
            }
            FeedMsg::Detects(detects) => {
                if !detects.is_empty() {
                    // The detect diagnostic reached `CoreData` and is about to increment
                    // `detects_rev`, which gates `ChartTabs::ingest` through `chart_tabs_sig`.
                    // Enable this path with `MOON_DETECT_DIAG`.
                    crate::detect_diag::line(&format!(
                        "[store] +{} detects → rev={}",
                        detects.len(),
                        self.detects_rev.wrapping_add(1)
                    ));
                    for det in detects {
                        self.detects.push_back(det);
                    }
                    if self.detects.len() > MAX_DETECTS {
                        while self.detects.len() > MAX_DETECTS {
                            self.detects.pop_front();
                        }
                    }
                    self.detects_rev = self.detects_rev.wrapping_add(1);
                }
            }
            FeedMsg::Strategies(strategies) => {
                self.strategies = strategies;
                self.strategies_rev = self.strategies_rev.wrapping_add(1);
            }
            FeedMsg::StrategySchema(schema) => {
                self.schema = Some(schema);
                self.schema_rev = self.schema_rev.wrapping_add(1);
            }
            FeedMsg::Assets(assets) => {
                self.assets = assets;
                self.assets_stale = false;
                self.assets_rev = self.assets_rev.wrapping_add(1);
            }
            FeedMsg::TransferAssets(transfer) => {
                self.transfer_assets = transfer;
                self.transfer_rev = self.transfer_rev.wrapping_add(1);
            }
            FeedMsg::License(license) => {
                if self.license != Some(license) {
                    self.license = Some(license);
                    self.license_rev = self.license_rev.wrapping_add(1);
                }
            }
            FeedMsg::ClientSettings(settings) => {
                if self.client_settings.as_ref() != Some(&settings) {
                    self.client_settings = Some(settings);
                    self.client_settings_rev = self.client_settings_rev.wrapping_add(1);
                }
            }
            FeedMsg::LevManage(lev) => {
                if self.lev_manage.as_ref() != Some(&lev) {
                    self.lev_manage = Some(lev);
                    self.lev_manage_rev = self.lev_manage_rev.wrapping_add(1);
                }
            }
            FeedMsg::RuntimeState(state) => {
                if self.runtime_state != Some(state) {
                    self.runtime_state = Some(state);
                    self.runtime_state_rev = self.runtime_state_rev.wrapping_add(1);
                }
            }
            FeedMsg::SysStatus(sys) => {
                // Telemetry arrives every Ping with a fresh `updated_ms`, so store it every
                // time to keep the panel's "Updated" column live — but bump `sys_rev` (the
                // repaint signature) ONLY when the metrics changed, else a steady core would
                // churn it every Ping. Repaints are also capped by the 250ms backend throttle
                // and the panel RenderGate.
                let metrics_changed = !self.sys.metrics_eq(&sys);
                self.sys = sys;
                if metrics_changed {
                    self.sys_rev = self.sys_rev.wrapping_add(1);
                }
            }
            FeedMsg::HedgeMode(on) => {
                if self.hedge_mode != Some(on) {
                    self.hedge_mode = Some(on);
                    self.hedge_mode_rev = self.hedge_mode_rev.wrapping_add(1);
                }
            }
            FeedMsg::EngineActions(results) => {
                self.engine_actions.extend(results);
                while self.engine_actions.len() > MAX_ENGINE_ACTIONS {
                    self.engine_actions.pop_front();
                }
            }
            FeedMsg::ChartAlerts(updates) => {
                for u in updates {
                    match u {
                        ChartAlertUpdate::Upserted(row) => {
                            self.chart_alerts
                                .insert((row.market, row.obj_uid), row.blob);
                        }
                        ChartAlertUpdate::Deleted { market, obj_uid } => {
                            self.chart_alerts.remove(&(market, obj_uid));
                        }
                    }
                }
                self.chart_alerts_rev = self.chart_alerts_rev.wrapping_add(1);
            }
            FeedMsg::ServerLog(lines) => {
                if !lines.is_empty() {
                    for l in lines {
                        self.server_log_raw.push_back(l.clone());
                        self.log.push_back(LogLine::core(l.time_ms, l.msg));
                    }
                    if self.log.len() > MAX_LOG {
                        let drop = self.log.len() - MAX_LOG;
                        self.log.drain(0..drop);
                    }
                    if self.server_log_raw.len() > MAX_LOG {
                        let drop = self.server_log_raw.len() - MAX_LOG;
                        self.server_log_raw.drain(0..drop);
                    }
                    self.log_rev = self.log_rev.wrapping_add(1);
                }
            }
            // Identity and market wake-up messages are not routed into this store.
            FeedMsg::Identity(_) | FeedMsg::CoreBase { .. } | FeedMsg::MarketDataChanged(_) => {}
        }
    }

    /// Best available trust classification for this core's `assets.global` USD figures.
    ///
    /// `Unpriced` outranks `Stale`: an unpriced figure has no number to show at all, so its
    /// freshness is moot. Staleness needs BOTH inputs — `assets_stale` covers the reconnect
    /// window (status returns to `Ready` before the new snapshot lands), while the `status`
    /// check covers a snapshot that arrived before the link ever reached `Ready`. The generation
    /// ambiguity documented on [`Self::assets_stale`] prevents this from proving freshness.
    pub fn balance_state(&self) -> BalanceState {
        if self.assets_rev == 0 {
            BalanceState::Awaiting
        } else if !self.assets.global.usd_rate_known {
            BalanceState::Unpriced
        } else if self.assets_stale || !matches!(self.status, ConnStatus::Ready) {
            BalanceState::Stale
        } else {
            BalanceState::Live
        }
    }
}

impl Default for CoreData {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct CoreStore {
    cores: HashMap<CoreId, CoreData>,
}

impl CoreStore {
    pub fn ensure(&mut self, id: CoreId) {
        self.cores.entry(id).or_default();
    }

    /// Remove account data for a core whose server was removed from configuration.
    /// The session lifecycle separately removes its feed handle, market client, and coordination
    /// state.
    pub fn remove(&mut self, id: CoreId) {
        self.cores.remove(&id);
    }

    pub fn core(&self, id: CoreId) -> Option<&CoreData> {
        self.cores.get(&id)
    }

    pub fn core_mut(&mut self, id: CoreId) -> Option<&mut CoreData> {
        self.cores.get_mut(&id)
    }

    /// Iterate over owned snapshots of every core's status for Settings badges.
    pub fn statuses(&self) -> impl Iterator<Item = (CoreId, ConnStatus)> + '_ {
        self.cores.iter().map(|(id, d)| (*id, d.status.clone()))
    }

    /// Iterate over core ids and data for chart-alert reconciliation and similar consumers.
    pub fn cores(&self) -> impl Iterator<Item = (CoreId, &CoreData)> + '_ {
        self.cores.iter().map(|(id, d)| (*id, d))
    }

    /// Return the combined chart-alert revision across all cores.
    ///
    /// This cheaply detects whether any server-owned alert set changed and gates remote-figure
    /// reconciliation.
    pub fn chart_alerts_activity(&self) -> u64 {
        self.cores
            .values()
            .fold(0u64, |a, c| a.wrapping_add(c.chart_alerts_rev))
    }

    /// Return the combined log revision across all cores.
    ///
    /// This cheaply detects new log lines on any core so the application can request a frame for
    /// windows whose Log tab is active.
    pub fn log_activity(&self) -> u64 {
        self.cores
            .values()
            .fold(0u64, |a, c| a.wrapping_add(c.log_rev))
    }
}

#[cfg(test)]
/// Checks for the balance trust classifier every UI surface reads through.
mod tests;
