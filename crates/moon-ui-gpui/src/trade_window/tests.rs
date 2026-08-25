use super::remembered_geometry;
use moon_core::config::layout::GeomRect;

fn rect(x: i32, y: i32, w: u32, h: u32, display_uuid: Option<uuid::Uuid>) -> GeomRect {
    GeomRect {
        x,
        y,
        w,
        h,
        maximized: false,
        fullscreen: false,
        display_uuid,
    }
}

/// `trade_window::remembered_geometry` must undo a cascade rather than persist it. Deleting the
/// nonzero-cascade branch makes each reopened trade window remember its offset and walk off-screen
/// over time, while losing the previous display identity can restore it on the wrong monitor.
#[test]
fn remembered_trade_window_geometry_never_persists_a_cascade_offset() {
    let identity = uuid::Uuid::from_u128(0xfeed_cafe_dead_beef_0123_4567_89ab_cdef);
    let observed = rect(134, 234, 900, 600, Some(identity));

    let uncascaded = remembered_geometry(None, observed, 0.0);
    assert_eq!(
        (
            uncascaded.x,
            uncascaded.y,
            uncascaded.w,
            uncascaded.h,
            uncascaded.display_uuid
        ),
        (
            observed.x,
            observed.y,
            observed.w,
            observed.h,
            observed.display_uuid
        ),
        "an uncascaded observation must be remembered whole"
    );
    let first_cascade = remembered_geometry(None, observed, 34.0);
    assert_eq!(
        (
            first_cascade.x,
            first_cascade.y,
            first_cascade.w,
            first_cascade.h,
            first_cascade.display_uuid
        ),
        (100, 200, 900, 600, Some(identity)),
        "the first cascaded window must subtract its opening offset before saving"
    );

    let previous = rect(100, 200, 640, 480, Some(identity));
    let subsequent_cascade = remembered_geometry(Some(previous), observed, 34.0);
    assert_eq!(
        (
            subsequent_cascade.x,
            subsequent_cascade.y,
            subsequent_cascade.w,
            subsequent_cascade.h,
            subsequent_cascade.display_uuid
        ),
        (100, 200, 900, 600, Some(identity)),
        "a cascaded window keeps the remembered origin and display while retaining its new size"
    );

    let mut saved = previous;
    for _ in 0..5 {
        let observed = rect(saved.x + 34, saved.y + 34, saved.w, saved.h, Some(identity));
        saved = remembered_geometry(Some(saved), observed, 34.0);
        assert_eq!(
            (saved.x, saved.y, saved.display_uuid),
            (100, 200, Some(identity)),
            "reopening a cascaded window repeatedly must not drift its remembered origin"
        );
    }
}
