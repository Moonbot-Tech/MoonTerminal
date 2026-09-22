//! Runtime durable closed-trade loading for one exact Main-chart target.
//!
//! With `history_all_cores` on, and only while the panel's group is in Auto Overview, the read
//! widens to every core of that overview on the chart core's own exchange. The stored flag alone
//! never widens: the gate is applied here, where the request is built.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::*;
use moon_core::db::{self, ChartTradeHistory, FailKind, ReadFail, ReportFilter};
use moon_core::session::CoreId;
use rust_i18n::t;

use super::ChartPanel;
use crate::Backend;
use crate::backend::ChartHistoryScope;
use crate::chartdx::trade_history_sync::TradeHistoryCores;
use crate::core_order::{ExchangeSection, section_of};
use crate::load_state::{db_read_failed_hint, db_read_failed_retryable};
use crate::workspace::RetainedCoreScope;

/// Maximum durable rows drawn for one Main chart.
const HISTORY_LIMIT: usize = 1_000;

/// Minimum spacing between retries of a NotReady/Failed durable read.
///
/// Unrelated to how fast a settled read reacts to a new report commit (see
/// [`HISTORY_LIVE_REFRESH_INTERVAL`]): this backs off a BROKEN or not-yet-ready replica reader, so a
/// busy detect feed re-adding the same market to extend its TTL costs at most one `open_reader` call
/// every 5 s rather than one per detection.
const HISTORY_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// Minimum spacing between report-generation refreshes on the FOREGROUND ("fast") chart.
///
/// A closed trade is a live, `ReportPublication::Immediate` commit (`moon_core::db::apply_msg`), so
/// this panel's own report-revision observer fires within one 100 ms coordination tick of the write
/// — see `crate::startup::ReportRevisionGate`. Coalescing that edge at the old 5 s value (matching
/// [`HISTORY_RETRY_BACKOFF`]) is what produced the reported 5-7 s lag before a closed trade's dashed
/// line and triangle appeared: the wait before the NEXT due refresh is up to the full interval,
/// regardless of how recent or rare the triggering commit was. 250 ms keeps the same coalescing
/// shape (a burst of live commits still costs at most 4 durable reads/s on this one panel) while
/// making an isolated trade close imperceptible, matching the 250 ms cadence already used elsewhere
/// in this crate for UI-observable coalescing (`Backend::flush_backend_notify`).
const HISTORY_LIVE_REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// Minimum spacing between report-generation refreshes on a BACKGROUND chart tile.
///
/// One report generation would otherwise start one durable read per open tile, and a stack can hold
/// dozens; a tile in the corner of a stack is not worth a fresh SQLite connection four times a
/// second. The foreground chart does not share this bound — see [`HISTORY_LIVE_REFRESH_INTERVAL`].
const HISTORY_REFRESH_INTERVAL_BACKGROUND: Duration = Duration::from_secs(30);

/// In-process Busy retries before the overlay names a failure.
///
/// Matches `moon_core::db::rep::table_cols_for_init`: three attempts, 250 ms times the attempt
/// number between them. A momentary lock must not cost the user his trade markers.
const BUSY_READ_ATTEMPTS: u32 = 3;
const BUSY_READ_BACKOFF_MS: u64 = 250;

/// Visible durable-history load state for a Main chart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    ///
    /// Carries the classified cause so the overlay can name it and hide Retry when
    /// retrying cannot help (`Corrupt`, `ReplicaAccessDenied`).
    Failed(FailKind),
}

impl ReportTradesStatus {
    /// Overlay copy for states the user can act on.
    ///
    /// A settled read is already visible as the arrows themselves, so Ready/Empty
    /// stay silent. Failed uses the shared reports-replica guidance, not a single
    /// collapsed sentence.
    ///
    /// Returns:
    ///     Localized badge text, or `None` when the row should not be stated.
    pub(super) fn overlay_label(self) -> Option<String> {
        match self {
            Self::Idle | Self::Ready | Self::Empty => None,
            Self::Loading => Some(t!("chart.trade_history.loading").to_string()),
            Self::NotReady => Some(t!("chart.trade_history.not_ready").to_string()),
            Self::Failed(kind) => Some(db_read_failed_hint(kind)),
        }
    }

