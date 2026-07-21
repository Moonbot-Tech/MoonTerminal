//! Background loads of the "By coin" mode: the per-coin aggregates, the selected strategies'
//! saved lists, and the debounced rescan of the "Fact vs v1" matrix.
//!
//! Split from the rendering (`coins/mod.rs`) so the two halves stay legible on their own —
//! this file only ever WRITES `CoinsState`, the rendering only ever reads it.

use std::time::Duration;

use gpui::*;

use super::super::super::AnalyticsView;
use super::state::{CoinLists, CoinPlan};
use moon_core::db::analytics::GroupStat;

/// How long a burst of ticks is allowed to keep coalescing before the KPI is rescanned.
///
/// Each recompute is a full scan of the period, and editing the lists is this mode's main
/// gesture: ticking twenty coins must cost one scan, not twenty.
const KPI_DEBOUNCE: Duration = Duration::from_millis(350);

impl AnalyticsView {
    /// Recompute the coin table: the per-coin aggregates of the current scope, the
    /// selected strategies' coin lists, and the KPI matrix — one background pass.
    pub(in crate::analytics) fn reload_coins(&mut self, cx: &mut Context<Self>) {
        self.coins.seq = self.coins.seq.wrapping_add(1);
        self.coins.kpi_seq = self.coins.kpi_seq.wrapping_add(1);
        let req = self.coins.seq;
        let kpi_req = self.coins.kpi_seq;
        let q = self.tuner_query();
        let kpi_q = q.clone();
        // The variant expands its coin tokens against the BASE coin set — the same rows
        // the table draws, so the scan and the table always mean the same coins.
        let universe = self.coin_universe();
        // Planning needs the coin set; without it the pass still refreshes the table, but
        // must not clear `dirty` — see the completion handler.
        let universe_empty = universe.is_empty();
        // Lists come from EVERY selected strategy, not just the anchor: the set is tuned
        // and written as one, so its lists are one union — and a union BY COIN, because
        // summing sizes would count a coin twice for every pair that happens to share it.
        let targets: Vec<(i64, Option<u64>)> = self
            .selected_targets()
            .into_iter()
            .map(|t| (t.sid, t.core))
            .collect();
        // The picker reads the SAME strategies, so it rides this pass rather than opening
        // the database a second time. Its own generation guards it, though:
        // `CoinListsState::invalidate` must be able to retire it on its own.
        self.coin_lists.seq = self.coin_lists.seq.wrapping_add(1);
        let lists_req = self.coin_lists.seq;
        let list_targets = targets.clone();
        self.coin_lists.rows.begin();
        self.coins.stats.begin();
        self.coins.kpi.begin();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let (stats, kpi, lists, bl_n, wl_n, entries) = executor
                .spawn(async move {
                    let entries = moon_core::db::coin_lists::coin_lists(&list_targets);
                    let stats = moon_core::db::analytics::coin_groups(&q);
                    // An absent strategy DB or a kind without the fields yields empty
                    // lists, which read as "these strategies list no coins" — true.
                    let mut lists = CoinLists::default();
                    for (sid, core) in targets {
                        let m = moon_core::db::tuner::strategy_current_values(
                            sid,
                            core,
                            &["CoinsBlackList".to_string(), "CoinsWhiteList".to_string()],
                        );
                        let get = |k: &str| m.get(k).map(String::as_str).unwrap_or("");
                        lists.merge(CoinLists::parse(
                            get("CoinsBlackList"),
                            get("CoinsWhiteList"),
                        ));
                    }
                    // The plan is built HERE, from the lists this pass just read: building
                    // it before the read scored the PREVIOUS lists, which after a scope
                    // change are empty — that is why v1 never appeared until the user
                    // clicked something.
                    let plan = CoinPlan::build(&lists, &universe);
                    let kpi = moon_core::db::tuner::variant_stats(&kpi_q, &plan.variants);
                    (stats, kpi, lists, plan.bl, plan.wl, entries)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    // Guarded separately from the table below: the panels have their own
                    // generation, so a scope change that retired only them still lands here
                    // without publishing their stale answer.
                    //
                    // A FAILED list read has to leave the pass stale like any other, or
                    // `needs_reload` never fires again and both panels sit on that error for
                    // the rest of the session.
                    //
                    // `NotReady` is deliberately NOT a failure here: it means there is no
                    // strategy database at all, which is a COMPLETED answer and one that will
                    // not change by asking again. Counting it would pin `dirty` on forever and
                    // make every entry into this mode re-run the full period scan behind the
                    // busy overlay, for a question already answered.
                    let lists_failed =
                        matches!(entries, Err(moon_core::db::ReadFail::Failed { .. }));
                    if this.coin_lists.seq == lists_req {
                        this.coin_lists.rows.apply(entries);
                    }
                    if this.coins.seq != req {
                        // The list read above may still have published under its own live
                        // generation; without this the picker holds new data that nothing
                        // asked to be drawn.
                        cx.notify();
                        return;
                    }
                    // An edit landing mid-flight bumps the KPI generation, and that is also
                    // what says "the user has touched the working lists since this request
                    // started" — so it must not be overwritten by a baseline read before it.
                    let edited = this.coins.kpi_seq != kpi_req;
                    // Clear "recompute me" only when everything this pass owed came back. The
                    // KPI counts only when it is actually applied — a discarded request's
                    // error says nothing about the table. An empty universe means the plan
                    // could not be built at all, so that too has to stay stale.
                    this.coins.dirty = stats.is_err()
                        || lists_failed
                        || universe_empty
                        || (!edited && kpi.is_err());
                    this.coins.stats.apply(stats);
                    // The edit is replayed onto the new baseline rather than re-seeded from
                    // it — see `adopt`. After a scope change `invalidate` has emptied both
                    // sets, so there is no delta and the new lists are adopted whole; the
                    // previous selection's picks cannot leak into this one.
                    this.coins.adopt(lists);
                    // A view filtered by a list that just came back empty has nothing
                    // left to show; fall back before the table renders "no matches".
                    this.coins.settle_filter();
                    if !edited {
                        this.coins.kpi.apply(kpi);
                        this.coins.kpi_bl = bl_n;
                        this.coins.kpi_wl = wl_n;
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// The BASE coin set: every coin the selected cores traded in the period.
    ///
    /// Cloned deliberately — it is handed to a background task, and it is read on the
    /// reload path (a few times per user action), never per frame.
    pub(in crate::analytics::tuner) fn coin_universe(&self) -> Vec<GroupStat> {
        self.data
            .data()
            .map(|d| d.coins.clone())
            .unwrap_or_default()
    }

    /// Click a coin: it becomes the selection, and the strategy list lights up whoever
    /// traded it. A read, not a filter — the table itself does not change.
    pub(in crate::analytics::tuner) fn pick_coin(
        &mut self,
        coin: String,
        multi: bool,
        cx: &mut Context<Self>,
    ) {
        self.coins.pick_coin(coin, multi);
        self.reload_picked_strats(cx);
        cx.notify();
    }

    /// Background: which strategies traded the picked coins.
    ///
    /// Not wrapped in `op_started`: this is a selection cue, and blanking the window behind a
    /// "loading" overlay for it would make clicking a coin feel heavier than the answer is.
    fn reload_picked_strats(&mut self, cx: &mut Context<Self>) {
        self.coins.picked_seq = self.coins.picked_seq.wrapping_add(1);
        let req = self.coins.picked_seq;
        if self.coins.picked.is_empty() {
            self.coins.picked_strats.clear();
            return;
        }
        let q = self.tuner_query_all();
        let mut coins: Vec<String> = self.coins.picked.iter().cloned().collect();
        coins.sort();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let hit = executor
                .spawn(async move { moon_core::db::analytics::strategies_for_coins(&q, &coins) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.coins.picked_seq != req {
                        return;
                    }
                    // A failed read leaves nothing highlighted rather than a stale set: the
                    // highlight is an assertion about these coins, and a wrong one is worse
                    // than none.
                    this.coins.picked_strats = hit.unwrap_or_default().into_iter().collect();
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Tick / untick a coin on one list. The lists drive variant v1, so the matrix is
    /// rescanned — after the burst settles.
    ///
    /// Keyed by TOKEN, not by report name: a strategy list naming `BTC` covers `BTC_RP`
    /// and `BTC_1230` alike, so ticking either contract ticks the coin on every row of it.
    pub(in crate::analytics::tuner) fn toggle_coin_list(
        &mut self,
        black_side: bool,
        token: String,
        cx: &mut Context<Self>,
    ) {
        self.coins.toggle(black_side, token);
        self.arm_coin_kpi(cx);
        cx.notify();
    }

    /// THE single entry point for "the pick set changed": retire whatever matrix is pending
    /// or in flight (it describes the previous set), then wait out the burst before paying
    /// for a scan. Every edit of the picks goes through here — a caller that armed its own
    /// rescan would be the one gesture that skips the debounce.
    pub(in crate::analytics::tuner) fn arm_coin_kpi(&mut self, cx: &mut Context<Self>) {
        let req = self.coins.bump_kpi();
        // Storing the task REPLACES the previous one, and dropping a gpui `Task` cancels it:
        // a burst of ticks leaves one live timer rather than one per click.
        self.coins.kpi_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(KPI_DEBOUNCE).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    // A later edit landed during the wait — that one owns the recompute.
                    if this.coins.kpi_seq == req {
                        this.run_coin_kpi(req, cx);
                    }
                });
            });
        }));
    }

