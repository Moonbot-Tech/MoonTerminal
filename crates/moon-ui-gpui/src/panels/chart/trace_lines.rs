//! The panel's half of the "Trades: Moonbot lines" style: what to resolve, and handing the answers
//! to the engine.
//!
//! The engine says which closed trades it wants lines for (the ones nearest its right edge —
//! `chartdx::archived_lines`); this side asks the process's one trace resolver
//! (`backend::traces`) for them and, on the resolver's wake, collects every answered trade of
//! this chart's history into one map the engine draws from. Nothing here reads the archive or
//! the core: the trade window and this panel go through the same resolver, so a trade opened in
//! the window is already resolved for the chart and the other way round.
//!
//! Asking is rate-limited and change-gated: a pan moves the right edge every frame, and ranking a
//! thousand rows per frame to ask about the same thirty would be work for nothing.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::*;
use moon_core::config::TradeHistoryStyle;
use moon_core::feed::ArchivedOrderTrace;

use super::ChartPanel;

/// Most trades one chart asks the core about per ranking: the ones nearest the right edge. The
/// rest of the history is resolved from the archive alone and, when the user pans to it, asked
/// for then.
pub(super) const ASK_CAP: usize = 30;

/// Least time between two rankings driven by the view alone. A forced request (new history, a
/// style change) ignores it.
const VIEW_ASK_INTERVAL: Duration = Duration::from_millis(400);

/// What the panel remembers between rankings and wakes.
#[derive(Default)]
pub(super) struct TraceLinesState {
    /// The uids of the last ranking handed to the resolver, so an unchanged view costs nothing.
    last_wanted: Vec<i64>,
    /// When the last view-driven ranking ran.
    last_ask: Option<Instant>,
    /// Signature of the resolver stamps the engine's current map was built from.
    last_sig: u64,
    /// The resolver's per-core revision the last walk saw, so a wake about another core's rows
    /// — a backfill files hundreds a minute — costs this chart one compare and no walk.
    last_core_rev: u64,
}

impl ChartPanel {
    /// Ask the resolver for the trades the engine wants, in the lines style.
    ///
    /// Args:
    ///     force: Rank now even if the view-driven interval has not passed — the history or the
    ///         style changed, not merely the camera.
    ///     cx: Panel context.
    pub(super) fn request_trace_lines(&mut self, force: bool, cx: &mut Context<Self>) {
        // The trade window's panel draws its trade's lines through its own frozen store; this
        // path is the live chart's, like every other trade-history method here.
        if self.historical
            || self.effective_chart_graphics(cx).trade_history_style
                != TradeHistoryStyle::MoonbotLines
        {
            return;
        }
        let now = Instant::now();
        if !force
            && self
                .trace_lines
                .last_ask
                .is_some_and(|last| now.duration_since(last) < VIEW_ASK_INTERVAL)
        {
            return;
        }
        self.trace_lines.last_ask = Some(now);
        let wanted = self.chart.wanted_trace_uids(ASK_CAP);
        if wanted.is_empty() || (!force && wanted == self.trace_lines.last_wanted) {
            return;
        }
        // One core per ranking: a chart's history is the exact target's, and the resolver is
        // keyed by core. A compare tab's panes each carry their own history through their own
        // engine, so this holds there too.
        let Some(core) = self
            .chart
            .trade_history()
            .iter()
            .find(|record| record.report_uid.is_some_and(|uid| wanted.contains(&uid)))
            .map(|record| record.core_uid)
        else {
            return;
        };
        self.trace_lines.last_wanted = wanted.clone();
        self.backend.update(cx, |backend, cx| {
            backend.ensure_traces(core, wanted, ASK_CAP, cx);
        });
    }

    /// Collect the resolver's answers for this chart's trades and hand them to the engine.
    ///
    /// Called from the resolver's wake and after anything that changes which trades the chart
    /// draws. Exits on the style first, then on the resolver's per-core revision — a wake about
    /// another core's rows costs one compare — and walks the history only when its own core's
    /// rows moved, handing over a new map only when a stamp of this chart's changed.
    ///
    /// Args:
    ///     force: Walk even when the core's revision did not move — the history or the style
    ///         changed on this side.
    ///     cx: Panel context.
    pub(super) fn sync_trace_lines(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.historical
            || self.effective_chart_graphics(cx).trade_history_style
                != TradeHistoryStyle::MoonbotLines
        {
            // Forget the signature while the style is arrows: the history can change underneath
            // it, and a later flip back to lines has to rebuild the map from scratch.
            self.trace_lines.last_sig = 0;
            self.trace_lines.last_core_rev = 0;
            return;
        }
        let history = self.chart.trade_history();
        let Some(core) = history.first().map(|record| record.core_uid) else {
            return;
        };
        let (map, sig) = {
            let backend = self.backend.read(cx);
            let core_rev = backend.traces_core_rev(core);
            if !force && core_rev == self.trace_lines.last_core_rev {
                return;
            }
            self.trace_lines.last_core_rev = core_rev;
            let mut sig = 0u64;
            let mut map: HashMap<i64, Arc<[ArchivedOrderTrace]>> = HashMap::new();
            for record in history.iter() {
                let Some(uid) = record.report_uid else {
                    continue;
                };
                let (state, stamp) = backend.trace_state_stamped(record.core_uid, uid);
                sig = sig.wrapping_mul(31).wrapping_add(stamp);
                if let Some(lines) = state.lines().filter(|lines| !lines.is_empty()) {
                    map.insert(uid, lines.clone());
                }
            }
            (map, sig)
        };
        if sig == self.trace_lines.last_sig {
            return;
        }
        self.trace_lines.last_sig = sig;
        log::info!(
            "[traces] chart {}: {} of {} trades have lines",
            self.chart_id_for_log(),
            map.len(),
            history.len()
        );
        if self.chart.set_archived_lines(Rc::new(map)) {
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// The chart's target as one string for the log line above; empty before a target exists.
    fn chart_id_for_log(&self) -> String {
        self.chart
            .trade_history()
            .first()
            .map(|record| format!("core={} {}", record.core_uid, record.coin))
            .unwrap_or_default()
    }
}
