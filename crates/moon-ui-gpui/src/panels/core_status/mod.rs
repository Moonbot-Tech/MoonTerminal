//! Core Status panel: connection state and typed protocol-v4 resource telemetry
//! (`Event::KernelHealth`) for every core in scope.
//!
//! `CoreData::sys` holds the latest sample, while `sys_rev` invalidates the panel
//! when metric values or the decoded endpoint change.
//!
//! Like the Assets panel, it is scoped to a window group and can live in a dock
//! tab or a detached window. [`crate::persistence::table_persist`] stores separate column
//! widths for `:dock` and `:win`. This module owns data and lifecycle; [`server_view`]
//! and [`table`] own the two presentations.

mod chart;
mod model;
mod presentation;
mod server_view;
mod table;
#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    DockArea, MoonDataTableState, MoonInputEvent, MoonInputState, MoonPalette, MoonSegmentItem,
    MoonSegmentedControl, MoonTreeState, Panel, PanelEvent, PanelState, h_flex, v_flex,
};

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use moon_core::feed::ConnStatus;
use moon_core::session::{CoreId, CoreSysStatus};
use rust_i18n::t;

use std::collections::VecDeque;

use model::{CoreStatusRow, ServerKey, ServerStatusGroup, aggregate_servers};

/// CPU is averaged over this many recent one-second buckets before display.
const CPU_WINDOW_SECS: i64 = 3;
/// Memory-growth is judged against the minimum used within this many recent seconds.
const MEM_WINDOW_SECS: i64 = 30;
/// A memory rise of at least this many MB above the window minimum flags growth.
const MEM_GROWTH_MB: u16 = 64;
/// ...or a rise of at least this percent above the window minimum.
const MEM_GROWTH_PCT: u32 = 12;
/// Machine CPU at or above this percent (averaged) counts toward the sustained-CPU warning.
const WARN_CPU_PCT: u32 = 70;
/// The CPU warning fires only after the machine has stayed high this many consecutive seconds.
/// This is what separates a warning (SUSTAINED) from the number color (an instantaneous threshold).
const CPU_SUSTAIN_SECS: u32 = 5;
/// Chart X-axis span selectable in the detached window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChartWindow {
    /// Last five minutes.
    Min5,
    /// Last hour.
    Hour1,
}

impl ChartWindow {
    /// Number of seconds (and points at 1 Hz) the window spans.
    fn secs(self) -> usize {
        match self {
            Self::Min5 => 300,
            Self::Hour1 => 3600,
        }
    }

    /// Localization key for the span label (`5 мин` / `1 ч`).
    fn label_key(self) -> &'static str {
        match self {
            Self::Min5 => "core_status.chart_5m",
            Self::Hour1 => "core_status.chart_1h",
        }
    }
}

impl Default for ChartWindow {
    fn default() -> Self {
        Self::Min5
    }
}

/// One second of CPU samples, tagged with its second so a reused ring slot resets cleanly.
#[derive(Default, Clone, Copy)]
struct CpuBucket {
    /// Unix second this bucket accumulates.
    sec: i64,
    /// Summed process CPU percentages this second.
    proc_sum: u32,
    /// Summed whole-machine CPU percentages this second.
    sys_sum: u32,
    /// Number of samples in the sums.
    samples: u32,
}

/// Per-core rolling telemetry for CPU averaging, memory-growth detection, and sustained CPU.
#[derive(Default, Clone)]
struct CoreTrack {
    /// Three one-second CPU buckets, indexed by `second % 3`.
    cpu: [CpuBucket; 3],
    /// One `(second, used MB)` sample per second over the memory-growth window.
    mem: VecDeque<(i64, u16)>,
    /// Consecutive seconds the averaged machine CPU has stayed at/above `WARN_CPU_PCT`.
    cpu_high_secs: u32,
}

impl CoreTrack {
    /// Average `(process, system)` CPU over the last `CPU_WINDOW_SECS` seconds.
    ///
    /// Args:
    ///     now_sec: Current Unix second, bounding which buckets count.
    ///
    /// Returns:
    ///     Averaged process and system CPU, each `None` when no bucket has samples.
    fn averaged(&self, now_sec: i64) -> (Option<u8>, Option<u8>) {
        let (mut proc, mut system, mut n) = (0u32, 0u32, 0u32);
        for bucket in &self.cpu {
            if bucket.samples > 0 && now_sec - bucket.sec < CPU_WINDOW_SECS {
                proc += bucket.proc_sum;
                system += bucket.sys_sum;
                n += bucket.samples;
            }
        }
        if n == 0 {
            (None, None)
        } else {
            (
                Some((proc / n).min(255) as u8),
                Some((system / n).min(255) as u8),
            )
        }
    }
}

/// Whether a memory window shows sustained growth: the latest sample sits notably above the
/// window minimum.
///
/// A flat footprint keeps the minimum near the current value (no warning); a leak lifts the
/// current well above the window low; a spike that returns leaves the minimum low, so it does not
/// flag. Needs a few samples before it will fire.
///
/// Args:
///     mem: `(second, used MB)` samples over the growth window, oldest first.
///
/// Returns:
///     True when the rise above the window minimum clears both the absolute-MB and percent floors.
fn mem_grew(mem: &VecDeque<(i64, u16)>) -> bool {
    if mem.len() < 5 {
        return false;
    }
    let current = mem.back().map(|(_, used)| *used).unwrap_or(0);
    let min = mem.iter().map(|(_, used)| *used).min().unwrap_or(current);
    let growth = u32::from(current.saturating_sub(min));
    growth >= u32::from(MEM_GROWTH_MB).max(u32::from(min) * MEM_GROWTH_PCT / 100)
}

