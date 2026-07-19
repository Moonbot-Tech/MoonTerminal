//! Три heatmap-ползунка «По времени» под сеткой строк: цветная дорожка (средний
//! профит по ячейкам 🟢/🟠) + два перетаскиваемых хендла (от/до) + подсветка
//! выбранного участка. Как триммер в видео.
//!   - «Недельно» (поле 0): 168 ячеек день×час; тянет `WorkingWeekTime` (минута недели).
//!   - «Сутки»    (поле 1): 24 ячейки час суток; тянет WorkingTime (режим суток).
//!   - «В часе»   (поле 2): 60 ячеек минута-в-часе; тянет WorkingTime (режим часа).
//! «Сутки» и «В часе» — одно поле WorkingTime → активный чистит другой (взаимоискл.).
//!
//! Значение = ПОЛЯ строки (источник истины): полный диапазон = «без ограничения» =
//! пусто. Drag читает захваченные `slider_track` bounds (через `canvas`) и пишет поля;
//! движение мыши ловит overlay в `strat_time`, чтобы не терять курсор за дорожкой.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::super::calendar::split_i18n;
use super::grid::{WEEK_MIN, fmt_min, fmt_week_ep, parse_moh, parse_time};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::SliderProfiles;

/// Цвет ячейки по среднему профиту `v` и максимуму `cmax` (🟢 плюс / 🟠 минус).
fn cell_color(v: f32, cmax: f32, p: MoonPalette) -> Hsla {
    let a = (v.abs() / cmax * 0.9).min(0.9);
    if v > 0.0 {
        moon_alpha(p.green, a)
    } else if v < 0.0 {
        moon_alpha(p.orange, a)
    } else {
        moon_alpha(p.panel_high, 0.4)
    }
}

/// Полоса-heatmap: `data` (профит по ячейкам) в `n≤maxcells` сегментов, КАЖДЫЙ —
/// 2-стоповый градиент от цвета соседа слева к своему → плавные переходы без граней
/// (форк-градиент держит только 2 стопа). Крупные оси (сутки 1440) прорежаются.
fn heat_gradient_row(data: &[f32], maxcells: usize, p: MoonPalette) -> Div {
    let len = data.len().max(1);
    let n = len.min(maxcells.max(1));
    let cmax = data.iter().fold(0f32, |m, &v| m.max(v.abs())).max(1e-9);
    let bucket = |i: usize| -> f32 {
        let a = i * len / n;
        let b = ((i + 1) * len / n).max(a + 1).min(len);
        data[a..b].iter().sum::<f32>() / (b - a) as f32
    };
    let sign = |v: f32| {
        if v > 0.0 {
            1i8
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    };
    let mut row = h_flex().size_full();
    let mut prev_c = cell_color(bucket(0), cmax, p);
    let mut prev_s = sign(bucket(0));
    for i in 0..n {
        let v = bucket(i);
        let c = cell_color(v, cmax, p);
        let s = sign(v);
        // Блендим ТОЛЬКО внутри одного знака (🟢→🟢 / 🟠→🟠); на смене знака — резко,
        // иначе градиент green↔orange проходит через жёлтую грязь.
        let from = if s != 0 && s == prev_s { prev_c } else { c };
        row = row.child(div().flex_1().h_full().bg(linear_gradient(
            90.0,
            linear_color_stop(from, 0.0),
            linear_color_stop(c, 1.0),
        )));
        prev_c = c;
        prev_s = s;
    }
    row
}

/// Ряд тик-подписей над дорожкой (день/час/минута), выровнены по левым краям сегментов.
fn tick_row(
    labels: Vec<String>,
    boundaries: bool,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> Div {
    let base = h_flex()
        .w_full()
        .flex_none()
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_muted));
    if boundaries {
        // Метки на ГРАНИЦАХ (0,3,…,24 / 0,5,…,60): justify_between = ровно на долях k/(n-1).
        base.justify_between()
            .children(labels.into_iter().map(|l| div().flex_none().child(l)))
    } else {
        // Подписи ПО СЕГМЕНТАМ (дни недели): по центру своей 1/n доли.
        let mut row = base;
        for l in labels {
            row = row.child(div().flex_1().min_w_0().truncate().text_center().child(l));
        }
        row
    }
}

