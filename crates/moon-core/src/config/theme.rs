//! Chart appearance theme (background/grid/crosshair) in a separate portable
//! `theme.toml` file in the config directory. Copying the file transfers the appearance.
//! Stored as a pair of `[dark]`/`[light]` sets (like
//! `orders.toml`), so the file is self-contained and portable regardless of the recipient's
//! selected UI mode. Colors are specified in sRGB (like the palette/UI inputs); shaders
//! convert them to linear values (see [[srgb-shader-colors]]).

use serde::{Deserialize, Serialize};

use super::paths;
use crate::palette;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChartTheme {
    // --- Chart: background and grid ---
    /// Chart background (sRGB).
    pub bg: [u8; 3],
    /// Grid-line color (sRGB).
    pub grid: [u8; 3],
    /// Grid visibility, 0..1 (0 = hidden).
    pub grid_alpha: f32,
    /// Persisted photo-background opacity, 0..1; inactive while photo backgrounds are disabled.
    pub background_opacity: f32,
    /// Font-size adjustment for order-line labels AND the chart cursor (pixels, +/- from base 11.5).
    /// Slider in Settings/Interface. Does NOT affect axis labels or the corner ticker.
    pub label_font_delta: f32,

    // --- Chart: crosshair ---
    /// Crosshair color (sRGB).
    pub cross: [u8; 3],
    /// Crosshair-line opacity, 0..1.
    pub cross_alpha: f32,
    /// Crosshair-line half-thickness, in pixels.
    pub cross_thickness: f32,

    // --- Candles ---
    /// Rising-candle color (close ≥ open), sRGB. Default = buy-cross green.
    pub candle_up: [u8; 3],
    /// Falling-candle color, sRGB. Default = sell-cross orange.
    pub candle_down: [u8; 3],
    /// Neutral candle color in the trade zone ("Neutral color in zone" checkbox), sRGB.
    pub candle_neutral: [u8; 3],
    /// Candle-body fill opacity, 0..1 (outlines/wicks are drawn more opaquely).
    pub candle_fill_alpha: f32,

    // --- Order book ---
    /// Order-book background BETWEEN the best bid/ask (spread gap), sRGB.
    pub book_bg: [u8; 3],
    /// Background of the ask half of the book (above the best ask), sRGB.
    pub book_bg_ask: [u8; 3],
    /// Background of the bid half of the book (below the best bid), sRGB.
    pub book_bg_bid: [u8; 3],
    /// Bid-side (buy) color, sRGB.
    pub book_bid: [u8; 3],
    /// Ask-side (sell) color, sRGB.
    pub book_ask: [u8; 3],
    /// Brightness/opacity of individual order-book level lines, 0..1.
    pub book_level_alpha: f32,
    /// Thickness of individual order-book level lines, in physical pixels.
    pub book_level_width: f32,

    // --- Chart labels ---
    /// Color of positive values in labels (profit %, bid depth), sRGB.
    pub label_positive: [u8; 3],
    /// Color of negative values in labels (loss %, ask depth), sRGB.
    pub label_negative: [u8; 3],
    /// Neutral-label color, sRGB.
    pub label_neutral: [u8; 3],
    /// Price/time axis-label color, sRGB.
    pub axis_label: [u8; 3],
    /// Corner core/market caption color, sRGB.
    pub caption_label: [u8; 3],
    /// Neutral cursor/readout text color, sRGB.
    pub readout_label: [u8; 3],
    /// Alpha of the opaque cursor/readout backing.
    pub readout_bg_alpha: f32,
    /// Alpha of the light corner-caption backing (core name/ticker).
    pub readout_soft_bg_alpha: f32,
    /// Alpha of order-line label backings. Semi-opaque: appears solid, but when labels overlap,
    /// the higher-priority label covers the lower one (which shows through → "slides under" rather than disappearing).
    pub line_label_bg_alpha: f32,
    /// Alpha of the readout-backing border.
    pub readout_border_alpha: f32,
    /// Readout-border thickness, in pixels. 0 = no border.
    pub readout_border_px: f32,

    // Order-line styles (colors/thicknesses/markers) live in a separate orders.toml
    // (see config::orders::OrdersStyle) and are not duplicated in the theme.

    // --- Chart engine bootstrap ---
    /// Initial chart-engine palette seed (sRGB); the active `MoonPalette` replaces it before rendering.
    pub panel_bg: [u8; 3],
}

impl Default for ChartTheme {
    fn default() -> Self {
        Self {
            bg: [30, 30, 30],
            grid: [40, 40, 40],
            grid_alpha: 1.0,
            background_opacity: 0.18,
            label_font_delta: -1.5,
            cross: [128, 128, 128],
            cross_alpha: 0.5,
            cross_thickness: 1.0,
            // Candles use trade-cross colors (crosses.hlsl buy/sell); neutral is gray.
            candle_up: [47, 168, 92],
            candle_down: [255, 142, 90],
            candle_neutral: [128, 128, 128],
            candle_fill_alpha: 0.85,
            book_bg: [30, 30, 30],
            // The two halves of the order book are lightly tinted by side; the spread gap
            // between the best bid/ask remains the neutral book_bg.
            book_bg_ask: [42, 30, 27],
            book_bg_bid: [30, 36, 26],
            book_bid: [75, 86, 48],
            book_ask: [170, 73, 39],
            book_level_alpha: 0.5,
            book_level_width: 1.5,
            label_positive: palette::GREEN,
            label_negative: palette::RED,
            label_neutral: [211, 211, 211],
            axis_label: [211, 211, 211],
            caption_label: [211, 211, 211],
            readout_label: [211, 211, 211],
            readout_bg_alpha: 0.96,
            readout_soft_bg_alpha: 0.20,
            line_label_bg_alpha: 0.85,
            readout_border_alpha: 0.0,
            readout_border_px: 0.0,
            panel_bg: [24, 25, 27],
        }
    }
}

