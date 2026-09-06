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
    /// Rising-candle color (close ≥ open), sRGB. Default = [`crate::palette::CANDLE_UP`].
    pub candle_up: [u8; 3],
    /// Falling-candle color, sRGB. Default = [`crate::palette::CANDLE_DOWN`].
    pub candle_down: [u8; 3],
    /// Neutral candle color in the trade zone ("Neutral color in zone" checkbox), sRGB.
    pub candle_neutral: [u8; 3],
    /// Candle-body fill opacity, 0..1 (outlines/wicks are drawn more opaquely).
    pub candle_fill_alpha: f32,

    // --- Price lines ---
    // Colour and width used to live as literals in the shaders (one copy per backend). They are
    // uniforms now, so all three backends read the same numbers from here.
    /// Last-price line colour, sRGB.
    pub price_line: [u8; 3],
    /// Last-price line opacity, 0..1.
    pub price_line_alpha: f32,
    /// Mark-price line colour, sRGB.
    pub mark_line: [u8; 3],
    /// Mark-price line opacity, 0..1.
    pub mark_line_alpha: f32,
    /// Thickness of BOTH price lines, in LOGICAL pixels. The device scale is applied once, where
    /// the uniform is built.
    pub price_line_px: f32,

    // The trade-mark sizes and the bottom-volume band moved to
    // `super::layout::ChartGraphicsCfg`, edited from the chart's palette popup. They describe a
    // chart TAB rather than a colour scheme, so two tabs on one theme can now differ.
    // `super::theme_legacy` carries an existing user's values across.

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
            // Desaturated candle tones shared with the light set; neutral is gray. Deliberately
            // NOT the trade-cross colors any more: a saturated pair made the chart, the bottom
            // volume band (which reads these two fields) and the order book compete at once.
            candle_up: palette::CANDLE_UP,
            candle_down: palette::CANDLE_DOWN,
            candle_neutral: [128, 128, 128],
            candle_fill_alpha: 0.85,
            // The five numbers below reproduce the literals the shaders used to carry, so a fresh
            // install renders byte-identically to the version before they became configurable:
            // last was vec4(0.82, 0.60, 0.36, 0.82), mark was vec4(0.42, 0.72, 1.00, 0.78), and
            // the line half-width was a fixed 0.85 px (hence 1.7 full width).
            price_line: [209, 153, 92],
            price_line_alpha: 0.82,
            mark_line: [107, 184, 255],
            mark_line_alpha: 0.78,
            price_line_px: 1.7,
            book_bg: [30, 30, 30],
            // The two halves of the order book are lightly tinted by side; the spread gap
            // between the best bid/ask remains the neutral book_bg.
            book_bg_ask: [42, 30, 27],
            book_bg_bid: [30, 36, 26],
            // The depth wall is drawn at a fixed alpha of 1.0 by the book shaders, so its weight
            // is set here, in the colour: each side is its candle tone composited at ~40% over
            // `book_bg`, which reads as a side panel rather than a wall.
            book_bid: [33, 84, 80],
            book_ask: [114, 51, 50],
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
        // Same desaturated pair as the dark set: these two tones were chosen to carry on either
        // background, so the light theme no longer needs a saturated primary of its own.
        self.candle_up = palette::CANDLE_UP;
        self.candle_down = palette::CANDLE_DOWN;
        self.candle_neutral = [150, 150, 150];
        self.candle_fill_alpha = 0.85;
        // Only the colours are overridden: the dark tan and light blue both wash out on white.
        // The sizes and opacities read the same in either mode, so they are deliberately absent.
        //
        // The bottom-volume opacity and scale colour USED to be overridden here as well. They live
        // on `ChartGraphicsCfg` now, which is per chart TAB and therefore has no notion of a theme
        // mode, so that pair no longer switches with the mode. A light-mode user who never tuned
        // them lands on the dark numbers once and can set them back from the chart's palette popup;
        // `theme_legacy` carries the values of anyone who DID tune them.
        //
        // Caveat, and it predates these fields: this runs only from `default_light`, so a user
        // whose `theme.toml` already has a `[light]` table gets the DARK value for any key that
        // table does not mention (serde fills a missing key from `ChartTheme::default`). Deleting
        // `theme.toml` or the `[light]` table restores these.
        self.price_line = [166, 110, 46];
        self.mark_line = [26, 115, 190];
        self.book_bg = [255, 255, 255];
        self.book_bg_ask = [255, 244, 242];
        self.book_bg_bid = [243, 250, 242];
        // Each candle tone composited at ~35% over white. Not paler: the book shaders brighten a
        // level line to `min(rgb * 1.25, 1)`, and a paler wall clamps those stripes into the
        // background until the individual levels stop reading at all.
        self.book_bid = [179, 224, 220];
        self.book_ask = [249, 195, 194];
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

