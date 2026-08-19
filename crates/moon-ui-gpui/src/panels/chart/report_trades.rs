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
    /// The `(show_real_trades, show_emulator_trades)` pair the current history was read with.
    ///
    /// The two checkboxes narrow the durable read itself rather than the drawing, because the
    /// real-vs-emulator flag never reaches the terminal per row — it exists only as a predicate on
    /// the report replica — so a toggle has to re-read. Remembering the pair is what makes that
    /// re-read fire on a real change and on nothing else.
    last_trade_kinds: Option<(bool, bool)>,
    pub(super) status: ReportTradesStatus,
}

/// Which emulator kinds one durable-history read may return.
///
/// Three-valued because `ReportFilter::emulator` is an `Option<bool>` and cannot say "neither", while
/// both checkboxes off is a legal and reachable UI state. Collapsing that onto `None` would show
/// EVERY trade at the exact moment the user asked for none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EmulatorAdmission {
    /// No predicate at all: real and emulator alike.
    All,
    /// Emulator only (`true`) or real only (`false`).
    Only(bool),
    /// No row can match, so the read is skipped entirely.
    Nothing,
}

/// Intersect the chart's two trade-kind checkboxes with the Report scope's own emulator predicate.
///
/// Both are independent narrowings of one set, so the answer is their CONJUNCTION. A chart opened
/// from a Report pinned to emulator trades, with the emulator checkbox unticked, admits nothing —
/// answering `Some(true)` there would put back on the chart precisely the rows the user just hid,
/// and answering `Some(false)` would show rows the Report scope excluded. Neither widening is
/// acceptable, so the contradiction is named instead of resolved.
///
/// Args:
///     show_real: Whether real (non-emulator) trades are drawn.
///     show_emulator: Whether emulator trades are drawn.
///     scope: The published Report scope's own emulator predicate, if it carries one.
///
/// Returns:
///     The predicate one read must carry, or [`EmulatorAdmission::Nothing`].
fn admitted_emulator_kinds(
    show_real: bool,
    show_emulator: bool,
    scope: Option<bool>,
) -> EmulatorAdmission {
    // The scope expressed as the same pair, so the intersection is one boolean AND per kind rather
    // than a nine-arm match over two different encodings of the same fact.
    let (scope_real, scope_emulator) = match scope {
        None => (true, true),
        Some(false) => (true, false),
        Some(true) => (false, true),
    };
    match (show_real && scope_real, show_emulator && scope_emulator) {
        (true, true) => EmulatorAdmission::All,
        (true, false) => EmulatorAdmission::Only(false),
        (false, true) => EmulatorAdmission::Only(true),
        (false, false) => EmulatorAdmission::Nothing,
    }
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
    fn publish_trade_history(
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
    ///     focus: Whether the first successful result may focus one trade interval.
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
        focus: bool,
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
        let (mut filter, report_coin, focus_record_id) = match &scope {
            ChartHistoryScope::Default => (None, None, None),
            ChartHistoryScope::Report {
                filter,
                exact_coin,
                focus_record_id,
            } => (
                Some(filter.clone()),
                Some(exact_coin.clone()),
                *focus_record_id,
            ),
        };
        if let Some(report_coin) = report_coin.filter(|coin| !coin.trim().is_empty())
            && !exact_coins
                .iter()
                .any(|coin| coin.eq_ignore_ascii_case(&report_coin))
        {
            exact_coins.push(report_coin);
        }

        // Which trade kinds the graphics popup admits, intersected with whatever the Report scope
        // already asked for. Normalized first, for the same reason every other reader of this config
        // normalizes: `layout.toml` is hand-editable.
        let graphics =
            moon_chart::normalize_chart_graphics(self.backend.read(cx).layout.chart_graphics);
        let trade_kinds = (graphics.show_real_trades, graphics.show_emulator_trades);
        let admitted = admitted_emulator_kinds(
            trade_kinds.0,
            trade_kinds.1,
            filter.as_ref().and_then(|f| f.emulator),
        );

        self.report_trades.sequence = self.report_trades.sequence.wrapping_add(1);
        let sequence = self.report_trades.sequence;
        self.report_trades.target = Some((core, market.clone()));
        self.report_trades.scope = scope.clone();
        self.report_trades.last_trade_kinds = Some(trade_kinds);
        if admitted == EmulatorAdmission::Nothing {
            // Nothing can match, so no SQL round trip is worth making. The visible set is cleared
            // whatever `replace_visible` says: the user asked for no trades, and leaving the previous
            // ones drawn would answer the opposite.
            self.report_trades.status = ReportTradesStatus::Empty;
            self.publish_trade_history(Rc::new(Vec::new()), cx);
            cx.notify();
            return;
        }
        // A manufactured filter is not a widening: `query_chart_trade_history` itself does
        // `filter.cloned().unwrap_or_default()`, so `Some(ReportFilter::default())` and `None` are the
        // same query. Only `emulator` is being set here.
        match admitted {
            EmulatorAdmission::All => {
                if let Some(f) = filter.as_mut() {
                    f.emulator = None;
                }
            }
            EmulatorAdmission::Only(emulator) => {
                filter.get_or_insert_with(ReportFilter::default).emulator = Some(emulator);
            }
            EmulatorAdmission::Nothing => unreachable!("returned above"),
        }
        if default_needs_catalog && !catalog_ready {
            self.report_trades.status = ReportTradesStatus::NotReady;
            if replace_visible {
                self.publish_trade_history(Rc::new(Vec::new()), cx);
            }
            cx.notify();
            return;
        }
        self.report_trades.last_refresh_start = Some(Instant::now());
        self.report_trades.refresh_timer_armed = false;
        self.report_trades.refresh_timer_token =
            self.report_trades.refresh_timer_token.wrapping_add(1);
        if replace_visible {
            self.report_trades.status = ReportTradesStatus::Loading;
            self.publish_trade_history(Rc::new(Vec::new()), cx);
            cx.notify();
        }

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
                            if focus
                                && let Some(record) = focus_record_id
                                    .and_then(|id| {
                                        history.records.iter().find(|record| record.record_id == id)
                                    })
                                    .or_else(|| history.records.first())
                            {
                                this.chart.show_time_range(
                                    record.buy_date as f64 * 1_000.0,
                                    record.close_date as f64 * 1_000.0,
                                );
                            }
                            this.report_trades.status = ReportTradesStatus::Ready;
                            this.publish_trade_history(Rc::new(history.records), cx);
                        }
                        Err(ReadFail::NotReady) => {
                            this.report_trades.status = ReportTradesStatus::NotReady;
                            if replace_visible {
                                this.publish_trade_history(Rc::new(Vec::new()), cx);
                            }
                        }
                        Err(error) => {
                            log::warn!("chart trade history read failed: {error}");
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
        let same_target = self
            .report_trades
            .target
            .as_ref()
            .is_some_and(|target| target.0 == core && target.1 == market);
        let settled_same_scope = same_target
            && self.report_trades.scope == scope
            && matches!(
                self.report_trades.status,
                ReportTradesStatus::Loading | ReportTradesStatus::Ready | ReportTradesStatus::Empty
            );
        if settled_same_scope {
            return;
        }
        self.load_history_scope(core, market, scope, true, true, cx);
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
        let elapsed = self
            .report_trades
            .last_refresh_start
            .map(|started| started.elapsed())
            .unwrap_or(HISTORY_REFRESH_INTERVAL);
        if elapsed >= HISTORY_REFRESH_INTERVAL {
            self.refresh_trade_history(cx);
            return;
        }
        if self.report_trades.refresh_timer_armed {
            return;
        }
        let wait = HISTORY_REFRESH_INTERVAL.saturating_sub(elapsed);
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
        let graphics =
            moon_chart::normalize_chart_graphics(self.backend.read(cx).layout.chart_graphics);
        let kinds = (graphics.show_real_trades, graphics.show_emulator_trades);
        // `None` means no read has settled yet, and the read that does will stamp the pair itself.
        if self.report_trades.last_trade_kinds == Some(kinds) {
            return;
        }
        self.refresh_trade_history(cx);
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
            false,
            cx,
        );
    }
}

#[cfg(test)]
mod tests;
