//! Methods for the application's shared backend state ([`Backend`]). The struct is declared in
//! `main.rs`, the crate root, so its private fields are visible to descendant modules. Methods in
//! this sibling module use `pub(crate)` because private items here would not be crate-wide.

pub(crate) mod core_warn;
mod detect_sound;
mod figures;
pub(crate) mod server_chart;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use gpui::Context;

use crate::Backend;
use crate::chartdx::ChartDataHandle;
use crate::core_order::{CoreOrder, OrderedCores};
use moon_core::config::{
    DEFAULT_ORDER_SIZES_USD, GroupExitSettings, GroupTradeSettings, TakeProfitMode,
};
use moon_core::feed::ClientSettingsEdit;
use moon_core::session::CoreId;

/// Seconds of history kept on each side of a warning start for its card graph.
const WARN_SLICE_BACK_MS: i64 = 60_000;
const WARN_SLICE_FWD_MS: i64 = 60_000;
/// Cap on episodes queued for slice capture, so a burst of warnings cannot grow the queue without
/// bound; the oldest pending capture is dropped past it.
const WARN_PENDING_CAP: usize = 1024;

/// A closed, persisted episode whose ±1 min history slice is captured later — once its forward tail
/// (`start_ms + WARN_SLICE_FWD_MS`) has accumulated in the live ring — and written to `warn_store`.
pub(crate) struct PendingWarnSlice {
    /// The `core_warnings` row id the slice is keyed to.
    pub(crate) episode_id: i64,
    /// Server whose history ring the slice is read from.
    pub(crate) ip: IpAddr,
    /// The episode start; the window is `[start - BACK, start + FWD]`.
    pub(crate) start_ms: i64,
    /// Unix ms at which the forward tail is expected to be present.
    pub(crate) capture_at_ms: i64,
}

/// Slice a history ring to the ±1 min window around `at_ms`, positionally at 1 Hz (the same read the
/// live card uses). `None` when the ring is absent, too short, or does not reach the window.
fn ring_slice<T: Copy>(
    ring: Option<&std::collections::VecDeque<T>>,
    at_ms: i64,
    now_ms: i64,
) -> Option<Vec<T>> {
    let ring = ring?;
    let len = ring.len();
    if len < 2 {
        return None;
    }
    let now_sec = now_ms / 1000;
    let at_sec = at_ms / 1000;
    // Window edges in seconds, derived from the same constants that set `base_ms`/`capture_at_ms`.
    let back = WARN_SLICE_BACK_MS / 1000;
    let fwd = WARN_SLICE_FWD_MS / 1000;
    // Seconds back from now to each window edge; the newer edge (at+fwd) may still be the future.
    let k_lo = (now_sec - (at_sec - back)).max(0) as usize;
    let k_hi = (now_sec - (at_sec + fwd)).max(0) as usize;
    let lo = len.saturating_sub(1 + k_lo);
    let hi = len.saturating_sub(1 + k_hi);
    if hi.saturating_sub(lo) < 1 {
        return None;
    }
    Some(ring.iter().skip(lo).take(hi - lo + 1).copied().collect())
}

/// Capture and persist one episode's ±1 min server slice from the current ring, overwriting any
/// earlier partial capture for it (the store uses `INSERT OR REPLACE`). A ring with no data in the
/// window writes nothing, so the card simply shows no graph.
fn capture_series(
    store: &crate::backend::core_warn::store::WarnStore,
    ring: Option<&std::collections::VecDeque<(u8, u8)>>,
    ping_ring: Option<&std::collections::VecDeque<u16>>,
    exch_ring: Option<&std::collections::VecDeque<u16>>,
    episode_id: i64,
    start_ms: i64,
    now_ms: i64,
) {
    let base_ms = start_ms - WARN_SLICE_BACK_MS;
    if let Some(samples) = ring_slice(ring, start_ms, now_ms) {
        if let Err(err) = store.insert_series(episode_id, "server", base_ms, &samples) {
            log::warn!("core warning slice persist failed: {err}");
        }
    }
    // Each ping slice rides the same window under its own subject (a u16 blob).
    if let Some(pings) = ring_slice(ping_ring, start_ms, now_ms) {
        if let Err(err) = store.insert_ping_series(episode_id, "ping", base_ms, &pings) {
            log::warn!("core warning ping slice persist failed: {err}");
        }
    }
    if let Some(exch) = ring_slice(exch_ring, start_ms, now_ms) {
        if let Err(err) = store.insert_ping_series(episode_id, "exch", base_ms, &exch) {
            log::warn!("core warning exch slice persist failed: {err}");
        }
    }
}

/// Push a pending capture, evicting the oldest if the queue is at its cap.
fn push_pending_slice(queue: &mut Vec<PendingWarnSlice>, item: PendingWarnSlice) {
    if queue.len() >= WARN_PENDING_CAP {
        queue.remove(0);
    }
    queue.push(item);
}

/// Apply one visible toolbar edit with the same wire quantization used by MoonProto.
fn apply_group_exit_edit(exit: &mut GroupExitSettings, edit: ClientSettingsEdit) -> bool {
    match edit {
        ClientSettingsEdit::TakeProfit { pct, extended } => {
            let mode = if extended {
                TakeProfitMode::Extended
            } else {
                TakeProfitMode::Normal
            };
            let Some(pct) = mode.canonical_take_profit_pct(pct) else {
                return false;
            };
            exit.take_profit_mode = mode;
            exit.take_profit_pct = pct;
            for fixed_pct in &mut exit.fixed_sell_pcts {
                *fixed_pct = exit
                    .take_profit_mode
                    .canonical_fixed_sell_pct(*fixed_pct)
                    .unwrap_or_default();
            }
            exit.fixed_sell_slot = None;
        }
        ClientSettingsEdit::ScalpTakeProfit(pct) => {
            let Some(pct) = TakeProfitMode::Scalp.canonical_take_profit_pct(pct) else {
                return false;
            };
            exit.take_profit_mode = TakeProfitMode::Scalp;
            exit.take_profit_pct = pct;
            for fixed_pct in &mut exit.fixed_sell_pcts {
                *fixed_pct = exit
                    .take_profit_mode
                    .canonical_fixed_sell_pct(*fixed_pct)
                    .unwrap_or_default();
            }
            exit.fixed_sell_slot = None;
        }
        ClientSettingsEdit::StopLossPct(pct) => {
            let Some(pct) = GroupExitSettings::canonical_stop_loss_pct(pct) else {
                return false;
            };
            exit.stop_loss_pct = pct;
        }
        ClientSettingsEdit::SelectFixedSellSlot(slot) if (1..=6).contains(&slot) => {
            exit.fixed_sell_slot = Some(slot);
        }
        ClientSettingsEdit::EngageMainTakeProfit => exit.fixed_sell_slot = None,
        ClientSettingsEdit::SetFixedSellPct { slot, pct } if (1..=6).contains(&slot) => {
            let Some(pct) = exit.take_profit_mode.canonical_fixed_sell_pct(pct) else {
                return false;
            };
            exit.fixed_sell_pcts[slot - 1] = pct;
        }
        ClientSettingsEdit::UseStopMarket(on) => exit.use_stop_market = on,
        ClientSettingsEdit::PanicIfPriceDrop(on) => exit.stop_loss_enabled = on,
        _ => return false,
    }
    true
}