/// Advance the consecutive-high-seconds counter for the sustained-CPU warning.
///
/// Args:
///     prev: Prior consecutive-high seconds.
///     system_cpu: Averaged machine CPU for the just-elapsed second, if known.
///
/// Returns:
///     One more than `prev` while CPU stays at/above the threshold, else zero.
fn next_high_secs(prev: u32, system_cpu: Option<u8>) -> u32 {
    match system_cpu {
        Some(value) if u32::from(value) >= WARN_CPU_PCT => prev.saturating_add(1),
        _ => 0,
    }
}

/// Available Core Status presentations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreStatusMode {
    /// Expandable server rows grouped by endpoint address.
    ByIp,
    /// Existing one-row-per-core telemetry table.
    Flat,
}

impl Default for CoreStatusMode {
    /// Return the server-by-IP presentation used on every new panel instance.
    fn default() -> Self {
        Self::ByIp
    }
}

/// Group-scoped Core Status panel for a dock tab or detached window.
pub struct CoreStatusView {
    pub(super) backend: Entity<Backend>,
    /// Window group whose cores define this panel's scope, matching the Assets panel.
    group: String,
    /// Multi-select core filter; an empty set means every core in the group.
    pub(super) sel_cores: HashSet<CoreId>,
    /// Per-core rolling CPU and memory history for averaging and growth detection.
    track: HashMap<CoreId, CoreTrack>,
    /// Unix ms of the last repaint; telemetry repaints at most once per second.
    last_repaint_ms: i64,
    /// Whether any server currently has a warning; drives the dock-tab badge.
    has_warn: bool,
    /// Whether this instance is a detached window (vs a dock tab). Only a window renders the chart.
    detached: bool,
    /// Selected chart X-axis span in the detached window.
    chart_window: ChartWindow,
    /// Server whose chart the detached window shows; clicking a server row selects it. `None` falls
    /// back to the first server.
    chart_server: Option<ServerKey>,
    cached_rows: Rc<Vec<CoreStatusRow>>,
    cached_groups: Rc<Vec<ServerStatusGroup>>,
    /// Servers whose IP is momentarily revealed by the eye control. Transient: cleared on blur,
    /// never persisted, so IPs return to masked when focus leaves the panel.
    revealed_ips: HashSet<ServerKey>,
    /// Server whose name is being renamed inline, if any.
    editing: Option<ServerKey>,
    /// Input state backing the inline rename field while [`Self::editing`] is set.
    edit_input: Option<Entity<MoonInputState>>,
    /// Active flat-table sort as `(column key, ascending)`, or `None` for the default
    /// attention-first order.
    flat_sort: Option<(String, bool)>,
    mode: CoreStatusMode,
    tree_state: Entity<MoonTreeState>,
    table_state: Entity<MoonDataTableState>,
    /// Context-qualified column-width persistence ID (`core-status-table:dock` or `:win`).
    widths_id: String,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl CoreStatusView {
    /// Construct a group-scoped Core Status panel and its table/tree state.
    ///
    /// Args:
    ///     backend: Shared terminal backend.
    ///     group: Window group that defines the core scope.
    ///     detached: Whether column widths use the detached-window persistence key.
    ///     _window: Host window reserved for panel construction symmetry.
    ///     cx: View context used for observers and child entities.
    ///
    /// Returns:
    ///     A panel whose default presentation is server-by-IP.
    fn new(
        backend: Entity<Backend>,
        group: String,
        detached: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // This fires on every backend notify (event-driven, ≤4 Hz — not a timer/poll), but ALL work
        // is gated to once per second: sample, average, warn-tick, rebuild, record, repaint. Faster
        // drains do nothing but a timestamp check, so the panel never adds ~4 Hz churn.
        cx.observe(&backend, |this, backend, cx| {
            let now = moon_chart::paint::now_unix_ms() as i64;
            if now - this.last_repaint_ms < 1000 {
                return;
            }
            this.last_repaint_ms = now;
            this.accumulate_track(backend.read(cx), now / 1000);
            this.tick_warn_counters(now / 1000);
            this.rebuild_cache(cx);
            this.record_chart_history(cx);
            cx.notify();
        })
        .detach();

        let widths_id = crate::persistence::table_persist::ctx_id("core-status-table", detached);
        let saved_widths = crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            s
        });
        cx.observe(&table_state, |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();
        let tree_state = cx.new(|cx| MoonTreeState::new(cx));
        let focus = cx.focus_handle();

        // A revealed IP is momentary: when focus leaves the panel it re-masks. The eye control
        // focuses this handle on reveal, so this blur fires when the user clicks away.
        cx.on_blur(&focus, window, |this, _window, cx| {
            if !this.revealed_ips.is_empty() {
                this.revealed_ips.clear();
                cx.notify();
            }
        })
        .detach();

        let mut this = Self {
            backend,
            group,
            sel_cores: HashSet::new(),
            track: HashMap::new(),
            last_repaint_ms: 0,
            has_warn: false,
            detached,
            chart_window: ChartWindow::default(),
            chart_server: None,
            cached_rows: Rc::new(Vec::new()),
            cached_groups: Rc::new(Vec::new()),
            revealed_ips: HashSet::new(),
            editing: None,
            edit_input: None,
            flat_sort: None,
            mode: CoreStatusMode::default(),
            tree_state,
            table_state,
            widths_id,
            dock: None,
            focus,
        };
        this.rebuild_cache(cx);
        this
    }

