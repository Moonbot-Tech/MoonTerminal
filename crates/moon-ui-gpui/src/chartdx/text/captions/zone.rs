use super::model::*;
use super::*;

/// The one band of a zone whose width has to be decided from the others, or `None`.
///
/// `None` for a zone with no elastic band, for one where nothing else is drawn — there is nothing
/// to divide against — and for one holding TWO of them: dividing those against each other leaves
/// the loser under `MIN_LEGIBLE_W`, where a band is dropped rather than truncated. Nothing ships
/// two; the detect module is the only prose there is.
pub(super) fn elastic_band(bands: &[Band; 3]) -> Option<&Band> {
    if bands.iter().filter(|band| !band.is_empty()).count() < 2 {
        return None;
    }
    let mut elastic = bands.iter().filter(|band| band.elastic);
    match (elastic.next(), elastic.next()) {
        (Some(band), None) => Some(band),
        _ => None,
    }
}

/// Left and right edge of one zone, in logical pixels.
///
/// THE one place a zone's bounds are stated: the anchor and the width budget are both read off it,
/// and two copies of this arithmetic would be free to disagree about where a band ends. The control
/// strip is bounded by ITS OWN edges, never the plot's, or a caption there would run out over the
/// candles; its right edge is already inset clear of the pane's close button by `caption_geom`.
pub(super) fn zone_bounds(
    zone: LabelZone,
    geom: &CaptionGeomInput,
    corner: &CaptionGeom,
) -> (f32, f32) {
    if zone.is_control_zone() {
        return (corner.zone_left, corner.right_x);
    }
    // The plot's own edges, EXCEPT that the top band also clears the pane's close button. That
    // button sits in the pane's top-right corner and is drawn over whatever is beneath it, so a
    // right-aligned caption on this band hides under it — `ZONE_PAD` is six pixels and the button
    // takes twenty-six. `corner.right_x` is the same inset the control strip already keeps
    // (`caption_geom`), and taking the SMALLER of the two changes nothing while a book is drawn:
    // the plot ends well before the strip does. It matters exactly where the book is off and the
    // plot runs to the pane's edge — the trade-detail window, which draws no book at all.
    let right = match zone {
        LabelZone::ChartTop => (geom.plot_right - ZONE_PAD).min(corner.right_x),
        _ => geom.plot_right - ZONE_PAD,
    };
    (geom.plot_left + ZONE_PAD, right)
}

/// Width the bands of one zone share, in logical pixels.
pub(super) fn zone_width(zone: LabelZone, geom: &CaptionGeomInput, corner: &CaptionGeom) -> f32 {
    let (left, right) = zone_bounds(zone, geom, corner);
    (right - left).max(0.0)
}

/// Where one band anchors, and at which fraction of its own width.
///
/// A left-aligned plot band clears a left price-axis gutter by sixteen logical pixels, which the
/// caption pass scales with the rest of its geometry. Every other zone and alignment keeps the
/// ordinary zone inset.
///
/// Only the anchor: what a band may SPEND is what its neighbours in the same zone left it, which
/// only `draw_all_zones` knows.
pub(super) fn zone_anchor(
    zone: LabelZone,
    align: LabelAlign,
    geom: &CaptionGeomInput,
    corner: &CaptionGeom,
) -> (f32, f32) {
    // Each band knows its own edges; the alignment picks which of them the row anchors to.
    let (left, right) = zone_bounds(zone, geom, corner);
    let has_left_axis_gutter = geom.plot_left - geom.pane_left > 0.5 / geom.scale_factor.max(0.1);
    let x = match align {
        LabelAlign::Left if !zone.is_control_zone() && has_left_axis_gutter => {
            geom.plot_left + 16.0
        }
        LabelAlign::Left => left,
        LabelAlign::Center => (left + right) * 0.5,
        LabelAlign::Right => right,
    };
    (x, align.fraction())
}

/// Y of a zone's first row: below the plot's top edge, or above its bottom one.
pub(super) fn zone_start_y(
    zone: LabelZone,
    align: LabelAlign,
    geom: &CaptionGeomInput,
    corner: &CaptionGeom,
) -> f32 {
    if zone.is_top() {
        match zone {
            // The control strip keeps the inset its own geometry resolved, which clears the pane's
            // close button; the plot's corners only clear the plot edge.
            LabelZone::ZoneTop => corner.top_y,
            // The plot's top edge, plus the corner strip where one stands. Mirror of the right-edge
            // inset in `zone_bounds`: there a right-aligned caption clears the pane's close button,
            // here a LEFT-aligned one clears the pin/lock/broom strip. Left-aligned ChartTop only —
            // the strip occupies that one corner, and the control strip resolves its own inset.
            _ => {
                geom.plot_top
                    + ZONE_PAD
                    + match (zone, align) {
                        (LabelZone::ChartTop, LabelAlign::Left) => {
                            // Never push a caption OUT of the plot to dodge a button. `draw_stack` draws a band's
                            // FIRST line even when it overflows — a pane can be shorter than one line of text — so
                            // on a compressed stack slot this reservation would move that line past `plot_bottom`
                            // and print it over the neighbour below. Spend the strip only where the room below the
                            // pad can hold it AND the tallest line a caption can be: the caption size is a user
                            // setting, so the strip's own height is no guide to it. Where it cannot, the old
                            // button-over-caption overlap is the lesser glitch.
                            let strip = geom.corner_strip_h.max(0.0);
                            let room = geom.plot_bottom - geom.plot_top - ZONE_PAD;
                            match room >= strip + MAX_CAPTION_LINE_H {
                                true => strip,
                                false => 0.0,
                            }
                        }
                        _ => 0.0,
                    }
            }
        }
    } else {
        // ChartBottom shares the plot with the volume bars. Every module in that zone sits above
        // the band height the caller reported — zero when the tab prints over the bars; the
        // control strip does not — it runs the full height of the plot, and the bars never reach
        // it.
        let floor = match zone {
            LabelZone::ChartBottom => geom.plot_bottom - geom.volume_band_h.max(0.0),
            _ => geom.plot_bottom,
        };
        floor - ZONE_PAD
    }
}
