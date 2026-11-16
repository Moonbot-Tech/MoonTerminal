//! Main's own detect feed: with the tab's `main_detects` switch on, every detect the group's
//! Detects feed would show also opens its chart here.
//!
//! The model is the AddToChart tab's, moved onto Main's stack. A detect chart goes in BEHIND the one
//! on screen — it never takes fullscreen from a chart the reader is looking at, and is reached from
//! the tab row or by expanding the stack — unless Main was empty, where there is nothing to hide
//! behind. It lives for its strategy's `KeepInChart`, exactly like the pane an AddToChart tab would
//! have given it, so the panel's own TTL timer closes it and `prune_empty` drops the slot. A row
//! no strategy produced has no `KeepInChart` to speak of and lives as long as its feed card
//! (`ChartTabs::ingest` resolves the lifetime before the row gets here).
//!
//! Main's idle auto-close (`prune_idle`) still applies on top, to these charts as to any other:
//! it is the reader's own rule for the whole tab, and whichever deadline comes first closes the
//! chart. A repeat detect counts as a fresh arrival for it, as it does for the lifetime.
//!
//! An arrival on an EMPTY Main becomes its selected chart, and so the group's trading target
//! (`ChartTabs::main_chart_target`): there is no other chart for it to stand behind, and a Main
//! holding charts always has one selected.
//!
//! The cap ([`admit`]) counts DETECT charts only. A chart the reader opened by hand is their
//! workspace, not the feed's: it neither takes room from the feed nor gives way to it. Opening a
//! detect chart by hand adopts it for good (see `open_or_focus`).

use super::*;
use crate::chart_tabs::add_stack::detect_cap::{
    Admission, admit, resolved_max_charts, resolved_max_charts_evict,
};

impl MainChartStack {
    /// Whether this Main opens the Detects feed's charts. Unset is off.
    pub(crate) fn main_detects_on(&self) -> bool {
        self.main_detects.unwrap_or(false)
    }

    /// Put one detect's chart on Main, honoring the switch, the cap and an existing chart.
    ///
    /// Args:
    ///     core: Core that fired the detect.
    ///     market: Canonical market of the detect.
    ///     ttl_ms: The chart's lifetime as `ChartTabs::ingest` resolved it: the strategy's
    ///         `KeepInChart` (infinite for `0`), or the feed card's lifetime for a strategy-less row.
    ///     cx: Stack context used to open, retire and publish charts.
    pub(in crate::chart_tabs) fn add_detect_coin(
        &mut self,
        core: CoreId,
        market: &str,
        ttl_ms: f64,
        cx: &mut Context<Self>,
    ) {
        if !self.main_detects_on() {
            return;
        }
        let key = (core, market.to_string());
        if let Some(ix) = self.index_of(&key) {
            // A repeat detect refreshes the lifetime of a chart the feed opened. A chart the reader
            // opened stays theirs: handing it a TTL would close a workspace chart behind their back.
            if self.charts[ix].owner == SlotOwner::DetectFeed {
                // Both clocks restart: the pane's lifetime and the idle-close deadline, which
                // counts from the arrival and would otherwise close a chart a detect just renewed.
                self.charts[ix].arrived_at = Instant::now();
                self.charts[ix]
                    .panel
                    .update(cx, |p, pcx| p.add_coin(core, market, ttl_ms, pcx));
                self.arm_idle_timer(cx);
            }
            return;
        }
        let active_key = self
            .active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| (entry.core, entry.market.clone()));
        match self.admit_detect(cx) {
            Admission::Accept => {}
            Admission::Drop => {
                if moon_core::detect_diag::enabled() {
                    moon_core::detect_diag::line(&format!(
                        "[main] {market}: at cap {}, detect NOT shown",
                        resolved_max_charts(self.max_charts)
                    ));
                }
                return;
            }
            Admission::Evict(ix) => {
                if moon_core::detect_diag::enabled() {
                    let victim = &self.charts[ix];
                    moon_core::detect_diag::line(&format!(
                        "[main] {market}: at cap {}, replacing stalest {}/{}",
                        resolved_max_charts(self.max_charts),
                        victim.core,
                        victim.market
                    ));
                }
                self.remove_chart_at(ix, cx);
            }
        }
        let panel = self.create_panel(core, market, cx);
        // `create_panel` opens the market as a Main pane; the detect re-tags that same pane as an
        // AddToChart one carrying the strategy's lifetime, which also arms the panel's TTL timer.
        panel.update(cx, |p, pcx| {
            p.add_coin(core, market, ttl_ms, pcx);
            // Not `apply_history_scope`: that one jumps the camera to the newest closed trade,
            // which is right for a chart the reader asked for and wrong for one arriving live.
            p.track_history_scope(core, market.to_string(), pcx);
        });
        self.charts.push(ChartStackEntry::new(
            core,
            market.to_string(),
            panel,
            SlotOwner::DetectFeed,
        ));
        // The chart on screen stays on screen. Identity, not index: an eviction above may have
        // shifted it. An empty Main has nothing to keep, and the arrival is what it shows.
        self.active = remap_active_index(
            self.charts.len(),
            active_key.as_ref(),
            Some(self.charts.len() - 1),
            |ix, key| self.charts.get(ix).is_some_and(|entry| entry.is(key)),
        );
        if active_key.is_none() {
            self.show_stack = false;
        }
        self.sync_visibility(cx);
        self.sync_backend_open_markets(cx);
        self.sync_compare(cx);
        self.arm_idle_timer(cx);
        cx.notify();
    }

    /// Decide a NEW detect chart's fate against Main's cap.
    ///
    /// Only the feed's own charts are slots here. Of those, the SELECTED one is never the victim,
    /// fullscreen or as a tile of the expanded stack: it is the chart the reader is looking at and
    /// the group's trading target, and evicting it would hand both to a coin that just arrived.
    /// That is on top of what `eviction_rank_ms` already withholds: pinned, and in use right now.
    ///
    /// The price is paid at a cap of ONE on a Main holding nothing but the feed's chart: that chart
    /// is the selected one, so later detects go unshown until it expires or is closed. Deliberate —
    /// the alternative is a trading target that changes coin on every detect.
    fn admit_detect(&self, cx: &App) -> Admission {
        let cap = resolved_max_charts(self.max_charts);
        if cap == 0 {
            return Admission::Accept;
        }
        let selected = self.active;
        let slots: Vec<(usize, Option<f64>)> = self
            .charts
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.owner == SlotOwner::DetectFeed && entry.is_live(cx))
            .map(|(ix, entry)| {
                let rank = entry.eviction_rank_ms(cx).filter(|_| Some(ix) != selected);
                (ix, rank)
            })
            .collect();
        admit(
            Some(cap),
            resolved_max_charts_evict(self.max_charts_evict),
            &slots,
        )
    }
}
