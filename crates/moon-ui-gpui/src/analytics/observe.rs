//! Analytics observe extracted from the window module.

use super::*;

impl AnalyticsView {
    /// Return the latest report-derived generation visible to this Analytics window.
    ///
    /// Returns:
    ///     Wrapping sum of report and valuation generations.
    pub(super) fn current_report_generation(&self) -> u64 {
        let reports = self
            .report_generation
            .as_ref()
            .map(|generation| generation.load(Ordering::Relaxed))
            .unwrap_or(0);
        let valuation = self
            .valuation_generation
            .as_ref()
            .map(|generation| generation.load(Ordering::Relaxed))
            .unwrap_or(0);
        reports.wrapping_add(valuation)
    }

    /// Refresh published valuation health, without marking any report-derived result stale.
    ///
    /// Health is deliberately kept out of `current_report_generation`: a stall changes no rows, and
    /// feeding it to the refresh gate would reload every Analytics surface on health-only
    /// transitions from a provider that is failing anyway.
    ///
    /// Args:
    ///     cx: Analytics window context notified only when the published health moved.
    pub(super) fn observe_valuation_health(&mut self, cx: &mut Context<Self>) {
        let mut last = self.last_valuation_status_rev;
        let refreshed = self
            .backend
            .read(cx)
            .valuation
            .as_ref()
            .and_then(|valuation| valuation.status_if_changed(&mut last));
        let Some(status) = refreshed else {
            return;
        };
        self.last_valuation_status_rev = last;
        self.valuation_status = status;
        cx.notify();
    }

    /// Adopt a newly measured core offset, catching up every surface that was computed on the
    /// old one.
    ///
    /// Polled beside the valuation health rather than pushed, following this window's own idiom.
    /// Unlike health, an adoption DOES change rows — every date bucket, every calendar cell and
    /// every period bound moves. But the SCOPE (period, filters) does not, so this is a
    /// writer-driven catch-up, not a user reload: the visible snapshot stays on screen, with no
    /// blocking overlay, until the replacement lands. The observer retires EVERY in-flight read
    /// identity for the old axis — `seq`, `cal_seq`, `cancel_reads_for_axis_move`, plus `time_tuner`,
    /// `coins` and `coin_lists` `invalidate()` for the axes that keep their own request
    /// generations — because a cancelled read is not silently dropped: the DB layer raises a
    /// real SQLite interrupt that gets classified as a durable `Settled` failure, so a read
    /// whose identity was not retired would pass its own `seq != req` guard and publish that
    /// failure as if it were a real result. Retiring the tuner's read identities still clears any
    /// unsaved filter draft, matching this observer's behavior before it stopped calling
    /// `reload()` for axis changes.
    ///
    /// The "By filter" joint suggestion is the one exception. It runs through `spawn_db`, which
    /// installs no read-cancellation token, so it is not among the lanes `cancel_latest_reads`
    /// retires — no interrupt can reach it, and therefore no fake `Settled` can be published for
    /// it. Once that run has materialized its sample, an adoption does not rescan it; the result
    /// is captioned as fitted across the move. A minutes-long composition is the most expensive
    /// thing this window does, and a report generation advance — a strictly larger change —
    /// already does not retire it (`TunerState::mark_report_stale`).
    /// `TunerState::invalidate_for_axis` is that path. The Entry/Exit search is the other: its lane
    /// is left out of the cancel and `TicksState::invalidate_for_axis` keeps it running. With no
    /// joint run live the tuner is invalidated exactly as before: drafts cleared, every identity
    /// retired.
    ///
    /// Args:
    ///     cx: Analytics window context used to schedule a catch-up only when the axis moved.
    ///
    /// Returns:
    ///     Nothing; an axis change retires in-flight read identities other than a live joint
    ///     suggestion, and schedules a writer-driven catch-up.
    pub(super) fn observe_report_axis(&mut self, cx: &mut Context<Self>) {
        let axis = self.backend.read(cx).report_axis(self.display_zone);
        if axis == self.axis {
            return;
        }
        if refresh::defer_axis_adoption(
            self.visible_first_load_pending(),
            // `db_ops` only: those reads finish through `bg`, whose completion re-polls the
            // axis; a save or purge counted in `busy_ops` alone never would.
            self.db_ops > 0,
        ) {
            return;
        }
        self.axis = axis;
        self.seq = self.seq.wrapping_add(1);
        self.cal_seq = self.cal_seq.wrapping_add(1);
        self.cancel_reads_for_axis_move();
        self.tuner.invalidate_for_axis();
        self.time_tuner.invalidate();
        self.coins.invalidate();
        self.ticks.invalidate_for_axis();
        self.coin_lists.invalidate();
        self.mark_report_data_stale();
        self.request_report_refresh(RefreshUrgency::Writer, false, cx);
        cx.notify();
    }

    /// Whether the visible tab still has no settled result to keep on screen.
    ///
    /// Returns:
    ///     `true` while the visible surface shows its initial loading state.
    pub(super) fn visible_first_load_pending(&self) -> bool {
        match self.tab {
            Tab::Summary => matches!(self.data, ProfitLoadState::Loading),
            Tab::Strategies => matches!(self.strategy_data, ProfitLoadState::Loading),
            Tab::Calendar => matches!(self.cal_days, ProfitLoadState::Loading),
        }
    }

