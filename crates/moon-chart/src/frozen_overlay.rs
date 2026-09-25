//! What a frozen viewer draws beside its archived order lines: the entry corridor the core saved
//! for the trade, and trades a MODEL says a variant of the strategy would have made — with the
//! path its entry order walked and, as bands, the corridor around it.
//!
//! Neither is an order of the trade, so neither goes through the order store: the store draws only
//! in the "Moonbot lines" trade style, and both of these are asked for whatever the style is. They
//! are drawn from the orders style all the same — the corridor in the take-profit tint a live
//! MoonShot corridor takes, a modelled trade in the entry and exit colours of an order — so the
//! picture speaks one vocabulary; what sets a modelled trade apart is its pen (dashed or dotted),
//! never a colour of its own.

use moon_core::config::{ChartGraphicsCfg, OrdersStyle};

use crate::layers::rgb_with_alpha as rgba;
use crate::layers::{
    MARKER_SHAPE_ARROW_DOWN, MARKER_SHAPE_ARROW_UP, MarkerInstance, SEG_CLAMP_NONE,
    SEG_EXTEND_NONE, SegInstance, ZoneInstance,
};
use crate::order_geometry::MB_MOONSHOT_ZONE_ALPHA;
use crate::trade_marks::{ARROW_HALF_H, ARROW_HALF_W, clamp_arrow_scale};

/// A price band shaded between two instants — the entry corridor from the order's placement to
/// its fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayBand {
    /// Band start, Unix UTC ms.
    pub from_ms: f64,
    /// Band end, Unix UTC ms.
    pub to_ms: f64,
    /// The two prices, in either order.
    pub prices: (f32, f32),
}

/// One modelled trade: where the entry filled and, when the model closed it, where it exited.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayTrade {
    /// The entry order's path as the model walked it — `(Unix UTC ms, level)` of each placement,
    /// ending at the fill; empty when the model walks no path (a shift, a kind without an entry
    /// model), and then only the fill is drawn.
    pub path: Vec<(f64, f32)>,
    /// Entry fill instant, Unix UTC ms.
    pub fill_ms: f64,
    /// Entry fill price.
    pub fill_price: f32,
    /// Exit instant and price; `None` when the position was still open where the tape ends.
    pub exit: Option<(f64, f32)>,
    /// The sell order's path as the model walked it — `(Unix UTC ms, level)` of each placement
    /// from the fill, stepped to the exit like the entry path; empty draws the exit line flat at
    /// the exit price, as a Moonbot line pair does.
    pub exit_path: Vec<(f64, f32)>,
    /// Whether the position is short, which picks the short styles and the arrows' direction.
    pub is_short: bool,
    /// Pen of the entry path, the exit line and the connector (`SEG_PATTERN_*`) — what tells two
    /// modelled trades, and a modelled trade from the fact, apart.
    pub pattern: f32,
}

/// Everything a frozen viewer draws beside its store.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrozenOverlay {
    pub bands: Vec<OverlayBand>,
    pub trades: Vec<OverlayTrade>,
}

impl FrozenOverlay {
    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty() && self.trades.is_empty()
    }
}

/// Opacity of a modelled trade's connector: a guide between its two ends, quieter than the lines.
const CONNECTOR_ALPHA: f32 = 0.6;