    /// Reconstruct a dock tab from `docks.json` using the `:dock` width context.
    pub fn restored_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, group, false, window, cx)
    }

    /// Build detached-window content, framed by `DetachedWindow`, using the `:win` width context.
    pub fn detached_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, group, true, window, cx)
    }

    /// Return the table state used by the detached window's auto-width reset button.
    pub fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    /// Return this panel group's cores in canonical order.
    pub(super) fn scope_cores(&self, b: &Backend) -> OrderedCores {
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| s.group == self.group)
    }

    /// Sample every scoped core's current CPU and memory into its rolling window.
    ///
    /// Runs on every backend drain (faster than the repaint). CPU lands in the current one-second
    /// bucket; memory keeps one sample per second over the growth window.
    ///
    /// Args:
    ///     b: Backend snapshot whose scoped cores are sampled.
    ///     now_sec: Current Unix second.
    ///
    /// Returns:
    ///     Nothing; per-core history grows in place and self-prunes to its windows.
    fn accumulate_track(&mut self, b: &Backend, now_sec: i64) {
        let cores = self.scope_cores(b);
        let store = b.session.store();
        for (id, _) in cores {
            let Some(core) = store.core(id) else {
                continue;
            };
            let sys = core.sys;
            if sys.updated_ms == 0 {
                continue;
            }
            let track = self.track.entry(id).or_default();
            let slot = &mut track.cpu[now_sec.rem_euclid(3) as usize];
            if slot.sec != now_sec {
                *slot = CpuBucket {
                    sec: now_sec,
                    ..Default::default()
                };
            }
            if let Some(value) = sys.process_cpu_percent {
                slot.proc_sum += u32::from(value);
            }
            if let Some(value) = sys.system_cpu_percent {
                slot.sys_sum += u32::from(value);
            }
            slot.samples += 1;

            if let Some(used) = sys.used_memory_mb {
                if track.mem.back().map(|(sec, _)| *sec) == Some(now_sec) {
                    track.mem.back_mut().unwrap().1 = used;
                } else {
                    track.mem.push_back((now_sec, used));
                    while track
                        .mem
                        .front()
                        .is_some_and(|(sec, _)| now_sec - sec > MEM_WINDOW_SECS)
                    {
                        track.mem.pop_front();
                    }
                }
            }
        }
    }

    /// Replace a core's CPU fields with the average over the last few seconds.
    ///
    /// Args:
    ///     id: Core whose window to apply.
    ///     sys: Latest store telemetry to base other fields on.
    ///     now_sec: Current Unix second, bounding which buckets count.
    ///
    /// Returns:
    ///     Telemetry with CPU averaged over the window, or the input unchanged when no sample exists.
    fn averaged_sys(&self, id: CoreId, mut sys: CoreSysStatus, now_sec: i64) -> CoreSysStatus {
        if let Some(track) = self.track.get(&id) {
            let (proc, system) = track.averaged(now_sec);
            if let Some(proc) = proc {
                sys.process_cpu_percent = Some(proc);
            }
            if let Some(system) = system {
                sys.system_cpu_percent = Some(system);
            }
        }
        sys
    }

    /// Advance every core's sustained-high-CPU counter for the just-elapsed second.
    ///
    /// Runs once per second (unlike the faster sampling), so a counter of N means N real seconds
    /// held high. This is the SUSTAINED signal behind the CPU warning, separate from the number
    /// color which is an instantaneous threshold.
    ///
    /// Args:
    ///     now_sec: Current Unix second, bounding the averaging window.
    ///
    /// Returns:
    ///     Nothing; each track's `cpu_high_secs` is updated in place.
    fn tick_warn_counters(&mut self, now_sec: i64) {
        for track in self.track.values_mut() {
            let system = track.averaged(now_sec).1;
            track.cpu_high_secs = next_high_secs(track.cpu_high_secs, system);
        }
    }

    /// Record this second's RAW telemetry into the SHARED backend history, so it survives this
    /// panel and any window open/close.
    ///
    /// Two subjects are recorded per server: the whole machine (`system CPU %`, `occupied memory %`)
    /// into `core_chart_hist`, and each core (`process CPU %`, `process memory share %`) into
    /// `core_line_hist`. Runs once per second in every Core Status panel; the backend deduplicates
    /// per subject per second, so multiple panels never double-count. CPU is read raw from the store
    /// (not the table's smoothed value) so the chart shows real fluctuation.
    ///
    /// Args:
    ///     cx: View context used to read the store and update the backend.
    ///
    /// Returns:
    ///     Nothing; each subject's history grows by at most one point.
    fn record_chart_history(&mut self, cx: &mut Context<Self>) {
        let sec = self.last_repaint_ms / 1000;
        let mut server_samples: Vec<(IpAddr, u8, u8)> = Vec::new();
        let mut core_samples: Vec<(CoreId, u8, u8)> = Vec::new();
        {
            let b = self.backend.read(cx);
            let store = b.session.store();
            for group in self.cached_groups.iter() {
                let Some(ip) = group.address else {
                    continue;
                };
                // Whole-machine CPU: the FRESHEST core's raw system value. Not the smoothed table
                // value (the chart shows real fluctuation), and not the alphabetically-first core,
                // which may be stale/disconnected and record a false 0. Clamped to the 0..100 axis.
                let cpu = group
                    .cores
                    .iter()
                    .filter_map(|core| store.core(core.id).map(|data| data.sys))
                    .filter_map(|sys| sys.system_cpu_percent.map(|cpu| (sys.updated_ms, cpu)))
                    .max_by_key(|(updated_ms, _)| *updated_ms)
                    .map(|(_, cpu)| cpu)
                    .unwrap_or(0)
                    .min(100);
                let mem = occupied_pct(group.process_memory_mb, group.free_physical_memory_mb);
                server_samples.push((ip, cpu, mem));

                // Per core: raw process CPU and this process's share of the reconstructed machine
                // total (process RAM sum + free), keeping every line on the same 0..100 % axis. When
                // free memory is unknown the server mem line reads 0 (occupied_pct), so leave the
                // total unavailable here too so the core mem lines stay consistent with it instead of
                // dividing by the process sum alone and towering to ~100 %.
                let total = match group.free_physical_memory_mb {
                    Some(free) => group.process_memory_mb.unwrap_or(0) + u64::from(free),
                    None => 0,
                };
                for core in &group.cores {
                    let sys = store.core(core.id).map(|c| c.sys);
                    let proc_cpu = sys.and_then(|s| s.process_cpu_percent).unwrap_or(0).min(100);
                    let proc_mem = match sys.and_then(|s| s.used_memory_mb) {
                        Some(used) if total > 0 => (u64::from(used) * 100 / total).min(100) as u8,
                        _ => 0,
                    };
                    core_samples.push((core.id, proc_cpu, proc_mem));
                }
            }
        }
        if server_samples.is_empty() && core_samples.is_empty() {
            return;
        }
        self.backend.update(cx, |b, _| {
            for (ip, cpu, mem) in server_samples {
                b.core_chart_hist.record(ip, sec, cpu, mem);
            }
            for (id, cpu, mem) in core_samples {
                b.core_line_hist.record(id, sec, cpu, mem);
            }
        });
    }

    /// Switch the detached-window chart span and repaint.
    ///
    /// Args:
    ///     window: Requested X-axis span.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; only the span and a repaint change.
    fn set_chart_window(&mut self, window: ChartWindow, cx: &mut Context<Self>) {
        if self.chart_window != window {
            self.chart_window = window;
            cx.notify();
        }
    }

    /// Select which server's chart the detached window shows (from a server-row click).
    ///
    /// Args:
    ///     key: Clicked server identity.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; only the selection and a repaint change.
    fn select_chart_server(&mut self, key: ServerKey, cx: &mut Context<Self>) {
        if self.chart_server != Some(key) {
            self.chart_server = Some(key);
            cx.notify();
        }
    }

    /// Toggle one server's expansion from a chevron click (the headless tree does not do it).
    ///
    /// Args:
    ///     key: Server identity whose folder to expand or collapse.
    ///     cx: View context used to update the tree state and repaint.
    ///
    /// Returns:
    ///     Nothing; the tree's expanded set flips for this server.
    fn toggle_server_expand(&mut self, key: ServerKey, cx: &mut Context<Self>) {
        let id = SharedString::from(key.tree_id());
        self.tree_state.update(cx, |state, cx| {
            let mut ids = state.expanded_ids().into_iter().collect::<HashSet<_>>();
            if !ids.remove(&id) {
                ids.insert(id);
            }
            state.set_expanded(ids, cx);
        });
        cx.notify();
    }

    /// Compute a server's `(cpu_warn, mem_warn)` from sustained CPU and per-core memory growth.
    ///
    /// These are the SUSTAINED/TREND signals shown as a ⚠ next to the metric and on the tab, and
    /// are deliberately distinct from the threshold-based number colors.
    ///
    /// Args:
    ///     group: Aggregated server whose cores' tracked history is inspected.
    ///
    /// Returns:
    ///     Whether the machine CPU has held high, and whether any core's memory is growing.
    fn compute_warns(&self, group: &ServerStatusGroup) -> (bool, bool) {
        let mut cpu_warn = false;
        let mut mem_warn = false;
        for core in &group.cores {
            if let Some(track) = self.track.get(&core.id) {
                if track.cpu_high_secs >= CPU_SUSTAIN_SECS {
                    cpu_warn = true;
                }
                if mem_grew(&track.mem) {
                    mem_warn = true;
                }
            }
        }
        (cpu_warn, mem_warn)
    }

    /// Collect filtered core rows for the group scope in canonical order.
    ///
    /// Args:
    ///     b: Backend snapshot containing config, sessions, and market identity.
    ///     now_sec: Current Unix second, bounding the CPU average window.
    ///
    /// Returns:
    ///     Canonically ordered visible rows with CPU averaged over the window.
    fn collect(&self, b: &Backend, now_sec: i64) -> Vec<CoreStatusRow> {
        let store = b.session.store();
        let mut out = Vec::new();
        for (id, name) in self.scope_cores(b) {
            if !self.sel_cores.is_empty() && !self.sel_cores.contains(&id) {
                continue;
            }
            let endpoint = store.core(id).and_then(|core| core.endpoint);
            let (status, sys) = store
                .core(id)
                .map(|c| (c.status.clone(), c.sys))
                .unwrap_or((ConnStatus::Disconnected, CoreSysStatus::default()));
            out.push(CoreStatusRow {
                id,
                name,
                status,
                sys: self.averaged_sys(id, sys, now_sec),
                endpoint,
            });
        }
        out
    }

    /// Rebuild flat and grouped snapshots, tag warnings, sort, then reconcile MoonTree items.
    ///
    /// Args:
    ///     cx: View context used to read the backend and update tree state.
    ///
    /// Returns:
    ///     Nothing; all render caches are replaced atomically.
    fn rebuild_cache(&mut self, cx: &mut Context<Self>) {
        let now_sec = moon_chart::paint::now_unix_ms() as i64 / 1000;
        let backend = self.backend.clone();
        let (rows, names) = {
            let b = backend.read(cx);
            (self.collect(b, now_sec), b.layout.core_server_names.clone())
        };
        let mut groups = aggregate_servers(&rows);
        assign_server_names(&mut groups, &names);
        for group in &mut groups {
            let (cpu_warn, mem_warn) = self.compute_warns(group);
            group.cpu_warn = cpu_warn;
            group.mem_warn = mem_warn;
        }
        // Warned servers first, then by server NAME (natural order, so `Server 2` < `Server 10`
        // and custom names like `F1` sort alphabetically). No user-selectable sort.
        groups.sort_by(|a, b| {
            let (aw, bw) = (a.cpu_warn || a.mem_warn, b.cpu_warn || b.mem_warn);
            bw.cmp(&aw)
                .then_with(|| natural_cmp(&a.display_name, &b.display_name))
        });
        self.has_warn = groups.iter().any(|group| group.cpu_warn || group.mem_warn);
        self.cached_groups = Rc::new(groups);
        self.cached_rows = Rc::new(rows);
        self.rebuild_tree(cx);
    }

    /// Replace the tree items, preserving which servers the user has expanded.
    ///
    /// The cache rebuilds on every telemetry tick, so the current expansion is read back and
    /// re-applied; otherwise servers would collapse each tick. New servers stay collapsed.
    ///
    /// Args:
    ///     cx: View context used to update the MoonTree state entity.
    ///
    /// Returns:
    ///     Nothing; only tree items and their expansion change.
    fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        let expanded = self
            .tree_state
            .read(cx)
            .expanded_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        let items = server_view::tree_items(&self.cached_groups);
        self.tree_state.update(cx, |state, cx| {
            state.set_items(items, cx);
            state.set_expanded(expanded, cx);
        });
    }

    /// Order flat-mode rows: the active column sort, or the default attention-first order.
    ///
    /// Args:
    ///     rows: Current filtered core snapshots.
    ///
    /// Returns:
    ///     A sorted copy for the flat table.
    fn sorted_flat_rows(&self, rows: &[CoreStatusRow]) -> Vec<CoreStatusRow> {
        let mut out = model::ordered_flat_rows(rows);
        if let Some((key, ascending)) = &self.flat_sort {
            if key == "server" {
                // The "server" column sorts by the displayed server NAME (natural order), which
                // lives on the aggregated groups rather than the row.
                let names: HashMap<ServerKey, String> = self
                    .cached_groups
                    .iter()
                    .map(|group| (group.key, group.display_name.clone()))
                    .collect();
                let name_of = |row: &CoreStatusRow| {
                    names
                        .get(&ServerKey::for_row(row))
                        .cloned()
                        .unwrap_or_default()
                };
                out.sort_by(|a, b| {
                    let ordering = natural_cmp(&name_of(a), &name_of(b));
                    if *ascending {
                        ordering
                    } else {
                        ordering.reverse()
                    }
                });
            } else {
                out.sort_by(|a, b| {
                    let ordering = compare_flat_rows(a, b, key);
                    if *ascending {
                        ordering
                    } else {
                        ordering.reverse()
                    }
                });
            }
        }
        out
    }

    /// Apply a flat-table header sort from a column click.
    ///
    /// Args:
    ///     key: Column key from the table header.
    ///     ascending: Whether the click requested ascending order.
    ///     cx: View context used to repaint with the new order.
    ///
    /// Returns:
    ///     Nothing; only the sort state and a repaint change.
    pub(super) fn set_flat_sort(&mut self, key: &str, ascending: bool, cx: &mut Context<Self>) {
        let next = Some((key.to_string(), ascending));
        if self.flat_sort != next {
            self.flat_sort = next;
            cx.notify();
        }
    }

    /// Toggle one core in the multi-select filter, or toggle the All item.
    ///
    /// `Some(id)` toggles one core. `None` clears a selection containing every non-empty scoped
    /// core, otherwise replacing it with that full set. Stale ids do not stand in for current cores.
    ///
    /// Args:
    ///     id: Core to toggle, or `None` for the All row.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the in-memory filter and cached rows are updated in place.
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        let all: HashSet<CoreId> = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        match id {
            None => crate::controls::toggle_all_core_selection(&mut self.sel_cores, all),
            Some(id) => {
                if !self.sel_cores.remove(&id) {
                    self.sel_cores.insert(id);
                }
            }
        }
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Toggle every still-available core from one clicked exchange section.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A
    /// fully selected exchange is removed without changing selections from other exchanges.
    /// Rendered ids that left this panel's group are ignored.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from one rendered exchange section.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a changed selection rebuilds the cache once, while a stale-only batch is a
    ///     no-op.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<CoreId>,
        cx: &mut Context<Self>,
    ) {
        let available = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            self.rebuild_cache(cx);
            cx.notify();
        }
    }

    /// Toggle the momentary IP reveal for one server.
    ///
    /// Revealing focuses the panel so the [`Context::on_blur`] handler re-masks the IP when focus
    /// later leaves the panel. Hiding is immediate.
    ///
    /// Args:
    ///     key: Server identity whose IP should be shown or hidden.
    ///     window: Host window used to move focus to the panel on reveal.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; the reveal set is transient and never persisted.
    fn toggle_reveal(&mut self, key: ServerKey, window: &mut Window, cx: &mut Context<Self>) {
        if !self.revealed_ips.remove(&key) {
            self.revealed_ips.insert(key);
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Begin inline renaming of one server, seeding the field with its current display name.
    ///
    /// Args:
    ///     key: Server identity being renamed. Only address servers persist a name.
    ///     current: Current display name used as the initial field value.
    ///     window: Host window used to build and focus the input state.
    ///     cx: View context used to create the input entity and subscribe to its commit events.
    ///
    /// Returns:
    ///     Nothing; the input entity and its subscription live until the edit commits or cancels.
    fn start_rename(
        &mut self,
        key: ServerKey,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(current));
        cx.subscribe(&state, move |this, state, event: &MoonInputEvent, cx| {
            if matches!(
                event,
                MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                let value = state.read(cx).value().to_string();
                this.commit_rename(key, value, cx);
            }
        })
        .detach();
        self.edit_input = Some(state);
        self.editing = Some(key);
        cx.notify();
    }

    /// Commit or clear a server's custom name, then re-resolve display names.
    ///
    /// An empty name removes the custom entry so the default `Server N` ordinal returns. Only
    /// address servers persist; an unknown-endpoint edit simply closes.
    ///
    /// Args:
    ///     key: Server identity whose name is being committed.
    ///     text: New name from the input; trimmed, with empty meaning "reset to default".
    ///     cx: View context used to update the backend layout and rebuild caches.
    ///
    /// Returns:
    ///     Nothing; a persisted change marks the layout dirty for the shared save path.
    fn commit_rename(&mut self, key: ServerKey, text: String, cx: &mut Context<Self>) {
        if self.editing != Some(key) {
            return;
        }
        if let ServerKey::Address(address) = key {
            let ip = address.to_string();
            let text = text.trim().to_string();
            self.backend.update(cx, |b, _| {
                let changed = if text.is_empty() {
                    b.layout.core_server_names.remove(&ip).is_some()
                } else if b.layout.core_server_names.get(&ip) != Some(&text) {
                    b.layout.core_server_names.insert(ip, text);
                    true
                } else {
                    false
                };
                if changed {
                    b.layout_dirty = true;
                }
            });
        }
        self.editing = None;
        self.edit_input = None;
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Switch between grouped and flat presentations.
    ///
    /// Args:
    ///     mode: Requested presentation.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; data caches and visibility state are retained.
    fn set_mode(&mut self, mode: CoreStatusMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            cx.notify();
        }
    }
}

