//! Detection-feed display settings: EACH card size (mini/medium/large) has its own dimensions,
//! chart type, colored server rail, and SLOT-BASED field layout — each slot receives a field plus
//! "over chart"/"right"/"below chart" flags. Persistence uses a SEPARATE portable
//! `detects_view.toml` (per group, keyed by group name; like theme.toml/badges.json, it can be
//! copied between users). Old `layout.toml::detect_view_by_group` entries (the checkbox-based
//! scheme) are not migrated; a missing entry uses the design-spec default.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{paths, toml_io};

/// Detection-card size.
pub const DETECT_SIZE_MINI: u8 = 0;
pub const DETECT_SIZE_MEDIUM: u8 = 1;
pub const DETECT_SIZE_LARGE: u8 = 2;

/// Maximum number of slots (large). Mini/medium use the first 4/6 slots of the same array —
/// one configuration type for all sizes (and `Copy` without a Vec).
pub const DETECT_MAX_SLOTS: usize = 9;

/// Number of slots by size: mini 4 (2 rows × 2), medium 6 (2 rows × 3), large 9.
pub fn detect_slot_count(size: u8) -> usize {
    match size {
        DETECT_SIZE_MINI => 4,
        DETECT_SIZE_MEDIUM => 6,
        _ => DETECT_MAX_SLOTS,
    }
}

/// Maximum width of the colored server rail, in pixels.
pub const DETECT_RAIL_MAX: u8 = 5;

/// Card mini-chart type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectChart {
    /// No chart.
    None,
    /// Frozen 5-minute candles (~2 hours).
    #[default]
    Candles,
    /// 24-hour price line.
    Line,
}

/// Field assigned to a card slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectField {
    /// Empty slot.
    #[default]
    None,
    /// Coin token (large monospace label).
    Coin,
    /// Countdown ("Ns").
    Time,
    /// Detection-type badge.
    Badge,
    /// Core-name badge.
    Core,
    /// 24-hour delta, %.
    Delta24h,
    /// 1-hour delta, %.
    Delta1h,
    /// Exchange name.
    Exchange,
    /// Exchange kind (spot/futures/…).
    ExchangeKind,
}

impl DetectField {
    /// All assignable fields (slot-dropdown order; `None` = "—").
    pub const ALL: [DetectField; 9] = [
        DetectField::None,
        DetectField::Coin,
        DetectField::Time,
        DetectField::Badge,
        DetectField::Core,
        DetectField::Delta24h,
        DetectField::Delta1h,
        DetectField::Exchange,
        DetectField::ExchangeKind,
    ];
}

/// One layout slot. For mini/medium, the row position (top/bottom) is set by the slot INDEX
/// (first half = top, second half = bottom); for large, the `below` flag selects the row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectSlot {
    /// What to display (None = empty slot).
    pub field: DetectField,
    /// Whether to draw over the chart (with a backing) rather than in the text area. Applies only
    /// when the chart is enabled.
    pub over: bool,
    /// Whether to align to the right edge of its area (otherwise left).
    pub right: bool,
    /// Large size: below the chart (otherwise above). Ignored by mini/medium.
    pub below: bool,
}

impl DetectSlot {
    const fn new(field: DetectField, over: bool, right: bool, below: bool) -> Self {
        Self {
            field,
            over,
            right,
            below,
        }
    }
}

/// Settings for ONE card size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectSizeCfg {
    /// Card dimensions in logical pixels (before UI scaling).
    pub w: u16,
    pub h: u16,
    /// Mini-chart type (the mini size never draws a chart, regardless of this value).
    pub chart: DetectChart,
    /// Width of the colored server rail on the left, in pixels (0 = off, max 5).
    pub rail_w: u8,
    /// Width of the gradient fade from the rail, in pixels (0 = off, max = card width).
    pub rail_grad: u16,
    /// Layout slots (the first [`detect_slot_count`] entries are used).
    pub slots: [DetectSlot; DETECT_MAX_SLOTS],
}

impl Default for DetectSizeCfg {
    fn default() -> Self {
        // Not used directly (each size has its own default in DetectViewCfg), but required
        // by serde(default) for partially populated TOML entries.
        Self {
            w: 184,
            h: 44,
            chart: DetectChart::Candles,
            rail_w: 3,
            rail_grad: 20,
            slots: [DetectSlot::default(); DETECT_MAX_SLOTS],
        }
    }
}

impl DetectSizeCfg {
    /// Rail width clamped to 0..=5.
    pub fn rail_w_clamped(&self) -> u8 {
        self.rail_w.min(DETECT_RAIL_MAX)
    }

    /// Gradient width clamped to 0..=w.
    pub fn rail_grad_clamped(&self) -> u16 {
        self.rail_grad.min(self.w)
    }
}

const F: fn(DetectField, bool, bool, bool) -> DetectSlot = DetectSlot::new;
const EMPTY: DetectSlot = DetectSlot::new(DetectField::None, false, false, false);

// Defaults = the user's working set from 2026-07-15 (captured from their detects_view.toml).

fn default_mini() -> DetectSizeCfg {
    DetectSizeCfg {
        w: 100,
        h: 40,
        chart: DetectChart::None,
        rail_w: 3,
        rail_grad: 30,
        // Top row: coin on the left, time on the right; bottom: badge on the left, core on the right.
        slots: [
            F(DetectField::Coin, false, false, false),
            F(DetectField::Time, false, true, false),
            F(DetectField::Badge, false, false, false),
            F(DetectField::Core, false, true, false),
            EMPTY,
            EMPTY,
            EMPTY,
            EMPTY,
            EMPTY,
        ],
    }
}

