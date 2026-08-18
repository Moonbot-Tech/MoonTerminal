//! Order-line geometry: logical time_rel/price plus the orders style into the line, segment,
//! marker and zone instances the GPU pass draws.

/// Moonbot: `ShowLightLines := T.RangeT > 0.02`, where RangeT is measured in Delphi days.
const MB_TRACE_LIGHT_RANGE_MS: f32 = 0.02 * 86_400_000.0;
/// Moonbot draws MoonShot area with fixed 0.15 opacity, independent from order line alpha.
const MB_MOONSHOT_ZONE_ALPHA: f32 = 0.15;

use crate::layers::{
    LineInstance, MARKER_SHAPE_CROSS, MARKER_SHAPE_KNOT, MarkerInstance, SEG_EXTEND_EDGE,
    SEG_EXTEND_NONE, SEG_PATTERN_DASH_DOT_DOT, SEG_PATTERN_DOT, SEG_PATTERN_SOLID, SegInstance,
    ZoneInstance,
};

use moon_core::config::{ChartGraphicsCfg, LineStyle, OrdersStyle};
use moon_core::session::order_lines::{LineKind, OrderLineStore, RetainedOrder};

use crate::layers::rgb_with_alpha as rgba;

// The entry line's fill mark speaks the closed-trade-history vocabulary on purpose: the user
// should read a fill on an order line exactly as they read one in trade history, so it reuses
// that layer's glyphs and its resting half-extents rather than inventing a shape of its own.
use crate::layers::{MARKER_SHAPE_ARROW_DOWN, MARKER_SHAPE_ARROW_UP};
use crate::trade_marks::{ARROW_HALF_H, ARROW_HALF_W, clamp_arrow_scale};

