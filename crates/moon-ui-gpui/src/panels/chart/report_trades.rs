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
    Ready { count: usize, truncated: bool },
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
    pub(super) status: ReportTradesStatus,
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
        let (filter, report_coin, focus_record_id) = match &scope {
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

        self.report_trades.sequence = self.report_trades.sequence.wrapping_add(1);
        let sequence = self.report_trades.sequence;
        self.report_trades.target = Some((core, market.clone()));
        self.report_trades.scope = scope.clone();
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
                            let count = history.records.len();
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
                            this.report_trades.status = ReportTradesStatus::Ready {
                                count,
                                truncated: history.truncated,
                            };
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
                ReportTradesStatus::Loading
                    | ReportTradesStatus::Ready { .. }
                    | ReportTradesStatus::Empty
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
