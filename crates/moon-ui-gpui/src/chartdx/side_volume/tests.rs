//! Native-device recovery must retain the last side-volume samples without another source read.

use super::{SideVolumeGpu, SideVolumeLayer};
use crate::chartdx::gpu::test_support::TestTarget;

/// Consuming the upload must not discard the only replayable GPU input.
#[test]
fn native_recovery_replays_side_volumes_and_clear() {
    let mut layer = SideVolumeLayer::new();
    let row = SideVolumeGpu {
        t_open_rel: 12.0,
        tf_rel: 5.0,
        buy: 7.0,
        sell: 3.0,
    };
    layer.set(vec![row]);
    for generation in 1..=3 {
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
    layer.set(Vec::new());
    for generation in 4..=5 {
        let target = TestTarget::new(32, 32);
        layer.prepare(&target.device, &target.context, &target.raw(generation));
        assert_eq!(layer.count, 0, "recovery must not resurrect cleared data");
    }
}