/// Append the overlay's geometry to buffers the order pass already filled.
///
/// Appends, unlike `build_order_geometry`, which clears: the overlay is drawn beside the orders,
/// never instead of them.
///
/// Args:
///     overlay: What to draw.
///     style: The orders style — the corridor tint, the entry and exit colours and widths.
///     graphics: The tab's graphics, for the arrow scale.
///     scale: Device pixels per logical pixel; arrow extents are physical.
///     epoch_ms: The pane's epoch; every instance is relative to it.
///     zones: Zone buffer.
///     segs: Segment buffer.
///     markers: Marker buffer.
#[allow(clippy::too_many_arguments)]
pub fn build_overlay_geometry(
    overlay: &FrozenOverlay,
    style: &OrdersStyle,
    graphics: &ChartGraphicsCfg,
    scale: f32,
    epoch_ms: f64,
    zones: &mut Vec<ZoneInstance>,
    segs: &mut Vec<SegInstance>,
    markers: &mut Vec<MarkerInstance>,
) {
    let to_rel = |t_ms: f64| (t_ms - epoch_ms) as f32;
    let price_ok = |p: f32| p.is_finite() && p > 0.0;
    for band in &overlay.bands {
        let (a, b) = band.prices;
        // Exactly flat is dropped, but no epsilon above that: an absolute one discards a real
        // band on a market priced at 1e-8 (the same rule `figures` keeps).
        if !price_ok(a) || !price_ok(b) || a == b || band.to_ms < band.from_ms {
            continue;
        }
        zones.push(ZoneInstance {
            price0: a.min(b),
            price1: a.max(b),
            t0_rel: to_rel(band.from_ms),
            t1_rel: to_rel(band.to_ms),
            color: rgba(style.take_profit.color, MB_MOONSHOT_ZONE_ALPHA),
        });
    }
    let arrow_scale = clamp_arrow_scale(graphics.trade_arrow_scale);
    let arrow = |t_ms: f64, price: f32, up: bool, color: [f32; 4]| {
        MarkerInstance::arrow(
            to_rel(t_ms),
            price,
            ARROW_HALF_H * scale * arrow_scale,
            ARROW_HALF_W * scale * arrow_scale,
            if up {
                MARKER_SHAPE_ARROW_UP
            } else {
                MARKER_SHAPE_ARROW_DOWN
            },
            1,
            color,
        )
    };
    for trade in &overlay.trades {
        if !price_ok(trade.fill_price) {
            continue;
        }
        let (entry_style, exit_style) = if trade.is_short {
            (&style.buy_short, &style.sell_short)
        } else {
            (&style.buy, &style.sell)
        };
        // The entry line the model walked, to the fill.
        let entry_color = rgba(entry_style.color, 1.0);
        push_stepped(
            segs,
            &trade.path,
            Pen {
                end_ms: trade.fill_ms,
                thickness: entry_style.thickness,
                pattern: trade.pattern,
                color: entry_color,
                epoch_ms,
            },
        );
        // Direction follows the ACTION, as trade history's arrows do: a long enters with a buy.
        markers.push(arrow(
            trade.fill_ms,
            trade.fill_price,
            !trade.is_short,
            entry_color,
        ));
        let Some((exit_ms, exit_price)) = trade.exit.filter(|(_, p)| price_ok(*p)) else {
            continue;
        };
        let exit_color = rgba(exit_style.color, 1.0);
        let exit_end_ms = exit_ms.max(trade.fill_ms);
        // A path none of whose levels stood before the close — a stop inside `SellDelay`, before
        // the sell was ever placed, or on the very ms it was — draws as a path without one would.
        let path_drawn = trade
            .exit_path
            .iter()
            .any(|&(t_ms, level)| t_ms < exit_end_ms && price_ok(level));
        if !path_drawn {
            // The exit line as a Moonbot line pair draws it: at the exit price, from the fill
            // (where the exit order is placed) to the close.
            segs.push(SegInstance {
                t0_rel: to_rel(trade.fill_ms),
                p0: exit_price,
                t1_rel: to_rel(exit_end_ms),
                p1: exit_price,
                thickness: exit_style.thickness,
                pattern: trade.pattern,
                extend: SEG_EXTEND_NONE,
                clamp: SEG_CLAMP_NONE,
                color: exit_color,
            });
        } else {
            // The sell order the model walked, from its placement to the close — a stop's exit
            // lands off it, at the stop's own price.
            push_stepped(
                segs,
                &trade.exit_path,
                Pen {
                    end_ms: exit_end_ms,
                    thickness: exit_style.thickness,
                    pattern: trade.pattern,
                    color: exit_color,
                    epoch_ms,
                },
            );
        }
        segs.push(SegInstance {
            t0_rel: to_rel(trade.fill_ms),
            p0: trade.fill_price,
            t1_rel: to_rel(exit_ms.max(trade.fill_ms)),
            p1: exit_price,
            thickness: 1.0,
            pattern: trade.pattern,
            extend: SEG_EXTEND_NONE,
            clamp: SEG_CLAMP_NONE,
            color: rgba(exit_style.color, CONNECTOR_ALPHA),
        });
        markers.push(arrow(exit_ms, exit_price, trade.is_short, exit_color));
    }
}

/// How [`push_stepped`] draws one path.
struct Pen {
    /// Nothing is drawn past this instant, Unix UTC ms.
    end_ms: f64,
    thickness: f32,
    pattern: f32,
    color: [f32; 4],
    /// The pane's epoch; every instance is relative to it.
    epoch_ms: f64,
}

/// A path the model walked, stepped like a repriced order's: a level per placement to the next
/// one, a riser at each move, nothing past the pen's end.
fn push_stepped(segs: &mut Vec<SegInstance>, path: &[(f64, f32)], pen: Pen) {
    let to_rel = |t_ms: f64| (t_ms - pen.epoch_ms) as f32;
    let price_ok = |p: f32| p.is_finite() && p > 0.0;
    for (i, &(t_ms, level)) in path.iter().enumerate() {
        if !price_ok(level) || t_ms > pen.end_ms {
            continue;
        }
        let next = path.get(i + 1);
        let to_ms = next
            .map_or(pen.end_ms, |(next_ms, _)| *next_ms)
            .min(pen.end_ms)
            .max(t_ms);
        segs.push(SegInstance {
            t0_rel: to_rel(t_ms),
            p0: level,
            t1_rel: to_rel(to_ms),
            p1: level,
            thickness: pen.thickness,
            pattern: pen.pattern,
            extend: SEG_EXTEND_NONE,
            clamp: SEG_CLAMP_NONE,
            color: pen.color,
        });
        let riser = next.filter(|(next_ms, next)| *next_ms <= pen.end_ms && price_ok(*next));
        if let Some(&(next_ms, next)) = riser {
            segs.push(SegInstance {
                t0_rel: to_rel(next_ms),
                p0: level,
                t1_rel: to_rel(next_ms),
                p1: next,
                thickness: 1.0,
                pattern: pen.pattern,
                extend: SEG_EXTEND_NONE,
                clamp: SEG_CLAMP_NONE,
                color: pen.color,
            });
        }
    }
}

#[cfg(test)]
mod tests;
