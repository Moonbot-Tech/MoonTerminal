//! `impl Render for DetachedChartHost`: шапка окна (поиск монеты + масштаб + попап раскладки ⚙ +
//! «закрыть все графики») над панелью чарт-стека. Оверлей попапа ⚙ и обвязка поиска монеты —
//! общие с полоской вкладок ([`super::super::common`]).

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonPalette, MoonWindowFrame,
    MoonWindowFrameControls, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::candle_popup::{self, CandlePopupHost};
use super::super::common::{self, LayoutPopupHost};
use super::super::{chart_pane_label, coin_search};
use super::DetachedChartHost;
use crate::design;

impl Render for DetachedChartHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Коррекция размера восстановленного окна (один раз): окно уже на целевом мониторе с
        // верным scale → форсим сохранённый логический размер, перебивая DPICHANGED-сжатие.
        if let Some(sz) = self.restore_size.take() {
            window.resize(sz);
        }
        // Убрать кнопку из таскбара (DeleteTab), оставив окно independent → FancyZones его видит.
        // Несколько первых рендеров — на случай, если кнопка появляется чуть позже показа окна.
        if self.taskbar_hide_ticks > 0 {
            crate::windowing::hide_window_from_taskbar(window);
            self.taskbar_hide_ticks -= 1;
        }
        // [Shift+СКМ] на графике ЭТОГО окна → X-масштаб на панель окна + persist в спек.
        {
            let (rev, req) = {
                let b = self.backend.read(cx);
                (b.chart_x_sync_rev, b.chart_x_sync)
            };
            if rev != self.last_x_sync_rev {
                self.last_x_sync_rev = rev;
                if let Some((handle, ppm)) = req {
                    if handle == window.window_handle() {
                        self.panel.update(cx, |s, c| s.set_x_ppm(Some(ppm), true, c));
                        let backend = self.backend.clone();
                        let (num, bucket) = (self.num, self.bucket.clone());
                        common::upsert_spec(&backend, &self.group.clone(), num, &bucket, cx, move |s| {
                            s.x_ppm = Some(ppm)
                        });
                    }
                }
            }
        }
        let p = MoonPalette::active(cx);
        // Масштаб — СВОЙ у этой панели (по-вкладочно), правится прямо в неё.
        let scale = self.panel.read(cx).scale();
        let panel = self.panel.clone();
        let close_all_panel = self.panel.clone();
        let title = chart_pane_label(&self.backend, &self.group, self.num, &self.bucket, cx);
        let frame = MoonWindowFrame::detached_chart("detached-chart-window-frame", 0.0)
            .header_height(34.0)
            .controls(MoonWindowFrameControls::Close)
            .show_controls(design::show_custom_window_controls());
        let popup_open = self.layout_popup_open;
        let layout_popup = common::layout_popup_overlay(
            self,
            "detached-chart-layout",
            px(38.0),
            t!("chart.layout.apply_all_charts").to_string(),
            cx,
        );
        let layout_dismiss = common::layout_popup_dismiss(self, "detached-chart-layout", cx);
        // Попап «Свечи и трейды» (глобальный) — кнопка ❚ рядом с ⚙, якорь под шапкой.
        let candle_popup_open = self.candle_popup_open;
        let candle_popup =
            candle_popup::candle_popup_overlay(self, "detached-chart-candles", px(38.0), cx);
        let candle_dismiss = candle_popup::candle_popup_dismiss(self, "detached-chart-candles", cx);
        // Поле ввода монеты (поиск) шапки + список совпадений. Список рисуем на уровне v_flex
        // (после тела), иначе тело окна (paint-порядок ниже) перекроет выпадашку из шапки.
        let coin_search_el = div().w(design::font_w_px(cx, 80.0)).child(
            MoonInput::new("detached-coin-search")
                .state(&self.coin_input)
                .cleanable(true)
                .small(),
        );
        let coin_popup = self.coin_popup_open.then(|| {
            let results = self.coin_results(cx);
            coin_search::render_popup(
                "detached-coin",
                results,
                &std::collections::HashSet::new(),
                false,
                p,
                cx,
                common::coin_pick_handler(cx, self.coin_input.clone()),
                |_core, _market, _app| {},
                |_app| {},
            )
            .absolute()
            .right(px(6.0))
            .top(px(38.0))
        });
        // Перехватчик клика вне списка — только ниже шапки (top 34), чтобы не блокировать само поле.
        let coin_dismiss = self.coin_popup_open.then(|| {
            div()
                .id("detached-coin-dismiss")
                .absolute()
                .top(px(34.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .on_mouse_down(MouseButton::Left, common::coin_dismiss_handler(cx))
        });
        // Шапка — ТОЛЬКО у выносных окон вкладок (в основном доке её нет): масштаб слева,
        // «закрыть все графики» справа.
        v_flex()
            .size_full()
            .relative()
            // Фокусируемый корень + ловля хоткеев окна (единый диспетчер).
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|this, ev: &KeyDownEvent, _window, cx| this.on_hotkey(ev, cx)),
            )
            .child(
                h_flex()
                    .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .pl(design::ui_px(cx, design::titlebar_leading_inset()))
                    .pr(design::ui_px(cx, 6.0))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.shell_high))
                    .child(
                        frame
                            .title_cluster(title, cx)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .items_center(),
                    )
                    .child(coin_search_el)
                    .child(crate::controls::scale_dropdown_for_add_stack(
                        cx,
                        scale,
                        panel.clone(),
                        p,
                    ))
                    .child({
                        let entity = cx.entity();
                        MoonButton::new("detached-candle-settings")
                            .label("❚")
                            .tooltip(t!("chart.candles.tip").to_string())
                            .size(MoonButtonSize::Micro)
                            .variant(if candle_popup_open {
                                MoonButtonVariant::Blue
                            } else {
                                MoonButtonVariant::Ghost
                            })
                            .selected(candle_popup_open)
                            .on_click(move |_, _window, app| {
                                entity.update(app, |this, cx| this.toggle_candle_popup(cx));
                            })
                            .render()
                    })
                    .child({
                        let entity = cx.entity();
                        div().relative().child(
                            MoonButton::new("detached-layout-settings")
                                .label("⚙")
                                .tooltip(t!("chart.layout.tip").to_string())
                                .size(MoonButtonSize::Micro)
                                .variant(if popup_open {
                                    MoonButtonVariant::Blue
                                } else {
                                    MoonButtonVariant::Ghost
                                })
                                .selected(popup_open)
                                .on_click(move |_, window, app| {
                                    entity.update(app, |this, cx| {
                                        this.toggle_layout_popup(window, cx)
                                    });
                                })
                                .render(),
                        )
                    })
                    .child(
                        MoonButton::new("detached-close-all")
                            .label("🗑")
                            .tooltip(t!("chartwin.clear").to_string())
                            .size(MoonButtonSize::Micro)
                            .variant(MoonButtonVariant::Ghost)
                            .on_click(move |_, _w, app| {
                                close_all_panel.update(app, |p, cx| p.close_all_panes(cx));
                            })
                            .render(),
                    )
                    .when(design::show_custom_window_controls(), |this| {
                        this.child(frame.visual_controls(cx))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    // БЕЗ .bg(): own-pass чарта и его text layer лежат under-scene, любой
                    // непрозрачный фон тела перекроет график. Подложку под/между чартами закрывает
                    // тёмный clear окна (правка форка MoonUI), белого нет.
                    .child(self.panel.clone()),
            )
            .children(coin_dismiss)
            .children(coin_popup)
            .children(layout_dismiss)
            .children(layout_popup)
            .children(candle_dismiss)
            .children(candle_popup)
    }
}
