//! Session startup, config reconciliation, reconnects, feed draining, and status summaries.

use std::collections::{HashMap, HashSet};

use crate::config::{AppConfig, ServerConfig};
use crate::db::ReportTx;
use crate::feed::{
    self, ConnStatus, CoreTimeOffsetStatus, EngineActionResult, FeedHandle, FeedMsg, FeedWakeTx,
};
use crate::market::{MarketDataMode, MarketDataSource, MarketStore};
use crate::session::core_time_offset::OffsetSource;
use crate::venue::CoreVenue;

use super::{
    ConnSummary, CoreId, CoreSession, CoreStore, DrainStats, LicenseSummary, SessionManager,
    conn_sig, orderbook_kind_for_exchange,
};

#[cfg(test)]
mod order_tests;

/// Position of `id` in the configured order; ids missing from `order` rank last.
pub(super) fn rank_of(order: &[CoreId], id: CoreId) -> usize {
    order.iter().position(|o| *o == id).unwrap_or(order.len())
}

/// Where `id` must be inserted into `existing` to keep it ranked by the configured order.
///
/// Operates on ids so ordering can be checked without constructing live feed handles.
/// Unranked ids tie at the tail and do not displace configured cores.
pub(super) fn insert_index(existing: &[CoreId], order: &[CoreId], id: CoreId) -> usize {
    let target = rank_of(order, id);
    existing
        .iter()
        .position(|core| rank_of(order, *core) > target)
        .unwrap_or(existing.len())
}

impl SessionManager {
    /// Register a session at its configured position for every insertion path.
    ///
    /// Also publishes the core's name for log labels: every line that identifies a core by id
    /// resolves the name through the registry, and this is the one place a session comes into being.
    fn insert_session(&mut self, session: CoreSession) {
        crate::feed::set_core_name(session.id, &session.name);
        let existing: Vec<CoreId> = self.sessions.iter().map(|s| s.id).collect();
        let at = insert_index(&existing, &self.config_order, session.id);
        self.sessions.insert(at, session);
    }

    /// Start sessions for every active server in an active group, preserving config order.
    ///
    /// The optional report channel is cloned into each feed.
    pub fn start(
        config: &AppConfig,
        epoch_ms: f64,
        reports: Option<&ReportTx>,
        feed_wake: Option<FeedWakeTx>,
    ) -> Self {
        let market = MarketStore::shared(epoch_ms);
        let market_source = MarketDataSource::new(market.clone());
        // The optional local kline cache preserves candle history across restarts.
        market_source.init_kline_cache(crate::config::paths::klines_db_path());
        let mut mgr = Self {
            sessions: Vec::new(),
            config_order: config.servers.iter().map(|s| s.id).collect(),
            feed_wake,
            store: CoreStore::default(),
            market,
            market_source,
            mode: MarketDataMode::default(),
            core_venue: HashMap::new(),
            core_base: HashMap::new(),
            core_provider: HashMap::new(),
            providers: HashMap::new(),
            wanted: HashMap::new(),
            pending_drop: HashMap::new(),
            last_cmd: HashMap::new(),
            core_updates: crate::session::core_update::CoreUpdateQueue::default(),
        };
        for s in config
            .servers
            .iter()
            .filter(|s| s.active && config.group(&s.group).active)
        {
            mgr.spawn_core(s, conn_sig(s), config.chart_memory_percent, reports);
        }
        if mgr.sessions.is_empty() {
            log::warn!("нет серверов в конфиге — добавь ядра в Настройках");
        }
        mgr.seed_time_offsets();
        mgr
    }

