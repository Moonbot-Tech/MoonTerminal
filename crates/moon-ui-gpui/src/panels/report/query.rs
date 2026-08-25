//! Background report query: the read, its result types, filter assembly, and requery scheduling.

use super::*;

/// Maximum number of report rows loaded for the current filter and sort order.
///
/// Period totals come from a separate [`db::query_totals`] aggregate over the complete filtered
/// data set, so they remain exact regardless of this limit. Writer generations can request repeated
/// background reads, coalesced by the panel's five-second throttle; keeping the row cap small avoids
/// repeatedly materializing very large result sets.
pub(super) const MAX_REPORT_ROWS: usize = 500;

/// Minimum spacing between automatic Report queries driven by writer generations.
const GENERATION_QUERY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Wake action selected after observing one committed report generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GenerationRefreshPlan {
    /// Existing pending or due work already owns the required wake.
    Idle,
    /// The throttle has elapsed and a rendered Report panel may start the query.
    NotifyNow,
    /// One versioned timer must release the due edge after the remaining throttle interval.
    NotifyAfter {
        /// Remaining time before automatic work may become due.
        wait: std::time::Duration,
        /// Token that prevents a superseded timer from releasing newer work.
        timer_token: u64,
    },
}

/// Durable automatic Report-refresh state shared by generation, timer, and render paths.
#[derive(Default)]
pub(super) struct GenerationRefreshGate {
    /// Whether a committed generation is not yet covered by a query start.
    pending: bool,
    /// Whether the throttle elapsed and a rendered Report panel may consume the refresh.
    due: bool,
    /// Whether one timer already owns the remaining throttle wait.
    timer_armed: bool,
    /// Identity of the current timer; query starts invalidate every older wake.
    timer_token: u64,
}

impl GenerationRefreshGate {
    /// Record one committed generation without starting database work.
    ///
    /// Args:
    ///     since_query_start: Time elapsed since the latest Report query began.
    ///
    /// Returns:
    ///     Whether to notify now, arm one timer, or leave an existing wake in charge.
    fn observe(&mut self, since_query_start: std::time::Duration) -> GenerationRefreshPlan {
        self.pending = true;
        if self.due {
            return GenerationRefreshPlan::Idle;
        }
        if since_query_start >= GENERATION_QUERY_INTERVAL {
            self.due = true;
            return GenerationRefreshPlan::NotifyNow;
        }
        if self.timer_armed {
            return GenerationRefreshPlan::Idle;
        }
        self.timer_armed = true;
        self.timer_token = self.timer_token.wrapping_add(1);
        GenerationRefreshPlan::NotifyAfter {
            wait: GENERATION_QUERY_INTERVAL - since_query_start,
            timer_token: self.timer_token,
        }
    }

    /// Release the matching timer slot and make still-pending work available to a panel render.
    ///
    /// Args:
    ///     timer_token: Identity captured when this timer was armed.
    ///
    /// Returns:
    ///     `true` when the current timer published a new due edge that should notify the panel.
    fn timer_fired(&mut self, timer_token: u64) -> bool {
        if !self.timer_armed || timer_token != self.timer_token {
            return false;
        }
        self.timer_armed = false;
        if !self.pending || self.due {
            return false;
        }
        self.due = true;
        true
    }

    /// Consume one due automatic refresh from a rendered Report panel.
    ///
    /// Returns:
    ///     `true` exactly once for each due generation burst.
    pub(super) fn take_due(&mut self) -> bool {
        if !self.due {
            return false;
        }
        self.pending = false;
        self.due = false;
        true
    }

    /// Mark every generation visible at a new query start as covered.
    ///
    /// Returns:
    ///     Nothing; pending work and any older timer ownership are invalidated in place.
    fn query_started(&mut self) {
        self.pending = false;
        self.due = false;
        self.timer_armed = false;
        self.timer_token = self.timer_token.wrapping_add(1);
    }
}

