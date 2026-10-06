//! Analytics construct extracted from the window module.

use super::*;

impl AnalyticsView {
    /// The time axis every replicated report timestamp in this window is read through.
    ///
    /// Answers from the CACHED axis rather than rebuilding one per call. A core with no
    /// measurement contributes no segment and therefore still reads exactly as stored, which is
    /// what reproduces MoonBot's own report for an unmeasured fleet.
    ///
    /// Returns:
    ///     The cached time axis for replicated report timestamps.
    pub(super) fn report_axis(&self) -> ReportAxis {
        self.axis.clone()
    }

    /// The zone a period BOUND and a calendar window resolve in.
    ///
    /// A bound is compared against the raw replicated column, so it must land on the SAME axis as
    /// that column — never on the user's display zone independently, which is what made a picked
    /// day select a different day's rows. Day labels follow it for the same reason: a caption that
    /// disagreed with the window it selects is worse than either answer alone.
    ///
    /// Returns:
    ///     The display zone carried by the cached report axis.
    pub(super) fn bound_zone(&self) -> chrono_tz::Tz {
        self.report_axis().zone()
    }
    /// Build an Analytics view from durable layout preferences and process-lifetime UI choices.
    ///
    /// Args:
    ///     backend: Shared application state containing layout and UI-session snapshots.
    ///     window: Newly opened Analytics window used to observe geometry and create controls.
    ///     cx: View context used to subscribe controls and start the initial reload.
    ///
    /// Returns:
    ///     A fully initialized Analytics view.
    pub(super) fn new(
        backend: Entity<Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Window geometry lives in the layout, as it does for Screener and Strategies.
        cx.observe_window_bounds(window, |this, window, cx| {
            let geom = crate::window::windowing::window_geom_rect(window, cx);
            this.backend.update(cx, |b, _| {
                let geom = geom.keeping_display_of(b.layout.analytics_window);
                if b.layout.analytics_window != Some(geom) {
                    b.layout.analytics_window = Some(geom);
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        // Observe the dedicated post-commit wake channel only while this view exists.
        let report_generation = backend
            .read(cx)
            .reports
            .as_ref()
            .map(|reports| reports.generation.clone());
        let valuation_generation = backend
            .read(cx)
            .valuation
            .as_ref()
            .map(|valuation| valuation.generation.clone());
        let (last_valuation_status_rev, valuation_status) = backend
            .read(cx)
            .valuation
            .as_ref()
            .map(|valuation| valuation.seed_status())
            .unwrap_or_default();
        let initial_report_generation = report_generation
            .as_ref()
            .map(|generation| generation.load(Ordering::Relaxed))
            .unwrap_or(0)
            .wrapping_add(
                valuation_generation
                    .as_ref()
                    .map(|generation| generation.load(Ordering::Relaxed))
                    .unwrap_or(0),
            );
        let report_revision = backend.read(cx).report_revision.clone();
        cx.observe(&report_revision, |this, _revision, cx| {
            this.observe_report_generation(cx);
        })
        .detach();
        let display_zone =
            moon_core::util::display_time::zone_or_utc(backend.read(cx).header_clock_zone());
        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _revision, cx| {
            let zone = moon_core::util::display_time::zone_or_utc(
                this.backend.read(cx).header_clock_zone(),
            );
            if zone == this.display_zone {
                return;
            }
            // The calendar day and the period fields live on the REPORT AXIS, not on the display
            // zone, so a zone change must leave them exactly where they are: re-projecting them
            // here is what used to slide the selected day sideways and change which trades the
            // period held. The re-projection helper this used to call was deleted with the
            // change: the axis carries the display zone now, so nothing re-projects a day.
            this.display_zone = zone;
            // The axis carries the zone beside the offsets, so it is stale even though nothing
            // new was measured.
            this.axis = this.backend.read(cx).report_axis(zone);
            this.coin_lists.invalidate_display_time();
            this.cal_dirty = true;
            this.reload(cx);
            cx.notify();
        })
        .detach();
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            let previous_selection = this.effective_strategy_selection();
            let next_workspace = analytics_workspace_scope(this.backend.read(cx));
            let next_display = analytics_display_scope(this.backend.read(cx));
            if next_workspace == this.workspace_scope && next_display == this.display_scope {
                return;
            }
            this.workspace_scope = next_workspace;
            this.display_scope = next_display;
            if this.effective_strategy_selection() != previous_selection {
                this.reconcile_workspace_strategy_scope();
            }
            // Retire every pending query identity before rebuilding from the new singleton scope.
            // The retained `sel_cores` and process-lifetime session snapshot are never written.
            this.reload(cx);
            cx.notify();
        })
        .detach();

        // Drain the active bulk-write watch on the same general signal every other panel already
        // reacts to for session data — never a timer of its own. `observe_in` (not `observe`) is
        // needed here, and only here among these three, because a clean resolution pushes a
        // window-level toast.
        cx.observe_in(&backend, window, |this, _backend, window, cx| {
            this.drain_strategy_edit_watch(window, cx);
        })
        .detach();

        // The strategy-name mask: restored before the first read, so the very first Summary the
        // window draws is already narrowed and the box already shows what narrowed it.
        let saved_strategy_mask = backend
            .read(cx)
            .layout
            .analytics_strategy_mask
            .clone()
            .unwrap_or_default();
        let strategy_mask_input = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(saved_strategy_mask.clone())
                .placeholder(t!("analytics.filter.strategy_mask_ph").to_string())
        });
        cx.subscribe(
            &strategy_mask_input,
            |this, input, event: &MoonInputEvent, cx| match event {
                MoonInputEvent::Change => {
                    let value = input.read(cx).value().to_string();
                    this.set_strategy_mask(value, false, cx);
                }
                // Finishing the edit commits at once: a user who presses Enter or clicks away has
                // stopped typing, and should not sit out the rest of the debounce.
                MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. } => {
                    let value = input.read(cx).value().to_string();
                    this.set_strategy_mask(value, true, cx);
                }
                _ => {}
            },
        )
        .detach();