    /// Publish one `FeedMsg::TimeOffset` per core with a durable segment, so a restart does not
    /// show a previously-measured core as "never measured" until its next live sample arrives.
    ///
    /// Reads through [`crate::db::ReportAxis::load`] — its module doc names it "the one seam
    /// that leaves it" — rather than `db::rep::core_offset::load_all` directly: the latter sits
    /// behind `db`'s private `rep` module and is not reachable from `session`. A read failure is
    /// logged and skipped entirely rather than treated as "nothing measured": on a skewed core
    /// the empty axis is the wrong-money axis.
    ///
    /// Returns:
    ///     Nothing; each active core with a durable segment receives its seeded status.
    fn seed_time_offsets(&mut self) {
        let conn = match crate::db::open_reader() {
            Ok(conn) => conn,
            // A fresh install genuinely has no replica file yet — not a failure to seed from.
            Err(crate::db::ReadFail::NotReady) => return,
            Err(e) => {
                log::error!("core_time_offset: читатель БД недоступен для сида при старте: {e:?}");
                return;
            }
        };
        let axis = match crate::db::ReportAxis::load(&conn, chrono_tz::Tz::UTC) {
            Ok(axis) => axis,
            Err(e) => {
                log::error!("core_time_offset: не удалось прочитать сохранённые смещения: {e:?}");
                return;
            }
        };
        let now_ms = crate::util::now_unix_ms_i64();
        for core_uid in axis.measured_cores() {
            let Some(segment) = axis.segment_at(core_uid, now_ms.div_euclid(1_000)) else {
                continue;
            };
            if let Some(core) = self.store.core_mut(core_uid) {
                core.apply(FeedMsg::TimeOffset(CoreTimeOffsetStatus {
                    offset_secs: Some(segment.offset_secs),
                    // The segment's OWN start, not the seed time: a value measured last week must
                    // not come back from a restart claiming it was measured at boot. That is the
                    // whole reason `segment_at` returns the segment rather than just its offset.
                    observed_at_utc: segment.from_utc.saturating_mul(1_000),
                    // A seeded value has no live samples behind it and must not claim any.
                    samples: 0,
                    source: OffsetSource::Log,
                }));
            }
        }
    }

    /// Start a core feed thread with `feed::spawn` and register its market client.
    fn spawn_feed(&self, server: ServerConfig, mem: u16, reports: Option<&ReportTx>) -> FeedHandle {
        let id = server.id;
        let handle = feed::spawn(
            server,
            mem,
            reports.cloned(),
            self.feed_wake.clone(),
            Some(self.market.clone()),
        );
        self.market_source.set_client(id, handle.client.clone());
        handle
    }

    /// Clear a core's market coordination state.
    ///
    /// After a respawn or reconnect, later `Identity` and `CoreBase` events may repopulate the key
    /// and base before coordination recomputes the provider role and last command. Removal leaves
    /// this state absent.
    fn clear_core_coordination(&mut self, id: CoreId) {
        self.core_venue.remove(&id);
        self.core_base.remove(&id);
        self.core_provider.remove(&id);
        self.providers.retain(|_, prov| *prov != id);
        self.last_cmd.remove(&id);
    }

    /// Reconcile live sessions with the config without restarting unaffected feeds.
    ///
    /// Adds active cores, removes inactive or deleted cores, respawns feeds whose connection
    /// fields changed, updates metadata in place, and restores config order.
    pub fn reconcile(&mut self, config: &AppConfig, reports: Option<&ReportTx>) {
        let mem = config.chart_memory_percent;
        // Refresh the rank table first: a reorder or a newly added server must be visible to
        // every insertion made below.
        self.config_order = config.servers.iter().map(|s| s.id).collect();
        // An existing session may now rank differently — a server was added, removed, or the
        // import rewrote the list — and reordering touches no feed thread. This is CONFIG
        // order, which decides where a reactivated session is inserted; the order the user
        // sees is applied separately by the UI's `core_order` module.
        let order = self.config_order.clone();
        self.sessions.sort_by_key(|s| rank_of(&order, s.id));
        let desired: Vec<ServerConfig> = config
            .servers
            .iter()
            .filter(|s| s.active && config.group(&s.group).active)
            .cloned()
            .collect();
        let desired_ids: HashSet<CoreId> = desired.iter().map(|s| s.id).collect();

        // Stop cores removed from the active server and group set.
        let removed: Vec<CoreId> = self
            .sessions
            .iter()
            .map(|s| s.id)
            .filter(|id| !desired_ids.contains(id))
            .collect();
        for id in removed {
            self.drop_core(id);
        }

        // Start new cores, respawn changed connections, and update other metadata in place.
        for s in &desired {
            let sig = conn_sig(s);
            match self.sessions.iter().position(|x| x.id == s.id) {
                None => self.spawn_core(s, sig, mem, reports),
                Some(idx) => {
                    // A rename alone does not touch the connection, so the label registry has to be
                    // updated here too — otherwise logs keep the old name until the next respawn.
                    feed::set_core_name(s.id, &s.name);
                    self.sessions[idx].name = s.name.clone();
                    self.sessions[idx].group = s.group.clone();
                    if self.sessions[idx].conn_sig != sig {
                        // Connection changes require a fresh feed handle.
                        self.respawn_session(
                            s.clone(),
                            sig,
                            mem,
                            reports,
                            "reconnect (config changed)",
                        );
                    }
                }
            }
        }
    }

