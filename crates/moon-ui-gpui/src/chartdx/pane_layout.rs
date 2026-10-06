//! Shared pane, order-book and horizontal-volume area layout.

use super::*;

/// A pane's areas as ONE layout decides them.
///
/// `prepare` draws with these, and the input and geometry paths hit-test against the same call so
/// they cannot answer for a layout that was never drawn; the book-only broom mode, where the book
/// takes the whole pane, is the case that punished a second copy of the arithmetic hardest.
#[derive(Clone, Copy)]
pub(crate) struct PaneAreas {
    /// Effective axis position: broom mode hides the price axis whatever the tab configured.
    pub axis_pos: crate::persistence::chart_persist::PriceAxisPos,
    /// The plot area. Its width is floored at one pixel, so an unpresented slot and a broom pane
    /// both report a plot that exists but holds nothing.
    pub plot: Rect,
    /// The order book's area, `w == 0.0` when no book is drawn.
    pub glass: Rect,
    /// The horizontal volumes' zone, `w == 0.0` when none is drawn: the tab has them off, the
    /// pane is too narrow to hold one, or the broom took the pane.
    pub hvol: Rect,
}

/// Order-book layout inputs shared by rendering and hit testing.
#[derive(Clone, Copy)]
pub(crate) struct BookLayout {
    /// Broom mode gives the book the entire pane.
    pub only: bool,
    /// Whether the ordinary book is drawn.
    pub enabled: bool,
    /// App-wide width in physical pixels, before the automatic pane limits.
    pub width_px: f32,
}

/// Lay one pane out into its horizontal-volume, plot and order-book areas.
///
/// Left to right: `[hvol]` `[axis]` plot `[book]` `[axis]` — the horizontal-volume zone sits at
/// the pane's LEFT edge, outboard of a left axis gutter, as the reference draws it; with a RIGHT
/// axis the book follows the plot and the axis gutter stays outboard of it.
///
/// Args:
///     rect: The pane's full rectangle in device pixels.
///     book: App-wide book width and the pane's book/broom flags.
///     time_axis_visible: Whether the time axis reserves its gutter under every area.
///     price_axis_pos: Configured per-tab price-axis position.
///     hvol: The horizontal volumes' width, `None` when they are off.
///     pixel_scale: Device pixels per chart-design pixel (the platform factor, excluding UI zoom).
///
/// Returns:
///     The pane's [`PaneAreas`].
pub(crate) fn pane_layout(
    rect: Rect,
    book: BookLayout,
    time_axis_visible: bool,
    price_axis_pos: crate::persistence::chart_persist::PriceAxisPos,
    hvol: Option<moon_chart::hvol::HvolZoneSpec>,
    pixel_scale: f32,
) -> PaneAreas {
    use crate::persistence::chart_persist::PriceAxisPos;
    let axis_pos = if book.only {
        PriceAxisPos::Hide
    } else {
        price_axis_pos
    };
    let price_axis_w = if matches!(axis_pos, PriceAxisPos::Hide) {
        0.0
    } else {
        moon_chart::PRICE_AXIS_W * pixel_scale
    };
    let book_width = moon_core::config::book_width::normalize(book.width_px);
    let glass_cap = rect.w * 0.5;
    let glass_base = book_width.min(glass_cap);
    let chart_w_base = rect.w - price_axis_w - glass_base;
    let glass_w = if book.only {
        (rect.w - price_axis_w).max(1.0)
    } else if !book.enabled {
        0.0
    } else if chart_w_base < glass_base * 2.0 {
        (book_width * 0.8).min(glass_cap)
    } else {
        glass_base
    };
    // The zone takes its share of the PANE, not of what the book leaves: the reader sized it
    // against the pane, and a book toggle must not resize it. Too narrow to show a row — a
    // cramped slot in a stack — and it is left out rather than drawn as a sliver; the broom
    // owns the whole pane. Laid over the plot, the zone takes nothing from it, so only a carved
    // zone narrows the plot here; the overlaid one is sized below, once the plot is known.
    let hvol_floor = moon_chart::hvol::ZONE_MIN_PX * pixel_scale;
    let hvol_asked = match hvol {
        Some(spec) if !book.only => Some(((rect.w * spec.width_frac).round(), spec.overlay)),
        _ => None,
    };
    let hvol_overlay = hvol_asked.is_some_and(|(_, overlay)| overlay);
    let hvol_carved_w = match hvol_asked {
        Some((w, false)) if w >= hvol_floor => w,
        _ => 0.0,
    };
    let chart_w = (rect.w - price_axis_w - glass_w - hvol_carved_w).max(1.0);
    // Left puts the axis gutter on the left and shifts the plot right; Right and Hide start the
    // plot at the zone's edge. The zone sits outboard of the axis gutter.
    let chart_x = if matches!(axis_pos, PriceAxisPos::Left) {
        rect.x + hvol_carved_w + price_axis_w
    } else {
        rect.x + hvol_carved_w
    };
    // The book follows the plot only for a right-side axis, which leaves that gutter outboard of
    // it; otherwise it sits against the pane's right edge.
    let glass_x = if matches!(axis_pos, PriceAxisPos::Right) {
        chart_x + chart_w
    } else {
        rect.x + (rect.w - glass_w).max(0.0)
    };
    // A hidden time axis reserves no label gutter, letting every area use the full height.
    let time_axis_h = if time_axis_visible {
        moon_chart::TIME_AXIS_H * pixel_scale
    } else {
        0.0
    };
    let h = (rect.h - time_axis_h).max(1.0);
    PaneAreas {
        axis_pos,
        plot: Rect {
            x: chart_x,
            y: rect.y,
            w: chart_w,
            h,
        },
        glass: Rect {
            x: glass_x,
            y: rect.y,
            w: glass_w,
            h,
        },
        // Carved, the zone sits outboard of everything at the pane's left edge; laid over, it is
        // the plot's own left strip, never wider than the plot — which is the one case a book
        // toggle CAN resize it, a plot narrower than the strip — and floored AFTER that clamp, so
        // a plot cramped to a few pixels leaves the zone out rather than drawing the sliver the
        // pane-relative check above had let through.
        hvol: Rect {
            x: if hvol_overlay { chart_x } else { rect.x },
            y: rect.y,
            w: match hvol_asked {
                Some((w, true)) => {
                    let w = w.min(chart_w);
                    if w >= hvol_floor { w } else { 0.0 }
                }
                _ => hvol_carved_w,
            },
            h,
        },
    }
}