/// Rows and exact period totals from one completed report read.
///
/// The schema is stored separately so a completed failed read cannot collapse
/// column controls. Rows and totals are discarded because retaining them under
/// a changed filter would present stale figures as current.
pub(super) struct ReportData {
    /// Exact published filter that produced these rows.
    pub(super) filter: Arc<ReportFilter>,
    pub(super) rows: Vec<Vec<Value>>,
    pub(super) core_uids: Vec<u64>,
    /// Stable semantic identity parallel to `rows`; malformed legacy rows without `id` are `None`
    /// and remain unselectable rather than receiving an index-based identity that can drift.
    pub(super) row_keys: Vec<Option<selection::ReportRowKey>>,
    /// Exact per-quote totals over the full filter, not the displayed top N.
    pub(super) totals: db::QuoteBreakdown,
    /// Still-running positions under the same filter, counted apart from `totals`.
    ///
    /// Empty whenever the resolved [`db::RowScope`] excludes open rows — the period not reaching
    /// the present, the scope field's open-positions switch being off, or an Analytics-scoped
    /// host. Every one of those is the same condition that keeps those rows out of the grid,
    /// because both come from one filter — so the footer can never name positions the table above
    /// it is not showing.
    pub(super) open: db::OpenPositions,
    /// The conversion these figures were computed under.
    ///
    /// Travels WITH the data rather than being read live at render: a mode change requeries, and
    /// the previous result stays on screen while that query runs. Labelling it from the live
    /// setting would put the new mode's words under the old mode's numbers for exactly as long as
    /// the requery takes — the one thing this feature must never do.
    pub(super) valuation: db::valuation::ValuationMode,
}

/// One completed background batch with optional selector-metadata refresh.
struct ReportRead {
    cores: Option<Vec<(u64, String)>>,
    /// Canonical non-strategy scope and its choices from this exact snapshot.
    strategy_metadata: Option<(ReportFilter, Vec<ReportStrategy>)>,
    cols: Vec<String>,
    data: ReportData,
}

/// Canonicalize the predicates that determine which strategies are available in the selector.
///
/// Args:
///     filter: Complete Report filter, including the current strategy selection.
///
/// Returns:
///     An equality-stable filter with no strategy predicates, sorted core ids, and the same coin
///     normalization used by the SQL read layer.
fn strategy_catalog_scope(filter: &ReportFilter) -> ReportFilter {
    let mut scope = filter.clone();
    scope.strategies = None;
    scope.strategy_name_mask.clear();
    scope.core_uids.sort_unstable();
    scope.core_uids.dedup();
    scope.coin = scope.coin.trim().to_uppercase();
    scope
}

/// Decide whether one query must refresh the strategy catalog and retain its exact scope.
///
/// Args:
///     filter: Complete filter for the pending rows query.
///     published_scope: Scope of the last successfully published strategy catalog.
///     periodic_refresh: Whether the existing metadata refresh interval elapsed.
///
/// Returns:
///     The canonical scope to query, or `None` when the published catalog remains valid.
fn strategy_metadata_request(
    filter: &ReportFilter,
    published_scope: Option<&ReportFilter>,
    periodic_refresh: bool,
) -> Option<ReportFilter> {
    let scope = strategy_catalog_scope(filter);
    (periodic_refresh || published_scope != Some(&scope)).then_some(scope)
}

/// Return whether a completed read still belongs to the panel's current semantic query.
///
/// Args:
///     request_id: Sequence captured when the background read started.
///     current_id: Panel sequence at publication time.
///     requested: Exact filter used by the background read.
///     current: Filter represented by the controls and workspace at publication time.
///
/// Returns:
///     `true` only when neither sequence nor any filter predicate drifted.
fn report_query_result_is_current(
    request_id: u64,
    current_id: u64,
    requested: &ReportFilter,
    current: &ReportFilter,
) -> bool {
    request_id == current_id && requested == current
}

