//! Попапы торговых метрик тулбара (TP/SL/Lev): открытие/закрытие, засев слайдера+поля
//! значением активного ядра и коммит правок в ядро. Вынесено из `shell.rs`.
//! Методы, дёргаемые из `mod.rs` (`new`/`render`), помечены `pub(super)` — приватный
//! `fn` в `impl Shell` этого подмодуля иначе не виден родителю `shell`.

use gpui::*;

use moon_ui::{MoonInputState, MoonSliderEvent, MoonSliderState};

use moon_core::feed::{ClientSettingsEdit, LevManageEdit};

use crate::{controls, design};

use super::Shell;

impl Shell {
    /// Открыть/закрыть попап метрики тулбара (клик по кнопке TP/SL/Lev). При открытии сидирует
    /// слайдер/поле текущим значением активного ядра (Context есть — без backend-замыканий).
    pub(crate) fn toggle_metric_popup(
        &mut self,
        metric: controls::TradeMetric,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open_metric_popup == Some(metric) {
            self.open_metric_popup = None;
        } else {
            self.open_metric_popup = Some(metric);
            self.metric_popup_hovered = false;
            self.seed_metric_popup(metric, window, cx);
        }
        cx.notify();
    }

    /// Создать файн-слайдер TP (0..2, шаг 0.01) с подпиской: на изменение шлёт суб-процентный
    /// TP через scalp и живо обновляет поле. Активность (disabled) — на стороне рендера попапа.
    pub(super) fn make_tp_fine_slider(cx: &mut Context<Self>) -> Entity<MoonSliderState> {
        let s = cx.new(|_| {
            MoonSliderState::new()
                .min(0.0)
                .max(controls::TP_FINE_MAX)
                .step(0.01)
                .default_value(0.0)
        });
        cx.subscribe(&s, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_client_edit(ClientSettingsEdit::ScalpTakeProfit(v as f64), cx);
                this.live_set_field(this.tp_input.clone(), controls::fmt_field2(v), cx);
            }
        })
        .detach();
        s
    }

    /// Левый/верхний отступ overlay-попапа метрики: под её кнопкой в тулбаре. Ширины метрик
    /// фиксированы (TP/SL=74.6, Lev=61.6); top = высота шапки + тулбара (те же fit-формулы).
    pub(super) fn metric_popup_pos(
        &self,
        metric: controls::TradeMetric,
        cx: &App,
    ) -> (Pixels, Pixels) {
        use controls::TradeMetric;
        let pad = f32::from(design::ui_px(cx, 12.0));
        let gap = f32::from(design::ui_px(cx, 6.0));
        let left = pad
            + match metric {
                TradeMetric::Tp => 0.0,
                TradeMetric::Sl => 74.6 + gap,
                TradeMetric::Lev => 74.6 + gap + 74.6 + gap,
            };
        let header_h = f32::from(design::fit_h_px(cx, design::HEADER_TOP_H, 14.0, 9.0));
        let toolbar_h = f32::from(design::fit_h_px(cx, controls::TOOLBAR_H, 13.0, 9.5));
        (px(left), px(header_h + toolbar_h))
    }

    pub(super) fn close_metric_popup(&mut self, cx: &mut Context<Self>) {
        if self.open_metric_popup.take().is_some() {
            cx.notify();
        }
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
                // Нижний (файн) слайдер 0..2: ставим на текущий TP в этом диапазоне.
                let fine = val.clamp(0.0, controls::TP_FINE_MAX);
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

    /// Отправить правку управления плечом активному ядру.
    pub(super) fn commit_lev_edit(&self, edit: LevManageEdit, cx: &mut Context<Self>) {
        let b = self.backend.read(cx);
        let Some(core) = b.active_trade_core(&self.group) else {
            return;
        };
        if let Err(error) = b.session.edit_lev_manage(core, edit) {
            log::warn!("toolbar lev manage edit failed: {error:#}");
        }
    }
}
