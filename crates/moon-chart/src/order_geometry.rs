//! Order-line geometry: logical time_rel/price plus the orders style into the line, segment,
//! marker and zone instances the GPU pass draws.

const SEG_PATTERN_SOLID: f32 = 0.0;
const SEG_PATTERN_DASH_DOT_DOT: f32 = 1.0;
const SEG_PATTERN_DOT: f32 = 2.0;
/// Moonbot: `ShowLightLines := T.RangeT > 0.02`, где RangeT — Delphi days.
const MB_TRACE_LIGHT_RANGE_MS: f32 = 0.02 * 86_400_000.0;
/// Moonbot draws MoonShot area with fixed 0.15 opacity, independent from order line alpha.
const MB_MOONSHOT_ZONE_ALPHA: f32 = 0.15;

use crate::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};

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
        // линия становится ярче (как в Moonbot). Закрытый — отдельный, самый тусклый уровень.
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
                    rgba(style.take_profit.color, MB_MOONSHOT_ZONE_ALPHA),
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
            // После закрытия ордера (исполнен/отменён) на графике остаются ТОЛЬКО вход/выход
            // (Buy/Sell) полупрозрачными (`closed_alpha`). Стоп/трейлинг/встоп/ТП/pending-линии
            // и их серверные трассы у закрытого ордера убираем.
            if closed && idx != LineKind::Buy as usize && idx != LineKind::Sell as usize {
                continue;
            }
            // Шорт-ордер красим вход/выход отдельными стилями (как long/short в Moonbot:
            // BuyShort/SellShort): Buy → `buy_short`, Sell → `sell_short`.
            let st = if ord.is_short && idx == LineKind::Buy as usize {
                &style.buy_short
            } else if ord.is_short && idx == LineKind::Sell as usize {
                &style.sell_short
            } else {
                st
            };
            let line = &ord.lines[idx];
            // ВЫКЛЮЧЕННАЯ линия живого ордера (off_ms: стоп/TP/vstop сняли) не рисуется
            // ВООБЩЕ — «история до момента снятия» выглядела огрызком-артефактом у правого
            // края/в стакане (репорт мак-тестера 2026-07-09). История жизни ордера нужна
            // только входу/выходу (Buy/Sell), они off не бывают.
            if line.off_ms.is_some()
                && idx != LineKind::Buy as usize
                && idx != LineKind::Sell as usize
            {
                continue;
            }
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

            let trace_points = &line.server_points;
            let has_server_trace = !trace_points.is_empty();
            if has_server_trace {
                // MoonProtoBeta уже хранит points в том же формате, что Delphi
                // TOrderLine.SetPointTrade: anchor + группы по 3 точки. Рисуем
                // именно как TOrderLine.DrawInternal, а не как обычную polyline.
                // ВАЖНО: это отдельная серверная трасса. Она не подменяет live-цену
                // рабочей линии ордера ниже.
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
                while k + 3 < trace_points.len() {
                    let p0 = trace_points[k];
                    let p1 = trace_points[k + 1];
                    let p2 = trace_points[k + 2];
                    let p3 = trace_points[k + 3];
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
                        (trace_points.last(), line.tmp_point)
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
                    trace_points.first(),
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
            }

            let points = &line.steps;
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
            // Текущая цена — последняя live-ступень. Основная линия ПРЯМАЯ на текущей цене
            // от начала до конца (вся переезжает при перестановке).
            let cur_p = preview_price.unwrap_or(points[n - 1].1);
            let t0_rel = to_rel(start_t);
            // Активная линия — до правого края (edge_rel, через стакан); завершённая —
            // до своего времени конца.
            let t1_rel = if ended { to_rel(line_end) } else { edge_rel };

            if !has_server_trace && path.show && n > 1 {
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
mod tests;
