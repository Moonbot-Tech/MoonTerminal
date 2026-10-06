//! Colour helpers over the palette tokens.

use super::*;

pub fn solid(hex: u32) -> Rgba {
    rgb(hex)
}

/// Convert a `0xRRGGBB` palette token to opaque `Hsla`.
///
/// This centralizes a conversion formerly duplicated in `screener/table.rs`, `panels/alerts.rs`,
/// and `strategies/mod.rs`.
///
/// Args:
///     hex: Palette color encoded as `0xRRGGBB`.
///
/// Returns:
///     The color with alpha set to one.
pub fn moon(hex: u32) -> Hsla {
    rgba_from(hex, 1.0)
}

/// Convert a `0xRRGGBB` palette token to `Hsla` with an explicit alpha value.
///
/// Args:
///     hex: Palette color encoded as `0xRRGGBB`.
///     alpha: Alpha passed through to `rgba_from`.
///
/// Returns:
///     The color with the requested alpha.
pub fn moon_alpha(hex: u32, alpha: f32) -> Hsla {
    rgba_from(hex, alpha)
}

/// Return the theme-correct positive colour.
///
/// The light theme uses its darker text token for legibility; the dark theme uses its base green.
pub fn positive_color(p: MoonPalette) -> u32 {
    if p.is_light() { p.green_text } else { p.green }
}

/// Return the theme-correct danger colour.
///
/// The light theme uses its darker text token for legibility; the dark theme uses its base red.
pub fn danger_color(p: MoonPalette) -> u32 {
    if p.is_light() { p.red_text } else { p.red }
}

/// Convert GPUI's `0xRRGGBB` representation to palette/config `[u8; 3]` RGB bytes.
pub fn u32_to_rgb(c: u32) -> [u8; 3] {
    [
        ((c >> 16) & 0xff) as u8,
        ((c >> 8) & 0xff) as u8,
        (c & 0xff) as u8,
    ]
}

/// Convert palette/config `[u8; 3]` RGB bytes to GPUI's `0xRRGGBB` representation.
pub fn rgb_to_u32(c: [u8; 3]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

/// Convert palette/config `[u8; 3]` RGB bytes directly to a `MoonColorPicker` `Hsla` value.
///
/// The one-liner every `MoonColorPickerState` seed/init site repeats (`rgb(rgb_to_u32(c)).into()`)
/// — kept here beside `rgb_to_u32` rather than re-derived at each call site.
pub fn rgb_bytes_to_hsla(c: [u8; 3]) -> Hsla {
    rgb(rgb_to_u32(c)).into()
}

/// Build the explicit palette supplied to `MoonColorPicker::colors`.
///
/// It contains five saturation/lightness variants for each of 12 hues plus five grays, for 65
/// swatches total. Each picker row contains the five variants of one hue. The forked component has
/// no free-form HSV picker and otherwise exposes only ten theme colors.
///
/// Returns:
///     The complete ordered swatch palette.
pub fn picker_palette() -> Vec<Hsla> {
    let mut out = Vec::with_capacity(65);
    // Pairs are ordered by decreasing lightness from light to dark; saturation varies independently.
    const SHADES: [(f32, f32); 5] = [
        (0.85, 0.72),
        (0.85, 0.58),
        (0.90, 0.46),
        (0.85, 0.34),
        (0.70, 0.24),
    ];
    for hue_step in 0..12 {
        let h = hue_step as f32 / 12.0;
        for (s, l) in SHADES {
            out.push(hsla(h, s, l, 1.0));
        }
    }
    for l in [0.95, 0.75, 0.50, 0.30, 0.10] {
        out.push(hsla(0.0, 0.0, l, 1.0));
    }
    out
}

/// Convert a `MoonColorPicker` `Hsla` value to 8-bit sRGB for byte-backed color fields.
///
/// The alpha channel is intentionally omitted. Consumers include `fig_styles` and strategy hex
/// fields, which retain or encode alpha separately.
///
/// Args:
///     h: Picker color to convert.
///
/// Returns:
///     Rounded red, green, and blue bytes.
pub fn hsla_to_rgb8(h: Hsla) -> [u8; 3] {
    let c: Rgba = h.into();
    [
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    ]
}

pub fn mono() -> SharedString {
    SharedString::from("Geist Mono")
}

pub fn ui_font() -> SharedString {
    SharedString::from("Inter")
}