/// Mirror one live toolbar mutation into an open Settings preview without replacing draft fields.
fn update_group_trade_pair(
    live: &mut GroupTradeSettings,
    preview: Option<&mut GroupTradeSettings>,
    update: impl Fn(&mut GroupTradeSettings),
) {
    update(live);
    if let Some(preview) = preview {
        update(preview);
    }
}

/// Convert a positive USD equivalent to base quantity, rejecting unavailable or invalid rates.
fn usd_to_base_amount(usd: f64, rate: Option<f64>) -> Option<f64> {
    let rate = rate?;
    if !(usd.is_finite() && usd > 0.0 && rate.is_finite() && rate > 0.0) {
        return None;
    }
    let size = usd / rate;
    (size.is_finite() && size > 0.0).then_some(size)
}

/// Group-owned terms resolved before one manual order is submitted to a core.
pub(crate) struct ManualOrderTerms {
    /// Base-currency quantity sent to the target core.
    pub(crate) size_base: f64,
    /// Visible USD equivalent, absent when an isolated FireTest overrides the base size.
    pub(crate) size_usd: Option<f64>,
    /// Complete visible exit generation serialized before the order.
    pub(crate) exit: GroupExitSettings,
}

impl Backend {
    /// Return the six USD-equivalent presets and selected slot for one window group.
    pub(crate) fn manual_order_size_state(&self, group: &str) -> ([f64; 6], usize) {
        self.config
            .group_ref(group)
            .map_or((DEFAULT_ORDER_SIZES_USD, 2), |group| {
                (
                    group.trade.order_sizes_usd,
                    group
                        .trade
                        .order_size_sel
                        .min(group.trade.order_sizes_usd.len() - 1),
                )
            })
    }

    /// Select an F1-F6 USD-equivalent preset for one group.
    pub(crate) fn set_order_size_sel(&mut self, group: &str, ix: usize) {
        if ix >= 6 {
            return;
        }
        self.update_group_trade(group, |trade| trade.order_size_sel = ix);
    }

    /// Return the selected USD-equivalent order amount for one group.
    pub(crate) fn manual_order_size_usd(&self, group: &str) -> f64 {
        let (sizes, sel) = self.manual_order_size_state(group);
        sizes[sel]
    }

