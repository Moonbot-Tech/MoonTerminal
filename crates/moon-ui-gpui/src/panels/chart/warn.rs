//! Core-warning badges on the chart: a gem on the plot's bottom edge for each cluster of warning
//! episodes on this chart's server, plus a hover card that reads it out.
//!
//! Episodes that fall within a minute of each other — a CPU spike that flickers, or CPU and memory
//! warnings from the same moment — are ONE badge, not several: a warning "event" is the cluster.
//!
//! Split of responsibilities, like news:
//! - WHICH episodes belong to this chart comes from the backend (persisted + open, by server IP);
//! - the GEMS are own-pass geometry (`chartdx::warn_sync`), scrolling with the live edge every frame;
//! - the CARD is a GPUI overlay, anchored at the cursor, appearing while a badge is hovered.
//!
//! The card carries the readings captured at detection (axis peaks, cores). The per-episode ±1 min
//! graph is deliberately absent until the slice is written to `core_warning_series`.

use moon_core::figures::Proj;
use std::net::IpAddr;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, MoonSurface, MoonSurfaceVariant, h_flex, v_flex};
use rust_i18n::t;

use moon_chart::news_marks::{MarkHit, NewsMark};
use moon_core::session::CoreId;
use moon_core::util::now_unix_ms_i64;

use super::ChartPanel;
use crate::backend::core_warn::{WarnAxis, WarnEpisode, WarnSnapshot};
use crate::design;

/// How far back warning episodes are collected for a chart; the shader clips whatever is off-plot.
const WARN_SPAN_MS: i64 = 24 * 3600 * 1000;
/// Episodes within this gap of each other are the same warning event (one badge).
const CLUSTER_GAP_MS: i64 = 60_000;
/// Card width in logical pixels; goes through `design::ui_px` so it follows the UI-scale slider.
const CARD_W: f32 = 220.0;
/// Gap between the gem's top tip and the card's bottom edge.
const CARD_GAP: f32 = 8.0;
/// Gap kept above the card so it never touches the slot's top edge.
const CARD_TOP_INSET: f32 = 8.0;
/// Floor for the card's height.
const CARD_MIN_H: f32 = 48.0;
/// Height of the card's ±1 min graph in logical pixels.
const GRAPH_H: f32 = 44.0;

/// One warning event: a run of nearby episodes on one server, merged across axes.
struct WarnCluster {
    /// The earliest (persisted) episode's id, used to fetch its stored ±1 min slice for the graph.
    id: u64,
    /// Earliest episode start in the cluster (where the gem sits).
    start_ms: i64,
    /// Latest episode end, or `None` while any member is still open.
    end_ms: Option<i64>,
    /// Latest activity (end, or start while open), for clustering and duration.
    reach: i64,
    /// Worst sustained-CPU peak among members, if any CPU episode.
    cpu_peak: Option<u16>,
    /// Worst memory-growth peak (used MB) among members, if any memory episode.
    mem_peak: Option<u16>,
    /// Worst client↔core ping peak (RTT ms) among members, if any ping episode.
    ping_peak: Option<u16>,
    /// Worst core→exchange ping peak (order-latency ms) among members, if any exch-ping episode.
    exch_peak: Option<u16>,
    /// Whether any member is a connectivity (dropped-core) warning.
    conn: bool,
    /// Whether any member is still open.
    open: bool,
    /// Distinct cores named by per-core members.
    cores: Vec<CoreId>,
    /// Server telemetry captured when the event first fired.
    snap: WarnSnapshot,
}