/// Fill each group's display name from a custom name or a stable `Server N` ordinal.
///
/// Ordinals rank address servers by sorted address so a name stays put under attention-first
/// reordering. Unknown-endpoint servers keep a core-qualified fallback label.
///
/// Args:
///     groups: Aggregated server snapshots to name in place.
///     names: Custom names keyed by endpoint IP string.
///
/// Returns:
///     Nothing; `display_name` is set on every group.
fn assign_server_names(groups: &mut [ServerStatusGroup], names: &HashMap<String, String>) {
    let mut addresses = groups
        .iter()
        .filter_map(|group| group.address)
        .collect::<Vec<IpAddr>>();
    addresses.sort();
    for group in groups.iter_mut() {
        group.display_name = match group.address {
            Some(address) => {
                let ip = address.to_string();
                names.get(&ip).cloned().unwrap_or_else(|| {
                    let ordinal = addresses
                        .iter()
                        .position(|candidate| *candidate == address)
                        .map(|index| index + 1)
                        .unwrap_or(0);
                    t!("core_status.server_n", n = ordinal).to_string()
                })
            }
            None => {
                let core = group
                    .cores
                    .first()
                    .map(|core| core.name.as_str())
                    .unwrap_or("-");
                t!("core_status.unknown_server", core = core).to_string()
            }
        };
    }
}

