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

use std::net::IpAddr;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, MoonSurface, MoonSurfaceVariant, h_flex, v_flex};
use rust_i18n::t;

use moon_chart::axes::fmt_clock_dated;
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

/// One warning event: a run of nearby episodes on one server, merged across axes.
struct WarnCluster {
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
        if self.warn.sig == Some(rev) && self.warn.ip == Some(ip) && self.warn.amber == Some(amber) {
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
        let (cpu, mem) = ring[ring.len() - 1 - back];
        let name = b
            .config
            .servers
            .iter()
            .find(|server| server.id == core)
            .map(|server| server.name.clone())
            .filter(|name| !name.is_empty())?;
        Some((cpu, mem, name))
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
            self.warn.marks.iter().map(|m| map.x_of_time(m.time_ms as f64)),
            pos.0,
            self.last_ppp,
        )
    }

    /// Apply a hit result and republish the grown badge. Returns whether the tree must repaint.
    fn apply_warn_hover(&mut self, hit: Option<MarkHit>, cx: &mut Context<Self>) -> bool {
        if self.warn.hover == hit {
            return false;
        }
        self.warn.hover = hit;
        self.publish_warn_marks(cx);
        true
    }

    /// The hover card, or `None` when no badge is hovered. Anchored at the cursor.
    pub(super) fn warn_card(&self, ppp: f32, palette: MoonPalette, cx: &App) -> Option<AnyElement> {
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
        let body = cluster_card_body(
            cluster,
            mark,
            now_ms,
            core_reading,
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
                    start_ms: episode.start_ms,
                    end_ms: episode.end_ms,
                    reach,
                    cpu_peak: None,
                    mem_peak: None,
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
            cluster.cpu_peak = Some(cluster.cpu_peak.map_or(episode.peak, |p| p.max(episode.peak)))
        }
        WarnAxis::MemGrowth => {
            cluster.mem_peak = Some(cluster.mem_peak.map_or(episode.peak, |p| p.max(episode.peak)))
        }
        WarnAxis::Unreachable => cluster.conn = true,
    }
    if let Some(core) = episode.core_id {
        if !cluster.cores.contains(&core) {
            cluster.cores.push(core);
        }
    }
}

/// The card contents for one warning cluster: time and duration, the worst value per axis that
/// fired, and the full server + current-core state captured at detection.
fn cluster_card_body(
    cluster: &WarnCluster,
    mark: &NewsMark,
    now_ms: i64,
    core_reading: Option<(u8, u8, String)>,
    backend: &crate::Backend,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let clock = fmt_clock_dated(
        mark.time_ms as f64,
        crate::chartdx::axes::local_offset_sec(),
        true,
        now_ms as f64,
    );
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
        lines.push(reading(t!("core_status.chart_cpu").to_string(), format!("{cpu}%"), p, cx));
    }
    if let Some(mem) = cluster.mem_peak {
        lines.push(reading(
            t!("core_status.chart_mem").to_string(),
            format!("{} {}", mem, t!("core_status.mb")),
            p,
            cx,
        ));
    }
    if cluster.conn {
        lines.push(reading(t!("core_status.warn_conn").to_string(), String::new(), p, cx));
    }

    // Full state at detection: the server, then the chart's current core.
    let snap = &cluster.snap;
    let cpu_lbl = t!("core_status.chart_cpu").to_string();
    let mem_lbl = t!("core_status.chart_mem").to_string();
    let server_val = format!("{cpu_lbl} {}% · {mem_lbl} {}%", snap.sys_cpu, snap.occ_mem);
    let server_extra = format!(
        "{} · {}",
        t!(
            "core_status.free_memory",
            value = format!("{} {}", snap.free_mb, t!("core_status.mb"))
        ),
        t!("core_status.logical_cpus", value = snap.logical_cpus)
    );
    let core_line = core_reading.map(|(cpu, mem, name)| {
        reading(name, format!("{cpu_lbl} {cpu}% · {mem_lbl} {mem}%"), p, cx)
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
        .child(muted_line(t!("core_status.warn_at_detect").to_string(), p, cx))
        .child(reading(t!("core_status.warn_server").to_string(), server_val, p, cx))
        .child(muted_line(server_extra, p, cx))
        .children(core_line)
        .when(!cores.is_empty(), |this| this.child(muted_line(cores.join(", "), p, cx)))
        .into_any_element()
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

/// Human duration of a cluster: `Nс` / `Nм`, or "идёт" while still open.
fn duration_text(cluster: &WarnCluster) -> String {
    match cluster.end_ms {
        None => t!("core_status.warn_ongoing").to_string(),
        Some(end) => {
            let secs = ((end - cluster.start_ms).max(0)) / 1000;
            if secs < 60 {
                t!("core_status.ago_s", n = secs).to_string()
            } else {
                t!("core_status.ago_m", n = secs / 60).to_string()
            }
        }
    }
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