/// Ячейки профиля ползунка `field` (0 неделя×час, 1 час суток, 2 минута в часе).
fn slider_cells(pr: &SliderProfiles, field: usize) -> &[f32] {
    match field {
        0 => &pr.week,
        1 => &pr.day,
        _ => &pr.hour,
    }
}

/// Вертикальный хендл ползунка на доле `frac` (0..1) ширины дорожки.
fn handle_bar(frac: f32, p: MoonPalette, cx: &Context<AnalyticsView>) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(relative(frac))
        .w(design::ui_px(cx, 2.0))
        .bg(moon(p.text))
}

impl AnalyticsView {
    /// Максимум ползунка `field` в его единицах (минута недели / минута суток / минута часа).
    pub(super) fn slider_max(field: usize) -> u16 {
        match field {
            0 => WEEK_MIN - 1,
            1 => 1439,
            _ => 59,
        }
    }

    /// Текущий диапазон ползунка `field` из полей строки; пусто → полный диапазон.
    fn slider_range(&self, field: usize) -> (u16, u16) {
        match field {
            0 => self.time_tuner.week_span(0).unwrap_or((0, WEEK_MIN - 1)),
            1 => {
                let (f, t) = &self.time_tuner.bounds[0][1];
                (parse_time(f).unwrap_or(0), parse_time(t).unwrap_or(1439))
            }
            _ => {
                let (f, t) = &self.time_tuner.bounds[0][2];
                (
                    parse_moh(f).unwrap_or(0) as u16,
                    parse_moh(t).unwrap_or(59) as u16,
                )
            }
        }
    }

    /// Значение в единицах ползунка по X мыши (через захваченные bounds дорожки).
    fn slider_value_at(&self, field: usize, x: Pixels) -> u16 {
        let Some(b) = self.slider_track[field] else {
            return 0;
        };
        let w = f32::from(b.size.width);
        if w <= 0.0 {
            return 0;
        }
        let frac = ((f32::from(x) - f32::from(b.origin.x)) / w).clamp(0.0, 1.0);
        (frac * Self::slider_max(field) as f32).round() as u16
    }

    /// Записать диапазон ползунка в поля строки. Полный диапазон → очистка («без
    /// ограничения»). WT-строки (1/2) взаимоисключаем. Пересчёт KPI.
    fn set_slider_range(&mut self, field: usize, from: u16, to: u16, cx: &mut Context<Self>) {
        let max = Self::slider_max(field);
        let (from, to) = (from.min(to), from.max(to));
        let full = from == 0 && to == max;
        if full {
            self.clear_field(0, field);
        } else {
            let (a, b) = match field {
                0 => (fmt_week_ep(from, false), fmt_week_ep(to, true)),
                1 => (fmt_min(from), fmt_min(to)),
                _ => (from.to_string(), to.to_string()),
            };
            self.set_v1_cell(field, a, b);
        }
        // «Сутки»↔«В часе» — одно поле WorkingTime: активная строка чистит другую.
        if !full && field == 1 {
            self.clear_field(0, 2);
        }
        if !full && field == 2 {
            self.clear_field(0, 1);
        }
        // БЕЗ reload_time: во время drag только двигаем поля/полосу; KPI пересчитаем
        // ОДИН раз на отпускании (`slider_release`) — иначе шторм SQL-запросов.
        cx.notify();
    }

    /// Завершить drag ползунка: снять флаг и пересчитать KPI ОДИН раз (на отпускании).
    fn slider_release(&mut self, cx: &mut Context<Self>) {
        if self.slider_drag.take().is_some() {
            self.reload_time(cx);
            cx.notify();
        }
    }

