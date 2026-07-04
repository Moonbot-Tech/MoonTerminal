//! `impl Render for DetachedChartHost`: шапка окна (поиск монеты + масштаб + попап раскладки ⚙ +
//! «закрыть все графики») над панелью чарт-стека. Вынесено вербатим из `mod.rs`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonPalette, MoonWindowFrame,
    MoonWindowFrameControls, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::{chart_pane_label, coin_search, layout_popup};
use super::DetachedChartHost;
use crate::chart_persist::StackLayoutMode;
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
        let layout_popup = self.layout_popup_open.then(|| {
            let mode = self.panel_layout(cx).0.unwrap_or(StackLayoutMode::Fit);
            let orientation = self
                .panel
                .read(cx)
                .layout_orientation()
                .unwrap_or(crate::chart_persist::StackOrientation::Vertical);
            let orderbook_enabled = self.panel.read(cx).orderbook_enabled().unwrap_or(true);
            let liquidations_enabled = self.panel.read(cx).liquidations_enabled().unwrap_or(true);
            let show_zone = self.panel.read(cx).show_zone().unwrap_or(true);
            let auto_pin = self.panel.read(cx).auto_pin().unwrap_or(false);
            let (cancel_pos, panic_pos) = {
                let (c, pp) = self.panel.read(cx).action_btn_pos();
                (c.unwrap_or_default(), pp.unwrap_or_default())
            };
            let price_axis_pos = self.panel.read(cx).price_axis_pos().unwrap_or_default();
            let time_axis_visible = self.panel.read(cx).time_axis_visible().unwrap_or(true);
            let line_labels = self.panel.read(cx).line_labels().unwrap_or(true);
            let cursor_labels = self.panel.read(cx).cursor_labels().unwrap_or(true);
            let is_custom = self.is_custom(cx);
            let pick_entity = cx.entity();
            let all_entity = cx.entity();
            let ob_entity = cx.entity();
            let liq_entity = cx.entity();
            let sz_entity = cx.entity();
            let ap_entity = cx.entity();
            let or_entity = cx.entity();
            let cbp_entity = cx.entity();
            let psp_entity = cx.entity();
            let pap_entity = cx.entity();
            let tav_entity = cx.entity();
            let ll_entity = cx.entity();
            let cl_entity = cx.entity();
            let hover_entity = cx.entity();
            let popup_w = layout_popup::content_width(cx, is_custom);
            div()
                .id("detached-chart-layout-popup-scene")
                .absolute()
                .right(px(6.0))
                .top(px(38.0))
                .w(popup_w)
                .on_mouse_down(MouseButton::Left, |_, _window, app| {
                    app.stop_propagation();
                })
                .on_hover(move |hovered, _window, app| {
                    hover_entity.update(app, |this, cx| {
                        if *hovered {
                            this.layout_popup_hovered = true;
                        } else if this.layout_popup_hovered {
                            this.close_layout_popup(true, cx);
                        }
                    });
                })
                .child(layout_popup::render_layout_popup(
                    "detached-chart-layout",
                    mode,
                    orientation,
                    is_custom.then_some(&self.custom_name_input),
                    &self.layout_fit_input,
                    &self.layout_scroll_input,
                    orderbook_enabled,
                    liquidations_enabled,
                    show_zone,
                    auto_pin,
                    cancel_pos,
                    panic_pos,
                    price_axis_pos,
                    time_axis_visible,
                    line_labels,
                    cursor_labels,
                    p,
                    cx,
                    move |mode, app| {
                        pick_entity.update(app, |this, cx| {
                            let hf = this.read_layout_height(StackLayoutMode::Fit, cx);
                            let hs = this.read_layout_height(StackLayoutMode::Scroll, cx);
                            this.apply_layout(Some(mode), hf, hs, cx);
                        });
                    },
                    t!("chart.layout.apply_all_charts").to_string(),
                    move |app| {
                        all_entity.update(app, |this, cx| {
                            let (mode, _, _) = this.panel_layout(cx);
                            let hf = this.read_layout_height(StackLayoutMode::Fit, cx);
                            let hs = this.read_layout_height(StackLayoutMode::Scroll, cx);
                            this.apply_layout_to_all_charts(
                                Some(mode.unwrap_or(StackLayoutMode::Fit)),
                                hf,
                                hs,
                                cx,
                            );
                        });
                    },
                    move |checked, app| {
                        ob_entity.update(app, |this, cx| this.apply_orderbook(checked, cx));
                    },
                    move |checked, app| {
                        liq_entity.update(app, |this, cx| this.apply_liquidations(checked, cx));
                    },
                    move |checked, app| {
                        sz_entity.update(app, |this, cx| this.apply_show_zone(checked, cx));
                    },
                    move |checked, app| {
                        ap_entity.update(app, |this, cx| this.apply_auto_pin(checked, cx));
                    },
                    move |app| {
                        or_entity.update(app, |this, cx| {
                            use crate::chart_persist::StackOrientation as O;
                            let next = match this
                                .panel
                                .read(cx)
                                .layout_orientation()
                                .unwrap_or(O::Vertical)
                            {
                                O::Vertical => O::Horizontal,
                                O::Horizontal => O::Vertical,
                            };
                            this.apply_orientation(next, cx);
                        });
                    },
                    move |pos, app| {
                        cbp_entity.update(app, |this, cx| this.apply_cancel_pos(pos, cx));
                    },
                    move |pos, app| {
                        psp_entity.update(app, |this, cx| this.apply_panic_pos(pos, cx));
                    },
                    move |pos, app| {
                        pap_entity.update(app, |this, cx| this.apply_price_axis_pos(pos, cx));
                    },
                    move |checked, app| {
                        tav_entity
                            .update(app, |this, cx| this.apply_time_axis_visible(checked, cx));
                    },
                    move |checked, app| {
                        ll_entity.update(app, |this, cx| this.apply_line_labels(checked, cx));
                    },
                    move |checked, app| {
                        cl_entity.update(app, |this, cx| this.apply_cursor_labels(checked, cx));
                    },
                ))
        });
        let layout_dismiss = self.layout_popup_open.then(|| {
            let entity = cx.entity();
            div()
                .id("detached-chart-layout-popup-dismiss")
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, move |_, _window, app| {
                    entity.update(app, |this, cx| this.close_layout_popup(true, cx));
                    app.stop_propagation();
                })
        });
        // Поле ввода монеты (поиск) шапки + список совпадений. Список рисуем на уровне v_flex
        // (после тела), иначе тело окна (paint-порядок ниже) перекроет выпадашку из шапки.
        let coin_search_el = div().w(px(80.0)).child(
            MoonInput::new("detached-coin-search")
                .state(&self.coin_input)
                .cleanable(true)
                .small(),
        );
        let coin_popup = self.coin_popup_open.then(|| {
            let results = self.coin_results(cx);
            let view = cx.entity();
            let input = self.coin_input.clone();
            coin_search::render_popup(
                "detached-coin",
                results,
                &std::collections::HashSet::new(),
                false,
                p,
                cx,
                move |core, market, window, app| {
                    view.update(app, |this, cx| this.open_coin(core, market, cx));
                    input.update(app, |inp, c| {
                        inp.set_value(SharedString::default(), window, c)
                    });
                    view.update(app, |this, cx| this.clear_coin_search(cx));
                },
                |_core, _market, _app| {},
                |_app| {},
            )
            .absolute()
            .right(px(6.0))
            .top(px(38.0))
        });
        // Перехватчик клика вне списка — только ниже шапки (top 34), чтобы не блокировать само поле.
        let coin_dismiss = self.coin_popup_open.then(|| {
            let entity = cx.entity();
            div()
                .id("detached-coin-dismiss")
                .absolute()
                .top(px(34.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .on_mouse_down(MouseButton::Left, move |_, _w, app| {
                    entity.update(app, |this, cx| this.clear_coin_search(cx));
                    app.stop_propagation();
                })
        });
        // Шапка — ТОЛЬКО у выносных окон вкладок (в основном доке её нет): масштаб слева,
        // «закрыть все графики» справа.
        v_flex()
            .size_full()
            .relative()
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
                        scale,
                        panel.clone(),
                        p,
                    ))
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
    }
}