/// This chart's warning badges (one per cluster), the card payload, and the hover that drives them.
#[derive(Default)]
pub(super) struct WarnState {
    /// Cluster + gem pairs, oldest first: the cluster backs the card, the gem carries the time.
    items: Rc<Vec<(WarnCluster, NewsMark)>>,
    /// The gems alone, shared with the engine's own-pass geometry.
    marks: Rc<Vec<NewsMark>>,
    /// Engine episode revision the marks were built from; `None` means none built yet.
    sig: Option<u64>,
    /// Server IP the marks belong to, so a slot reused for another coin/core rebuilds them.
    ip: Option<IpAddr>,
    /// Badge colour the gems were built with, so a theme switch rebuilds them.
    amber: Option<u32>,
    /// Badges under the cursor: the nearest one grows and its cluster fills the card.
    hover: Option<MarkHit>,
    /// Last hit-test point, carrying the Delphi movement threshold the order-line hover uses.
    probe: Option<(f32, f32)>,
    /// Whether the secondary modifier (Ctrl, Command on macOS) was held at the last pointer event.
    /// The card only shows while it is held, matching the news card.
    ctrl: bool,
}

impl ChartPanel {
    /// Rebuild this chart's warning badges when the episode revision, server, or theme moved, and
    /// publish them to the engine. Returns whether the engine's geometry changed.
    pub(super) fn sync_warn_marks(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((core, _market)) = self.chart.active_target() else {
            return self.clear_warn(cx);
        };
        let amber = MoonPalette::active(cx).amber;
        let (ip, rev) = {
            let b = self.backend.read(cx);
            let ip = b
                .session
                .store()
                .core(core)
                .and_then(|core| core.endpoint)
                .map(|endpoint| endpoint.address);
            (ip, b.warn.episode_rev())
        };
        let Some(ip) = ip else {
            return self.clear_warn(cx);
        };
        if self.warn.sig == Some(rev) && self.warn.ip == Some(ip) && self.warn.amber == Some(amber)
        {
            return false;
        }
        self.warn.sig = Some(rev);
        self.warn.ip = Some(ip);
        self.warn.amber = Some(amber);

        let now_ms = now_unix_ms_i64();
        let episodes = {
            let b = self.backend.read(cx);
            b.warn_episodes_for_server(ip, now_ms - WARN_SPAN_MS, now_ms)
        };
        let items: Vec<(WarnCluster, NewsMark)> = cluster_episodes(episodes)
            .into_iter()
            .map(|cluster| {
                let mark = NewsMark::new(cluster.start_ms, std::iter::once(amber));
                (cluster, mark)
            })
            .collect();
        self.warn.marks = Rc::new(items.iter().map(|(_, mark)| *mark).collect());
        self.warn.items = Rc::new(items);
        self.warn.probe = None;
        self.warn.hover = self.warn_hit_at_cursor();
        self.publish_warn_marks(cx)
    }

    /// Push the current badges and the hovered one to the engine, forcing the userdata rebuild.
    fn publish_warn_marks(&mut self, cx: &mut Context<Self>) -> bool {
        let hovered = self.warn.hover.as_ref().map(|h| h.nearest);
        if !self.chart.set_warn_marks(self.warn.marks.clone(), hovered) {
            return false;
        }
        let b = self.backend.read(cx);
        self.chart.sync_orders_if_visible(&b.session, true);
        true
    }

    /// Drop the badges when the chart has no server, so a reused slot cannot show stale ones.
    fn clear_warn(&mut self, cx: &mut Context<Self>) -> bool {
        if self.warn.sig.is_none() {
            return false;
        }
        self.warn = WarnState::default();
        self.publish_warn_marks(cx)
    }

    /// Update the hovered badges from a pointer position; returns whether the tree must repaint.
    pub(super) fn sync_warn_hover(
        &mut self,
        pos: (f32, f32),
        within: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !within {
            return self.clear_warn_hover(cx);
        }
        if !super::trade::hover_probe_due(self.warn.probe, pos) {
            return false;
        }
        self.warn.probe = Some(pos);
        let hit = self.warn_hit(pos);
        self.apply_warn_hover(hit, cx)
    }

    /// Drop the hover when the pointer leaves the chart slot entirely.
    pub(super) fn clear_warn_hover(&mut self, cx: &mut Context<Self>) -> bool {
        self.warn.probe = None;
        self.apply_warn_hover(None, cx)
    }