    /// Rescan ONLY the "Fact vs v1" matrix — all a tick changes. The table's numbers and
    /// the coin lists do not depend on the picks, so re-running them here would pay for a
    /// full per-coin grouping to redraw one column.
    ///
    /// Deliberately NOT wrapped in `op_started`: the blocking "loading" overlay carries
    /// `occlude()`, and arming it on the mode's main gesture makes the second tick
    /// unclickable until the first scan lands. The matrix shows its own loading state.
    fn run_coin_kpi(&mut self, req: u64, cx: &mut Context<Self>) {
        let q = self.tuner_query();
        let universe = self.coin_universe();
        if universe.is_empty() {
            // The coin set is not known, so the lists cannot be expanded into a scan. Stay
            // stale and let the full reload (which waits for that set) do it, rather than
            // publishing a matrix planned against nothing.
            self.coins.dirty = true;
            return;
        }
        let plan = self.coins.plan(&universe);
        let (bl_n, wl_n) = (plan.bl, plan.wl);
        let variants = plan.variants;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let kpi = executor
                .spawn(async move { moon_core::db::tuner::variant_stats(&q, &variants) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.coins.kpi_seq != req {
                        return;
                    }
                    // Same rule as the full reload: a failed read leaves the panel stale so
                    // `needs_reload` re-arms it, instead of pinning the matrix to an error.
                    if kpi.is_err() {
                        this.coins.dirty = true;
                    }
                    this.coins.kpi.apply(kpi);
                    this.coins.kpi_bl = bl_n;
                    this.coins.kpi_wl = wl_n;
                    cx.notify();
                });
            });
        })
        .detach();
    }
}
