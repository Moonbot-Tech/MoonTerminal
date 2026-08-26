//! Methods for the application's shared backend state ([`Backend`]). The struct is declared in
//! `main.rs`, the crate root, so its private fields are visible to descendant modules. Methods in
//! this sibling module use `pub(crate)` because private items here would not be crate-wide.

pub(crate) mod core_warn;
mod detect_sound;
mod figures;
mod manual_trading;
mod open_request;
mod quiet;
pub(crate) mod server_chart;
#[cfg(test)]
mod tests;

pub(crate) use manual_trading::{MANUAL_STRATEGY_KIND, ManualOrderTerms};
pub(crate) use open_request::{ChartHistoryScope, OpenCompareRequest, OpenMainRequest};

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use gpui::{Context, WindowId};

use crate::Backend;
use crate::backend::core_warn::axis_has_series;
use crate::chartdx::ChartDataHandle;
use crate::core_order::{CoreOrder, OrderedCores};
use moon_core::config::{CoreGroup, WorkspaceMode};
use moon_core::db::valuation::ValuationMode;
use moon_core::feed::{StrategyEditOutcome, StrategyEditResult};
use moon_core::market::MarketLimits;
use moon_core::session::CoreId;
use moon_ui::{DockAreaState, DockTopologyByName};

/// Milliseconds of history kept on each side of a warning start for its persisted graphs (±30 s, a
/// 60 s window — enough context for analysis without bloating the per-core slices).
const WARN_SLICE_BACK_MS: i64 = 30_000;
const WARN_SLICE_FWD_MS: i64 = 30_000;
/// Persisted warning-episode SLICES (not the episode rows) are pruned once they are older than this,
/// so the per-core graph blobs — which scale with the core count — cannot grow the file forever.
const WARN_SLICE_RETENTION_MS: i64 = 30 * 24 * 3600 * 1000;
/// How often the retention prune re-runs (once a day), so a session that outlives the 30-day
/// retention window keeps trimming instead of pruning only at startup.
const WARN_PRUNE_INTERVAL_MS: i64 = 24 * 3600 * 1000;
/// Cap on episodes queued for slice capture, so a burst of warnings cannot grow the queue without
/// bound; the oldest pending capture is dropped past it.
const WARN_PENDING_CAP: usize = 1024;

/// Filter, prioritize, and cap warning episodes for one effective workspace list.
///
/// Args:
///     all: Open and persisted warning episodes to reconcile.
///     enabled: Current warning-axis switches.
///     core_ids: Effective core identities accepted by core-specific warning axes.
///     server_ips: Effective server identities accepted by server-wide warning axes.
///     limit: Maximum rows to publish after filtering and ordering.
///
/// Returns:
///     Enabled in-scope episodes with open warnings first, then newest, capped only after scope
///     membership is applied.
fn finalize_recent_warning_episodes(
    mut all: Vec<crate::backend::core_warn::WarnEpisode>,
    enabled: crate::backend::core_warn::WarnEnabled,
    core_ids: &HashSet<CoreId>,
    server_ips: &HashSet<IpAddr>,
    limit: usize,
) -> Vec<crate::backend::core_warn::WarnEpisode> {
    all.retain(|episode| {
        enabled.allows(episode.axis)
            && (episode.core_id.is_some_and(|core| core_ids.contains(&core))
                || (episode.core_id.is_none()
                    && episode.server_ip.is_some_and(|ip| server_ips.contains(&ip))))
    });
    // Still-open episodes lead, then newest first. Without the pin, a warning that has been
    // ongoing for weeks -- an expiring API key is exactly that -- has the oldest start time and is
    // the first row the limit drops, hiding the one warning that is still true.
    all.sort_by(|a, b| {
        b.end_ms
            .is_none()
            .cmp(&a.end_ms.is_none())
            .then(b.start_ms.cmp(&a.start_ms))
    });
    all.truncate(limit);
    all
}

