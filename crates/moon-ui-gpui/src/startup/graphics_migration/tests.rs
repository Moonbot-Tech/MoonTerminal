use super::*;
use moon_core::market::candles::{VOLUME_STYLE_HILLS, VOLUME_STYLE_LEGACY_BARS, VOLUME_STYLE_OFF};

/// `graphics_migration.rs:resolve` — a theme file predates the bought/sold switch, so its style
/// alone said whether the band drew. Leaving the switch at the shipped default (on) would open a
/// user who had the band OFF on it, and drop the fold for one who had bars.
#[test]
fn legacy_style_decides_the_band_switch() {
    let off = resolve(LegacyChartGraphics {
        candle_volume_style: Some(VOLUME_STYLE_OFF),
        ..LegacyChartGraphics::default()
    });
    assert_eq!(off.candle_volume_style, VOLUME_STYLE_OFF);
    assert!(!off.candle_volume_sides);

    let bars = resolve(LegacyChartGraphics {
        candle_volume_style: Some(VOLUME_STYLE_LEGACY_BARS),
        ..LegacyChartGraphics::default()
    });
    assert_eq!(bars.candle_volume_style, VOLUME_STYLE_HILLS);
    assert!(bars.candle_volume_sides);

    // A file that never named the style keeps the shipped default: the band on.
    let unnamed = resolve(LegacyChartGraphics::default());
    assert_eq!(unnamed.candle_volume_style, VOLUME_STYLE_HILLS);
    assert!(unnamed.candle_volume_sides);
}