/// Return a stable ordinal for sorting the flat table's status column.
///
/// Args:
///     s: Current connection status.
///
/// Returns:
///     A rank placing ready cores first and failed cores last.
fn status_ord(s: &ConnStatus) -> u64 {
    match s {
        ConnStatus::Ready => 0,
        ConnStatus::Connecting | ConnStatus::Stage(_) => 1,
        ConnStatus::Disconnected => 2,
        ConnStatus::Failed(_) => 3,
    }
}

/// Occupied memory as a whole-percent share of the reconstructed machine total (process sum + free).
///
/// Args:
///     process_mem_mb: Sum of per-process resident memory, decimal MB.
///     free_mb: Free physical memory, decimal MB.
///
/// Returns:
///     Occupied percent (0..100), or 0 until both fields have arrived.
fn occupied_pct(process_mem_mb: Option<u64>, free_mb: Option<u16>) -> u8 {
    let (Some(process), Some(free)) = (process_mem_mb, free_mb) else {
        return 0;
    };
    let total = process + u64::from(free);
    if total == 0 {
        0
    } else {
        (process * 100 / total).min(100) as u8
    }
}

/// Compare two names in natural order: digit runs compare as numbers, everything else
/// case-insensitively. So `Server 2` sorts before `Server 10`, and `F1` before `Server 1`.
///
/// Args:
///     a: First name.
///     b: Second name.
///
/// Returns:
///     The natural ordering of the two names.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                // Compare digit runs numerically without parsing: strip leading zeros, then longer
                // run wins, then lexically.
                let da = take_digits(&mut a);
                let db = take_digits(&mut b);
                let na = da.trim_start_matches('0');
                let nb = db.trim_start_matches('0');
                match na.len().cmp(&nb.len()).then_with(|| na.cmp(nb)) {
                    Ordering::Equal => {}
                    ord => return ord,
                }
            }
            (Some(ca), Some(cb)) => match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                Ordering::Equal => {
                    a.next();
                    b.next();
                }
                ord => return ord,
            },
        }
    }
}

