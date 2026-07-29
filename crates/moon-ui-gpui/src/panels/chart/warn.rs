//! Core-warning badges on the chart: an amber gem on the plot's bottom edge at each warning
//! episode's start for this chart's server (Moonbot-style, mirroring the news marks).
//!
//! Split of responsibilities, like news:
//! - WHICH episodes belong to this chart comes from the backend: the chart resolves its core → server
//!   IP, then reads persisted + open episodes for that server in the visible span;
//! - the GEMS are own-pass geometry (`chartdx::warn_sync`), so they scroll with the live edge every
//!   frame without a GPUI relayout.
//!
//! Rebuilt only when the engine's episode revision or this chart's server moves — the backend
//! observer fires far more often than warnings occur, and the revision gate keeps that cheap.

use std::net::IpAddr;
use std::rc::Rc;

use gpui::Context;
use moon_chart::news_marks::NewsMark;
use moon_core::util::now_unix_ms_i64;
use moon_ui::MoonPalette;

use super::ChartPanel;

/// How far back warning episodes are collected for a chart. Older ones scroll off the left anyway;
/// the shader clips whatever falls outside the plot.
const WARN_SPAN_MS: i64 = 24 * 3600 * 1000;

/// This chart's warning badges and the state that rebuilds them only on a real change.
#[derive(Default)]
pub(super) struct WarnState {
    /// Amber gems, one per episode, shared with the engine's own-pass geometry.
    marks: Rc<Vec<NewsMark>>,
    /// Engine episode revision the marks were built from; `None` means none built yet.
    sig: Option<u64>,
    /// Server IP the marks belong to, so a slot reused for another coin/core rebuilds them.
    ip: Option<IpAddr>,
}

impl ChartPanel {
    /// Rebuild this chart's warning badges when the episode revision or the chart's server moved, and
    /// publish them to the engine. Returns whether the engine's geometry changed.
    pub(super) fn sync_warn_marks(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((core, _market)) = self.chart.active_target() else {
            // No target on this slot: drop the badges so a reused panel cannot show another server's.
            return self.clear_warn(cx);
        };
        let palette = MoonPalette::active(cx);
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
        if self.warn.sig == Some(rev) && self.warn.ip == Some(ip) {
            return false;
        }
        self.warn.sig = Some(rev);
        self.warn.ip = Some(ip);

        let now_ms = now_unix_ms_i64();
        let amber = palette.amber;
        let marks: Vec<NewsMark> = {
            let b = self.backend.read(cx);
            b.warn_episodes_for_server(ip, now_ms - WARN_SPAN_MS, now_ms)
                .into_iter()
                .map(|episode| NewsMark::new(episode.start_ms, std::iter::once(amber)))
                .collect()
        };
        self.warn.marks = Rc::new(marks);
        self.publish_warn_marks(cx)
    }

    /// Push the current badges to the engine, forcing the userdata rebuild that owns them.
    fn publish_warn_marks(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.chart.set_warn_marks(self.warn.marks.clone(), None) {
            return false;
        }
        // Userdata rebuilds only through `sync_orders_*`; trigger it now so the badges appear at once.
        let b = self.backend.read(cx);
        self.chart.sync_orders_if_visible(&b.session, true);
        true
    }

    /// Drop the badges when the chart has no server, so a reused slot cannot show stale ones.
    fn clear_warn(&mut self, cx: &mut Context<Self>) -> bool {
        if self.warn.marks.is_empty() && self.warn.sig.is_none() && self.warn.ip.is_none() {
            return false;
        }
        self.warn = WarnState::default();
        self.publish_warn_marks(cx)
    }
}
