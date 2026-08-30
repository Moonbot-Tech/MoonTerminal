//! Toolbar metric popups (TP/SL/Lev): open-state reconciliation, editor seeding, and guarded edits
//! bound either to the window group or to the leverage core and market captured at open time.

use gpui::*;

use moon_ui::{MoonInputState, MoonSliderEvent, MoonSliderState};

use moon_core::feed::ClientSettingsEdit;

use crate::controls;

use super::Shell;

impl Shell {
    /// Drop an open metric popup that no longer belongs where it was opened.
    ///
    /// Two classes of change can orphan it, neither of which produces an event to hang this on, so
    /// both are reconciled at render time like the state polled by the panels:
    ///
    /// * **the metric stopped being editable** (its SL toggle switched off, the manual strategy
    ///   armed, or Auto scope switched to Overview and no longer names a leverage core).
    ///   `MoonPopover` then renders only a disabled trigger and fires no `on_open_change`,
    ///   so the popup vanishes while the state stays `Some` — and pops back open, unclicked, the
    ///   moment the metric is available again, holding values seeded before the change.
    /// * **the edit target changed underneath it.** A different selected core or Main chart market
    ///   moves leverage; Auto Overview removes its target altogether. Group-local TP and SL
    ///   deliberately keep the same address across core
    ///   changes. Event handlers resolve the address when they fire, so availability alone cannot
    ///   distinguish a stale leverage popup from a live one.
    ///
    /// Lives beside the state it guards rather than in the frame-composition module, so the next
    /// rule added to [`controls::TradeMetric`] is edited within sight of it.
    pub(super) fn reconcile_metric_popup(&mut self, cx: &App) {
        let manual_core =
            controls::toolbar::effective_manual_strategy_core(&self.backend, &self.group, cx);
        let stale = self.open_metric_popup.as_ref().is_some_and(|open| {
            let b = self.backend.read(cx);
            !open.target.is_live(open.metric, b, &self.group)
                || !open.metric.available(b, &self.group, manual_core)
        });
        if stale {
            self.open_metric_popup = None;
        }
    }

    /// Commit a group-exit edit on behalf of the open TP or SL popup.
    ///
    /// Refuses when the popup is not open for `metric`, or when the address it was seeded from is
    /// no longer the one this metric resolves to. That check has to happen HERE, at event time:
    /// [`Self::reconcile_metric_popup`] takes a stale popup off the screen, but only at the next
    /// render, and repaints pass three stacked throttles. A slider drag inside that window would
    /// otherwise continue changing the no-longer-visible core or leverage market.
    pub(super) fn commit_metric_edit(
        &self,
        metric: controls::TradeMetric,
        edit: ClientSettingsEdit,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open_metric_popup.as_ref() else {
            return;
        };
        let is_live = {
            let b = self.backend.read(cx);
            open.metric == metric && open.target.is_live(metric, b, &self.group)
        };
        if !is_live {
            return;
        }
        self.backend.update(cx, |b, _| {
            b.edit_group_exit(&self.group, edit);
        });
    }

    /// Open or close a toolbar metric's popup — the `on_open_change` of its anchored `MoonPopover`.
    ///
    /// Opening seeds the slider and field with the current group or leverage-target value.
    ///
    /// The `EngageMainTakeProfit` edit goes out ONLY on OPENING TP. `MoonPopover` fires
    /// `on_open_change` for every close — a click elsewhere, Escape, a second click on the trigger —
    /// and none of those is the user acting on TP. Closing therefore only clears UI state.
    ///
    /// Opening is a no-op unless both the target and its seed value exist. Group-local TP and SL
    /// always satisfy that guard; leverage still needs a core, Main-chart market, and known value.
    /// Without the guard, its long-lived editor could show stale data from the previous target.
    pub(crate) fn set_metric_popup_open(
        &mut self,
        metric: controls::TradeMetric,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if open {
            let b = self.backend.read(cx);
            // BOTH an address and a value to show, or the popup does not open at all.
            //
            // The editors are long-lived entities: they keep whatever the LAST popup left in them.
            // Group-local TP and SL always have a complete value. Leverage seeding can still give
            // up when there is no active core, Main-chart market, or leverage entry; opening its
            // editor in that state would display the previous core's or coin's number as current.
            // For leverage that is not merely misleading: Apply sends the FIELD, which the user need
            // never have touched, straight to the exchange.
            //
            // This is a backstop, not the user-facing rule: the toolbar folds the same readiness
            // into the button's enabled state, so a button that cannot open is drawn disabled rather
            // than swallowing the click.
            let Some(target) = metric.target(b, &self.group) else {
                return;
            };
            if metric.seed_value(b, &self.group).is_none() {
                return;
            }
            // The coin's stated maximum leverage, read ONCE here so the range the slider is seeded
            // with and the figure the popup later compares against cannot come from two different
            // reads. Zero for TP and SL, which have no coin, and zero when the exchange has stated
            // no maximum for this market.
            //
            // Through the SAME resolver the row and the popover use, so all three agree by
            // construction rather than by three matching copies of one match.
            let lev_coin_max = match metric {
                controls::TradeMetric::Lev => self
                    .limits_for(Some(&target), cx)
                    .0
                    .map(|limits| limits.max_leverage)
                    .unwrap_or(0),
                _ => 0,
            };
            // Recorded BEFORE the seed and before any edit goes out, so both address the same place
            // — see [`Self::reconcile_metric_popup`] and [`Self::commit_metric_edit`].
            self.open_metric_popup = Some(controls::OpenMetricPopup {
                metric,
                target,
                lev_coin_max,
            });
            // Clicking TP hands control back to the main take profit (extinguishing the active S
            // slot) without changing the TP value itself.
            if metric == controls::TradeMetric::Tp {
                self.commit_metric_edit(metric, ClientSettingsEdit::EngageMainTakeProfit, cx);
            }
            self.seed_metric_popup(metric, window, cx);
        } else if self
            .open_metric_popup
            .as_ref()
            .is_some_and(|open| open.metric == metric)
        {
            self.open_metric_popup = None;
        }
        cx.notify();
    }

