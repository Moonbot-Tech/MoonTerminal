//! Detection-type badges — a portable `badges.json` file (modeled after
//! `orders.toml`/`figures.json`). For each strategy kind (`StrategyKind`
//! ordinal), it specifies whether to draw the badge (`active`), a short code (≤3 characters,
//! not necessarily letters) SEPARATELY for long/short (when `distinguish_dir` is enabled), and
//! a color SEPARATELY for the dark/light theme. The outline is PER ROW: an `outline` toggle plus
//! theme-specific long/short colors.
//!
//! The list of kinds grows over time — new ones are added to the UI by ordinal;
//! unconfigured kinds fall back to the neutral `UNK` code at runtime.

use serde::{Deserialize, Serialize};

use super::paths;
use super::write_file_atomic;

/// `0xRRGGBB` → sRGB `[u8;3]`.
const fn c(hex: u32) -> [u8; 3] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

/// Neutral (muted) fallback color for unconfigured kinds.
const MUTED_DARK: [u8; 3] = c(0x97928A);
const MUTED_LIGHT: [u8; 3] = c(0x4F5B68);

// Default direction-outline colors per theme (long = green, short = red).
const OUT_LONG_DARK: [u8; 3] = c(0x2FA85C);
const OUT_LONG_LIGHT: [u8; 3] = c(0x168A49);
const OUT_SHORT_DARK: [u8; 3] = c(0xFF4A4A);
const OUT_SHORT_LIGHT: [u8; 3] = c(0xD92D3A);

/// One detection-type badge.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BadgeEntry {
    /// Strategy-kind ordinal (`StrategyKind`). Key that associates the badge with a detection.
    pub ordinal: u8,
    /// Human-readable kind name (for the settings UI row).
    pub name: String,
    /// Whether to draw a badge for this detection type. False hides the badge.
    pub active: bool,
    /// Badge code (for long, and for both directions when `distinguish_dir=false`). ≤3.
    pub code: String,
    /// Whether to distinguish the code by direction: when true, short uses `code_short`.
    pub distinguish_dir: bool,
    /// Badge code for SHORT (used only when `distinguish_dir` is enabled). ≤3.
    pub code_short: String,
    /// Badge color in the dark theme.
    pub color_dark: [u8; 3],
    /// Badge color in the light theme.
    pub color_light: [u8; 3],
    /// Whether to draw the badge outline (per row). When true, outline colors depend on direction.
    pub outline: bool,
    pub outline_long_dark: [u8; 3],
    pub outline_long_light: [u8; 3],
    pub outline_short_dark: [u8; 3],
    pub outline_short_light: [u8; 3],
}

impl Default for BadgeEntry {
    fn default() -> Self {
        Self {
            ordinal: 0,
            name: String::new(),
            active: true,
            code: "UNK".to_string(),
            distinguish_dir: false,
            code_short: String::new(),
            color_dark: MUTED_DARK,
            color_light: MUTED_LIGHT,
            outline: false,
            outline_long_dark: OUT_LONG_DARK,
            outline_long_light: OUT_LONG_LIGHT,
            outline_short_dark: OUT_SHORT_DARK,
            outline_short_light: OUT_SHORT_LIGHT,
        }
    }
}

impl BadgeEntry {
    /// Badge color for the active theme.
    pub fn color(&self, is_light: bool) -> [u8; 3] {
        if is_light {
            self.color_light
        } else {
            self.color_dark
        }
    }

    /// Badge code for a direction: use `code_short` for short when `distinguish_dir` is enabled
    /// (if it is non-empty); otherwise use the primary `code`.
    pub fn code_for(&self, is_short: bool) -> &str {
        if self.distinguish_dir && is_short && !self.code_short.is_empty() {
            &self.code_short
        } else {
            &self.code
        }
    }

    /// Outline color for the direction and theme (when the outline is enabled).
    pub fn outline_color(&self, is_short: bool, is_light: bool) -> Option<[u8; 3]> {
        if !self.outline {
            return None;
        }
        Some(match (is_short, is_light) {
            (true, false) => self.outline_short_dark,
            (true, true) => self.outline_short_light,
            (false, false) => self.outline_long_dark,
            (false, true) => self.outline_long_light,
        })
    }
}

