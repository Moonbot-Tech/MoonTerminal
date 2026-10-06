//! Analytics controls extracted from the window module.

use super::*;

impl AnalyticsView {
    /// Apply a period preset to the active tab and reload its scope immediately.
    ///
    /// Args:
    ///     p: Period selected by the user.
    ///     window: Window owning the shared date-picker states.
    ///     cx: GPUI context used to persist the choice and start the reload.
    pub(super) fn set_period(&mut self, p: Period, window: &mut Window, cx: &mut Context<Self>) {
        // Clicking the active preset remains an immediate manual refresh, independent of the
        // load-shed automatic path. The period bar edits the ACTIVE tab's time window: Summary and
        // Tuning are independent.
        let strat = self.tab == Tab::Strategies;
        if strat {
            self.strat_period = p;
        } else {
            self.period = p;
        }
        // A preset supersedes the custom range, so clear the from/to fields.
        if !matches!(p, Period::Custom(..)) {
            self.write_bound(Bound::From, None, window, cx);
            self.write_bound(Bound::To, None, window, cx);
        }
        // Persist the selection under its OWN key so the window reopens with it next time.
        let id = Some(p.persist_id());
        self.backend.update(cx, |b, _| {
            let slot = if strat {
                &mut b.layout.analytics_strat_period
            } else {
                &mut b.layout.analytics_period
            };
            if *slot != id {
                *slot = id;
                b.layout_dirty = true;
            }
        });
        self.reload(cx);
        cx.notify();
    }

