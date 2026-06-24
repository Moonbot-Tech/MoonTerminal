//! Чистая логика разметки осей: «круглые» шаги сетки цены/времени, число знаков
//! после запятой, формат часов. UI-агностично (ни egui, ни gpui) — рисуют поверх
//! и egui-оболочка (`src/chart/axes.rs`), и GPUI-оболочка (`moon-ui-gpui`), беря
//! отсюда одинаковые тики. Порт из moonweb (`coords.ts` niceInterval/priceDecimals).

/// Срез состояния вида, нужный шкалам. Снимается ПОСЛЕ кадра (значения текущего
/// кадра рендера). Общий тип для обеих оболочек.
#[derive(Clone, Copy)]
pub struct AxisSnapshot {
    /// Пикселей на миллисекунду (физ.) — ширина окна времени.
    pub px_per_ms: f32,
    /// Доля окна-«будущего» справа (right_margin_frac).
    pub right_margin_frac: f32,
    /// Цена в центре зоны и видимый диапазон.
    pub render_center: f32,
    pub render_range: f32,
    /// Точка отсчёта времени (unix ms) и время у правого якоря (unix ms).
    pub epoch_ms: f64,
    pub right_time_ms: f64,
    /// Смещение локального времени от UTC, сек (для подписей часов).
    pub tz_offset_sec: i64,
}

/// «Круглый» шаг сетки цены для ~`target_lines` линий (порт niceInterval).
pub fn nice_interval(range: f32, target_lines: f32) -> f32 {
    let rough = range / target_lines.max(1.0);
    if !(rough > 0.0) {
        return 1.0;
    }
    let mag = 10f32.powf(rough.log10().floor());
    let n = rough / mag;
    let nice = if n < 1.5 {
        1.0
    } else if n < 3.0 {
        2.0
    } else if n < 7.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Знаков после запятой для цены такого порядка (порт priceDecimals).
pub fn price_decimals(price: f32) -> usize {
    let p = price.abs();
    if p >= 1000.0 {
        1
    } else if p >= 10.0 {
        2
    } else if p >= 1.0 {
        3
    } else {
        4
    }
}

/// «Круглый» шаг времени, сек, чтобы влезло ~`target` подписей.
pub fn nice_time_step(window_sec: f64, target: f64) -> f64 {
    const STEPS: [f64; 16] = [
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
        14400.0, 21600.0,
    ];
    let want = window_sec / target.max(1.0);
    for s in STEPS {
        if s >= want {
            return s;
        }
    }
    STEPS[STEPS.len() - 1]
}

/// Часы локального времени из unix ms (HH:MM:SS или HH:MM).
pub fn fmt_clock(unix_ms: f64, offset_sec: i64, with_sec: bool) -> String {
    let total = (unix_ms / 1000.0).floor() as i64 + offset_sec;
    let sod = ((total % 86_400) + 86_400) % 86_400;
    let (h, m, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    if with_sec {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}")
    }
}