/// Maximum remembered custom colours. Matches the MoonUI colour-picker widget's own
/// `MAX_CUSTOM_COLORS`; a larger stored/seeded list here would just be silently trimmed the next
/// time a picker seeds itself from it.
const CUSTOM_COLORS_MAX: usize = 20;

/// Detection-badge configuration (portable `badges.json`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BadgesConfig {
    /// Entries by strategy kind (ordinal → code/colors/outline).
    pub entries: Vec<BadgeEntry>,
    /// Colours typed into a badge colour picker's hex field, most recent first. A picker-wide
    /// reuse palette for the whole Badges tab (shared by every row's colour/outline pickers), not
    /// per-row data — hence not a field on `BadgeEntry`.
    pub custom_colors: Vec<[u8; 3]>,
}

/// Default 24 kinds: `(ordinal, name, code, dark_color, light_color)`. Colors come from
/// the `MoonTone` families (see the badge swatch). Kept here as the single source of truth —
/// written to `badges.json` on first launch and edited in Settings thereafter.
#[rustfmt::skip]
const DEFAULTS: &[(u8, &str, &str, u32, u32)] = &[
    // Entry / "moon" signals — Positive (green)
    (2,  "Drops",         "DRP", 0x2FA85C, 0x168A49),
    (6,  "MoonShot",      "SHT", 0x2FA85C, 0x168A49),
    (13, "MoonStrike",    "STR", 0x2FA85C, 0x168A49),
    (20, "MoonHook",      "HOK", 0x2FA85C, 0x168A49),
    (14, "New Listing",   "NEW", 0x2FA85C, 0x168A49),
    // Momentum / volume — Warning (amber)
    (4,  "Volumes",       "VOL", 0xFFB347, 0xB97800),
    (5,  "PumpDetection", "PMP", 0xFFB347, 0xB97800),
    (8,  "Delta",         "DLT", 0xFFB347, 0xB97800),
    (9,  "Waves",         "WAV", 0xFFB347, 0xB97800),
    (21, "Activity",      "ACT", 0xFFB347, 0xB97800),
    // Order book / structure — Info (blue)
    (3,  "Walls",         "WAL", 0x7FC9FF, 0x126CBF),
    (19, "Chart Wall",    "CHW", 0x7FC9FF, 0x126CBF),
    (18, "Spread",        "SPR", 0x7FC9FF, 0x126CBF),
    // Risk — Danger (red)
    (15, "Liquidations",  "LIQ", 0xFF4A4A, 0xD92D3A),
    // Indicator / derivatives — Accent
    (17, "EMA",           "EMA", 0xD2691E, 0xB95C18),
    (7,  "V Lite",        "VLT", 0xD2691E, 0xB95C18),
    (16, "TopMarket",     "TOP", 0xD2691E, 0xB95C18),
    (10, "Combo",         "CMB", 0xD2691E, 0xB95C18),
    // External signal — Notice (yellow)
    (1,  "Telegram",      "TLG", 0xFFD93D, 0xB48A00),
    (11, "UDP",           "UDP", 0xFFD93D, 0xB48A00),
    // Meta / utility — Muted
    (12, "Manual",        "MAN", 0x97928A, 0x4F5B68),
    (22, "Alerts",        "ALR", 0x97928A, 0x4F5B68),
    (23, "Watcher",       "WCH", 0x97928A, 0x4F5B68),
    // Utility
    (0,  "Unknown",       "UNK", 0xE8E4DC, 0x18202A),
];

impl Default for BadgesConfig {
    fn default() -> Self {
        Self {
            entries: DEFAULTS
                .iter()
                .map(|&(ordinal, name, code, cd, cl)| BadgeEntry {
                    ordinal,
                    name: name.to_string(),
                    code: code.to_string(),
                    color_dark: c(cd),
                    color_light: c(cl),
                    ..BadgeEntry::default()
                })
                .collect(),
            custom_colors: Vec::new(),
        }
    }
}

