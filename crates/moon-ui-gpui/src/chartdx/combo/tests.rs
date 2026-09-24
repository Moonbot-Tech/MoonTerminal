//! CPU-only regression checks for native tick selection and bake-window scaling.

use super::{ChartCross, ComboLayer, TickTimeOrder};
use crate::chartdx::types::reset_cross_ring;

/// Reinstating overlap/wrap guards rebakes every dense live append after offscreen eviction.
#[test]
fn offscreen_eviction_with_overlapping_append_keeps_incremental_bake() {
    for head in [0, 3] {
        let mut layer = ComboLayer::new();
        layer.set_capacity(4, 1);
        let chronological = [
            cross(0.0, 0, 8.0),
            cross(1.0, 0, 8.0),
            cross(80.0, 0, 8.0),
            cross(100.0, 1, 8.0),
        ];
        let mut slots = chronological;
        slots.rotate_right(head);
        resident(&mut layer, &slots);
        layer.resident_head = head;
        layer.append(&[cross(100.0, 0, 1.0), cross(100.0, 1, 1.0)]);
        let span = (50.0, 250.0);
        assert!(!layer.append_invalidates_bake(layer.pending_append.len(), span));
        assert!(
            layer.append_invalidates_bake(3, span),
            "visible eviction must repaint"
        );
        assert!(
            layer.append_invalidates_bake(4, span),
            "full replacement must repaint"
        );
    }
}

/// Build distinct tick prices so selection assertions detect dropped or substituted rows.
fn cross(time: f32, side: u32, qty: f32) -> ChartCross {
    ChartCross {
        time_rel: time,
        side,
        qty,
        price: time + 100.0,
    }
}

/// Model an already-uploaded reset without creating a GPU device.
fn resident(layer: &mut ComboLayer, rows: &[ChartCross]) {
    reset_cross_ring(
        &mut layer.resident_crosses,
        &mut layer.resident_head,
        &mut layer.resident_count,
        layer.cross_capacity as usize,
        rows,
    );
    layer.tick_time_order = TickTimeOrder::default();
    layer
        .tick_time_order
        .extend(rows.iter().map(|c| c.time_rel));
}

/// Searching resident storage alone would expose evicted ticks and miss pending equal-time prints.
#[test]
fn bounded_samples_include_pending_eviction_and_reset() {
    let mut layer = ComboLayer::new();
    layer.set_capacity(4, 1);
    resident(
        &mut layer,
        &[
            cross(1.0, 0, 1.0),
            cross(2.0, 0, 2.0),
            cross(3.0, 1, 3.0),
            cross(4.0, 0, 4.0),
        ],
    );
    layer.append(&[cross(4.0, 1, 5.0), cross(5.0, 0, 6.0)]);
    assert_eq!(
        layer
            .tick_samples(2.0, 4.0)
            .map(|c| c.qty)
            .collect::<Vec<_>>(),
        [3.0, 4.0, 5.0]
    );
    layer.reset(vec![cross(20.0, 0, 7.0), cross(21.0, 1, 8.0)]);
    layer.append(&[cross(21.0, 0, 9.0)]);
    assert_eq!(
        layer
            .tick_samples(21.0, 21.0)
            .map(|c| c.qty)
            .collect::<Vec<_>>(),
        [8.0, 9.0]
    );
}

/// A sorted assumption or clearing evidence on resource loss drops out-of-order liquidation ticks.
#[test]
fn pending_disorder_survives_resource_recreation() {
    let mut layer = ComboLayer::new();
    layer.set_capacity(8, 1);
    layer.reset(vec![cross(10.0, 0, 1.0), cross(30.0, 1, 2.0)]);
    layer.append(&[cross(20.0, 2, 3.0)]);
    layer.refresh_pending_time_order();
    let selected: Vec<_> = layer
        .tick_samples(19.0, 21.0)
        .filter(|c| c.time_rel >= 19.0 && c.time_rel <= 21.0)
        .map(|c| c.qty)
        .collect();
    assert_eq!(selected, [3.0]);
}

/// Using the global maximum or dropping the +/-2px band edges changes visible bar heights.
#[test]
fn bounded_volume_scale_preserves_exact_band_window() {
    let mut layer = ComboLayer::new();
    layer.set_capacity(8, 1);
    resident(
        &mut layer,
        &[
            cross(0.0, 0, 999.0),
            cross(9.0, 0, 7.0),
            cross(10.0, 1, 4.0),
            cross(21.0, 1, 8.0),
            cross(22.0, 0, 100.0),
        ],
    );
    assert_eq!(
        layer.volume_scale_for_bake_window(10.0, 20.0, 2.0),
        (7.0, 8.0)
    );
    assert_eq!(
        layer.volume_scale_for_bake_window(11.0, 16.0, 2.0),
        (1e-6, 4.0)
    );
}

/// Clearing evidence on a capacity-sized append violates the conservative arrival-time bound.
#[test]
fn capacity_sized_append_keeps_lateness_until_reset() {
    let mut layer = ComboLayer::new();
    layer.set_capacity(2, 1);
    layer.reset(vec![cross(30.0, 0, 1.0), cross(10.0, 2, 1.0)]);
    layer.append(&[cross(40.0, 0, 1.0), cross(50.0, 1, 1.0)]);
    assert_eq!(layer.tick_time_order.max_lateness(), 20.0);
    layer.reset(vec![cross(60.0, 0, 1.0), cross(60.0, 1, 1.0)]);
    assert_eq!(layer.tick_time_order.max_lateness(), 0.0);
}
