//! Хостинг попапа «настройки ядра» (кнопка ⚙ рядом с селектором ядра в шапке): открытие/
//! закрытие, сид числовых полей значением активного ядра, overlay+dismiss-слои и стадия
//! подтверждения «Отменить все ордера». Контент — `crate::core_settings_popup`.

use gpui::*;

use moon_ui::MoonPalette;

use crate::{core_settings_popup, design};

use super::Shell;

impl Shell {
    /// Открыть/закрыть попап настроек ядра (клик по ⚙). При открытии сидирует числовые поля
    /// (глоб-TP / трейлинг) значением активного ядра и сбрасывает стадию подтверждения.
    pub(crate) fn toggle_core_settings_popup(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.core_settings_open {
            self.core_settings_open = false;
        } else {
            self.core_settings_open = true;
            self.core_settings_hovered = false;
            self.core_settings_cancel_confirm = false;
            self.seed_core_settings_popup(window, cx);
        }
        cx.notify();
    }

    pub(super) fn close_core_settings_popup(&mut self, cx: &mut Context<Self>) {
        if self.core_settings_open {
            self.core_settings_open = false;
            self.core_settings_cancel_confirm = false;
            cx.notify();
        }
    }

    /// Засеять слайдеры+поля паники (price_drop_level) / глоб-TP / трейлинга значениями ядра.
    /// Слайдеры клампим в их диапазоны (трейлинг 0 = выкл → клампится к минимуму магнитуды).
    fn seed_core_settings_popup(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (gtp, trailing, vstop, bl_text) = {
            let b = self.backend.read(cx);
            let cs = b
                .active_trade_core(&self.group)
                .and_then(|c| b.session.store().core(c))
                .and_then(|d| d.client_settings.as_ref());
            match cs {
                Some(s) => (
                    s.global_take_profit_pct as f32,
                    s.trailing_drop_pct,
                    s.vol_drop_level,
                    s.blacklist_text.clone(),
                ),
                None => return,
            }
        };
        let clamp = |v: f32, (lo, hi, _): (f32, f32, f32)| v.clamp(lo, hi);
        // ВАЖНО: трейлинг/V-Stop НЕ имеют отдельного флага вкл/выкл на проводе (выкл = значение 0).
        // Если на ядре значение 0 (выключено) — НЕ перетираем слайдер/поле нулём, а оставляем
        // последнее показанное значение (как MoonBot: галка снята, но число видно/помнится).
        // Так пользователь видит и может вернуть прежнее значение. Сидируем только при ненулевом.
        self.gtp_slider.update(cx, |st, c| {
            st.set_value(clamp(gtp, core_settings_popup::CORE_GTP_BOUNDS), window, c)
        });
        self.gtp_input
            .update(cx, |st, c| st.set_value(format!("{gtp:.1}"), window, c));
        if trailing.abs() > 1e-6 {
            self.trailing_slider.update(cx, |st, c| {
                st.set_value(
                    clamp(trailing, core_settings_popup::CORE_TRAILING_BOUNDS),
                    window,
                    c,
                )
            });
            self.trailing_input.update(cx, |st, c| {
                st.set_value(format!("{trailing:.2}"), window, c)
            });
        }
        if vstop != 0 {
            self.vstop_slider.update(cx, |st, c| {
                st.set_value(
                    clamp(vstop as f32, core_settings_popup::CORE_VSTOP_BOUNDS),
                    window,
                    c,
                )
            });
            self.vstop_input
                .update(cx, |st, c| st.set_value(format!("{vstop}"), window, c));
        }
        self.blacklist_input
            .update(cx, |st, c| st.set_value(bl_text, window, c));
    }

    /// Клик по «Отменить все ордера»: первый клик — подтверждение, второй — реальная отмена.
    pub(super) fn core_settings_cancel_all_click(&mut self, cx: &mut Context<Self>) {
        if !self.core_settings_cancel_confirm {
            self.core_settings_cancel_confirm = true;
            cx.notify();
            return;
        }
        self.core_settings_cancel_confirm = false;
        let b = self.backend.read(cx);
        if let Some(core) = b.active_trade_core(&self.group) {
            if let Err(error) = b.session.cancel_all_orders(core) {
                log::warn!("cancel all orders failed: {error:#}");
            }
        }
        cx.notify();
    }

    /// Левый/верхний отступ overlay-попапа настроек ядра: под кнопкой ⚙ (после бренда и
    /// селектора в шапке). Координаты приблизительные (как у `metric_popup_pos`) — при
    /// необходимости подогнать. Top = высота шапки.
    fn core_settings_popup_pos(&self, cx: &App) -> (Pixels, Pixels) {
        let left = f32::from(design::ui_px(cx, design::titlebar_leading_inset()))
            + f32::from(design::ui_px(cx, 8.0));
        let header_h = f32::from(design::fit_h_px(cx, design::HEADER_TOP_H, 14.0, 9.0));
        (px(left), px(header_h))
    }

    /// Слой попапа настроек ядра: сам попап (absolute, под ⚙) + полноэкранный dismiss-слой.
    /// Возвращает `(попап, dismiss)` — оба `None`, если попап закрыт. Зеркало `metric_popup_layers`.
    pub(super) fn core_settings_popup_layers(
        &self,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        if !self.core_settings_open {
            return (None, None);
        }
        let view = cx.entity();
        let content = core_settings_popup::core_settings_content(
            &self.gtp_slider,
            &self.trailing_slider,
            &self.vstop_slider,
            &self.gtp_input,
            &self.trailing_input,
            &self.vstop_input,
            &self.blacklist_input,
            self.core_settings_cancel_confirm,
            &self.backend,
            &self.group,
            p,
            cx,
            move |app| view.update(app, |this, cx| this.core_settings_cancel_all_click(cx)),
        );
        let (left, top) = self.core_settings_popup_pos(cx);
        let overlay = div()
            .id("core-settings-popup-box")
            .absolute()
            .left(left)
            .top(top)
            // Клик/драг внутри не закрывает (иначе нельзя возиться с полями).
            .on_mouse_down(MouseButton::Left, |_, _w, app| app.stop_propagation())
            // Авто-выход по уводу мыши, но не во время drag (как у метрик-попапа).
            .on_hover(cx.listener(|this, hovered: &bool, _w, cx| {
                if *hovered {
                    this.core_settings_hovered = true;
                } else if this.core_settings_hovered && !cx.has_active_drag() {
                    this.close_core_settings_popup(cx);
                }
            }))
            .child(content)
            .into_any_element();
        let dismiss = div()
            .id("core-settings-popup-dismiss")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev, _w, cx| {
                    this.close_core_settings_popup(cx);
                    cx.stop_propagation();
                }),
            )
            .into_any_element();
        (Some(overlay), Some(dismiss))
    }
}
