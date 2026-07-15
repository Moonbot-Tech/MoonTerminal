//! Рендер полоски чарт-вкладок (`impl Render for ChartTabs`): сам таб-стрип (Main + AddToChart),
//! кнопки «собрать окна» (▦) и настроек раскладки (⚙ + canvas-проба её rect), плюс активная панель
//! ниже. Логика вкладок/синхронизации — в [`super`] (mod.rs), выносные окна — в [`super::windows`].

use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonPalette, MoonRect, MoonTabItem,
    MoonTabStrip, h_flex, v_flex,
};
use rust_i18n::t;

use super::candle_popup::{self, CandlePopupHost};
use super::common::{self, LayoutPopupHost};
use super::{ChartTabs, Tab, chart_tab_strip_h, coin_search};
use crate::design;

impl Render for ChartTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Снимок вкладок — чтобы callbacks не держали borrow self.add. (Tab, label, count для
        // ширины, unread для бейджа, detachable.)
        let mut tabs: Vec<(Tab, String, usize, usize, bool)> =
            vec![(Tab::Main, "Main".to_string(), 0, 0, false)];
        tabs.extend(self.add.iter().map(|(n, bucket, panel)| {
            let count = panel.read(cx).pane_count(cx);
            let seen = self.seen.get(&(*n, bucket.clone())).copied().unwrap_or(0);
            (
                Tab::Add(*n, bucket.clone()),
                self.add_label(*n, bucket, cx),
                count,
                count.saturating_sub(seen),
                true,
            )
        }));
        // Кастомные (мульти-монетные) вкладки: своё имя, без бейджа, закрываемы (×). Дабл-клик
        // НЕ открепляет (guard в on_click), но closable=true → крестик есть.
        tabs.extend(self.custom.iter().map(|(n, bucket, _)| {
            (
                Tab::Custom(*n, bucket.clone()),
                self.custom_label(*n),
                0,
                0,
                true,
            )
        }));
        let tab_keys = Rc::new(
            tabs.iter()
                .map(|(tab, _, _, _, _)| tab.clone())
                .collect::<Vec<_>>(),
        );
        let items = tabs
            .iter()
            .map(|(tab, label, _count, unread, detachable)| {
                let width = (label.chars().count() as f32 * 7.0
                    + if *unread > 0 { 38.0 } else { 28.0 }
                    + if *detachable { 20.0 } else { 0.0 })
                .clamp(72.0, 168.0);
                let mut item = MoonTabItem::new(label.clone())
                    .width(width)
                    .selected(self.active == *tab)
                    .closable(*detachable);
                if *unread > 0 {
                    item = item.badge(unread.to_string());
                }
                item
            })
            .collect::<Vec<_>>();
        let view = cx.entity();
        // MoonTabStrip рисует ВСЕ табы абсолютно и режет по `overflow_hidden` ПО СВОИМ
        // bounds. Без явных bounds его root схлопывается в 0×0 → полоска невидима, а чарт
        // (flex_1 ниже) забирает всю высоту (ровно баг «график есть, вкладок нет»). Даём
        // ширину окна (контейнер ниже обрежет до ширины панели) и фикс. высоту полосы.
        let strip_w = f32::from(window.viewport_size().width).max(1.0);
        // Высота полосы = высоте таба (fit_height), чтобы при смене ui_scale/шрифта полоса и
        // линия под ней не отъезжали от табов. См. chart_tab_strip_h.
        let strip_h = chart_tab_strip_h(cx);
        let strip = MoonTabStrip::new("chart-tabs-strip")
            .padding_left(8.0)
            .gap(4.0)
            .bounds(MoonRect::new(0.0, 0.0, strip_w, strip_h))
            .items(items)
            .on_click({
                let tab_keys = tab_keys.clone();
                let view = view.clone();
                move |ix, event, _window, app| {
                    let Some(tab_id) = tab_keys.get(ix).cloned() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        // Дабл-клик открепляет Add и Custom-вкладки в своё ОС-окно (Main — нет).
                        if matches!(tab_id, Tab::Add(..) | Tab::Custom(..))
                            && event.click_count() >= 2
                        {
                            this.detach(tab_id, cx);
                            return;
                        }
                        let exists = matches!(tab_id, Tab::Main)
                            || this
                                .add
                                .iter()
                                .any(|(n, c, _)| Tab::Add(*n, c.clone()) == tab_id)
                            || this
                                .custom
                                .iter()
                                .any(|(n, c, _)| Tab::Custom(*n, c.clone()) == tab_id);
                        if exists && this.active != tab_id {
                            this.active = tab_id;
                            this.sync_seen_for_active(cx);
                            this.sync_active_scale(cx);
                            this.sync_inactive_chart_visibility(cx);
                            this.refresh_orderbook_gates(cx);
                            // Торговый таргет: на compare-вкладке с замком — якорь (как Main-фулскрин).
                            this.sync_main_chart_target(cx);
                            this.persist_scales(cx);
                            cx.notify();
                        }
                    });
                }
            })
            .on_close({
                let tab_keys = tab_keys.clone();
                let view = view.clone();
                move |ix, _event, _window, app| {
                    let Some(tab_id) = tab_keys.get(ix).cloned() else {
                        return;
                    };
                    if matches!(tab_id, Tab::Main) {
                        return;
                    }
                    view.update(app, |this, cx| {
                        this.add
                            .retain(|(n, c, _)| Tab::Add(*n, c.clone()) != tab_id);
                        // Кастомную вкладку закрываем совсем: убираем стек, лейбл и её спек из
                        // charts.json (закрытие = удаление сохранённой вкладки).
                        if let Tab::Custom(n, _) = &tab_id {
                            let n = *n;
                            this.custom
                                .retain(|(num, c, _)| Tab::Custom(*num, c.clone()) != tab_id);
                            this.custom_labels.remove(&n);
                            this.remove_custom_spec(n, cx);
                        }
                        if this.active == tab_id {
                            this.active = Tab::Main;
                        }
                        this.sync_seen_for_active(cx);
                        this.sync_active_scale(cx);
                        this.sync_inactive_chart_visibility(cx);
                        this.refresh_orderbook_gates(cx);
                        this.sync_main_chart_target(cx);
                        this.persist_scales(cx);
                        cx.notify();
                    });
                }
            });

        // Кнопка «собрать окна» — справа в полосе вкладок, ТОЛЬКО если у группы есть откреп-окна.
        // Восстанавливает/показывает/возвращает на экран окна чартов, если они свёрнуты/спрятаны/
        // уехали за пределы экранов (они независимы и не ходят за Main).
        let detached_count = self
            .backend
            .read(cx)
            .detached_chart_windows
            .iter()
            .filter(|(g, _)| *g == self.group)
            .count();
        let gather_btn = (detached_count > 0).then(|| {
            let entity = cx.entity();
            MoonButton::new("chart-gather-windows")
                .label("▦")
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Ghost)
                .on_click(move |_, _w, app| {
                    entity.update(app, |this, cx| this.gather_windows(cx));
                })
                .render()
        });

        // Кнопка настроек раскладки активной вкладки (⚙) + дропдаун масштаба активной
        // вкладки (рядом, слева) — оба per-вкладочные. Попап — обычный in-scene overlay:
        // chart text рисуется under-scene и не пробивает UI-слои.
        let popup_open = self.layout_popup_open;
        let p_strip = MoonPalette::active(cx);
        let scale_dropdown = crate::controls::scale_dropdown_for_tabs(
            cx,
            self.active_scale_value(cx),
            cx.entity(),
            p_strip,
        );
        let settings_btn = {
            let entity = cx.entity();
            MoonButton::new("chart-layout-settings")
                .label("⚙")
                .size(MoonButtonSize::Micro)
                .variant(if popup_open {
                    MoonButtonVariant::Blue
                } else {
                    MoonButtonVariant::Ghost
                })
                .selected(popup_open)
                .on_click(move |_, window, app| {
                    entity.update(app, |this, cx| this.toggle_layout_popup(window, cx));
                })
                .render()
        };
        // Кнопка настроек отображения свечей/трейдов (❚) — ГЛОБАЛЬНЫЙ набор, рядом с ⚙.
        let candle_popup_open = self.candle_popup_open;
        let candle_btn = {
            let entity = cx.entity();
            MoonButton::new("chart-candle-settings")
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
        };
        // Поле ввода монеты (поиск) — слева от масштаба, своё на окно; набор зависит от ядер
        // активной вкладки. Список совпадений рисуем абсолютно от обёртки поля (top_full), а сам
        // кластер выносим на уровень v_flex (ниже): overflow_hidden полоски не срежет выпадашку.
        let coin_popup = self.coin_popup_open.then(|| {
            let results = self.coin_results(cx);
            let view_toggle = cx.entity();
            let view_open = cx.entity();
            coin_search::render_popup(
                "tabs-coin",
                results,
                &self.coin_selected,
                true,
                p_strip,
                cx,
                common::coin_pick_handler(cx, self.coin_input.clone()),
                move |core, market, app| {
                    view_toggle.update(app, |this, cx| this.toggle_coin_selected(core, market, cx));
                },
                move |app| {
                    view_open.update(app, |this, cx| this.open_selected_in_new_tab(cx));
                },
            )
            .absolute()
            .top_full()
            .right_0()
            .mt(px(2.0))
        });
        let coin_search_el = div()
            .relative()
            .child(
                div().w(design::font_w_px(cx, 80.0)).child(
                    MoonInput::new("tabs-coin-search")
                        .state(&self.coin_input)
                        .cleanable(true)
                        .small(),
                ),
            )
            .children(coin_popup);
        // Слой-перехватчик клика вне списка монеты (закрыть). Ниже кластера в z-порядке.
        let coin_dismiss = self.coin_popup_open.then(|| {
            div()
                .id("tabs-coin-dismiss")
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, common::coin_dismiss_handler(cx))
        });

        let fig_tools = self.render_fig_tools(cx);

        // Правый кластер полосы вкладок: [фигуры] [монета] [масштаб] [▦?] [⚙]. ⚙ держим у
        // правого края (right≈6px) — попап раскладки якорится именно к нему (right(6) ниже).
        let right_cluster = div().absolute().right(px(6.0)).top(px(4.0)).child(
            h_flex()
                .items_center()
                .gap(px(4.0))
                .child(fig_tools)
                .child(coin_search_el)
                .child(scale_dropdown)
                .children(gather_btn)
                .child(candle_btn)
                .child(settings_btn),
        );
        // Попап раскладки ⚙ активной вкладки + слой-дисмиссер: общий оверлей с выносными
        // окнами (все колбэки — через LayoutPopupHost, см. chart_tabs/common.rs).
        let apply_all_label = if matches!(self.active, Tab::Main) {
            t!("chart.layout.apply_all_windows").to_string()
        } else {
            t!("chart.layout.apply_all_charts").to_string()
        };
        let layout_popup = common::layout_popup_overlay(
            self,
            "chart-layout",
            px(strip_h + design::ui_value(cx, 4.0)),
            apply_all_label,
            cx,
        );
        let layout_dismiss = common::layout_popup_dismiss(self, "chart-layout", cx);
        // Попап «Свечи и трейды» (глобальный) — тот же якорь, что у ⚙.
        let candle_popup = candle_popup::candle_popup_overlay(
            self,
            "chart-candles",
            px(strip_h + design::ui_value(cx, 4.0)),
            cx,
        );
        let candle_dismiss = candle_popup::candle_popup_dismiss(self, "chart-candles", cx);

        v_flex()
            .size_full()
            .relative()
            .child(
                // Только таб-стрип под overflow_hidden (режет лишние табы). Правый кластер и
                // выпадашки — отдельными детьми v_flex ниже, чтобы их не срезало по высоте полосы.
                div()
                    .h(px(strip_h))
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .child(strip),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.active_element()),
            )
            // coin_dismiss ниже кластера в z-порядке: клик по строке списка (в кластере) ловит
            // строка, клик мимо — этот слой закрывает список.
            .children(coin_dismiss)
            .child(right_cluster)
            .children(layout_dismiss)
            .children(layout_popup)
            .children(candle_dismiss)
            .children(candle_popup)
    }
}
