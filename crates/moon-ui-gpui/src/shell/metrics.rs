//! Toolbar metric popups (TP/SL/Lev): open-state reconciliation, editor seeding, and guarded edits
//! bound to the core and optional market captured when the popup opened.

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
    ///   armed). `MoonPopover` then renders only a disabled trigger and fires no `on_open_change`,
    ///   so the popup vanishes while the state stays `Some` — and pops back open, unclicked, the
    ///   moment the metric is available again, holding values seeded before the change.
    /// * **the edit target changed underneath it.** A different active core moves every metric;
    ///   changing the Main chart market moves leverage alone. Event handlers resolve that address
    ///   when they fire, so availability alone cannot distinguish the stale popup from a live one.
    ///
    /// Lives beside the state it guards rather than in the frame-composition module, so the next
    /// rule added to [`controls::TradeMetric`] is edited within sight of it.
    pub(super) fn reconcile_metric_popup(&mut self, cx: &App) {
        let stale = self
            .open_metric_popup
            .as_ref()
            .is_some_and(|(metric, target)| {
                let b = self.backend.read(cx);
                !target.is_live(*metric, b, &self.group) || !metric.available(b, &self.group)
            });
        if stale {
            self.open_metric_popup = None;
        }
    }

    /// Send a `ClientSettings` edit ON BEHALF OF the open metric popup.
    ///
    /// Refuses when the popup is not open for `metric`, or when the address it was seeded from is
    /// no longer the one this metric resolves to. That check has to happen HERE, at event time:
    /// [`Self::reconcile_metric_popup`] takes a stale popup off the screen, but only at the next
    /// render, and repaints pass three stacked throttles. A slider drag inside that window would
    /// otherwise continue changing the no-longer-visible core or leverage market.
    ///
    /// Distinct from [`Self::commit_client_edit`], which the core-settings gear popup uses and which
    /// still resolves the active core at event time. That is a KNOWN GAP, not a design decision:
    /// `seed_core_settings_popup` freezes Global TP, TrailingDrop and V-Stop from the core active
    /// when the gear popup opened, so it has this same defect and wants this same treatment. It is
    /// left alone here only because its files are outside this change.
    pub(super) fn commit_metric_edit(
        &self,
        metric: controls::TradeMetric,
        edit: ClientSettingsEdit,
        cx: &mut Context<Self>,
    ) {
        let Some((open, target)) = self.open_metric_popup.as_ref() else {
            return;
        };
        let b = self.backend.read(cx);
        if *open != metric || !target.is_live(metric, b, &self.group) {
            return;
        }
        if let Err(error) = b.session.edit_client_settings(target.core, edit) {
            log::warn!("toolbar metric edit failed: {error:#}");
        }
    }

    /// Open or close a toolbar metric's popup — the `on_open_change` of its anchored `MoonPopover`.
    ///
    /// Opening seeds the slider and the field with the active core's current value.
    ///
    /// The `EngageMainTakeProfit` edit goes out ONLY on OPENING TP. `MoonPopover` fires
    /// `on_open_change` for every close — a click elsewhere, Escape, a second click on the trigger —
    /// and none of those is the user acting on TP. Sending a trading command from there is doubly
    /// unsafe: `edit_client_settings` transmits the WHOLE last accepted `ClientSettings` snapshot,
    /// so closing the popup right after editing TP — before the core echoed the new value back —
    /// would roll that edit back. Closing now only clears UI state.
    ///
    /// Opening is a no-op unless both the target and its seed value exist. Every edit needs a core,
    /// leverage also needs a Main-chart market, and a missing value would leave the long-lived
    /// editor showing stale data from the previous target.
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
            // `seed_metric_popup` gives up silently when the value is missing — no client settings
            // yet, or a market with no leverage entry — so an editor opened in that state would
            // display the previous core's or the previous coin's number as if it were this one's.
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
            // Recorded BEFORE the seed and before any edit goes out, so both address the same place
            // — see [`Self::reconcile_metric_popup`] and [`Self::commit_metric_edit`].
            self.open_metric_popup = Some((metric, target));
            // Clicking TP hands control back to the main take profit (extinguishing the active S
            // slot) without changing the TP value itself.
            if metric == controls::TradeMetric::Tp {
                self.commit_metric_edit(metric, ClientSettingsEdit::EngageMainTakeProfit, cx);
            }
            self.seed_metric_popup(metric, window, cx);
        } else if self
            .open_metric_popup
            .as_ref()
            .is_some_and(|(m, _)| *m == metric)
        {
            self.open_metric_popup = None;
        }
        cx.notify();
    }

    /// Создать файн-слайдер TP (0..2, шаг 0.01) с подпиской: на изменение шлёт суб-процентный
    /// TP через scalp и живо обновляет поле. Активность (disabled) — на стороне рендера попапа.
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

    /// Засеять слайдер+поле попапа значением активного ядра. Для TP выбирает обычный/
    /// расширенный слайдер по текущему `x_tmode`.
    fn seed_metric_popup(
        &self,
        metric: controls::TradeMetric,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use controls::TradeMetric;
        // Значение тянем заранее (отдельный read), чтобы не держать заём backend при update сущностей.
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
                // Нижний (файн) слайдер 0..1.99: ставим на текущий TP в этом диапазоне.
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
                self.lev_slider
                    .update(cx, |st, c| st.set_value(val, window, c));
                self.lev_input.update(cx, |st, c| {
                    st.set_value(format!("{}", val as i32), window, c)
                });
            }
        }
    }

    /// Текущий режим расширенного диапазона TP (`x_tmode`) активного ядра — для отправки
    /// правки TP из поля в нужный диапазон. Нет ядра/настроек → false (обычный 1..100%).
    pub(super) fn active_tp_extended(&self, cx: &App) -> bool {
        let b = self.backend.read(cx);
        b.active_trade_core(&self.group)
            .and_then(|c| b.session.store().core(c))
            .and_then(|d| d.client_settings.as_ref())
            .map(|s| s.take_profit_extended)
            .unwrap_or(false)
    }

    /// Живо обновить поле попапа значением слайдера (drag → numeric-фидбэк). Через
    /// `defer` + window-handle, т.к. `MoonInputState::set_value` требует `&mut Window`.
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

    /// Программно выставить значение слайдера (нужен `&mut Window` → через defer+window-handle).
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

    /// Отправить правку `ClientSettings` активному торговому ядру окна (из попапа тулбара).
    /// Нет активного ядра — no-op.
    pub(super) fn commit_client_edit(&self, edit: ClientSettingsEdit, cx: &mut Context<Self>) {
        let b = self.backend.read(cx);
        let Some(core) = b.active_trade_core(&self.group) else {
            return;
        };
        if let Err(error) = b.session.edit_client_settings(core, edit) {
            log::warn!("toolbar client settings edit failed: {error:#}");
        }
    }
}