    /// Re-run the hit test from the last cursor position, without the movement threshold.
    pub(super) fn revalidate_warn_hover(&mut self, cx: &mut Context<Self>) {
        if self.warn.hover.is_none() {
            return;
        }
        let hit = self.warn_hit_at_cursor();
        self.apply_warn_hover(hit, cx);
    }

    /// Hit-test at the panel's last known cursor position, if it has one.
    fn warn_hit_at_cursor(&self) -> Option<MarkHit> {
        self.warn_hit(self.input.cursor?)
    }

    /// This chart's current core at a past moment: `(process CPU %, process memory %, core name)` from
    /// its history ring. `None` when there is no current core or the ring no longer reaches that far.
    fn core_reading_at(&self, at_ms: i64, now_ms: i64, cx: &App) -> Option<(u8, u8, String)> {
        let (core, _market) = self.chart.active_target()?;
        let b = self.backend.read(cx);
        let ring = b.core_line_hist.ring(core)?;
        let back = ((now_ms - at_ms).max(0) / 1000) as usize;
        if back >= ring.len() {
            return None;
        }
        let metrics = ring[ring.len() - 1 - back];
        let (cpu, mem) = (metrics.cpu, metrics.mem);
        let name = b
            .config
            .servers
            .iter()
            .find(|server| server.id == core)
            .map(|server| server.name.clone())
            .filter(|name| !name.is_empty())?;
        Some((cpu, mem, name))
    }

    /// The server's `(system CPU %, occupied memory %)` history around a moment: the ±60 s window
    /// centred on `at_ms`, sliced positionally from the 1 Hz ring. `None` when there is too little.
    ///
    /// Right after the warning only `[at-60, now]` exists; a minute later the full `[at-60, at+60]`
    /// window has filled in, so the card's graph "grows" to the full ±1 min. The backend owns the
    /// slicing so the same read backs the capture that persists it for old episodes.
    fn server_slice(&self, ip: IpAddr, at_ms: i64, now_ms: i64, cx: &App) -> Option<Vec<(u8, u8)>> {
        self.backend.read(cx).warn_server_slice(ip, at_ms, now_ms)
    }

    /// Hit-test the badges' row at `pos` (panel-local device pixels). Mirror of `news_hit`.
    fn warn_hit(&self, pos: (f32, f32)) -> Option<MarkHit> {
        if self.warn.marks.is_empty() || self.orderbook_only {
            return None;
        }
        let pane = self.input.pane_at(pos.0, pos.1)?;
        let map = self.pane_map(pane)?;
        if !moon_chart::news_marks::in_mark_row(pos.1, map.plot.y + map.plot.h, self.last_ppp) {
            return None;
        }
        if pos.0 < map.plot.x || pos.0 > map.plot.x + map.plot.w {
            return None;
        }
        moon_chart::news_marks::hit_marks(
            self.warn
                .marks
                .iter()
                .map(|m| map.x_of_time(m.time_ms as f64)),
            pos.0,
            self.last_ppp,
        )
    }

    /// Apply a hit result and republish the grown badge. The gem grows own-pass regardless, so a
    /// GPUI repaint is needed only while the Ctrl card is on screen.
    fn apply_warn_hover(&mut self, hit: Option<MarkHit>, cx: &mut Context<Self>) -> bool {
        if self.warn.hover == hit {
            return false;
        }
        self.warn.hover = hit;
        self.publish_warn_marks(cx);
        self.warn.ctrl
    }

    /// Track the secondary modifier and repaint when it changes what the card shows.
    pub(super) fn note_warn_modifiers(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        let ctrl = modifiers.secondary();
        if self.warn.ctrl == ctrl {
            return;
        }
        self.warn.ctrl = ctrl;
        // Only a badge under the cursor can produce a card; a bare Ctrl press must not repaint.
        if self.warn.hover.is_some() {
            cx.notify();
        }
    }

