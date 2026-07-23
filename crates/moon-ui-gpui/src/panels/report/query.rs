//! Background report query: the read, its result types, filter assembly, and requery scheduling.

use super::*;

/// Maximum number of report rows loaded for the current filter and sort order.
///
/// Period totals come from a separate [`db::query_totals`] aggregate over the complete filtered
/// data set, so they remain exact regardless of this limit. Writer generations can request repeated
/// background reads, coalesced by the panel's five-second throttle; keeping the row cap small avoids
/// repeatedly materializing very large result sets.
const MAX_REPORT_ROWS: usize = 100;

/// Rows and exact period totals from one completed report read.
///
/// The schema is stored separately so a completed failed read cannot collapse
/// column controls. Rows and totals are discarded because retaining them under
/// a changed filter would present stale figures as current.
pub(super) struct ReportData {
    pub(super) rows: Vec<Vec<Value>>,
    pub(super) core_uids: Vec<u64>,
    /// `newrecid` per row, parallel to `rows`; `0` for a legacy row that cannot be soft-deleted.
    /// The deletion-mode checkboxes read it to address rows in `set_report_rows_deleted`.
    pub(super) rec_ids: Vec<i64>,
    /// Exact `(profit sum, order count)` over the full filter, not the displayed top N.
    pub(super) totals: (f64, i64),
}

/// One completed background batch: data, schema, and optional core refresh.
///
/// An empty `cores` vector is also used when the expensive core query is skipped.
struct ReportRead {
    cores: Vec<(u64, String)>,
    cols: Vec<String>,
    data: ReportData,
}

/// Read cores when requested, rows, and totals from one WAL snapshot.
///
/// `NotReady` means the reports replica is absent. `Failed` means opening the
/// connection, pinning the snapshot, probing schema, or running any query failed.
/// `with_cores` skips the expensive full-database grouping on most rounds.
fn run_report_query(
    filter: ReportFilter,
    sort_key: String,
    sort_desc: bool,
    with_cores: bool,
) -> ReadResult<ReportRead> {
    let started = std::time::Instant::now();
    let conn = db::open_reader()?;
    // Pin one snapshot across cores, rows, and totals. Separate autocommit reads
    // could straddle a writer commit, including legacy cleanup after sync, and
    // publish rows and totals from different database states.
    let snap = db::read_snapshot(&conn)?;
    let cores = if with_cores {
        db::distinct_cores(&snap)?
    } else {
        Vec::new()
    };
    let table = db::query_reports(&snap, &filter, &sort_key, sort_desc, MAX_REPORT_ROWS)?;
    let totals = db::query_totals(&snap, &filter)?;
    // Log measured query latency so slow refreshes are visible; the panel controls their frequency.
    let ms = started.elapsed().as_millis();
    if ms > 250 {
        log::warn!(
            "отчёты: медленный query {ms}ms (rows={} cores={} filter: dates={:?}/{:?})",
            table.rows.len(),
            with_cores,
            filter.date_from,
            filter.date_to,
        );
    } else {
        log::debug!("отчёты: query {ms}ms (rows={})", table.rows.len());
    }
    Ok(ReportRead {
        cores,
        cols: table.cols,
        data: ReportData {
            rows: table.rows,
            core_uids: table.core_uids,
            rec_ids: table.rec_ids,
            totals,
        },
    })
}

impl ReportPanel {
    pub(super) fn filter(&self, cx: &App) -> ReportFilter {
        // A preset overrides the manual date only on the edge it SETS itself. Every preset
        // but "All" sets the lower one; only "Yesterday" sets the upper one — for the rest
        // `to = None`, and then the upper edge comes from the "To:" field if it holds a date.
        let (pfrom, pto) = self.period.range();
        let date_from = pfrom.or_else(|| db::parse_ymd(&self.from.read(cx).value()));
        let date_to = pto.or_else(|| db::parse_ymd(&self.to.read(cx).value()).map(|d| d + 86_399));
        ReportFilter {
            core_uids: self.sel_cores.iter().copied().collect(),
            date_from,
            date_to,
            // Normalize Russian-layout keystrokes for the SQL filter as well as the search popup;
            // otherwise Cyrillic input would reach `coin LIKE` unchanged and return no matches.
            coin: crate::controls::coin_search::normalize_layout(&self.coin.read(cx).value())
                .into_owned(),
            side: self.side,
            emulator: self.kind.to_filter(),
            deleted_only: self.deleted_only,
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
        // Refresh the core list at most once per minute because it groups across the full database.
        let with_cores = self
            .last_cores_at
            .map(|t| t.elapsed() >= std::time::Duration::from_secs(60))
            .unwrap_or(true);
        if with_cores {
            self.last_cores_at = Some(std::time::Instant::now());
        }

        let request_id = self.query_seq;
        let filter = self.filter(cx);
        let sort_key = self.sort_key.clone();
        let sort_desc = self.sort_desc;

        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move { run_report_query(filter, sort_key, sort_desc, with_cores) })
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
                            // An empty core result never replaces the cached list;
                            // this also preserves it when the query is skipped.
                            if !read.cores.is_empty() {
                                this.cores = read.cores;
                            }
                            let cols_changed = *this.cols != read.cols;
                            this.cols = Rc::new(read.cols);
                            this.data.apply(Ok(read.data));
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
                        Err(e) => this.data.apply(Err(e)),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}