/// Palette generation [`ChartThemeSet::default`] currently ships.
///
/// Bumped together with an appended entry in [`RETIRED_DARK`] / [`RETIRED_LIGHT`] whenever a
/// shipped default colour is retired. A file below this number is carried across once and stamped;
/// a file at or above it is never touched again.
const CURRENT_PALETTE_REV: u32 = 1;

/// The palette generation of a `theme.toml` written BEFORE that field existed.
///
/// Deliberately NOT [`CURRENT_PALETTE_REV`], and for the same reason
/// [`super::schema::default_version`] is not `SCHEMA_VERSION`: this is what serde substitutes for
/// an ABSENT key, and every file missing it predates the marker and therefore still holds the
/// retired colours. `ChartThemeSet::default` — a fresh install, which ships the current palette —
/// stamps the current rev instead.
fn absent_palette_rev() -> u32 {
    0
}

/// One generation of retired default colours for ONE theme set.
///
/// Only the values this file has STOPPED shipping live here, and they are frozen literals on
/// purpose: dark `candle_up` happens to equal [`palette::GREEN`] and dark `candle_down`
/// [`palette::ORANGE`] today, but a retired value must not follow a token that moves later. The
/// REPLACEMENT is never stored — it is read from the live `Default`, so one pass carries a
/// two-generation-old file all the way to the current colour.
struct RetiredColors {
    candle_up: [u8; 3],
    candle_down: [u8; 3],
    book_bid: [u8; 3],
    book_ask: [u8; 3],
}

/// Dark-set defaults this file no longer ships, oldest generation first. APPEND-ONLY: a later
/// palette change adds an entry whose values are today's defaults.
const RETIRED_DARK: &[RetiredColors] = &[RetiredColors {
    candle_up: [47, 168, 92],
    candle_down: [255, 142, 90],
    book_bid: [75, 86, 48],
    book_ask: [170, 73, 39],
}];

/// Light-set defaults this file no longer ships. See [`RETIRED_DARK`].
const RETIRED_LIGHT: &[RetiredColors] = &[RetiredColors {
    candle_up: [0, 128, 0],
    candle_down: [255, 0, 0],
    book_bid: [0, 128, 0],
    book_ask: [255, 0, 0],
}];

/// Replace every colour in `theme` that still equals a retired default with the current one.
///
/// Compared and swapped PER FIELD, never per struct: dark `label_positive` carries the same bytes
/// as the retired dark `candle_up`, and a struct-wide comparison would migrate a field nobody
/// retired. A value the user chose — anything not byte-identical to a retired default — is left
/// exactly as it was.
///
/// Idempotent: no current default equals any retired value, so a second pass changes nothing.
///
/// Args:
///     theme: The set read off disk, mutated in place.
///     fresh: The current defaults for this same theme mode, the source of every replacement.
///     retired: Every generation of defaults this mode has stopped shipping.
///
/// Returns:
///     Whether any field changed, which is what decides if the file is written back.
fn retire_theme(theme: &mut ChartTheme, fresh: &ChartTheme, retired: &[RetiredColors]) -> bool {
    let mut changed = false;
    let mut swap = |slot: &mut [u8; 3], old: [u8; 3], new: [u8; 3]| {
        if *slot == old {
            *slot = new;
            changed = true;
        }
    };
    for gen in retired {
        swap(&mut theme.candle_up, gen.candle_up, fresh.candle_up);
        swap(&mut theme.candle_down, gen.candle_down, fresh.candle_down);
        swap(&mut theme.book_bid, gen.book_bid, fresh.book_bid);
        swap(&mut theme.book_ask, gen.book_ask, fresh.book_ask);
    }
    changed
}

