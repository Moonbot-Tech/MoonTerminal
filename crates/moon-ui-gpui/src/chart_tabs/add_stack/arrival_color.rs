//! Pure colour decision for a chart's arrival border, independent of pulse scheduling.

/// Require 3:1 at peak opacity so a thin, fading border remains distinguishable from its ground.
const MIN_FLASH_CONTRAST: f64 = 3.0;

/// Return the core's RGB colour when readable against `background`, otherwise `accent`.
pub(super) fn arrival_color(core: Option<[u8; 3]>, background: [u8; 3], accent: u32) -> u32 {
    let Some(core) = core else {
        return accent;
    };
    let foreground = luminance(core);
    let background = luminance(background);
    let contrast = (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05);
    if contrast >= MIN_FLASH_CONTRAST {
        crate::design::rgb_to_u32(core)
    } else {
        accent
    }
}

/// Convert an sRGB triplet to relative luminance for the flash contrast check.
fn luminance(rgb: [u8; 3]) -> f64 {
    let linear = rgb.map(|channel| {
        let channel = f64::from(channel) / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
}

#[cfg(test)]
mod tests;