/// Consume and return a run of consecutive ASCII digits from a peekable char iterator.
fn take_digits<I: Iterator<Item = char>>(it: &mut std::iter::Peekable<I>) -> String {
    let mut digits = String::new();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            digits.push(c);
            it.next();
        } else {
            break;
        }
    }
    digits
}

/// Compare two flat rows on one column key for header-click sorting.
///
/// `None` metrics sort before any value, so cores that have not reported a field group together.
///
/// Args:
///     a: First row.
///     b: Second row.
///     key: Column key from the table header.
///
/// Returns:
///     The ascending ordering for that column.
fn compare_flat_rows(a: &CoreStatusRow, b: &CoreStatusRow, key: &str) -> Ordering {
    match key {
        // "server" is handled by name in `sorted_flat_rows`, not here.
        "status" => status_ord(&a.status).cmp(&status_ord(&b.status)),
        "cpu_proc" => a.sys.process_cpu_percent.cmp(&b.sys.process_cpu_percent),
        "cpu_sys" => a.sys.system_cpu_percent.cmp(&b.sys.system_cpu_percent),
        "mem_used" => a.sys.used_memory_mb.cmp(&b.sys.used_memory_mb),
        "free_phys" => a
            .sys
            .free_physical_memory_mb
            .cmp(&b.sys.free_physical_memory_mb),
        "cpus" => a.sys.logical_cpu_count.cmp(&b.sys.logical_cpu_count),
        // "core" and any unknown key sort by name.
        _ => a.name.cmp(&b.name),
    }
}

