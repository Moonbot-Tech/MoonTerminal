//! Analytics load extracted from the window module.

use super::*;

impl AnalyticsView {
    /// Start accounting for a blocking background operation.
    ///
    /// Every operation that calls `op_started` must decrement exactly once, including stale
    /// completions. Completion handlers publish or discard their result first, then balance this
    /// counter before deferred report work is scheduled.
    ///
    /// The idle-to-busy transition arms one delayed repaint for the whole batch. Keeping timer
    /// creation out of `render` prevents a repaint from scheduling its own successor.
    ///
    /// Args:
    ///     cx: GPUI context used to arm the delayed overlay repaint.
    pub(super) fn op_started(&mut self, cx: &mut Context<Self>) {
        self.busy_ops += 1;
        if self.busy_since.is_none() {
            let opened_at = std::time::Instant::now();
            self.busy_since = Some(opened_at);
            cx.spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                executor.timer(BUSY_OVERLAY_DELAY).await;
                cx.update(|cx| {
                    let _ = this.update(cx, |this, cx| {
                        // Detached timers outlive their batches, so match the captured start time
                        // before repainting; a later batch has its own delay and timer.
                        if this.busy_since == Some(opened_at) {
                            cx.notify();
                        }
                    });
                });
            })
            .detach();
        }
    }
    /// Raise a write failure where the user is looking, and log it.
    ///
    /// Shown until dismissed rather than for a few seconds: this is the difference between
    /// "your strategies were changed" and "they were not", and it must not be possible to
    /// miss by looking away.
    pub(super) fn set_write_error(&mut self, msg: String, cx: &mut Context<Self>) {
        log::warn!("analytics: {msg}");
        self.write_error = Some(msg);
        cx.notify();
    }

    /// Finish one blocking operation.
    ///
    /// Args:
    ///     cx: GPUI context used to repaint the overlay.
    pub(super) fn op_finished(&mut self, cx: &mut Context<Self>) {
        self.busy_ops = self.busy_ops.saturating_sub(1);
        if self.busy_ops == 0 {
            self.busy_since = None;
        }
        cx.notify();
    }

    /// Whether the current batch has been running long enough to show the busy overlay.
    ///
    /// This remains a pure render-time read; `op_started` owns the delayed repaint.
    ///
    /// Returns:
    ///     `true` once the open batch is older than `BUSY_OVERLAY_DELAY`.
    pub(super) fn busy_overlay_due(&self) -> bool {
        self.busy_since
            .is_some_and(|since| since.elapsed() >= BUSY_OVERLAY_DELAY)
    }

    /// Reload Analytics after a user-controlled period or filter scope change.
    ///
    /// Args:
    ///     cx: GPUI context used to start all required background reads.
    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        // Whatever started this read, it carries the mask AS IT STANDS NOW — `query_for` copies the
        // live text, not the debounced one. So this IS the applied value, and recording it here is
        // what keeps a still-pending debounce from starting the very same read a second time after
        // an unrelated filter change happened to reload first.
        self.strategy_mask_applied = self.strategy_mask.clone();
        self.cancel_latest_reads();
        // Retire every request this scope change invalidates, including ones no branch below
        // restarts. On the Calendar tab neither `reload_summary` nor `reload_strategy_base` runs,
        // so without this bump a cancelled background read's completion would pass its own
        // `seq != req` guard and publish an interrupt as a real result under the new scope.
        self.seq = self.seq.wrapping_add(1);
        self.report_busy_retries.reset();
        self.acknowledge_report_refresh();
        // The tuner uses the same filters: invalidate it and recompute immediately in the active
        // mode, or defer recomputation until the next entry into Filters mode.
        self.tuner.invalidate();
        // The By time axis uses the shared filters and `strat_period`. This common reload path
        // conservatively retires its in-flight auto-suggestion even on a Summary-only period
        // change; otherwise a stale result could be written into v1 and become saveable.
        // The profile is marked stale the same way, recomputed immediately below when the
        // axis is the active one, or deferred until entry.
        self.time_tuner.invalidate();
        // The "By coin" axis lives on the same scope (its "Fact vs v1" matrix included):
        // mark it stale. It is NOT started here, unlike the other axes: it expands its coin
        // lists against the base coin set that this very request is about to replace, so
        // starting it now would plan against the PREVIOUS period's coins (or, on a first
        // show, against none at all). It is armed from the completion handler below.
        self.coins.invalidate();
        self.ticks.invalidate();
        // The list panels ride the same reload, so they are retired with it — otherwise a
        // reply already in flight for the previous scope lands under the new heading.
        self.coin_lists.invalidate();
        // Calendar uses the same filters: mark it stale and recompute immediately on the active
        // tab, or defer until entry.
        self.cal_seq = self.cal_seq.wrapping_add(1);
        self.cal_dirty = true;
        if self.tab == Tab::Calendar {
            self.reload_calendar(cx);
        }
        self.data_dirty = true;
        self.strategy_dirty = true;
        self.undated = None;
        self.undated_error = None;
        match self.tab {
            Tab::Summary => self.reload_summary(false, true, cx),
            Tab::Strategies => self.reload_strategy_base(false, true, true, cx),
            Tab::Calendar => {}
        }
    }

    /// Reload the full Summary without resetting tuner drafts.
    ///
    /// Args:
    ///     after_report: Whether this writer-driven catch-up may keep a settled snapshot while a
    ///         transient outcome with a scheduled correction remains.
    ///     show_overlay: Whether this refresh must block interaction with visible progress feedback.
    ///     cx: GPUI context used to run and publish the shared background query.
    pub(super) fn reload_summary(
        &mut self,
        after_report: bool,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        // Mark the request at its start so an error from another period cannot remain under the
        // current period label. Quote identity belongs to the request scope, so a MANUAL change
        // drops the previous figures: retaining them would render a scalar under the new
        // metric/core filters before its exact unit is known.
        //
        // A report-driven catch-up changes neither the period nor the filters, so the visible
        // snapshot stays until its replacement lands — the same rule `reload_strategy_base`
        // already follows. Dropping it would make the page blink through "loading" after every
        // live trade, and repeatedly under the current-rate mode, whose worker republishes on its
        // own schedule.
        if !after_report {
            self.data = ProfitLoadState::default();
            self.summary_derived = None;
        }
        // Presets keep their enum across civil rollovers while their resolved bounds slide, so
        // only the range can tell whether retained bucket hovers still name the same data.
        let active_range = self.active_period().range(self.bound_zone());
        let range_moved = active_range != self.data_range;
        self.summary_popup_hover
            .reset_for_reload(!after_report || range_moved);
        if !after_report || range_moved {
            // Bucket indices are time-ordered and survive a same-scope catch-up, but a moved
            // range shifts their meaning without changing the preset enum.
            self.hover_daily_bucket = None;
            self.hover_cum_bucket = None;
        }
        // Kinds are profit-sorted on every read, so an existing index can name another kind even
        // during a same-scope catch-up; a closed popup is safer than a silently wrong one.
        self.hover_kind = None;
        self.seq = self.seq.wrapping_add(1);
        let req = self.seq;
        let report_req = self.current_report_generation();
        // Record the ACTIVE tab's time window that `data` is being computed for.
        self.data_period = self.active_period();
        self.data_range = active_range;
        let q = self.query();
        let read_cores = self.core_metadata_due(cx);
        self.spawn_latest_db(
            &[bg::ReadLane::Summary],
            show_overlay,
            cx,
            move || moon_core::db::analytics::summary_data(&q, read_cores),
            move |this, result, cx| {
                if this.seq != req {
                    return; // The period or filters have already changed.
                }
                let data = result.data;
                let undated = result.undated;
                let cores = result.cores;
                // Computed before `data` moves into `apply` below.
                let data_outcome = CatchUpOutcome::of_scope(&data);
                let undated_error = undated.as_ref().err().cloned();
                let cores_error = cores
                    .as_ref()
                    .and_then(|cores| cores.as_ref().err())
                    .cloned();
                let transient = data_outcome.is_transient()
                    || CatchUpOutcome::of_read(&undated).is_transient()
                    || cores
                        .as_ref()
                        .is_some_and(|cores| CatchUpOutcome::of_read(cores).is_transient());
                if let Some(Ok(cores)) = cores {
                    this.cores = cores;
                    this.last_cores_at = Some(std::time::Instant::now());
                    this.core_refresh_needed = false;
                }
                // A snapshot survives only while a scheduled correction can replace it; otherwise
                // stale values would look current with no remaining correction path.
                let preserve_snapshot = !matches!(this.data, ProfitLoadState::Loading)
                    && this.keep_on_catch_up(after_report, data_outcome, report_req);
                if !preserve_snapshot {
                    this.data.apply(data);
                }
                this.data_dirty = refresh::report_result_is_stale(
                    report_req,
                    this.current_report_generation(),
                    !matches!(data_outcome, CatchUpOutcome::Replacement)
                        || undated_error.is_some()
                        || cores_error.is_some(),
                );
                this.apply_undated_result(undated, after_report, report_req);
                this.settle_report_refresh_retry(transient, cx);
                cx.notify();
            },
        );
    }

    /// Reload the compact Strategies base and optionally continue with its visible axis.
    ///
    /// Args:
    ///     after_report: Whether a transient outcome with a scheduled correction may preserve the
    ///         visible snapshot under report-style catch-up semantics.
    ///     chain_visible_axis: Whether a successful base read should continue into the active axis.
    ///     show_overlay: Whether this refresh must block interaction with visible progress feedback.
    ///     cx: GPUI context used to run and publish the shared background query.
    pub(super) fn reload_strategy_base(
        &mut self,
        after_report: bool,
        chain_visible_axis: bool,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        // Manual scope changes must not show values from the old scope. An automatic report
        // refresh keeps the current snapshot until its replacement lands, so the strategy list,
        // quote selector, and trade count do not blink through Loading after every live trade.
        if !after_report {
            self.strategy_data = ProfitLoadState::default();
        }
        self.seq = self.seq.wrapping_add(1);
        let req = self.seq;
        let report_req = self.current_report_generation();
        self.strategy_data_period = self.strat_period;
        let q = self.query_for(self.strat_period);
        let read_cores = self.core_metadata_due(cx);
        self.spawn_latest_db(
            &[bg::ReadLane::StrategyBase],
            show_overlay,
            cx,
            move || moon_core::db::analytics::strategy_base_data(&q, read_cores),
            move |this, result, cx| {
                if this.seq != req {
                    return;
                }
                let data = result.data;
                let undated = result.undated;
                let cores = result.cores;
                // Computed before `data` moves into `apply` below.
                let data_outcome = CatchUpOutcome::of_scope(&data);
                let data_error = data.as_ref().err().cloned();
                let undated_error = undated.as_ref().err().cloned();
                let cores_error = cores
                    .as_ref()
                    .and_then(|cores| cores.as_ref().err())
                    .cloned();
                let transient = data_outcome.is_transient()
                    || CatchUpOutcome::of_read(&undated).is_transient()
                    || cores
                        .as_ref()
                        .is_some_and(|cores| CatchUpOutcome::of_read(cores).is_transient());
                if let Some(Ok(cores)) = cores {
                    this.cores = cores;
                    this.last_cores_at = Some(std::time::Instant::now());
                    this.core_refresh_needed = false;
                }
                // Read `data` before `apply` moves it: the comparison needs both the currently
                // shown group set and the one about to be published.
                let core_names_changed = tuner::core_names_changed(
                    this.strategy_data.data().map(|d| d.strategies.as_slice()),
                    tuner::published_groups(&data),
                );
                // Keep a settled same-scope snapshot only until a scheduled correction replaces
                // it. An initial `Loading` state must publish its failure rather than remain
                // pending forever; `Split`, `NotReady`, and `Failed` are already settled
                // snapshots unless still transient (see `CatchUpOutcome`).
                let preserve_snapshot = !matches!(this.strategy_data, ProfitLoadState::Loading)
                    && this.keep_on_catch_up(after_report, data_outcome, report_req);
                if !preserve_snapshot {
                    this.strategy_data.apply(data);
                    // Measuring every core name is expensive, so invalidate only when its input
                    // text changed.
                    if core_names_changed {
                        this.strat_core_w = None;
                    }
                    // The numeric widths are measured from the figures themselves, which any
                    // replacement can move, so they are dropped with every published base.
                    this.strat_metric_w = None;
                    // Both caches describe the group set that was just replaced. The memo's key
                    // also carries that set's address, but an address is only unique among LIVE
                    // allocations: a failed load drops the old buffer and a later successful one
                    // can be handed the same address back, which unchanged filters would then
                    // accept as "same data". Dropping the memo here is what closes that.
                    this.strat_visible = None;
                }
                this.strategy_dirty = refresh::report_result_is_stale(
                    report_req,
                    this.current_report_generation(),
                    !matches!(data_outcome, CatchUpOutcome::Replacement)
                        || undated_error.is_some()
                        || cores_error.is_some(),
                );
                this.apply_undated_result(undated, after_report, report_req);
                let probe_took_over = probe_selects_strategy() && this.probe_select_first(cx);
                if refresh::strategy_base_allows_axis(
                    data_error.is_some(),
                    undated_error.is_some(),
                    cores_error.is_some(),
                ) && this.strategy_data.split().is_none()
                    && !probe_took_over
                    && chain_visible_axis
                    && this.tab == Tab::Strategies
                {
                    this.reload_axis_after_report(this.strat_mode, show_overlay, cx);
                }
                this.settle_report_refresh_retry(transient, cx);
                cx.notify();
            },
        );
    }

    /// Apply an undated-close result without replacing a settled strip with a transient alert
    /// that a scheduled correction will immediately remove.
    ///
    /// Every other failure still publishes: otherwise stale counts would look current without a
    /// correction path.
    ///
    /// Args:
    ///     result: Current undated-close read outcome.
    ///     after_report: Whether this is a writer-driven catch-up rather than a manual reload.
    ///     started_generation: Report generation captured immediately before the read started.
    pub(super) fn apply_undated_result(
        &mut self,
        result: moon_core::db::ReadResult<moon_core::db::analytics::UndatedCloses>,
        after_report: bool,
        started_generation: u64,
    ) {
        let outcome = CatchUpOutcome::of_read(&result);
        match result {
            Ok(undated) => {
                self.undated = Some(undated);
                self.undated_error = None;
            }
            Err(error) => {
                if self.keep_on_catch_up(after_report, outcome, started_generation) {
                    return;
                }
                self.undated = None;
                self.undated_error = Some(error);
            }
        }
    }

    // cal_query/cal_query_prev/reload_calendar live in calendar/mod.rs — a page's
    // recomputation belongs beside that page.
}
