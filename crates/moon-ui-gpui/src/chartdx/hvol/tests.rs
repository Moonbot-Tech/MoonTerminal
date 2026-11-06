//! Native-device recovery must retain the last horizontal-volume profile without another read.

use super::{HvolLayer, HvolRowGpu};
use crate::chartdx::gpu::test_support::TestTarget;

/// Recover the uploaded profile, including a replacement queued just before device loss.
#[test]
fn native_recovery_replays_horizontal_volumes_and_pending_replacement() {
    let mut layer = HvolLayer::new();
    let mut row = HvolRowGpu {
        price_lo: 10.0,
        price_hi: 11.0,
        buy: 7.0,
        sell: 3.0,
    };
    layer.set(vec![row]);
    for generation in 1..=4 {
        if generation == 3 {
            row.buy = 25.0;
            layer.set(vec![row]);
        }
        let target = TestTarget::new(32, 32);
        layer.prepare(&target.device, &target.context, &target.raw(generation));
        assert_eq!(
            layer.count, 1,
            "retained row must remain drawable on device {generation}"
        );
        let bytes = bytemuck::bytes_of(&row);
        assert_eq!(
            target.buffer_bytes(&layer.pipe.as_ref().unwrap().buffer, bytes.len()),
            bytes
        );
    }
}