/// Chart theme SEPARATELY for dark and light UI modes (per theme, like
/// [`super::OrdersStyleSet`]). Stored in one `theme.toml` with `[dark]`/`[light]` tables;
/// `ui_theme_mode` selects the active set.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChartThemeSet {
    /// Which palette generation this file's colours belong to; see [`CURRENT_PALETTE_REV`].
    ///
    /// Declared FIRST because it must serialise before the two tables — TOML takes every key after
    /// a table header as belonging to that table. Not user-facing and absent from Settings: it
    /// records what the app already did, it is not a choice anyone makes.
    #[serde(default = "absent_palette_rev")]
    pub palette_rev: u32,
    pub dark: ChartTheme,
    pub light: ChartTheme,
}

impl Default for ChartThemeSet {
    fn default() -> Self {
        Self {
            palette_rev: CURRENT_PALETTE_REV,
            dark: ChartTheme::default(),
            light: ChartTheme::default_light(),
        }
    }
}

impl ChartThemeSet {
    /// Carry both sets off an older palette generation onto the current defaults, ONCE.
    ///
    /// Gated on [`Self::palette_rev`], not on the colour values alone. A value gate would be
    /// repeatable, which would make every retired colour impossible to KEEP: the Settings pickers
    /// and the supported Moonbot import both write these four fields
    /// (`super::moonbot_import::apply`), and Moonbot's own `CandleGreen` is byte-identical to a
    /// retired light default. The stamp is what lets a user deliberately choose an old colour and
    /// still have it there after a restart.
    ///
    /// See [`retire_theme`] for the per-field rule.
    ///
    /// Returns:
    ///     Whether anything changed, which is what decides if `theme.toml` is written back.
    fn retire_old_defaults(&mut self) -> bool {
        if self.palette_rev >= CURRENT_PALETTE_REV {
            return false;
        }
        retire_theme(&mut self.dark, &ChartTheme::default(), RETIRED_DARK);
        retire_theme(&mut self.light, &ChartTheme::default_light(), RETIRED_LIGHT);
        // Stamped even when no colour moved: the file has now been SEEN by this generation, and
        // without the stamp a user who had already customised all four would be re-examined, and
        // re-written, on every single launch.
        self.palette_rev = CURRENT_PALETTE_REV;
        true
    }

    /// Set for the active mode: `light=true` → light, otherwise dark.
    pub fn get(&self, light: bool) -> &ChartTheme {
        if light { &self.light } else { &self.dark }
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
        let text = std::fs::read_to_string(paths::theme_path()).ok();
        let (set, write_back) = Self::resolve(text.as_deref());
        if write_back {
            let _ = set.save();
        }
        set
    }

