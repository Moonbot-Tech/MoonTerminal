//! Хостинг попапа выбора источника тикера курса в шапке (элемент «1 BTC = …» слева после
//! логотипа): открытие/закрытие, поле поиска монеты и список «BTC - Bybit1» (реюз
//! [`crate::chart_tabs::coin_search`]). Выбор строки пишется в Backend
//! (`set_header_ticker` → layout, по стабильному uid ядра). Overlay+dismiss — тот же
//! механизм, что попапы метрик/настроек ядра.

use gpui::*;

use moon_ui::{MoonInput, MoonPalette, v_flex};
use rust_i18n::t;

use crate::chart_tabs::coin_search;
use crate::design;

use super::Shell;

impl Shell {
    /// Открыть/закрыть попап выбора тикера (клик по курсу в шапке). При открытии чистим
    /// прежний запрос поиска.
    pub(crate) fn toggle_ticker_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ticker_popup_open {
            self.ticker_popup_open = false;
        } else {
            self.ticker_popup_open = true;
            self.ticker_popup_hovered = false;
            self.ticker_input
                .update(cx, |st, c| st.set_value(String::new(), window, c));
        }
        cx.notify();
    }

    pub(super) fn close_ticker_popup(&mut self, cx: &mut Context<Self>) {
        if self.ticker_popup_open {
            self.ticker_popup_open = false;
            cx.notify();
        }
    }

    /// Слой попапа тикера: absolute-бокс под элементом курса + полноэкранный dismiss-слой.
    /// Возвращает `(попап, dismiss)` — оба `None`, если попап закрыт.
    pub(super) fn ticker_popup_layers(
        &self,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        if !self.ticker_popup_open {
            return (None, None);
        }
        let query = self.ticker_input.read(cx).value().to_string();
        let results = {
            let b = self.backend.read(cx);
            coin_search::search(&b, &self.group, None, &query)
        };
        let backend = self.backend.clone();
        let view = cx.entity();
        let list = coin_search::render_popup(
            "header-ticker-search",
            results,
            &Default::default(),
            false,
            p,
            cx,
            move |core, market, _window, app| {
                backend.update(app, |b, bcx| {
                    b.set_header_ticker(core, market);
                    bcx.notify();
                });
                view.update(app, |this, cx| this.close_ticker_popup(cx));
            },
            |_, _, _| {},
            |_| {},
        );

        // Позиция: под шапкой, слева после бренда (как сам элемент курса).
        let left = f32::from(design::ui_px(cx, design::titlebar_leading_inset()))
            + f32::from(design::ui_px(cx, 8.0));
        let top = f32::from(design::fit_h_px(cx, design::HEADER_TOP_H, 14.0, 9.0));
        let overlay = div()
            .id("header-ticker-popup-box")
            .absolute()
            .left(px(left))
            .top(px(top))
            .on_mouse_down(MouseButton::Left, |_, _w, app| app.stop_propagation())
            .on_hover(cx.listener(|this, hovered: &bool, _w, cx| {
                if *hovered {
                    this.ticker_popup_hovered = true;
                } else if this.ticker_popup_hovered && !cx.has_active_drag() {
                    this.close_ticker_popup(cx);
                }
            }))
            .child(
                // Ширина = список `render_popup` (фикс. 240) + свои паддинги (2×6, масштабируются
                // слайдером шрифта) + рамка (2×1): иначе внутренняя рамка списка вылазит за
                // границы попапа.
                v_flex()
                    .w(px(240.0 + 2.0 * f32::from(design::ui_px(cx, 6.0)) + 2.0))
                    .gap(design::ui_px(cx, 4.0))
                    .p(design::ui_px(cx, 6.0))
                    .bg(rgb(p.panel_high))
                    .border_1()
                    .border_color(rgb(p.border))
                    .rounded(px(4.0))
                    .child(
                        div()
                            .text_size(design::t_caption(cx))
                            .text_color(rgb(p.text_muted))
                            .child(t!("header.ticker_pick").to_string()),
                    )
                    .child(
                        MoonInput::new("header-ticker-query")
                            .state(&self.ticker_input)
                            .small(),
                    )
                    .child(list),
            )
            .into_any_element();
        let dismiss = div()
            .id("header-ticker-popup-dismiss")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev, _w, cx| {
                    this.close_ticker_popup(cx);
                    cx.stop_propagation();
                }),
            )
            .into_any_element();
        (Some(overlay), Some(dismiss))
    }
}
