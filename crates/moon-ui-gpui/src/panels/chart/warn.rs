//! Core-warning badges on the chart: an amber gem on the plot's bottom edge at each warning
//! episode's start for this chart's server, plus a hover card that reads it out with a small
//! CPU/memory sparkline (mirroring the news marks, but shown on plain hover rather than Ctrl).
//!
//! Split of responsibilities, like news:
//! - WHICH episodes belong to this chart comes from the backend (persisted + open, by server IP);
//! - the GEMS are own-pass geometry (`chartdx::warn_sync`), scrolling with the live edge every frame;
//! - the CARD is a GPUI overlay, anchored at the cursor, appearing while a badge is hovered.

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
use crate::backend::core_warn::{WarnAxis, WarnEpisode};
use crate::design;

/// How far back warning episodes are collected for a chart; the shader clips whatever is off-plot.
const WARN_SPAN_MS: i64 = 24 * 3600 * 1000;
/// Card width in logical pixels; goes through `design::ui_px` so it follows the UI-scale slider.
const CARD_W: f32 = 240.0;
/// Gap between the gem's top tip and the card's bottom edge.
const CARD_GAP: f32 = 8.0;
/// Gap kept above the card so it never touches the slot's top edge.
const CARD_TOP_INSET: f32 = 8.0;
/// Floor for the card's height.
const CARD_MIN_H: f32 = 48.0;
/// Sparkline height in logical pixels.
const SPARK_H: f32 = 44.0;
/// Recent seconds of history the sparkline shows (the ring is 1 Hz).
const SPARK_SECS: usize = 120;

/// This chart's warning badges, the card payload, and the hover that drives them.
#[derive(Default)]
pub(super) struct WarnState {
    /// Episode + gem pairs, oldest first: the episode backs the card, the gem carries the time.
    items: Rc<Vec<(WarnEpisode, NewsMark)>>,
    /// The gems alone, shared with the engine's own-pass geometry.
    marks: Rc<Vec<NewsMark>>,
    /// Engine episode revision the marks were built from; `None` means none built yet.
    sig: Option<u64>,
    /// Server IP the marks belong to, so a slot reused for another coin/core rebuilds them.
    ip: Option<IpAddr>,
    /// Badge colour the gems were built with, so a theme switch (which changes amber) rebuilds them.
    amber: Option<u32>,
    /// Badges under the cursor: the nearest one grows and its episode fills the card.
    hover: Option<MarkHit>,
    /// Last hit-test point, carrying the Delphi movement threshold the order-line hover uses.
    probe: Option<(f32, f32)>,
}

impl ChartPanel {
    /// Rebuild this chart's warning badges when the episode revision or the chart's server moved, and
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
        let items: Vec<(WarnEpisode, NewsMark)> = {
            let b = self.backend.read(cx);
            b.warn_episodes_for_server(ip, now_ms - WARN_SPAN_MS, now_ms)
                .into_iter()
                .map(|episode| {
                    let mark = NewsMark::new(episode.start_ms, std::iter::once(amber));
                    (episode, mark)
                })
                .collect()
        };
        self.warn.marks = Rc::new(items.iter().map(|(_, mark)| *mark).collect());
        self.warn.items = Rc::new(items);
        // Indices into the old list mean nothing now; re-derive the hover from the live cursor.
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

    /// Re-run the hit test from the last cursor position, without the movement threshold (the chart
    /// scrolls between pointer events, so a badge slides out from under a resting cursor).
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

    /// Apply a hit result and republish the grown badge. Returns whether the tree must repaint (the
    /// card appears/vanishes on any hover change, so any change repaints).
    fn apply_warn_hover(&mut self, hit: Option<MarkHit>, cx: &mut Context<Self>) -> bool {
        if self.warn.hover == hit {
            return false;
        }
        self.warn.hover = hit;
        self.publish_warn_marks(cx);
        true
    }