/// Read selector metadata when requested, rows, and totals from one WAL snapshot.
///
/// `NotReady` means the reports replica is absent. `Failed` means opening the
/// connection, pinning the snapshot, probing schema, or running any query failed.
/// Core metadata keeps its minute cadence, while strategy metadata also refreshes whenever its
/// canonical non-strategy filter scope changes.
///
/// Args:
///     filter: Complete database filter.
///     sort_key: Validated-at-query report sort candidate.
///     sort_desc: Whether rows sort descending.
///     with_core_metadata: Whether to refresh the core selector in this snapshot.
///     strategy_scope: Canonical strategy-catalog scope to refresh, or `None` to reuse it.
///
/// Returns:
///     One consistent report batch.
///
/// Errors:
///     Propagates report database readiness, schema, and query failures.
fn run_report_query(
    filter: ReportFilter,
    sort_key: String,
    sort_desc: bool,
    with_core_metadata: bool,
    strategy_scope: Option<ReportFilter>,
) -> ReadResult<ReportRead> {
    let started = std::time::Instant::now();
    let conn = db::open_reader()?;
    // Pin one snapshot across cores, rows, and totals. Separate autocommit reads
    // could straddle a writer commit, including legacy cleanup after sync, and
    // publish rows and totals from different database states.
    // The current-rate snapshot is pinned for the same span and the same reason: a worker
    // publication landing between the rows and the totals would convert the two at different
    // rates, so the footer would stop summing the column above it.
    let _rates = db::valuation::pin_current_rates();
    let snap = db::read_snapshot(&conn)?;
    let cores = if with_core_metadata {
        Some(db::distinct_cores(&snap)?)
    } else {
        None
    };
    let strategy_metadata = strategy_scope
        .map(|scope| db::distinct_strategies(&snap, &scope).map(|strategies| (scope, strategies)))
        .transpose()?;
    let table = db::query_reports(&snap, &filter, &sort_key, sort_desc, MAX_REPORT_ROWS)?;
    let totals = db::query_totals(&snap, &filter)?;
    let row_keys = selection::row_keys(&table.cols, &table.rows, &table.core_uids, &table.rec_ids);
    // Log measured query latency so slow refreshes are visible; the panel controls their frequency.
    let ms = started.elapsed().as_millis();
    if ms > 250 {
        log::warn!(
            "отчёты: медленный query {ms}ms (rows={} cores={} filter: dates={:?}/{:?})",
            table.rows.len(),
            with_core_metadata,
            filter.date_from,
            filter.date_to,
        );
    } else {
        log::debug!("отчёты: query {ms}ms (rows={})", table.rows.len());
    }
    Ok(ReportRead {
        cores,
        strategy_metadata,
        cols: table.cols,
        data: ReportData {
            filter: Arc::new(filter.clone()),
            rows: table.rows,
            core_uids: table.core_uids,
            row_keys,
            totals: totals.quotes,
            open: totals.open,
            valuation: filter.valuation,
        },
    })
}

impl ReportPanel {
    /// Assemble the exact database filter represented by the retained controls.
    ///
    /// Args:
    ///     cx: Application context used to read the application-wide valuation mode.
    ///
    /// Returns:
    ///     A filter shared by rows, totals, and export.
    pub(super) fn filter(&self, cx: &App) -> ReportFilter {
        // A preset overrides the manual date only on the edge it SETS itself. Every preset
        // but "All" sets the lower one; only "Yesterday" sets the upper one — for the rest
        // `to = None`, and then the upper edge comes from the "To:" field if it holds a date.
        // ONE clock reading for the whole filter: the period bounds and the decision about whether
        // that period still reaches the present must not straddle a second boundary, or a query can
        // ask for a window ending before the instant it just judged to be inside it.
        let now = moon_core::util::time::now_unix_secs() as i64;
        let (pfrom, pto) = self.period.range_at(now, self.bound_zone());
        let date_from = pfrom.or(self.from_query);
        // The upper field names a whole minute and the SQL bound is inclusive, so it reaches that
        // minute's last second: "from 04.08 00:00 to 04.08 23:59" is the whole day, and an equal
        // pair is that one minute rather than an empty range.
        let date_to = pto.or_else(|| self.to_query.map(date_range::inclusive_end));
        let backend = self.backend.read(cx);
        let strategy_name_mask =
            if super::strategy_name_mask_enabled(self.workspace_scope(backend).as_ref()) {
                self.strategy_name_mask.trim().to_string()
            } else {
                String::new()
            };
        ReportFilter {
            core_uids: self.effective_core_ids(backend),
            date_from,
            date_to,
            // Normalize Russian-layout keystrokes for the SQL filter as well as the search popup;
            // otherwise Cyrillic input would reach `coin LIKE` unchanged and return no matches.
            coin: crate::controls::coin_search::normalize_layout(&self.coin_query).into_owned(),
            exact_coins: None,
            side: self.side,
            emulator: self.kind.to_filter(),
            deleted_only: self.deleted_only,
            rows: super::row_scope_for(self.closed_only, self.show_open, date_to, now),
            strategies: normalized_strategy_filter_keys(self.selected_strategies.as_ref()),
            strategy_name_mask,
            // Read from the backend at build time rather than mirrored into the panel: the rows,
            // the totals and the export all derive from this ONE filter, so they convert alike.
            valuation: backend.valuation_mode(),
        }
    }

    pub(super) fn request_requery(&mut self, cx: &mut Context<Self>) {
        self.needs_query = true;
        self.schedule_requery(cx);
        cx.notify();
    }

