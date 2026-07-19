//! Вкладка «Календарь прибыли» окна «Аналитика».
//! Три режима (кнопки-переключатели, как пресеты периода Сводки):
//! - «Месяц» — крупные карточки-дни (дата слева, PnL+сделки+W/L+winrate справа),
//!   серый фон + красно/зелёный оверлей прозрачностью по |PnL|; ряд KPI сверху
//!   (с дельтой к пред. месяцу), полоса плюс/минус-дней снизу. Клик по дню →
//!   режим «День»;
//! - «Год» — все года разом, каждый = сетка 12 месяцев-квадратов (текущий год
//!   сверху, будущие месяцы серым). Клик по месяцу → режим «Месяц». См. `year`;
//! - «День» — детализация одного дня (пока заглушка). См. `day`.
//! Свои запросы по режиму (`cal_query` в mod.rs); период-бар окна тут скрыт.

mod day;
mod month;
mod year;

use chrono::{Datelike, NaiveDate};
use gpui::*;
use moon_ui::{MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::AnalyticsView;
use crate::design;
use crate::design::moon;

/// Режим календаря (кнопки-переключатели вкладки).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CalMode {
    Day,
    Month,
    Year,
}

impl CalMode {
    pub(super) const ALL: [CalMode; 3] = [CalMode::Year, CalMode::Month, CalMode::Day];
    pub(super) fn id(self) -> &'static str {
        match self {
            CalMode::Day => "day",
            CalMode::Month => "month",
            CalMode::Year => "year",
        }
    }
    pub(super) fn from_id(id: &str) -> Option<CalMode> {
        CalMode::ALL.into_iter().find(|m| m.id() == id)
    }
    fn title(self) -> String {
        match self {
            CalMode::Day => t!("analytics.cal.day"),
            CalMode::Month => t!("analytics.heat.month"),
            CalMode::Year => t!("analytics.heat.year"),
        }
        .to_string()
    }
}

// ── Календарные хелперы дат (UTC) ───────────────────────────────────────────

/// unix-секунды (полночь UTC) → дата (тонкая обёртка над общим хелпером окна).
pub(super) fn date_of(secs: i64) -> NaiveDate {
    super::day_of_secs(secs).unwrap_or_default()
}

/// Текущий (реальный) месяц — граница, за которую «Вперёд» не листает.
pub(super) fn now_ym() -> (i32, u32) {
    let d = date_of(moon_core::util::now_unix_ms_i64() / 1000);
    (d.year(), d.month())
}

/// Полночь текущих суток (UTC) — граница «будущего» дня в сетке.
pub(super) fn today_start() -> i64 {
    (moon_core::util::now_unix_ms_i64() / 1000).div_euclid(86_400) * 86_400
}

/// Полночь 1-го числа месяца (unix-сек UTC).
pub(super) fn month_start(y: i32, m: u32) -> i64 {
    super::secs_of_day(NaiveDate::from_ymd_opt(y, m, 1).unwrap_or_default())
}

pub(super) fn next_month(y: i32, m: u32) -> (i32, u32) {
    if m == 12 { (y + 1, 1) } else { (y, m + 1) }
}

fn prev_month(y: i32, m: u32) -> (i32, u32) {
    if m == 1 { (y - 1, 12) } else { (y, m - 1) }
}

/// Число дней в месяце (разность полуночей 1-х чисел).
fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = next_month(y, m);
    ((month_start(ny, nm) - month_start(y, m)) / 86_400).max(28) as u32
}

/// Диапазон `[from, to)` месяца — для `cal_query` (mod.rs).
pub(super) fn month_range((y, m): (i32, u32)) -> (i64, i64) {
    let (ny, nm) = next_month(y, m);
    (month_start(y, m), month_start(ny, nm))
}

/// Диапазон `[from, to)` ПРЕДЫДУЩЕГО месяца — для дельт KPI (mod.rs).
pub(super) fn prev_month_range((y, m): (i32, u32)) -> (i64, i64) {
    month_range(prev_month(y, m))
}

/// Вся история `[-1, завтра)` — режим «Год» показывает все года разом.
pub(super) fn all_history_range() -> (i64, i64) {
    (-1, today_start() + 86_400)
}

/// Сколько суток-строк показывает режим «День».
pub(super) const DAY_ROWS: i64 = 30;

/// Окно суток для режима «День»: выбранный день по центру, но НЕ выше — если
/// ниже уходило бы будущее, окно прижимается так, что низ = «сегодня»
/// (выбранный день опускается к нижней строке). Возвращает `(top, bottom)`
/// day-start; строк ровно `DAY_ROWS`, `bottom = top+(DAY_ROWS-1)д`.
pub(super) fn day_window(sel: i64) -> (i64, i64) {
    let bottom = (sel + (DAY_ROWS / 2) * 86_400).min(today_start());
    (bottom - (DAY_ROWS - 1) * 86_400, bottom)
}

/// Локализованный список из строки "a,b,c".
pub(super) fn split_i18n(s: String) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).collect()
}

impl AnalyticsView {
    /// Смена режима (+ персист) — диапазон запроса меняется, перечитываем.
    fn set_cal_mode(&mut self, m: CalMode, cx: &mut Context<Self>) {
        if self.cal_mode == m {
            return;
        }
        self.cal_mode = m;
        self.cal_hover = None;
        self.backend.update(cx, |b, _| {
            let id = Some(m.id().to_string());
            if b.layout.analytics_heat_mode != id {
                b.layout.analytics_heat_mode = id;
                b.layout_dirty = true;
            }
        });
        self.reload_calendar(cx);
        cx.notify();
    }