impl BadgesConfig {
    /// Reads `badges.json`. A corrupt file yields the default; a missing file creates and saves
    /// the default so the user has something to edit manually, as in `OrdersStyleSet::load`.
    pub fn load() -> Self {
        let path = paths::badges_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<BadgesConfig>(&s) {
                Ok(cfg) => cfg,
                Err(e) => {
                    log::warn!("badges.json битый ({e}) — беру дефолт");
                    Self::default()
                }
            },
            Err(_) => {
                let cfg = Self::default();
                cfg.save();
                cfg
            }
        }
    }

    /// Writes `badges.json` atomically. Failures are non-fatal and are logged.
    pub fn save(&self) {
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) =
                    write_file_atomic(&paths::badges_path(), s.as_bytes(), "badges.json")
                {
                    log::warn!("не записал badges.json: {e:#}");
                }
            }
            Err(e) => log::warn!("сериализация badges.json не удалась: {e}"),
        }
    }

    /// Text in badges.json format for "Copy" in Settings (= file contents).
    pub fn to_share_string(&self) -> Option<String> {
        serde_json::to_string_pretty(self).ok()
    }

    /// Parses badges.json text (clipboard paste / file contents). Requires an `entries`
    /// array; otherwise any JSON would silently produce the default (`serde(default)`).
    /// `None` means the text is not a badge configuration.
    ///
    /// `custom_colors` is always kept from `current` rather than the pasted text: it is local
    /// reuse history, not a shared setting, and `to_share_string` writes the same struct back out
    /// — so without this, pasting a colleague's `badges.json` (whose own history is unrelated, or
    /// simply absent under `#[serde(default)]`) would silently wipe the local user's remembered
    /// custom colours.
    ///
    /// Args:
    ///     text: Pasted `badges.json` text to validate and deserialize.
    ///     current: Local configuration whose custom-colour reuse history must survive the paste.
    ///
    /// Returns:
    ///     Parsed shared badge settings with `current.custom_colors`, or `None` when `text` does
    ///     not contain a badge `entries` array.
    pub fn parse_share(text: &str, current: &Self) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        v.get("entries")?.as_array()?;
        let mut parsed: Self = serde_json::from_str(text).ok()?;
        parsed.custom_colors = current.custom_colors.clone();
        Some(parsed)
    }

    /// Remember a colour typed into a badge colour picker's hex field, most-recent-first, capped
    /// at `CUSTOM_COLORS_MAX` (dropping the oldest). Mirrors the MoonUI widget's own dedupe/cap so
    /// the persisted list and a freshly-seeded picker never disagree on order.
    ///
    /// Args:
    ///     color: RGB value the user committed through a picker hex field.
    ///
    /// Returns:
    ///     Whether the list actually changed — `false` when `color` was already the front entry.
    pub fn remember_custom_color(&mut self, color: [u8; 3]) -> bool {
        if self.custom_colors.first() == Some(&color) {
            return false;
        }
        self.custom_colors.retain(|c| *c != color);
        self.custom_colors.insert(0, color);
        self.custom_colors.truncate(CUSTOM_COLORS_MAX);
        true
    }

    /// Entry by ordinal (first match).
    pub fn entry(&self, ordinal: u8) -> Option<&BadgeEntry> {
        self.entries.iter().find(|e| e.ordinal == ordinal)
    }

    /// Whether to draw the badge for a kind (an unconfigured kind shows UNK).
    pub fn active(&self, ordinal: u8) -> bool {
        self.entry(ordinal).map(|e| e.active).unwrap_or(true)
    }

    /// Badge code for a kind and direction (fallback: `UNK`).
    pub fn code(&self, ordinal: u8, is_short: bool) -> &str {
        self.entry(ordinal)
            .map(|e| e.code_for(is_short))
            .unwrap_or("UNK")
    }

    /// Badge color for a kind in the active theme (fallback: neutral).
    pub fn color(&self, ordinal: u8, is_light: bool) -> [u8; 3] {
        self.entry(ordinal)
            .map(|e| e.color(is_light))
            .unwrap_or(if is_light { MUTED_LIGHT } else { MUTED_DARK })
    }

    /// Badge outline color for a direction/theme (None = no outline).
    pub fn outline_color(&self, ordinal: u8, is_short: bool, is_light: bool) -> Option<[u8; 3]> {
        self.entry(ordinal)
            .and_then(|e| e.outline_color(is_short, is_light))
    }
}

#[cfg(test)]
mod tests;
