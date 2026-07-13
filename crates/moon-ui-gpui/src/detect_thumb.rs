//! Растеризатор мини-чарта для карточки детекта: OHLC-срез 5м-свечей
//! (`moon_core::market::DetectSnapshot::bars` = `(open, high, low, close)`) → маленький
//! BGRA-битмап в `RenderImage`. Пекётся ОДИН РАЗ в момент детекта и морозится в карточке
//! (детект — историческое событие) — на холостом ходу ни CPU, ни аплоада в атлас.
//! Данные бесплатны (kline-кэш + трейд-ринг, не биржевой API); см. [[chart-candles-layer]].
//!
//! Рисуем ПОЛЫЕ свечи (hollow candlesticks, как TradingView): тонкий фитиль high–low +
//! тело open–close, которое ПОЛОЕ (контур) при росте (close≥open) и ЗАКРАШЕННОЕ при падении.
//! Цвет по close≷open (up/down; равные — нейтраль/полое). Масштаб по high–low (фитили
//! наполняют высоту → плоские свечи видны). Фон ПРОЗРАЧНЫЙ — просвечивает тинт карточки.

use std::sync::Arc;

use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};

/// Испечь тумбнейл. `bars` — `(open, high, low, close)` старые→новые (high/low не рисуем);
/// `w`/`h` — физ. пиксели. Цвета — RGB из темы. Фон ПРОЗРАЧНЫЙ (alpha=0). `None` при пустом/0.
pub fn render_thumb(
    bars: &[(f32, f32, f32, f32)],
    w: u32,
    h: u32,
    up: [u8; 3],
    down: [u8; 3],
    neutral: [u8; 3],
) -> Option<Arc<RenderImage>> {
    if w == 0 || h == 0 || bars.is_empty() {
        return None;
    }
    // Буфер нулевой = прозрачный чёрный BGRA [0,0,0,0]: фон не заливаем (просвечивает тинт
    // карточки), тела пишем непрозрачными (alpha=0xff).
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];

    // Ценовой диапазон по high/low всех свечей (фитили наполняют высоту).
    let mut hi = f32::NEG_INFINITY;
    let mut lo = f32::INFINITY;
    for &(_, bh, bl, _) in bars {
        hi = hi.max(bh);
        lo = lo.min(bl);
    }
    if !hi.is_finite() || !lo.is_finite() {
        return None;
    }
    let span = (hi - lo).max(1e-9);

    let n = bars.len() as f32;
    let col_w = (w as f32 / n).max(1.0);
    let wick_w = (col_w * 0.22).clamp(1.0, 3.0);
    let body_w = (col_w * 0.68).max(wick_w);
    let pad_y = if h > 6 { 1.0 } else { 0.0 };
    let usable_h = (h as f32 - 2.0 * pad_y).max(1.0);
    let yfn = |price: f32| pad_y + (hi - price) / span * usable_h;

    // Залить прямоугольник [x0,x1)×[y0,y1) цветом (min высота 1px).
    let mut fill = |x0: f32, x1: f32, y0: f32, y1: f32, color: [u8; 3]| {
        let xa = (x0.floor().max(0.0)) as i32;
        let xb = ((x1.ceil() as i32).max(xa + 1)).min(w as i32);
        let ya = (y0.floor().max(0.0)) as i32;
        let yb = ((y1.ceil() as i32).max(ya + 1)).min(h as i32);
        for y in ya..yb {
            let row = (y as usize) * (w as usize);
            for x in xa..xb {
                let idx = (row + x as usize) * 4;
                buf[idx] = color[2];
                buf[idx + 1] = color[1];
                buf[idx + 2] = color[0];
                buf[idx + 3] = 0xff;
            }
        }
    };

    for (i, &(o, bh, bl, c)) in bars.iter().enumerate() {
        let xc = (i as f32 + 0.5) * col_w;
        let color = if c > o {
            up
        } else if c < o {
            down
        } else {
            neutral
        };
        let (x0, x1) = (xc - body_w * 0.5, xc + body_w * 0.5);
        let (yt, yb) = (yfn(o).min(yfn(c)), yfn(o).max(yfn(c)));
        // Фитиль (high–low) — двумя сегментами НАД и ПОД телом (не сквозь него), тонкой полосой.
        let (wx0, wx1) = (xc - wick_w * 0.5, xc + wick_w * 0.5);
        if yt - yfn(bh) > 0.5 {
            fill(wx0, wx1, yfn(bh), yt, color);
        }
        if yfn(bl) - yb > 0.5 {
            fill(wx0, wx1, yb, yfn(bl), color);
        }
        // Тело: падение (close<open) — ЗАКРАШЕНО; рост/доджи — ПОЛОЕ (контур 1px, внутри тинт).
        if c < o {
            fill(x0, x1, yt, yb, color);
        } else {
            let t = 1.0_f32;
            fill(x0, x1, yt, yt + t, color); // верх
            fill(x0, x1, yb - t, yb, color); // низ
            fill(x0, x0 + t, yt, yb, color); // лево
            fill(x1 - t, x1, yt, yb, color); // право
        }
    }

    let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, buf)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(img)])))
}