    /// The hover card, or `None` unless a badge is hovered AND Ctrl (Command on macOS) is held.
    pub(super) fn warn_card(
        &self,
        ppp: f32,
        ctrl: bool,
        palette: MoonPalette,
        cx: &App,
    ) -> Option<AnyElement> {
        if !ctrl {
            return None;
        }
        let hover = self.warn.hover.as_ref()?;
        let (cursor_x, _) = self.input.cursor?;
        let (cluster, mark) = self.warn.items.get(hover.nearest)?;
        let ppp = ppp.max(0.1);
        let slot = self.chart.slot_dev_size();
        let slot_w = slot.0 as f32 / ppp;
        let slot_h = slot.1 as f32 / ppp;
        let card_w = f32::from(design::ui_px(cx, CARD_W));
        let left = (cursor_x / ppp - card_w * 0.5).clamp(0.0, (slot_w - card_w).max(0.0));
        let axis_h = if self.time_axis_visible {
            moon_chart::TIME_AXIS_H
        } else {
            0.0
        };
        let bottom = axis_h
            + moon_chart::news_marks::mark_center_offset(true) * 2.0
            + f32::from(design::ui_px(cx, CARD_GAP));
        let max_h = (slot_h - bottom - CARD_TOP_INSET).max(CARD_MIN_H);

        let now_ms = now_unix_ms_i64();
        let core_reading = self.core_reading_at(cluster.start_ms, now_ms, cx);
        let graph = self
            .warn
            .ip
            .and_then(|ip| self.server_slice(ip, cluster.start_ms, now_ms, cx))
            .or_else(|| {
                // Older than the live ring: fall back to the slice persisted at capture. Only a
                // closed cluster has one (an open episode carries an engine id, not a stored row id).
                (!cluster.open)
                    .then(|| self.backend.read(cx).warn_series_slice(cluster.id))
                    .flatten()
            });
        // Each ping line rides the same window on its own ms scale (live ring, then persisted slice).
        let ping_graph = self
            .warn
            .ip
            .and_then(|ip| {
                self.backend
                    .read(cx)
                    .warn_server_ping_slice(ip, cluster.start_ms, now_ms)
            })
            .or_else(|| {
                (!cluster.open)
                    .then(|| self.backend.read(cx).warn_ping_series_slice(cluster.id))
                    .flatten()
            });
        let exch_graph = self
            .warn
            .ip
            .and_then(|ip| {
                self.backend
                    .read(cx)
                    .warn_server_exch_slice(ip, cluster.start_ms, now_ms)
            })
            .or_else(|| {
                (!cluster.open)
                    .then(|| self.backend.read(cx).warn_exch_series_slice(cluster.id))
                    .flatten()
            });
        let body = cluster_card_body(
            cluster,
            mark,
            now_ms,
            core_reading,
            graph,
            ping_graph,
            exch_graph,
            self.backend.read(cx),
            palette,
            cx,
        );

        Some(
            div()
                .absolute()
                .left(px(left))
                .bottom(px(bottom))
                .w(px(card_w))
                .max_h(px(max_h))
                .overflow_hidden()
                .shadow_md()
                .child(
                    MoonSurface::new()
                        .variant(MoonSurfaceVariant::Card)
                        .bg_color(palette.card)
                        .bg_alpha(1.0)
                        .border_color(palette.border_card)
                        .child(body),
                )
                .into_any_element(),
        )
    }
}

/// Merge episodes into warning events: nearby-in-time episodes (any axis) on the server are one.
fn cluster_episodes(mut episodes: Vec<WarnEpisode>) -> Vec<WarnCluster> {
    episodes.sort_by_key(|episode| episode.start_ms);
    let mut out: Vec<WarnCluster> = Vec::new();
    for episode in episodes {
        let reach = episode.end_ms.unwrap_or(episode.start_ms);
        match out.last_mut() {
            Some(cluster) if episode.start_ms - cluster.reach <= CLUSTER_GAP_MS => {
                cluster.reach = cluster.reach.max(reach);
                if episode.end_ms.is_none() {
                    cluster.open = true;
                }
                if !cluster.open {
                    cluster.end_ms = Some(cluster.end_ms.map_or(reach, |end| end.max(reach)));
                } else {
                    cluster.end_ms = None;
                }
                merge_axis(cluster, &episode);
            }
            _ => {
                let mut cluster = WarnCluster {
                    id: episode.id,
                    start_ms: episode.start_ms,
                    end_ms: episode.end_ms,
                    reach,
                    cpu_peak: None,
                    mem_peak: None,
                    ping_peak: None,
                    exch_peak: None,
                    conn: false,
                    open: episode.end_ms.is_none(),
                    cores: Vec::new(),
                    // The first (earliest) episode's snapshot is the event's detection state.
                    snap: episode.snap,
                };
                merge_axis(&mut cluster, &episode);
                out.push(cluster);
            }
        }
    }
    out
}