impl EventEmitter<PanelEvent> for CoreStatusView {}
impl Focusable for CoreStatusView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for CoreStatusView {
    fn panel_name(&self) -> &'static str {
        "CoreStatus"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    /// Draw a warning triangle on the dock tab (right of the label) while any server warns, like
    /// the News tab's unread badge. It clears itself when every warning resolves.
    fn title_suffix(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = MoonPalette::active(cx);
        self.has_warn.then(|| {
            svg()
                .path("icons/triangle-alert.svg")
                .size(px(13.0))
                .flex_none()
                .text_color(rgb(p.amber))
                .into_any_element()
        })
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::persistence::dock_persist::panel_state_with_group("CoreStatus", &self.group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        Some(vec![crate::persistence::table_persist::reset_button(
            "core-status-reset-widths",
            &self.table_state,
        )])
    }
}

impl Render for CoreStatusView {
    /// Render the active presentation with shared filters and counters.
    ///
    /// Args:
    ///     _window: Host window reserved for renderer symmetry.
    ///     cx: View context used for backend reads, palette, and callbacks.
    ///
    /// Returns:
    ///     Full dock, detached-window, or group-host panel contents.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cores = self.scope_cores(self.backend.read(cx));
        let rows = self.cached_rows.clone();
        let groups = self.cached_groups.clone();
        let p = MoonPalette::active(cx);
        let total_cores = rows.len();