    /// Make a writer-generation refresh due at most once every five seconds.
    ///
    /// A trailing timer publishes only a bounded due edge. The selected-panel render owns the
    /// heavy query start, so a Report tab hidden behind another tab cannot consume resources.
    /// User filter edits bypass this throttle through [`Self::request_requery`].
    pub(super) fn requery_on_generation(&mut self, cx: &mut Context<Self>) {
        let since = self
            .last_query_start
            .map(|t| t.elapsed())
            .unwrap_or(GENERATION_QUERY_INTERVAL);
        match self.generation_refresh.observe(since) {
            GenerationRefreshPlan::Idle => {}
            GenerationRefreshPlan::NotifyNow => cx.notify(),
            GenerationRefreshPlan::NotifyAfter { wait, timer_token } => {
                cx.spawn(async move |this, cx| {
                    let executor = cx.update(|cx| cx.background_executor().clone());
                    executor.timer(wait).await;
                    let _ = cx.update(|cx| {
                        let _ = this.update(cx, |this, cx| {
                            if this.generation_refresh.timer_fired(timer_token) {
                                cx.notify();
                            }
                        });
                    });
                })
                .detach();
            }
        }
    }

    /// Start one pending Report query, whether manual or released by a panel render.
    ///
    /// Args:
    ///     cx: Panel context used to spawn and publish the background read.
    ///
    /// Returns:
    ///     Nothing; an in-flight read absorbs the request through `needs_query`.
    pub(super) fn schedule_requery(&mut self, cx: &mut Context<Self>) {
        if !self.needs_query || self.query_inflight {
            return;
        }
        self.needs_query = false;
        self.generation_refresh.query_started();
        self.query_inflight = true;
        self.query_seq = self.query_seq.wrapping_add(1);
        self.last_query_start = Some(std::time::Instant::now());
        // Keep stale rows while a refresh is in flight to avoid flicker. A
        // completed `NotReady` or `Failed` result discards them through `LoadState`.
        self.data.begin();
        // Refresh cores at most once per minute. Strategies share that periodic refresh but also
        // refresh immediately when a non-strategy filter changes; sort-only reads reuse the catalog.
        let with_core_metadata = self
            .last_metadata_at
            .map(|t| t.elapsed() >= std::time::Duration::from_secs(60))
            .unwrap_or(true);

        let request_id = self.query_seq;
        let filter = self.filter(cx);
        let request_filter = filter.clone();
        let strategy_scope = strategy_metadata_request(
            &filter,
            self.last_strategy_scope.as_ref(),
            with_core_metadata,
        );
        let sort_key = self.sort_key.clone();
        let sort_desc = self.sort_desc;

        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move {
                    run_report_query(
                        filter,
                        sort_key,
                        sort_desc,
                        with_core_metadata,
                        strategy_scope,
                    )
                })
                .await;

            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if !report_query_result_is_current(
                        request_id,
                        this.query_seq,
                        &request_filter,
                        &this.filter(cx),
                    ) {
                        this.query_inflight = false;
                        this.needs_query = true;
                        cx.notify();
                        return;
                    }
                    this.query_inflight = false;
                    if this.needs_query {
                        // A manual scope change must not publish the old query under new controls.
                        // Preserve the stale snapshot and let the next panel render catch up.
                        cx.notify();
                        return;
                    }

                    match result {
                        Ok(read) => {
                            let ReportRead {
                                cores,
                                strategy_metadata,
                                cols,
                                data,
                            } = read;
                            let metadata_changed = cores.is_some() || strategy_metadata.is_some();
                            if cores.is_some() {
                                this.last_metadata_at = Some(std::time::Instant::now());
                            }
                            if let Some(cores) = cores {
                                this.cores = cores;
                            }
                            if let Some((strategy_scope, strategies)) = strategy_metadata {
                                let (strategies, available) = merge_strategy_metadata(
                                    &this.strategies,
                                    strategies,
                                    this.selected_strategies.as_ref(),
                                );
                                this.strategies = strategies;
                                this.available_strategy_keys = available;
                                this.last_strategy_scope = Some(strategy_scope);
                            }
                            this.selection.retain_visible(&data.row_keys);
                            if metadata_changed {
                                this.queue_strategy_select_sync(true, cx);
                            }
                            let cols_changed = *this.cols != cols;
                            this.cols = Rc::new(cols);
                            this.data.apply(Ok(data));
                            this.natural_widths.clear();
                            // Complete the width map when schema membership changes
                            // so new columns stay stable while neighbors resize. A
                            // double-click reset intentionally removes one entry.
                            if cols_changed {
                                let cols = this.cols.clone();
                                this.table_state.update(cx, |s, c| {
                                    if !s.column_widths.is_empty() {
                                        complete_widths(&mut s.column_widths, &cols);
                                        c.notify();
                                    }
                                });
                            }
                        }
                        // Preserve schema and core choices across a failed read;
                        // only rows and totals disappear so the failure can render.
                        Err(e) => {
                            this.selection.clear();
                            this.data.apply(Err(e));
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests;