        // Period: the previous layout selection, defaulting to the current calendar month.
        let saved_period = backend
            .read(cx)
            .layout
            .analytics_period
            .as_deref()
            .and_then(Period::from_id);
        // The Tuning period has its own persisted key, independent of Summary.
        let saved_strat_period = backend
            .read(cx)
            .layout
            .analytics_strat_period
            .as_deref()
            .and_then(Period::from_id);
        // Persisted "By filter" search knobs; `TunerState::load` owns their normalization.
        let saved_tuner_iters = backend.read(cx).layout.analytics_tuner_iters;
        let saved_tuner_edges = backend.read(cx).layout.analytics_tuner_edges;
        let saved_tuner_seed = backend.read(cx).layout.analytics_tuner_seed.clone();
        let saved_tuner_train = backend.read(cx).layout.analytics_tuner_train;
        let saved_tuner_fields = backend.read(cx).layout.analytics_tuner_fields.clone();
        let saved_tuner_compose = backend.read(cx).layout.analytics_tuner_compose;
        // The "Entry/Exit" axis' settings. The model's are process-wide — every replay path
        // reads them (`ticks::model_cfg`) — and the saved ones are what the last window left.
        let mut ticks = tuner::TicksState::default();
        if let Some(saved) = backend.read(cx).layout.analytics_ticks.as_ref() {
            ticks.restore(saved);
            tuner::ticks::model_cfg::replace(saved.model);
            tuner::ticks::tail::replace(saved.min_tail_s);
        }
        // Strategy-list sort is process-persistent. Unknown keys return to the same
        // profit-descending default used before this preference existed.
        let saved_strat_sort =
            tuner::restore_strat_sort(backend.read(cx).layout.analytics_strat_sort.clone());
        let saved_coin_sort = crate::persistence::table_persist::saved_sort(
            backend.read(cx),
            "analytics-tuner-coins:win",
        );
        // Profit metric from the previous run (default raw quote money).
        let saved_metric = if backend.read(cx).layout.analytics_profit_percent {
            ProfitMetric::Percent
        } else {
            ProfitMetric::Quote
        };
        // Money scale from the previous run (default: follow each period's own quote).
        let saved_prefer_usdt = backend.read(cx).layout.analytics_profit_usdt;
        // KPI matrix collapse state from the previous run (default expanded).
        let saved_kpi_collapsed = backend.read(cx).layout.analytics_kpi_collapsed;
        // Distribution card collapse state from the previous run (default expanded).
        let saved_hist_collapsed = backend.read(cx).layout.analytics_hist_collapsed;
        // "Profit by core" card mode from the previous run (default: the compact overview).
        let saved_cores_show_all = backend.read(cx).layout.analytics_cores_show_all;
        // Right-column collapse state from the previous run (default expanded).
        let saved_side_collapsed = backend.read(cx).layout.analytics_tuner_side_collapsed;
        // Visible strategy-list columns from the previous run, one mask per axis. An older
        // config holding the single-mask key seeds all three, so a choice already made is
        // carried over instead of reset; absent entirely, each axis takes its own default.
        let saved_strat_cols = {
            let layout = &backend.read(cx).layout;
            tuner::restore_strat_columns(
                layout.analytics_strat_cols_modes2,
                layout.analytics_strat_cols_modes,
                layout.analytics_strat_cols2,
            )
        };
        // Calendar mode from the previous run, defaulting to Month.
        let saved_mode = backend
            .read(cx)
            .layout
            .analytics_heat_mode
            .as_deref()
            .and_then(calendar::CalMode::from_id)
            .unwrap_or(calendar::CalMode::Month);
        let session = backend.read(cx).ui_session.analytics.clone();
        let workspace_scope = analytics_workspace_scope(backend.read(cx));
        let display_scope = analytics_display_scope(backend.read(cx));

