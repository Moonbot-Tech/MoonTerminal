//! The per-tab detect cap: how many charts the DETECT feed may keep on one tab, and what a detect
//! does once that number is reached.
//!
//! The rule itself is the pure [`admit`] below, tested without a live stack; [`AddChartStack`]'s
//! half is only the entity work around it — reading each slot, freeing one, and logging what was
//! decided. Manual paths do not come through here at all: the cap governs the feed, and a coin the
//! reader typed is not the feed's to close.
//!
//! A chart whose strategy sets `KeepInChart = 0` never expires on its own, so on such a tab this
//! cap is the ONLY thing bounding how many charts accumulate — which is why [`admit`] is handed
//! each slot's STALENESS rather than its TTL deadline: a chart held forever has no deadline, but it
//! is still more or less stale than its neighbours. A tab EXPLICITLY set to "no cap" keeps its old
//! meaning and now has no bound at all, which is what asking for no cap means.
//!
//! A tab that never touched the setting is CAPPED, at [`DEFAULT_MAX_CHARTS`] and replacing the
//! stalest chart. [`resolved_max_charts`] and [`resolved_max_charts_evict`] are the one place that
//! default is applied, so the runtime and the popup that shows it can never disagree.

use gpui::*;

use super::super::stack::{SlotOwner, resolve_layout};
use super::AddChartStack;
use moon_core::session::CoreId;

/// How many charts the detect feed may keep on a tab that names no cap of its own.
///
/// Eight is the most a stack tab can still show as CHARTS rather than slivers — Fit splits a
/// ~900 px tab into ~110 px rows, and Scroll's `DEFAULT_SCROLL_HEIGHT` of 300 px puts three on
/// screen with one short scroll — while bounding a 1100-detect storm at eight GPU canvases, eight
/// history requests and eight live subscriptions per tab instead of the low thousands.
pub(in crate::chart_tabs) const DEFAULT_MAX_CHARTS: u16 = 8;

/// What an unconfigured tab does once that cap is reached: take the stalest slot rather than let
/// the detect go unshown. A cap that silently drops detects reads as the feed having stopped, and
/// the chart longest without a detect is the one the reader is least likely to be watching.
pub(in crate::chart_tabs) const DEFAULT_MAX_CHARTS_EVICT: bool = true;

/// Resolve a tab's stored cap into the number actually in force.
///
/// `None` is "never configured" — every `charts.json` written before the setting existed — and
/// takes [`DEFAULT_MAX_CHARTS`]. `Some(0)` is the EXPLICIT "no cap" the popup's hint names, and is
/// returned as zero so it survives a save and reload unchanged.
///
/// This is the one place the default is applied. Every reader that must agree with the runtime —
/// the popup's number field, the diag lines, the admission rule — goes through it, while the
/// accessors and the persisted spec keep the raw `Option`, which is what lets "not configured"
/// stay distinguishable from "configured to today's default".
///
/// Args:
///     max: Raw cap stored for the tab.
///
/// Returns:
///     The effective cap, including the built-in default for `None`.
pub(in crate::chart_tabs) fn resolved_max_charts(max: Option<u16>) -> u16 {
    max.unwrap_or(DEFAULT_MAX_CHARTS)
}

/// Resolve a tab's stored "replace the stalest" flag, `None` taking [`DEFAULT_MAX_CHARTS_EVICT`].
///
/// Args:
///     evict: Raw eviction policy stored for the tab.
///
/// Returns:
///     The effective policy, including the built-in default for `None`.
pub(in crate::chart_tabs) fn resolved_max_charts_evict(evict: Option<bool>) -> bool {
    evict.unwrap_or(DEFAULT_MAX_CHARTS_EVICT)
}

/// What a detect should do when it wants to put a NEW chart on this tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::chart_tabs) enum Admission {
    /// Room to spare — open it the ordinary way.
    Accept,
    /// At the cap: free the chart in this slot and take the room it leaves.
    Evict(usize),
    /// At the cap with nothing that may give way — the detect goes unshown.
    Drop,
}