/// Fold one episode's axis, peak, and core into a cluster.
fn merge_axis(cluster: &mut WarnCluster, episode: &WarnEpisode) {
    match episode.axis {
        WarnAxis::SysCpu => {
            cluster.cpu_peak = Some(
                cluster
                    .cpu_peak
                    .map_or(episode.peak, |p| p.max(episode.peak)),
            )
        }
        WarnAxis::MemGrowth => {
            cluster.mem_peak = Some(
                cluster
                    .mem_peak
                    .map_or(episode.peak, |p| p.max(episode.peak)),
            )
        }
        WarnAxis::Ping => {
            cluster.ping_peak = Some(
                cluster
                    .ping_peak
                    .map_or(episode.peak, |p| p.max(episode.peak)),
            )
        }
        WarnAxis::ExchPing => {
            cluster.exch_peak = Some(
                cluster
                    .exch_peak
                    .map_or(episode.peak, |p| p.max(episode.peak)),
            )
        }
        WarnAxis::Unreachable => cluster.conn = true,
        // Never reaches here: `warn_chart` reports false for this axis, so the chart read path
        // filters it out before clustering. An expiring key is a standing state with no per-second
        // history, so it has nothing to draw on a time axis.
        WarnAxis::ApiExpiry => {}
    }
    if let Some(core) = episode.core_id {
        if !cluster.cores.contains(&core) {
            cluster.cores.push(core);
        }
    }
}