/// A closed, persisted episode whose ±1 min history slice is captured later — once its forward tail
/// (`start_ms + WARN_SLICE_FWD_MS`) has accumulated in the live ring — and written to `warn_store`.
pub(crate) struct PendingWarnSlice {
    /// The `core_warnings` row id the slice is keyed to.
    pub(crate) episode_id: i64,
    /// Server whose history ring the slice is read from, or `None` for an unknown-endpoint episode.
    pub(crate) ip: Option<IpAddr>,
    /// The cores to capture per-core slices for, frozen at close time (the roster on the IP, or the
    /// episode's own core when there is no IP).
    pub(crate) roster: Vec<CoreId>,
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

/// Capture and persist one episode's FULL ±30 s topology from the current rings, overwriting any
/// earlier partial capture (the store uses `INSERT OR REPLACE`). A ring with no data in the window
/// writes nothing. So any warning — on a core or a server — leaves a self-contained record for
/// analysis: the server graph (`badge 0`) plus EVERY core on the server (`badge = core id`), each
/// with its own CPU/memory and both pings. An unknown-endpoint episode has no server ring, so only
/// its core's slice is written.
///
/// Args:
///     store: The warnings database.
///     server: Server `(cpu %, mem %)` ring, or `None` for an unknown endpoint.
///     ping: Server worst client↔core ping ring.
///     exch: Server worst core→exchange ping ring.
///     cores: Each core to record with its per-core metrics ring.
///     episode_id: The `core_warnings` row id these slices belong to.
///     start_ms: Episode start; the window is `[start - BACK, start + FWD]`.
///     now_ms: Current time, bounding the forward edge to what has actually accrued.
#[allow(clippy::too_many_arguments)]
fn capture_topology(
    store: &crate::backend::core_warn::store::WarnStore,
    server: Option<&std::collections::VecDeque<(u8, u8)>>,
    ping: Option<&std::collections::VecDeque<u16>>,
    exch: Option<&std::collections::VecDeque<u16>>,
    cores: &[(
        CoreId,
        Option<&std::collections::VecDeque<crate::backend::server_chart::CoreMetrics>>,
    )],
    episode_id: i64,
    start_ms: i64,
    now_ms: i64,
) {
    // NOTE for the future timestamped reader: `base_ms` assumes the slice reaches the full back edge.
    // A subject whose ring is shorter than the back window yields a slice whose first sample is NEWER
    // than `base_ms`, so times must be derived from the slice (aligned to `start_ms` and its length),
    // not from `base_ms`. No current reader uses `base_ms`, so this is latent.
    let base_ms = start_ms - WARN_SLICE_BACK_MS;
    let warn = |what: &str, r: rusqlite::Result<()>| {
        if let Err(err) = r {
            log::warn!("core warning {what} slice persist failed: {err}");
        }
    };
    // Server graph (badge 0) + its two worst-ping lines.
    if let Some(s) = ring_slice(server, start_ms, now_ms) {
        warn(
            "server",
            store.insert_series(episode_id, 0, "server", base_ms, &s),
        );
    }
    if let Some(p) = ring_slice(ping, start_ms, now_ms) {
        warn(
            "ping",
            store.insert_ping_series(episode_id, 0, "ping", base_ms, &p),
        );
    }
    if let Some(e) = ring_slice(exch, start_ms, now_ms) {
        warn(
            "exch",
            store.insert_ping_series(episode_id, 0, "exch", base_ms, &e),
        );
    }
    // Every core (badge = core id): its own CPU/memory pair and its two pings, split out of the
    // combined per-core sample so each rides its existing blob format. Distinct subjects from the
    // server ("core*"/"server"+"ping"/"exch") so a core id of 0 cannot collide with the server
    // sentinel badge (0) on the shared unique key.
    for (id, ring) in cores {
        let Some(slice) = ring_slice(*ring, start_ms, now_ms) else {
            continue;
        };
        let badge = *id as i64;
        let cm: Vec<(u8, u8)> = slice.iter().map(|m| (m.cpu, m.mem)).collect();
        let pings: Vec<u16> = slice.iter().map(|m| m.ping).collect();
        let exchs: Vec<u16> = slice.iter().map(|m| m.exch).collect();
        warn(
            "core",
            store.insert_series(episode_id, badge, "core", base_ms, &cm),
        );
        warn(
            "core ping",
            store.insert_ping_series(episode_id, badge, "core_ping", base_ms, &pings),
        );
        warn(
            "core exch",
            store.insert_ping_series(episode_id, badge, "core_exch", base_ms, &exchs),
        );
    }
}

/// Push a pending capture, evicting the oldest if the queue is at its cap.
fn push_pending_slice(queue: &mut Vec<PendingWarnSlice>, item: PendingWarnSlice) {
    if queue.len() >= WARN_PENDING_CAP {
        queue.remove(0);
    }
    queue.push(item);
}

impl Backend {
    /// Apply one edit to the saved core groups, sanitize the result, and persist it.
    ///
    /// The dialogs use this centralized write path so their edits cannot store a shape the loader
    /// would have to repair or forget `config_dirty`. The edit works on a copy: the dirty flag is
    /// raised only when the sanitized result actually differs, so a rename to the same text or a
    /// refused reorder costs no disk write.
    ///
    /// The result is mirrored into an open Settings PREVIEW, exactly as `update_group_trade` does.
    /// Settings saves the preview it cloned when it opened and then replaces the live config with
    /// it, so an edit written only here would be rolled back by a Settings save that touched
    /// nothing related.
    ///
    /// Args:
    ///     edit: The mutation, reporting whether it changed anything worth sanitizing.
    ///
    /// Returns:
    ///     Whether the saved list actually changed. `false` covers a refused edit AND one the
    ///     sanitizer undid — at the group ceiling an append survives the closure and not the
    ///     sanitize, and a caller reporting success there would close its dialog over nothing.
    pub(crate) fn edit_core_groups(
        &mut self,
        edit: impl FnOnce(&mut Vec<CoreGroup>) -> bool,
    ) -> bool {
        let mut groups = self.config.core_groups.clone();
        if !edit(&mut groups) {
            return false;
        }
        moon_core::config::sanitize_core_groups(&mut groups);
        if groups == self.config.core_groups {
            return false;
        }
        if let Some(preview) = self.preview.as_mut() {
            preview.core_groups = groups.clone();
        }
        self.config.core_groups = groups;
        self.config_dirty = true;
        true
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

    /// Resolve the pending Main request's current owner from live session state.
    ///
    /// Returns:
    ///     Current group of the pending target core, or `None` after removal/consumption.
    fn current_open_main_group(&self) -> Option<&str> {
        let core = self.open_main_request.pending_core()?;
        self.session
            .sessions()
            .iter()
            .find(|session| session.id == core)
            .map(|session| session.group.as_str())
    }

    /// Reconcile pending Main routing after session topology changes.
    ///
    /// Returns:
    ///     `true` when a moved core retargeted the request or a removed core cancelled it.
    pub(crate) fn reconcile_open_main_request_group(&mut self) -> bool {
        let current_group = self.current_open_main_group().map(str::to_string);
        let authority_valid = match (
            self.open_main_request.authority_group(),
            self.open_main_request.pending_core(),
        ) {
            (Some(authority_group), Some(core)) => {
                current_group.as_deref() == Some(authority_group)
                    && self.workspace_action_allows_core(Some(authority_group), core)
            }
            (None, Some(_)) => true,
            (_, None) => false,
        };
        let current_group = authority_valid.then_some(current_group).flatten();
        self.open_main_request.reconcile_group(current_group)
    }

    /// Return a pending Main target only to its current live group.
    ///
    /// Args:
    ///     group: ChartTabs group requesting a read-phase target copy.
    ///
    /// Returns:
    ///     Borrowed target only when the target core currently belongs to `group`.
    pub(crate) fn pending_open_main_request_for_group(
        &self,
        group: &str,
    ) -> Option<&(CoreId, String)> {
        (self.current_open_main_group().as_deref() == Some(group))
            .then_some(())
            .and(self.open_main_request.pending_target())
    }

    /// Return the pending Main revision only to its current live group.
    ///
    /// Args:
    ///     group: ChartTabs group assembling its observer signature.
    ///
    /// Returns:
    ///     Request revision when the pending core currently belongs to `group`, otherwise zero.
    pub(crate) fn pending_open_main_revision_for_group(&self, group: &str) -> u64 {
        if self.current_open_main_group().as_deref() == Some(group) {
            self.open_main_request.revision()
        } else {
            0
        }
    }

    /// Revalidate and drain a matching Main request from its current live group.
    ///
    /// Args:
    ///     group: ChartTabs group attempting consumption.
    ///     expected: Target copied during the preceding read phase.
    ///     cx: Backend context used to wake the request's newly resolved owner.
    ///
    /// Returns:
    ///     Owned target, history scope, and activation bit only when current session ownership
    ///     still matches.
    pub(crate) fn take_open_main_request_if_matches(
        &mut self,
        group: &str,
        expected: &(CoreId, String),
        cx: &mut Context<Self>,
    ) -> Option<(CoreId, String, ChartHistoryScope, bool)> {
        if self.reconcile_open_main_request_group() {
            cx.notify();
        }
        self.open_main_request.take_if_matches(group, expected)
    }

    /// Return whether this panel is currently recorded as detached from `group`.
    ///
    /// The question every detach route asks before opening a window: a panel already pulled out
    /// must not be detached twice, or the second window takes over `detached_panel_windows` and
    /// leaves the first unable to repin.
    ///
    /// Args:
    ///     group: Window group the panel would be detached from.
    ///     panel: Stable panel name shared by `DetachedSpec` and the dock.
    ///
    /// Returns:
    ///     `true` while a `DetachedSpec` for this pair exists.
    pub(crate) fn is_detached(&self, group: &str, panel: &str) -> bool {
        self.detached
            .iter()
            .any(|spec| spec.group == group && spec.panel == panel)
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

    /// Publish the group's current Main target and remember a genuine Classic core change.
    ///
    /// Repeated synchronization of the same target preserves a manual header selection. In
    /// Classic, moving Main or a locked comparison anchor to another core makes that core the new
    /// durable selection. In Auto, the target remains runtime chart context and cannot overwrite
    /// `active_trade_core_by_group`.
    ///
    /// Args:
    ///     group: Window group whose target changed.
    ///     target: Active core and market, or `None` when no Main trading target exists. A target
    ///         whose live core no longer belongs to `group` is treated as `None`.
    ///
    /// Returns:
    ///     Nothing; runtime target state and, only for a Classic core change, layout state update.
    pub(crate) fn set_main_chart_target(&mut self, group: &str, target: Option<(CoreId, String)>) {
        let target = target.filter(|(core, _)| self.core_belongs_to_group(group, *core));
        if self.main_chart_targets.get(group) == target.as_ref() {
            return;
        }
        let prev_core = self.main_chart_targets.get(group).map(|(core, _)| *core);
        if let Some(new_core) = target.as_ref().and_then(|(new_core, _)| {
            Self::classic_trade_core_for_main_transition(
                self.workspace_mode(group),
                prev_core,
                *new_core,
            )
        }) {
            self.set_active_trade_core(group, new_core);
        }
        self.store_main_chart_target(group, target);
    }

    /// Resolve whether a Main target transition may update durable Classic trade state.
    ///
    /// This is the exact production guard used by [`Self::set_main_chart_target`]. It remains a
    /// narrow associated function so the Auto chart-open regression can mutate and prove the real
    /// decision without constructing the unrelated report, chart, and window fields of Backend.
    ///
    /// Args:
    ///     mode: Current group workspace mode.
    ///     previous_core: Core previously targeted by Main, if any.
    ///     new_core: Newly published Main core.
    ///
    /// Returns:
    ///     Core to remember only for a genuine Classic transition, otherwise `None`.
    fn classic_trade_core_for_main_transition(
        mode: WorkspaceMode,
        previous_core: Option<CoreId>,
        new_core: CoreId,
    ) -> Option<CoreId> {
        (previous_core != Some(new_core)
            && crate::workspace::should_remember_classic_trade_core(mode))
        .then_some(new_core)
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

    /// Return one market's exchange trading limits, for the leverage control.
    ///
    /// Takes the core and market EXPLICITLY rather than resolving a group's current chart, because
    /// its two consumers ask different questions: the toolbar readout wants the market on screen
    /// now, while an open leverage popup must ask about the address it was SEEDED from — the chart
    /// can move to another coin while that popup stands open, and answering with the new coin's cap
    /// is how a limit gets read against the wrong market. A group-shaped convenience here would
    /// make the wrong call the easy one.
    ///
    /// Args:
    ///     core: Core whose provider owns the market data.
    ///     market: Canonical market name.
    ///
    /// Returns:
    ///     The limits, or `None` while the provider snapshot or the market has not arrived.
    pub(crate) fn market_limits(&self, core: CoreId, market: &str) -> Option<MarketLimits> {
        self.session.market_source().market_limits(core, market)
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

    /// Return one group's latest ordered Auto dock-surface transition.
    ///
    /// Args:
    ///     group: Exact group window whose navigation cursor is being reconciled.
    ///
    /// Returns:
    ///     Latest revision and surface, or `None` before the group publishes its first transition.
    pub(crate) fn auto_workspace_surface_request(
        &self,
        group: &str,
    ) -> Option<(u64, crate::workspace::AutoWorkspaceSurface)> {
        self.auto_workspace_surface_requests.current(group)
    }

    /// Publish ChartTabs after a group successfully consumes and opens a Main request.
    ///
    /// Args:
    ///     group: Exact group whose `ChartTabs` accepted the request.
    ///
    /// Returns:
    ///     Nothing; the shared sequence advances after every successful Main open.
    pub(crate) fn request_chart_tabs_after_main_open(&mut self, group: &str) {
        self.auto_workspace_surface_requests
            .request(group, crate::workspace::AutoWorkspaceSurface::ChartTabs);
    }

    /// Publish Report only after Escape actually empties an Auto group's Main stack.
    ///
    /// Args:
    ///     group: Exact group addressed by the Escape request.
    ///     closed: Whether `MainChartStack::close_active` removed a chart.
    ///     main_surface_active: Whether Main was visible instead of an Add or Custom chart tab.
    ///
    /// Returns:
    ///     `true` when a new Report transition was published and observers need notification.
    pub(crate) fn request_report_after_main_close(
        &mut self,
        group: &str,
        closed: bool,
        main_surface_active: bool,
    ) -> bool {
        if !crate::workspace::should_return_to_report_after_main_close(
            self.workspace_mode(group),
            closed,
            main_surface_active,
            self.main_open_markets(group).len(),
        ) {
            return false;
        }
        self.auto_workspace_surface_requests
            .request(group, crate::workspace::AutoWorkspaceSurface::Report);
        true
    }

    /// Publish Assets so Auto can reveal the group-owned panel instead of a detached window.
    ///
    /// Classic is a no-op: the request is not published, so a header double-click opens the Assets
    /// window on its own path rather than relying on this method.
    ///
    /// Args:
    ///     group: Exact group whose Assets surface should become visible.
    ///
    /// Returns:
    ///     `true` when a new Auto Assets transition was published and observers need notification.
    pub(crate) fn request_assets_surface(&mut self, group: &str) -> bool {
        if self.workspace_mode(group) != WorkspaceMode::AutoTrading {
            return false;
        }
        self.auto_workspace_surface_requests
            .request(group, crate::workspace::AutoWorkspaceSurface::Assets);
        true
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

    /// Resolve one concrete trading address for commands and core-settings controls.
    ///
    /// A valid selected Auto workspace core takes precedence. Auto Overview and Classic then use
    /// the still-valid remembered header selection, followed by the visible chart target and the
    /// group's first core. This keeps writes singular even while Overview shows all cores; display
    /// surfaces must gate per-core figures with [`Self::is_auto_overview_scope`] before reading it.
    ///
    /// Args:
    ///     group: Window group whose command or core-settings control needs a concrete core.
    ///
    /// Returns:
    ///     A live core belonging to the group, or `None` when the group has no live cores.
    pub(crate) fn active_trade_core(&self, group: &str) -> Option<CoreId> {
        if self.workspace_mode(group) == WorkspaceMode::AutoTrading
            && let Some(core) = self.valid_auto_workspace_core(group)
        {
            return Some(core);
        }
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

    /// Set the group's active trading core in the shared durable Classic layout.
    ///
    /// Args:
    ///     group: Window group that owns the selection.
    ///     core: Stable UID of the selected live core. Values outside `group` are ignored.
    ///
    /// Returns:
    ///     Nothing; Auto mode refuses this legacy writer, while a changed Classic selection marks
    ///     layout persistence dirty.
    pub(crate) fn set_active_trade_core(&mut self, group: &str, core: CoreId) {
        // The Auto Shell rail uses `select_auto_workspace_core`, whose revision wakes scoped
        // consumers. Refusing the legacy writer here is the final guard against chart/header paths
        // silently replacing the user's remembered Classic manual-trading core.
        if !crate::workspace::should_remember_classic_trade_core(self.workspace_mode(group)) {
            return;
        }
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

    /// Return the persisted workspace mode for one group, defaulting legacy layouts to Classic.
    ///
    /// Args:
    ///     group: Group window whose preset is requested.
    ///
    /// Returns:
    ///     The group's own saved mode, else the layout-wide default. First-run loading is what
    ///     seeds that default; a layout carrying none resolves to [`WorkspaceMode::Classic`],
    ///     which is every layout written before the preset existed. The fallback lives here
    ///     because this is the single read choke point every workspace-mode consumer goes through.
    pub(crate) fn workspace_mode(&self, group: &str) -> WorkspaceMode {
        self.layout
            .workspace_mode_by_group
            .get(group)
            .copied()
            .unwrap_or_else(|| self.layout.default_workspace_mode())
    }

    /// Return the raw persisted Auto top-tab preference for one group.
    ///
    /// Args:
    ///     group: Group window whose Auto preference is requested.
    ///
    /// Returns:
    ///     Saved stable panel name. The Shell validates eligibility and applies its safe fallback.
    pub(crate) fn auto_workspace_tab(&self, group: &str) -> Option<&str> {
        self.layout
            .auto_workspace_tab_by_group
            .get(group)
            .map(String::as_str)
    }

    /// Persist a Shell-validated eligible Auto top-tab name for one group.
    ///
    /// This preference changes neither effective workspace scope nor another live Shell, so it
    /// marks only `layout.toml` dirty and deliberately publishes no workspace revision.
    ///
    /// Args:
    ///     group: Owning Auto workspace group.
    ///     panel_name: Stable eligible panel name already validated by the Shell.
    ///
    /// Returns:
    ///     `true` only when the saved preference changed.
    pub(crate) fn set_auto_workspace_tab(&mut self, group: &str, panel_name: &str) -> bool {
        if self.auto_workspace_tab(group) == Some(panel_name) {
            return false;
        }
        self.layout
            .auto_workspace_tab_by_group
            .insert(group.to_string(), panel_name.to_string());
        self.layout_dirty = true;
        true
    }

    /// Return the shared Auto-workspace availability facts for one configured core.
    ///
    /// Args:
    ///     group: Owning group expected by the caller.
    ///     core: Stable core UID to resolve across config, session, and window lifecycle.
    ///
    /// Returns:
    ///     Complete availability record consumed by scope, setters, trade overlay, and roster.
    pub(crate) fn workspace_core_availability(
        &self,
        group: &str,
        core: CoreId,
    ) -> crate::workspace::WorkspaceCoreAvailability {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server.id == core && server.group == group);
        let window = if self.group_windows.contains_key(group) {
            crate::workspace::WorkspaceWindowState::Live
        } else if self.opening_group_windows.contains(group) {
            crate::workspace::WorkspaceWindowState::Opening
        } else {
            crate::workspace::WorkspaceWindowState::Missing
        };
        crate::workspace::WorkspaceCoreAvailability {
            // Missing group metadata means the configured server uses GroupConfig's active
            // defaults; requiring an explicit groups.toml row would disable legacy groups.
            group_active: self
                .config
                .group_ref(group)
                .is_none_or(|group| group.active),
            core_active: server.is_some_and(|server| server.active),
            live_session: self.core_belongs_to_group(group, core),
            window,
        }
    }

    /// Return a saved Auto core only while it remains a live member of the owning group.
    ///
    /// Args:
    ///     group: Group whose Auto workspace selection is requested.
    ///
    /// Returns:
    ///     Valid selected core, or `None` for Overview and stale persisted references.
    pub(crate) fn valid_auto_workspace_core(&self, group: &str) -> Option<CoreId> {
        self.layout
            .auto_workspace_core_by_group
            .get(group)
            .copied()
            .filter(|core| {
                self.workspace_core_availability(group, *core)
                    .is_available()
            })
    }

    /// Return whether the group's visible scope names NO single core — the Auto Overview state.
    ///
    /// The chrome that prints a per-core figure — the header balance, the toolbar's leverage and
    /// the exchange max-order readout — asks THIS rather than [`Self::active_trade_core`], whose
    /// Overview fallback silently answers with the group's first core. See
    /// [`crate::workspace::is_auto_overview_scope`] for why the two must differ.
    ///
    /// Args:
    ///     group: Group whose header and toolbar are being rendered.
    ///
    /// Returns:
    ///     `true` only in Auto Overview, where no single core owns the visible scope.
    pub(crate) fn is_auto_overview_scope(&self, group: &str) -> bool {
        crate::workspace::is_auto_overview_scope(
            self.workspace_mode(group),
            self.valid_auto_workspace_core(group),
        )
    }

    /// Resolve one group panel's effective scope without mutating its retained Classic filter.
    ///
    /// Args:
    ///     group: Owning group window.
    ///     retained: Panel-owned Classic all/subset filter.
    ///
    /// Returns:
    ///     Canonical live core IDs selected by Classic, Auto Overview, or Auto selected-core mode.
    pub(crate) fn effective_workspace_scope(
        &self,
        group: &str,
        retained: crate::workspace::RetainedCoreScope<'_>,
    ) -> crate::workspace::EffectiveCoreScope {
        let cores: Vec<CoreId> = self
            .group_cores(group)
            .into_iter()
            .map(|(core, _)| core)
            .filter(|core| {
                self.workspace_core_availability(group, *core)
                    .is_available()
            })
            .collect();
        crate::workspace::resolve_group_scope(
            self.workspace_mode(group),
            self.valid_auto_workspace_core(group),
            &cores,
            retained,
        )
    }

    /// Authorize a delayed core-specific action against the current Auto workspace.
    ///
    /// Args:
    ///     group: Optional owning group. Group panels and charts pass their owner; standalone and
    ///         deliberately global callers pass `None` and preserve their existing authority.
    ///     core: Core captured by the row, menu, dialog, or asynchronous request.
    ///
    /// Returns:
    ///     `false` only when an Auto-owned group no longer exposes `core`; Classic and unscoped
    ///     callers retain their existing behavior.
    pub(crate) fn workspace_action_allows_core(&self, group: Option<&str>, core: CoreId) -> bool {
        let Some(group) = group else {
            return true;
        };
        self.core_belongs_to_group(group, core)
            && (self.workspace_mode(group) != WorkspaceMode::AutoTrading
                || self
                    .effective_workspace_scope(group, crate::workspace::RetainedCoreScope::All)
                    .contains(core))
    }

    /// Queue one Main-chart navigation only while its captured core remains workspace-visible.
    ///
    /// Args:
    ///     group: Group that owned the rendered callback, or `None` for a standalone/global host.
    ///     target: Captured core and market to reveal on Main.
    ///     activate: Whether the receiving group window may be raised for this request.
    ///
    /// Returns:
    ///     `true` when the request was authorized and queued; stale Auto callbacks return `false`.
    pub(crate) fn open_on_main_if_authorized(
        &mut self,
        group: Option<&str>,
        target: (CoreId, String),
        activate: bool,
    ) -> bool {
        if !self.workspace_action_allows_core(group, target.0) {
            return false;
        }
        self.queue_open_on_main(
            target,
            ChartHistoryScope::Default,
            group.map(str::to_string),
            activate,
        );
        true
    }

    /// Queue a Report-refined Main-chart navigation while preserving its exact captured core.
    ///
    /// Args:
    ///     group: Group that owned the published Report row, or `None` for standalone Report.
    ///     target: Captured core and catalog-verified market.
    ///     history: Published Report scope and clicked-row identity.
    ///     activate: Whether the receiving group window may be raised.
    ///
    /// Returns:
    ///     `true` when current workspace authority accepted the exact core.
    pub(crate) fn open_report_on_main_if_authorized(
        &mut self,
        group: Option<&str>,
        target: (CoreId, String),
        history: ChartHistoryScope,
        activate: bool,
    ) -> bool {
        if !self.workspace_action_allows_core(group, target.0) {
            return false;
        }
        self.queue_open_on_main(target, history, group.map(str::to_string), activate);
        true
    }

    /// Queue one comparison navigation only while its captured core remains workspace-visible.
    ///
    /// Args:
    ///     group: Group that owned the rendered callback, or `None` for an unscoped host.
    ///     target: Captured core and market used to seed the comparison tab.
    ///
    /// Returns:
    ///     `true` when the current authority accepted and published the request.
    pub(crate) fn open_compare_if_authorized(
        &mut self,
        group: Option<&str>,
        target: (CoreId, String),
    ) -> bool {
        if !self.workspace_action_allows_core(group, target.0) {
            return false;
        }
        self.open_compare_request =
            Some(OpenCompareRequest::new(target, group.map(str::to_string)));
        self.open_compare_request_rev = self.open_compare_request_rev.wrapping_add(1);
        true
    }

    /// Ask for a comparison BETWEEN two charts: the one it was started from, and the target.
    ///
    /// Both cores are checked against the workspace rail, not just the target: the anchor is put on
    /// the same tab, and a comparison that quietly dropped it would answer a different question
    /// from the one asked.
    pub(crate) fn open_compare_pair_if_authorized(
        &mut self,
        group: Option<&str>,
        anchor: (CoreId, String),
        target: (CoreId, String),
    ) -> bool {
        if !self.workspace_action_allows_core(group, target.0)
            || !self.workspace_action_allows_core(group, anchor.0)
        {
            return false;
        }
        self.open_compare_request = Some(OpenCompareRequest::pair(
            anchor,
            target,
            group.map(str::to_string),
        ));
        self.open_compare_request_rev = self.open_compare_request_rev.wrapping_add(1);
        true
    }

    /// Revalidate and drain one comparison request only for its live authorized group.
    ///
    /// Args:
    ///     group: ChartTabs group attempting to consume the request.
    ///
    /// Returns:
    ///     Owned target when live ownership and captured authority still match; stale scoped
    ///     requests are discarded rather than rerouted.
    pub(crate) fn take_open_compare_request_for_group(
        &mut self,
        group: &str,
    ) -> Option<((CoreId, String), Option<(CoreId, String)>)> {
        let request = self.open_compare_request.as_ref()?;
        let (core, _) = &request.target;
        let live_group = self
            .session
            .sessions()
            .iter()
            .find(|session| session.id == *core)
            .map(|session| session.group.as_str());
        let workspace_allowed = request
            .authority_group
            .as_deref()
            .is_none_or(|authority| self.workspace_action_allows_core(Some(authority), *core));
        if !request.allows_group(group, live_group, workspace_allowed) {
            if request.authority_group.is_some() {
                self.open_compare_request = None;
            }
            return None;
        }
        self.open_compare_request
            .take()
            .map(|request| (request.target, request.anchor))
    }

    /// Return the comparison revision only to the request's current authorized group.
    ///
    /// Args:
    ///     group: ChartTabs group assembling its observer signature.
    ///
    /// Returns:
    ///     Current request revision when this group may consume it, otherwise zero.
    pub(crate) fn pending_open_compare_revision_for_group(&self, group: &str) -> u64 {
        let Some(request) = self.open_compare_request.as_ref() else {
            return 0;
        };
        let (core, _) = &request.target;
        let live_group = self
            .session
            .sessions()
            .iter()
            .find(|session| session.id == *core)
            .map(|session| session.group.as_str());
        let workspace_allowed = request
            .authority_group
            .as_deref()
            .is_none_or(|authority| self.workspace_action_allows_core(Some(authority), *core));
        if request.allows_group(group, live_group, workspace_allowed) {
            self.open_compare_request_rev
        } else {
            0
        }
    }

    /// Resolve the live Auto owner inherited by Analytics and Strategies.
    ///
    /// Returns:
    ///     Last focused live Auto group plus its valid selected core, or `None` so the singleton
    ///     retains its own Classic filter.
    pub(crate) fn singleton_workspace(&self) -> Option<crate::workspace::SingletonWorkspace> {
        let focus = self.workspace_focus.as_ref()?;
        let group = focus.group();
        let owner_registered =
            self.group_windows.contains_key(group) || self.opening_group_windows.contains(group);
        let live_cores = self
            .group_cores(group)
            .into_iter()
            .map(|(core, _)| core)
            .filter(|core| {
                self.workspace_core_availability(group, *core)
                    .is_available()
            })
            .collect::<Vec<_>>();
        crate::workspace::resolve_singleton_workspace(
            group,
            owner_registered,
            self.workspace_mode(group),
            self.layout.auto_workspace_core_by_group.get(group).copied(),
            &live_cores,
        )
    }

    /// Return the dedicated entity observed by cached and asynchronous workspace consumers.
    ///
    /// Returns:
    ///     Shared revision entity whose notifications describe effective-scope invalidations.
    pub(crate) fn workspace_revision(&self) -> gpui::Entity<crate::workspace::WorkspaceRevision> {
        self.workspace_revision.clone()
    }

    /// Return the revision entity notified after retained market data changes.
    ///
    /// Returns:
    ///     A narrow wake channel for catalog-sensitive consumers such as Auto chart retargeting.
    pub(crate) fn market_data_revision(&self) -> gpui::Entity<crate::MarketDataRevision> {
        self.market_data_revision.clone()
    }

    /// Return the cores the Profit Monitor is currently broadcasting.
    ///
    /// Returns:
    ///     Selected core ids; empty means every core, matching each panel's own retained filter.
    pub(crate) fn core_filter(&self) -> &HashSet<CoreId> {
        &self.core_filter
    }

    /// Return the wake channel every core-selector panel observes for the broadcast filter.
    ///
    /// Returns:
    ///     Shared notification-only entity advanced by [`Self::set_core_filter`].
    pub(crate) fn core_filter_revision(&self) -> gpui::Entity<crate::CoreFilterRevision> {
        self.core_filter_revision.clone()
    }

    /// Publish a new cross-window core filter to every panel that owns a core selector.
    ///
    /// Equality-guarded: a click that resolves to the selection already on air must not wake five
    /// panels into rebuilding rows that cannot have changed.
    ///
    /// Args:
    ///     cores: Replacement selection; empty releases every panel back to all cores.
    ///     cx: Backend context used to notify the dedicated revision entity.
    ///
    /// Returns:
    ///     Nothing; observers see the new value only after the notification.
    pub(crate) fn set_core_filter(&mut self, cores: HashSet<CoreId>, cx: &mut Context<Self>) {
        if self.core_filter == cores {
            return;
        }
        self.core_filter = cores;
        self.core_filter_revision
            .update(cx, |_revision, revision_cx| revision_cx.notify());
    }

    /// Return the revision entity observed by every Auto Shell layout consumer.
    ///
    /// Returns:
    ///     Shared notification authority for topology and global rail-width changes.
    pub(crate) fn auto_workspace_layout_revision(
        &self,
    ) -> gpui::Entity<crate::workspace::AutoWorkspaceLayoutRevision> {
        self.auto_workspace_layout_revision.clone()
    }

    /// Borrow the persisted process-wide Auto dock topology authority.
    ///
    /// Returns:
    ///     Normalized panel-name topology loaded from or destined for `auto_dock.json`, or `None`
    ///     so a Shell uses the deterministic safe preset for missing or protected invalid data.
    pub(crate) fn auto_dock_topology(&self) -> Option<&DockTopologyByName> {
        self.auto_dock_topology.as_ref()
    }

    /// Accept a user-edited shared Auto dock topology when its normalized value changed.
    ///
    /// Args:
    ///     topology: Topology-only panel-name tree projected from the user-edited Auto dock.
    ///     cx: Backend context used to notify every other open Auto Shell.
    ///
    /// Returns:
    ///     `true` when the authority changed and now awaits persistence to `auto_dock.json`.
    pub(crate) fn set_auto_dock_topology(
        &mut self,
        topology: DockTopologyByName,
        cx: &mut Context<Self>,
    ) -> bool {
        let topology = topology.normalized();
        if self.auto_dock_topology.as_ref() == Some(&topology) {
            return false;
        }
        self.auto_dock_automatic_persistence_allowed = true;
        self.auto_dock_topology = Some(topology);
        self.auto_dock_dirty = true;
        self.publish_auto_workspace_layout_revision(cx);
        true
    }

    /// Store a live dock dump only while the group is in Classic mode.
    ///
    /// Args:
    ///     group: Group whose live DockArea produced the state.
    ///     state: Complete serialized live dock state.
    ///
    /// Returns:
    ///     `true` when Classic persistence accepted the state; Auto callers are ignored so their
    ///     shared topology and temporary panel instances cannot overwrite `docks.json`.
    pub(crate) fn store_classic_dock_state(&mut self, group: String, state: DockAreaState) -> bool {
        if self.workspace_mode(&group) == WorkspaceMode::AutoTrading {
            return false;
        }
        self.dock_states.insert(group, state);
        self.dock_dirty = true;
        true
    }

    /// Reconcile topology produced by a programmatic Auto install or name-based repair.
    ///
    /// Missing first-run state and valid loaded state may persist this automatic transition.
    /// Invalid or unreadable startup state remains protected until
    /// [`Self::set_auto_dock_topology`] receives a distinct user-edited topology.
    ///
    /// Args:
    ///     topology: Actual normalized topology resolved against one Shell's live panel names.
    ///     cx: Backend context used to notify other Auto Shells when the authority changes.
    ///
    /// Returns:
    ///     `true` when the in-memory authority changed.
    pub(crate) fn reconcile_auto_dock_topology(
        &mut self,
        topology: DockTopologyByName,
        cx: &mut Context<Self>,
    ) -> bool {
        let topology = topology.normalized();
        if self.auto_dock_topology.as_ref() == Some(&topology) {
            return false;
        }
        self.auto_dock_topology = Some(topology);
        if self.auto_dock_automatic_persistence_allowed {
            self.auto_dock_dirty = true;
        }
        self.publish_auto_workspace_layout_revision(cx);
        true
    }

    /// Return the persisted logical-pixel Auto rail width shared by every group window.
    ///
    /// Returns:
    ///     Finite width clamped by the layout decoder and every runtime setter.
    pub(crate) fn auto_workspace_rail_width(&self) -> f32 {
        self.layout.auto_workspace_rail_width()
    }

    /// Persist and publish a global Auto rail resize only when its normalized value changed.
    ///
    /// Args:
    ///     requested: Raw logical-pixel width reported by one Shell resize state.
    ///     cx: Backend context used to notify every other open Shell.
    ///
    /// Returns:
    ///     `true` when the clamped preference changed; repeated equal samples are ignored.
    pub(crate) fn set_auto_workspace_rail_width(
        &mut self,
        requested: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(width) = crate::workspace::changed_auto_workspace_rail_width(
            self.auto_workspace_rail_width(),
            requested,
        ) else {
            return false;
        };
        self.layout.auto_workspace_rail_width = Some(width);
        self.layout_dirty = true;
        self.publish_auto_workspace_layout_revision(cx);
        true
    }

    /// Persist a group workspace mode and publish one effective-scope transition.
    ///
    /// Args:
    ///     group: Live configured group whose preset changes.
    ///     mode: New workspace preset.
    ///     cx: Backend context used to publish the dedicated revision.
    ///
    /// Returns:
    ///     `true` when mode or singleton ownership changed.
    pub(crate) fn set_workspace_mode(
        &mut self,
        group: &str,
        mode: WorkspaceMode,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.group_is_configured(group) {
            return false;
        }
        let mode_changed = self.workspace_mode(group) != mode;
        let focus_changed = match mode {
            WorkspaceMode::Classic => {
                crate::workspace::close_workspace_owner(&mut self.workspace_focus, group)
            }
            WorkspaceMode::AutoTrading => {
                crate::workspace::focus_workspace_owner(&mut self.workspace_focus, group)
            }
        };
        if !mode_changed && !focus_changed {
            return false;
        }
        if mode_changed {
            self.layout
                .workspace_mode_by_group
                .insert(group.to_string(), mode);
            self.layout_dirty = true;
        }
        self.publish_workspace_revision(cx);
        true
    }

    /// Select one live core or Overview for an already active Auto workspace.
    ///
    /// Args:
    ///     group: Owning group window.
    ///     core: Live group core, or `None` for Overview.
    ///     cx: Backend context used to publish one revision.
    ///
    /// Returns:
    ///     `true` when persisted selection or singleton ownership changed.
    pub(crate) fn select_auto_workspace_core(
        &mut self,
        group: &str,
        core: Option<CoreId>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.workspace_mode(group) != WorkspaceMode::AutoTrading
            || core
                .is_some_and(|core| !self.workspace_core_availability(group, core).is_available())
        {
            return false;
        }
        let previous = self.layout.auto_workspace_core_by_group.get(group).copied();
        if let Some(core) = core {
            self.layout
                .auto_workspace_core_by_group
                .insert(group.to_string(), core);
        } else {
            self.layout.auto_workspace_core_by_group.remove(group);
        }
        let selection_changed = previous != core;
        let focus_changed =
            crate::workspace::focus_workspace_owner(&mut self.workspace_focus, group);
        if !selection_changed && !focus_changed {
            return false;
        }
        if selection_changed {
            self.layout_dirty = true;
        }
        self.publish_workspace_revision(cx);
        true
    }

    /// Enter Auto mode and select a destination core as one cross-group transition.
    ///
    /// Args:
    ///     group: Destination group whose existing window will be activated by the caller.
    ///     core: Live core in that destination group.
    ///     cx: Backend context used to publish one revision.
    ///
    /// Returns:
    ///     `true` when the validated transition changed state.
    pub(crate) fn activate_auto_workspace_core(
        &mut self,
        group: &str,
        core: CoreId,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.workspace_core_availability(group, core).is_available() {
            return false;
        }
        let mode_changed = self.workspace_mode(group) != WorkspaceMode::AutoTrading;
        let selection_changed = self.valid_auto_workspace_core(group) != Some(core);
        let focus_changed =
            crate::workspace::focus_workspace_owner(&mut self.workspace_focus, group);
        if !mode_changed && !selection_changed && !focus_changed {
            return false;
        }
        self.layout
            .workspace_mode_by_group
            .insert(group.to_string(), WorkspaceMode::AutoTrading);
        self.layout
            .auto_workspace_core_by_group
            .insert(group.to_string(), core);
        self.layout_dirty |= mode_changed || selection_changed;
        self.publish_workspace_revision(cx);
        true
    }

    /// Record a live Auto group as the owner of singleton scope.
    ///
    /// Args:
    ///     group: Group whose toolbar or interaction established ownership.
    ///     cx: Backend context used to publish one revision.
    ///
    /// Returns:
    ///     `true` when focus moved to this group.
    pub(crate) fn focus_auto_workspace(&mut self, group: &str, cx: &mut Context<Self>) -> bool {
        if self.workspace_mode(group) != WorkspaceMode::AutoTrading
            || !self.group_windows.contains_key(group)
            || !crate::workspace::focus_workspace_owner(&mut self.workspace_focus, group)
        {
            return false;
        }
        self.publish_workspace_revision(cx);
        true
    }

    /// Remove one registered primary group window and publish its workspace transition once.
    ///
    /// Args:
    ///     closed_id: Native window identity reported by GPUI.
    ///     cx: Backend context used for the single workspace revision publication.
    ///
    /// Returns:
    ///     Closed group and whether it was the last primary window, or `None` for detached and
    ///     already-processed window identities.
    pub(crate) fn close_group_window(
        &mut self,
        closed_id: WindowId,
        cx: &mut Context<Self>,
    ) -> Option<(String, bool)> {
        let group = self
            .group_windows
            .iter()
            .find(|(_, handle)| handle.window_id() == closed_id)
            .map(|(group, _)| group.clone())?;
        self.group_windows.remove(&group)?;
        self.opening_group_windows.remove(&group);
        crate::workspace::close_workspace_owner(&mut self.workspace_focus, &group);
        self.publish_workspace_revision(cx);
        Some((group, self.group_windows.is_empty()))
    }

    /// Publish one final combined config and group-window lifecycle transition.
    ///
    /// Args:
    ///     closed_groups: Groups removed in this final transition; empty for a completed open.
    ///     cx: Backend context used to publish exactly one transition.
    ///
    /// Returns:
    ///     Nothing; ownership is reconciled once against final state before the notification.
    pub(crate) fn publish_workspace_window_change(
        &mut self,
        closed_groups: &[String],
        cx: &mut Context<Self>,
    ) {
        let focus_valid = self.workspace_focus.as_ref().is_none_or(|focus| {
            self.workspace_mode(focus.group()) == WorkspaceMode::AutoTrading
                && self.group_is_configured(focus.group())
                && !closed_groups.iter().any(|group| group == focus.group())
                && (self.group_windows.contains_key(focus.group())
                    || self.opening_group_windows.contains(focus.group()))
        });
        crate::workspace::reconcile_workspace_focus(&mut self.workspace_focus, focus_valid);
        self.publish_workspace_revision(cx);
    }

    /// Return whether a group still has an active configured window owner.
    ///
    /// Args:
    ///     group: Group name to validate against the committed configuration.
    ///
    /// Returns:
    ///     `true` when the group appears in the canonical group-window enumeration.
    fn group_is_configured(&self, group: &str) -> bool {
        crate::window::group_window::groups(&self.config)
            .iter()
            .any(|configured| configured == group)
    }

    /// Advance and notify the dedicated workspace revision entity.
    ///
    /// Args:
    ///     cx: Backend context whose app handle updates the revision entity.
    ///
    /// Returns:
    ///     Nothing; one call produces one generation increment and one notification.
    fn publish_workspace_revision(&mut self, cx: &mut Context<Self>) {
        self.workspace_revision
            .update(cx, |revision, revision_cx| revision.advance(revision_cx));
    }

    /// Publish one equality-guarded Auto topology or rail-width transition.
    ///
    /// Args:
    ///     cx: Backend context whose app handle updates the shared revision entity.
    ///
    /// Returns:
    ///     Nothing; callers must update their authority before publishing.
    fn publish_auto_workspace_layout_revision(&mut self, cx: &mut Context<Self>) {
        self.auto_workspace_layout_revision
            .update(cx, |revision, revision_cx| revision.advance(revision_cx));
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
            if let Some(core) = self.core_of_uid(sel.core_uid) {
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
        let Some(uid) = self.uid_of(core) else {
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

    /// The stable UID of a configured core, or `None` when it has no config entry.
    ///
    /// Persisted UI state names cores by UID rather than by `CoreId` so it survives a configuration
    /// reorder; this is the one place that translation is written, in either direction, together
    /// with [`Self::core_of_uid`].
    ///
    /// Args:
    ///     core: Live session identifier to translate.
    ///
    /// Returns:
    ///     The configured stable UID, or `None` when the core has no configuration entry.
    fn uid_of(&self, core: CoreId) -> Option<u64> {
        self.config
            .servers
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.uid)
    }

    /// The live `CoreId` a persisted UID refers to, or `None` when that core is gone.
    ///
    /// Args:
    ///     uid: Stable configured UID to resolve.
    ///
    /// Returns:
    ///     The current session identifier, or `None` when the configuration no longer contains it.
    fn core_of_uid(&self, uid: u64) -> Option<CoreId> {
        self.config
            .servers
            .iter()
            .find(|s| s.uid == uid)
            .map(|s| s.id)
    }

    /// Record a market as the most recently opened one in the coin-search history.
    ///
    /// Thin wrapper: the MRU policy (move-to-front, dedup, cap) lives on [`WindowLayout`], beside
    /// the field it persists. A core with no config entry has no stable UID to store, so the call
    /// is a no-op rather than writing a UID that cannot be resolved back.
    ///
    /// Args:
    ///     core: Live core on which the market was opened.
    ///     market: Canonical market name.
    pub(crate) fn push_recent_coin(&mut self, core: CoreId, market: &str) {
        let Some(uid) = self.uid_of(core) else {
            return;
        };
        if self.layout.push_recent_coin(uid, market) {
            self.layout_dirty = true;
        }
    }

    /// Recently opened markets, newest first, resolved from stable UIDs to `CoreId`s.
    ///
    /// An entry whose core is gone from the configuration is skipped, but one whose core is merely
    /// OFFLINE is kept: liveness is decided once, downstream, by the resolver that also needs the
    /// session's name (`controls::coin_search::hits_for`), so this does not scan sessions itself.
    /// Nothing is ever dropped from the file here — a core that is offline right now keeps its
    /// history.
    ///
    /// Returns:
    ///     Resolvable `(core, market)` entries in most-recent-first order.
    pub(crate) fn recent_coins(&self) -> Vec<(CoreId, String)> {
        self.layout
            .recent_coins
            .iter()
            .flatten()
            .filter_map(|entry| Some((self.core_of_uid(entry.core_uid)?, entry.market.clone())))
            .collect()
    }

    /// Rebuild this field's coin suggestions unless a fresh list is already cached.
    ///
    /// Called when a coin-search popup OPENS — never from a render pass. Building the list walks
    /// every market of every provider feeding the field, which is far too expensive to repeat at
    /// frame rate; the chart chrome must not do work at present frequency.
    ///
    /// Args:
    ///     group: Window group whose cores feed the search field.
    ///     bucket: Optional chart bucket narrowing that core scope.
    pub(crate) fn refresh_coin_suggest(
        &mut self,
        group: &str,
        bucket: Option<&moon_core::config::ChartBucket>,
    ) {
        use crate::controls::coin_search;

        let key = (group.to_string(), bucket.cloned());
        let sig = coin_search::universe_sig(self, group, bucket);
        if self
            .coin_suggest
            .get(&key)
            .is_some_and(|entry| entry.is_fresh(&sig))
        {
            return;
        }
        let markets = coin_search::suggest_volatile(
            self,
            group,
            bucket,
            crate::controls::coin_search::COIN_SUGGEST_LIMIT,
        )
        .into_iter()
        .map(|hit| (hit.core, hit.market))
        .collect::<Vec<_>>();
        // An empty answer is "not ready yet", not "nothing to suggest": at startup the providers'
        // market snapshots have not arrived, so caching that emptiness would keep the section blank
        // — and the popup reading "no connected cores" — for the whole TTL after data does arrive.
        // Drop the entry instead, and let the next open try again.
        if markets.is_empty() {
            self.coin_suggest.remove(&key);
            return;
        }
        self.coin_suggest.insert(
            key,
            coin_search::CoinSuggestEntry {
                at: std::time::Instant::now(),
                sig,
                markets,
            },
        );
    }

    /// The cached suggestion markets for this field, or nothing when none is valid right now.
    ///
    /// This runs on the popup's RENDER path, so it only checks the entry's age. Validating the core
    /// and provider set here instead would re-sort the group's cores and take one market-source
    /// lock per core on every frame the popup is open — precisely the work the chart chrome must
    /// not do at present frequency. That check belongs to [`Self::refresh_coin_suggest`], which
    /// runs when the popup opens; between two opens an entry can at worst outlive a core by the
    /// TTL, and a suggestion for a core that just dropped resolves to nothing downstream anyway.
    ///
    /// Args:
    ///     group: Window group whose cached entry should be read.
    ///     bucket: Optional chart bucket narrowing that cache key.
    ///
    /// Returns:
    ///     Cached `(core, market)` pairs, or an empty vector when the entry is absent or expired.
    pub(crate) fn coin_suggest_markets(
        &self,
        group: &str,
        bucket: Option<&moon_core::config::ChartBucket>,
    ) -> Vec<(CoreId, String)> {
        self.coin_suggest
            .get(&(group.to_string(), bucket.cloned()))
            .filter(|entry| entry.is_recent())
            .map(|entry| entry.markets.clone())
            .unwrap_or_default()
    }

    /// The exact IANA zone id used application-wide, or `None` for an untouched profile.
    ///
    /// Returns the raw id rather than a parsed zone: resolution lives in the chrome layer, which
    /// already depends on `Backend`, and returning its type from here would close that loop.
    ///
    /// Returns:
    ///     Persisted IANA id, or `None` only for an untouched profile.
    pub(crate) fn header_clock_zone(&self) -> Option<&str> {
        self.layout.header_clock_zone.as_deref()
    }

    /// Store the application-wide display zone and publish a dedicated zone revision.
    ///
    /// `offset_min` is that zone's current offset, mirrored into the compatibility field so readers
    /// that understand only fixed offsets still show the right clock. Such a reader can rewrite the
    /// layout without the zone field, so a stale mirror would also lose the selection on its next
    /// save. A mirror-only change marks the layout dirty because summer time can move the offset
    /// while the zone remains stable; `chrome::clock` derives both values from one IANA zone.
    ///
    /// Args:
    ///     zone: Valid IANA zone id used by every civil-time surface.
    ///     offset_min: Current offset mirror retained for older layout readers.
    ///     cx: Backend context used to notify civil-time consumers when the zone identity changes.
    ///
    /// Returns:
    ///     Nothing; changed fields are marked dirty and zone observers are notified in place.
    pub(crate) fn set_header_clock_zone(
        &mut self,
        zone: &str,
        offset_min: i32,
        cx: &mut Context<Self>,
    ) {
        crate::chartdx::axes::set_display_zone(crate::chrome::clock::resolved_header_clock_zone(
            Some(zone),
        ));
        let zone_changed = self.layout.header_clock_zone.as_deref() != Some(zone);
        if zone_changed || self.layout.header_clock_offset_min != offset_min {
            self.layout.header_clock_zone = Some(zone.to_string());
            self.layout.header_clock_offset_min = offset_min;
            self.layout_dirty = true;
            if zone_changed {
                self.display_time_revision.update(cx, |_, cx| cx.notify());
            }
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

    /// Which conversion every quote-money surface currently applies.
    ///
    /// One application-wide setting rather than one per panel: the many simultaneous Report hosts —
    /// a docked tab per group window, a detached window per group, the scoped standalone window,
    /// and Analytics — must not present the same period under two conversions. It also makes the
    /// worker's demand signal exact: one setting, one flag, no reference counting to leak.
    ///
    /// Returns:
    ///     The saved valuation mode.
    pub(crate) fn valuation_mode(&self) -> ValuationMode {
        self.config.report_valuation_mode
    }

    /// The time axis every replicated report timestamp is DISPLAYED on.
    ///
    /// Built from the retained per-core snapshots rather than by reading `reports.sqlite`, because
    /// this is called while rendering: a database read per frame would put a file-system round trip
    /// inside the paint path, and the numbers it would return are already in memory — the writer
    /// publishes `FeedMsg::TimeOffset` only AFTER its transaction commits, so a core whose snapshot
    /// carries an offset is a core whose segment is already durable.
    ///
    /// This axis carries ONE segment per core, the one in force. The read paths load the full
    /// segment history from the table inside their own pinned snapshot; they need it because they
    /// convert instants across all of history, while a panel only ever renders what is current.
    /// The two therefore agree on exactly the thing they share, which is the CURRENT offset.
    ///
    /// A core with no measurement contributes no segment, so it converts as the identity -- the
    /// same answer the uncorrected terminal gave, which is the only assumption that cannot make an
    /// honest UTC core worse.
    ///
    /// # Known limitation: ONE segment, so no offset HISTORY
    ///
    /// The durable table keeps an append-only segment list and picks the segment covering each
    /// row's own instant. This axis carries only the CURRENT segment per core, because it is built
    /// from the retained live snapshot rather than from SQLite -- and `ReportAxis` extends its
    /// earliest segment backward without bound, so every historical row is corrected by the offset
    /// in force TODAY.
    ///
    /// For a core whose offset never moves -- which is every fixed-offset fleet, and every case
    /// the reported bug was about -- that is exactly correct. It goes wrong only once a core's
    /// offset actually CHANGES, i.e. across a DST transition or a machine that moved zone: rows
    /// from before the change then render one delta out, and the Report's period bounds shift with
    /// them, so the grid and the filter stay consistent with each other while both sit an hour off
    /// for the older half of the year. The error is bounded by the offset delta and never
    /// compounds.
    ///
    /// Fixing it means publishing the DURABLE axis as a backend-level artifact refreshed off the
    /// database thread, since this function is called from render paths and must not read SQLite.
    /// That is a deliberate follow-up, not an oversight.
    ///
    /// Args:
    ///     zone: The user's selected display zone.
    ///
    /// Returns:
    ///     The axis to render and to build period bounds on.
    pub(crate) fn report_axis(&self, zone: chrono_tz::Tz) -> moon_core::db::ReportAxis {
        let store = self.session.store();
        let measured = self
            .config
            .servers
            .iter()
            .filter_map(|server| {
                let core = store.core(server.id)?;
                let offset_secs = core.time_offset.offset_secs?;
                Some((
                    server.uid,
                    vec![moon_core::db::OffsetSegment {
                        from_utc: core.time_offset.observed_at_utc.div_euclid(1_000),
                        offset_secs,
                    }],
                ))
            })
            .collect();
        moon_core::db::ReportAxis::from_measured(measured, zone)
    }

    /// Activate the valuation mode a Settings save just committed.
    ///
    /// The value itself is already in `config`, written by the save. Two things do not follow from
    /// that on their own:
    ///
    /// * the worker only fetches current rates while the mode demands them, so this activation
    ///   point sets the demand flag rather than leaving it to each view;
    /// * every open surface reads the mode without polling it. They observe the report revision,
    ///   which nothing else would move here — a mode switch changes no rows, so neither generation
    ///   advances. Without this wake they would keep rendering the previous mode's numbers under
    ///   the new mode's label until some unrelated data change happened along.
    ///
    /// Args:
    ///     cx: Backend context used to publish the revision the other surfaces observe.
    pub(crate) fn apply_valuation_mode(&mut self, cx: &mut Context<Self>) {
        if let Some(valuation) = &self.valuation {
            valuation.set_current_wanted(self.valuation_mode() == ValuationMode::Current);
        }
        self.report_revision.update(cx, |_, cx| cx.notify());
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
                        // Aged to NOW, exactly as the panel classifies it: judging the raw stored
                        // count here would let a key expire unnoticed on a core that went down, and
                        // would disagree with the number the operator is reading. An unanswered
                        // check and a key with no expiry both arrive as `None` and stay silent.
                        api_days: core
                            .api_expiry
                            .and_then(|expiry| expiry.days_left_at(now_ms)),
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
        //
        // Quiet mode silences the SOUND only, and only for the axes it is not told to let through:
        // the episode above is already recorded, so the warning list, the badges and the charts
        // still show every night the operator slept through.
        for axis in &result.opened {
            if !self.quiet_allows_warn(*axis) {
                continue;
            }
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
                    self.core_chart_hist.record(ip, sec, (ring.cpu, ring.mem))
                }
                crate::backend::core_warn::RingSubject::Core(id) => self.core_line_hist.record(
                    id,
                    sec,
                    crate::backend::server_chart::CoreMetrics {
                        cpu: ring.cpu,
                        mem: ring.mem,
                        ping: ring.ping,
                        exch: ring.exch,
                    },
                ),
            }
        }
        // Per-server ping history (client↔core and core→exchange), recorded backend-always like the
        // CPU/memory rings.
        for (ip, link, exch) in &result.pings {
            self.server_ping_hist.record(*ip, sec, *link);
            self.server_exch_hist.record(*ip, sec, *exch);
        }
        // Retention prune: drop the graph slices of episodes older than 30 days (the episode rows
        // themselves stay forever), so the per-core blobs cannot grow the file without bound. Repeated
        // once a day rather than only at startup, so a session outliving the retention window keeps
        // trimming — `warn_last_prune_ms == 0` fires it on the first tick.
        if now_ms - self.warn_last_prune_ms >= WARN_PRUNE_INTERVAL_MS {
            self.warn_last_prune_ms = now_ms;
            if let Some(store) = self.warn_store.as_ref() {
                if let Err(err) = store.prune_slices(now_ms - WARN_SLICE_RETENTION_MS) {
                    log::warn!("core warning slice prune failed: {err}");
                }
            }
        }
        // Persist each closed episode plus its full topology (collect the follow-up re-captures into a
        // local first, so the per-episode `&self` capture calls don't clash with the queue push).
        let mut pending: Vec<PendingWarnSlice> = Vec::new();
        for episode in &result.closed {
            // An axis turned off mid-episode closes its open warning on this tick; honor
            // "off = not persisted" by dropping it rather than writing it to the log.
            if !enabled.allows(episode.axis) {
                continue;
            }
            let rowid = match self.warn_store.as_ref().map(|s| s.insert_episode(episode)) {
                Some(Ok(rowid)) => rowid,
                Some(Err(err)) => {
                    log::warn!("core warning persist failed: {err}");
                    continue;
                }
                None => continue,
            };
            // Graph slices exist to fill a chart badge's hover card. An axis with no per-second
            // series would write one blob per core for a card that can never have content, so it
            // keeps the episode row and skips the slices.
            //
            // Structural, NOT `warn_chart`: that folds in the user's "show on chart" checkbox, and
            // gating PERSISTENCE on a display toggle would silently stop recording slices for CPU
            // the moment someone unticks it — and leave those episodes graph-less forever after.
            if !axis_has_series(episode.axis) {
                continue;
            }
            // The roster to record per-core slices for: every core on the server, or the episode's
            // own core when it has no known endpoint.
            let ip = episode.server_ip;
            let roster = match ip {
                Some(ip) => self.cores_on_ip(ip),
                None => episode.core_id.into_iter().collect(),
            };
            // Capture immediately with whatever window exists now, so a shutdown before the forward
            // tail fills still leaves a (partial) graph on disk.
            self.capture_episode_topology(rowid, ip, &roster, episode.start_ms, now_ms);
            // If the forward tail has not accrued yet, re-capture the full window later; OR REPLACE
            // overwrites the partial with the complete slice.
            let capture_at_ms = episode.start_ms + WARN_SLICE_FWD_MS;
            if capture_at_ms > now_ms {
                pending.push(PendingWarnSlice {
                    episode_id: rowid,
                    ip,
                    roster,
                    start_ms: episode.start_ms,
                    capture_at_ms,
                });
            }
        }
        for item in pending {
            push_pending_slice(&mut self.warn_pending_slices, item);
        }
        self.drain_pending_slices(now_ms);
    }

    /// Capture one episode's full topology (server graph + every core in `roster`) from the current
    /// rings into `warn_store`. A no-op when persistence is off.
    fn capture_episode_topology(
        &self,
        episode_id: i64,
        ip: Option<IpAddr>,
        roster: &[CoreId],
        start_ms: i64,
        now_ms: i64,
    ) {
        let Some(store) = self.warn_store.as_ref() else {
            return;
        };
        let cores: Vec<_> = roster
            .iter()
            .map(|id| (*id, self.core_line_hist.ring(*id)))
            .collect();
        capture_topology(
            store,
            ip.and_then(|ip| self.core_chart_hist.ring(ip)),
            ip.and_then(|ip| self.server_ping_hist.ring(ip)),
            ip.and_then(|ip| self.server_exch_hist.ring(ip)),
            &cores,
            episode_id,
            start_ms,
            now_ms,
        );
    }

    /// Core ids whose configured endpoint resolves to `ip` (the server's roster this second).
    fn cores_on_ip(&self, ip: IpAddr) -> Vec<CoreId> {
        let store = self.session.store();
        self.session
            .sessions()
            .iter()
            .filter(|session| {
                store
                    .core(session.id)
                    .and_then(|core| core.endpoint)
                    .map(|endpoint| endpoint.address)
                    == Some(ip)
            })
            .map(|session| session.id)
            .collect()
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
        if self.warn_store.is_none() {
            self.warn_pending_slices.clear();
            return;
        }
        // Take the ready captures out of the queue first (in insertion order), THEN capture: the
        // capture borrows `&self`, so it cannot run while the queue is being mutated.
        let mut ready: Vec<PendingWarnSlice> = Vec::new();
        let mut i = 0;
        while i < self.warn_pending_slices.len() {
            if self.warn_pending_slices[i].capture_at_ms > now_ms {
                i += 1;
                continue;
            }
            // `remove` (not `swap_remove`) keeps insertion order so the cap's oldest-first eviction
            // stays meaningful; the queue is tiny, so the shift is cheap.
            ready.push(self.warn_pending_slices.remove(i));
        }
        for pending in ready {
            self.capture_episode_topology(
                pending.episode_id,
                pending.ip,
                &pending.roster,
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
            .series_for_episode(episode_id as i64, 0, "server")
            .ok()
            .flatten()
    }

    /// The persisted client↔core ping slice for a closed episode, past the live ring. `None` if it
    /// was never captured or persistence is off.
    pub(crate) fn warn_ping_series_slice(&self, episode_id: u64) -> Option<Vec<u16>> {
        self.warn_store
            .as_ref()?
            .ping_series_for_episode(episode_id as i64, 0, "ping")
            .ok()
            .flatten()
    }

    /// The persisted core→exchange ping slice for a closed episode, past the live ring.
    pub(crate) fn warn_exch_series_slice(&self, episode_id: u64) -> Option<Vec<u16>> {
        self.warn_store
            .as_ref()?
            .ping_series_for_episode(episode_id as i64, 0, "exch")
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
            api: axes.api,
        }
    }

    /// The engine's numeric detection thresholds, projected from the persisted per-axis params.
    /// Latency percents (`yellow`/`red` as +N %) become the ratio ×100 the engine consumes.
    fn warn_tuning(&self) -> crate::backend::core_warn::WarnTuning {
        let p = &self.layout.warn_params;
        // Baseline multiplier ×100 (config stores ×N, e.g. 2 → ×2 → 200). Clamp yellow to red so a
        // mis-set yellow > red can't make the yellow band unreachable (severity checks red first).
        let ping_red = u32::from(p.ping.red) * 100;
        let exch_red = u32::from(p.exch.red) * 100;
        crate::backend::core_warn::WarnTuning {
            cpu_pct: u32::from(p.cpu.pct),
            // Hold clamped to ≥1: a hand-edited 0 would otherwise make `next >= hold` true even on a
            // non-critical second (counter resets to 0), warning permanently.
            cpu_hold: u32::from(p.cpu.hold).max(1),
            mem_pct: u32::from(p.mem.pct),
            mem_window: i64::from(p.mem.window),
            ping_yellow_num: (u32::from(p.ping.yellow) * 100).min(ping_red),
            ping_red_num: ping_red,
            ping_window: i64::from(p.ping.window),
            ping_hold: u32::from(p.ping.hold).max(1),
            exch_yellow_num: (u32::from(p.exch.yellow) * 100).min(exch_red),
            exch_red_num: exch_red,
            exch_window: i64::from(p.exch.window),
            exch_hold: u32::from(p.exch.hold).max(1),
            // A `days` of 0 is meaningful here (warn only on the key's last day), so it is NOT
            // floored like the sustain counters above. The ceiling is the popup's own range: a
            // hand-edited `layout.toml` asking for 60 000 days would warn on every dated key.
            api_days: i32::from(p.api.days.min(moon_core::config::layout::API_WARN_MAX_DAYS)),
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
            WarnAxis::ApiExpiry => &p.api.sound,
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
            // Never on a chart: an expiring key has no per-second history, so its badge would open
            // a card with an empty graph. It lives in Core Status and the Warnings list only.
            WarnAxis::ApiExpiry => false,
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

    /// Warning episodes for one effective workspace scope, filtered before the persisted limit.
    ///
    /// Open episodes are filtered in memory. Persisted episodes are filtered in SQLite before its
    /// `ORDER BY` and `LIMIT`, so unrelated cores cannot crowd the selected workspace out.
    ///
    /// Args:
    ///     core_ids: Effective core identities accepted by core-specific warning axes.
    ///     server_ips: Effective server identities accepted by server-wide warning axes.
    ///     limit: Maximum rows to return after merging open and persisted episodes.
    ///
    /// Returns:
    ///     Still-open episodes first, then closed ones newest first, capped once to `limit`.
    pub(crate) fn warn_episodes_recent_for_scope(
        &self,
        core_ids: &HashSet<CoreId>,
        server_ips: &HashSet<IpAddr>,
        limit: usize,
    ) -> Vec<crate::backend::core_warn::WarnEpisode> {
        let mut all = self.warn.open_episodes();
        if let Some(store) = &self.warn_store
            && let Ok(closed) = store.recent_episodes_for_scope(core_ids, server_ips, limit)
        {
            all.extend(closed);
        }
        finalize_recent_warning_episodes(all, self.warn_enabled(), core_ids, server_ips, limit)
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

    /// Queue the first configured market for the render diagnostic once its owner is available.
    ///
    /// Args:
    ///     cx: Backend context used to notify diagnostic observers after the request is queued.
    ///
    /// Returns:
    ///     Nothing. With no group window it remains pending; once a window exists it either queues
    ///     the first eligible market or finishes with a diagnostic warning when none exists.
    pub(crate) fn maybe_diag_open_first_market(&mut self, cx: &mut Context<Self>) {
        if !self.diag_open_first_market
            || self.diag_open_done
            || self.open_main_request.is_pending()
        {
            return;
        }
        if self.group_windows.is_empty() {
            return;
        }

        let candidate = self.config.servers.iter().find_map(|server| {
            let market = server.market.trim();
            (self
                .workspace_core_availability(&server.group, server.id)
                .is_available()
                && !market.is_empty()
                && self.group_windows.contains_key(&server.group))
            .then(|| (server.id, market.to_string()))
        });

        let Some((core, market)) = candidate else {
            self.diag_open_done = true;
            log::warn!("diag auto-open: no available server with default market");
            return;
        };

        self.diag_open_done = true;
        self.open_on_main((core, market.clone()), false);
        if std::env::var_os("MOON_RENDER_DIAG_PAUSE_AFTER_OPEN").is_some() {
            self.follow = false;
        }
        log::info!(
            "diag auto-open: core={} market={market}",
            moon_core::feed::core_label(core)
        );
        cx.notify();
    }

    /// Request opening `target` on its group's Main chart as one atomic identity.
    ///
    /// `activate` raises and focuses the Main window: the chart double-click and the Alerts coin
    /// click pass `true`, while table, detect, log, and screener navigation pass `false` to open
    /// the market without stealing focus from the current window.
    ///
    /// Args:
    ///     target: Live core and canonical market to address atomically.
    ///     activate: Whether ChartTabs should raise the owning group window after opening.
    ///
    /// Returns:
    ///     Nothing; a target without a live session is ignored.
    pub(crate) fn open_on_main(&mut self, target: (CoreId, String), activate: bool) {
        self.queue_open_on_main(target, ChartHistoryScope::Default, None, activate);
    }

    /// Resolve and queue one Main request with immutable optional producer authority.
    fn queue_open_on_main(
        &mut self,
        target: (CoreId, String),
        history: ChartHistoryScope,
        authority_group: Option<String>,
        activate: bool,
    ) {
        let Some(group) = self
            .session
            .sessions()
            .iter()
            .find(|session| session.id == target.0)
            .map(|session| session.group.clone())
        else {
            return;
        };
        self.open_main_request
            .request(target, history, group, authority_group, activate);
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

    /// Register interest in one strategy edit so the shell can report a non-clean outcome.
    ///
    /// The coin menu is fire-and-forget with no surface of its own; this is the only place an
    /// edit it dispatched can become visible. Self-draining: an entry is dropped as soon as the
    /// edit resolves or its core disappears.
    pub fn watch_strategy_edit(&mut self, core: CoreId, id: u64, coin: String) {
        self.strategy_edit_watches.push(PendingStrategyEditWatch {
            core,
            id,
            coin,
            sent: false,
            timed_out_shown: false,
        });
    }

    /// Drain queued coin-menu strategy-edit outcomes into toast events, advancing each watched
    /// core's note cursor.
    ///
    /// The split ownership across this feature is deliberate, so nobody adds a second pump for
    /// the same signal: the params panel reads `CoreData` directly and renders in place (no
    /// toast); the Analytics window owns its own banner and toast because `push_notification`
    /// targets the ACTIVE window and Analytics is a separate one; this queue is the only source
    /// for the shell's main-window toast, drained by `Shell::drain_strategy_edit_toasts`.
    pub(crate) fn take_strategy_edit_toasts(&mut self) -> Vec<StrategyEditToast> {
        let mut events = Vec::new();

        for watch in &mut self.strategy_edit_watches {
            if !watch.sent {
                watch.sent = true;
                events.push(StrategyEditToast::Sent {
                    coin: watch.coin.clone(),
                });
            }
        }

        let watches = std::mem::take(&mut self.strategy_edit_watches);

        // Group the surviving watches by core so every watch on a core sees the SAME batch of
        // unseen notes -- one watch's scan must never advance another watch's core cursor past a
        // note that watch has not had the chance to consume yet.
        let mut by_core: HashMap<CoreId, Vec<PendingStrategyEditWatch>> = HashMap::new();
        for watch in watches {
            if self.session.store().core(watch.core).is_none() {
                continue; // core disappeared; drop silently
            }
            by_core.entry(watch.core).or_default().push(watch);
        }

        let mut kept = Vec::new();
        for (core, core_watches) in by_core {
            let cursor = *self.strategy_edit_note_cursor.get(&core).unwrap_or(&0);
            let mut batch_max = cursor;
            // `store()` is re-fetched per core rather than hoisted: different cores need
            // different `CoreData`, and the immutable borrow must end before the cursor map (a
            // different field) is written below.
            if let Some(core_data) = self.session.store().core(core) {
                for note in core_data.strategy_edit_notes_since(cursor) {
                    batch_max = batch_max.max(note.seq);
                }
            }
            self.strategy_edit_note_cursor.insert(core, batch_max);

            for mut watch in core_watches {
                let outcome = self
                    .session
                    .store()
                    .core(watch.core)
                    .map(|cd| cd.resolve_strategy_edit(watch.id, cursor))
                    .unwrap_or(StrategyEditOutcome::Pending);
                match outcome {
                    StrategyEditOutcome::Resolved(StrategyEditResult::Confirmed) => continue,
                    StrategyEditOutcome::Resolved(StrategyEditResult::Adjusted) => {
                        events.push(StrategyEditToast::Adjusted {
                            coin: watch.coin.clone(),
                        });
                        continue; // resolved; drop the watch
                    }
                    StrategyEditOutcome::Resolved(StrategyEditResult::Superseded) => {
                        events.push(StrategyEditToast::Superseded {
                            coin: watch.coin.clone(),
                        });
                        continue; // resolved; drop the watch
                    }
                    StrategyEditOutcome::TimedOut => {
                        if !watch.timed_out_shown {
                            watch.timed_out_shown = true;
                            events.push(StrategyEditToast::TimedOut {
                                coin: watch.coin.clone(),
                            });
                        }
                    }
                    StrategyEditOutcome::Pending => {}
                }
                kept.push(watch);
            }
        }
        self.strategy_edit_watches = kept;

        events
    }
}

/// One coin-menu strategy edit awaiting resolution, so [`Backend::take_strategy_edit_toasts`]
/// can still report it once resolved. Self-draining: dropped as soon as a resolved note for its
/// id arrives, or its core disappears from the store.
pub(crate) struct PendingStrategyEditWatch {
    core: CoreId,
    id: u64,
    coin: String,
    /// Whether the "sent" toast has already been queued for this watch.
    sent: bool,
    /// Whether the one-shot `TimedOut` warning has already been queued. `TimedOut` is not
    /// terminal upstream -- a late core echo can still confirm, adjust or supersede it -- so the
    /// watch stays open after this fires instead of being dropped.
    timed_out_shown: bool,
}

/// One coin-menu strategy-edit outcome ready to become a toast. `Backend` never constructs
/// `MoonNotification` or touches a window -- that stays in `Shell::drain_strategy_edit_toasts`,
/// the only place with window access.
pub(crate) enum StrategyEditToast {
    Sent { coin: String },
    Adjusted { coin: String },
    Superseded { coin: String },
    TimedOut { coin: String },
}