    /// The whole load DECISION, over the file's TEXT rather than the file.
    ///
    /// Split out purely so it can be exercised, exactly as
    /// [`super::theme_legacy::read_legacy_from`] was: [`Self::load`] reads
    /// [`paths::theme_path`] and [`Self::save`] writes it, so a test of the branch behaviour
    /// through `load` would read and then OVERWRITE the developer's own `theme.toml`. Everything
    /// worth asserting lives here; `load` is left holding nothing but the two file operations.
    ///
    /// Args:
    ///     text: The file's contents, or `None` when it is missing or could not be read. The two
    ///         are deliberately not distinguished — that is the behaviour this file has always
    ///         had, and changing it is not in this function's remit.
    ///
    /// Returns:
    ///     The set to install, and whether it must be written back. The flag is what keeps a
    ///     marker-shaped file that neither moved generation nor needed reshaping from being
    ///     rewritten on every launch.
    fn resolve(text: Option<&str>) -> (Self, bool) {
        let Some(text) = text else {
            return (Self::default(), true);
        };
        if text.contains("[dark") || text.contains("[light") {
            let mut set: Self = match toml::from_str(text) {
                Ok(set) => set,
                Err(e) => {
                    // Unchanged: a marker-shaped file that will not parse yields the default and
                    // is NOT written back, so the damaged file survives for the user to inspect.
                    log::warn!("theme.toml повреждён ({e}); беру дефолт");
                    return (Self::default(), false);
                }
            };
            // This branch never re-saved before, so it still writes only when the palette
            // generation actually moved. An already-migrated or fully custom file costs no write.
            let moved = set.retire_old_defaults();
            return (set, moved);
        }
        // Markerless text: the legacy FLAT shape, or text that does not parse at all and becomes
        // the default flat theme. Both have always been rewritten in the new shape here, and both
        // still are — the write-back below is unconditional, exactly as before.
        let mut flat: ChartTheme = toml::from_str(text).unwrap_or_default();
        // Scoped to the flat side deliberately: `light` below is built fresh from the current
        // defaults and cannot hold a retired byte, and a flat file predates the marker by
        // construction, so there is no generation to gate on either.
        retire_theme(&mut flat, &ChartTheme::default(), RETIRED_DARK);
        let set = Self {
            palette_rev: CURRENT_PALETTE_REV,
            dark: flat,
            light: ChartTheme::default_light(),
        };
        (set, true)
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
            let mut set: Self = toml::from_str(text).ok()?;
            // The condition above is an OR, so ONE table is enough to land here. serde fills the
            // other from the SHIPPED default rather than from the user's live theme, which
            // silently discards the side they never sent — the exact loss the flat branch below
            // was written to avoid. Restore it the same way. (Pre-dates the palette work; the
            // "Copy" button always emits both tables, so only a hand-made paste reaches it.)
            let sent_dark = table_has("dark");
            let sent_light = table_has("light");
            if !sent_dark {
                set.dark = current.dark.clone();
            }
            if !sent_light {
                set.light = current.light.clone();
            }
            // Carried across on the PASTED file's own generation — a paste is "load this file",
            // and `load` would do it on the next launch anyway. Only the sides actually sent are
            // touched: a side taken from `current` has already been through its own carry-over and
            // may legitimately hold a retired colour the user chose afterwards.
            let pasted_rev = set.palette_rev;
            if pasted_rev < CURRENT_PALETTE_REV {
                if sent_dark {
                    retire_theme(&mut set.dark, &ChartTheme::default(), RETIRED_DARK);
                }
                if sent_light {
                    retire_theme(&mut set.light, &ChartTheme::default_light(), RETIRED_LIGHT);
                }
            }
            // The generation the PASTED side now sits at: carried up to the current one just
            // above, or already past it when the file came from a newer build.
            let sent_rev = pasted_rev.max(CURRENT_PALETTE_REV);
            set.palette_rev = if sent_dark && sent_light {
                sent_rev
            } else {
                // MIXED set: one side from the paste, one from the live theme, and they may sit at
                // DIFFERENT generations. `palette_rev` describes the whole set, so it has to be the
                // OLDER of the two — the set is only as migrated as its least migrated half.
                // Stamping the pasted side's generation over an older live side would tell a future
                // build that the live side needs nothing, and it would keep its stale colours
                // forever, which is the one failure this marker exists to prevent.
                sent_rev.min(current.palette_rev)
            };
            return Some(set);
        }
        if v.as_table()
            .is_some_and(|t| KEYS.iter().any(|k| t.contains_key(*k)))
        {
            let mut flat: ChartTheme = toml::from_str(text).ok()?;
            // ONLY the pasted dark side, which is why this does not go through
            // `retire_old_defaults`. `current.light` is the user's own live theme, which this
            // branch promises to preserve untouched; carrying it across here would let a paste of
            // an unrelated flat DARK theme rewrite a light colour the user never sent — and it may
            // legitimately hold a retired colour the user chose after their own migration ran.
            retire_theme(&mut flat, &ChartTheme::default(), RETIRED_DARK);
            return Some(Self {
                palette_rev: CURRENT_PALETTE_REV,
                dark: flat,
                light: current.light.clone(),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests;
