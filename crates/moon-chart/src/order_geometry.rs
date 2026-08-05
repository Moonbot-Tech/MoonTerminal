//! Order-line geometry: logical time_rel/price plus the orders style into the line, segment,
//! marker and zone instances the GPU pass draws.

/// Moonbot: `ShowLightLines := T.RangeT > 0.02`, where RangeT is measured in Delphi days.
const MB_TRACE_LIGHT_RANGE_MS: f32 = 0.02 * 86_400_000.0;
/// Moonbot draws MoonShot area with fixed 0.15 opacity, independent from order line alpha.
const MB_MOONSHOT_ZONE_ALPHA: f32 = 0.15;

use crate::layers::{
    LineInstance, MarkerInstance, SegInstance, ZoneInstance, MARKER_SHAPE_CROSS, MARKER_SHAPE_KNOT,
    SEG_PATTERN_DASH_DOT_DOT, SEG_PATTERN_DOT, SEG_PATTERN_SOLID,
};

use moon_core::config::{LineStyle, OrdersStyle};
use moon_core::session::order_lines::{LineKind, OrderLineStore, RetainedOrder};

/// sRGB color [u8;3] + alpha → [f32;4] (the shader converts RGB to linear).
fn rgba(c: [u8; 3], alpha: f32) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        alpha,
    ]
}

/// Traced line kinds: (style, index into RetainedOrder::lines).
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

/// Builds order-line geometry for `market`: primary order lines, a separate trace history
/// of their movement, start/end crosses, fallback-step knots, and a continuous liquidation
/// line. Culls orders outside the visible time window.
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

    // Visible set: open orders plus the newest max_closed_orders closed orders, in store-ring
    // order (without sorting; the store itself caps closed orders). Then cull by time window.
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
        // Cull by the time window (relative milliseconds).
        let start_rel = to_rel(ord.create_ms);
        let end_rel = to_rel(order_end);
        if end_rel < left_rel || start_rel > right_rel {
            continue;
        }
        // A placed but NOT yet filled order (entry unfilled, fill=0) is dimmer; after filling,
        // the line becomes brighter (as in Moonbot). Closed orders use a separate, dimmest level.
        let alpha = if closed {
            style.closed_alpha
        } else if ord.fill_pct <= 0.0 {
            // Placed but unfilled: opacity comes from the order's entry-line style according
            // to its side: `buy` (long) or `buy_short` (short). After filling, it is brighter
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

        // Liquidation is a continuous horizontal line without markers. Draw it ONLY for an active
        // (not closed) order: once the position is closed, the order closes and liquidation no
        // longer applies. Otherwise the line would linger after closure (a closed order retains
        // its last `liq` and remains in the draw set at closed_alpha for some time).
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
            // After an order closes (filled/cancelled), ONLY its entry/exit (Buy/Sell) remain
            // on the chart, translucent (`closed_alpha`). Remove stop/trailing/vstop/take-profit/
            // pending lines and their server traces for a closed order.
            if closed && idx != LineKind::Buy as usize && idx != LineKind::Sell as usize {
                continue;
            }
            // Color a short order's entry/exit with separate styles (like long/short in Moonbot:
            // BuyShort/SellShort): Buy → `buy_short`, Sell → `sell_short`.
            let st = if ord.is_short && idx == LineKind::Buy as usize {
                &style.buy_short
            } else if ord.is_short && idx == LineKind::Sell as usize {
                &style.sell_short
            } else {
                st
            };
            let line = &ord.lines[idx];
            // A DISABLED line of a live order (off_ms: stop/TP/vstop removed) is not drawn
            // AT ALL: its "history until removal" looked like a fragment artifact at the right
            // edge/in the order book (Mac tester report, 2026-07-09). Order lifetime history is
            // needed only for entry/exit (Buy/Sell), which are never disabled.
            if line.off_ms.is_some()
                && idx != LineKind::Buy as usize
                && idx != LineKind::Sell as usize
            {
                continue;
            }
            let ended = line.off_ms.is_some() || closed;
            let dashed =
                st.dashed || (idx == LineKind::Buy as usize && ord.pending && style.pending_dashed);
            // The entry line of a PLACED (not yet filled) order may have its own color
            // (`pending_color`); after filling, it uses the primary `color`. Buy line only (entry).
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
                // MoonProtoBeta already stores points in the same format as Delphi's
                // TOrderLine.SetPointTrade: an anchor plus groups of three points. Draw them
                // exactly like TOrderLine.DrawInternal, not as an ordinary polyline.
                // IMPORTANT: this is a separate server trace. It does not replace the live price
                // of the primary order line below.
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
                        markers.push(MarkerInstance::at_price(
                            to_rel(start_t),
                            preview_price,
                            st.marker_size * highlight_marker_mul,
                            st.marker_thickness * highlight_thickness_mul,
                            MARKER_SHAPE_CROSS,
                            col,
                        ));
                    }
                }
                continue;
            }
            // A line is finished if it is disabled or the order is closed. An active (unfinished)
            // line has NO END: it extends to the plot's right edge (through the order book),
            // without an end cross. A finished line ends at its disable/close time.
            let line_end = line.off_ms.unwrap_or(order_end);

            let start_t = points[0].0;
            // The current price is the last live step. The main line is STRAIGHT at the current
            // price from start to end (the entire line moves when the order is repriced).
            let cur_p = preview_price.unwrap_or(points[n - 1].1);
            let t0_rel = to_rel(start_t);
            // An active line extends to the right edge (edge_rel, through the order book); a
            // finished line extends to its own end time.
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

            // Main straight line at the current price.
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

            // Knots are fallback-step points on the straight line. For a server trace, do not
            // duplicate knots on the primary line because the trace is already a separate object.
            if st.knots && !has_server_trace {
                for i in 1..n {
                    markers.push(MarkerInstance::at_price(
                        to_rel(points[i].0),
                        cur_p,
                        st.knot_size * highlight_marker_mul,
                        st.marker_thickness * highlight_thickness_mul,
                        MARKER_SHAPE_KNOT,
                        col,
                    ));
                }
            }

            // Start and end crosses sit at the ends of the straight line (at the current price).
            if st.start_marker {
                markers.push(MarkerInstance::at_price(
                    t0_rel,
                    cur_p,
                    st.marker_size * highlight_marker_mul,
                    st.marker_thickness * highlight_thickness_mul,
                    MARKER_SHAPE_CROSS,
                    col,
                ));
            }
            if st.end_marker && ended {
                markers.push(MarkerInstance::at_price(
                    t1_rel,
                    cur_p,
                    st.marker_size * highlight_marker_mul,
                    st.marker_thickness * highlight_thickness_mul,
                    MARKER_SHAPE_CROSS,
                    col,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests;
