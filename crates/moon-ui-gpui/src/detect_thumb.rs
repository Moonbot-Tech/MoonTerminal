//! Растеризатор мини-чарта для карточки детекта: OHLC-срез 5м-свечей
//! (`moon_core::market::DetectSnapshot::bars` = `(open, high, low, close)`) → маленький
//! BGRA-битмап в `RenderImage`. Пекётся ОДИН РАЗ в момент детекта и морозится в карточке
//! (детект — историческое событие) — на холостом ходу ни CPU, ни аплоада в атлас.
//! Данные бесплатны (kline-кэш + трейд-ринг, не биржевой API); см. [[chart-candles-layer]].
//!
//! Рисуем НАСТОЯЩИЕ свечи: тонкий фитиль (high–low) + тело (open–close), цвет по
//! close≥open (up/down; равные — нейтраль). Фон ПРОЗРАЧНЫЙ — просвечивает тинт карточки.

use std::sync::Arc;

use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};

/// Испечь тумбнейл. `bars` — `(open, high, low, close)` старые→новые; `w`/`h` — физ. пиксели.
/// Цвета — RGB из темы. Фон ПРОЗРАЧНЫЙ (alpha=0). `None` при пустом входе/нулевом размере.
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
    // карточки), свечи пишем непрозрачными (alpha=0xff).
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];

    // Ценовой диапазон по high/low всех свечей.
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
    let body_w = (col_w * 0.7).max(1.0);
    let wick_w = (col_w * 0.16).max(1.0);
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
        // Фитиль (high–low) тонкой полосой по центру.
        fill(xc - wick_w * 0.5, xc + wick_w * 0.5, yfn(bh), yfn(bl), color);
        // Тело (open–close) на всю ширину свечи.
        let (yo, yc) = (yfn(o), yfn(c));
        fill(
            xc - body_w * 0.5,
            xc + body_w * 0.5,
            yo.min(yc),
            yo.max(yc),
            color,
        );
    }

    let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, buf)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(img)])))
}
