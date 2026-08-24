//! Runtime durable closed-trade loading for one exact Main-chart target.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::*;
use moon_core::db::{self, ChartTradeHistory, ReadFail, ReportFilter};
use moon_core::session::CoreId;

use super::ChartPanel;
use crate::backend::ChartHistoryScope;

/// Maximum durable rows drawn for one Main chart.
const HISTORY_LIMIT: usize = 1_000;

/// Minimum interval between durable refresh reads triggered by report commits.
const HISTORY_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// Minimum spacing between report-generation refreshes on a BACKGROUND chart tile.
///
/// One report generation would otherwise start one durable read per open tile, and a stack can hold
/// dozens. The foreground chart keeps the 5 s edge because it is the one being read; a tile in the
/// corner of a stack is not worth a fresh SQLite connection six times a minute.
const HISTORY_REFRESH_INTERVAL_BACKGROUND: Duration = Duration::from_secs(30);

/// Visible durable-history load state for a Main chart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum ReportTradesStatus {
    /// No exact target has requested durable history yet.
    #[default]
    Idle,
    /// The SQLite snapshot is being read in the background.
    Loading,
    /// At least one exact-scope closed trade is rendered.
    ///
    /// Carries no count: the marks themselves are what state the history, and a settled read is not
    /// stated in the overlay row at all, so a tally here would be data assembled for no reader.
    Ready,
    /// The exact query completed successfully with no closed trades.
    Empty,
    /// The report replica or selected core catalog is not ready.
    NotReady,
    /// The durable read failed without disabling the live chart.
    Failed,
}

/// Request token, exact target, and visible status owned by one chart panel.
#[derive(Default)]
pub(super) struct ReportTradesState {
    sequence: u64,
    target: Option<(CoreId, String)>,
    scope: ChartHistoryScope,
    last_refresh_start: Option<Instant>,
    refresh_timer_armed: bool,
    refresh_timer_token: u64,
    /// Whether the settings behind the current history admitted ANY trade kind.
    ///
    /// The checkboxes filter at DRAWING time (`ChartTradeRecord::emulator` is carried per row), so
    /// ticking one costs no read at all. The one transition that does is between "nothing is drawn"
    /// — where the read is skipped outright, because a set nobody will draw is not worth a round
    /// trip — and "something is drawn", which needs the set that was never fetched. Remembering the
    /// single boolean is what makes that re-read fire on that transition and on nothing else.
    last_admitted_any: Option<bool>,
    pub(super) status: ReportTradesStatus,
}

/// Whether a panel's settings admit ANY closed trade at all.
///
/// Both checkboxes clear is the one setting that changes what is READ rather than what is drawn:
/// with nothing to draw, the durable query is skipped outright instead of fetching a set that would
/// be filtered away to nothing. Every other combination reads the same set and differs only in the
/// drawing filter, which is why ticking a single box costs no database work.
///
/// Args:
///     graphics: The panel's effective chart-drawing settings.
///
/// Returns:
///     Whether at least one trade kind is drawn.
fn draws_any_trade_kind(graphics: &moon_core::config::ChartGraphicsCfg) -> bool {
    graphics.show_real_trades || graphics.show_emulator_trades
}

/// Read one durable history snapshot without touching GPUI state.
///
/// Args:
///     core: Exact runtime core that owns the chart.
///     exact_coins: Case-insensitive exact database coin identities for the market.
///     filter: Optional published Report filter refinement.
///
/// Returns:
///     Bounded durable records and truncation state.
///
/// Errors:
///     Propagates report-replica readiness, snapshot, schema, and SQL failures.
fn load_history(
    core: CoreId,
    exact_coins: Vec<String>,
    filter: Option<ReportFilter>,
) -> db::ReadResult<ChartTradeHistory> {
    let conn = db::open_reader()?;
    let snapshot = db::read_snapshot(&conn)?;
    db::query_chart_trade_history(
        &snapshot,
        core,
        &exact_coins,
        filter.as_ref(),
        HISTORY_LIMIT,
    )
}