    /// The hover card, or `None` when no badge is hovered. Anchored at the cursor (the gem moves with
    /// the live edge every frame while GPUI repaints slower, so a gem-anchored card would lag).
    pub(super) fn warn_card(&self, ppp: f32, palette: MoonPalette, cx: &App) -> Option<AnyElement> {
        let hover = self.warn.hover.as_ref()?;
        let (cursor_x, _) = self.input.cursor?;
        let (episode, mark) = self.warn.items.get(hover.nearest)?;
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
        let spark = self.episode_spark(episode, now_ms, cx);
        let header = warn_header(episode, mark, now_ms, self.backend.read(cx), palette, cx);
        let extra = hover.stack.len().saturating_sub(1);

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
                        .child(
                            v_flex()
                                .w_full()
                                .gap(design::ui_px(cx, 6.0))
                                .p(design::ui_px(cx, 8.0))
                                .child(header)
                                .children(spark.map(|points| warn_sparkline(points, palette)))
                                .when(extra > 0, |this| {
                                    this.child(
                                        div()
                                            .text_size(design::t_caption(cx))
                                            .text_color(rgb(palette.text_muted))
                                            .child(format!("+{extra}")),
                                    )
                                }),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Recent `(cpu %, mem %)` points for a card's episode, most recent last, or `None` when a live
    /// sparkline would be misleading (the episode ended longer ago than the window) or there is no
    /// history.
    ///
    /// A per-core (memory-growth) episode uses that core's PROCESS ring; a server episode uses the
    /// MACHINE ring, matching the subject the header names. This is the recent live history, shown
    /// only while it still covers the episode; the precise per-episode ±1 min slice is a follow-up
    /// (the `core_warning_series` table).
    fn episode_spark(&self, episode: &WarnEpisode, now_ms: i64, cx: &App) -> Option<Vec<(u8, u8)>> {
        let covered = episode
            .end_ms
            .is_none_or(|end| now_ms - end < SPARK_SECS as i64 * 1000);
        if !covered {
            return None;
        }
        let b = self.backend.read(cx);
        let ring = match episode.core_id {
            Some(core) => b.core_line_hist.ring(core),
            None => self.warn.ip.and_then(|ip| b.core_chart_hist.ring(ip)),
        }?;
        if ring.len() < 2 {
            return None;
        }
        let start = ring.len().saturating_sub(SPARK_SECS);
        Some(ring.iter().skip(start).copied().collect())
    }
}

/// The card's top line: clock, axis label, peak, and the core name for a per-core episode.
fn warn_header(
    episode: &WarnEpisode,
    mark: &NewsMark,
    now_ms: i64,
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
    let mut head = h_flex()
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
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text))
                .child(t!(axis_key(episode.axis)).to_string()),
        );
    if let Some(peak) = peak_text(episode) {
        head = head.child(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.amber))
                .font_family(design::mono())
                .child(peak),
        );
    }
    // A per-core episode names its core (never the raw IP, which the Core Status panel masks).
    if let Some(name) = episode
        .core_id
        .and_then(|id| core_name(backend, id))
        .filter(|name| !name.is_empty())
    {
        head = head.child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_soft))
                .child(name),
        );
    }
    head.into_any_element()
}

/// Localization key for an axis label.
fn axis_key(axis: WarnAxis) -> &'static str {
    match axis {
        WarnAxis::SysCpu => "core_status.chart_cpu",
        WarnAxis::MemGrowth => "core_status.chart_mem",
        WarnAxis::Unreachable => "core_status.warn_conn",
    }
}

/// Peak reading for a CPU or memory episode; connectivity has no numeric peak.
fn peak_text(episode: &WarnEpisode) -> Option<String> {
    match episode.axis {
        WarnAxis::SysCpu => Some(format!("{}%", episode.peak)),
        WarnAxis::MemGrowth => Some(format!("{} {}", episode.peak, t!("core_status.mb"))),
        WarnAxis::Unreachable => None,
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

/// A compact CPU (blue) / occupied-memory (green) sparkline on a fixed 0..100 % axis.
fn warn_sparkline(points: Vec<(u8, u8)>, p: MoonPalette) -> impl IntoElement {
    let cpu = design::moon(p.blue);
    let mem = design::moon(p.green);
    let n = points.len().max(2);
    let cpu_pts: Vec<(f32, f32)> = points
        .iter()
        .enumerate()
        .map(|(i, s)| (i as f32 / (n - 1) as f32, s.0 as f32))
        .collect();
    let mem_pts: Vec<(f32, f32)> = points
        .iter()
        .enumerate()
        .map(|(i, s)| (i as f32 / (n - 1) as f32, s.1 as f32))
        .collect();
    div().w_full().h(px(SPARK_H)).child(
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
            },
        )
        .size_full(),
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
