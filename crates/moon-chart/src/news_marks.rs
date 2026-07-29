//! Geometry and hit-testing for NEWS MARKS: the small faceted gem the chart puts on its bottom edge
//! at the moment each news item for this coin was published (Moonbot parity).
//!
//! A mark belongs to a MOMENT, not to a price, so it is anchored to the plot's bottom edge in screen
//! space ([`MARKER_ANCHOR_BOTTOM`]) and only its X comes from chart time. That keeps the row of marks
//! pinned to the axis while the Y range auto-fits, and it means panning/zooming never rebuilds this
//! geometry — the shader re-maps time→x every frame.
//!
//! COLOUR carries the news tags: a mark is split into as many wedges as the item has DISTINCT tag
//! colours (see [`NewsMark::new`]), so one glance says "listing" or "hack" without opening anything.
//! Each wedge is its own instance drawing the same gem and discarding every fragment outside its
//! angular slice — the marker instance has room for one colour, and up to four wedges is far cheaper
//! than a second layer with a wider vertex format.
//!
//! Sizes are in LOGICAL pixels: the caller multiplies by the device pixel ratio so a mark stays the
//! same visual size on a HiDPI screen, exactly like the chart's other markers.

use crate::layers::{MarkerInstance, MARKER_ANCHOR_BOTTOM};

/// Half-height of the gem in logical pixels; it is taller than wide, which reads as a marker rather
/// than as a trade dot.
pub const MARK_HALF_H: f32 = 10.0;
/// Half-width of the gem in logical pixels.
pub const MARK_HALF_W: f32 = 6.5;
/// How much the gem under the cursor grows. It grows UPWARD only — see [`mark_center_offset`].
pub const MARK_HOVER_SCALE: f32 = 1.15;

/// Distance from the plot's bottom edge to the gem's CENTER, in logical pixels.
///
/// Always equal to the gem's CURRENT half height, which is what pins its lower tip to the bottom
/// edge: growing on hover then only extends it upward instead of pushing it off the axis.
pub fn mark_center_offset(hovered: bool) -> f32 {
    MARK_HALF_H * if hovered { MARK_HOVER_SCALE } else { 1.0 }
}
/// Most wedges one mark can carry — the tag palette holds exactly four distinct colours.
pub const MAX_MARK_COLORS: usize = 4;
/// Opacity of a mark. Slightly translucent so a mark never hides the volume bars underneath.
const MARK_ALPHA: f32 = 0.88;
/// Shader shape id for the news gem (a vertically elongated diamond).
pub const SHAPE_GEM: f32 = 2.0;
/// Shader shape id for a warning badge (an upward triangle with an exclamation mark).
pub const SHAPE_WARN: f32 = 3.0;
/// Extra pixels around a mark that still count as a hit, so a small gem stays comfortable to hit.
const HIT_SLACK: f32 = 3.0;

/// One news mark: when it happened, and the colours its tags give it.
///
/// Colours are sRGB `0xRRGGBB`, already resolved from the user's per-tag palette by the UI layer —
/// this crate never learns what a tag is. An EMPTY colour set means the item carries no coloured
/// tag, and the caller's neutral chart colour is used instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct NewsMark {
    /// Mark time, Unix ms.
    pub time_ms: i64,
    colors: [u32; MAX_MARK_COLORS],
    count: u8,
}

impl NewsMark {
    /// Build a mark from its time and its tag colours, keeping the first [`MAX_MARK_COLORS`]
    /// DISTINCT ones in the order given.
    ///
    /// Deduplication is the whole point: two tags sharing one palette colour must not split the gem
    /// into two identical halves, and an item whose only other tag is colourless stays a single
    /// solid gem.
    pub fn new(time_ms: i64, colors: impl IntoIterator<Item = u32>) -> Self {
        let mut out = [0u32; MAX_MARK_COLORS];
        let mut count = 0usize;
        for c in colors {
            if count == MAX_MARK_COLORS {
                break;
            }
            if out[..count].contains(&c) {
                continue;
            }
            out[count] = c;
            count += 1;
        }
        Self {
            time_ms,
            colors: out,
            count: count as u8,
        }
    }

    /// The distinct tag colours, empty when the item has no coloured tag.
    pub fn colors(&self) -> &[u32] {
        &self.colors[..self.count as usize]
    }
}

fn rgba(color: u32, alpha: f32) -> [f32; 4] {
    [
        ((color >> 16) & 0xFF) as f32 / 255.0,
        ((color >> 8) & 0xFF) as f32 / 255.0,
        (color & 0xFF) as f32 / 255.0,
        alpha,
    ]
}

