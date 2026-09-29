//! Volume bar scale over a bake window: the full-scan fold and its incremental carry across an
//! append, so a live tick batch does not rescan the whole resident ring for two maxima.

use super::super::gpu::ChartCross;

/// Rows a bake window's scale reads: `bake_t0 - 2/ttp ..= bake_t0 + (tex_w + 2)/ttp`.
pub(super) fn volume_window_bounds(bake_t0: f32, tex_w: f32, time_to_px: f32) -> (f32, f32) {
    (
        bake_t0 - 2.0 / time_to_px,
        bake_t0 + (tex_w + 2.0) / time_to_px,
    )
}

/// Whether `c` contributes to the scale of the window `left..=right`.
fn counts(c: &ChartCross, left: f32, right: f32) -> bool {
    !(c.time_rel < left || c.time_rel > right || c.qty <= 0.0)
}

/// Fold rows into a `(buy, sell)` maximum; sides >= 2 are liquidations without volume bars.
pub(super) fn fold_volume_scale<'a>(
    mut acc: (f32, f32),
    rows: impl IntoIterator<Item = &'a ChartCross>,
    left: f32,
    right: f32,
) -> (f32, f32) {
    for c in rows {
        if !counts(c, left, right) {
            continue;
        }
        match c.side {
            0 => acc.0 = acc.0.max(c.qty),
            1 => acc.1 = acc.1.max(c.qty),
            _ => {}
        }
    }
    acc
}

/// Carry a window's cached scale across an append, or `None` when an evicted in-window row may
/// have held a side's maximum and only a rescan gives the exact answer.
pub(super) fn carry_volume_scale<'a>(
    cached: (f32, f32),
    evicted: impl IntoIterator<Item = &'a ChartCross>,
    appended: impl IntoIterator<Item = &'a ChartCross>,
    left: f32,
    right: f32,
) -> Option<(f32, f32)> {
    let lost_max = evicted.into_iter().any(|c| {
        counts(c, left, right)
            && match c.side {
                0 => c.qty >= cached.0,
                1 => c.qty >= cached.1,
                _ => false,
            }
    });
    (!lost_max).then(|| fold_volume_scale(cached, appended, left, right))
}