        let core_bar = self.core_bar(&cores, cx);
        let footer = self.footer(&groups, total_cores, cx);
        let content = match self.mode {
            CoreStatusMode::ByIp => server_view::grouped_server_view(
                groups.clone(),
                Rc::new(self.revealed_ips.clone()),
                self.editing,
                self.edit_input.clone(),
                self.chart_server,
                &self.tree_state,
                cx,
            ),
            CoreStatusMode::Flat => {
                let server_names: HashMap<ServerKey, String> = groups
                    .iter()
                    .map(|group| (group.key, group.display_name.clone()))
                    .collect();
                table::core_status_table(
                    "core-status-table",
                    Rc::new(self.sorted_flat_rows(&rows)),
                    Rc::new(server_names),
                    &self.table_state,
                    cx,
                )
                .into_any_element()
            }
        };

        // A detached window gets a live CPU/memory chart for the expanded (or first) server. The
        // dock tab builds nothing here, so it pays no chart cost.
        let chart_el: Option<AnyElement> = self
            .detached
            .then(|| {
                // Follows the clicked server (chart_server), not expansion; falls back to the first.
                let target = self
                    .chart_server
                    .and_then(|key| groups.iter().find(|group| group.key == key))
                    .or_else(|| groups.first())?;
                let ip = target.address?;
                let b = self.backend.read(cx);
                let points = b.core_chart_hist.ring(ip).cloned().unwrap_or_default();
                // Per-core line pairs only when the server runs more than one core (a single core's
                // pair would just shadow the server pair). Cores with no history yet are skipped.
                let core_series: Vec<chart::CoreLine> = if target.cores.len() > 1 {
                    target
                        .cores
                        .iter()
                        .enumerate()
                        .filter_map(|(i, core)| {
                            let points = b.core_line_hist.ring(core.id)?.clone();
                            Some(chart::CoreLine {
                                name: core.name.clone(),
                                points,
                                color: chart::core_line_color(p, i),
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let now_sec = moon_chart::paint::now_unix_ms() as i64 / 1000;
                Some(chart::server_chart(
                    &points,
                    &core_series,
                    target.display_name.clone(),
                    self.chart_window,
                    now_sec,
                    cx.entity().downgrade(),
                    p,
                    cx,
                ))
            })
            .flatten();

        v_flex()
            .id("core-status-panel")
            .size_full()
            .relative()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .child(core_bar)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(
                v_flex()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(content),
            )
            .children(chart_el.map(|el| {
                v_flex()
                    .w_full()
                    .flex_none()
                    .child(div().w_full().h(px(1.0)).bg(rgb(p.border)))
                    .child(el)
            }))
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(footer)
    }
}

impl CoreStatusView {
    /// Render the core multi-selector in the top bar.
    ///
    /// Clicking a known exchange header batch-toggles its currently available group cores.
    ///
    /// Args:
    ///     cores: Scoped cores in canonical display order.
    ///     cx: View context used to read exchanges and wire selection callbacks.
    ///
    /// Returns:
    ///     The top-bar row containing the fixed-trigger dropdown.
    fn core_bar(&self, cores: &OrderedCores, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let exchange_view = view.clone();
        let exchange_names = self
            .backend
            .read(cx)
            .session
            .market_source()
            .core_exchange_names();
        let combo = crate::controls::core_combo(
            "core-status-core",
            cores,
            &exchange_names,
            &self.sel_cores,
            t!("core_status.all_cores").to_string(),
            |n| t!("core_status.cores_n", n = n).to_string(),
            170.0,
            move |id, app| {
                view.update(app, |t, c| t.toggle_core(id, c));
            },
            move |exchange_cores, app| {
                exchange_view.update(app, |t, c| {
                    t.toggle_exchange_cores(exchange_cores, c);
                });
            },
        );
        let weak_view = cx.entity().downgrade();
        let modes = MoonSegmentedControl::new("core-status-mode")
            .items([
                MoonSegmentItem::new("", t!("core_status.mode.by_ip").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::ByIp),
                MoonSegmentItem::new("", t!("core_status.mode.flat").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::Flat),
            ])
            .on_click(move |index, _, _, app| {
                let Some(view) = weak_view.upgrade() else {
                    return;
                };
                let mode = if index == 0 {
                    CoreStatusMode::ByIp
                } else {
                    CoreStatusMode::Flat
                };
                view.update(app, |this, cx| this.set_mode(mode, cx));
            });
        // The core selector yields first in a narrow side dock, keeping both mode actions
        // reachable without introducing a horizontal scroll host.
        h_flex()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(div().flex_1().min_w_0().overflow_hidden().child(combo))
            .child(modes)
    }

    /// Render server, core, and readiness counters.
    ///
    /// Args:
    ///     groups: Current server snapshots.
    ///     total_cores: Cores in the current selector scope.
    ///     cx: View context used for palette and localization.
    ///
    /// Returns:
    ///     Compact footer shared by grouped and flat presentations.
    fn footer(
        &self,
        groups: &[ServerStatusGroup],
        total_cores: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let online = groups
            .iter()
            .filter(|group| group.ready_count == group.cores.len() && !group.cores.is_empty())
            .count();
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .text_size(design::t_body(cx))
            .text_color(rgb(p.text_muted))
            .child(t!(
                "core_status.footer",
                servers = groups.len(),
                cores = total_cores,
                online = online
            ))
    }
}