/// Appends the gem geometry for `marks` (one instance per wedge).
///
/// `epoch_ms` is the pane's time epoch, `hovered` the index of the mark under the cursor (it grows
/// upward from the axis and turns fully opaque), `fallback` the chart's neutral sRGB colour for marks
/// whose tags carry none, and `scale` the device pixel ratio.
pub fn build_news_geometry(
    marks: &[NewsMark],
    epoch_ms: f64,
    hovered: Option<usize>,
    fallback: [u8; 3],
    scale: f32,
    shape: f32,
    markers: &mut Vec<MarkerInstance>,
) {
    let scale = scale.max(0.1);
    let fallback = ((fallback[0] as u32) << 16) | ((fallback[1] as u32) << 8) | fallback[2] as u32;
    // One instance PER WEDGE, so a multi-colour set must not realloc mid-loop.
    markers.reserve(marks.iter().map(|m| m.colors().len().max(1)).sum());
    for (i, mark) in marks.iter().enumerate() {
        let hot = hovered == Some(i);
        let grow = if hot { MARK_HOVER_SCALE } else { 1.0 };
        let half_h = MARK_HALF_H * grow * scale;
        let colors = mark.colors();
        let count = colors.len().max(1);
        for sector in 0..count {
            let color = colors.get(sector).copied().unwrap_or(fallback);
            markers.push(MarkerInstance {
                t_rel: (mark.time_ms as f64 - epoch_ms) as f32,
                // Under MARKER_ANCHOR_BOTTOM this is the distance above the plot's bottom edge, and
                // it tracks the half height so the gem's lower tip stays ON the axis as it grows.
                price: half_h,
                size: half_h,
                // The gem reads `thickness` as its half WIDTH, which is what makes it vertical.
                thickness: MARK_HALF_W * grow * scale,
                shape,
                anchor: MARKER_ANCHOR_BOTTOM,
                sector: sector as f32,
                sectors: count as f32,
                color: rgba(color, if hot { 1.0 } else { MARK_ALPHA }),
            });
        }
    }
}

/// Horizontal reach of a mark's hit area from its center, in device pixels.
///
/// Measured against the RESTING size: a hovered gem grows, but the cursor that is already on it must
/// not change what counts as a hit, or the mark would flicker on its own growth.
fn hit_reach_x(scale: f32) -> f32 {
    (MARK_HALF_W + HIT_SLACK) * scale.max(0.1)
}

/// Vertical reach of a mark's hit area from its center, in device pixels.
fn hit_reach_y(scale: f32) -> f32 {
    (MARK_HALF_H + HIT_SLACK) * scale.max(0.1)
}

/// Whether `cursor_y` is inside the marks' row along the plot's bottom edge (device pixels).
///
/// The Y test is separate from [`hit_marks`] on purpose: it is the cheap gate a pointer move pays
/// before anything scans the marks, and it fails for everything above the row.
pub fn in_mark_row(cursor_y: f32, plot_bottom: f32, scale: f32) -> bool {
    let center_y = plot_bottom - mark_center_offset(false) * scale.max(0.1);
    (cursor_y - center_y).abs() <= hit_reach_y(scale)
}

/// The marks `cursor_x` touches: the nearest one plus the whole stack it belongs to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MarkHit {
    /// Index of the mark closest to the cursor — the one the chart grows.
    pub nearest: usize,
    /// Every touched mark, in the order `xs` came in (chronological), which is the order the hover
    /// card lists them in. Drifting a pixel inside a cluster must not reorder this.
    pub stack: Vec<usize>,
}

/// Hit-test the marks' row: which gems does `cursor_x` touch?
///
/// Marks stack when several news land within seconds of each other, so the whole stack comes back
/// rather than just the closest one — the hover card then shows all of them instead of silently
/// hiding the ones underneath. `xs` are the marks' centers in the panel's device pixels, in the same
/// order as the marks; the caller has already checked [`in_mark_row`]. ONE pass and no sorting: this
/// runs on the pointer path.
pub fn hit_marks(xs: impl IntoIterator<Item = f32>, cursor_x: f32, scale: f32) -> Option<MarkHit> {
    let reach = hit_reach_x(scale);
    let mut stack: Vec<usize> = Vec::new();
    let mut nearest = 0usize;
    let mut best = f32::INFINITY;
    for (i, x) in xs.into_iter().enumerate() {
        let dx = (cursor_x - x).abs();
        if dx > reach {
            continue;
        }
        if dx < best {
            best = dx;
            nearest = i;
        }
        stack.push(i);
    }
    (!stack.is_empty()).then_some(MarkHit { nearest, stack })
}

#[cfg(test)]
mod tests;