    /// Build the fine TP slider over `0..=TP_FINE_CAP` in `0.01` steps.
    /// Changes send the scalp TP edit and update the numeric field live; popup rendering controls
    /// whether the slider is enabled.
    pub(super) fn make_tp_fine_slider(cx: &mut Context<Self>) -> Entity<MoonSliderState> {
        let s = cx.new(|_| {
            MoonSliderState::new()
                .min(0.0)
                .max(controls::TP_FINE_CAP)
                .step(0.01)
                .default_value(0.0)
        });
        cx.subscribe(&s, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_metric_edit(
                    controls::TradeMetric::Tp,
                    ClientSettingsEdit::ScalpTakeProfit(v as f64),
                    cx,
                );
                this.live_set_field(this.tp_input.clone(), controls::fmt_field2(v), cx);
            }
        })
        .detach();
        s
    }

    /// Seed a metric popup from the group exit or the active core's Main-chart leverage.
    /// TP selects its normal or extended slider from the group's exact take-profit mode.
    fn seed_metric_popup(
        &self,
        metric: controls::TradeMetric,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use controls::TradeMetric;
        // Read the value first so entity updates do not retain a backend borrow.
        let val = metric.current(self.backend.read(cx), &self.group);
        let Some(val) = val else { return };
        match metric {
            TradeMetric::Tp => {
                let extended = self.active_tp_extended(cx);
                let slider = if extended {
                    &self.tp_slider_ext
                } else {
                    &self.tp_slider_normal
                };
                slider.update(cx, |st, c| st.set_value(val, window, c));
                self.tp_input.update(cx, |st, c| {
                    st.set_value(controls::fmt_field2(val), window, c)
                });
                // Clamp the current TP into the lower fine slider's `0..=TP_FINE_CAP` range.
                let fine = val.clamp(0.0, controls::TP_FINE_CAP);
                self.tp_fine_slider
                    .update(cx, |st, c| st.set_value(fine, window, c));
            }
            TradeMetric::Sl => {
                self.sl_slider
                    .update(cx, |st, c| st.set_value(val, window, c));
                self.sl_input.update(cx, |st, c| {
                    st.set_value(controls::fmt_field2_signed(val), window, c)
                });
            }
            TradeMetric::Lev => {
                // The slider stops at THIS COIN's maximum, not at the terminal's 1..125 fallback.
                // Taken from the maximum recorded when the popup opened, so the range the slider
                // gets and the figure the popup later compares against come from one read.
                let coin_max = self
                    .open_metric_popup
                    .as_ref()
                    .map(|open| open.lev_coin_max)
                    .unwrap_or(0);
                let (lo, hi, step) = controls::lev_bounds_for(coin_max);
                self.lev_slider.update(cx, |st, c| {
                    // Replace the state's VALUE in place; never `cx.new(..)` a fresh entity here.
                    // `Shell::init` registers a subscription on THIS entity that mirrors drags into
                    // the leverage field, and a replacement entity drops it silently — the slider
                    // would keep moving while the field, which is what Apply actually sends, froze.
                    *st = MoonSliderState::new().min(lo).max(hi).step(step);
                    st.set_value(val, window, c);
                });
                self.lev_input.update(cx, |st, c| {
                    st.set_value(format!("{}", val as i32), window, c)
                });
            }
        }
    }

    /// Return the extended-TP mode for edits entered in the TP field, read through the SAME
    /// source those edits will write to.
    ///
    /// Not the plain group-local getter: a `TakeProfit` edit carries this mode alongside its pct
    /// (see [`Self::seed_metric_popup`] and the TP field's blur/Enter handler in `shell::init`),
    /// and if the write targets a core opted into the per-core route, reading the group's mode
    /// here would carry the wrong mode into that core alongside the edit (goal A2 FIX-2).
    pub(super) fn active_tp_extended(&self, cx: &App) -> bool {
        let b = self.backend.read(cx);
        b.write_aligned_group_exit(&self.group).take_profit_mode
            == moon_core::config::TakeProfitMode::Extended
    }

    /// Update a popup field from its slider to provide live numeric feedback while dragging.
    /// Defers through the window handle because `MoonInputState::set_value` requires `&mut Window`.
    pub(super) fn live_set_field(
        &self,
        input: Entity<MoonInputState>,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                input.update(app, |st, c| st.set_value(text, window, c));
            });
        });
    }

    /// Set a slider programmatically through a deferred window-handle update.
    pub(super) fn defer_set_slider(
        &self,
        slider: Entity<MoonSliderState>,
        val: f32,
        cx: &mut Context<Self>,
    ) {
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                slider.update(app, |st, c| st.set_value(val, window, c));
            });
        });
    }
}