    /// Adopt a valuation mode saved in Settings, and reload if it moved.
    ///
    /// The mode is application-wide and is edited nowhere in this window, so it can change under
    /// an open Analytics window at any time.
    /// Mirrored into a field because [`Self::query`] is called from helpers that hold no `App`,
    /// and synced HERE rather than at render because adopting it schedules a reload — which the
    /// Analytics render root must never do.
    ///
    /// Args:
    ///     cx: Analytics window context used to read the backend and schedule the reload.
    pub(super) fn observe_valuation_mode(&mut self, cx: &mut Context<Self>) {
        let mode = self.backend.read(cx).valuation_mode();
        if self.valuation_mode == mode {
            return;
        }
        self.valuation_mode = mode;
        self.reload(cx);
    }

    /// Adopt core renames saved in Settings, and reload if a configured name moved.
    ///
    /// Mirrored and synced like [`Self::observe_valuation_mode`], for the same reasons.
    ///
    /// Args:
    ///     cx: Analytics window context used to read the backend and schedule the reload.
    pub(super) fn observe_core_names(&mut self, cx: &mut Context<Self>) {
        let names = self.backend.read(cx).report_core_names();
        if self.core_names == names {
            return;
        }
        self.core_names = names;
        self.reload(cx);
    }

    /// Mark report-derived caches stale and schedule a load-shed automatic refresh.
    ///
    /// This poll also adopts application-wide valuation-mode changes before comparing data
    /// generations, because switching modes changes query results without committing report rows.
    ///
    /// Args:
    ///     cx: GPUI context used to schedule or start the refresh.
    pub(super) fn observe_report_generation(&mut self, cx: &mut Context<Self>) {
        self.observe_valuation_health(cx);
        self.observe_report_axis(cx);
        self.observe_valuation_mode(cx);
        self.observe_core_names(cx);
        let generation = self.current_report_generation();
        if !self
            .report_refresh
            .observe_generation(generation, std::time::Instant::now())
        {
            return;
        }
        self.report_busy_retries.observe_generation();
        self.mark_report_data_stale();
        self.schedule_report_refresh(cx);
    }

    /// Mark report-derived results stale without clearing drafts or retiring snapshot progress.
    ///
    /// The method has no return value; the refresh gate later reloads the visible surface.
    pub(super) fn mark_report_data_stale(&mut self) {
        self.data_dirty = true;
        self.strategy_dirty = true;
        self.cal_dirty = true;
        self.tuner.mark_report_stale();
        self.time_tuner.mark_report_stale();
        self.coins.mark_report_stale();
        self.ticks.mark_report_stale();
    }

    /// Acknowledge every committed generation visible when a refresh begins.
    pub(super) fn acknowledge_report_refresh(&mut self) {
        let generation = self.current_report_generation();
        self.report_refresh
            .refresh_started(generation, std::time::Instant::now());
    }

    /// Settle the transient retry episode and optionally schedule its next bounded attempt.
    ///
    /// Permanent corruption and unclassified I/O failures remain visible instead of creating an
    /// endless full-history retry loop.
    ///
    /// Args:
    ///     transient: Whether any part of the completed read classified as a transient outcome.
    ///     cx: GPUI context used to arm the quiet-period retry.
    pub(super) fn settle_report_refresh_retry(&mut self, transient: bool, cx: &mut Context<Self>) {
        if !transient {
            self.report_busy_retries.resolve();
            return;
        }
        if !self.report_busy_retries.claim() {
            log::warn!("analytics: automatic database retry budget exhausted");
            return;
        }
        // WRITER urgency lets this follow-up share the quiet period with a fresh report
        // generation instead of immediately starting another full-period read.
        self.report_refresh.request_refresh(
            std::time::Instant::now(),
            false,
            RefreshUrgency::Writer,
        );
        self.schedule_report_refresh(cx);
    }

    /// Decide whether a writer-driven catch-up outcome may keep a visible snapshot instead of
    /// publishing it.
    ///
    /// Centralizing the predicate keeps every load surface aligned as their call sites evolve. A
    /// preserved snapshot is always covered by a scheduled correction: a transient outcome with a
    /// scheduled correction (retry allowance or a newer generation) left, never a bare exhausted
    /// budget.
    ///
    /// Args:
    ///     after_report: Whether this is a writer-driven catch-up rather than a manual reload.
    ///     outcome: The classified outcome this read completed with.
    ///     started_generation: Report generation captured immediately before the read started.
    ///
    /// Returns:
    ///     `true` only when the outcome is transient AND a correction is already scheduled.
    pub(super) fn keep_on_catch_up(
        &self,
        after_report: bool,
        outcome: CatchUpOutcome,
        started_generation: u64,
    ) -> bool {
        refresh::preserve_on_catch_up(
            after_report,
            outcome,
            self.report_busy_retries.has_allowance(),
            started_generation != self.current_report_generation(),
        )
    }

