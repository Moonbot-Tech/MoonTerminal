//! Хостинг попапа «настройки ядра» (кнопка ⚙ рядом с селектором ядра в шапке): открытие/
//! закрытие, сид числовых полей значением активного ядра, overlay+dismiss-слои и стадия
//! подтверждения «Отменить все ордера». Контент — `crate::core_settings_popup`.

use gpui::*;

use moon_ui::MoonPalette;

use crate::core_settings_popup;

use super::Shell;

impl Shell {
    /// Смена открытости попапа настроек ядра (`MoonPopover.on_open_change` у кнопки ⚙).
    /// При открытии сидирует числовые поля (глоб-TP / трейлинг) значением активного ядра
    /// и сбрасывает стадию подтверждения.
    pub(crate) fn set_core_settings_open(
        &mut self,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.core_settings_open == open {
            return;
        }
        self.core_settings_open = open;
        self.core_settings_cancel_confirm = false;
        if open {
            self.core_settings_bl_expanded = false;
            self.seed_core_settings_popup(window, cx);
        }
        cx.notify();
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
        // ВАЖНО: трейлинг/V-Stop НЕ имеют отдельного флага вкл/выкл на проводе (выкл = значение 0,
        // ядро «прежнее» число не хранит — как Delphi Moonbot, который помнит его только в СВОЁМ
        // UI: `TrailingDropOld`). Если на ядре 0 (выключено) — НЕ перетираем слайдер/поле нулём,
        // а оставляем последнее показанное значение. Пустое поле при выключенном стопе — честное
        // состояние: на ядре значения нет (НЕ подставляем дефолт, чтобы не врать о состоянии ядра).
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
            .update(cx, |st, c| st.set_value(bl_text.clone(), window, c));
        self.blacklist_area
            .update(cx, |st, c| st.set_value(bl_text, window, c));
    }

    /// Закоммитить текст чёрного списка монет активному ядру (флаг вкл — текущий у ядра).
    /// Общая точка для подписок Blur/Enter обоих полей (строка + textarea) и тогла «…».
    pub(super) fn commit_blacklist_text(&self, text: String, cx: &Context<Self>) {
        let b = self.backend.read(cx);
        let Some(core) = b.active_trade_core(&self.group) else {
            return;
        };
        let on = b
            .session
            .store()
            .core(core)
            .and_then(|d| d.client_settings.as_ref())
            .map(|s| s.use_blacklist)
            .unwrap_or(false);
        if let Err(error) = b.session.set_blacklist(core, on, text) {
            log::warn!("set blacklist text failed: {error:#}");
        }
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

    /// Контент попапа настроек ядра для `MoonPopover` у кнопки ⚙ (позиционируется к кнопке
    /// самим popover'ом — прежние захардкоженные координаты absolute-оверлея уехали, когда
    /// в шапку добавился тикер). Строится только при открытом попапе.
    pub(super) fn core_settings_popup_content(
        &self,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = cx.entity();
        let toggle_view = view.clone();
        let content = core_settings_popup::core_settings_content(
            &self.gtp_slider,
            &self.trailing_slider,
            &self.vstop_slider,
            &self.gtp_input,
            &self.trailing_input,
            &self.vstop_input,
            &self.blacklist_input,
            &self.blacklist_area,
            &self.def_strategy_input,
            self.core_settings_bl_expanded,
            self.core_settings_cancel_confirm,
            &self.backend,
            &self.group,
            p,
            cx,
            move |app| view.update(app, |this, cx| this.core_settings_cancel_all_click(cx)),
            move |window, app| {
                toggle_view.update(app, |this, cx| {
                    let expanding = !this.core_settings_bl_expanded;
                    this.core_settings_bl_expanded = expanding;
                    // Текст синкается между однострочным полем и multi-line редактором:
                    // стейты РАЗНЫЕ намеренно (textarea необратимо портит single-line стейт).
                    if expanding {
                        let text = this.blacklist_input.read(cx).value().to_string();
                        this.blacklist_area
                            .update(cx, |st, c| st.set_value(text, window, c));
                    } else {
                        let text = this.blacklist_area.read(cx).value().to_string();
                        this.blacklist_input
                            .update(cx, |st, c| st.set_value(text.clone(), window, c));
                        // Сворачивание = завершение правки: коммитим (Blur textarea при
                        // подмене элемента может не прийти).
                        this.commit_blacklist_text(text, cx);
                    }
                    cx.notify();
                })
            },
        );
        content
    }
}