/// Decide whether one asynchronous result still belongs to the panel's latest exact request.
///
/// Args:
///     expected_sequence: Sequence captured before background dispatch.
///     current_sequence: Latest sequence owned by the panel.
///     expected_target: Exact core and market captured before dispatch.
///     current_target: Latest exact target owned by the panel.
///
/// Returns:
///     `true` only when neither scope sequence nor exact target changed.
fn history_result_is_current(
    expected_sequence: u64,
    current_sequence: u64,
    expected_target: &(CoreId, String),
    current_target: Option<&(CoreId, String)>,
) -> bool {
    expected_sequence == current_sequence && current_target == Some(expected_target)
}

impl ChartPanel {
    /// Install durable markers and force the shared userdata union to rebuild while visible.
    ///
    /// Args:
    ///     records: Exact-target durable record set.
    ///     cx: Panel context used to force visible order/userdata synchronization.
    ///
    /// Returns:
    ///     Nothing; unchanged records leave the existing buffer intact.
    pub(crate) fn publish_trade_history(
        &mut self,
        records: Rc<Vec<moon_core::db::ChartTradeRecord>>,
        cx: &mut Context<Self>,
    ) {
        if self.chart.set_trade_history(records) {
            self.sync_orders_if_visible(cx, true);
            self.view_dirty = true;
        }
    }

