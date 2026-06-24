//! Рендер полоски чарт-вкладок (`impl Render for ChartTabs`): сам таб-стрип (Main + AddToChart),
//! кнопки «собрать окна» (▦) и настроек раскладки (⚙ + canvas-проба её rect), плюс активная панель
//! ниже. Логика вкладок/синхронизации — в [`super`] (mod.rs), выносные окна — в [`super::windows`].

use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonRect, MoonTabItem,
    MoonTabStrip, v_flex,
};
use rust_i18n::t;

use super::{CHART_TAB_STRIP_H, ChartTabs, Tab, layout_popup};
use crate::chart_persist::StackLayoutMode;

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
        let strip = MoonTabStrip::new("chart-tabs-strip")
            .padding_left(8.0)
            .gap(4.0)
            .bounds(MoonRect::new(0.0, 0.0, strip_w, CHART_TAB_STRIP_H))
            .items(items)
            .on_click({
                let tab_keys = tab_keys.clone();
                let view = view.clone();
                move |ix, event, _window, app| {
                    let Some(tab_id) = tab_keys.get(ix).cloned() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        if !matches!(tab_id, Tab::Main) && event.click_count() >= 2 {
                            this.detach(tab_id, cx);
                        } else if matches!(tab_id, Tab::Main)
                            || this
                                .add
                                .iter()
                                .any(|(n, c, _)| Tab::Add(*n, c.clone()) == tab_id)
                        {
                            if this.active != tab_id {
                                this.active = tab_id;
                                this.sync_seen_for_active(cx);
                                this.sync_active_scale(cx);
                                this.sync_inactive_chart_visibility(cx);
                                this.persist_scales(cx);
                                cx.notify();
                            }
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
                        if this.active == tab_id {
                            this.active = Tab::Main;
                        }
                        this.sync_seen_for_active(cx);
                        this.sync_active_scale(cx);
                        this.sync_inactive_chart_visibility(cx);
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
        // Правый кластер полосы вкладок: [масштаб] [▦?] [⚙]. ⚙ держим у правого края
        // (right≈6px) — попап раскладки якорится именно к нему (right(6) ниже).
        let right_cluster = div().absolute().right(px(6.0)).top(px(4.0)).child(
            moon_ui::h_flex()
                .items_center()
                .gap(px(4.0))
                .child(scale_dropdown)
                .children(gather_btn)
                .child(settings_btn),
        );
        let layout_popup = self.layout_popup_open.then(|| {
            let p = MoonPalette::active(cx);
            let mode = self.active_layout_mode(cx).unwrap_or(StackLayoutMode::Fit);
            let orderbook_enabled = self.active_orderbook_enabled(cx);
            let include_main = matches!(self.active, Tab::Main);
            let apply_all_label = if include_main {
                t!("chart.layout.apply_all_windows").to_string()
            } else {
                t!("chart.layout.apply_all_charts").to_string()
            };
            let pick_entity = cx.entity();
            let all_entity = cx.entity();
            let ob_entity = cx.entity();
            let hover_entity = cx.entity();
            let size = layout_popup::content_size(cx);
            div()
                .id("chart-layout-popup-scene")
                .absolute()
                .right(px(6.0))
                .top(px(CHART_TAB_STRIP_H + 4.0))
                .w(size.width)
                .h(size.height)
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
                    "chart-layout",
                    mode,
                    &self.layout_fit_input,
                    &self.layout_scroll_input,
                    orderbook_enabled,
                    p,
                    cx,
                    move |mode, app| {
                        pick_entity.update(app, |this, cx| {
                            let hf = this.read_layout_height(StackLayoutMode::Fit, cx);
                            let hs = this.read_layout_height(StackLayoutMode::Scroll, cx);
                            this.apply_layout(Some(mode), hf, hs, cx);
                        });
                    },
                    apply_all_label,
                    move |app| {
                        all_entity.update(app, |this, cx| {
                            let hf = this.read_layout_height(StackLayoutMode::Fit, cx);
                            let hs = this.read_layout_height(StackLayoutMode::Scroll, cx);
                            let mode =
                                Some(this.active_layout_mode(cx).unwrap_or(StackLayoutMode::Fit));
                            // Копируем ВСЕ настройки активной вкладки: + масштаб + галку стакана.
                            let scale = this.active_scale_value(cx);
                            let ob = Some(this.active_orderbook_enabled(cx));
                            this.apply_layout_to_all(include_main, mode, hf, hs, scale, ob, cx);
                        });
                    },
                    move |checked, app| {
                        ob_entity.update(app, |this, cx| this.apply_orderbook(checked, cx));
                    },
                ))
        });
        let layout_dismiss = self.layout_popup_open.then(|| {
            let entity = cx.entity();
            div()
                .id("chart-layout-popup-dismiss")
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, move |_, _window, app| {
                    entity.update(app, |this, cx| this.close_layout_popup(true, cx));
                    app.stop_propagation();
                })
        });

        v_flex()
            .size_full()
            .relative()
            .child(
                div()
                    .h(px(CHART_TAB_STRIP_H))
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .child(strip)
                    .child(right_cluster),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.active_element()),
            )
            .children(layout_dismiss)
            .children(layout_popup)
    }
}