impl ChartTheme {
    /// Default light set: dark default plus Moonbot light overrides.
    fn default_light() -> Self {
        let mut t = Self::default();
        t.apply_light_defaults();
        t
    }

    /// Moonbot light defaults. Background/grid use values from the light MoonUI palette
    /// (chart_bg 0xFFFFFF / row_line 0xECEFF2). The renderer used to override them from the
    /// palette on the fly; now they are simply editable defaults in the light set.
    fn apply_light_defaults(&mut self) {
        self.bg = [255, 255, 255];
        self.grid = [236, 239, 242];
        self.cross = [128, 128, 128];
        self.candle_up = [0, 128, 0];
        self.candle_down = [255, 0, 0];
        self.candle_neutral = [150, 150, 150];
        self.candle_fill_alpha = 0.85;
        self.book_bg = [255, 255, 255];
        self.book_bg_ask = [255, 244, 242];
        self.book_bg_bid = [243, 250, 242];
        self.book_bid = [0, 128, 0];
        self.book_ask = [255, 0, 0];
        self.book_level_alpha = 0.5;
        self.book_level_width = 1.5;
        self.label_positive = [0, 128, 0];
        self.label_negative = [255, 0, 0];
        self.label_neutral = [0, 0, 0];
        self.axis_label = [0, 0, 0];
        self.caption_label = [0, 0, 0];
        self.readout_label = [0, 0, 0];
        self.readout_bg_alpha = 0.96;
        self.readout_soft_bg_alpha = 0.20;
        self.line_label_bg_alpha = 0.85;
        self.readout_border_alpha = 0.0;
        self.readout_border_px = 0.0;
    }
}

/// Chart theme SEPARATELY for dark and light UI modes (per theme, like
/// [`super::OrdersStyleSet`]). Stored in one `theme.toml` with `[dark]`/`[light]` tables;
/// `ui_theme_mode` selects the active set.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChartThemeSet {
    pub dark: ChartTheme,
    pub light: ChartTheme,
}

impl Default for ChartThemeSet {
    fn default() -> Self {
        Self {
            dark: ChartTheme::default(),
            light: ChartTheme::default_light(),
        }
    }
}

impl ChartThemeSet {
    /// Set for the active mode: `light=true` → light, otherwise dark.
    pub fn get(&self, light: bool) -> &ChartTheme {
        if light {
            &self.light
        } else {
            &self.dark
        }
    }
    pub fn get_mut(&mut self, light: bool) -> &mut ChartTheme {
        if light {
            &mut self.light
        } else {
            &mut self.dark
        }
    }

    /// Reads `theme.toml`. The new format uses `[dark]`/`[light]` tables. An OLD flat
    /// `ChartTheme` (effectively a dark theme; light mode overrode it with defaults) becomes
    /// `dark`, while `light` takes the light default; the file is immediately saved again.
    /// A missing file yields and saves the default; a corrupt file yields the default without failing.
    pub fn load() -> Self {
        let path = paths::theme_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            let def = Self::default();
            let _ = def.save();
            return def;
        };
        if text.contains("[dark") || text.contains("[light") {
            return toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("theme.toml повреждён ({e}); беру дефолт");
                Self::default()
            });
        }
        let flat: ChartTheme = toml::from_str(&text).unwrap_or_default();
        let set = Self {
            dark: flat,
            light: ChartTheme::default_light(),
        };
        let _ = set.save();
        set
    }

    /// Writes theme.toml (open, human-readable TOML that can be shared).
    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::theme_path(), self, "theme.toml")
    }

    /// Text in theme.toml format for "Copy" in Settings (= file contents).
    pub fn to_share_string(&self) -> Option<String> {
        toml::to_string_pretty(self).ok()
    }

    /// Parses theme.toml text (clipboard paste / file contents). Validates using distinctive
    /// theme keys; serde ignores unknown fields and would silently produce the default for a
    /// foreign file. An old flat `ChartTheme` (effectively dark) replaces `dark` in `current`
    /// (as in the load migration). `None` means the text is not a chart theme.
    pub fn parse_share(text: &str, current: &Self) -> Option<Self> {
        const KEYS: [&str; 4] = ["bg", "cross", "book_bid", "panel_bg"];
        let v: toml::Value = toml::from_str(text).ok()?;
        let table_has = |name: &str| {
            v.get(name)
                .and_then(|x| x.as_table())
                .is_some_and(|t| KEYS.iter().any(|k| t.contains_key(*k)))
        };
        if table_has("dark") || table_has("light") {
            return toml::from_str(text).ok();
        }
        if v.as_table()
            .is_some_and(|t| KEYS.iter().any(|k| t.contains_key(*k)))
        {
            let flat: ChartTheme = toml::from_str(text).ok()?;
            return Some(Self {
                dark: flat,
                light: current.light.clone(),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests;
