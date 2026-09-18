//! Unit coverage for the header strip's derived colours.

use super::palette;
use crate::contrast::contrast_ratio;

/// `ink.rs:palette` must keep both text registers at the universal readable floor; otherwise a
/// user-selected chart theme makes the burnt-in screenshot header disappear against its own band.
#[test]
fn palette_keeps_both_registers_readable_on_every_required_ground() {
    for (bg, text) in [
        ([30, 30, 30], [211, 211, 211]),
        ([255, 255, 255], [0, 0, 0]),
        ([128, 128, 128], [211, 211, 211]),
        ([117, 117, 117], [211, 211, 211]),
        ([77, 77, 77], [77, 77, 77]),
        ([0, 255, 0], [211, 211, 211]),
    ] {
        let colors = palette(bg, text);
        assert!(contrast_ratio(colors.primary, colors.band) >= 4.5);
        assert!(contrast_ratio(colors.secondary, colors.band) >= 4.5);
    }
}

/// `ink.rs:palette` must reach the higher scanned-text target in the shipped themes; otherwise the
/// standard dark and light screenshots lose the hierarchy intended for rapid chart reading.
#[test]
fn palette_reaches_the_primary_target_on_both_shipped_themes() {
    for (bg, text) in [
        ([30, 30, 30], [211, 211, 211]),
        ([255, 255, 255], [0, 0, 0]),
    ] {
        let colors = palette(bg, text);
        assert!(contrast_ratio(colors.primary, colors.band) >= 7.0);
    }
}

/// `ink.rs:toward_band` must stop before the muted register exceeds the primary register; otherwise
/// secondary context becomes louder than the figures the shared screenshot is meant to foreground.
#[test]
fn the_secondary_register_is_never_louder_than_primary() {
    for (bg, text) in [
        ([30, 30, 30], [211, 211, 211]),
        ([255, 255, 255], [0, 0, 0]),
        ([128, 128, 128], [211, 211, 211]),
        ([0, 255, 0], [211, 211, 211]),
    ] {
        let colors = palette(bg, text);
        assert!(
            contrast_ratio(colors.secondary, colors.band)
                <= contrast_ratio(colors.primary, colors.band)
        );
    }
}

/// `ink.rs:palette` must give the lead the primary ink and distinguish the band from the chart;
/// otherwise the hierarchy collapses or the strip becomes invisible on a pasted chart image.
#[test]
fn palette_uses_primary_ink_for_the_lead_and_a_visible_band_and_hairline() {
    let bg = [30, 30, 30];
    let colors = palette(bg, [211, 211, 211]);

    assert_eq!(colors.lead, colors.primary);
    assert_ne!(colors.band, bg);
    assert!(contrast_ratio(colors.hairline, bg) > contrast_ratio(colors.band, bg));
}

/// `ink.rs:palette` must preserve the readable floor across every greyscale ground; otherwise a
/// bounded colour walk can terminate at an unreadable pole for a theme users are allowed to choose.
#[test]
fn palette_rederives_the_universal_floor_across_all_grey_grounds() {
    let minimum = (0u8..=255)
        .map(|value| {
            let colors = palette([value; 3], [211, 211, 211]);
            contrast_ratio(colors.primary, colors.band)
                .min(contrast_ratio(colors.secondary, colors.band))
        })
        .fold(f64::INFINITY, f64::min);

    assert!(minimum >= 4.5, "lowest grey-ground contrast was {minimum}");
}