fn default_medium() -> DetectSizeCfg {
    DetectSizeCfg {
        w: 210,
        h: 44,
        chart: DetectChart::Line,
        rail_w: 3,
        rail_grad: 30,
        // Top: coin and time on the left, Δ24 on the right; bottom: badge and core on the left, Δ1 on the right.
        slots: [
            F(DetectField::Coin, false, false, false),
            F(DetectField::Time, false, false, false),
            F(DetectField::Delta24h, false, true, false),
            F(DetectField::Badge, false, false, false),
            F(DetectField::Core, false, false, false),
            F(DetectField::Delta1h, false, true, false),
            EMPTY,
            EMPTY,
            EMPTY,
        ],
    }
}

fn default_large() -> DetectSizeCfg {
    DetectSizeCfg {
        w: 140,
        h: 100,
        chart: DetectChart::Candles,
        rail_w: 3,
        rail_grad: 30,
        // Above the chart: coin+badge on the left, Δ24 on the right, time overlaid; below:
        // core on the left, Δ1 on the right.
        slots: [
            F(DetectField::Coin, false, false, false),
            F(DetectField::Badge, false, false, false),
            F(DetectField::Delta24h, false, true, false),
            F(DetectField::Time, true, false, false),
            F(DetectField::None, true, false, false),
            EMPTY,
            F(DetectField::Core, false, false, true),
            F(DetectField::None, false, false, true),
            F(DetectField::Delta1h, false, true, true),
        ],
    }
}

/// Detection-feed display for one group: active size plus settings for all three sizes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectViewCfg {
    /// Active card size: 0=mini, 1=medium, 2=large.
    pub size: u8,
    /// Decimal places for deltas (Δ24h/Δ1h), 0..=2 — ONE setting for all sizes.
    pub delta_decimals: u8,
    /// Whether detects routed to a chart tab (`AddToChart > 0`) also appear in the feed — ONE
    /// setting for all sizes. They are ordinary detects in every other respect: the feed still
    /// requires `SoundAlert` or an alert firing, and the card still lives for `KeepAlert` seconds.
    /// Off keeps the historical behavior, where a chart tab consumed them alone.
    pub show_add_to_chart: bool,
    pub mini: DetectSizeCfg,
    pub medium: DetectSizeCfg,
    pub large: DetectSizeCfg,
}

impl Default for DetectViewCfg {
    fn default() -> Self {
        Self {
            size: DETECT_SIZE_MEDIUM,
            delta_decimals: 1,
            show_add_to_chart: false,
            mini: default_mini(),
            medium: default_medium(),
            large: default_large(),
        }
    }
}

impl DetectViewCfg {
    /// Normalized size (clamped to the valid range).
    pub fn size_clamped(&self) -> u8 {
        self.size.min(DETECT_SIZE_LARGE)
    }

    /// Number of delta decimal places, clamped to 0..=2.
    pub fn delta_decimals_clamped(&self) -> usize {
        self.delta_decimals.min(2) as usize
    }

    /// Settings for a specific size.
    pub fn size_cfg(&self, size: u8) -> &DetectSizeCfg {
        match size {
            DETECT_SIZE_MINI => &self.mini,
            DETECT_SIZE_MEDIUM => &self.medium,
            _ => &self.large,
        }
    }

    pub fn size_cfg_mut(&mut self, size: u8) -> &mut DetectSizeCfg {
        match size {
            DETECT_SIZE_MINI => &mut self.mini,
            DETECT_SIZE_MEDIUM => &mut self.medium,
            _ => &mut self.large,
        }
    }

    /// Settings for the active size.
    pub fn active(&self) -> &DetectSizeCfg {
        self.size_cfg(self.size_clamped())
    }

    /// Single-group text in detects_view.toml format for "Copy" in the popup.
    pub fn to_share_string(&self) -> Option<String> {
        toml::to_string_pretty(self).ok()
    }

    /// Parses "Paste" text (one group's configuration). Invalid TOML yields None; valid TOML
    /// uses serde defaults for fields it does not contain.
    pub fn parse_share(text: &str) -> Option<Self> {
        toml::from_str::<Self>(text).ok()
    }
}

/// `detects_view.toml` file: detection-feed settings by group (keyed by group name).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectViewFile {
    pub groups: HashMap<String, DetectViewCfg>,
}

impl DetectViewFile {
    pub fn load() -> Self {
        toml_io::load_or_default(&paths::detects_view_path(), "detects_view.toml", |_| {})
    }

    pub fn save(&self) {
        if let Err(e) = toml_io::save(&paths::detects_view_path(), self, "detects_view.toml") {
            log::warn!("не записал detects_view.toml: {e:#}");
        }
    }

    /// Group settings (or the design-spec default).
    pub fn group(&self, group: &str) -> DetectViewCfg {
        self.groups.get(group).copied().unwrap_or_default()
    }

    pub fn set_group(&mut self, group: &str, cfg: DetectViewCfg) {
        self.groups.insert(group.to_string(), cfg);
    }

    /// Whether one group's feed shows chart-routed detects.
    ///
    /// Separate from [`Self::group`] because this answer is wanted on every backend notification,
    /// while `group` copies the whole configuration and builds three size layouts for a group that
    /// has none.
    ///
    /// Args:
    ///     group: Window-group name the feed belongs to.
    ///
    /// Returns:
    ///     `true` when that group asked for `AddToChart` detects; the default is `false`.
    pub fn shows_add_to_chart(&self, group: &str) -> bool {
        self.groups.get(group).is_some_and(|c| c.show_add_to_chart)
    }
}

#[cfg(test)]
mod tests;