/// Спарклайн (простая линия цены) для режима «линия». `prices` — close-цены старые→новые;
/// цвет: `up` если последняя ≥ первой (рост), иначе `down`. Фон ПРОЗРАЧНЫЙ. Линия непрерывна
/// (по колонкам, соединяя соседние точки). `None` при <2 точках / нулевом размере.
pub fn render_line(
    prices: &[f32],
    w: u32,
    h: u32,
    up: [u8; 3],
    down: [u8; 3],
) -> Option<Arc<RenderImage>> {
    if w == 0 || h == 0 || prices.len() < 2 {
        return None;
    }
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];

    let color = if prices[prices.len() - 1] >= prices[0] {
        up
    } else {
        down
    };

    // Сглаживание: усредняем сырые точки в бины (~1 точка на 3px ширины) — иначе 288 шумных
    // 5м-close рисуются «рывками» как свечи. Бины дают плавную линию.
    let target = ((w / 3).max(2) as usize).min(prices.len());
    let pts: Vec<f32> = (0..target)
        .map(|k| {
            let a = k * prices.len() / target;
            let b = (((k + 1) * prices.len() / target).max(a + 1)).min(prices.len());
            let slice = &prices[a..b];
            slice.iter().copied().filter(|v| v.is_finite()).sum::<f32>()
                / (slice.iter().filter(|v| v.is_finite()).count().max(1) as f32)
        })
        .collect();
    let n = pts.len();

    // hi/lo — по СГЛАЖЕННОЙ серии (что реально рисуем!), а НЕ по сырым ценам: иначе выброс-спайк
    // в сырых данных раздувал `hi`, и вся усреднённая линия прижималась ко дну диапазона.
    let mut hi = f32::NEG_INFINITY;
    let mut lo = f32::INFINITY;
    for &p in &pts {
        if p.is_finite() {
            hi = hi.max(p);
            lo = lo.min(p);
        }
    }
    if !hi.is_finite() || !lo.is_finite() {
        return None;
    }
    let span = (hi - lo).max(1e-9);
    // Поля сверху/снизу (18%), чтобы линия не упиралась/не резалась о рамку.
    let pad_y = (h as f32 * 0.18).max(2.0);
    let usable_h = (h as f32 - 2.0 * pad_y).max(1.0);
    let yfn = |p: f32| pad_y + (hi - p) / span * usable_h;
    // Полутолщина в px: холст = размеру показа (1:1), 1.0 → линия ~2px на экране.
    let thick = 1.0_f32;

    // По колонкам: интерполируем y между сглаженными точками и соединяем с предыдущей.
    let mut prev_y: Option<f32> = None;
    for x in 0..w {
        let t = (x as f32) / ((w.saturating_sub(1)).max(1) as f32) * ((n - 1).max(1) as f32);
        let i = (t.floor() as usize).min(n - 1);
        let y = if i + 1 < n {
            let frac = t - i as f32;
            yfn(pts[i]) * (1.0 - frac) + yfn(pts[i + 1]) * frac
        } else {
            yfn(pts[i])
        };
        let (y0, y1) = match prev_y {
            Some(p) => (p.min(y), p.max(y)),
            None => (y, y),
        };
        let ya = ((y0 - thick).floor().max(0.0)) as i32;
        let yb = (((y1 + thick).ceil()) as i32).min(h as i32);
        for yy in ya..yb {
            let idx = ((yy as usize) * (w as usize) + x as usize) * 4;
            buf[idx] = color[2];
            buf[idx + 1] = color[1];
            buf[idx + 2] = color[0];
            buf[idx + 3] = 0xff;
        }
        prev_y = Some(y);
    }

    let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, buf)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(img)])))
}
