//! Native chart-state restoration regressions.

use super::restored_chart_size;
use crate::persistence::chart_persist::WinGeom;
use gpui::{px, size};

/// Removing the native-state guard resizes a maximized chart down to its restore rectangle;
/// suppressing every correction instead regresses normal windows restored on another DPI.
#[test]
fn dpi_correction_is_only_for_restored_windowed_charts() {
    for (fields, expected) in [
        ("", Some(size(px(900.0), px(620.0)))),
        (r#","maximized":true"#, None),
        (r#","fullscreen":true"#, None),
        (r#","maximized":true,"fullscreen":true"#, None),
    ] {
        let geom: WinGeom =
            serde_json::from_str(&format!(r#"{{"x":400,"y":260,"w":900,"h":620{fields}}}"#))
                .unwrap();
        assert_eq!(restored_chart_size(true, geom), expected);
        assert_eq!(restored_chart_size(false, geom), None);
    }
}