    /// Start one exact-target durable read, optionally replacing visible state and refocusing.
    ///
    /// Args:
    ///     core: Exact runtime core captured by the producer.
    ///     market: Catalog-verified canonical market.
    ///     scope: Default or published Report history scope.
    ///     replace_visible: Whether loading/errors clear the prior marker set.
    ///     cx: Panel context used for background work and publication.
    ///
    /// Returns:
    ///     Nothing; stale completions are rejected by sequence and target.
    fn load_history_scope(
        &mut self,
        core: CoreId,
        market: String,
        scope: ChartHistoryScope,
        replace_visible: bool,
        cx: &mut Context<Self>,
    ) {
        let mut exact_coins = vec![market.clone()];
        let label_coin = self
            .backend
            .read(cx)
            .session
            .market_source()
            .market_label(core, &market)
            .coin;
        let catalog_ready = !label_coin.is_empty();
        let default_needs_catalog = matches!(scope, ChartHistoryScope::Default);
        if catalog_ready
            && !exact_coins
                .iter()
                .any(|coin| coin.eq_ignore_ascii_case(&label_coin))
        {
            exact_coins.push(label_coin);
        }
        let (filter, report_coin) = match &scope {
            ChartHistoryScope::Default => (None, None),
            ChartHistoryScope::Report {
                filter, exact_coin, ..
            } => (Some(filter.clone()), Some(exact_coin.clone())),
        };
        if let Some(report_coin) = report_coin.filter(|coin| !coin.trim().is_empty())
            && !exact_coins
                .iter()
                .any(|coin| coin.eq_ignore_ascii_case(&report_coin))
        {
            exact_coins.push(report_coin);
        }

        // THIS panel's effective settings: the popup is per tab, so two tabs on the same market can
        // legitimately draw different sets.
        //
        // The trade-kind checkboxes deliberately do NOT narrow this query — see
        // `ChartTradeRecord::emulator`: the row cap is applied after the predicate, so filtering
        // here would let a checkbox decide which trades the history CONTAINS, freeing slots under
        // the cap and surfacing older trades of the kept kind that had been truncated away. The
        // price of that choice is the mirror image: with one box clear, the drawn set is that
        // kind's share of the newest rows rather than a full cap of them. The drawing filter
        // in `chartdx/trade_history_sync.rs` is the one place that reads them. The Report scope's
        // own `filter.emulator` is a different thing and travels untouched: it says which rows the
        // user asked to see, not how they are drawn.
        let draws_any_kind = {
            let graphics = self.effective_chart_graphics(cx);
            draws_any_trade_kind(&graphics)
        };

        self.report_trades.sequence = self.report_trades.sequence.wrapping_add(1);
        let sequence = self.report_trades.sequence;
        self.report_trades.target = Some((core, market.clone()));
        self.report_trades.scope = scope.clone();
        self.report_trades.last_admitted_any = Some(draws_any_kind);
        self.report_trades.last_refresh_start = Some(Instant::now());
        if !draws_any_kind {
            // Both checkboxes are clear, so nothing would be drawn from this set: skip the round
            // trip entirely. The visible set is cleared whatever `replace_visible` says — the user
            // asked for no trades, and leaving the previous ones drawn would answer the opposite.
            self.report_trades.status = ReportTradesStatus::Empty;
            self.publish_trade_history(Rc::new(Vec::new()), cx);
            cx.notify();
            return;
        }
        if default_needs_catalog && !catalog_ready {
            // Stamped before the early return as well, or the NotReady rate limit below is inert and
            // every re-add repeats the label lookup and the republish.
            self.report_trades.status = ReportTradesStatus::NotReady;
            if replace_visible {
                self.publish_trade_history(Rc::new(Vec::new()), cx);
            }
            cx.notify();
            return;
        }
        self.report_trades.refresh_timer_armed = false;
        self.report_trades.refresh_timer_token =
            self.report_trades.refresh_timer_token.wrapping_add(1);
        if replace_visible {
            self.report_trades.status = ReportTradesStatus::Loading;
            self.publish_trade_history(Rc::new(Vec::new()), cx);
            cx.notify();
        }

        crate::diag::bump(&crate::diag::CHART_TRADE_HISTORY_READS);
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move { load_history(core, exact_coins, filter) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if !history_result_is_current(
                        sequence,
                        this.report_trades.sequence,
                        &(core, market.clone()),
                        this.report_trades.target.as_ref(),
                    ) {
                        return;
                    }
                    match result {
                        Ok(history) if history.records.is_empty() => {
                            this.report_trades.status = ReportTradesStatus::Empty;
                            this.publish_trade_history(Rc::new(Vec::new()), cx);
                        }
                        Ok(history) => {
                            // NO VIEWPORT CHANGE HERE, and that is the decision rather
                            // than an omission. Clicking a coin in the Report opens that COIN's
                            // chart; the trades are drawn on it as markers, but the reader asked
                            // for the market, not for one position. Moving or rescaling their
                            // chart on their behalf takes it off the live edge and changes a view
                            // they own, to show them something the markers already show.
                            //
                            // The double-click path is the one that frames a trade, and it does so
                            // in its OWN window, which exists for exactly that. Keeping the two
                            // apart is the point: this one stays an ordinary live chart.
                            this.report_trades.status = ReportTradesStatus::Ready;
                            this.publish_trade_history(Rc::new(history.records), cx);
                        }
                        Err(ReadFail::NotReady) => {
                            // Re-stamped at the FAILURE, not at the request: a read slower than the
                            // interval would otherwise leave the retry guard already expired.
                            this.report_trades.last_refresh_start = Some(Instant::now());
                            this.report_trades.status = ReportTradesStatus::NotReady;
                            if replace_visible {
                                this.publish_trade_history(Rc::new(Vec::new()), cx);
                            }
                        }
                        Err(error) => {
                            log::warn!("chart trade history read failed: {error}");
                            this.report_trades.last_refresh_start = Some(Instant::now());
                            this.report_trades.status = ReportTradesStatus::Failed;
                            if replace_visible {
                                this.publish_trade_history(Rc::new(Vec::new()), cx);
                            }
                        }
                    }
                    this.view_dirty = true;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Apply default or Report-refined history to an exact Main core and market.
    ///
    /// Args:
    ///     core: Exact runtime core captured by the producer.
    ///     market: Catalog-verified canonical market.
    ///     scope: Default or published Report history scope.
    ///     cx: Panel context used to start the load.
    ///
    /// Returns:
    ///     Nothing; the explicit request replaces prior history scope.
    pub(crate) fn apply_history_scope(
        &mut self,
        core: CoreId,
        market: String,
        scope: ChartHistoryScope,
        cx: &mut Context<Self>,
    ) {
        if self.history_request_is_redundant(core, &market, &scope) {
            return;
        }
        self.load_history_scope(core, market, scope, true, cx);
    }

    /// Point this panel's durable history at a market WITHOUT moving its camera.
    ///
    /// The focusing variant above belongs to an explicit user action — opening a market on Main, or
    /// clicking a Report row — where jumping the view to the trade being inspected IS the request.
    /// A chart tile that a detect just put on screen is the opposite case: it is showing the live
    /// edge, and focusing it on the newest closed trade would pull it off the live edge permanently
    /// (`show_time_range` ends in a persistent manual view).
    ///
    /// Args:
    ///     core: Core that owns the market.
    ///     market: Canonical market name.
    ///     cx: Panel context used to start the load.
    ///
    /// Returns:
    ///     Nothing; a redundant request for the settled target does no work.
    pub(crate) fn track_history_scope(
        &mut self,
        core: CoreId,
        market: String,
        cx: &mut Context<Self>,
    ) {
        let scope = ChartHistoryScope::Default;
        if self.history_request_is_redundant(core, &market, &scope) {
            return;
        }
        self.load_history_scope(core, market, scope, true, cx);
    }

    /// Whether a history request for this target would repeat work already done or under way.
    ///
    /// `Loading | Ready | Empty` are settled: the answer is either in hand or on its way. The two
    /// FAILURE states are not settled — they must be retried — but not on demand: an unavailable
    /// replica would otherwise turn a busy detect feed, which re-adds the same market to extend its
    /// TTL, into one `open_reader` per detection. They are rate-limited to the same interval a
    /// report-generation refresh uses, and the report-revision observer retries them anyway.
    fn history_request_is_redundant(
        &self,
        core: CoreId,
        market: &str,
        scope: &ChartHistoryScope,
    ) -> bool {
        let same_target = self
            .report_trades
            .target
            .as_ref()
            .is_some_and(|target| target.0 == core && target.1 == market);
        if !same_target || &self.report_trades.scope != scope {
            return false;
        }
        match self.report_trades.status {
            ReportTradesStatus::Loading | ReportTradesStatus::Ready | ReportTradesStatus::Empty => {
                true
            }
            ReportTradesStatus::NotReady | ReportTradesStatus::Failed => self
                .report_trades
                .last_refresh_start
                .is_some_and(|started| started.elapsed() < HISTORY_REFRESH_INTERVAL),
            ReportTradesStatus::Idle => false,
        }
    }

    /// Coalesce report-generation refreshes to at most one durable read every five seconds.
    ///
    /// Args:
    ///     cx: Panel context used to arm a trailing refresh timer or start a due read.
    ///
    /// Returns:
    ///     Nothing when no exact target exists; otherwise retains one trailing refresh edge.
    pub(super) fn requery_trade_history_on_generation(&mut self, cx: &mut Context<Self>) {
        if self.report_trades.target.is_none() {
            return;
        }
        // A panel drawing no trade kind at all has nothing to refresh: its set is empty by request,
        // and re-running the skip would only wake the panel once per generation.
        if self.report_trades.last_admitted_any == Some(false) {
            return;
        }
        let interval = self.history_refresh_interval();
        let elapsed = self
            .report_trades
            .last_refresh_start
            .map(|started| started.elapsed())
            .unwrap_or(interval);
        if elapsed >= interval {
            self.refresh_trade_history(cx);
            return;
        }
        if self.report_trades.refresh_timer_armed {
            return;
        }
        let wait = interval.saturating_sub(elapsed);
        self.report_trades.refresh_timer_armed = true;
        self.report_trades.refresh_timer_token =
            self.report_trades.refresh_timer_token.wrapping_add(1);
        let timer_token = self.report_trades.refresh_timer_token;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(wait).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if !this.report_trades.refresh_timer_armed
                        || this.report_trades.refresh_timer_token != timer_token
                    {
                        return;
                    }
                    this.report_trades.refresh_timer_armed = false;
                    this.refresh_trade_history(cx);
                });
            });
        })
        .detach();
    }

    /// How often this panel re-reads durable history when report generations commit.
    ///
    /// The foreground chart is the one the user is reading; background tiles trade freshness for the
    /// per-tile cost of a durable read, because a generation reaches every one of them at once.
    fn history_refresh_interval(&self) -> Duration {
        if self.fast {
            HISTORY_REFRESH_INTERVAL
        } else {
            HISTORY_REFRESH_INTERVAL_BACKGROUND
        }
    }

    /// Re-read durable history when the graphics popup changes which trade kinds are drawn.
    ///
    /// Guarded on the PAIR itself rather than on the settings signature: that signature also moves for
    /// a theme edit or an arrow-size step, and a SQLite read per theme change is not what a checkbox
    /// asked for. Two booleans compared per observer fire is the steady-state cost, and the observer
    /// is a GPUI notification raised by the popup — not a present tick, not a scroll — so this never
    /// runs at live-scroll or mousemove frequency.
    ///
    /// No coalescing, unlike `requery_trade_history_on_generation`: this input is a human ticking a
    /// box, not the report generator.
    ///
    /// Args:
    ///     cx: Panel context used to start a non-clearing refresh.
    ///
    /// Returns:
    ///     Nothing; idle panels and unchanged settings do no work.
    pub(super) fn requery_trade_history_on_trade_kinds(&mut self, cx: &mut Context<Self>) {
        if self.report_trades.target.is_none() {
            return;
        }
        let graphics = self.effective_chart_graphics(cx);
        let admits_any = draws_any_trade_kind(&graphics);
        // `None` means no read has settled yet, and the read that does will stamp this itself.
        // Ticking one box while the other is already on changes only the drawing filter, so the
        // common case leaves here without touching the database.
        if self.report_trades.last_admitted_any == Some(admits_any) {
            return;
        }
        self.refresh_trade_history(cx);
    }

    /// Drop the history target when this panel no longer shows the market it belongs to.
    ///
    /// A stale target is not inert: every refresh edge — a report generation, a trade-kind change —
    /// would start a read for a market this panel stopped drawing, and the records would sit in
    /// memory for the panel's whole life. A retained COMPRESS slot is exactly this case: it keeps
    /// its panel while showing nothing.
    ///
    /// Args:
    ///     cx: Panel context used to clear the drawn set.
    ///
    /// Returns:
    ///     Nothing; a target the panel still shows is left alone.
    pub(super) fn clear_history_target_if_unused(&mut self, cx: &mut Context<Self>) {
        let Some((core, market)) = self.report_trades.target.clone() else {
            return;
        };
        if self.chart.uses_market(core, &market) {
            return;
        }
        self.report_trades.target = None;
        self.report_trades.scope = ChartHistoryScope::Default;
        self.report_trades.last_admitted_any = None;
        self.report_trades.status = ReportTradesStatus::Idle;
        // Bump the sequence so a read still in flight for that market cannot land afterwards.
        self.report_trades.sequence = self.report_trades.sequence.wrapping_add(1);
        self.publish_trade_history(Rc::new(Vec::new()), cx);
        cx.notify();
    }

    /// Refresh the current exact history after a committed Report generation without refocusing.
    ///
    /// Args:
    ///     cx: Panel context used to start a non-clearing refresh.
    ///
    /// Returns:
    ///     Nothing when idle; otherwise starts an exact-target refresh.
    pub(super) fn refresh_trade_history(&mut self, cx: &mut Context<Self>) {
        let Some((core, market)) = self.report_trades.target.clone() else {
            return;
        };
        self.load_history_scope(
            core,
            market,
            self.report_trades.scope.clone(),
            false,
            cx,
        );
    }

    /// Retry the currently captured exact target without consulting global active-core state.
    ///
    /// Args:
    ///     cx: Panel context used to restart the current request.
    ///
    /// Returns:
    ///     Nothing when idle; otherwise replaces visible state and retries without refocusing.
    pub(super) fn retry_trade_history(&mut self, cx: &mut Context<Self>) {
        let Some((core, market)) = self.report_trades.target.clone() else {
            return;
        };
        self.load_history_scope(
            core,
            market,
            self.report_trades.scope.clone(),
            true,
            cx,
        );
    }
}

#[cfg(test)]
mod tests;