    /// Навигация Назад/Вперёд: «Месяц» — на месяц, «День» — на сутки, «Год» —
    /// не листается (показаны все года).
    fn cal_shift(&mut self, forward: bool, cx: &mut Context<Self>) {
        match self.cal_mode {
            CalMode::Year => return,
            CalMode::Month => {
                let (cy, cm) = now_ym();
                let (y, m) = self.cal_ym;
                if forward && (y, m) >= (cy, cm) {
                    return; // в будущее не листаем — сделок там нет
                }
                self.cal_ym = if forward { next_month(y, m) } else { prev_month(y, m) };
            }
            CalMode::Day => {
                if forward && self.cal_day >= today_start() {
                    return;
                }
                self.cal_day += if forward { 86_400 } else { -86_400 };
            }
        }
        self.cal_hover = None;
        self.reload_calendar(cx);
        cx.notify();
    }

    /// Переход на конкретный месяц (клик по ячейке года).
    pub(super) fn cal_goto_month(&mut self, y: i32, m: u32, cx: &mut Context<Self>) {
        self.cal_ym = (y, m);
        self.cal_mode = CalMode::Month;
        self.cal_hover = None;
        self.persist_cal_mode(cx);
        self.reload_calendar(cx);
        cx.notify();
    }

    /// Переход на конкретный день (клик по ячейке месяца).
    fn cal_goto_day(&mut self, dsec: i64, cx: &mut Context<Self>) {
        self.cal_day = dsec;
        self.cal_mode = CalMode::Day;
        self.cal_hover = None;
        self.persist_cal_mode(cx);
        self.reload_calendar(cx);
        cx.notify();
    }

    fn persist_cal_mode(&mut self, cx: &mut Context<Self>) {
        let id = Some(self.cal_mode.id().to_string());
        self.backend.update(cx, |b, _| {
            if b.layout.analytics_heat_mode != id {
                b.layout.analytics_heat_mode = id;
                b.layout_dirty = true;
            }
        });
    }

    /// Ховер-листенер клетки: держит `cal_hover` = день под курсором.
    fn cell_hover(
        &self,
        secs: i64,
        cx: &Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        cx.listener(move |this, hov: &bool, _, cx| {
            if *hov {
                if this.cal_hover != Some(secs) {
                    this.cal_hover = Some(secs);
                    cx.notify();
                }
            } else if this.cal_hover == Some(secs) {
                this.cal_hover = None;
                cx.notify();
            }
        })
    }

    pub(super) fn calendar_tab(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let Some(days) = self.cal_days.clone() else {
            return v_flex()
                .size_full()
                .child(self.cal_nav(p, cx))
                .child(
                    div()
                        .p(design::ui_px(cx, 18.0))
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.loading").to_string()),
                )
                .into_any_element();
        };
        // Кадр: панель навигации (как период-бар Сводки) + содержимое режима.
        let content = match self.cal_mode {
            CalMode::Month => self.calendar_month(&days, p, cx),
            CalMode::Year => self.calendar_year(&days, p, cx),
            CalMode::Day => self.calendar_day(&days, p, cx),
        };
        v_flex().size_full().child(self.cal_nav(p, cx)).child(content).into_any_element()
    }

    // ── Панель навигации (как период-бар Сводки: px10 py8) ───────────────────

    fn cal_nav(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let (y, m) = self.cal_ym;
        let months = split_i18n(t!("analytics.heat.months").to_string());
        let month_name = |mm: u32| months.get((mm as usize).saturating_sub(1)).cloned().unwrap_or_default();
        let label = match self.cal_mode {
            CalMode::Month => format!("{} {}", month_name(m), y),
            CalMode::Year => t!("analytics.cal.all_years").to_string(),
            CalMode::Day => super::fmt_day(self.cal_day),
        };
        // «Вперёд» гаснет: в «Год» — обе, в «Месяц/День» — на текущем/будущем.
        let (cy, cm) = now_ym();
        let (prev_off, next_off) = match self.cal_mode {
            CalMode::Year => (true, true),
            CalMode::Month => (false, (y, m) >= (cy, cm)),
            CalMode::Day => (false, self.cal_day >= today_start()),
        };
        let nav_btn = |id: &'static str, label: String, forward: bool, off: bool| {
            MoonButton::new(id)
                .variant(MoonButtonVariant::Soft)
                .size(MoonButtonSize::Micro)
                .disabled(off)
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.cal_shift(forward, cx)))
                .render()
        };
        let mode_btn = |mode: CalMode| {
            let on = self.cal_mode == mode;
            MoonButton::new(SharedString::from(format!("cm-{}", mode.id())))
                .variant(if on { MoonButtonVariant::Amber } else { MoonButtonVariant::Soft })
                .size(MoonButtonSize::Micro)
                .selected(on)
                .label(mode.title())
                .on_click(cx.listener(move |this, _, _, cx| this.set_cal_mode(mode, cx)))
                .render()
        };
        // Слева: Год · Месяц · День · Назад · <период> · Вперёд; заголовок справа.
        h_flex()
            .flex_none()
            .w_full()
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .items_center()
            .gap(design::ui_px(cx, 4.0))
            .child(mode_btn(CalMode::Year))
            .child(mode_btn(CalMode::Month))
            .child(mode_btn(CalMode::Day))
            .child(div().w(design::ui_px(cx, 8.0)))
            .child(nav_btn("cal-prev", t!("analytics.cal.prev").to_string(), false, prev_off))
            .child(
                div()
                    .px(design::ui_px(cx, 8.0))
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(p.text))
                    .child(label),
            )
            .child(nav_btn("cal-next", t!("analytics.cal.next").to_string(), true, next_off))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(design::t_title(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(p.text))
                    .child(t!("analytics.cal.title").to_string()),
            )
    }
}
