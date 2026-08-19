//! Persistence compatibility for detached chart-window geometry.

use super::*;

/// A `charts.json` written before chart geometry carried a display identity must still load, and a
/// geometry that names no display must not start writing the key.
///
/// `detached` inside this file is what reopens a chart tab as its own window; a decode broken by
/// the new field would turn every detached chart back into an in-window tab on the next launch.
#[test]
fn detached_chart_geometry_without_a_display_identity_still_loads() {
    let legacy = r#"{"x":400,"y":260,"w":900,"h":620}"#;
    let geom: WinGeom = serde_json::from_str(legacy).expect("a pre-display WinGeom must decode");
    assert_eq!((geom.x, geom.y, geom.w, geom.h), (400, 260, 900, 620));
    assert_eq!(geom.display_uuid, None);

    let re_encoded = serde_json::to_string(&geom).expect("geometry must re-encode");
    assert!(
        !re_encoded.contains("display_uuid"),
        "a geometry with no display must keep the file's previous shape"
    );

    let identity = uuid::Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
    let saved = WinGeom {
        display_uuid: Some(identity),
        ..geom
    };
    let back: WinGeom = serde_json::from_str(&serde_json::to_string(&saved).unwrap())
        .expect("geometry must round-trip");
    assert_eq!(back.display_uuid, Some(identity));
    assert_eq!(
        back, saved,
        "equality must cover the display: on macOS the same x/y on another monitor is a real move"
    );
}