/// Colour of the still-holding continuation drawn from a filled entry to the pane's right edge.
///
/// A literal rather than an [`OrdersStyle`] field on purpose: this segment is not a LINE the core
/// reports and the user can drag, it is a read-out of the entry line's own state, so it carries no
/// per-kind style of its own and inherits the entry's THICKNESS. `palette::ORANGE` is reused rather
/// than redefined so the chart keeps one orange.
///
/// It does NOT inherit the entry's dash, and that is deliberate rather than an oversight: the entry
/// line dashes to say "placed, not yet filled" (`pending_dashed`), and this segment exists only once
/// the entry HAS filled. Carrying the dash across the fill arrow would keep flying the pending flag
/// over a position that is actually held, which is the opposite of what the segment is for.
const POSITION_HOLD_RGB: [u8; 3] = moon_core::palette::ORANGE;

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
/// of their movement, start crosses plus end crosses or filled-entry arrows, fallback-step knots,
/// and a continuous liquidation line. Culls orders outside the visible time window.
///
/// The ENTRY line is the one exception to "an active line runs to the right edge": once the wire
/// dates the entry's fill it ends there instead, marked with the closed-trade-history arrow rather
/// than an end cross. A still-OPEN order then continues from that arrow to the edge in
/// [`POSITION_HOLD_RGB`], because the position is still held and the entry line alone would have
/// made it disappear. Every other line kind keeps running to the edge, because the order is still
/// working after a fill.
#[allow(clippy::too_many_arguments)]
pub fn build_order_geometry(
    store: &OrderLineStore,
    market: &str,
    style: &OrdersStyle,
    graphics: &ChartGraphicsCfg,
    // Device pixels per logical pixel. Marker half-extents are PHYSICAL px, so the fill arrow
    // needs it for the same reason a trade-history arrow does.
    scale: f32,
    highlight_uid: Option<u64>,
    drag_preview: Option<(u64, LineKind, f32)>,
    epoch_ms: f64,
    now_ms: f64,
    left_rel: f32,
    right_rel: f32,
    edge_rel: f32,
    // Whether a MoonShot order fills its corridor. Per-chart, from `CandleViewCfg::moonshot_zone`:
    // the corridor spans the whole pane width and is the loudest thing on a chart watching a
    // MoonShot, so it is the one zone the user can turn off. The panic-sell fill below is NOT
    // covered by it — that one marks an order about to be dumped and stays unconditional.
    show_moonshot_zone: bool,
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
        // Cull by the time window (relative milliseconds). A line can begin BEFORE the order's own
        // `create_ms` — the core dated the order itself not at all and it fell back to the local
        // clock — so a right-side cull is confirmed against the true earliest time. That walk over
        // the order's lines is paid only by the orders this test would otherwise drop.
        let start_rel = to_rel(ord.create_ms);
        let end_rel = to_rel(order_end);
        if end_rel < left_rel || (start_rel > right_rel && to_rel(ord.earliest_ms()) > right_rel) {
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
                    zones.push(ZoneInstance::full_width(a.min(b), a.max(b), color));
                }
            };
            if ord.is_moon_shot && show_moonshot_zone {
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
            // A closed order's sell line is the pale stripe left behind after the exit filled: it
            // reads as a price the terminal is still tracking when nothing is tracking it any more.
            // Hidden by default, and only for CLOSED orders — a live order always keeps its exit.
            if closed && graphics.hide_closed_sell_line && idx == LineKind::Sell as usize {
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
            // The ENTRY leg stops existing forward in time the moment it fills, so its line must
            // not run on to the pane edge pretending the order is still working there. Only the
            // entry (Buy, for a long and a short alike) is affected: the order itself lives on
            // after a fill — it is repriced, it trails, it has TP/SL — so every other kind keeps
            // `fill_ms` at `None` by construction and its geometry is untouched. A `None` here is
            // also what an order the core never dated a fill for gets, and that one keeps drawing
            // exactly as before rather than being marked at a guessed position.
            let fill_ms = if idx == LineKind::Buy as usize {
                ord.entry_fill_ms
            } else {
                None
            };
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
            // `ended` deliberately keeps its old meaning: disabled or order-closed. A live filled
            // entry therefore still retains its server-trace temporary point below; `stopped`
            // widens only the straight-line drawing end. An active (unfinished) line has NO END:
            // it extends to the plot's right edge (through the order book), without an end cross.
            // A finished line ends at its disable/close time.
            let line_end = line.off_ms.unwrap_or(order_end);

            let start_t = points[0].0;
            // The current price is the last live step. The main line is STRAIGHT at the current
            // price from start to end (the entire line moves when the order is repriced).
            let cur_p = preview_price.unwrap_or(points[n - 1].1);
            let t0_rel = to_rel(start_t);
            // Where the entry line actually ENDS: at the fill once there is one, otherwise where it
            // ended before. The fill is bounded by the line's own lifetime — below by its first
            // step, above by its end — so a wire fill outside that span degenerates the segment to
            // a point instead of inverting it into a smear. The upper bound is itself raised to
            // `start_t` first: `wire_line_start` may hand back a start LATER than `now_ms` when the
            // local clock steps back, and bounding above by a `line_end` already below the start
            // would place the end before the beginning — max-then-min alone does not prevent that,
            // it only survives it. `clamp` is avoided for the same reason it is at `wire_line_start`
            // — it panics when the bounds cross.
            let line_end_eff = match fill_ms {
                Some(t) => t.max(start_t).min(line_end.max(start_t)),
                None => line_end,
            };
            let fill_rel = fill_ms.map(|_| to_rel(line_end_eff));
            // A filled entry line is finished for drawing purposes even while the order is live.
            let stopped = ended || fill_rel.is_some();
            // An active line extends to the right edge (edge_rel, through the order book); a
            // finished line extends to its own end time, and a filled entry to its fill. The fill
            // wins over a later closure by the `min` above: closure is a fact about the ORDER,
            // while the entry LEG ceased at the fill and the closed order keeps its Buy line.
            let t1_rel = match fill_rel {
                Some(rel) => rel,
                None if ended => to_rel(line_end),
                None => edge_rel,
            };

            if !has_server_trace && path.show && n > 1 {
                for i in 0..n {
                    let (t, p) = points[i];
                    // The repricing staircase is the ENTRY line's own history, so it stops where
                    // that line stops. Bounding it by `line_end_eff` rather than `line_end` is what
                    // keeps a filled order's path from walking on past the fill arrow — this path
                    // is drawn by default (`PathStyle::show`), so leaving it out would have shown
                    // the very thing the fill mark exists to deny, one line lower on the chart.
                    let seg_end_t = if i + 1 < n {
                        points[i + 1].0.min(line_end_eff)
                    } else {
                        line_end_eff
                    };
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
                    // The riser to the next price is drawn only where that reprice actually falls
                    // INSIDE the line's life. Testing the raw step time rather than the clamped
                    // `seg_end_t` is the point: every reprice after the fill clamps to the same
                    // instant, so testing the clamped value would stack a column of risers on the
                    // fill mark instead of drawing none.
                    if i + 1 < n && points[i + 1].0 <= line_end_eff {
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
                extend: if stopped {
                    SEG_EXTEND_NONE
                } else {
                    SEG_EXTEND_EDGE
                },
                color: col,
            });

            // "Entered here, still holding." The entry line stops dead at the fill arrow, which is
            // correct — the entry LEG ended there — but on its own it deletes an open position from
            // the chart entirely, and the user then cannot see that money is still on the table.
            // This picks the price up at the arrow and carries it to the pane edge in a colour the
            // entry line never uses, and it ENDS when the order closes, since `closed` is the one
            // fact that says the position is no longer held. Entry only: `fill_rel` is `None` for
            // every other kind by construction (see `fill_ms` above), and the explicit `idx` test
            // keeps it that way should that ever change, so Sell/Stop/Trailing/TakeProfit/VStop are
            // untouched. `SEG_EXTEND_EDGE` is how a live line already reaches the edge, so this one
            // follows the pane's own edge uniform rather than a `t1_rel` computed here.
            if idx == LineKind::Buy as usize && fill_rel.is_some() && !closed {
                segs.push(SegInstance {
                    t0_rel: t1_rel,
                    p0: cur_p,
                    t1_rel: edge_rel,
                    p1: cur_p,
                    thickness,
                    pattern: SEG_PATTERN_SOLID,
                    extend: SEG_EXTEND_EDGE,
                    color: rgba(POSITION_HOLD_RGB, line_alpha),
                });
            }

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
            // `t1_rel` IS the fill here — the match above returns `fill_rel` unchanged whenever it
            // is set — so the marker sits at the same end the segment does, named once. The cross
            // branch below reads `t1_rel` for the same reason.
            if fill_rel.is_some() {
                // A fill is an EVENT, not the decorative terminator `end_marker` governs, so it is
                // drawn whatever that flag says: with the flag off the entry line would simply stop
                // dead with nothing on the chart saying why. It REPLACES the cross rather than
                // joining it — two glyphs on one endpoint is noise, and the arrow already says
                // "ended, and here is how". Direction follows the ACTION, as in trade history: a
                // long's entry is a buy (up), a short's entry is a sell (down). The half-extents
                // are the resting values the shader grows itself, so a count of 1 draws them as-is.
                //
                // Scaled EXACTLY as `build_trade_geometry` scales its own arrows — device scale
                // times the clamped user multiplier — because `MarkerInstance::size`/`thickness`
                // are PHYSICAL px while `ARROW_HALF_*` are logical, and the whole point of reusing
                // the trade-history glyph is that the two read as one vocabulary at any DPI and any
                // setting. The surrounding order-line markers (start/end cross, knots) are still
                // unscaled — they carry `st.marker_size` from `OrdersStyle`, a separate and wider
                // inconsistency deliberately left alone here.
                let arrow_scale = clamp_arrow_scale(graphics.trade_arrow_scale);
                markers.push(MarkerInstance::arrow(
                    t1_rel,
                    cur_p,
                    ARROW_HALF_H * scale * arrow_scale,
                    ARROW_HALF_W * scale * arrow_scale,
                    if ord.is_short {
                        MARKER_SHAPE_ARROW_DOWN
                    } else {
                        MARKER_SHAPE_ARROW_UP
                    },
                    1,
                    col,
                ));
            } else if st.end_marker && ended {
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