    /// Start and register a new core at its configured position.
    fn spawn_core(&mut self, s: &ServerConfig, sig: u64, mem: u16, reports: Option<&ReportTx>) {
        let id = s.id;
        // Reject duplicate CoreIds before `spawn_feed` registers a market client. Dropping a
        // rejected handle afterwards would clear the existing session's client slot.
        if self.sessions.iter().any(|x| x.id == id) {
            log::warn!("duplicate CoreId {id} — вторая сессия не поднята");
            return;
        }
        self.store.ensure(id);
        // Publish the name BEFORE the feed thread starts: its first lines are logged from that
        // thread and would otherwise carry a bare id.
        feed::set_core_name(id, &s.name);
        let handle = self.spawn_feed(s.clone(), mem, reports);
        self.insert_session(CoreSession {
            id,
            name: s.name.clone(),
            group: s.group.clone(),
            conn_sig: sig,
            handle,
        });
        log::info!("session up: core={}", crate::feed::core_label(id));
    }

    /// Replace a core feed or insert the missing session, then reset provider coordination.
    ///
    /// Args:
    ///     server: Updated configuration used to spawn the replacement feed.
    ///     sig: Connection-sensitive configuration signature.
    ///     mem: Chart-history memory budget.
    ///     reports: Optional report-database channel.
    ///     why: Diagnostic reason logged after replacement.
    ///
    /// Returns:
    ///     Nothing; session, connection-scoped state, and coordination update in place.
    fn respawn_session(
        &mut self,
        server: ServerConfig,
        sig: u64,
        mem: u16,
        reports: Option<&ReportTx>,
        why: &str,
    ) {
        let id = server.id;
        let name = server.name.clone();
        let group = server.group.clone();
        // Before the replacement thread starts, for the same reason as in `spawn_core`.
        feed::set_core_name(id, &name);
        let handle = self.spawn_feed(server, mem, reports);
        match self.sessions.iter_mut().find(|s| s.id == id) {
            Some(sess) => {
                sess.handle = handle; // Dropping the old handle stops its feed thread.
                sess.conn_sig = sig;
                // A rename arrives as a respawn, so log labels follow it from here on.
                crate::feed::set_core_name(id, &name);
                sess.name = name;
                sess.group = group;
            }
            None => self.insert_session(CoreSession {
                id,
                name,
                group,
                conn_sig: sig,
                handle,
            }),
        }
        self.store.ensure(id);
        if let Some(core) = self.store.core_mut(id) {
            core.begin_connection_attempt();
        }
        self.clear_core_coordination(id);
        log::info!("{why}: core={}", crate::feed::core_label(id));
    }

    /// Stop a removed or deactivated core and clear its account data, market client, and
    /// coordination state. Dropping the session terminates its feed thread.
    fn drop_core(&mut self, id: CoreId) {
        self.sessions.retain(|s| s.id != id); // Dropping FeedHandle terminates the thread.
        self.store.remove(id);
        self.market_source.remove_client(id);
        self.clear_core_coordination(id);
        self.wanted.remove(&id);
        self.pending_drop.retain(|(core, _), _| *core != id);
        log::info!("session down: core={}", crate::feed::core_label(id));
        // Release the name AFTER the line above, so the last line still names the core; a later id
        // reuse must not inherit it.
        crate::feed::clear_core_name(id);
    }

