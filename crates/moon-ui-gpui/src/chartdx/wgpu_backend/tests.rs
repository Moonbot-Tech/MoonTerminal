use super::*;

fn crosses(side: u32, from_ms: f32, n: usize) -> Vec<ChartCross> {
    (0..n)
        .map(|i| ChartCross {
            time_rel: from_ms + i as f32,
            price: 1.0,
            side,
            qty: 1.0,
        })
        .collect()
}

/// A layer whose cross buffer already exists and holds everything it was handed: the state of
/// any frame after the first one.
fn uploaded_layers(capacity: usize) -> WgpuLayers {
    let mut layers = WgpuLayers::new();
    layers.set_combo_capacity(capacity, capacity);
    layers.reset_combo(crosses(0, 0.0, 4));
    layers.combo_buffers_dirty = false;
    layers.combo_upload_whole = false;
    layers
}

/// A full reload writes the trades and then appends the liquidations without evicting
/// (#1343). The liquidation append pushes its own slots as dirty ranges; the upload must still
/// copy the whole ring, or the reloaded trades never reach the GPU buffer while the
/// liquidations do.
#[test]
fn reload_then_liquidation_append_still_uploads_whole_ring() {
    let mut layers = uploaded_layers(100);
    layers.reset_combo(crosses(0, 0.0, 50));
    layers.append_combo(&crosses(2, 10.0, 3));
    assert!(!layers.combo_dirty_ranges.is_empty());
    assert!(!layers.cross_upload_is_partial());
}

/// A live append after an upload stays an incremental copy of the appended slots.
#[test]
fn live_append_after_upload_stays_partial() {
    let mut layers = uploaded_layers(100);
    layers.append_combo(&crosses(0, 10.0, 3));
    assert_eq!(layers.combo_dirty_ranges, vec![(4, 3)]);
    assert!(layers.cross_upload_is_partial());
}
