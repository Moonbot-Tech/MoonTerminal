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
const SEG_PATTERN_SOLID: f32 = 0.0;
const SEG_PATTERN_DASH_DOT_DOT: f32 = 1.0;
const SEG_PATTERN_DOT: f32 = 2.0;
/// MoonBot: `ShowLightLines := T.RangeT > 0.02`, где RangeT — Delphi days.
const MB_TRACE_LIGHT_RANGE_MS: f32 = 0.02 * 86_400_000.0;

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

/// Собирает геометрию линий ордеров рынка `market`: рабочие линии ордеров,
/// отдельную trace-историю их движения, кресты начала/конца, узелки fallback-
/// ступеней и непрерывную линию ликвидации. Куллит ордера вне видимого окна по
/// времени.
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
        // Выставленный, но ещё НЕ исполненный (вход не залит, fill=0) → тусклее: после исполнения
        // линия становится ярче (как в MoonBot). Закрытый — отдельный, самый тусклый уровень.
        let alpha = if closed {
            style.closed_alpha
        } else if ord.fill_pct <= 0.0 {
            // Выставлен, но не залит → прозрачность настраивается на входной линии ордера
            // по его стороне: `buy` (лонг) либо `buy_short` (шорт). После исполнения — ярче
            // (`active_alpha`).
            if ord.is_short {
                style.buy_short.pending_alpha
            } else {
                style.buy.pending_alpha
            }
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

        // Ликвидация — непрерывная горизонталь без маркеров. Рисуем ТОЛЬКО у активного
        // (не закрытого) ордера: закрыли позицию → ордер закрыт → ликвидации больше нет.
        // Иначе линия «висела» бы после закрытия (closed-ордер держит последний `liq` и
        // ещё какое-то время остаётся в наборе отрисовки на closed_alpha).
        if !closed {
            if let Some(p) = ord.liq {
                let s = &style.liq;
                hlines.push(LineInstance {
                    price: p,
                    color: rgba(s.color, line_alpha),
                    style: if s.dashed { 1.0 } else { 0.0 },
                    thickness: s.thickness * highlight_thickness_mul,
                });
            }
        }

        let path = &style.path;
        let path_col = rgba(path.color, alpha);
        let path_dash = if path.dashed {
            SEG_PATTERN_DASH_DOT_DOT
        } else {
            SEG_PATTERN_SOLID
        };

        for (st, idx) in kinds {
            // Шорт-ордер красим вход/выход отдельными стилями (как long/short в MoonBot:
            // BuyShort/SellShort): Buy → `buy_short`, Sell → `sell_short`.
            let st = if ord.is_short && idx == LineKind::Buy as usize {
                &style.buy_short
            } else if ord.is_short && idx == LineKind::Sell as usize {
                &style.sell_short
            } else {
                st
            };
            let line = &ord.lines[idx];
            let ended = line.off_ms.is_some() || closed;
            let dashed =
                st.dashed || (idx == LineKind::Buy as usize && ord.pending && style.pending_dashed);
            // Входная линия ВЫСТАВЛЕННОГО (ещё не залит) ордера может иметь свой цвет
            // (`pending_color`); после фила — основной `color`. Только Buy-линия (вход).
            let line_color = if idx == LineKind::Buy as usize && ord.fill_pct <= 0.0 {
                st.pending_color.unwrap_or(st.color)
            } else {
                st.color
            };
            let col = rgba(line_color, line_alpha);
            let dash = if dashed {
                SEG_PATTERN_DASH_DOT_DOT
            } else {
                SEG_PATTERN_SOLID
            };
            let thickness = st.thickness * highlight_thickness_mul;
            let preview_price = drag_preview
                .filter(|(_, kind, _)| *kind as usize == idx)
                .map(|(_, _, price)| price);

            let has_server_trace = !line.server_points.is_empty();
            let points: &[(f64, f32)] = if has_server_trace {
                &line.server_points
            } else {
                &line.steps
            };
            let n = points.len();
            if n == 0 {
                if let Some(preview_price) = preview_price {
                    let start_t = ord.create_ms;
                    segs.push(SegInstance {
                        t0_rel: to_rel(start_t),
                        p0: preview_price,
                        t1_rel: edge_rel,
                        p1: preview_price,
                        thickness,
                        pattern: dash,
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
                }
                continue;
            }
            // Линия завершена, если выключена сама или закрыт ордер. У активной
            // (незавершённой) линии КОНЦА НЕТ — она тянется до правого края plot
            // (через стакан), без креста конца. У завершённой конец = off/close время.
            let line_end = line.off_ms.unwrap_or(order_end);

            let start_t = points[0].0;
            // Текущая цена — последняя ступень. Основная линия ПРЯМАЯ на текущей цене
            // от начала до конца (вся переезжает при перестановке).
            let cur_p = preview_price.unwrap_or(points[n - 1].1);
            let t0_rel = to_rel(start_t);
            // Активная линия — до правого края (edge_rel, через стакан); завершённая —
            // до своего времени конца.
            let t1_rel = if ended { to_rel(line_end) } else { edge_rel };

            if has_server_trace {
                // MoonProtoBeta уже хранит points в том же формате, что Delphi
                // TOrderLine.SetPointTrade: anchor + группы по 3 точки. Рисуем
                // именно как TOrderLine.DrawInternal, а не как обычную polyline.
                let show_light_lines = (right_rel - left_rel) > MB_TRACE_LIGHT_RANGE_MS;
                let base_trace_alpha = if highlighted {
                    style.trace_alpha.max(0.7)
                } else {
                    style.trace_alpha
                };
                let trace_alpha = if show_light_lines {
                    base_trace_alpha * 0.5
                } else {
                    base_trace_alpha
                };
                let trace_color = rgba(line_color, trace_alpha);
                let trace_thickness = if highlighted { 2.0 } else { 1.0 };
                let trace_dash = if show_light_lines {
                    SEG_PATTERN_SOLID
                } else {
                    SEG_PATTERN_DASH_DOT_DOT
                };
                let trace_inner_dash = if show_light_lines {
                    SEG_PATTERN_SOLID
                } else {
                    SEG_PATTERN_DOT
                };
                let valid_trace_point = |(t, p): (f64, f32)| t > 1.0 && p.is_finite() && p > 0.0;

                let mut k = 0;
                while k + 3 < n {
                    let p0 = points[k];
                    let p1 = points[k + 1];
                    let p2 = points[k + 2];
                    let p3 = points[k + 3];
                    if valid_trace_point(p0) && valid_trace_point(p1) {
                        let a = to_rel(p0.0);
                        let b = to_rel(p1.0);
                        if a.max(b) < left_rel || a.min(b) > right_rel {
                            k += 3;
                            continue;
                        }
                    }
                    if valid_trace_point(p0) && valid_trace_point(p1) {
                        segs.push(SegInstance {
                            t0_rel: to_rel(p0.0),
                            p0: p0.1,
                            t1_rel: to_rel(p1.0),
                            p1: p1.1,
                            thickness: trace_thickness,
                            pattern: trace_dash,
                            extend: 0.0,
                            color: trace_color,
                        });
                    }
                    if valid_trace_point(p1) && valid_trace_point(p3) {
                        segs.push(SegInstance {
                            t0_rel: to_rel(p1.0),
                            p0: p1.1,
                            t1_rel: to_rel(p3.0),
                            p1: p3.1,
                            thickness: trace_thickness,
                            pattern: trace_dash,
                            extend: 0.0,
                            color: trace_color,
                        });
                    }
                    if valid_trace_point(p2) {
                        segs.push(SegInstance {
                            t0_rel: to_rel(p2.0),
                            p0: p1.1,
                            t1_rel: to_rel(p2.0),
                            p1: p2.1,
                            thickness: 1.0,
                            pattern: trace_inner_dash,
                            extend: 0.0,
                            color: trace_color,
                        });
                    }
                    k += 3;
                }

                if !ended {
                    if let (Some(&(last_t, last_p)), Some((tmp_t, tmp_p))) =
                        (points.last(), line.tmp_point)
                    {
                        if valid_trace_point((last_t, last_p)) && valid_trace_point((tmp_t, tmp_p))
                        {
                            segs.push(SegInstance {
                                t0_rel: to_rel(tmp_t),
                                p0: last_p,
                                t1_rel: to_rel(tmp_t),
                                p1: tmp_p,
                                thickness: 1.0,
                                pattern: SEG_PATTERN_DOT,
                                extend: 0.0,
                                color: trace_color,
                            });
                        }
                    }
                }

                if let (Some(stop_price), Some(stop_time_ms), Some(&(start_time, _))) = (
                    line.server_stop_price,
                    line.server_stop_time_ms,
                    points.first(),
                ) {
                    if start_time > 1.0
                        && stop_time_ms > 1.0
                        && stop_price.is_finite()
                        && stop_price > 0.0
                    {
                        segs.push(SegInstance {
                            t0_rel: to_rel(start_time),
                            p0: stop_price,
                            t1_rel: to_rel(stop_time_ms),
                            p1: stop_price,
                            thickness: 2.0,
                            pattern: SEG_PATTERN_DOT,
                            extend: 0.0,
                            color: rgba(style.stop.color, trace_alpha),
                        });
                    }
                }
            } else if path.show && n > 1 {
                for i in 0..n {
                    let (t, p) = points[i];
                    let seg_end_t = if i + 1 < n { points[i + 1].0 } else { line_end };
                    if seg_end_t > t {
                        segs.push(SegInstance {
                            t0_rel: to_rel(t),
                            p0: p,
                            t1_rel: to_rel(seg_end_t),
                            p1: p,
                            thickness: path.thickness,
                            pattern: path_dash,
                            extend: 0.0,
                            color: path_col,
                        });
                    }
                    if i + 1 < n {
                        let p2 = points[i + 1].1;
                        segs.push(SegInstance {
                            t0_rel: to_rel(seg_end_t),
                            p0: p,
                            t1_rel: to_rel(seg_end_t),
                            p1: p2,
                            thickness: path.thickness,
                            pattern: path_dash,
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
                pattern: dash,
                extend: if ended { 0.0 } else { 1.0 },
                color: col,
            });

            // Узелки — точки fallback-steps на прямой линии. Для серверной трассы
            // не дублируем узлы на рабочей линии: сама трасса уже отдельный объект.
            if st.knots && !has_server_trace {
                for i in 1..n {
                    markers.push(MarkerInstance {
                        t_rel: to_rel(points[i].0),
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

#[cfg(test)]
mod tests {
    use super::*;
    use moon_core::feed::{OrderRow, OrderTrace, OrderTracePoint};

    fn test_order_with_buy_trace() -> OrderRow {
        OrderRow {
            market: "BTCUSDT".into(),
            is_short: false,
            size: 0.01,
            sl_on: false,
            ts_on: false,
            sl_strat: false,
            ts_strat: false,
            vstop_on: false,
            buy_price: 60_000.0,
            sell_price: 0.0,
            create_time_ms: 1_000.0,
            price: 61_000.0,
            fill_pct: 0.0,
            strat: "test".into(),
            strat_id: 0,
            status: String::new(),
            uid: 42,
            emulator: false,
            job_is_done: false,
            pending: false,
            filled: false,
            stop_loss: None,
            trailing: None,
            take_profit: None,
            vstop: None,
            pending_cond: None,
            liq: None,
            panic_sell: false,
            is_moon_shot: false,
            corridor_price_down: 0.0,
            corridor_price_up: 0.0,
            buy_trace: Some(OrderTrace {
                points: vec![
                    OrderTracePoint {
                        time_ms: 1_000.0,
                        price: 60_000.0,
                    },
                    OrderTracePoint {
                        time_ms: 2_000.0,
                        price: 60_000.0,
                    },
                    OrderTracePoint {
                        time_ms: 0.0,
                        price: 0.0,
                    },
                    OrderTracePoint {
                        time_ms: 2_000.0,
                        price: 61_000.0,
                    },
                ],
                tmp_point: Some(OrderTracePoint {
                    time_ms: 2_500.0,
                    price: 61_500.0,
                }),
                stop_price: Some(59_500.0),
                stop_time_ms: Some(2_000.0),
            }),
            sell_trace: None,
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.001
    }

    #[test]
    fn server_trace_is_separate_from_active_order_line() {
        let mut store = OrderLineStore::default();
        assert!(store.update(&[test_order_with_buy_trace()]));

        let mut zones = Vec::new();
        let mut hlines = Vec::new();
        let mut segs = Vec::new();
        let mut markers = Vec::new();
        build_order_geometry(
            &store,
            "BTCUSDT",
            &OrdersStyle::default(),
            None,
            None,
            0.0,
            3_000.0,
            0.0,
            10_000.0,
            10_000.0,
            &mut zones,
            &mut hlines,
            &mut segs,
            &mut markers,
        );

        assert!(
            segs.iter().any(|s| {
                near(s.extend, 1.0)
                    && near(s.p0, 61_000.0)
                    && near(s.p1, 61_000.0)
                    && near(s.t0_rel, 1_000.0)
            }),
            "active order line must stay a straight current-price segment"
        );
        assert!(
            segs.iter().any(|s| {
                near(s.extend, 0.0)
                    && near(s.t0_rel, 1_000.0)
                    && near(s.t1_rel, 2_000.0)
                    && near(s.p0, 60_000.0)
                    && near(s.p1, 60_000.0)
                    && near(s.pattern, SEG_PATTERN_DASH_DOT_DOT)
                    && near(s.thickness, 1.0)
            }),
            "server trace must keep its own horizontal history segment"
        );
        assert!(
            segs.iter().any(|s| {
                near(s.extend, 0.0)
                    && near(s.t0_rel, 2_000.0)
                    && near(s.t1_rel, 2_000.0)
                    && near(s.p0, 60_000.0)
                    && near(s.p1, 61_000.0)
                    && near(s.pattern, SEG_PATTERN_DASH_DOT_DOT)
                    && near(s.thickness, 1.0)
            }),
            "server trace must keep its own vertical price-change segment"
        );
        assert!(
            segs.iter().any(|s| {
                near(s.extend, 0.0)
                    && near(s.t0_rel, 2_500.0)
                    && near(s.t1_rel, 2_500.0)
                    && near(s.p0, 61_000.0)
                    && near(s.p1, 61_500.0)
                    && near(s.pattern, SEG_PATTERN_DOT)
                    && near(s.thickness, 1.0)
            }),
            "server trace temp point must be drawn as MoonBot dotted vertical preview"
        );
        assert!(
            segs.iter().any(|s| {
                near(s.extend, 0.0)
                    && near(s.t0_rel, 1_000.0)
                    && near(s.t1_rel, 2_000.0)
                    && near(s.p0, 59_500.0)
                    && near(s.p1, 59_500.0)
                    && near(s.pattern, SEG_PATTERN_DOT)
                    && near(s.thickness, 2.0)
            }),
            "server trace stop-line must be drawn like MoonBot SetStopPrice"
        );
    }

    #[test]
    fn dragging_order_keeps_server_trace_visible() {
        let mut store = OrderLineStore::default();
        assert!(store.update(&[test_order_with_buy_trace()]));

        let mut zones = Vec::new();
        let mut hlines = Vec::new();
        let mut segs = Vec::new();
        let mut markers = Vec::new();
        build_order_geometry(
            &store,
            "BTCUSDT",
            &OrdersStyle::default(),
            None,
            Some((42, LineKind::Buy, 62_000.0)),
            0.0,
            3_000.0,
            0.0,
            10_000.0,
            10_000.0,
            &mut zones,
            &mut hlines,
            &mut segs,
            &mut markers,
        );

        assert!(
            segs.iter().any(|s| {
                near(s.extend, 1.0)
                    && near(s.p0, 62_000.0)
                    && near(s.p1, 62_000.0)
                    && near(s.t0_rel, 1_000.0)
            }),
            "drag preview must move only the active order line"
        );
        assert!(
            segs.iter().any(|s| {
                near(s.extend, 0.0)
                    && near(s.t0_rel, 1_000.0)
                    && near(s.t1_rel, 2_000.0)
                    && near(s.p0, 60_000.0)
                    && near(s.p1, 60_000.0)
                    && near(s.pattern, SEG_PATTERN_DASH_DOT_DOT)
            }),
            "drag preview must not hide the server trace object"
        );
    }
}