    /// Queue a visible catch-up behind any Analytics database work already in flight.
    ///
    /// Args:
    ///     urgency: Who asked; a user's request skips the writer's quiet period.
    ///     show_overlay: Whether a user action requires blocking progress feedback.
    ///     cx: GPUI context used to arm the shared refresh gate.
    pub(super) fn request_report_refresh(
        &mut self,
        urgency: RefreshUrgency,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        self.report_refresh
            .request_refresh(std::time::Instant::now(), show_overlay, urgency);
        self.schedule_report_refresh(cx);
    }

    /// Plan or start the sole trailing report refresh for this open window.
    ///
    /// Args:
    ///     cx: GPUI context used to arm a timer or start database work.
    pub(super) fn schedule_report_refresh(&mut self, cx: &mut Context<Self>) {
        let db_active = self.db_ops > 0 || self.busy_ops > 0;
        match self
            .report_refresh
            .plan(std::time::Instant::now(), db_active)
        {
            RefreshPlan::Idle => {}
            RefreshPlan::Now { show_overlay } => {
                self.refresh_visible_report_data(show_overlay, cx);
            }
            RefreshPlan::After(wait) => {
                cx.spawn(async move |this, cx| {
                    let executor = cx.update(|cx| cx.background_executor().clone());
                    executor.timer(wait).await;
                    cx.update(|cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.report_refresh.timer_fired();
                            this.schedule_report_refresh(cx);
                        });
                    });
                })
                .detach();
            }
        }
    }

    /// Recompute only the surface visible when a report-driven refresh becomes due.
    ///
    /// Strategies uses a compact list/coin base instead of paying for Summary-only charts,
    /// rankings, comparisons, and per-core series on every report commit.
    ///
    /// Args:
    ///     show_overlay: Whether coalesced user work requires blocking progress feedback.
    ///     cx: GPUI context used to start the visible surface's background reads.
    pub(super) fn refresh_visible_report_data(
        &mut self,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        self.acknowledge_report_refresh();
        let base_dirty = match self.tab {
            // Ask whether a core scan is DUE, never whether one is OWED.
            // `core_metadata_due` sets `core_refresh_needed` on EVERY call and clears it only
            // when cores were actually read, while cores are due once a minute — so the owed
            // flag is true for ~59 seconds out of every 60, and reading it here made every
            // report commit and every tab entry re-run the whole base scan over an already
            // current list. Nothing goes stale: `schedule_core_metadata_refresh` arms its own
            // timer and asks for the refresh the moment the 60-second cadence elapses.
            Tab::Strategies => {
                self.strategy_dirty
                    || refresh::core_metadata_wait(self.last_cores_at, std::time::Instant::now())
                        .is_zero()
            }
            _ => self.data_dirty,
        };
        match visible_refresh(self.tab, base_dirty) {
            VisibleRefresh::Summary => self.reload_summary(true, show_overlay, cx),
            VisibleRefresh::StrategyBaseAndAxis => {
                self.reload_strategy_base(true, true, show_overlay, cx);
            }
            VisibleRefresh::StrategyAxis => {
                self.reload_axis_after_report(self.strat_mode, show_overlay, cx);
            }
            VisibleRefresh::Calendar => self.reload_calendar_after_report(show_overlay, cx),
        }
    }

    /// Period of the active tab. Tuning keeps its OWN time window, separate from Summary.
    /// Calendar does not use the period bar because it has its own navigation, and
    /// `reload_calendar` builds its query directly without calling this method.
    pub(super) fn active_period(&self) -> Period {
        match self.tab {
            Tab::Strategies => self.strat_period,
            _ => self.period,
        }
    }

    /// Decide whether the shared core selector is due and preserve its original deadline.
    ///
    /// Args:
    ///     cx: GPUI context used to arm the sole trailing metadata timer.
    ///
    /// Returns:
    ///     `true` when the caller should include cores in its compound snapshot.
    pub(super) fn core_metadata_due(&mut self, cx: &mut Context<Self>) -> bool {
        let wait = refresh::core_metadata_wait(self.last_cores_at, std::time::Instant::now());
        self.core_refresh_needed = true;
        if wait.is_zero() {
            return true;
        }
        self.schedule_core_metadata_refresh(wait, cx);
        false
    }

    /// Arm one trailing core-list refresh shared by every Analytics tab.
    ///
    /// Args:
    ///     wait: Remaining time until the one-minute metadata cadence.
    ///     cx: GPUI context used to run and publish the timer.
    pub(super) fn schedule_core_metadata_refresh(
        &mut self,
        wait: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        if self.core_refresh_timer_armed {
            return;
        }
        self.core_refresh_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(wait).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.core_refresh_timer_armed = false;
                    if !this.core_refresh_needed {
                        return;
                    }
                    let remaining =
                        refresh::core_metadata_wait(this.last_cores_at, std::time::Instant::now());
                    if remaining.is_zero() {
                        // A background cadence, not a person: it keeps the quiet period.
                        this.request_report_refresh(RefreshUrgency::Writer, false, cx);
                    } else {
                        this.schedule_core_metadata_refresh(remaining, cx);
                    }
                });
            });
        })
        .detach();
    }
}
