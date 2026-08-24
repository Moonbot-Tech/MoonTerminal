//! The per-tab detect cap: how many charts the DETECT feed may keep on one tab, and what a detect
//! does once that number is reached.
//!
//! The rule itself is the pure [`admit`] below, tested without a live stack; [`AddChartStack`]'s
//! half is only the entity work around it — reading each slot, freeing one, and logging what was
//! decided. Manual paths do not come through here at all: the cap governs the feed, and a coin the
//! reader typed is not the feed's to close.

use gpui::*;

use super::super::stack::{SlotOwner, resolve_layout};
use super::AddChartStack;
use moon_core::session::CoreId;

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
/// `slots` carries one entry per LIVE chart: its index and its deadline IF it may be evicted at
/// all. `None` marks a chart that holds its place unconditionally — pinned, or opened by hand (see
/// `ChartStackEntry::evictable_deadline`). Both still count toward the cap, because both still
/// occupy the screen, but neither is ever the victim.
///
/// The victim is the EARLIEST deadline, which is the chart that has gone longest without a fresh
/// detect — every repeat detect pushes its `born_ms` forward — and therefore the one that would
/// have expired first anyway. That is what makes eviction feel like the TTL arriving early rather
/// than like an arbitrary chart vanishing.
pub(in crate::chart_tabs) fn admit(
    max: Option<u16>,
    evict: bool,
    slots: &[(usize, Option<f64>)],
) -> Admission {
    // No cap and a zero cap are the same thing, and both are what a tab that never touched the
    // setting does: an unbounded stack, exactly as before the setting existed.
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
        .filter_map(|(ix, deadline)| deadline.map(|d| (*ix, d)))
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
                        self.max_charts.unwrap_or(0),
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
                        self.max_charts.unwrap_or(0),
                        victim.core,
                        victim.market
                    ));
                }
                self.retire_slot(i, cx);
            }
        }
        self.open_coin(core, market, ttl_ms, SlotOwner::DetectFeed, cx);
    }

    /// Decide a detect's fate against the cap, reading each live chart's eviction deadline.
    fn admit_detect(&self, core: CoreId, market: &str, cx: &App) -> Admission {
        // Uncapped is the default and every existing `charts.json`, so it costs nothing: no walk of
        // the stack, no entity reads, no allocation on the ingest path.
        if self.max_charts.filter(|m| *m > 0).is_none() {
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
            .map(|(ix, e)| (ix, e.evictable_deadline(cx)))
            .collect();
        admit(
            self.max_charts,
            self.max_charts_evict.unwrap_or(false),
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