    /// Convert a target core's group-local USD amount into that core's base currency.
    pub(crate) fn manual_order_size_base(&self, core: CoreId) -> Option<(f64, f64)> {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server.id == core)?;
        let usd = self.manual_order_size_usd(&server.group);
        let base = self.session.core_base(core)?;
        let size = usd_to_base_amount(
            usd,
            self.session.market_source().currency_usd_rate(core, base),
        )?;
        Some((size, usd))
    }

    /// Return the visible USD equivalent only when the target core can currently convert it.
    pub(crate) fn prospective_order_usd(&self, core: CoreId) -> Option<f64> {
        self.manual_order_size_base(core).map(|(_, usd)| usd)
    }

    /// Resolve group-owned exit settings and either the visible USD size or a FireTest override.
    pub(crate) fn manual_order_terms(
        &self,
        core: CoreId,
        size_base_override: Option<f64>,
    ) -> Option<ManualOrderTerms> {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server.id == core)?;
        let exit = self.group_exit_settings(&server.group);
        let (size_base, size_usd) = match size_base_override {
            Some(size) if size.is_finite() && size > 0.0 => (size, None),
            Some(_) => return None,
            None => {
                let (size, usd) = self.manual_order_size_base(core)?;
                (size, Some(usd))
            }
        };
        Some(ManualOrderTerms {
            size_base,
            size_usd,
            exit,
        })
    }

    /// Return one group-local USD-equivalent F1-F6 preset.
    pub(crate) fn order_size_value(&self, group: &str, ix: usize) -> f64 {
        let (sizes, _) = self.manual_order_size_state(group);
        sizes[ix.min(sizes.len().saturating_sub(1))]
    }

    /// Write one group-local USD-equivalent F1-F6 preset.
    pub(crate) fn set_order_size_value(&mut self, group: &str, ix: usize, value: f64) {
        if ix >= 6 || !(value.is_finite() && value > 0.0) {
            return;
        }
        self.update_group_trade(group, |trade| trade.order_sizes_usd[ix] = value);
    }

    /// Return complete visible group exits, falling back to the neutral standard before repair.
    pub(crate) fn group_exit_settings(&self, group: &str) -> GroupExitSettings {
        self.config
            .group_ref(group)
            .map(|group| group.trade.exit)
            .unwrap_or_default()
    }

    /// Return one visible S1-S6 percentage for a group.
    pub(crate) fn fixed_sell_pct(&self, group: &str, ix: usize) -> f64 {
        self.group_exit_settings(group).fixed_sell_pcts[ix.min(5)]
    }

    /// Apply a visible ClientSettings edit to the group's local source of truth.
    pub(crate) fn edit_group_exit(&mut self, group: &str, edit: ClientSettingsEdit) -> bool {
        let mut exit = self.group_exit_settings(group);
        if !apply_group_exit_edit(&mut exit, edit) {
            return false;
        }
        self.update_group_trade(group, |trade| trade.exit = exit);
        true
    }

    /// Apply one group-trade mutation to both live config and an open Settings preview.
    fn update_group_trade(&mut self, group: &str, update: impl Fn(&mut GroupTradeSettings)) {
        let live = &mut self.config.group_mut(group).trade;
        let preview = self
            .preview
            .as_mut()
            .map(|preview| &mut preview.group_mut(group).trade);
        update_group_trade_pair(live, preview, update);
        self.config_dirty = true;
    }

    /// Synchronize each group's complete local exit generation to every live core in that group.
    pub(crate) fn sync_group_manual_settings(&mut self) {
        let live_ids: HashSet<CoreId> = self
            .session
            .sessions()
            .iter()
            .map(|session| session.id)
            .collect();
        self.group_exit_sync
            .retain(|core, _| live_ids.contains(core));

        for server in self
            .config
            .servers
            .iter()
            .filter(|server| server.active && live_ids.contains(&server.id))
        {
            let exit = self.group_exit_settings(&server.group);
            let (revision, ready, matches) = self
                .session
                .store()
                .core(server.id)
                .map(|data| {
                    (
                        data.client_settings_rev,
                        data.status == moon_core::feed::ConnStatus::Ready,
                        data.client_settings
                            .as_ref()
                            .is_some_and(|settings| settings.group_exit_settings() == exit),
                    )
                })
                .unwrap_or((0, false, false));
            let generation = (exit, revision, ready);
            if matches {
                self.group_exit_sync.insert(server.id, generation);
                continue;
            }
            if self.group_exit_sync.get(&server.id) == Some(&generation) {
                continue;
            }
            if let Err(error) = self.session.sync_group_exit(server.id, exit) {
                log::warn!(
                    "group manual settings sync failed: core={} group={}: {error:#}",
                    server.id,
                    server.group
                );
                continue;
            }
            self.group_exit_sync.insert(server.id, generation);
        }
    }

    /// Store a process-lifetime local manual-strategy override for immediate feedback.
    pub(crate) fn set_manual_strat_local(&mut self, core: CoreId, on: bool, id: u64) {
        self.manual_strat_local.insert(core, (on, id));
    }

    /// Return the core's effective manual-strategy state as `(enabled, id)`.
    ///
    /// A local override takes precedence over the `ClientSettings` snapshot and remains until
    /// replaced or process exit; core echoes and command failures do not reconcile it. If neither
    /// source exists, this returns `(false, 0)`.
    pub(crate) fn manual_strat_state(&self, core: CoreId) -> (bool, u64) {
        if let Some(v) = self.manual_strat_local.get(&core) {
            return *v;
        }
        self.session
            .store()
            .core(core)
            .and_then(|d| d.client_settings.as_ref())
            .map(|s| (s.use_manual_strategy, s.manual_strategy_id))
            .unwrap_or((false, 0))
    }

    /// Return whether panic sell is armed for `(core, market)` to highlight the Panic Sell button.
    ///
    /// The state is the union of the retained order-line snapshot and the process-local armed set.
    pub(crate) fn is_panic_armed(&self, core: CoreId, market: &str) -> bool {
        let snapshot_armed = self.session.store().core(core).is_some_and(|data| {
            data.order_lines
                .iter_market(market)
                .any(|order| order.closed_ms.is_none() && order.panic_sell)
        });
        if snapshot_armed {
            return true;
        }
        self.panic_armed.contains(&(core, market.to_string()))
    }

    /// Toggle panic sell for a market using the union of the order-line snapshot and local armed set.
    ///
    /// A successfully enabled local entry survives core updates until a later toggle removes it.
    pub(crate) fn toggle_panic_sell(&mut self, core: CoreId, market: String) -> bool {
        let key = (core, market.clone());
        let on = !self.is_panic_armed(core, &market);
        if let Err(error) = self.session.panic_sell_market(core, market, on) {
            log::warn!("panic sell market failed: {error:#}");
            return !on;
        }
        if on {
            self.panic_armed.insert(key);
        } else {
            self.panic_armed.remove(&key);
        }
        on
    }

    /// Cancel pending buy orders across all markets for a core for the "cancel all buys" hotkey.
    ///
    /// The retained order snapshot supplies unique markets with a pending, non-short order whose
    /// job is not done. A `cancel_market_buys` request is sent for each market, and the return value
    /// is the number of requests accepted.
    pub(crate) fn cancel_all_buys_for_core(&self, core: CoreId) -> usize {
        let markets: Vec<String> = self
            .session
            .store()
            .core(core)
            .map(|cd| {
                let mut set = std::collections::BTreeSet::new();
                for o in &cd.orders {
                    if !o.is_short && o.pending && !o.job_is_done {
                        set.insert(o.market.clone());
                    }
                }
                set.into_iter().collect()
            })
            .unwrap_or_default();
        let mut n = 0;
        for m in markets {
            n += self.cancel_buy_orders(core, &m);
        }
        n
    }

    /// Return the market position side for `join_sells`, where `true` means short.
    ///
    /// The first matching order in the retained snapshot determines the side; absent a match, the
    /// position defaults to long.
    pub(crate) fn market_position_short(&self, core: CoreId, market: &str) -> bool {
        self.session
            .store()
            .core(core)
            .and_then(|cd| {
                cd.orders
                    .iter()
                    .find(|o| o.market == market)
                    .map(|o| o.is_short)
            })
            .unwrap_or(false)
    }

    pub(crate) fn cancel_buy_orders(&self, core: CoreId, market: &str) -> usize {
        match self.session.cancel_market_buys(core, market.to_string()) {
            Ok(()) => {
                log::info!("cancel buy: requested market buys for core={core} market={market}");
                1
            }
            Err(err) => {
                log::warn!("cancel buy failed: core={core} market={market}: {err:#}");
                0
            }
        }
    }

    pub(crate) fn register_chart_consumer(&mut self, chart: ChartDataHandle) {
        if self
            .chart_consumers
            .iter()
            .any(|existing| existing == &chart)
        {
            return;
        }
        self.chart_consumers.push(chart);
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn register_debug_main_chart(&mut self, group: String, chart: ChartDataHandle) {
        self.debug_main_chart_handles.insert(group, chart);
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_main_chart_shift_hz(&self, group: &str) -> Option<f32> {
        self.debug_main_chart_handles
            .get(group)
            .filter(|chart| chart.is_alive())
            .and_then(ChartDataHandle::camera_shift_hz)
    }

    pub(crate) fn live_chart_consumers(&mut self) -> Vec<ChartDataHandle> {
        self.chart_consumers.retain(ChartDataHandle::is_alive);
        self.chart_consumers.clone()
    }

    /// Return whether a live core currently belongs to a Main window group.
    ///
    /// Args:
    ///     group: Window group that must own the core.
    ///     core: Stable core UID to validate.
    ///
    /// Returns:
    ///     `true` only while the matching live session remains in `group`.
    pub(crate) fn core_belongs_to_group(&self, group: &str, core: CoreId) -> bool {
        self.session
            .sessions()
            .iter()
            .any(|session| session.id == core && session.group == group)
    }

    /// Seed the group's runtime Main target without replacing a durable manual selection.
    ///
    /// Construction publishes restored Main state once after startup. Treating that baseline as a
    /// user-visible target change would immediately overwrite the core restored from layout.toml.
    ///
    /// Args:
    ///     group: Window group whose initial target is being published.
    ///     target: Restored active core and market, or `None`. Invalid cross-group targets are
    ///         treated as absent.
    ///
    /// Returns:
    ///     Nothing; only the process-lifetime target cache is initialized.
    pub(crate) fn initialize_main_chart_target(
        &mut self,
        group: &str,
        target: Option<(CoreId, String)>,
    ) {
        let target = target.filter(|(core, _)| self.core_belongs_to_group(group, *core));
        self.store_main_chart_target(group, target);
    }

    /// Publish the group's current Main trading target and remember a genuine core change.
    ///
    /// Repeated synchronization of the same target preserves a manual header selection. Moving
    /// Main or a locked comparison anchor to another core makes that core the new durable
    /// selection, matching what the header shows after the existing sticky choice yields.
    ///
    /// Args:
    ///     group: Window group whose target changed.
    ///     target: Active core and market, or `None` when no Main trading target exists. A target
    ///         whose live core no longer belongs to `group` is treated as `None`.
    ///
    /// Returns:
    ///     Nothing; runtime target state and, on a core change, layout state are updated in place.
    pub(crate) fn set_main_chart_target(&mut self, group: &str, target: Option<(CoreId, String)>) {
        let target = target.filter(|(core, _)| self.core_belongs_to_group(group, *core));
        if self.main_chart_targets.get(group) == target.as_ref() {
            return;
        }
        let prev_core = self.main_chart_targets.get(group).map(|(core, _)| *core);
        if let Some((new_core, _)) = &target {
            if prev_core != Some(*new_core) {
                self.set_active_trade_core(group, *new_core);
            }
        }
        self.store_main_chart_target(group, target);
    }

    /// Replace one group's process-lifetime Main target cache entry.
    ///
    /// Args:
    ///     group: Window group that owns the entry.
    ///     target: Validated core and market, or `None` to remove the entry.
    ///
    /// Returns:
    ///     Nothing; durable selection state is not changed here.
    fn store_main_chart_target(&mut self, group: &str, target: Option<(CoreId, String)>) {
        match target {
            Some(target) => {
                if self.main_chart_targets.get(group) != Some(&target) {
                    self.main_chart_targets.insert(group.to_string(), target);
                }
            }
            None => {
                self.main_chart_targets.remove(group);
            }
        }
    }

    /// Return the group's current Main trading target while it still belongs to that live group.
    ///
    /// Args:
    ///     group: Window group whose target is requested.
    ///
    /// Returns:
    ///     The stored core and market, or `None` when absent or stale after a group move.
    pub(crate) fn main_chart_target(&self, group: &str) -> Option<(CoreId, String)> {
        self.main_chart_targets
            .get(group)
            .filter(|(core, _)| self.core_belongs_to_group(group, *core))
            .cloned()
    }

    /// Publish the markets open in a group's Main stack from `MainChartStack`.
    pub(crate) fn set_main_open_markets(&mut self, group: &str, markets: Vec<(CoreId, String)>) {
        if markets.is_empty() {
            self.main_open_markets.remove(group);
        } else {
            self.main_open_markets.insert(group.to_string(), markets);
        }
    }

    /// Return the markets open in a group's Main stack for highlighting and sorting in Orders.
    pub(crate) fn main_open_markets(&self, group: &str) -> &[(CoreId, String)] {
        self.main_open_markets
            .get(group)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Record group-wide activity and reset Main's inactivity-close timer.
    ///
    /// Any active group-owned window can refresh this timestamp, including the primary window,
    /// detached chart windows, and detached panel windows.
    pub(crate) fn note_main_input(&mut self, group: &str) {
        self.last_main_input
            .insert(group.to_string(), std::time::Instant::now());
    }

    /// Return the last group-wide activity time used by Main's inactivity timeout.
    pub(crate) fn main_input_at(&self, group: &str) -> Option<std::time::Instant> {
        self.last_main_input.get(group).copied()
    }

    /// Return the core-local toggle for excluding blacklisted markets from delta calculations.
    pub(crate) fn exclude_bl_delta(&self, core: CoreId) -> bool {
        self.exclude_bl_delta.get(&core).copied().unwrap_or(false)
    }

    /// Remember whether a core excludes blacklisted markets from deltas; the caller sends the command.
    pub(crate) fn set_exclude_bl_delta(&mut self, core: CoreId, on: bool) {
        self.exclude_bl_delta.insert(core, on);
    }

    /// Return Main's configured inactivity-close timeout in seconds, where zero disables it.
    pub(crate) fn main_idle_close_secs(&self) -> u32 {
        self.preview
            .as_ref()
            .unwrap_or(&self.config)
            .main_idle_close_secs
    }

    /// Return the group's active trading core for the header and toolbar.
    ///
    /// A still-valid remembered header selection takes precedence, followed by the visible chart
    /// target and then the group's first core. This keeps the toolbar populated before a chart opens.
    ///
    /// Args:
    ///     group: Window group whose trading controls need a core.
    ///
    /// Returns:
    ///     A live core belonging to the group, or `None` when the group has no live cores.
    pub(crate) fn active_trade_core(&self, group: &str) -> Option<CoreId> {
        if let Some(&core) = self.layout.active_trade_core_by_group.get(group) {
            if self.core_belongs_to_group(group, core) {
                return Some(core);
            }
        }
        self.main_chart_target(group)
            .map(|(core, _)| core)
            // Match the visible first core so the trade fallback and header selector agree.
            .or_else(|| self.group_cores(group).first().map(|(id, _)| *id))
    }

    /// Set the group's active trading core in the shared durable layout.
    ///
    /// Args:
    ///     group: Window group that owns the selection.
    ///     core: Stable UID of the selected live core. Values outside `group` are ignored.
    ///
    /// Returns:
    ///     Nothing; a changed selection marks layout persistence dirty.
    pub(crate) fn set_active_trade_core(&mut self, group: &str, core: CoreId) {
        if !self.core_belongs_to_group(group, core) {
            return;
        }
        if self.layout.active_trade_core_by_group.get(group) != Some(&core) {
            self.layout
                .active_trade_core_by_group
                .insert(group.to_string(), core);
            self.layout_dirty = true;
        }
    }

    /// Refresh the cached fallback ticker from the first canonical live core.
    ///
    /// `force` recomputes immediately when sorting changes the first core.
    pub(crate) fn refresh_header_ticker_default(&mut self, force: bool) {
        if !force {
            if let Some((core, _)) = &self.header_ticker_default {
                if self.session.sessions().iter().any(|s| s.id == *core) {
                    return;
                }
            }
        }
        let now = Instant::now();
        if !force
            && self
                .last_header_ticker_refresh
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            return;
        }
        self.last_header_ticker_refresh = Some(now);
        // Match the first core shown by canonical selectors.
        let all = CoreOrder::new(&self.config).from_sessions(self.session.sessions(), |_| true);
        let Some(core) = all.first().map(|(id, _)| *id) else {
            self.header_ticker_default = None;
            return;
        };
        let ms = self.session.market_source();
        let market = ["BTCUSDT", "UBTCUSDC"]
            .iter()
            .find(|cand| ms.search_markets(core, cand, 2).iter().any(|m| m == *cand))
            .map(|c| c.to_string())
            .or_else(|| ms.search_markets(core, "BTC", 1).into_iter().next());
        self.header_ticker_default = market.map(|market| (core, market));
    }

    /// Return the header price ticker source.
    ///
    /// A layout selection keyed by stable core UID is used while a session for that core is present;
    /// otherwise the precomputed default cache is returned. Rendering neither searches markets nor
    /// mutates the backend.
    pub(crate) fn header_ticker(&self) -> Option<(CoreId, String)> {
        if let Some(sel) = &self.layout.header_ticker {
            let core = self
                .config
                .servers
                .iter()
                .find(|s| s.uid == sel.core_uid)
                .map(|s| s.id);
            if let Some(core) = core {
                if self.session.sessions().iter().any(|s| s.id == core) {
                    return Some((core, sel.market.clone()));
                }
            }
        }
        self.header_ticker_default
            .as_ref()
            .filter(|(core, _)| self.session.sessions().iter().any(|s| s.id == *core))
            .cloned()
    }

    /// Store the header ticker selected in the search popup by core UID and mark layout persistence dirty.
    pub(crate) fn set_header_ticker(&mut self, core: CoreId, market: String) {
        let Some(uid) = self
            .config
            .servers
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.uid)
        else {
            return;
        };
        let sel = moon_core::config::layout::HeaderTicker {
            core_uid: uid,
            market: market.clone(),
        };
        if self.layout.header_ticker.as_ref() != Some(&sel) {
            self.layout.header_ticker = Some(sel);
            self.layout_dirty = true;
        }
    }

    /// Return the header clock offset in minutes from UTC; the default zero means UTC.
    pub(crate) fn header_clock_offset_min(&self) -> i32 {
        self.layout.header_clock_offset_min
    }

    /// Store the header clock offset selected in the time-zone popup and mark layout persistence dirty.
    pub(crate) fn set_header_clock_offset_min(&mut self, off_min: i32) {
        if self.layout.header_clock_offset_min != off_min {
            self.layout.header_clock_offset_min = off_min;
            self.layout_dirty = true;
        }
    }

    /// The core-warning axis toggles (CPU / memory / connectivity / ping).
    pub(crate) fn warn_axes(&self) -> moon_core::config::layout::WarnAxesCfg {
        self.layout.warn_axes
    }

    /// Store the core-warning axis toggles from the Core Status gear popup and mark layout dirty.
    ///
    /// The engine reads these at the next tick, so a disabled axis stops opening episodes at once;
    /// the read paths also filter its persisted history out, so it also disappears from the charts.
    pub(crate) fn set_warn_axes(&mut self, axes: moon_core::config::layout::WarnAxesCfg) {
        if self.layout.warn_axes != axes {
            self.layout.warn_axes = axes;
            self.layout_dirty = true;
            // A toggle shifts which episodes charts should draw without opening/closing one, so push
            // the revision forward to invalidate the cached marks on every chart.
            self.warn.bump_rev();
        }
    }

    /// Return the group's cores in canonical order for the header selector.
    pub(crate) fn group_cores(&self, group: &str) -> OrderedCores {
        CoreOrder::new(&self.config).from_sessions(self.session.sessions(), |s| s.group == group)
    }

    pub(crate) fn retain_chart_market(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        *self.chart_market_refs.entry(key).or_insert(0) += 1;
        self.rebuild_desired_markets();
    }

    pub(crate) fn release_chart_market(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        let mut remove = false;
        if let Some(count) = self.chart_market_refs.get_mut(&key) {
            debug_assert!(*count > 0, "chart market refcount over-release");
            *count = count.saturating_sub(1);
            remove = *count == 0;
        } else {
            debug_assert!(false, "chart market refcount release without owner");
        }
        if remove {
            self.chart_market_refs.remove(&key);
        }
        self.rebuild_desired_markets();
    }

    pub(crate) fn retain_chart_orderbook(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        *self.chart_orderbook_refs.entry(key).or_insert(0) += 1;
        self.rebuild_orderbook_wanted();
    }

    pub(crate) fn release_chart_orderbook(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        let mut remove = false;
        if let Some(count) = self.chart_orderbook_refs.get_mut(&key) {
            *count = count.saturating_sub(1);
            remove = *count == 0;
        }
        if remove {
            self.chart_orderbook_refs.remove(&key);
        }
        self.rebuild_orderbook_wanted();
    }

    /// Rebuild `desired_orderbook` from markets with at least one enabled order-book consumer.
    ///
    /// A changed list marks the open-market request set dirty for resending.
    pub(crate) fn rebuild_orderbook_wanted(&mut self) {
        let mut want: Vec<(CoreId, String)> = self
            .chart_orderbook_refs
            .iter()
            .filter_map(|((core, market), count)| (*count > 0).then(|| (*core, market.clone())))
            .collect();
        want.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        if self.desired_orderbook != want {
            self.desired_orderbook = want;
            self.desired_open_dirty = true;
        }
    }

    pub(crate) fn rebuild_desired_markets(&mut self) {
        let mut desired: Vec<(CoreId, String)> = self
            .chart_market_refs
            .iter()
            .filter_map(|((core, market), count)| (*count > 0).then(|| (*core, market.clone())))
            .collect();
        desired.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        if self.desired != desired {
            self.desired = desired;
            self.desired_open_dirty = true;
        }
    }

    pub(crate) fn sync_open_markets_if_due(&mut self) {
        let now = Instant::now();
        // The 1s fallback is intentional: provider-side linger/drop/failover is
        // wall-clock based. The hot path itself is the boolean dirty flag; we no
        // longer hash the whole desired market list every 100ms.
        let due = now.duration_since(self.last_open_sync) >= Duration::from_secs(1);
        if self.desired_open_dirty || due {
            self.desired_open_dirty = false;
            self.last_open_sync = now;
            self.session
                .set_open(&self.desired, &self.desired_orderbook);
        }
    }

    /// Advance the backend warning engine one tick from the current core telemetry.
    ///
    /// Runs from the coordination loop (backend-always, ~10 Hz); the engine throttles itself to
    /// 1 Hz. Samples every live core's endpoint, status, and telemetry once.
    ///
    /// Args:
    ///     now_ms: Current Unix milliseconds.
    ///
    /// Returns:
    ///     Nothing; the engine's tracking, warning state, and episode log advance in place.
    pub(crate) fn tick_core_warnings(&mut self, now_ms: i64) {
        let samples: Vec<crate::backend::core_warn::CoreSample> = {
            let store = self.session.store();
            self.session
                .sessions()
                .iter()
                .filter_map(|session| {
                    let core = store.core(session.id)?;
                    Some(crate::backend::core_warn::CoreSample {
                        id: session.id,
                        ip: core.endpoint.map(|endpoint| endpoint.address),
                        status: core.status.clone(),
                        sys: core.sys,
                    })
                })
                .collect()
        };
        let enabled = self.warn_enabled();
        self.warn.set_enabled(enabled);
        let tuning = self.warn_tuning();
        self.warn.set_tuning(tuning);
        let result = self.warn.tick(&samples, now_ms);
        // Play each newly-opened axis's alert sound once (independent of chart visibility). The axis
        // is necessarily enabled — a disabled axis opens nothing.
        for axis in &result.opened {
            if let Some(name) = self.warn_sound(*axis) {
                crate::media::sound::play(&name);
            }
        }
        // Record this second's raw chart history into the shared rings (backend-always, so the
        // Core Status chart and the upcoming badge slices have data regardless of any open panel).
        let sec = now_ms / 1000;
        for ring in &result.rings {
            match ring.subject {
                crate::backend::core_warn::RingSubject::Server(ip) => {
                    self.core_chart_hist.record(ip, sec, ring.cpu, ring.mem)
                }
                crate::backend::core_warn::RingSubject::Core(id) => {
                    self.core_line_hist.record(id, sec, ring.cpu, ring.mem)
                }
            }
        }
        // Per-server ping history (client↔core and core→exchange), recorded backend-always like the
        // CPU/memory rings.
        for (ip, link, exch) in &result.pings {
            self.server_ping_hist.record(*ip, sec, *link);
            self.server_exch_hist.record(*ip, sec, *exch);
        }
        if let Some(store) = &self.warn_store {
            for episode in &result.closed {
                // An axis turned off mid-episode closes its open warning on this tick; honor
                // "off = not persisted" by dropping it rather than writing it to the log.
                if !enabled.allows(episode.axis) {
                    continue;
                }
                match store.insert_episode(episode) {
                    Ok(rowid) => {
                        if let Some(ip) = episode.server_ip {
                            // Capture immediately with whatever window exists now, so a shutdown
                            // before the forward tail fills still leaves a (partial) graph on disk.
                            capture_series(
                                store,
                                self.core_chart_hist.ring(ip),
                                self.server_ping_hist.ring(ip),
                                self.server_exch_hist.ring(ip),
                                rowid,
                                episode.start_ms,
                                now_ms,
                            );
                            // If the forward tail has not accrued yet, re-capture the full window
                            // later; OR REPLACE overwrites the partial with the complete slice.
                            let capture_at_ms = episode.start_ms + WARN_SLICE_FWD_MS;
                            if capture_at_ms > now_ms {
                                push_pending_slice(
                                    &mut self.warn_pending_slices,
                                    PendingWarnSlice {
                                        episode_id: rowid,
                                        ip,
                                        start_ms: episode.start_ms,
                                        capture_at_ms,
                                    },
                                );
                            }
                        }
                    }
                    Err(err) => log::warn!("core warning persist failed: {err}"),
                }
            }
        }
        self.drain_pending_slices(now_ms);
    }

    /// Capture and persist the ±1 min history slice of any pending episode whose forward window has
    /// now accumulated in the live ring. A pending capture with no ring data is dropped (its card
    /// simply shows no graph), so the queue never stalls.
    ///
    /// Args:
    ///     now_ms: Current Unix milliseconds.
    ///
    /// Returns:
    ///     Nothing; ready slices are written to `warn_store` and removed from the queue.
    fn drain_pending_slices(&mut self, now_ms: i64) {
        let Some(store) = &self.warn_store else {
            self.warn_pending_slices.clear();
            return;
        };
        let mut i = 0;
        while i < self.warn_pending_slices.len() {
            if self.warn_pending_slices[i].capture_at_ms > now_ms {
                i += 1;
                continue;
            }
            // `remove` (not `swap_remove`) keeps the queue in insertion order, so the cap's
            // oldest-first eviction stays meaningful; the queue is tiny, so the shift is cheap.
            let pending = self.warn_pending_slices.remove(i);
            capture_series(
                store,
                self.core_chart_hist.ring(pending.ip),
                self.server_ping_hist.ring(pending.ip),
                self.server_exch_hist.ring(pending.ip),
                pending.episode_id,
                pending.start_ms,
                now_ms,
            );
        }
    }

    /// The server history slice around a moment, from the live ring: the card's live-path graph.
    pub(crate) fn warn_server_slice(
        &self,
        ip: IpAddr,
        at_ms: i64,
        now_ms: i64,
    ) -> Option<Vec<(u8, u8)>> {
        ring_slice(self.core_chart_hist.ring(ip), at_ms, now_ms)
    }

    /// The server client↔core ping slice around a moment, from the live ring: the card's ping line.
    pub(crate) fn warn_server_ping_slice(
        &self,
        ip: IpAddr,
        at_ms: i64,
        now_ms: i64,
    ) -> Option<Vec<u16>> {
        ring_slice(self.server_ping_hist.ring(ip), at_ms, now_ms)
    }

    /// The server core→exchange ping slice around a moment, from the live ring: the card's exch line.
    pub(crate) fn warn_server_exch_slice(
        &self,
        ip: IpAddr,
        at_ms: i64,
        now_ms: i64,
    ) -> Option<Vec<u16>> {
        ring_slice(self.server_exch_hist.ring(ip), at_ms, now_ms)
    }

    /// The persisted server history slice for a closed episode, for a card whose warning has already
    /// rolled out of the live ring. `None` if it was never captured or persistence is off.
    pub(crate) fn warn_series_slice(&self, episode_id: u64) -> Option<Vec<(u8, u8)>> {
        self.warn_store
            .as_ref()?
            .series_for_episode(episode_id as i64, "server")
            .ok()
            .flatten()
    }

    /// The persisted client↔core ping slice for a closed episode, past the live ring. `None` if it
    /// was never captured or persistence is off.
    pub(crate) fn warn_ping_series_slice(&self, episode_id: u64) -> Option<Vec<u16>> {
        self.warn_store
            .as_ref()?
            .ping_series_for_episode(episode_id as i64, "ping")
            .ok()
            .flatten()
    }

    /// The persisted core→exchange ping slice for a closed episode, past the live ring.
    pub(crate) fn warn_exch_series_slice(&self, episode_id: u64) -> Option<Vec<u16>> {
        self.warn_store
            .as_ref()?
            .ping_series_for_episode(episode_id as i64, "exch")
            .ok()
            .flatten()
    }

    /// The engine's axis master switches, projected from the persisted layout toggles.
    fn warn_enabled(&self) -> crate::backend::core_warn::WarnEnabled {
        let axes = self.layout.warn_axes;
        crate::backend::core_warn::WarnEnabled {
            cpu: axes.cpu,
            mem: axes.mem,
            conn: axes.conn,
            ping: axes.ping,
            exch: axes.exch,
        }
    }

    /// The engine's numeric detection thresholds, projected from the persisted per-axis params.
    /// Latency percents (`yellow`/`red` as +N %) become the ratio ×100 the engine consumes.
    fn warn_tuning(&self) -> crate::backend::core_warn::WarnTuning {
        let p = &self.layout.warn_params;
        // Ratio ×100; clamp yellow to red so a mis-set yellow > red can't make the yellow band
        // unreachable (severity checks red first) — it just collapses to no yellow.
        let ping_red = 100 + u32::from(p.ping.red);
        let exch_red = 100 + u32::from(p.exch.red);
        crate::backend::core_warn::WarnTuning {
            cpu_pct: u32::from(p.cpu.pct),
            // Hold clamped to ≥1: a hand-edited 0 would otherwise make `next >= hold` true even on a
            // non-critical second (counter resets to 0), warning permanently.
            cpu_hold: u32::from(p.cpu.hold).max(1),
            mem_pct: u32::from(p.mem.pct),
            mem_window: i64::from(p.mem.window),
            ping_yellow_num: (100 + u32::from(p.ping.yellow)).min(ping_red),
            ping_red_num: ping_red,
            ping_window: i64::from(p.ping.window),
            ping_hold: u32::from(p.ping.hold).max(1),
            exch_yellow_num: (100 + u32::from(p.exch.yellow)).min(exch_red),
            exch_red_num: exch_red,
            exch_window: i64::from(p.exch.window),
            exch_hold: u32::from(p.exch.hold).max(1),
        }
    }

    /// The alert sound stem configured for one axis, or `None` when silent.
    fn warn_sound(&self, axis: crate::backend::core_warn::WarnAxis) -> Option<String> {
        use crate::backend::core_warn::WarnAxis;
        let p = &self.layout.warn_params;
        let sound = match axis {
            WarnAxis::SysCpu => &p.cpu.sound,
            WarnAxis::MemGrowth => &p.mem.sound,
            WarnAxis::Unreachable => &p.conn.sound,
            WarnAxis::Ping => &p.ping.sound,
            WarnAxis::ExchPing => &p.exch.sound,
        };
        sound.clone().filter(|s| !s.trim().is_empty())
    }

    /// Whether one axis draws on charts (separate from whether it is detected/recorded).
    fn warn_chart(&self, axis: crate::backend::core_warn::WarnAxis) -> bool {
        use crate::backend::core_warn::WarnAxis;
        let p = &self.layout.warn_params;
        match axis {
            WarnAxis::SysCpu => p.cpu.chart,
            WarnAxis::MemGrowth => p.mem.chart,
            WarnAxis::Unreachable => p.conn.chart,
            WarnAxis::Ping => p.ping.chart,
            WarnAxis::ExchPing => p.exch.chart,
        }
    }

    /// The persisted per-axis warning params (chart visibility, sound, thresholds).
    pub(crate) fn warn_params(&self) -> moon_core::config::layout::WarnParams {
        self.layout.warn_params.clone()
    }

    /// Replace the per-axis warning params, marking the layout dirty and invalidating chart marks.
    pub(crate) fn set_warn_params(&mut self, params: moon_core::config::layout::WarnParams) {
        if self.layout.warn_params != params {
            self.layout.warn_params = params;
            self.layout_dirty = true;
            self.warn.bump_rev();
        }
    }

    /// Warning episodes for one server within `[from_ms, to_ms]`: persisted (closed) plus still-open.
    ///
    /// The chart draws a badge per episode. Merges the SQLite log with the engine's live open
    /// episodes, so an in-progress warning already shows a badge.
    ///
    /// Args:
    ///     ip: Server endpoint address.
    ///     from_ms: Inclusive lower bound on `start_ms`.
    ///     to_ms: Inclusive upper bound on `start_ms`.
    ///
    /// Returns:
    ///     Matching episodes; empty if persistence is off and nothing is open.
    pub(crate) fn warn_episodes_for_server(
        &self,
        ip: IpAddr,
        from_ms: i64,
        to_ms: i64,
    ) -> Vec<crate::backend::core_warn::WarnEpisode> {
        let mut out = self
            .warn_store
            .as_ref()
            .and_then(|store| store.episodes_for_server(ip, from_ms, to_ms).ok())
            .unwrap_or_default();
        for open in self.warn.open_episodes() {
            if open.server_ip == Some(ip) && open.start_ms >= from_ms && open.start_ms <= to_ms {
                out.push(open);
            }
        }
        // A disabled axis hides its already-recorded history from the chart too; an axis with
        // "show on chart" off is still recorded and listed, just not drawn here.
        let enabled = self.warn_enabled();
        out.retain(|episode| enabled.allows(episode.axis) && self.warn_chart(episode.axis));
        out
    }

    /// The most recent warning episodes across all servers, newest first, for the Warnings list.
    ///
    /// Merges still-open episodes (not yet persisted) with the persisted log, so an in-progress
    /// warning appears at the top.
    ///
    /// Args:
    ///     limit: Maximum rows to return.
    ///
    /// Returns:
    ///     Episodes ordered newest first, capped to `limit`.
    pub(crate) fn warn_episodes_recent(
        &self,
        limit: usize,
    ) -> Vec<crate::backend::core_warn::WarnEpisode> {
        let mut all = self.warn.open_episodes();
        if let Some(store) = &self.warn_store {
            if let Ok(closed) = store.recent_episodes(limit) {
                all.extend(closed);
            }
        }
        // A disabled axis is also dropped from the Warnings list, matching the charts.
        let enabled = self.warn_enabled();
        all.retain(|episode| enabled.allows(episode.axis));
        all.sort_by(|a, b| b.start_ms.cmp(&a.start_ms));
        all.truncate(limit);
        all
    }

    pub(crate) fn mark_backend_dirty(&mut self, cx: &mut Context<Self>) {
        self.backend_dirty_since_notify = true;
        self.flush_backend_notify(cx);
    }

    pub(crate) fn flush_backend_notify(&mut self, cx: &mut Context<Self>) {
        if !self.backend_dirty_since_notify {
            return;
        }
        let due = self
            .last_backend_notify
            .is_none_or(|last| last.elapsed() >= Duration::from_millis(250));
        if !due {
            return;
        }
        self.backend_dirty_since_notify = false;
        self.last_backend_notify = Some(Instant::now());
        crate::diag::bump(&crate::diag::BACKEND_NOTIFY);
        cx.notify();
    }

    pub(crate) fn maybe_diag_open_first_market(&mut self, cx: &mut Context<Self>) {
        if !self.diag_open_first_market || self.diag_open_done || self.open_request.is_some() {
            return;
        }
        if self.group_windows.is_empty() {
            return;
        }

        let candidate = self.config.servers.iter().find_map(|server| {
            let market = server.market.trim();
            let session_exists = self
                .session
                .sessions()
                .iter()
                .any(|session| session.id == server.id && session.group == server.group);
            (server.active
                && server.show_window
                && self.config.group(&server.group).active
                && self.group_windows.contains_key(&server.group)
                && !market.is_empty()
                && session_exists)
                .then(|| (server.id, market.to_string(), server.name.clone()))
        });

        let Some((core, market, name)) = candidate else {
            self.diag_open_done = true;
            log::warn!("diag auto-open: no active visible server with default market");
            return;
        };

        self.diag_open_done = true;
        self.open_on_main((core, market.clone()), false);
        if std::env::var_os("MOON_RENDER_DIAG_PAUSE_AFTER_OPEN").is_some() {
            self.follow = false;
        }
        log::info!("diag auto-open: core={core} name={name} market={market}");
        cx.notify();
    }

    /// Request opening `target` on the group's Main chart, bumping `open_request_rev` so the
    /// group's `ChartTabs` wakes for exactly this request instead of a fallback render.
    ///
    /// `activate` raises and focuses the Main window: the chart double-click and the Alerts coin
    /// click pass `true`, while table, detect, log, and screener navigation pass `false` to open
    /// the market without stealing focus from the current window.
    pub(crate) fn open_on_main(&mut self, target: (CoreId, String), activate: bool) {
        self.open_request = Some(target);
        self.open_request_rev = self.open_request_rev.wrapping_add(1);
        self.open_request_activate = activate;
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn take_diag_open_10_btc(&mut self) -> bool {
        if !self.diag_open_10_btc || self.diag_open_10_btc_done {
            return false;
        }
        // Debug perf windows only need a live core id/group, not the main group window.
        // On headless Linux/X11 the main window can exist while the bookkeeping gate is
        // still false during early startup, which made MOON_RENDER_DIAG_OPEN_10_BTC
        // silently do nothing and broke automated perf runs.
        if self.session.sessions().is_empty() {
            return false;
        }
        if crate::diagnostics::debug_window::debug_chart_target(self).is_none() {
            return false;
        }
        self.diag_open_10_btc_done = true;
        true
    }
}
