//! A line or marker edit must not be reported as a base-cache edit.

use bytemuck::Zeroable;

use super::*;
use crate::chartdx::types::{MarkerGpu, ZoneGpu};

/// Breakage: `UserdataDirty::base_stale` is true for a marker or segment change.
/// Consequence: every trade-arrow or order-line revision rebakes the candle and book image.
#[test]
fn only_a_zone_change_stales_the_base() {
    let lines = UserdataDirty {
        zones: false,
        hlines: true,
        segs: true,
        markers: true,
    };
    assert!(!lines.base_stale());
    let zones = UserdataDirty {
        zones: true,
        hlines: false,
        segs: false,
        markers: false,
    };
    assert!(zones.base_stale());
}

/// Breakage: `replace_pod` rewrites a buffer whose bytes did not change, or keeps one that did.
/// Consequence: an unchanged order zone still invalidates the base, or a moved marker stays put.
#[test]
fn replace_pod_rewrites_only_a_different_buffer() {
    let mut slot = vec![ZoneGpu::zeroed()];
    let same = vec![ZoneGpu::zeroed()];
    assert!(!replace_pod(&mut slot, same));
    assert_eq!(slot.len(), 1);

    let mut moved = ZoneGpu::zeroed();
    moved.m[0] = 1.5;
    assert!(replace_pod(&mut slot, vec![moved]));
    assert_eq!(slot[0].m[0].to_bits(), 1.5f32.to_bits());

    let mut markers = vec![MarkerGpu::zeroed(); 3];
    let mut next = markers.clone();
    next[2].pos[0] = 4.0;
    assert!(replace_pod(&mut markers, next));
    assert_eq!(markers[2].pos[0].to_bits(), 4.0f32.to_bits());
    let again = markers.clone();
    assert!(!replace_pod(&mut markers, again));
}

/// Breakage: `resync_mapped` rewrites a marker the source still holds, or keeps one it replaced.
/// Consequence: a new trade either recopies the whole arrow union or leaves a stale arrow up.
#[test]
fn resync_mapped_rewrites_only_the_changed_span() {
    let mut retained = vec![1u32, 2, 3, 4];
    let mut gpu = vec![10u32, 20, 30, 40];
    assert!(!resync_mapped(
        &[1, 2, 3, 4],
        &mut retained,
        &mut gpu,
        |n| n * 10
    ));
    assert_eq!(gpu, [10, 20, 30, 40]);

    assert!(resync_mapped(&[1, 2, 3, 9], &mut retained, &mut gpu, |n| n * 10));
    assert_eq!(gpu, [10, 20, 30, 90]);
    assert_eq!(retained, [1, 2, 3, 9]);

    assert!(resync_mapped(
        &[1, 2, 3, 9, 5],
        &mut retained,
        &mut gpu,
        |n| n * 10
    ));
    assert_eq!(gpu, [10, 20, 30, 90, 50]);

    assert!(resync_mapped(&[1, 2, 3], &mut retained, &mut gpu, |n| n * 10));
    assert_eq!(gpu, [10, 20, 30]);

    assert!(resync_mapped(&[8, 2, 3], &mut retained, &mut gpu, |n| n * 10));
    assert_eq!(gpu, [80, 20, 30]);

    assert!(resync_mapped(&[8, 7, 3], &mut retained, &mut gpu, |n| n * 10));
    assert_eq!(gpu, [80, 70, 30]);
    assert_eq!(retained, [8, 7, 3]);

    assert!(resync_mapped(&[9, 7, 6], &mut retained, &mut gpu, |n| n * 10));
    assert_eq!(gpu, [90, 70, 60]);

    gpu.pop();
    assert!(resync_mapped(&[8, 2, 3], &mut retained, &mut gpu, |n| n * 10));
    assert_eq!(gpu, [80, 20, 30]);
}

/// Breakage: wgpu or Metal still clears the base cache on every `set_userdata` or `set_clusters`.
/// Consequence: a trade-arrow or order-line edit rebakes candles and the book.
#[test]
fn backends_rebake_the_base_only_when_zones_change() {
    for src in [
        include_str!("../wgpu_backend/layers.rs"),
        include_str!("../metal_backend.rs"),
    ] {
        assert!(
            !src.contains("userdata_buffers_dirty"),
            "a shared userdata flag uploads and invalidates every layer"
        );
        let start = src.find("pub fn set_userdata(").expect("set_userdata");
        let rest = &src[start..];
        let end = rest.find("pub fn set_clusters(").expect("set_clusters");
        let body = &rest[..end];
        assert!(body.contains("replace_pod("));
        assert_eq!(body.matches("base_cache.valid = false").count(), 1);
        let at = body.find("base_cache.valid = false").unwrap();
        assert!(
            body[..at].contains("if dirty.base_stale()"),
            "the base invalidation must be the shared zone predicate, not a local flag"
        );
        assert!(
            !body[..at].contains("if zones_changed"),
            "a local zones flag can be widened without failing a prefix search"
        );
        let clusters = &rest[end..];
        let next = clusters
            .find("\n    pub fn ")
            .expect("function after set_clusters");
        assert!(!clusters[..next].contains("base_cache"));
    }
}

/// Scale probe: how many of 1000 userdata sets rebake the base, and what the
/// byte compare of 100000 markers costs. Ignored: it allocates the union.
/// Marker sets must rebake nothing; zone sets rebake every time.
#[test]
#[ignore = "scale probe: zone-only base invalidation versus a 100k marker compare"]
fn userdata_base_scale_probe() {
    use std::time::Instant;

    use bytemuck::Zeroable;

    const SETS: usize = 1_000;
    const N: usize = 100_000;
    let mut marker_rebakes = 0usize;
    let mut zone_rebakes = 0usize;
    for _ in 0..SETS {
        let markers = UserdataDirty {
            zones: false,
            hlines: true,
            segs: true,
            markers: true,
        };
        if markers.base_stale() {
            marker_rebakes += 1;
        }
        let zones = UserdataDirty {
            zones: true,
            hlines: false,
            segs: false,
            markers: false,
        };
        if zones.base_stale() {
            zone_rebakes += 1;
        }
    }
    let mut slot = vec![MarkerGpu::zeroed(); N];
    let mut same_ms = Vec::with_capacity(7);
    let mut edit_ms = Vec::with_capacity(7);
    let mut rewrote = false;
    let mut edited = false;
    for run in 0..7 {
        let same = slot.clone();
        let start = Instant::now();
        rewrote = replace_pod(&mut slot, same);
        same_ms.push(start.elapsed().as_secs_f64() * 1_000.0);
        let mut moved = slot.clone();
        moved[N - 1].pos[0] = 3.0 + run as f32;
        let start = Instant::now();
        edited = replace_pod(&mut slot, moved);
        std::hint::black_box(slot.len());
        edit_ms.push(start.elapsed().as_secs_f64() * 1_000.0);
    }
    same_ms.sort_by(|a, b| a.total_cmp(b));
    edit_ms.sort_by(|a, b| a.total_cmp(b));
    println!(
        "userdata_base_scale sets {SETS} marker_rebakes {marker_rebakes} zone_rebakes {zone_rebakes} same_ms {:.3} edit_ms {:.3}",
        same_ms[3], edit_ms[3]
    );
    assert_eq!(marker_rebakes, 0);
    assert_eq!(zone_rebakes, SETS);
    assert!(!rewrote);
    assert!(edited);
}