    /// Drain every core channel into the session stores.
    ///
    /// Account messages go to `CoreStore`. Live and synthetic feeds publish market payloads to the
    /// read model or `MarketStore` and send only a lightweight `MarketDataChanged` wake-up here.
    /// Identity messages populate `core_venue`. This event-driven drain is independent of the
    /// coordination loop that calls `set_open`; the result separates general UI state from data
    /// that can change chart pixels.
    pub fn drain(&mut self) -> DrainStats {
        let mut stats = DrainStats::default();
        for sess in &self.sessions {
            while let Ok(msg) = sess.handle.rx.try_recv() {
                stats.any = true;
                match msg {
                    FeedMsg::Identity { id, dex, reported } => {
                        // One insert replaces the whole entry, so a reconnect onto a different
                        // venue cannot leave the previous caption or DEX name behind.
                        self.core_venue
                            .insert(sess.id, CoreVenue::identify(id.code, &dex, Some(&reported)));
                        self.market_source
                            .set_orderbook_kind(sess.id, orderbook_kind_for_exchange(id));
                        stats.ui_state = true;
                    }
                    FeedMsg::CoreBase { base } => {
                        self.core_base.insert(sess.id, base);
                        stats.ui_state = true;
                    }
                    FeedMsg::MarketDataChanged(markets) => {
                        if !markets.is_empty() {
                            self.market_source.mark_dirty(sess.id, &markets);
                            stats.market_data = true;
                        }
                    }
                    FeedMsg::Orders(orders) => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            let before = core.order_lines_rev;
                            core.apply(FeedMsg::Orders(orders));
                            stats.order_lines_data |= core.order_lines_rev != before;
                            stats.ui_state = true;
                        }
                    }
                    FeedMsg::OrderLines(orders) => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            let before = core.order_lines_rev;
                            core.apply(FeedMsg::OrderLines(orders));
                            let changed = core.order_lines_rev != before;
                            stats.order_lines_data |= changed;
                            stats.ui_state |= changed;
                        }
                    }
                    other => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            core.apply(other);
                            stats.ui_state = true;
                        }
                    }
                }
            }
        }
        stats
    }

    /// Debug-only stress fixture for the dev panel fill button.
    ///
    /// Production/default terminal builds deliberately do not enable MoonProto's
    /// diagnostics feature. The real hook is available only through the explicit
    /// `moonproto-diagnostics` feature, which `moon-ui-gpui/debug-tools` enables
    /// for local/manual stress runs.
    #[cfg(feature = "moonproto-diagnostics")]
    pub fn diag_fill_market_history_to_capacity(
        &self,
        core: CoreId,
        market: &str,
        now_ms: i64,
        span_ms: i64,
    ) -> bool {
        let provider = self.core_provider.get(&core).copied().unwrap_or(core);
        let Some(sess) = self.sessions.iter().find(|s| s.id == provider) else {
            log::warn!(
                "diag history fill skipped: provider core={} not found",
                crate::feed::core_label(provider)
            );
            return false;
        };
        let Some(client) = sess.handle.client.get() else {
            log::warn!(
                "diag history fill skipped: provider core={} has no MoonProto client",
                crate::feed::core_label(provider)
            );
            return false;
        };

        match client.diag_fill_market_history_to_capacity(market, now_ms, span_ms) {
            Ok(filled) => {
                log::info!(
                    "diag history fill: core={} provider={} market={market} \
                     span_ms={span_ms} filled={filled}",
                    crate::feed::core_label(core),
                    crate::feed::core_label(provider),
                );
                filled
            }
            Err(err) => {
                log::warn!(
                    "diag history fill failed: core={} provider={} \
                     market={market} span_ms={span_ms}: {err:#}",
                    crate::feed::core_label(core),
                    crate::feed::core_label(provider)
                );
                false
            }
        }
    }

    /// Same public method in production builds: the button path stays compiled,
    /// but hidden MoonProto diagnostics are not pulled into the dependency graph.
    #[cfg(not(feature = "moonproto-diagnostics"))]
    pub fn diag_fill_market_history_to_capacity(
        &self,
        core: CoreId,
        market: &str,
        now_ms: i64,
        span_ms: i64,
    ) -> bool {
        let provider = self.core_provider.get(&core).copied().unwrap_or(core);
        log::warn!(
            "diag history fill disabled: MoonProto diagnostics feature is forbidden in terminal \
             core={} provider={} market={market} now_ms={now_ms} span_ms={span_ms}",
            crate::feed::core_label(core),
            crate::feed::core_label(provider)
        );
        false
    }

    /// Drain queued Engine action results from every core as `(core name, result)` pairs.
    ///
    /// Only the active window's shell calls this method, so each toast appears exactly once; at
    /// most one OS window is active at a time.
    pub fn take_engine_action_toasts(&mut self) -> Vec<(String, EngineActionResult)> {
        let mut out = Vec::new();
        for sess in &self.sessions {
            if let Some(core) = self.store.core_mut(sess.id) {
                for r in core.take_engine_actions() {
                    out.push((sess.name.clone(), r));
                }
            }
        }
        out
    }

    /// Return an owned snapshot of every core's connection status for Settings window badges.
    pub fn status_map(&self) -> HashMap<CoreId, ConnStatus> {
        self.store.statuses().collect()
    }

    /// Return the retained connection FAULT of every core that has one, for the Settings badges.
    ///
    /// Separate from [`Self::status_map`] rather than folded into it: most cores never have a
    /// fault, so the map stays small, and the two are consumed by different code paths — the badge
    /// colour reads the status, only the tooltip reads the reason.
    ///
    /// Returns:
    ///     A core-id-keyed snapshot containing only cores with a retained fault.
    pub fn fault_map(&self) -> HashMap<CoreId, crate::feed::ConnFault> {
        self.sessions
            .iter()
            .filter_map(|s| {
                self.store
                    .core(s.id)
                    .and_then(|d| d.fault.clone())
                    .map(|f| (s.id, f))
            })
            .collect()
    }

    /// Return the retained STARTUP snapshot of every core, for the Settings badges.
    ///
    /// The still-syncing verdict is derived from the startup snapshot rather than from a fault —
    /// a core that merely never finished coming up produces no failure event at all — so the
    /// tooltip needs both maps.
    ///
    /// Returns:
    ///     A core-id-keyed startup snapshot for every core still retained by the session store.
    pub fn startup_map(&self) -> HashMap<CoreId, crate::feed::CoreStartupStatus> {
        self.sessions
            .iter()
            .filter_map(|s| self.store.core(s.id).map(|d| (s.id, d.startup)))
            .collect()
    }

    /// Summarize connections in one group as ready/total counts and non-ready core details.
    ///
    /// A group corresponds to an OS window, so each status bar reports only its own group. The
    /// summary includes headless cores because they still have sessions; `show_window` controls
    /// only whether the window exists.
    /// How many cores have not settled yet — still `Connecting`, or partway through an init
    /// `Stage`. A session with no store entry at all counts as pending: it has not reported.
    ///
    /// "Settled" deliberately includes `Failed` and `Disconnected`. The question this answers is
    /// "has the machine finished coming up", not "is every core healthy" — a core that will never
    /// connect must not hold a caller forever. Callers that need a WORKING core must check for
    /// `Ready` themselves.
    ///
    /// Exists for diagnostic runs, which otherwise sleep a fixed interval before measuring: that is
    /// wrong in both directions, wasteful with one core and far too short with twenty.
    pub fn cores_pending(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| {
                let st = self
                    .store
                    .core(s.id)
                    .map(|d| d.status.clone())
                    .unwrap_or(ConnStatus::Connecting);
                matches!(st, ConnStatus::Connecting | ConnStatus::Stage(_))
            })
            .count()
    }

    /// Summarize one window group's connection state with the evidence needed for its status bar.
    ///
    /// A non-ready row carries its retained fault and startup snapshot as well as its coarse
    /// status, because the bar must explain the current retry without reaching back into the store
    /// after it has made the group-scoped snapshot.
    ///
    /// Args:
    ///     group: Configured window group whose sessions are summarized.
    ///
    /// Returns:
    ///     Ready and total counts plus canonically ordered non-ready core diagnostics.
    pub fn conn_summary_group(&self, group: &str) -> ConnSummary {
        let mut total = 0;
        let mut ready = 0;
        let mut down = Vec::new();
        for s in self.sessions.iter().filter(|s| s.group == group) {
            total += 1;
            let core = self.store.core(s.id);
            let st = core
                .map(|d| d.status.clone())
                .unwrap_or(ConnStatus::Connecting);
            if st == ConnStatus::Ready {
                ready += 1;
            } else {
                down.push(crate::session::ConnDown {
                    id: s.id,
                    name: s.name.clone(),
                    status: st,
                    fault: core.and_then(|d| d.fault.clone()),
                    startup: core.map(|d| d.startup).unwrap_or_default(),
                });
            }
        }
        ConnSummary { ready, total, down }
    }

    /// Summarize license state for the cores in one group.
    ///
    /// License data arrives after connection and initialization, so `known` may be less than
    /// `total`; the UI must represent that distinction.
    pub fn license_summary_group(&self, group: &str) -> LicenseSummary {
        let mut out = LicenseSummary::default();
        for s in self.sessions.iter().filter(|s| s.group == group) {
            out.total += 1;
            let Some(core) = self.store.core(s.id) else {
                continue;
            };
            let Some(license) = core.license else {
                continue;
            };
            out.known += 1;
            if license.paid_version {
                out.paid += 1;
            } else {
                out.free += 1;
            }
            out.moon_credits += i64::from(license.moon_credits);
            out.moon_credits_hold += i64::from(license.moon_credits_hold);
            out.moon_credits_auction += i64::from(license.moon_credits_auction);
        }
        out
    }

    /// Reconnect one active core from the current config and reset its provider role.
    pub fn reconnect(&mut self, id: CoreId, config: &AppConfig, reports: Option<&ReportTx>) {
        let Some(server) = config.servers.iter().find(|s| s.id == id).cloned() else {
            return;
        };
        if !(server.active && config.group(&server.group).active) {
            return;
        }
        // Refresh ranks because reconnect may insert a missing session without reconciliation.
        self.config_order = config.servers.iter().map(|s| s.id).collect();
        let sig = conn_sig(&server);
        self.respawn_session(
            server,
            sig,
            config.chart_memory_percent,
            reports,
            "reconnect",
        );
    }
}