    /// Обновить перетаскиваемый край (`is_from`) ползунка `field` до значения `value`.
    pub(super) fn slider_drag_to(
        &mut self,
        field: usize,
        is_from: bool,
        value: u16,
        cx: &mut Context<Self>,
    ) {
        let (mut from, mut to) = self.slider_range(field);
        if is_from {
            from = value.min(to);
        } else {
            to = value.max(from);
        }
        self.set_slider_range(field, from, to, cx);
    }

    /// ЛКМ down по дорожке: выбрать ближний хендл, начать drag и сразу подвинуть.
    fn slider_mouse_down(&mut self, field: usize, x: Pixels, cx: &mut Context<Self>) {
        let value = self.slider_value_at(field, x);
        let (from, to) = self.slider_range(field);
        let is_from = (value as i32 - from as i32).abs() <= (value as i32 - to as i32).abs();
        self.slider_drag = Some((field, is_from));
        self.slider_drag_to(field, is_from, value, cx);
    }

    /// Блок трёх ползунков под сеткой строк.
    pub(super) fn time_sliders(&mut self, p: MoonPalette, cx: &mut Context<Self>) -> AnyElement {
        let prof = self.time_slider.clone();
        let mut col = v_flex()
            .w_full()
            .flex_none()
            .gap(design::ui_px(cx, 5.0))
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 6.0))
            .border_t_1()
            .border_color(moon(p.border));
        for field in 0..3usize {
            col = col.child(self.time_slider_row(field, prof.as_deref(), p, cx));
        }
        col.into_any_element()
    }

    /// Одна строка-ползунок: подпись + цветная дорожка с хендлами.
    fn time_slider_row(
        &self,
        field: usize,
        prof: Option<&SliderProfiles>,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let max = Self::slider_max(field) as f32;
        let (from, to) = self.slider_range(field);
        let (ff, tf) = (from as f32 / max, to as f32 / max);
        // Регион без инверсии (wrap-спан от>до вводится вручную — не рисуем внахлёст).
        let (rl, rr) = (ff.min(tf), ff.max(tf));
        // Пара «Сутки»/«Час» — одно поле WorkingTime: неиспользуемую строку приглушаем.
        let active = self.time_tuner.active_wt();
        let dim = matches!((field, active), (2, Some(1)));
        // Тайлинг-каскад: маска активного НИЖЕЛЕЖАЩЕГО поля проецируется на ЭТУ дорожку —
        // «В часе» → на «Сутки» И «Недельно» (в каждом часе); «Сутки» → на «Недельно»
        // (в каждом дне). ЗАТЕНЯЕМ ИСКЛЮЧЁННОЕ (не в окне) — активное показывает heatmap.
        let total = Self::slider_max(field) as u32 + 1;
        let dark_bands: Vec<(u16, u16)> = match active {
            Some(2) if field != 2 => {
                let (hf, ht) = self.slider_range(2);
                let mut v = Vec::new();
                for h in 0..(total / 60) as u16 {
                    let base = h * 60;
                    if hf > 0 {
                        v.push((base, base + hf - 1)); // до окна часа
                    }
                    if ht < 59 {
                        v.push((base + ht + 1, base + 59)); // после окна часа
                    }
                }
                v
            }
            Some(1) if field == 0 => {
                let (df, dt) = self.slider_range(1);
                let mut v = Vec::new();
                for d in 0..7u16 {
                    let base = d * 1440;
                    if df > 0 {
                        v.push((base, base + df - 1)); // до окна суток
                    }
                    if dt < 1439 {
                        v.push((base + dt + 1, base + 1439)); // после окна суток
                    }
                }
                v
            }
            _ => vec![],
        };
        let label_key = match field {
            0 => "analytics.time.field_week",
            1 => "analytics.time.field_day",
            _ => "analytics.time.field_hour",
        };
        // Ячейки-градиент; сутки (1440) прорежаем до 240 для перфа.
        let maxcells = match field {
            0 => 168usize,
            1 => 240,
            _ => 60,
        };
        let empty = [0.0f32];
        let data = prof.map(|pr| slider_cells(pr, field)).unwrap_or(&empty);
        let cells_row = heat_gradient_row(data, maxcells, p);
        // Тики над дорожкой: неделя — дни (по сегментам); сутки — часы 0..24 (каждые 3);
        // час — минуты 0..60 (каждые 5). `bound` = метки на границах (justify_between).
        let (ticks, tick_bound): (Vec<String>, bool) = match field {
            0 => (split_i18n(t!("analytics.heat.weekdays").to_string()), false),
            1 => ((0..=8u16).map(|k| (k * 3).to_string()).collect(), true),
            _ => ((0..=12u16).map(|k| (k * 5).to_string()).collect(), true),
        };

        let view = cx.entity();
        let mut track = div()
            .id(SharedString::from(format!("tt-sl-{field}")))
            .relative()
            .w_full()
            .h(design::ui_px(cx, 30.0))
            .rounded(design::ui_px(cx, 3.0))
            .overflow_hidden()
            .cursor(CursorStyle::OpenHand)
            .bg(moon(p.panel_high))
            .child(cells_row);
        // Затенить ИСКЛЮЧЁННЫЕ интервалы маски (тайлинг-каскад) — тёмная штриховка;
        // активное время остаётся heatmap. Рисуем ПОД своим выделением/хендлами.
        let total_f = total as f32;
        for (a, b) in &dark_bands {
            let l = *a as f32 / total_f;
            let w = (*b as f32 - *a as f32 + 1.0) / total_f;
            track = track.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(l))
                    .w(relative(w))
                    .bg(moon_alpha(p.shell, 0.72)),
            );
        }
        track = track
            // Своё выделение (недельный спан / окно) — ПОВЕРХ затенения, только рамка.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(rl))
                    .right(relative(1.0 - rr))
                    .border_2()
                    .border_color(moon(p.accent)),
            )
            // Два хендла (от/до).
            .child(handle_bar(ff, p, cx))
            .child(handle_bar(tf, p, cx));
        let track = track
            // Захват bounds дорожки для перевода X мыши в значение.
            .child(
                canvas(
                    move |bounds, _window, app| {
                        view.update(app, |this, _| this.slider_track[field] = Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _w, cx| {
                    this.slider_mouse_down(field, e.position.x, cx);
                }),
            );

        h_flex()
            .w_full()
            .items_end()
            .gap(design::ui_px(cx, 6.0))
            .opacity(if dim { 0.5 } else { 1.0 })
            .child(
                div()
                    .w(design::font_w_px(cx, 60.0))
                    .flex_none()
                    .pb(design::ui_px(cx, 3.0))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_soft))
                    .child(t!(label_key).to_string()),
            )
            .child(
                // Тики над дорожкой + сама дорожка (выровнены по ширине).
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(design::ui_px(cx, 1.0))
                    .child(tick_row(ticks, tick_bound, p, cx))
                    .child(track),
            )
            .into_any_element()
    }

    /// Прозрачный overlay на всё тело «По времени» на время drag ползунка: ловит
    /// движение мыши (даже за дорожкой) и отпускание. Рисуется только пока тянем.
    pub(super) fn slider_drag_overlay(&self, cx: &Context<Self>) -> Option<AnyElement> {
        self.slider_drag?;
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .cursor(CursorStyle::ClosedHand)
                .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _w, cx| {
                    if let Some((field, is_from)) = this.slider_drag {
                        let v = this.slider_value_at(field, e.position.x);
                        this.slider_drag_to(field, is_from, v, cx);
                    }
                }))
                // Отпускание внутри тела И вне его (`_out`, не hover-гейтится) — иначе
                // релиз за пределами overlay/окна оставил бы drag «залипшим».
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _e, _w, cx| this.slider_release(cx)),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _e, _w, cx| this.slider_release(cx)),
                )
                .into_any_element(),
        )
    }
}