/// Decide a detect's fate from the cap alone. Pure, so the rule is testable without a live stack.
///
/// `slots` carries one entry per LIVE chart: its index and how stale it is IF it may be evicted at
/// all. `None` marks a chart that holds its place unconditionally — pinned, or opened by hand (see
/// `ChartStackEntry::eviction_rank_ms`). Both still count toward the cap, because both still occupy
/// the screen, but neither is ever the victim.
///
/// The victim is the STALEST chart: the one that has gone longest without a fresh detect, since
/// every repeat detect pushes its `born_ms` forward. Staleness rather than a TTL deadline, because
/// a chart whose strategy says `KeepInChart = 0` has no deadline at all and would otherwise read as
/// "never evictable" — on a tab where every chart is such a chart, nothing could ever give way and
/// the tab would stop showing new coins once full.
///
/// Where the two ORDERS differ — a tab mixing `KeepInChart` values — this deliberately picks the
/// least recently seen chart rather than the one closest to expiring.
pub(in crate::chart_tabs) fn admit(
    max: Option<u16>,
    evict: bool,
    slots: &[(usize, Option<f64>)],
) -> Admission {
    // No cap and a zero cap are the same thing: an unbounded stack. Callers resolve their tab's
    // stored setting first, so an unconfigured tab arrives here already carrying the default.
    let Some(cap) = max.filter(|m| *m > 0) else {
        return Admission::Accept;
    };
    if slots.len() < usize::from(cap) {
        return Admission::Accept;
    }
    if !evict {
        return Admission::Drop;
    }
    slots
        .iter()
        .filter_map(|(ix, stale_ms)| stale_ms.map(|s| (*ix, s)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(Admission::Drop, |(ix, _)| Admission::Evict(ix))
}

impl AddChartStack {
    /// Open a DETECT's chart, honoring this tab's cap.
    ///
    /// Separate from `add_coin` so the cap governs the detect feed ALONE: the manual paths — coin
    /// search, custom tabs, a detached window's own field — keep calling `add_coin` and open what
    /// the reader explicitly asked for, cap or no cap.
    pub(in crate::chart_tabs) fn add_detect_coin(
        &mut self,
        core: CoreId,
        market: &str,
        ttl_ms: f64,
        cx: &mut Context<Self>,
    ) {
        match self.admit_detect(core, market, cx) {
            Admission::Accept => {}
            Admission::Drop => {
                // The message costs a walk of every panel entity, so it is built only when the
                // channel that would print it is on: this runs on the ingest path, once per
                // dropped detect, and a busy feed drops many.
                if moon_core::detect_diag::enabled() {
                    moon_core::detect_diag::line(&format!(
                        "[limit] n={} {market}: at cap {} (live={}), detect NOT shown",
                        self.num,
                        resolved_max_charts(self.max_charts),
                        self.pane_count(cx)
                    ));
                }
                return;
            }
            Admission::Evict(i) => {
                if moon_core::detect_diag::enabled() {
                    let victim = &self.charts[i];
                    moon_core::detect_diag::line(&format!(
                        "[limit] n={} {market}: at cap {}, replacing stalest {}/{}",
                        self.num,
                        resolved_max_charts(self.max_charts),
                        victim.core,
                        victim.market
                    ));
                }
                self.retire_slot(i, cx);
            }
        }
        self.open_coin(core, market, ttl_ms, SlotOwner::DetectFeed, cx);
    }

    /// Decide a detect's fate against the cap, reading how stale each live chart is.
    fn admit_detect(&self, core: CoreId, market: &str, cx: &App) -> Admission {
        // Only a tab that ASKED to be uncapped skips the walk now. An unconfigured tab is capped,
        // so the walk below is what an ordinary tab pays per NEW detect: a bounded number of slot
        // reads and one small allocation, against the `ChartPanel` — GPU canvas, history request,
        // live subscription — that a storm would otherwise open for every one of them.
        let cap = resolved_max_charts(self.max_charts);
        if cap == 0 {
            return Admission::Accept;
        }
        // A detect for a chart already ON SCREEN only extends its TTL — it opens nothing and can
        // never breach the cap. "On screen" by the same measure the slots below are counted with: a
        // slot whose panel has emptied is not showing anything, and reviving it is an arrival that
        // has to face the cap like any other.
        if self
            .charts
            .iter()
            .any(|e| e.core == core && e.market == market && e.is_live(cx))
        {
            return Admission::Accept;
        }
        let slots: Vec<(usize, Option<f64>)> = self
            .charts
            .iter()
            .enumerate()
            .filter(|(_, e)| e.is_live(cx))
            .map(|(ix, e)| (ix, e.eviction_rank_ms(cx)))
            .collect();
        admit(
            Some(cap),
            resolved_max_charts_evict(self.max_charts_evict),
            &slots,
        )
    }

    /// Free slot `i` for the chart about to take its place.
    ///
    /// Deliberately NOT `close_all_panes` plus `prune_or_hold`: that pair reads the whole stack, so
    /// evicting the only chart of a cap-of-one tab trips the "every slot empty" rule and clears the
    /// list, forcing a fresh `ChartPanel` — a new GPU canvas and a new history request — for every
    /// capped detect. Retiring one slot keeps the panel alive to be taken over.
    ///
    /// Done synchronously rather than left to the panel's `observe`: the newcomer must not go in
    /// while the outgoing chart still counts, which would breach the cap for a frame.
    fn retire_slot(&mut self, i: usize, cx: &mut Context<Self>) {
        let (_, compress, _) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        self.charts[i]
            .panel
            .clone()
            .update(cx, |p, pcx| p.close_all_panes(pcx));
        if compress && self.hold_vacated {
            // COMPRESS holds positions: this slot becomes a held placeholder and the neighbors
            // neither move nor resize. The newcomer takes the FIRST held slot, which is this one
            // only when no other was already standing empty — the cap is a count, not a seating
            // plan, and COMPRESS exists so that charts do not shuffle.
            self.charts[i].vacated = true;
        } else {
            self.charts.remove(i);
        }
        self.touch_count_change();
    }
}

#[cfg(test)]
mod tests;