    /// Whether the overlay offers Retry for this status.
    ///
    /// Returns:
    ///     `true` for NotReady and for the two Failed kinds a later read can still clear.
    pub(super) fn offers_retry(self) -> bool {
        match self {
            Self::NotReady => true,
            Self::Failed(kind) => db_read_failed_retryable(kind),
            Self::Idle | Self::Loading | Self::Ready | Self::Empty => false,
        }
    }

    /// Whether this Failed overlay should wake itself without a report commit.
    ///
    /// Returns:
    ///     `true` only for retryable Failed kinds. NotReady already retries through
    ///     detect re-adds; Corrupt and lease denial never recover on a timer.
    fn auto_retries(self) -> bool {
        matches!(self, Self::Failed(kind) if db_read_failed_retryable(kind))
    }
}

/// The wait a generation-triggered refresh owes before its next durable read.
///
/// A NotReady/Failed status always takes [`HISTORY_RETRY_BACKOFF`], whatever `fast` says: a
/// broken or not-yet-ready replica reader must back off the same way through EVERY entry point,
/// not just the explicit-navigation one `history_request_is_redundant` already guards. A settled
/// status takes the panel's own live-refresh cadence: [`HISTORY_LIVE_REFRESH_INTERVAL`] for the
/// foreground ("fast") chart, [`HISTORY_REFRESH_INTERVAL_BACKGROUND`] for a background tile.
///
/// Args:
///     status: The panel's current durable-history load state.
///     fast: Whether this is the foreground ("fast") chart rather than a background tile.
///
/// Returns:
///     The minimum spacing the next generation-triggered refresh must respect.
fn generation_refresh_interval(status: ReportTradesStatus, fast: bool) -> Duration {
    match status {
        ReportTradesStatus::NotReady | ReportTradesStatus::Failed(_) => HISTORY_RETRY_BACKOFF,
        _ if fast => HISTORY_LIVE_REFRESH_INTERVAL,
        _ => HISTORY_REFRESH_INTERVAL_BACKGROUND,
    }
}

