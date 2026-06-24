//! Чарт-математика и геометрия, wgpu-free. Даёт общей UI-оболочке:
//! layout-константы осей/стакана, тик-математику осей (`axes`), вид (`view::ChartView`
//! — зум/пан/Y), типы инстансов линий ордеров (`layers`), и `build_order_geometry`
//! (логические time_rel/price → примитивы).
//!
//! Сами данные рисует НАШ own-pass DX11 (`chartdx` в moon-ui-gpui), а не wgpu-движок:
//! старый wgpu-рендер (Chart/canvas/слои/style) удалён вместе с egui-бинарём.

// Подписи осей рисует UI-оболочка (GPUI-оверлей). Здесь — только layout-константы.
pub const PRICE_AXIS_W: f32 = 56.0;
pub const TIME_AXIS_H: f32 = 16.0;
/// Ширина зоны стакана справа (как BOOK_WIDTH_CSS стенда = 220), физ. пиксели.
pub const GLASS_ZONE_PX: f32 = 220.0;

pub mod axes;
pub mod container;
// `data` / market-source models live in moon-core. Ре-экспорт под прежним путём.
pub use moon_core::data;
pub mod layers;
pub mod paint;
pub mod view;

use layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};

use moon_core::config::{LineStyle, OrdersStyle};
use moon_core::session::order_lines::{LineKind, OrderLineStore, RetainedOrder};

/// sRGB-цвет [u8;3] + alpha → [f32;4] (шейдер переводит rgb в linear).
fn rgba(c: [u8; 3], alpha: f32) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        alpha,
    ]
}

/// Виды трассируемых линий: (стиль, индекс в RetainedOrder::lines).
fn traced_kinds(s: &OrdersStyle) -> [(&LineStyle, usize); 7] {
    [
        (&s.buy, LineKind::Buy as usize),
        (&s.sell, LineKind::Sell as usize),
        (&s.stop, LineKind::Stop as usize),
        (&s.trailing, LineKind::Trailing as usize),
        (&s.take_profit, LineKind::TakeProfit as usize),
        (&s.vstop, LineKind::VStop as usize),
        (&s.pending_cond, LineKind::PendingCond as usize),
    ]
}

