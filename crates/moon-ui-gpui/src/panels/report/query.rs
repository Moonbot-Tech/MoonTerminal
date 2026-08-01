//! Background report query: the read, its result types, filter assembly, and requery scheduling.

use super::*;

/// Maximum number of report rows loaded for the current filter and sort order.
///
/// Period totals come from a separate [`db::query_totals`] aggregate over the complete filtered
/// data set, so they remain exact regardless of this limit. Writer generations can request repeated
/// background reads, coalesced by the panel's five-second throttle; keeping the row cap small avoids
/// repeatedly materializing very large result sets.
pub(super) const MAX_REPORT_ROWS: usize = 500;

/// Rows and exact period totals from one completed report read.
///
/// The schema is stored separately so a completed failed read cannot collapse
/// column controls. Rows and totals are discarded because retaining them under
/// a changed filter would present stale figures as current.
pub(super) struct ReportData {
    pub(super) rows: Vec<Vec<Value>>,
    pub(super) core_uids: Vec<u64>,
    /// Stable semantic identity parallel to `rows`; malformed legacy rows without `id` are `None`
    /// and remain unselectable rather than receiving an index-based identity that can drift.
    pub(super) row_keys: Vec<Option<selection::ReportRowKey>>,
    /// Exact per-quote totals over the full filter, not the displayed top N.
    pub(super) totals: db::QuoteBreakdown,
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
///     An equality-stable filter with no strategy predicate, sorted core ids, and the same coin
///     normalization used by the SQL read layer.
fn strategy_catalog_scope(filter: &ReportFilter) -> ReportFilter {
    let mut scope = filter.clone();
    scope.strategies = None;
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
            rows: table.rows,
            core_uids: table.core_uids,
            row_keys,
            totals,
        },
    })
}

impl ReportPanel {
    /// Assemble the exact database filter represented by the retained controls.
    ///
    /// Args:
    ///     _cx: Application context retained for the panel filter interface.
    ///
    /// Returns:
    ///     A filter shared by rows, totals, and export.
    pub(super) fn filter(&self, _cx: &App) -> ReportFilter {
        // A preset overrides the manual date only on the edge it SETS itself. Every preset
        // but "All" sets the lower one; only "Yesterday" sets the upper one — for the rest
        // `to = None`, and then the upper edge comes from the "To:" field if it holds a date.
        let (pfrom, pto) = self.period.range();
        let date_from = pfrom.or_else(|| db::parse_ymd(&self.from_query));
        let date_to = pto.or_else(|| db::parse_ymd(&self.to_query).map(|d| d + 86_399));
        ReportFilter {
            core_uids: self.sel_cores.iter().copied().collect(),
            date_from,
            date_to,
            // Normalize Russian-layout keystrokes for the SQL filter as well as the search popup;
            // otherwise Cyrillic input would reach `coin LIKE` unchanged and return no matches.
            coin: crate::controls::coin_search::normalize_layout(&self.coin_query).into_owned(),
            side: self.side,
            emulator: self.kind.to_filter(),
            deleted_only: self.deleted_only,
            closed_only: self.closed_only,
            strategies: normalized_strategy_filter_keys(self.selected_strategies.as_ref()),
        }
    }

    pub(super) fn request_requery(&mut self, cx: &mut Context<Self>) {
        self.needs_query = true;
        self.schedule_requery(cx);
        cx.notify();
    }

    /// Request a writer-generation refresh at most once every five seconds.
    ///
    /// A trailing timer waits out the remaining interval so the final generation change is not
    /// lost. User filter edits bypass this throttle.
    pub(super) fn requery_on_generation(&mut self, cx: &mut Context<Self>) {
        const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
        let since = self
            .last_query_start
            .map(|t| t.elapsed())
            .unwrap_or(MIN_INTERVAL);
        if since >= MIN_INTERVAL {
            self.request_requery(cx);
            return;
        }
        if self.throttle_armed {
            return;
        }
        self.throttle_armed = true;
        let wait = MIN_INTERVAL - since;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(wait).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.throttle_armed = false;
                    this.request_requery(cx);
                });
            });
        })
        .detach();
    }

    /// Coalesce report reads and optionally refresh selector metadata once per minute.
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
                    if this.query_seq != request_id {
                        return;
                    }
                    this.query_inflight = false;
                    if this.needs_query {
                        this.schedule_requery(cx);
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