/// Backoff after a Busy durable-history attempt, or `None` when retries are exhausted.
///
/// Args:
///     attempt: 1-based attempt that just returned Busy.
///
/// Returns:
///     Sleep before the next read, or `None` after [`BUSY_READ_ATTEMPTS`].
fn busy_read_backoff(attempt: u32) -> Option<Duration> {
    if attempt == 0 || attempt >= BUSY_READ_ATTEMPTS {
        return None;
    }
    Some(Duration::from_millis(
        BUSY_READ_BACKOFF_MS * u64::from(attempt),
    ))
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
    /// Cores the current history request was made for, the chart's own core first.
    ///
    /// Remembered beside [`Self::last_admitted_any`] so a later wake can tell a changed admitted
    /// set from the one already loaded. Empty means no request has settled. A different set is a
    /// different request: the redundancy check compares it, or a wider read would be swallowed
    /// as the same target.
    cores: Vec<CoreId>,
    /// Coin aliases the last history read was built from, in query order.
    ///
    /// A sibling can already sit in [`Self::cores`] while its catalog still spells the coin as a
    /// name fallback. The catalog wake does not change the ids, only this spelling, and the read
    /// is the only place the aliases are rebuilt — so the wake has to compare this too.
    exact_coins: Vec<String>,
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

/// Cores whose closed trades this chart may draw, the chart's own core first.
///
/// One answer for the query and the draw filter, so the two cannot drift. Every early exit is
/// `vec![core]`: the flag off, a panel with no window group, anything that is not Auto Overview,
/// and a core whose venue nothing can name. That last one does not join other cores — an unnamed
/// venue matches nobody — so the chart stays on its own core rather than widening into the
/// unidentified bucket.
///
/// The owner is moved to element 0 even when the display order already contains it later. Keying
/// catalog readiness on `cores[0]` would otherwise treat a sibling's empty label as the chart's
/// own and, on a Default scope, clear the arrows.
///
/// Args:
///     b: Backend holding the session venues and the workspace scope.
///     group: The panel's window group, or `None` for a diagnostics or historical panel.
///     core: The chart's own core.
///     all_cores: The stored `history_all_cores` flag. Off never widens.
///
/// Returns:
///     Admitted cores, the owner at element 0. One element is today's picture.
pub(super) fn admitted_history_cores(
    b: &Backend,
    group: Option<&str>,
    core: CoreId,
    all_cores: bool,
) -> Vec<CoreId> {
    if !all_cores {
        return vec![core];
    }
    let Some(group) = group else {
        return vec![core];
    };
    if !b.is_auto_overview_scope(group) {
        return vec![core];
    }
    // Cloned so the scope call below can borrow `b` again. The map is one entry per core.
    let venues = b.session.core_venues().clone();
    let own = section_of(venues.get(&core));
    if own == ExchangeSection::Unidentified {
        return vec![core];
    }
    let mut admitted: Vec<CoreId> = b
        .effective_workspace_scope(group, RetainedCoreScope::All)
        .ids()
        .iter()
        .copied()
        .filter(|candidate| *candidate != core && section_of(venues.get(candidate)) == own)
        .collect();
    admitted.insert(0, core);
    admitted
}

/// Read one durable history snapshot without touching GPUI state.
///
/// Args:
///     cores: Explicit runtime cores, the chart's own core first.
///     exact_coins: Case-insensitive exact database coin identities for the market.
///     filter: Optional published Report filter refinement.
///
/// Returns:
///     Bounded durable records and truncation state.
///
/// Errors:
///     Propagates report-replica readiness, snapshot, schema, and SQL failures.
fn load_history(
    cores: Vec<CoreId>,
    exact_coins: Vec<String>,
    filter: Option<ReportFilter>,
) -> db::ReadResult<ChartTradeHistory> {
    let conn = db::open_reader_with(db::CHART_TRADE_HISTORY_ATTACH)?;
    let snapshot = db::read_snapshot(&conn)?;
    db::query_chart_trade_history_for_cores(
        &snapshot,
        &cores,
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
    /// The admitted core set for one chart core under this panel's flag and window group.
    ///
    /// The wakes that compare a request share this call, so a later edit cannot change one of
    /// them and leave the others on the old arguments. `load_history_scope` does not use it: that
    /// path already holds the graphics value and must not read the settings a second time.
    ///
    /// Args:
    ///     core: The chart's own core.
    ///     cx: Application context used to read the backend and the effective graphics.
    ///
    /// Returns:
    ///     Admitted cores, the owner first.
    fn admitted_cores_for(&self, core: CoreId, cx: &App) -> Vec<CoreId> {
        admitted_history_cores(
            self.backend.read(cx),
            self.workspace_group.as_deref(),
            core,
            self.effective_chart_graphics(cx).history_all_cores,
        )
    }

    /// Coin aliases one history read queries, in the order the SQL sees them.
    ///
    /// The market name comes first, then each admitted core's catalog label, then a Report
    /// scope's exact coin. An empty label is skipped. The catalog wake rebuilds this same list
    /// so a spelling change is visible even when the core ids are not.
    ///
    /// Args:
    ///     core: The chart's own core. Its label is the catalog-ready one, not `cores[0]`.
    ///     market: Canonical market the chart is showing.
    ///     cores: Admitted cores, the owner first.
    ///     scope: Default or published Report history scope.
    ///     cx: Application context used to read each core's market label.
    ///
    /// Returns:
    ///     Deduplicated aliases, case-insensitive.
    fn history_exact_coins(
        &self,
        core: CoreId,
        market: &str,
        cores: &[CoreId],
        scope: &ChartHistoryScope,
        cx: &App,
    ) -> Vec<String> {
        let owner_label = self
            .backend
            .read(cx)
            .session
            .market_source()
            .market_label(core, market)
            .coin;
        let mut exact_coins = vec![market.to_string()];
        for admitted in cores {
            let label = if *admitted == core {
                owner_label.clone()
            } else {
                self.backend
                    .read(cx)
                    .session
                    .market_source()
                    .market_label(*admitted, market)
                    .coin
            };
            if label.is_empty()
                || exact_coins
                    .iter()
                    .any(|coin| coin.eq_ignore_ascii_case(&label))
            {
                continue;
            }
            exact_coins.push(label);
        }
        if let ChartHistoryScope::Report { exact_coin, .. } = scope {
            let report_coin = exact_coin.clone();
            if !report_coin.trim().is_empty()
                && !exact_coins
                    .iter()
                    .any(|coin| coin.eq_ignore_ascii_case(&report_coin))
            {
                exact_coins.push(report_coin);
            }
        }
        exact_coins
    }

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
            // A new set of trades is a new set of lines to resolve and, for what is already
            // resolved, to draw.
            self.request_trace_lines(true, cx);
            self.sync_trace_lines(true, cx);
            self.sync_orders_if_visible(cx, true);
            self.view_dirty = true;
        }
    }

    /// Start one exact-target durable read, optionally replacing visible state and refocusing.
    ///
    /// The admitted core set is stored on the chart before any early return. Turning the flag off
    /// therefore drops foreign arrows immediately, and turning it on widens the draw filter while
    /// the rows still on screen belong to the previous core.
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
        // THIS panel's effective settings: the popup is per tab, so two tabs on the same market can
        // legitimately draw different sets. Hoisted above the alias block so the admitted set and
        // the "draw anything" check share one read.
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
        let graphics = self.effective_chart_graphics(cx);
        let draws_any_kind = draws_any_trade_kind(&graphics);
        let backend = self.backend.read(cx);
        let cores = admitted_history_cores(
            backend,
            self.workspace_group.as_deref(),
            core,
            graphics.history_all_cores,
        );
        // The OWNER's label, never `cores[0]`. `admitted_history_cores` already moves the owner to
        // the front; keying readiness on the owner by name is the second guard, so a sibling with
        // an empty label cannot make this read NotReady and clear the chart's own arrows.
        let owner_label = self
            .backend
            .read(cx)
            .session
            .market_source()
            .market_label(core, &market)
            .coin;
        let catalog_ready = !owner_label.is_empty();
        let default_needs_catalog = matches!(scope, ChartHistoryScope::Default);
        // One alias per admitted core. In PerCore each core has its own catalog, so two cores on
        // one exchange can spell the coin differently; several distinct aliases are the right
        // result, not a bug. An empty label contributes nothing and does not fail the read.
        let exact_coins = self.history_exact_coins(core, &market, &cores, &scope, cx);
        let filter = match &scope {
            ChartHistoryScope::Default => None,
            ChartHistoryScope::Report { filter, .. } => Some(filter.clone()),
        };

        self.report_trades.sequence = self.report_trades.sequence.wrapping_add(1);
        let sequence = self.report_trades.sequence;
        self.report_trades.target = Some((core, market.clone()));
        self.report_trades.scope = scope.clone();
        self.report_trades.cores = cores.clone();
        self.report_trades.exact_coins = exact_coins.clone();
        self.report_trades.last_admitted_any = Some(draws_any_kind);
        self.report_trades.last_refresh_start = Some(Instant::now());
        // Before the early return and before the re-read: toggling OFF narrows the drawn set
        // immediately, and toggling ON widens a filter whose current records are still one core.
        // The corner name is rebuilt by the order sync, and this set is not an order revision.
        // Force that sync the way a changed record list does, so the name moves with the arrows
        // instead of waiting for an unrelated order.
        if self
            .chart
            .set_trade_history_cores(Some(Rc::new(TradeHistoryCores {
                owner: core,
                admitted: cores.clone(),
            })))
        {
            self.sync_orders_if_visible(cx, true);
        }
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
            let mut last_busy = None;
            let mut result = None;
            for attempt in 1..=BUSY_READ_ATTEMPTS {
                let exact_coins = exact_coins.clone();
                let filter = filter.clone();
                let cores = cores.clone();
                let outcome = executor
                    .spawn(async move { load_history(cores, exact_coins, filter) })
                    .await;
                match outcome {
                    Ok(history) => {
                        result = Some(Ok(history));
                        break;
                    }
                    Err(error) if error.kind() == Some(FailKind::Busy) => {
                        log::warn!(
                            "chart trade history read busy ({error}) - attempt {attempt} of {BUSY_READ_ATTEMPTS}"
                        );
                        last_busy = Some(error);
                        if let Some(wait) = busy_read_backoff(attempt) {
                            executor.timer(wait).await;
                        }
                    }
                    Err(error) => {
                        result = Some(Err(error));
                        break;
                    }
                }
            }
            let result = result
                .or_else(|| last_busy.map(Err))
                .expect("the busy-retry loop always stores an outcome");
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
                            this.report_trades.status = ReportTradesStatus::Failed(
                                error.kind().unwrap_or(FailKind::Other),
                            );
                            if replace_visible {
                                this.publish_trade_history(Rc::new(Vec::new()), cx);
                            }
                            this.schedule_failed_history_retry(cx);
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
        let cores = self.admitted_cores_for(core, cx);
        if self.history_request_is_redundant(core, &market, &scope, &cores) {
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
        let cores = self.admitted_cores_for(core, cx);
        if self.history_request_is_redundant(core, &market, &scope, &cores) {
            return;
        }
        self.load_history_scope(core, market, scope, true, cx);
    }

    /// Whether a history request for this target would repeat work already done or under way.
    ///
    /// `cores` is the admitted set, the chart's own core first. It is compared with the set
    /// already stored, in that order, so a wider or narrower admission is not the same request.
    /// Alias spelling is a separate wake ([`Self::requery_trade_history_on_core_scope`]).
    ///
    /// `Loading | Ready | Empty` are settled: the answer is either in hand or on its way. The two
    /// FAILURE states are not settled — they must be retried — but not on demand: an unavailable
    /// replica would otherwise turn a busy detect feed, which re-adds the same market to extend its
    /// TTL, into one `open_reader` per detection. They are rate-limited by [`HISTORY_RETRY_BACKOFF`],
    /// and the report-revision observer retries them anyway.
    fn history_request_is_redundant(
        &self,
        core: CoreId,
        market: &str,
        scope: &ChartHistoryScope,
        cores: &[CoreId],
    ) -> bool {
        let same_target = self
            .report_trades
            .target
            .as_ref()
            .is_some_and(|target| target.0 == core && target.1 == market);
        if !same_target || &self.report_trades.scope != scope || self.report_trades.cores != cores {
            return false;
        }
        match self.report_trades.status {
            ReportTradesStatus::Loading | ReportTradesStatus::Ready | ReportTradesStatus::Empty => {
                true
            }
            ReportTradesStatus::NotReady | ReportTradesStatus::Failed(_) => self
                .report_trades
                .last_refresh_start
                .is_some_and(|started| started.elapsed() < HISTORY_RETRY_BACKOFF),
            ReportTradesStatus::Idle => false,
        }
    }

    /// Coalesce report-generation refreshes to at most one durable read per
    /// [`generation_refresh_interval`] — or, while the last read is still broken or not-ready,
    /// per [`HISTORY_RETRY_BACKOFF`] instead.
    ///
    /// A generation edge fires on ANY live report commit, not just this panel's own market, so
    /// without the status check below a foreground panel stuck on `NotReady`/`Failed` would retry
    /// `open_reader` at [`HISTORY_LIVE_REFRESH_INTERVAL`] (250 ms) for as long as trading stayed
    /// active elsewhere — the exact read/log storm [`HISTORY_RETRY_BACKOFF`] exists to prevent,
    /// just reached through this entry point instead of the explicit-navigation one.
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
        if matches!(
            self.report_trades.status,
            ReportTradesStatus::Failed(kind) if !db_read_failed_retryable(kind)
        ) {
            return;
        }
        let interval = generation_refresh_interval(self.report_trades.status, self.fast);
        let elapsed = self
            .report_trades
            .last_refresh_start
            .map(|started| started.elapsed())
            .unwrap_or(interval);
        if elapsed >= interval {
            self.refresh_trade_history(cx);
            return;
        }
        self.arm_history_refresh_timer(interval.saturating_sub(elapsed), cx);
    }

    /// Arm one trailing durable-history timer, replacing none that is already waiting.
    ///
    /// Args:
    ///     wait: Remaining backoff before the next `requery_trade_history_on_generation`.
    ///     cx: Panel context used to spawn the timer.
    fn arm_history_refresh_timer(&mut self, wait: Duration, cx: &mut Context<Self>) {
        if self.report_trades.refresh_timer_armed {
            return;
        }
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
                    // Re-derive the interval and elapsed time rather than refreshing
                    // unconditionally: the status this timer was armed under may have flipped to
                    // NotReady/Failed while it was waiting, and a stale short wait must not let a
                    // retry through faster than HISTORY_RETRY_BACKOFF.
                    this.requery_trade_history_on_generation(cx);
                });
            });
        })
        .detach();
    }

    /// Wake a retryable Failed overlay on its own timer, without waiting for a report commit.
    ///
    /// Args:
    ///     cx: Panel context used to arm the trailing retry.
    fn schedule_failed_history_retry(&mut self, cx: &mut Context<Self>) {
        if !self.report_trades.status.auto_retries() {
            return;
        }
        self.arm_history_refresh_timer(HISTORY_RETRY_BACKOFF, cx);
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

    /// Re-read durable history when the admitted core set changes.
    ///
    /// The set is the chart's own core, plus — only in Auto Overview, and only while the flag is
    /// on — every other core of that overview on the same exchange. A settings change, a workspace
    /// revision, and a sibling catalog arriving are the wakes, and each is rare. This is not on
    /// the coalesced Backend path: that one fires four times a second, and a scope walk there
    /// would be three orders of magnitude off the background refresh budget.
    ///
    /// Args:
    ///     cx: Panel context used to start a non-clearing refresh.
    ///
    /// Returns:
    ///     Nothing; idle panels and an unchanged set do no work.
    pub(super) fn requery_trade_history_on_core_scope(&mut self, cx: &mut Context<Self>) {
        let Some((core, market)) = self.report_trades.target.clone() else {
            return;
        };
        let cores = self.admitted_cores_for(core, cx);
        // A single core's catalog is already retried by `catalog_ready`. More than one core
        // can be admitted by venue before its catalog spells the stored coin, and that
        // spelling change does not move `cores`.
        let aliases = if cores.len() > 1 {
            Some(self.history_exact_coins(core, &market, &cores, &self.report_trades.scope, cx))
        } else {
            None
        };
        let same_aliases = match &aliases {
            None => true,
            Some(aliases) => aliases == &self.report_trades.exact_coins,
        };
        if self.report_trades.cores == cores && same_aliases {
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
        self.report_trades.cores.clear();
        self.report_trades.exact_coins.clear();
        // Same wake as a set installed by a load: clearing the set is not an order revision,
        // and the corner name would otherwise keep naming every core until one moved.
        if self.chart.set_trade_history_cores(None) {
            self.sync_orders_if_visible(cx, true);
        }
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
        self.load_history_scope(core, market, self.report_trades.scope.clone(), false, cx);
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
        self.load_history_scope(core, market, self.report_trades.scope.clone(), true, cx);
    }
}

#[cfg(test)]
mod tests;
