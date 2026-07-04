//! Форматирование значений тулбара (поля метрик, size/sell) и шаги колеса мыши.
//! Вынесено из `controls.rs` точь-в-точь.

use gpui::{ScrollDelta, ScrollWheelEvent};

/// Формат значения с сотыми, точка-разделитель: `50` → "50.00".
pub fn fmt_field2(v: f32) -> String {
    format!("{v:.2}")
}

/// Со знаком (для SL, который может быть и +, и −): `1` → "+1.00", `-20` → "-20.00".
pub fn fmt_field2_signed(v: f32) -> String {
    format!("{v:+.2}")
}

/// Умная подпись значения по порядку величины (size/sell). Точность адаптивная: ≥100 — целое
/// (без десятых), 10..100 — десятые (без сотых), 1..10 — сотые, <1 — столько знаков, чтобы
/// показать ~2 значащих (0.6→"0.6", 0.001→"0.001", 0.00001→"0.00001"). Хвостовые нули убираем
/// (убирает и float-мусор от f32, напр. 0.6000000238 → "0.6").
pub fn fmt_adaptive(v: f64) -> String {
    let a = v.abs();
    let decimals: usize = if a == 0.0 {
        0
    } else if a >= 100.0 {
        0
    } else if a >= 10.0 {
        1
    } else if a >= 1.0 {
        2
    } else {
        // <1: ~2 значащих цифры. 0.6→2, 0.001→4, 0.00001→6.
        let lead = (-a.log10().floor()) as i32; // 0.6→1, 0.001→3, 0.00001→5
        (lead + 1).clamp(2, 8) as usize
    };
    let s = format!("{:.*}", decimals, v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

pub(super) fn fmt_sell_pct(v: f64) -> String {
    let a = v.abs();
    if a >= 1000.0 {
        let k = v / 1000.0;
        let mut s = if k.abs() >= 100.0 {
            format!("{k:.0}")
        } else if k.abs() >= 10.0 {
            format!("{k:.1}")
        } else {
            format!("{k:.2}")
        };
        if s.contains('.') {
            s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        }
        format!("{s}k")
    } else {
        fmt_adaptive(v)
    }
}

/// Шаг колеса по порядку величины: `step = frac · 10^floor(log10(v))`. `frac=1.0` — полный
/// разряд (размер: 18→20→30; 93→100→200; 980→1000→2000; 0.001→0.002). `frac=0.5` — полразряда
/// (sell: 10→15→20→25; 0.1→0.15→0.2). Вверх — следующий кратный, вниз — предыдущий; на точной
/// степени 10 шаг вниз падает на разряд ниже (111→100→90→80, а не стоп на 100).
pub(super) fn wheel_step(value: f64, up: bool, frac: f64) -> f64 {
    if !(value > 0.0) {
        return value;
    }
    let step = frac * 10f64.powf(value.log10().floor());
    let raw = if up {
        ((value / step + 1e-9).floor() + 1.0) * step
    } else {
        let mut down = ((value / step - 1e-9).ceil() - 1.0) * step;
        if down <= 0.0 {
            // value на точной ступени (например 100 при frac=1) → один шаг разрядом ниже.
            let lower = frac * 10f64.powf((value * (1.0 - 1e-9)).log10().floor());
            down = value - lower;
        }
        if down <= 0.0 {
            return value;
        }
        down
    };
    (raw * 1e8).round() / 1e8
}

/// Направление колеса (вверх = +Y). Если в реале инвертировано — поменять знак сравнения.
pub(super) fn scroll_up(ev: &ScrollWheelEvent) -> bool {
    let y = match ev.delta {
        ScrollDelta::Lines(p) => p.y,
        ScrollDelta::Pixels(p) => f32::from(p.y),
    };
    y > 0.0
}
