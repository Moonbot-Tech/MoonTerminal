//! Unit coverage for the header strip's derived colours.

use super::{contrast_ratio, mix, palette, pole, relative_luminance};

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

/// `ink.rs:pole` must select the more-contrasting pole on a mid-grey ground; otherwise a familiar
/// luminance-threshold simplification chooses white and drops the screenshot header below 4.5:1.
#[test]
fn pole_picks_black_for_the_mid_grey_that_defeats_a_half_luminance_threshold() {
    let mid_grey = [128, 128, 128];

    assert!(relative_luminance(mid_grey) > 0.179);
    assert!(relative_luminance(mid_grey) < 0.5);
    assert_eq!(pole(mid_grey), [0, 0, 0]);
    assert!(contrast_ratio(pole(mid_grey), mid_grey) > contrast_ratio([255, 255, 255], mid_grey));
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

/// `ink.rs:contrast_ratio` must remain a symmetric WCAG ratio; otherwise the stated floors vary
/// with argument order and a contrast check can approve unreadable text.
#[test]
fn contrast_ratio_is_symmetric_and_has_known_endpoints() {
    let grey = [60, 120, 180];

    assert_eq!(contrast_ratio(grey, grey), 1.0);
    assert_eq!(contrast_ratio([0, 0, 0], [255, 255, 255]), 21.0);
    assert_eq!(
        contrast_ratio(grey, [250, 250, 250]),
        contrast_ratio([250, 250, 250], grey)
    );
}

/// `ink.rs:mix` must preserve both endpoint colours; otherwise a full muting or full contrast walk
/// changes the target colour and misses the intended contrast floor.
#[test]
fn mix_is_exact_at_both_endpoints() {
    let from = [11, 22, 33];
    let to = [201, 202, 203];

    assert_eq!(mix(from, to, 0.0), from);
    assert_eq!(mix(from, to, 1.0), to);
}
