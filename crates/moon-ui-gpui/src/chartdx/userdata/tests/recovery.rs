//! Read native buffers after device replacement, preserving both queued unions and hover patches.

use super::super::{LineInstance, MarkerInstance, SegInstance, UserDataLayer, ZoneInstance};
use crate::chartdx::{gpu::test_support::TestTarget, types::mk_of};
use bytemuck::Zeroable;

/// Replacing the device must keep every uploaded geometry buffer and the latest marker patch.
#[test]
fn native_recovery_keeps_userdata_and_hover() {
    let mut layer = UserDataLayer::new();
    let mut zone = ZoneInstance::zeroed();
    zone.color = [0.1, 0.2, 0.3, 1.0];
    let mut line = LineInstance::zeroed();
    line.color = [0.4, 0.5, 0.6, 1.0];
    let mut seg = SegInstance::zeroed();
    seg.color = [0.7, 0.8, 0.9, 1.0];
    let mut marker = MarkerInstance::zeroed();
    marker.size = 5.0;
    layer.set(&[zone], &[line], &[seg], &[marker]);
    let first = TestTarget::new(32, 32);
    layer.prepare(&first.device, &first.context, &first.raw(1));
    let original = buffers(&layer, &first);
    for uploaded in &original {
        assert!(
            uploaded.iter().any(|byte| *byte != 0),
            "fixture must populate every native buffer"
        );
    }
    marker.size = 11.0;
    assert!(layer.patch_markers(&[(0, marker)]));
    layer.prepare(&first.device, &first.context, &first.raw(1));
    let patched = buffers(&layer, &first);
    assert_ne!(patched[3], original[3]);
    assert_eq!(patched[3], bytemuck::bytes_of(&mk_of(&marker)));
    for generation in 2..=3 {
        let target = TestTarget::new(32, 32);
        layer.prepare(&target.device, &target.context, &target.raw(generation));
        assert_eq!(
            buffers(&layer, &target),
            patched,
            "device recovery must preserve the visible hover union"
        );
    }
    // A later source sync is allowed to replace the patch; queue it before replacing the device.
    marker.size = 4.0;
    layer.set(&[zone], &[line], &[seg], &[marker]);
    let target = TestTarget::new(32, 32);
    layer.prepare(&target.device, &target.context, &target.raw(4));
    assert_eq!(
        buffers(&layer, &target)[3],
        bytemuck::bytes_of(&mk_of(&marker))
    );
    layer.set(&[], &[], &[], &[]);
    layer.prepare(&target.device, &target.context, &target.raw(4));
    let target = TestTarget::new(32, 32);
    layer.prepare(&target.device, &target.context, &target.raw(5));
    assert_eq!(
        (
            layer.zone_count,
            layer.hl_count,
            layer.seg_count,
            layer.mk_count
        ),
        (0, 0, 0, 0)
    );
}

/// Read the actual uploaded buffers and require that the draw path will consume each row.
fn buffers(layer: &UserDataLayer, target: &TestTarget) -> [Vec<u8>; 4] {
    assert_eq!(
        (
            layer.zone_count,
            layer.hl_count,
            layer.seg_count,
            layer.mk_count
        ),
        (1, 1, 1, 1)
    );
    let pipe = layer.pipe.as_ref().unwrap();
    use crate::chartdx::types::{HLineGpu, MarkerGpu, SegGpu, ZoneGpu};
    [
        target.buffer_bytes(&pipe.zone_buf, std::mem::size_of::<ZoneGpu>()),
        target.buffer_bytes(&pipe.hl_buf, std::mem::size_of::<HLineGpu>()),
        target.buffer_bytes(&pipe.seg_buf, std::mem::size_of::<SegGpu>()),
        target.buffer_bytes(&pipe.mk_buf, std::mem::size_of::<MarkerGpu>()),
    ]
}