/// The card contents for one warning cluster: time and duration, the worst value per axis that
/// fired, and the full server + current-core state captured at detection.
#[allow(clippy::too_many_arguments)]
fn cluster_card_body(
    cluster: &WarnCluster,
    mark: &NewsMark,
    now_ms: i64,
    core_reading: Option<(u8, u8, String)>,
    graph: Option<Vec<(u8, u8)>>,
    ping_graph: Option<Vec<u16>>,
    exch_graph: Option<Vec<u16>>,
    backend: &crate::Backend,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let clock = crate::chartdx::axes::format_clock_dated(mark.time_ms as f64, true, now_ms as f64);
    let head = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 5.0))
        .child(design::status_dot(p.amber, cx))
        .child(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .font_family(design::mono())
                .child(clock),
        )
        .child(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .font_family(design::mono())
                .child(duration_text(cluster)),
        );

    // Worst value per axis that fired (mirrors the tab's CPU / RAM / Link).
    let mut lines: Vec<AnyElement> = Vec::new();
    if let Some(cpu) = cluster.cpu_peak {
        lines.push(reading(
            t!("core_status.chart_cpu").to_string(),
            format!("{cpu}%"),
            p,
            cx,
        ));
    }
    if let Some(mem) = cluster.mem_peak {
        lines.push(reading(
            t!("core_status.chart_mem").to_string(),
            format!("{} {}", mem, t!("core_status.mb")),
            p,
            cx,
        ));
    }
    if let Some(ping) = cluster.ping_peak {
        lines.push(ms_reading(
            t!("core_status.chart_ping").to_string(),
            ping,
            p,
            cx,
        ));
    }
    if let Some(exch) = cluster.exch_peak {
        lines.push(ms_reading(
            t!("core_status.chart_exch").to_string(),
            exch,
            p,
            cx,
        ));
    }
    if cluster.conn {
        lines.push(reading(
            t!("core_status.warn_conn").to_string(),
            String::new(),
            p,
            cx,
        ));
    }

    // Full state at detection: the server (CPU / memory only — the pings live under the core), then
    // the chart's current core with both pings beneath it.
    let snap = &cluster.snap;
    let cpu_lbl = t!("core_status.chart_cpu").to_string();
    let mem_lbl = t!("core_status.chart_mem").to_string();
    let server_val = format!("{cpu_lbl} {}% · {mem_lbl} {}%", snap.sys_cpu, snap.occ_mem);
    // Both pings as their own reading lines, so a long value can never run off the card edge and they
    // read as belonging to the core rather than crammed onto the server line. Skipped when the reading
    // never arrived (0 = no sample), matching the row's dash-when-absent.
    let ping_line = (snap.round_trip_ms > 0).then(|| {
        ms_reading(
            t!("core_status.chart_ping").to_string(),
            snap.round_trip_ms,
            p,
            cx,
        )
    });
    let exch_line = (snap.order_api_latency_ms > 0).then(|| {
        ms_reading(
            t!("core_status.chart_exch").to_string(),
            snap.order_api_latency_ms,
            p,
            cx,
        )
    });
    let core_line = core_reading.map(|(cpu, mem, name)| {
        // (0, 0) at detection means the core process had not reported telemetry yet (e.g. it was
        // still starting up) — show a dash rather than a misleading 0 %.
        let value = if cpu == 0 && mem == 0 {
            "—".to_string()
        } else {
            format!("{cpu_lbl} {cpu}% · {mem_lbl} {mem}%")
        };
        reading(name, value, p, cx)
    });
    // Cores named by per-core members, never the raw IP.
    let cores: Vec<String> = cluster
        .cores
        .iter()
        .filter_map(|id| core_name(backend, *id))
        .filter(|name| !name.is_empty())
        .collect();

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 6.0))
        .p(design::ui_px(cx, 8.0))
        .child(head)
        .children(lines)
        .child(muted_line(
            t!("core_status.warn_at_detect").to_string(),
            p,
            cx,
        ))
        .child(reading(
            t!("core_status.warn_server").to_string(),
            server_val,
            p,
            cx,
        ))
        .children(core_line)
        // Both pings (client↔core, then core→exchange) at detection, under the core they belong to.
        .children(ping_line)
        .children(exch_line)
        .when(!cores.is_empty(), |this| {
            this.child(muted_line(cores.join(", "), p, cx))
        })
        // The ±1 min graph around the warning (system CPU / occupied memory / ping), once the ring
        // covers it. Drawn if either series has enough points.
        .children({
            let has_cpu_mem = graph.as_ref().is_some_and(|g| g.len() >= 2);
            let has_ping = ping_graph.as_ref().is_some_and(|g| g.len() >= 2);
            let has_exch = exch_graph.as_ref().is_some_and(|g| g.len() >= 2);
            (has_cpu_mem || has_ping || has_exch).then(|| {
                warn_graph(
                    graph.unwrap_or_default(),
                    ping_graph.unwrap_or_default(),
                    exch_graph.unwrap_or_default(),
                    p,
                )
            })
        })
        .into_any_element()
}

