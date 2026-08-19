//! Per-core account data: status, orders, detects, and strategies. Each core has its own state.
//! Market data such as price ticks and order books is shared per exchange and lives separately in
//! `crate::market::MarketStore`, deduplicated through a provider core; it never enters this store.
//!
//! Revision counters replace dirty flags: each panel decides when to reload its data, which matters
//! when one core is displayed in multiple panels.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::applog::LogLine;
use crate::feed::{
    AssetsSnapshot, ChartAlertUpdate, ClientSettings, ConnStatus, DetectRow, EngineActionResult,
    FeedMsg, LevManageState, LicenseState, NewsSnapshot, OrderRow, RuntimeState, StrategyRow,
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

/// Retained account-plane and operational state for one configured core.
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
    /// Exchange API-key expiration, or `None` while this core has never answered. A LATER failure
    /// does not clear it: the last successful answer is retained until the connection is replaced,
    /// so a core whose checks start failing keeps showing what it last reported.
    pub api_expiry: Option<crate::feed::ApiKeyExpiry>,
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
    /// The Core Status panel observes it through `sys_rev`.
    pub sys: crate::feed::CoreSysStatus,
    /// Latest startup progress and channel measurements polled from the moonproto client.
    /// The Core Status panel observes it through `startup_rev`. It FREEZES once the core settles,
    /// so after a successful startup `elapsed_ms` is how long that core took to come up, not a
    /// running clock.
    pub startup: crate::feed::CoreStartupStatus,
    /// Endpoint decoded by the live feed from the exported key.
    ///
    /// It is stored beside health telemetry because the Core Status panel groups processes by the
    /// host address without ever reading the plaintext key.
    pub endpoint: Option<crate::feed::CoreEndpoint>,
    /// Latest reduced news snapshot (logical items + tags catalog) for this core. The News panel
    /// observes it through `news_rev` and merges across the scoped cores by `meta.id`.
    pub news: NewsSnapshot,
    /// Terminal receive time (Unix ms) per news `meta.id`, stamped on first sight, so the News
    /// panel's latency chain has a "received by terminal" anchor the wire does not carry. Pruned to
    /// the ids still in the current ring.
    news_seen_at: HashMap<String, i64>,
    /// Advances for every new combined order-row batch and gates the Orders table.
    pub orders_table_rev: u64,
    /// Advances only when chart order-line geometry or state changes.
    pub order_lines_rev: u64,
    /// Local time of the latest `order_lines_rev` increment.
    pub order_lines_rev_ms: i64,
    pub detects_rev: u64,
    pub strategies_rev: u64,
    /// Advances on each core acknowledgement of a checkbox delta.
    ///
    /// Separate from `strategies_rev` on purpose: that counter also advances for a snapshot the
    /// protocol library rebuilt from its OWN locally-applied change, so it cannot distinguish
    /// "we asked" from "the core agreed". Anything that must not act until the core has committed
    /// a checkbox change waits on this one.
    pub strategies_ack_rev: u64,
    pub schema_rev: u64,
    pub assets_rev: u64,
    pub transfer_rev: u64,
    pub license_rev: u64,
    pub client_settings_rev: u64,
    pub lev_manage_rev: u64,
    pub runtime_state_rev: u64,
    pub hedge_mode_rev: u64,
    /// Advances only when the API-key ANSWER changes — not when the same answer is re-received on
    /// the six-hourly poll, and not on the receipt stamp alone.
    pub api_expiry_rev: u64,
    pub log_rev: u64,
    /// Total number of log lines ever pushed into `log`, including those the ring has since
    /// evicted.
    ///
    /// Separate from `log_rev`, which counts BATCHES and so cannot say how many lines a reader
    /// missed. A consumer that keeps its own copy of the rows stores this value and asks
    /// [`CoreData::log_since`] for the difference, which is what lets the Log panel append new
    /// lines instead of re-reading and re-parsing the whole ring on every batch.
    pub log_seq: u64,
    pub chart_alerts_rev: u64,
    /// Advances when typed `KernelHealth` metric values or the decoded endpoint change, gating
    /// Core Status without repainting for receipt-time-only updates.
    pub sys_rev: u64,
    /// Advances when the polled startup snapshot reports different PROGRESS, per
    /// `CoreStartupStatus::progress_eq`. Deliberately separate from `sys_rev`: that counter is
    /// documented as covering `KernelHealth` metrics and the decoded endpoint, its field is CLEARED
    /// on a new connection attempt while startup is RESTARTED, and a compound counter could not be
    /// gated on selectively by a later consumer.
    pub startup_rev: u64,
    /// Advances only when the reduced news snapshot changes, gating the News panel without
    /// repainting for duplicate frames that reduce to the same logical set.
    pub news_rev: u64,
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
            api_expiry: None,
            engine_actions: VecDeque::new(),
            chart_alerts: HashMap::new(),
            log: VecDeque::new(),
            server_log_raw: VecDeque::new(),
            sys: crate::feed::CoreSysStatus::default(),
            startup: crate::feed::CoreStartupStatus::default(),
            endpoint: None,
            news: NewsSnapshot::default(),
            news_seen_at: HashMap::new(),
            orders_table_rev: 0,
            order_lines_rev: 0,
            order_lines_rev_ms: 0,
            detects_rev: 0,
            strategies_rev: 0,
            strategies_ack_rev: 0,
            schema_rev: 0,
            assets_stale: false,
            assets_rev: 0,
            transfer_rev: 0,
            license_rev: 0,
            client_settings_rev: 0,
            lev_manage_rev: 0,
            runtime_state_rev: 0,
            hedge_mode_rev: 0,
            api_expiry_rev: 0,
            log_rev: 0,
            log_seq: 0,
            chart_alerts_rev: 0,
            sys_rev: 0,
            startup_rev: 0,
            news_rev: 0,
        }
    }

    /// Return the latest `max` core log lines from oldest to newest for the Log panel.
    pub fn log_snapshot(&self, max: usize) -> Vec<LogLine> {
        let start = self.log.len().saturating_sub(max);
        self.log.iter().skip(start).cloned().collect()
    }

    /// Return the log lines pushed since `cursor`, oldest first, and the cursor to store next.
    ///
    /// The count of missed lines comes from [`CoreData::log_seq`], so a reader gets exactly what it
    /// has not seen and never a duplicate. Two cases are deliberately NOT duplicates:
    ///
    /// * More lines arrived than the ring holds — the overflow is already evicted, so the whole ring
    ///   is returned and the gap is unrecoverable. That is the same history loss the ring imposes on
    ///   a full re-read; the alternative, returning nothing, would hide it.
    /// * `cursor` is ahead of `log_seq` — the store was rebuilt under the reader (a removed and
    ///   re-added core reuses its id). The counter restarted, so the whole ring is returned and the
    ///   caller's stale rows belong to a core that no longer exists.
    ///
    /// Args:
    ///     cursor: Value returned by the previous call, or 0 to read the whole ring.
    ///
    /// Returns:
    ///     An iterator over the unseen lines, oldest first, and the cursor for the next call.
    pub fn log_since(&self, cursor: u64) -> (impl Iterator<Item = &LogLine>, u64) {
        let missed = if cursor > self.log_seq {
            self.log.len()
        } else {
            (self.log_seq - cursor) as usize
        };
        let take = missed.min(self.log.len());
        (self.log.iter().skip(self.log.len() - take), self.log_seq)
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

    /// Begin a replacement feed without carrying endpoint-scoped telemetry across connections.
    ///
    /// Args:
    ///     self: Retained core state whose connection attempt is being replaced.
    ///
    /// Returns:
    ///     Nothing; status becomes connecting and Core Status inputs are cleared in place.
    pub(crate) fn begin_connection_attempt(&mut self) {
        self.apply(FeedMsg::Status(ConnStatus::Connecting));
        let inputs_changed =
            self.endpoint.take().is_some() || self.sys != crate::feed::CoreSysStatus::default();
        self.sys = crate::feed::CoreSysStatus::default();
        if inputs_changed {
            self.sys_rev = self.sys_rev.wrapping_add(1);
        }
        // Startup is RESTARTED by a replacement feed, not merely stale: the previous connection's
        // "came up in 8.4 s" describes a startup that is over, and carrying it would render a
        // finished figure beside a core that is connecting again. Unlike `sys` this returns to the
        // DEFAULT `Connecting` snapshot rather than an absence, because that is what is now true.
        if self.startup != crate::feed::CoreStartupStatus::default() {
            self.startup = crate::feed::CoreStartupStatus::default();
            self.startup_rev = self.startup_rev.wrapping_add(1);
        }
        // The API key belongs to the MoonBot behind the endpoint, and a replacement feed may point
        // at a different one. Keeping the previous host's day count would warn — or stay silent —
        // about a key this core no longer uses.
        if self.api_expiry.take().is_some() {
            self.api_expiry_rev = self.api_expiry_rev.wrapping_add(1);
        }
    }

    /// Apply an account-plane message to this core.
    ///
    /// The coordinator routes `Identity`, `CoreBase`, and `MarketDataChanged` without applying them
    /// to `CoreData`.
    ///
    /// Args:
    ///     msg: Typed feed update for this core.
    ///
    /// Returns:
    ///     Nothing; retained state and the relevant revision counter update in place.
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
                    // Enable this path with `channels.detect` in `cfg/diagnostics.toml`.
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
            FeedMsg::StrategiesAck => {
                self.strategies_ack_rev = self.strategies_ack_rev.wrapping_add(1);
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
            FeedMsg::Endpoint(endpoint) => {
                if self.endpoint != Some(endpoint) {
                    self.endpoint = Some(endpoint);
                    self.sys_rev = self.sys_rev.wrapping_add(1);
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
            FeedMsg::StartupStatus(startup) => {
                // Same shape as `SysStatus` above and for the same reason: retain every snapshot so
                // the panel reads the freshest figures, but bump the repaint signature ONLY when
                // the progress a reader can actually see changed. `progress_eq` also treats two
                // snapshots in the same terminal phase as equal, so a core that has finished
                // starting stops costing bumps entirely.
                let progress_changed = !self.startup.progress_eq(&startup);
                self.startup = startup;
                if progress_changed {
                    self.startup_rev = self.startup_rev.wrapping_add(1);
                }
            }
            FeedMsg::HedgeMode(on) => {
                if self.hedge_mode != Some(on) {
                    self.hedge_mode = Some(on);
                    self.hedge_mode_rev = self.hedge_mode_rev.wrapping_add(1);
                }
            }
            FeedMsg::ApiExpiry(expiry) => {
                // Compare the ANSWER, not the receipt stamp: an unchanged key answered again six
                // hours later is not a change.
                let changed = self
                    .api_expiry
                    .is_none_or(|current| !current.answer_eq(&expiry));
                self.api_expiry = Some(expiry);
                if changed {
                    self.api_expiry_rev = self.api_expiry_rev.wrapping_add(1);
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
                    let pushed = lines.len() as u64;
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
                    self.log_seq = self.log_seq.saturating_add(pushed);
                }
            }
            FeedMsg::News(mut news) => {
                // Stamp each item's terminal-receive time from the first sight of its id (the wire
                // carries none), then prune ids that dropped out of the ring so the map stays bounded.
                let now = now_unix_ms_i64();
                let mut live: HashSet<String> = HashSet::with_capacity(news.items.len());
                for item in &mut news.items {
                    let t = *self.news_seen_at.entry(item.id.clone()).or_insert(now);
                    item.recv_terminal_ms = Some(t);
                    live.insert(item.id.clone());
                }
                self.news_seen_at.retain(|id, _| live.contains(id));
                // Bump the repaint signature only when the reduced snapshot actually changed, so a
                // duplicate frame or an unchanged tags relay does not wake the panel.
                if self.news != news {
                    self.news = news;
                    self.news_rev = self.news_rev.wrapping_add(1);
                }
            }
            // Identity and market wake-up messages are not routed into this store.
            FeedMsg::Identity { .. } | FeedMsg::CoreBase { .. } | FeedMsg::MarketDataChanged(_) => {
            }
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