        // From/to date+time fields: any day or clock change switches the period to Custom. The
        // popup stays open after a day is clicked so the clock drums under the calendar remain
        // reachable — that is the picker's own contract, not something the window drives.
        let cal_from = cx.new(|cx| date_range::bound_picker(Bound::From, window, cx));
        let cal_to = cx.new(|cx| date_range::bound_picker(Bound::To, window, cx));
        // A seeded bound must be read on the axis it will be COMPARED on, not on the display zone;
        // `self` does not exist yet, so the same answer is resolved from a fresh axis here.
        let bound_zone = ReportAxis::identity_core_local().zone();
        // Armed probe → open straight on the surface under observation, so the channel
        // reports the coin table rather than the summary nobody asked about.
        let probe = probe_enabled();
        let tab = if probe { Tab::Strategies } else { session.tab };
        // Seed the fields from the period of the tab actually being opened, not from Summary's:
        // each tab keeps its own, so a tuning session reopened on a custom range must SHOW it.
        if let Some(Period::Custom(f, t)) = seed_period(tab, saved_period, saved_strat_period) {
            if let Some(d) = (f >= 0)
                .then(|| date_range::dt_of_secs(f, bound_zone))
                .flatten()
            {
                cal_from.update(cx, |s, cx| s.set_value(Some(d), window, cx));
            }
            if let Some(d) = date_range::field_of_exclusive(t, bound_zone) {
                cal_to.update(cx, |s, cx| s.set_value(Some(d), window, cx));
            }
        }
        let mut cal_subs = Vec::new();
        for picker in [&cal_from, &cal_to] {
            cal_subs.push(cx.subscribe_in(
                picker,
                window,
                |this, picker, ev: &MoonDateTimePickerEvent, window, cx| {
                    let MoonDateTimePickerEvent::Change(_) = ev;
                    // While the popup is open the user is still composing the bound — every drum
                    // step would otherwise reload every axis.
                    if picker.read(cx).is_open() {
                        this.range_dirty = true;
                        return;
                    }
                    this.apply_custom_range(window, cx);
                },
            ));
            // The picker only repaints on open/close, so the commit rides its notify rather than
            // an event of its own.
            cal_subs.push(cx.observe_in(picker, window, |this, picker, window, cx| {
                if picker.read(cx).is_open() || !this.range_dirty {
                    return;
                }
                this.range_dirty = false;
                this.apply_custom_range(window, cx);
            }));
        }
        let saved_valuation_mode = backend.read(cx).valuation_mode();
        let saved_core_names = backend.read(cx).report_core_names();
        // Seeded so a window opened on a fleet measured in an earlier session renders corrected
        // times on its first paint, not on the first backend wake after it.
        let seeded_axis = backend.read(cx).report_axis(display_zone);
        let mut this = Self {
            backend,
            display_zone,
            axis: seeded_axis,
            display_zone_fields_dirty: false,
            report_generation,
            valuation_generation,
            valuation_status,
            last_valuation_status_rev,
            report_refresh: RefreshGate::new(initial_report_generation, std::time::Instant::now()),
            report_busy_retries: BusyRetryBudget::default(),
            tab,
            period: saved_period.unwrap_or(Period::CurMonth),
            strat_period: saved_strat_period.unwrap_or(Period::CurMonth),
            data_period: saved_period.unwrap_or(Period::CurMonth),
            data_range: saved_period.unwrap_or(Period::CurMonth).range(bound_zone),
            data_dirty: false,
            cores: Vec::new(),
            last_cores_at: None,
            core_refresh_needed: false,
            core_refresh_timer_armed: false,
            workspace_scope,
            display_scope,
            sel_cores: session.sel_cores,
            core_caption: session.core_caption,
            side: SideFilter::All,
            // Default to Real, as in Report, because emulated trades add noise to the statistics.
            emu: Some(false),
            strategy_mask: saved_strategy_mask.clone(),
            strategy_mask_applied: saved_strategy_mask,
            strategy_mask_input,
            strategy_mask_debounce: None,
            metric: saved_metric,
            prefer_usdt: saved_prefer_usdt,
            valuation_mode: saved_valuation_mode,
            core_names: saved_core_names,
            data: ProfitLoadState::default(),
            summary_derived: None,
            strategy_data: ProfitLoadState::default(),
            strategy_data_period: saved_strat_period.unwrap_or(Period::CurMonth),
            strategy_dirty: true,
            undated: None,
            undated_error: None,
            undated_expanded: session.undated_expanded,
            write_error: None,
            strategy_edit_watch: None,
            strat_purge: None,
            purge_seq: 0,
            busy_ops: 0,
            db_ops: 0,
            latest_reads: bg::LatestReads::default(),
            busy_since: None,
            seq: 0,
            hover_daily_bucket: None,
            hover_cum_bucket: None,
            hover_kind: None,
            summary_popup_hover: summary::PopupHover::default(),
            show_all_core_ranks: saved_cores_show_all,
            sel_strategy: None,
            sel_extra: Vec::new(),
            strat_search: String::new(),
            strat_type: None,
            strat_active_only: true,
            strat_lists: tuner::StratListFilter::All,
            strat_search_input: None,
            strat_sort: saved_strat_sort,
            strat_cols: saved_strat_cols,
            strat_core_w: None,
            strat_metric_w: None,
            strat_visible: None,
            strat_scroll: MoonVirtualListScrollHandle::new(),
            cal_days: ProfitLoadState::default(),
            cal_seq: 0,
            cal_dirty: true,
            cal_mode: saved_mode,
            cal_ym: {
                use chrono::Datelike;
                let d = day_of_secs(moon_core::util::now_unix_ms_i64() / 1000, bound_zone)
                    .unwrap_or_default();
                (d.year(), d.month())
            },
            cal_day: moon_core::util::display_time::bucket_start(
                moon_core::util::now_unix_ms_i64() / 1000,
                86_400,
                bound_zone,
            )
            .unwrap_or(0),
            cal_prev: LoadState::default(),
            strat_mode: if probe {
                tuner::StratMode::Coins
            } else {
                session.strat_mode
            },
            kpi_collapsed: saved_kpi_collapsed,
            hist_collapsed: saved_hist_collapsed,
            side_collapsed: saved_side_collapsed,
            clear_shell: None,
            tuner: tuner::TunerState::load(
                saved_tuner_iters,
                saved_tuner_edges,
                saved_tuner_seed,
                saved_tuner_train,
                saved_tuner_fields,
                saved_tuner_compose,
            ),
            coins: tuner::CoinsState::load(saved_coin_sort),
            ticks,
            coin_lists: tuner::CoinListsState::default(),
            time_tuner: tuner::TimeTunerState::load(),
            cal_from,
            cal_to,
            range_dirty: false,
            integrity_poll_armed: false,
            _cal_subs: cal_subs,
            focus: cx.focus_handle(),
        };
        this.reload(cx);
        this
    }
}