/// A compact system-CPU (blue) / occupied-memory (green) graph on a fixed 0..100 % axis, with the
/// client↔core ping (accent) and core→exchange ping (orange) each drawn on its own auto-scaled ms
/// axis mapped into the same height. Any series may be empty.
fn warn_graph(
    points: Vec<(u8, u8)>,
    ping: Vec<u16>,
    exch: Vec<u16>,
    p: MoonPalette,
) -> impl IntoElement {
    let cpu = design::moon(p.blue);
    let mem = design::moon(p.green);
    let ping_col = design::moon(p.accent);
    let exch_col = design::moon(p.orange);
    // Map an index in a series of `len` to its x fraction across the plot.
    let frac = |len: usize, i: usize| i as f32 / (len.max(2) - 1) as f32;
    let cpu_pts: Vec<(f32, f32)> = points
        .iter()
        .enumerate()
        .map(|(i, s)| (frac(points.len(), i), s.0 as f32))
        .collect();
    let mem_pts: Vec<(f32, f32)> = points
        .iter()
        .enumerate()
        .map(|(i, s)| (frac(points.len(), i), s.1 as f32))
        .collect();
    // Each ping rides its own ms scale (round-trip is not a percentage) via the shared ping-axis
    // scale: a tidy 50 ms step with headroom so a line at its maximum does not hug/clip the top edge.
    let ms_scale = |series: &[u16]| {
        crate::panels::common::ms_axis_scale(series.iter().copied().max().unwrap_or(0))
    };
    let ping_scale = ms_scale(&ping);
    let ping_pts: Vec<(f32, f32)> = ping
        .iter()
        .enumerate()
        .map(|(i, &ms)| (frac(ping.len(), i), ms as f32 / ping_scale * 100.0))
        .collect();
    let exch_scale = ms_scale(&exch);
    let exch_pts: Vec<(f32, f32)> = exch
        .iter()
        .enumerate()
        .map(|(i, &ms)| (frac(exch.len(), i), ms as f32 / exch_scale * 100.0))
        .collect();
    // A fixed-height, non-shrinking row with the canvas absolutely filling it, so the graph reserves
    // its own space and never paints over the text above it (the same pattern the Core Status chart
    // uses; a plain in-flow canvas can collapse and overlap).
    div().relative().w_full().h(px(GRAPH_H)).flex_none().child(
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let w = f32::from(bounds.size.width);
                        let h = f32::from(bounds.size.height);
                        if w < 2.0 || h < 2.0 {
                            return;
                        }
                        stroke_line(window, bounds.origin, w, h, &mem_pts, mem);
                        stroke_line(window, bounds.origin, w, h, &cpu_pts, cpu);
                        stroke_line(window, bounds.origin, w, h, &exch_pts, exch_col);
                        stroke_line(window, bounds.origin, w, h, &ping_pts, ping_col);
                    },
                )
                .size_full(),
            ),
    )
}

/// Stroke one series of `(x fraction, 0..100 value)` points onto a 0..100 % plot.
fn stroke_line(
    window: &mut Window,
    origin: Point<Pixels>,
    w: f32,
    h: f32,
    series: &[(f32, f32)],
    color: Hsla,
) {
    if series.len() < 2 {
        return;
    }
    let x = |frac: f32| origin.x + px(frac * w);
    let y = |value: f32| origin.y + px((100.0 - value) / 100.0 * (h - 2.0) + 1.0);
    let mut line = PathBuilder::stroke(px(1.25));
    for (i, &(frac, value)) in series.iter().enumerate() {
        let pt = gpui::point(x(frac), y(value));
        if i == 0 {
            line.move_to(pt);
        } else {
            line.line_to(pt);
        }
    }
    if let Ok(path) = line.build() {
        window.paint_path(path, color);
    }
}

/// A muted, single-line caption in the card.
fn muted_line(text: String, p: MoonPalette, cx: &App) -> AnyElement {
    div()
        .w_full()
        .truncate()
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(text)
        .into_any_element()
}

/// One "label  value" reading line in the card.
fn reading(label: String, value: String, p: MoonPalette, cx: &App) -> AnyElement {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text))
                .child(label),
        )
        .child(
            div()
                .flex_none()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.amber))
                .font_family(design::mono())
                .child(value),
        )
        .into_any_element()
}

/// One `"label   N ms"` reading line (a latency in milliseconds).
fn ms_reading(label: String, ms: u16, p: MoonPalette, cx: &App) -> AnyElement {
    reading(label, format!("{} {}", ms, t!("core_status.ms")), p, cx)
}

/// Human duration of a cluster, through the shared episode formatter so a card and the Core Status
/// list never describe the same episode differently.
fn duration_text(cluster: &WarnCluster) -> String {
    crate::panels::common::warn_duration_text(cluster.start_ms, cluster.end_ms)
}

/// A core's configured display name, if it is still in the config.
fn core_name(backend: &crate::Backend, id: CoreId) -> Option<String> {
    backend
        .config
        .servers
        .iter()
        .find(|server| server.id == id)
        .map(|server| server.name.clone())
}
