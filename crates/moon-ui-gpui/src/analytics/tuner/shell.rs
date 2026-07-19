//! ОБЩАЯ оболочка тюнеров: тулбар (округление / Сделать копию / Сохранить) и
//! строка подбора (попыток / сделок ≥ / сложность / Подобрать / Подобрать всё).
//! Рисуется ОДНИМ кодом для всех осей («По фильтру», «По времени», …) —
//! различаются лишь строки сетки и действия. Действия диспатчатся по `TunerKind`
//! в конкретный тюнер; у «По времени» они появятся в фазе 2b (пока disabled).

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonInput, MoonInputEvent, MoonInputState, MoonMenuSize, MoonPalette, h_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::state::TunerKind;
use crate::design;
use crate::design::moon;

impl AnalyticsView {
    /// Общий тулбар карточки тюнера: заголовок + (при выбранной стратегии)
    /// «округление результата» + «Сделать копию» + «Сохранить». Действия — по оси.
    pub(super) fn shell_toolbar(
        &self,
        kind: TunerKind,
        title: String,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let time = kind == TunerKind::Time;
        let k = if time { "t" } else { "f" };
        let round = match kind {
            TunerKind::Filter => self.tuner.round_results,
            TunerKind::Time => self.time_tuner.round_results,
        };
        let mut header = h_flex()
            .w_full()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 8.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(
                div()
                    .text_size(design::t_title(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(div().flex_1());
        // Округление результата влияет на ПОДБОР — показываем всегда (подбор доступен
        // и без выбранной стратегии, на текущем скоупе).
        header = header
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.tuner.round_lbl").to_string()),
            )
            .child(
                div().flex_none().child(
                    MoonCheckbox::new(SharedString::from(format!("tun-round-{k}")))
                        .checked(round)
                        .size(MoonCheckboxSize::Compact)
                        .on_change({
                            let view = cx.entity();
                            move |ch: &bool, _w, app| {
                                let on = *ch;
                                view.update(app, |this, cx| {
                                    match kind {
                                        TunerKind::Filter => this.tuner.round_results = on,
                                        TunerKind::Time => this.time_tuner.round_results = on,
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                ),
            );
        // Запись (Копия / Сохранить) — ТОЛЬКО в выбранную стратегию.
        if self.sel_strategy.is_some() {
            header = header
                .child(
                    MoonButton::new(SharedString::from(format!("tun-copy-{k}")))
                        .variant(MoonButtonVariant::Soft)
                        .size(MoonButtonSize::Micro)
                        .label(t!("analytics.tuner.copy_btn").to_string())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            // Единая «копия» для всех осей: правки оси → в НОВУЮ стратегию.
                            match kind {
                                TunerKind::Filter => this.open_copy_dialog(window, cx),
                                TunerKind::Time => this.time_open_copy_dialog(window, cx),
                            }
                            cx.notify();
                        }))
                        .render(),
                )
                .child({
                    // «Сохранить» горит янтарным, когда есть что записывать (пороги
                    // фильтра ИЛИ расписание времени, отличное от текущего). Работает
                    // для обеих осей; действие диспатчится по `kind`.
                    let dirty = match kind {
                        TunerKind::Filter => self.save_dirty(),
                        TunerKind::Time => self.time_tuner.is_dirty(),
                    };
                    MoonButton::new(SharedString::from(format!("tun-save-{k}")))
                        .variant(if dirty {
                            MoonButtonVariant::Amber
                        } else {
                            MoonButtonVariant::Soft
                        })
                        .size(MoonButtonSize::Micro)
                        .label(t!("analytics.tuner.save_btn").to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            match kind {
                                TunerKind::Filter => this.open_save_dialog(cx),
                                TunerKind::Time => this.time_open_save_dialog(cx),
                            }
                            cx.notify();
                        }))
                        .render()
                });
        }
        header.into_any_element()
    }

    /// Общая строка подбора: попыток / сделок ≥ / сложность (квантилей) +
    /// «Подобрать» / «Подобрать всё». Настройки — из состояния оси; кнопки
    /// диспатчатся в её автоподбор (у «По времени» пока disabled — фаза 2b).
    pub(super) fn shell_config_row(
        &mut self,
        kind: TunerKind,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let time = kind == TunerKind::Time;
        let busy = match kind {
            TunerKind::Filter => self.tuner.sugg_busy,
            TunerKind::Time => self.time_tuner.sugg_busy,
        };
        let edges = match kind {
            TunerKind::Filter => self.tuner.edges,
            TunerKind::Time => self.time_tuner.edges,
        };
        let it_input = self.shell_cfg_input(kind, 0, "20", window, cx);
        let mn_input = self.shell_cfg_input(kind, 1, &t!("analytics.tuner.auto_ph"), window, cx);
        // Число квантилей — поле со списком (4/8/…/128).
        let ed_view = cx.entity();
        let ed_items = crate::panels::radio_items(
            [4usize, 8, 16, 32, 64, 128].map(|n| {
                (
                    n,
                    SharedString::from(format!("tun-ed-{n}")),
                    SharedString::from(n.to_string()),
                )
            }),
            edges,
            crate::panels::RadioMark::Highlight,
            move |app, n| {
                ed_view.update(app, |this, cx| {
                    match kind {
                        TunerKind::Filter => this.tuner.edges = n,
                        TunerKind::Time => this.time_tuner.edges = n,
                    }
                    cx.notify();
                });
            },
        );
        let ed_combo = MoonDropdown::new(SharedString::from(format!(
            "tun-cfg-ed-{}",
            if time { "t" } else { "f" }
        )))
        .label(format!("{edges} ▾"))
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Micro)
        .menu_width(design::font_w(cx, 64.0))
        .menu_size(MoonMenuSize::Compact)
        .items(ed_items);
        let mut cfg_row = h_flex()
            .w_full()
            .px(design::ui_px(cx, 12.0))
            .pb(design::ui_px(cx, 6.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            // «попыток» — координатный спуск фильтра; у времени не используется (скрыто).
            .when(!time, |el| {
                el.child(
                    div()
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.tuner.iters").to_string()),
                )
                .child(
                    div().w(design::font_w_px(cx, 46.0)).flex_none().child(
                        MoonInput::new(SharedString::from("tun-cfg-it-f"))
                            .state(&it_input)
                            .small(),
                    ),
                )
            })
            .child(
                div()
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.tuner.min_trades").to_string()),
            )
            .child(
                div().w(design::font_w_px(cx, 52.0)).flex_none().child(
                    MoonInput::new(SharedString::from(format!(
                        "tun-cfg-mn-{}",
                        if time { "t" } else { "f" }
                    )))
                    .state(&mn_input)
                    .small(),
                ),
            )
            // «сложность» (квантили) — у времени скрыта: там макс. точность фикс.
            .when(!time, |el| {
                el.child(
                    div()
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.tuner.edges").to_string()),
                )
                .child(div().flex_none().child(ed_combo))
            })
            .child(div().flex_1());
        // Кнопки подбора видны ВСЕГДА — подбор можно запустить на текущем скоупе (без
        // выбранной стратегии = по всем показанным). «Подобрать» (по одному полю) —
        // только у фильтра; «Подобрать всё» — обе оси. Записать результат можно лишь
        // в выбранную стратегию (гейт — на Копия/Сохранить в тулбаре).
        cfg_row = cfg_row
            // «Подобрать» (по одному полю) — только у фильтра; у времени скрыта.
            .when(!time, |el| {
                el.child(
                    MoonButton::new(SharedString::from("tun-suggest-one-f"))
                        .variant(MoonButtonVariant::Soft)
                        .size(MoonButtonSize::Micro)
                        .label(if busy {
                            "…".to_string()
                        } else {
                            t!("analytics.tuner.suggest_one").to_string()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if kind == TunerKind::Filter && !this.tuner.sugg_busy {
                                this.suggest_one_into_v1(cx);
                                cx.notify();
                            }
                        }))
                        .render(),
                )
            })
            .child(
                MoonButton::new(SharedString::from(format!(
                    "tun-suggest-run-{}",
                    if time { "t" } else { "f" }
                )))
                .variant(MoonButtonVariant::Blue)
                .size(MoonButtonSize::Micro)
                .label(if busy {
                    "…".to_string()
                } else {
                    t!("analytics.tuner.suggest_run").to_string()
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    match kind {
                        TunerKind::Filter => {
                            if !this.tuner.sugg_busy {
                                this.suggest_into_v1(cx);
                                cx.notify();
                            }
                        }
                        // time_suggest сам гейтит по своему sugg_busy.
                        TunerKind::Time => {
                            this.time_suggest(cx);
                            cx.notify();
                        }
                    }
                }))
                .render(),
            );
        cfg_row.into_any_element()
    }

    /// Инпут настройки подбора (попыток / мин. сделок) для оси `kind`, с ленивым
    /// кэшем в её состоянии. `which`: 0 = попыток, 1 = мин. сделок.
    fn shell_cfg_input(
        &mut self,
        kind: TunerKind,
        which: usize,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = format!(
            "{}-cfg-{which}",
            if kind == TunerKind::Time { "t" } else { "f" }
        );
        let cached = match kind {
            TunerKind::Filter => self.tuner.inputs.get(&id),
            TunerKind::Time => self.time_tuner.inputs.get(&id),
        };
        if let Some(state) = cached {
            return state.clone();
        }
        let value = match (kind, which) {
            (TunerKind::Filter, 1) => self.tuner.min_trades.clone(),
            (TunerKind::Filter, _) => self.tuner.iters.clone(),
            (TunerKind::Time, 1) => self.time_tuner.min_trades.clone(),
            (TunerKind::Time, _) => self.time_tuner.iters.clone(),
        };
        let ph = placeholder.to_string();
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(value)
                .placeholder(ph)
        });
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            // Change тоже коммитим: значение действует сразу при клике «Подобрать».
            if matches!(
                ev,
                MoonInputEvent::Change | MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                let value = state.read(cx).value().to_string();
                match (kind, which) {
                    (TunerKind::Filter, 1) => this.tuner.min_trades = value,
                    (TunerKind::Filter, _) => this.tuner.iters = value,
                    (TunerKind::Time, 1) => this.time_tuner.min_trades = value,
                    (TunerKind::Time, _) => this.time_tuner.iters = value,
                }
                if !matches!(ev, MoonInputEvent::Change) {
                    cx.notify();
                }
            }
        })
        .detach();
        match kind {
            TunerKind::Filter => self.tuner.inputs.insert(id, state.clone()),
            TunerKind::Time => self.time_tuner.inputs.insert(id, state.clone()),
        };
        state
    }
}