/// Собирает геометрию линий ордеров рынка `market`: отрезки лестницы (горизонтали
/// на ступенях + вертикальные стыки), кресты начала/конца, узелки перестановок и
/// непрерывную линию ликвидации. Куллит ордера вне видимого окна по времени.
#[allow(clippy::too_many_arguments)]
pub fn build_order_geometry(
    store: &OrderLineStore,
    market: &str,
    style: &OrdersStyle,
    highlight_uid: Option<u64>,
    drag_preview: Option<(u64, LineKind, f32)>,
    epoch_ms: f64,
    now_ms: f64,
    left_rel: f32,
    right_rel: f32,
    edge_rel: f32,
    zones: &mut Vec<ZoneInstance>,
    hlines: &mut Vec<LineInstance>,
    segs: &mut Vec<SegInstance>,
    markers: &mut Vec<MarkerInstance>,
) {
    zones.clear();
    hlines.clear();
    segs.clear();
    markers.clear();
    let to_rel = |t_ms: f64| (t_ms - epoch_ms) as f32;
    let kinds = traced_kinds(style);

    // Видимые: открытые + новейшие max_closed_orders закрытых, в порядке кольца стора
    // (без сорта — кап на закрытые делает сам стор). Дальше культим по окну времени.
    let visible: Vec<&RetainedOrder> =
        store.market_draw_orders(market, style.max_closed_orders as usize);
    for ord in visible {
        let closed = ord.closed_ms.is_some();
        let highlighted = highlight_uid == Some(ord.uid) && !closed;
        let drag_preview = drag_preview.filter(|(uid, _, price)| {
            *uid == ord.uid && !closed && price.is_finite() && *price > 0.0
        });
        let highlight_alpha_mul = if highlighted { 1.45 } else { 1.0 };
        let highlight_thickness_mul = if highlighted { 1.7 } else { 1.0 };
        let highlight_marker_mul = if highlighted { 1.25 } else { 1.0 };
        let order_end = ord.closed_ms.unwrap_or(now_ms);
        // Куллинг по окну времени (rel ms).
        let start_rel = to_rel(ord.create_ms);
        let end_rel = to_rel(order_end);
        if end_rel < left_rel || start_rel > right_rel {
            continue;
        }
        let alpha = if closed {
            style.closed_alpha
        } else {
            style.active_alpha
        };
        let line_alpha = (alpha * highlight_alpha_mul).min(1.0);

        if !closed {
            let mut push_zone = |a: f32, b: f32, color: [f32; 4]| {
                if a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 && (a - b).abs() > 1e-9 {
                    zones.push(ZoneInstance {
                        price0: a.min(b),
                        price1: a.max(b),
                        color,
                    });
                }
            };
            if ord.is_moon_shot {
                push_zone(
                    ord.corridor_price_down,
                    ord.corridor_price_up,
                    rgba(style.take_profit.color, alpha * 0.14),
                );
            }
            if ord.panic_sell {
                let buy = ord.lines[LineKind::Buy as usize].current_price();
                let sell = ord.lines[LineKind::Sell as usize].current_price();
                if let (Some(a), Some(b)) = (buy, sell) {
                    push_zone(a, b, rgba(style.stop.color, alpha * 0.12));
                }
            }
        }

        // Ликвидация — непрерывная горизонталь без маркеров.
        if let Some(p) = ord.liq {
            let s = &style.liq;
            hlines.push(LineInstance {
                price: p,
                color: rgba(s.color, line_alpha),
                style: if s.dashed { 1.0 } else { 0.0 },
                thickness: s.thickness * highlight_thickness_mul,
            });
        }

        let path = &style.path;
        let path_col = rgba(path.color, alpha);
        let path_dash = if path.dashed { 1.0 } else { 0.0 };

        for (st, idx) in kinds {
            let line = &ord.lines[idx];
            let ended = line.off_ms.is_some() || closed;
            let dashed =
                st.dashed || (idx == LineKind::Buy as usize && ord.pending && style.pending_dashed);
            let col = rgba(st.color, line_alpha);
            let dash = if dashed { 1.0 } else { 0.0 };
            let thickness = st.thickness * highlight_thickness_mul;

            if let Some((_, _, preview_price)) =
                drag_preview.filter(|(_, kind, _)| *kind as usize == idx)
            {
                let start_t = line
                    .server_points
                    .first()
                    .map(|(t, _)| *t)
                    .or_else(|| line.steps.first().map(|(t, _)| *t))
                    .unwrap_or(ord.create_ms);
                segs.push(SegInstance {
                    t0_rel: to_rel(start_t),
                    p0: preview_price,
                    t1_rel: edge_rel,
                    p1: preview_price,
                    thickness,
                    dashed: dash,
                    extend: 1.0,
                    color: col,
                });
                if st.start_marker {
                    markers.push(MarkerInstance {
                        t_rel: to_rel(start_t),
                        price: preview_price,
                        size: st.marker_size * highlight_marker_mul,
                        thickness: st.marker_thickness * highlight_thickness_mul,
                        shape: 0.0,
                        color: col,
                    });
                }
                continue;
            }

            if !line.server_points.is_empty() {
                for pair in line.server_points.windows(2) {
                    let (t0, p0) = pair[0];
                    let (t1, p1) = pair[1];
                    let t0_rel = to_rel(t0);
                    let t1_rel = to_rel(t1);
                    if t0_rel.max(t1_rel) < left_rel || t0_rel.min(t1_rel) > right_rel {
                        continue;
                    }
                    // П.8: ступенчатая (step-after) ордер-линия под ПРЯМЫМ углом, а не
                    // диагональ. Цена p0 действует с t0 до момента изменения t1 → горизонталь
                    // на уровне p0; затем скачок до p1 → вертикаль на t1. Раньше пара
                    // соединялась одним наклонным сегментом (подступенек был наклонным).
                    segs.push(SegInstance {
                        t0_rel,
                        p0,
                        t1_rel,
                        p1: p0,
                        thickness,
                        dashed: dash,
                        extend: 0.0,
                        color: col,
                    });
                    segs.push(SegInstance {
                        t0_rel: t1_rel,
                        p0,
                        t1_rel,
                        p1,
                        thickness,
                        dashed: dash,
                        extend: 0.0,
                        color: col,
                    });
                }

                if !ended {
                    let (last_t, last_p) = *line.server_points.last().unwrap();
                    if let Some((tmp_t, tmp_p)) = line.tmp_point {
                        // Живая temp-точка — ступенькой под ПРЯМЫМ углом, а не косым пунктиром.
                        // В норме цена та же (tmp_p == last_p) → видна только горизонталь; при
                        // перестановке ордера раньше мелькала диагональ от старой точки к новой
                        // (пользователь жаловался на «косой пунктир»). Горизонталь на last_p до
                        // tmp_t, затем вертикаль на tmp_t до tmp_p (вертикаль вырождена, если цена
                        // не менялась).
                        let last_rel = to_rel(last_t);
                        let tmp_rel = to_rel(tmp_t);
                        segs.push(SegInstance {
                            t0_rel: last_rel,
                            p0: last_p,
                            t1_rel: tmp_rel,
                            p1: last_p,
                            thickness,
                            dashed: 1.0,
                            extend: 0.0,
                            color: col,
                        });
                        segs.push(SegInstance {
                            t0_rel: tmp_rel,
                            p0: last_p,
                            t1_rel: tmp_rel,
                            p1: tmp_p,
                            thickness,
                            dashed: 1.0,
                            extend: 0.0,
                            color: col,
                        });
                    } else {
                        segs.push(SegInstance {
                            t0_rel: to_rel(last_t),
                            p0: last_p,
                            t1_rel: 0.0,
                            p1: last_p,
                            thickness,
                            dashed: dash,
                            extend: 1.0,
                            color: col,
                        });
                    }
                }

                if st.start_marker {
                    let (t, p) = line.server_points[0];
                    markers.push(MarkerInstance {
                        t_rel: to_rel(t),
                        price: p,
                        size: st.marker_size * highlight_marker_mul,
                        thickness: st.marker_thickness * highlight_thickness_mul,
                        shape: 0.0,
                        color: col,
                    });
                }
                if st.knots {
                    for &(t, p) in line.server_points.iter().skip(1) {
                        markers.push(MarkerInstance {
                            t_rel: to_rel(t),
                            price: p,
                            size: st.knot_size * highlight_marker_mul,
                            thickness: st.marker_thickness * highlight_thickness_mul,
                            shape: 1.0,
                            color: col,
                        });
                    }
                }
                if st.end_marker && ended {
                    if let Some(&(t, p)) = line.server_points.last() {
                        markers.push(MarkerInstance {
                            t_rel: to_rel(t),
                            price: p,
                            size: st.marker_size * highlight_marker_mul,
                            thickness: st.marker_thickness * highlight_thickness_mul,
                            shape: 0.0,
                            color: col,
                        });
                    }
                }
                continue;
            }

            let n = line.steps.len();
            if n == 0 {
                continue;
            }
            // Линия завершена, если выключена сама или закрыт ордер. У активной
            // (незавершённой) линии КОНЦА НЕТ — она тянется до правого края plot
            // (через стакан), без креста конца. У завершённой конец = off/close время.
            let line_end = line.off_ms.unwrap_or(order_end);

            let start_t = line.steps[0].0;
            // Текущая цена — последняя ступень. Основная линия ПРЯМАЯ на текущей цене
            // от начала до конца (вся переезжает при перестановке).
            let cur_p = line.steps[n - 1].1;
            let t0_rel = to_rel(start_t);
            // Активная линия — до правого края (edge_rel, через стакан); завершённая —
            // до своего времени конца.
            let t1_rel = if ended { to_rel(line_end) } else { edge_rel };

            // Опциональный «путь» (trail): змейка реальных позиций по истории —
            // рисуем ПОД основной линией, своим стилем.
            if path.show && n > 1 {
                for i in 0..n {
                    let (t, p) = line.steps[i];
                    let seg_end_t = if i + 1 < n {
                        line.steps[i + 1].0
                    } else {
                        line_end
                    };
                    if seg_end_t > t {
                        segs.push(SegInstance {
                            t0_rel: to_rel(t),
                            p0: p,
                            t1_rel: to_rel(seg_end_t),
                            p1: p,
                            thickness: path.thickness,
                            dashed: path_dash,
                            extend: 0.0,
                            color: path_col,
                        });
                    }
                    if i + 1 < n {
                        let p2 = line.steps[i + 1].1;
                        segs.push(SegInstance {
                            t0_rel: to_rel(seg_end_t),
                            p0: p,
                            t1_rel: to_rel(seg_end_t),
                            p1: p2,
                            thickness: path.thickness,
                            dashed: path_dash,
                            extend: 0.0,
                            color: path_col,
                        });
                    }
                }
            }

            // Основная прямая линия на текущей цене.
            segs.push(SegInstance {
                t0_rel,
                p0: cur_p,
                t1_rel,
                p1: cur_p,
                thickness,
                dashed: dash,
                extend: if ended { 0.0 } else { 1.0 },
                color: col,
            });

            // Узелки — точки на прямой линии в моменты перестановок (steps[1..]).
            if st.knots {
                for i in 1..n {
                    markers.push(MarkerInstance {
                        t_rel: to_rel(line.steps[i].0),
                        price: cur_p,
                        size: st.knot_size * highlight_marker_mul,
                        thickness: st.marker_thickness * highlight_thickness_mul,
                        shape: 1.0,
                        color: col,
                    });
                }
            }

            // Крест начала и конца — на концах прямой линии (на текущей цене).
            if st.start_marker {
                markers.push(MarkerInstance {
                    t_rel: t0_rel,
                    price: cur_p,
                    size: st.marker_size * highlight_marker_mul,
                    thickness: st.marker_thickness * highlight_thickness_mul,
                    shape: 0.0,
                    color: col,
                });
            }
            if st.end_marker && ended {
                markers.push(MarkerInstance {
                    t_rel: t1_rel,
                    price: cur_p,
                    size: st.marker_size * highlight_marker_mul,
                    thickness: st.marker_thickness * highlight_thickness_mul,
                    shape: 0.0,
                    color: col,
                });
            }
        }
    }
}
