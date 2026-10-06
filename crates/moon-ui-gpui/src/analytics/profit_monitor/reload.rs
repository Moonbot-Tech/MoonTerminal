//! Profit Monitor reload.

use super::*;

impl ProfitMonitorView {
    /// Sample live identity and valuation context on bounded wall-clock ticks.
    ///
    /// This avoids cloning every configured and live core name on unrelated high-rate Backend
    /// notifications while keeping reconnect and rename changes automatic.
    ///
    /// Args:
    ///     cx: View context used to own the recurring task.
    pub(super) fn start_context_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            loop {
                executor
                    .timer(duration_until_wall_clock_boundary(
                        SystemTime::now(),
                        CONTEXT_REFRESH_MS,
                    ))
                    .await;
                let alive =
                    cx.update(|cx| this.update(cx, |this, cx| this.sync_context(cx)).is_ok());
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    /// Mark the cached body dirty after one of its parent-owned inputs changes.
    ///
    /// GPUI cache invalidation follows view ancestry, not arbitrary entity reads. The body is a
    /// sibling of the clock, so body-state writers notify it explicitly while clock-only ticks do
    /// not.
    ///
    /// Args:
    ///     cx: Parent context used to notify the cached child entity.
    pub(super) fn invalidate_content(&self, cx: &mut Context<Self>) {
        self.content.update(cx, |_content, cx| cx.notify());
    }

    /// Apply the latest non-database labels and valuation mode.
    ///
    /// Args:
    ///     cx: View context used to read Backend and repaint or reload as required.
    pub(super) fn sync_context(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.read(cx);
        let next = retain_last_known_venues(&self.live, capture_live_context(backend));
        let valuation = backend.valuation_mode();
        let zone = moon_core::util::display_time::zone_or_utc(backend.header_clock_zone());
        let zone_changed = self.zone != zone;
        match context_change(&self.live, &next, self.valuation != valuation, zone_changed) {
            ContextChange::None => {}
            ContextChange::Regroup => {
                self.live = next;
                self.invalidate_content(cx);
                cx.notify();
            }
            ContextChange::Reload { restart_clock } => {
                self.live = next;
                self.valuation = valuation;
                self.zone = zone;
                if restart_clock {
                    self.start_clock_refresh(cx);
                }
                self.reload(false, cx);
            }
        }
    }

    /// Repaint the table when a core's run state, or a pending run intent, changed.
    ///
    /// Args:
    ///     cx: View context used to read the session and invalidate the cached body.
    pub(super) fn sync_run_state(&mut self, cx: &mut Context<Self>) {
        if !run_slots(self.prefs).any() {
            return;
        }
        // Both scopes the table can COMMAND, not just the one it displays. The per-row cells stand
        // for `core_order`, but the header's fleet cell stands for `action_core_ids`, which
        // deliberately still holds cores the active preset hides — a display boundary must not
        // narrow a command path. Folding only the displayed list would leave a hidden core's
        // run-state change invisible to this check, so the cached body would survive it and the
        // fleet button would keep offering the action the OLD folded state implied: press Stop
        // while the real fleet has gone mixed, and the command that reaches the visible cores is
        // the opposite of the one the button is now showing.
        // Deduped, and deduped IN ORDER on purpose. `Session::run_scope_rev` folds with a
        // non-commutative `mix`, so the token depends on both the membership AND the sequence:
        // folding a `HashSet` would hand it a different order per call and invalidate the cached
        // body forever, while folding the plain chain would pay a session-store lookup twice for
        // every core that is both displayed and active — which is most of a normal fleet.
        let mut commanded = self.live.core_order.clone();
        commanded.extend(
            self.live
                .action_core_ids
                .iter()
                .copied()
                .filter(|core| !self.live.core_order.contains(core)),
        );
        let rev = run_scope_rev(&self.backend, commanded, cx);
        if rev == self.run_rev {
            return;
        }
        self.run_rev = rev;
        self.invalidate_content(cx);
        cx.notify();
    }

    /// Arm the exact next wall-clock boundary for the selected period.
    ///
    /// Args:
    ///     cx: View context used to own the recurring task.
    pub(super) fn start_clock_refresh(&mut self, cx: &mut Context<Self>) {
        self.clock_timer_generation = self.clock_timer_generation.wrapping_add(1);
        let generation = self.clock_timer_generation;
        let Some(wait) = duration_until_period_refresh(self.period, self.zone, SystemTime::now())
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(wait).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.clock_timer_generation != generation {
                        return;
                    }
                    this.busy_retries.reset();
                    // A period boundary MOVES the query window — Today becomes another day,
                    // Yesterday shifts back one. Every core's newest trade is replaced by a
                    // different one, so the memory has to be dropped here even though the reload
                    // keeps the visible rows: diffing across the boundary is comparing two
                    // different questions.
                    this.rebaseline_arrivals();
                    // The wall-clock boundary replaces every value with another day's, so the
                    // measured column is released here too — `reload(true, ..)` keeps the visible
                    // rows and would otherwise carry yesterday's width into today.
                    this.release_profit_width();
                    this.reload(true, cx);
                    this.start_clock_refresh(cx);
                });
            });
        })
        .detach();
    }

    /// Return the report plus valuation generation represented by a monitor query.
    ///
    /// Returns:
    ///     Wrapping sum of both monotonic generations.
    pub(super) fn current_generation(&self) -> u64 {
        combined_generation(&self.report_generation, &self.valuation_generation)
    }

    /// Build the current real-trade query, scoped to the active preset's visible cores.
    ///
    /// Returns:
    ///     Quote-profit query for the selected period and global valuation mode. `cores` is
    ///     unfiltered exactly as an absent scope has always meant when the active preset hides no
    ///     configured core; otherwise it is [`scoped_query_core_ids`]'s inclusion list, built from
    ///     `self.live` and the previous successful read's own core set.
    pub(super) fn query(&self) -> Query {
        // The window resolves in THIS monitor's own selected zone. Under Phase 1 it could not:
        // the bounds went straight onto a core-local column, so computing "today" in the user's
        // zone slid the period sideways relative to the rows it filtered, and UTC was the only
        // self-consistent choice. Now the read side converts each bound onto the core's own clock
        // per offset group, so a civil boundary means what it says — and leaving this on UTC would
        // put a different "today" here than in the Report window beside it.
        //
        // Only the ZONE is carried: the offsets on this axis are replaced by the read path, which
        // loads them inside its own pinned snapshot.
        let axis = moon_core::db::ReportAxis::from_measured(Default::default(), self.zone);
        let (from, to) = self.period.range_at(now_utc(), axis.zone());
        // Read from `seen_data_cores`, NEVER from `self.data`: `reload` clears `data` to `Loading`
        // before this runs, so deriving the carry-forward from it would hand every scope-narrowing
        // read an empty list — and since that read's own result becomes the next universe, a
        // data-only core would be dropped permanently rather than for one cycle.
        let cores = scoped_query_core_ids(&self.live, &self.seen_data_cores);
        Query {
            axis,
            previous_period_basis: PreviousPeriodBasis::Civil,
            from,
            to,
            cores,
            side: SideFilter::All,
            emulator: Some(false),
            strategies: Vec::new(),
            // No mask: this window has no Analytics toolbar, so there is no mask to inherit.
            strategy_name_mask: String::new(),
            metric: ProfitMetric::Quote,
            valuation: self.valuation,
            // The monitor already renders one line per core, each in its own quote, so pinning the
            // scale here would convert figures the panel deliberately keeps native.
            prefer_usdt: false,
            core_names: moon_core::db::CoreNames::from_pairs(
                self.live
                    .core_names
                    .iter()
                    .map(|(id, name)| (*id, name.as_str())),
            ),
        }
    }

    /// Start a compact database read, preserving visible rows for automatic catch-up refreshes.
    ///
    /// Args:
    ///     after_report: Whether an existing visible snapshot may remain until replacement.
    ///     cx: View context used to spawn and publish the read.
    pub(super) fn reload(&mut self, after_report: bool, cx: &mut Context<Self>) {
        if !after_report {
            self.rebaseline_arrivals();
            // Same boundary the arrivals memory is dropped on: a new period, valuation mode or
            // display zone asks a different question, so the measured column starts over instead of
            // holding a width the answer no longer needs.
            self.release_profit_width();
        }
        if self.db_active {
            if !after_report {
                self.seq = self.seq.wrapping_add(1);
                self.data = ProfitLoadState::default();
                self.refresh_error = None;
                self.invalidate_content(cx);
                cx.notify();
            }
            // WRITER throughout: this window's timing is not what the urgency split was added
            // for, and it keeps exactly the behaviour it had before the gate learned to tell a
            // person from a commit stream.
            self.refresh
                .request_refresh(std::time::Instant::now(), false, RefreshUrgency::Writer);
            self.schedule_refresh(cx);
            return;
        }
        if !after_report {
            self.data = ProfitLoadState::default();
            self.refresh_error = None;
            self.invalidate_content(cx);
        }
        self.seq = self.seq.wrapping_add(1);
        let request = self.seq;
        let started_generation = self.current_generation();
        self.refresh
            .refresh_started(started_generation, std::time::Instant::now());
        let query = self.query();
        self.db_active = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    moon_core::db::analytics::profit_monitor_snapshot(&query)
                })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.db_active = false;
                    if this.seq == request {
                        let error = result.as_ref().err().cloned();
                        if !after_report || error.is_none() {
                            this.data.apply(result.map(|snapshot| {
                                this.currencies = snapshot.currencies;
                                snapshot.scope
                            }));
                            // Widen the carry-forward universe with what this read named, and only
                            // from a successful one: a failed or split read names no cores, and
                            // adopting its emptiness would discard the list a later scoped query
                            // needs. It ACCUMULATES rather than replaces, because a period holding
                            // no trade for a data-only core would otherwise forget that core and
                            // reintroduce the decay this field exists to stop. A core that has
                            // gone for good simply stops matching rows on later reads.
                            if let Some(data) = this.data.data() {
                                for core in &data.cores {
                                    if !this.seen_data_cores.contains(&core.core_uid) {
                                        this.seen_data_cores.push(core.core_uid);
                                    }
                                }
                            }
                            if matches!(this.data, ProfitLoadState::Split(_)) {
                                for partition in &this.currencies {
                                    for core in &partition.data.cores {
                                        if !this.seen_data_cores.contains(&core.core_uid) {
                                            this.seen_data_cores.push(core.core_uid);
                                        }
                                    }
                                }
                            }
                            this.observe_arrivals(cx);
                            this.invalidate_content(cx);
                        }
                        if error.is_none() {
                            this.refresh_error = None;
                        } else {
                            this.refresh_error.clone_from(&error);
                        }
                        let newer_generation = report_result_is_stale(
                            started_generation,
                            this.current_generation(),
                            false,
                        );
                        if newer_generation {
                            this.refresh.request_refresh(
                                std::time::Instant::now(),
                                false,
                                RefreshUrgency::Writer,
                            );
                        }
                        this.settle_busy_retry(error.as_ref(), cx);
                    }
                    this.schedule_refresh(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Observe a committed report or valuation generation and debounce its replacement read.
    ///
    /// Args:
    ///     cx: View context used to schedule the refresh.
    pub(super) fn observe_report_generation(&mut self, cx: &mut Context<Self>) {
        self.sync_context(cx);
        let generation = self.current_generation();
        if self
            .refresh
            .observe_generation(generation, std::time::Instant::now())
        {
            self.busy_retries.observe_generation();
            self.schedule_refresh(cx);
        }
    }

    /// Plan the single trailing refresh or timer.
    ///
    /// Args:
    ///     cx: View context used to arm work.
    pub(super) fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        match self.refresh.plan(std::time::Instant::now(), self.db_active) {
            RefreshPlan::Idle => {}
            RefreshPlan::Now { .. } => self.reload(true, cx),
            RefreshPlan::After(wait) => {
                cx.spawn(async move |this, cx| {
                    let executor = cx.update(|cx| cx.background_executor().clone());
                    executor.timer(wait).await;
                    cx.update(|cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.refresh.timer_fired();
                            this.schedule_refresh(cx);
                        });
                    });
                })
                .detach();
            }
        }
    }

    /// Apply the bounded automatic retry policy for transient SQLite contention.
    ///
    /// Args:
    ///     error: Latest query failure, if any.
    ///     cx: View context used to schedule a retry.
    pub(super) fn settle_busy_retry(&mut self, error: Option<&ReadFail>, cx: &mut Context<Self>) {
        if error.and_then(ReadFail::kind) != Some(FailKind::Busy) {
            self.busy_retries.resolve();
            return;
        }
        if self.busy_retries.claim() {
            // A bounded Busy retry keeps the quiet period: retrying at once would hammer a
            // database already under contention.
            self.refresh
                .request_refresh(std::time::Instant::now(), false, RefreshUrgency::Writer);
            self.schedule_refresh(cx);
        } else {
            log::warn!("profit monitor: automatic database retry budget exhausted");
        }
    }
}