    /// Synchronize the shared from/to pickers with the active tab's period, so after a tab switch
    /// the period bar shows that tab's OWN range rather than the previous tab's range.
    pub(super) fn sync_period_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (from, to) = match self.active_period() {
            Period::Custom(f, t) => (
                if f >= 0 {
                    date_range::dt_of_secs(f, self.bound_zone())
                } else {
                    None
                },
                date_range::field_of_exclusive(t, self.bound_zone()),
            ),
            _ => (None, None),
        };
        self.write_bound(Bound::From, from, window, cx);
        self.write_bound(Bound::To, to, window, cx);
    }

    /// The picker holding one edge of the custom range.
    pub(super) fn bound_picker(&self, bound: Bound) -> Entity<MoonDateTimePickerState> {
        match bound {
            Bound::From => self.cal_from.clone(),
            Bound::To => self.cal_to.clone(),
        }
    }

    /// Write one bound into its picker, restoring that field's default clock time when cleared.
    ///
    /// Args:
    ///     bound: Which edge is being written.
    ///     value: New bound, or `None` to empty the field.
    ///     window: Window forwarded to the picker's calendar.
    ///     cx: View context.
    pub(super) fn write_bound(
        &mut self,
        bound: Bound,
        value: Option<chrono::NaiveDateTime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bound_picker(bound).update(cx, |state, cx| {
            state.set_value(value, window, cx);
            if value.is_none() {
                // Clearing resets the picker's clock to midnight; the "to" field must go back to
                // the END of a day, or the next day picked there would select one minute of it.
                state.set_time(bound.default_time(), cx);
            }
        });
    }

    /// Put a cleared field's default clock time back after the user emptied it themselves.
    ///
    /// Guarded on the current time so the `Change` this emits cannot re-enter forever.
    pub(super) fn restore_default_time(&mut self, bound: Bound, cx: &mut Context<Self>) {
        let picker = self.bound_picker(bound);
        let default = bound.default_time();
        let state = picker.read(cx);
        if state.date().is_some() || state.time() == default {
            return;
        }
        picker.update(cx, |state, cx| state.set_time(default, cx));
    }

    /// Recompute the period from the from/to pickers. If both are empty, keep the existing period.
    /// Otherwise, an empty from means all history, an empty to means until tomorrow, and bounds are
    /// swapped if to precedes from.
    pub(super) fn apply_custom_range(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_default_time(Bound::From, cx);
        self.restore_default_time(Bound::To, cx);
        let mut f = self.cal_from.read(cx).value();
        let mut t = self.cal_to.read(cx).value();
        if let (Some(a), Some(b)) = (f, t)
            && b < a
        {
            (f, t) = (Some(b), Some(a));
            self.write_bound(Bound::From, f, window, cx);
            self.write_bound(Bound::To, t, window, cx);
        }
        if f.is_none() && t.is_none() {
            return;
        }
        let tomorrow = Period::Today.range(self.bound_zone()).1;
        let next = {
            let (from, to) = custom_bounds(f, t, tomorrow, self.bound_zone());
            Period::Custom(from, to)
        };
        // Programmatic writes (tab switch, preset reset, the swap above) emit `Change` too, so
        // without this the window would re-apply the period it already holds and loop.
        if self.active_period() == next {
            return;
        }
        self.set_period(next, window, cx);
    }

    /// Toggle one core or the All row in the multi-selection.
    ///
    /// The All row clears every explicit selection so only its own checkmark remains. A specific
    /// core toggles independently and therefore removes the All checkmark from the empty state.
    /// Auto on a concrete core pins the selector to its effective scope, so clicks are no-ops and
    /// the retained Classic selection remains byte-for-byte available when Auto ownership ends.
    /// Auto on Overview leaves the filter unpinned and edits the retained selection like Classic,
    /// while still carrying the group's action authority.
    ///
    /// Args:
    ///     core: Core to toggle, or `None` for the All row.
    ///     cx: Analytics context used to reload data and request a repaint.
    ///
    /// Returns:
    ///     Nothing; an unpinned selector updates in place, while a pinned selector leaves the
    ///     retained filter unchanged.
    pub(super) fn toggle_core(&mut self, core: Option<u64>, cx: &mut Context<Self>) {
        if self.core_filter_pinned() {
            return;
        }
        if crate::controls::toggle_core_selection(&mut self.sel_cores, core) {
            self.core_caption.manual_selection_changed();
            self.core_selection_changed(cx);
        }
    }

    /// Toggle every still-available core from one clicked exchange section.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A second
    /// click removes the exchange when all of its available cores are selected. Partial selections
    /// retain cores from other exchanges, while a stale-only batch is a no-op.
    /// Auto on a concrete core pins every exchange section to the workspace-derived scope, so
    /// clicks are no-ops and the retained Classic selection is neither persisted nor reloaded.
    /// Auto on Overview leaves the filter unpinned and edits the retained selection like Classic,
    /// while still carrying the group's action authority.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from the rendered Analytics exchange section.
    ///     cx: Analytics context used to persist and reload a changed selection.
    ///
    /// Returns:
    ///     Nothing; an unpinned selector persists changes atomically, while a pinned selector
    ///     retains the prior filter.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<u64>,
        cx: &mut Context<Self>,
    ) {
        if self.core_filter_pinned() {
            return;
        }
        let hidden = self.hidden_core_ids();
        let available = self
            .cores
            .iter()
            .map(|(core, _)| *core)
            .filter(|core| hidden.is_none_or(|h| !h.contains(core)))
            .collect();
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            self.core_caption.manual_selection_changed();
            self.core_selection_changed(cx);
        }
    }

    /// Persist the Analytics core filter and reload every dependent surface.
    ///
    /// Args:
    ///     cx: Analytics context used to update session state, reload data, and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the current core selection is published and reloaded in place.
    pub(super) fn core_selection_changed(&mut self, cx: &mut Context<Self>) {
        let selected = self.sel_cores.clone();
        let core_caption = self.core_caption.clone();
        self.backend.update(cx, |b, _| {
            b.ui_session.analytics.sel_cores = selected;
            b.ui_session.analytics.core_caption = core_caption;
        });
        self.reload(cx);
        cx.notify();
    }

    pub(super) fn set_side(&mut self, side: SideFilter, cx: &mut Context<Self>) {
        if self.side != side {
            self.side = side;
            self.reload(cx);
            cx.notify();
        }
    }

    pub(super) fn set_emu(&mut self, emu: Option<bool>, cx: &mut Context<Self>) {
        if self.emu != emu {
            self.emu = emu;
            self.reload(cx);
            cx.notify();
        }
    }

    /// Adopt typed mask text, persist it, and reload once the typing has settled.
    ///
    /// The decision is taken against [`Self::strategy_mask_applied`] — what the last STARTED read
    /// used — rather than against the mirrored text, because a `Change` event has already mirrored
    /// the value by the time the matching Enter or blur arrives. Comparing the two would make the
    /// commit path a no-op and leave the debounce as the only route, which is exactly the reload
    /// the commit exists to skip ahead of.
    ///
    /// Args:
    ///     value: Raw text now in the field. Trimming and folding belong to the query layer.
    ///     immediate: Whether the edit is finished (Enter, blur) rather than still being typed.
    ///     cx: Analytics view context used to persist, arm the timer, and reload.
    ///
    /// Returns:
    ///     Nothing.
    pub(super) fn set_strategy_mask(
        &mut self,
        value: String,
        immediate: bool,
        cx: &mut Context<Self>,
    ) {
        let text_changed = self.strategy_mask != value;
        if !text_changed && !immediate {
            return;
        }
        if text_changed {
            self.strategy_mask = value.clone();
            self.backend.update(cx, |b, _| {
                b.layout.analytics_strategy_mask = Some(value.clone());
                b.layout_dirty = true;
            });
        }
        // Any fresh decision retires whatever timer is still pending: dropping the task cancels it.
        self.strategy_mask_debounce = None;
        if StrategyQuery::parse(&self.strategy_mask_applied) == StrategyQuery::parse(&value) {
            // Nothing to read: the reads on screen already used an equivalent query.
            if text_changed {
                cx.notify();
            }
            return;
        }
        if immediate {
            // `reload` records the applied value itself.
            self.reload(cx);
            cx.notify();
            return;
        }
        self.strategy_mask_debounce = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(MASK_DEBOUNCE).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    // Something else may have reloaded meanwhile — a core, side or period change
                    // carries the live mask too — in which case this timer has nothing left to do.
                    // The handle is deliberately NOT cleared here: dropping a task from inside its
                    // own body cancels the body, and a spent handle sitting in the field until the
                    // next keystroke replaces it costs nothing.
                    if this.strategy_mask_applied == this.strategy_mask {
                        return;
                    }
                    this.reload(cx);
                    cx.notify();
                });
            });
        }));
        // Repaint at typing speed even though the database is not read yet.
        cx.notify();
    }

    /// Switch between raw quote money and percent. Every figure and tuner sweep is computed under
    /// the selected lens, so the switch persists and reloads rather than only changing labels.
    ///
    /// Args:
    ///     metric: New raw-quote or Percent lens.
    ///     cx: Analytics view context used to persist and reload.
    ///
    /// Returns:
    ///     Nothing.
    pub(super) fn set_metric_choice(
        &mut self,
        choice: toolbar::MetricChoice,
        cx: &mut Context<Self>,
    ) {
        let (metric, prefer_usdt) = choice.flags();
        if self.metric == metric && self.prefer_usdt == prefer_usdt {
            return;
        }
        self.metric = metric;
        self.prefer_usdt = prefer_usdt;
        self.backend.update(cx, |b, _| {
            b.layout.analytics_profit_percent = metric == ProfitMetric::Percent;
            b.layout.analytics_profit_usdt = prefer_usdt;
            b.layout_dirty = true;
        });
        self.reload(cx);
        cx.notify();
    }

    /// Collapse/expand the shared "Fact vs variants" KPI matrix. Collapsed keeps only its two
    /// top rows (trades + profit), so the fields grid below it fits on short screens. A pure
    /// display lens — unlike `set_metric` it changes no query, so it only persists and repaints.
    pub(super) fn toggle_kpi_collapsed(&mut self, cx: &mut Context<Self>) {
        self.kpi_collapsed = !self.kpi_collapsed;
        self.backend.update(cx, |b, _| {
            b.layout.analytics_kpi_collapsed = self.kpi_collapsed;
            b.layout_dirty = true;
        });
        cx.notify();
    }

    /// Collapse/expand the "By filter" distribution card. Collapsed keeps its title and subtitle
    /// and folds the chart away, so the fields grid above it fits on short screens.
    ///
    /// A pure display lens, like `toggle_kpi_collapsed`: it persists and repaints, and it must NOT
    /// gate the histogram read. `TunerState::needs_reload` counts `hist_dirty`, so a read suppressed
    /// while collapsed would leave that flag permanently set — the reload gate would re-fire every
    /// frame, and expanding would show a spinner where the user left a chart.
    pub(super) fn toggle_hist_collapsed(&mut self, cx: &mut Context<Self>) {
        self.hist_collapsed = !self.hist_collapsed;
        self.backend.update(cx, |b, _| {
            b.layout.analytics_hist_collapsed = self.hist_collapsed;
            b.layout_dirty = true;
        });
        cx.notify();
    }

    /// Collapse/expand the tuner's whole right-hand column, in every axis at once. Collapsed
    /// drops the "Fact vs variants" matrix and the axis' own tool below it, and the strategy
    /// list takes the freed width; the rail carrying this caret stays on screen in both states,
    /// because it is the only way back.
    ///
    /// A pure display lens, the widest one on this page, and the same rule as
    /// `toggle_hist_collapsed` with three times the blast radius: it persists and repaints, and
    /// it must NOT gate a read. Every axis' staleness gate — `TunerState::needs_reload` and its
    /// coins/time equivalents — counts dirty flags that only a COMPLETED read clears, so
    /// suppressing a read while the column is hidden would leave those flags permanently set:
    /// the reload gate would re-fire every frame in all three axes, and expanding would show a
    /// spinner where the user left numbers. Unlike `set_metric_choice`, this function must never
    /// grow a reload.
    ///
    /// It also leaves `kpi_collapsed` alone. With the column not built the matrix is dormant
    /// rather than contradictory, so expanding rebuilds it exactly as the user left it.
    pub(super) fn toggle_side_collapsed(&mut self, cx: &mut Context<Self>) {
        self.side_collapsed = !self.side_collapsed;
        self.backend.update(cx, |b, _| {
            b.layout.analytics_tuner_side_collapsed = self.side_collapsed;
            b.layout_dirty = true;
        });
        cx.notify();
    }
}
