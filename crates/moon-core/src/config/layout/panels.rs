//! Persisted panel placement and header ticker selection.

use super::*;

/// "Strategies" window panels: widths, their ownership, and the Versions collapsed state.
/// Width values are clamped by the window when applied.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategiesPanels {
    pub tree_w: f32,
    pub versions_w: f32,
    pub sections_w: f32,
    /// Whether a splitter drag made the stored widths user-owned.
    ///
    /// `false` (also what a layout serialized before this field deserializes to) means the
    /// widths are responsive defaults recomputed from the window each frame; the first drag
    /// sets it and the stored widths are never recomputed again.
    pub widths_user_set: bool,
    pub versions_collapsed: bool,
}

impl Default for StrategiesPanels {
    /// Return a first-run panel layout whose tree and section widths remain responsive until drag.
    fn default() -> Self {
        Self {
            tree_w: 418.0,
            versions_w: 166.0,
            sections_w: 264.0,
            widths_user_set: false,
            // By default, the versions column is collapsed into a strip with a counter.
            versions_collapsed: true,
        }
    }
}

/// Group-window geometry plus legacy egui compatibility state (map key = group name).
#[derive(Clone, Serialize, Deserialize)]
pub struct GroupLayout {
    /// Outer window position (physical desktop pixels).
    pub x: i32,
    pub y: i32,
    /// Inner size (physical pixels).
    pub w: u32,
    pub h: u32,
    #[serde(default)]
    pub maximized: bool,
    /// macOS fullscreen state (WindowBounds::Fullscreen). Separate from `maximized`:
    /// the green macOS button produces Fullscreen rather than Maximized, and it must be
    /// restored using its own variant or the window will open normally.
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default)]
    /// Legacy egui dock-collapsed state.
    pub collapsed: bool,
    /// Legacy egui active dock-tab index.
    #[serde(default)]
    pub tab: u8,
    /// Legacy expanded-dock height (egui points). 0 = unspecified → default.
    #[serde(default)]
    pub dock_h: f32,
    /// Legacy egui order sorting: 0=by creation, 1=Sell first, 2=Buy first.
    #[serde(default)]
    pub orders_primary: u8,
    /// Legacy egui time sorting for orders: newest first.
    #[serde(default = "def_true")]
    pub orders_newest_first: bool,
    /// Legacy egui "current market only" order filter.
    #[serde(default)]
    pub orders_only_current: bool,
    /// Legacy egui order-kind filter: 0=all, 1=real, 2=emulated.
    #[serde(default)]
    pub orders_kind: u8,
    /// Window display UUID (`PlatformDisplay::uuid`) as a string. On macOS, window coordinates
    /// are display-relative, so x/y cannot restore the display; only the UUID can.
    /// Point-containment detection remains the fallback for old layouts without this field.
    #[serde(default)]
    pub display_uuid: Option<String>,
}

/// Visible-column masks of the Tuning strategy list, ONE PER AXIS.
///
/// The list stands beside a different tool in each mode, so it is asked a different question in
/// each: "By coin" wants the strategy's coin-list counts, the other two want the width those
/// columns take. Named fields rather than an array — the axes are an enum, and an index would
/// silently re-point every saved mask the day their order changes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StratColsByMode {
    pub filter: u16,
    pub coins: u16,
    pub time: u16,
    /// The "Entry/Exit" axis. Added after the key shipped, so a file saved before it has no
    /// value here: `None` is "never chosen" and the UI substitutes the axis default, while a
    /// saved `Some(0)` is the deliberate all-hidden mask the other three slots also allow.
    pub ticks: Option<u16>,
}

impl Default for StratColsByMode {
    /// Zero is a legitimate mask ("no toggleable column"), so the absent-key default cannot be
    /// `0` — the UI substitutes its own defaults when the whole key is missing instead.
    fn default() -> Self {
        Self {
            filter: 0,
            coins: 0,
            time: 0,
            ticks: None,
        }
    }
}

/// Remembered split placement for a panel: which split (by anchor neighbors), which index, which
/// side, and which slot sizes it occupied, so it can return to THE SAME position and retain its
/// previous proportions (important for splits with 3+ panels).
#[derive(Clone, Serialize, Deserialize)]
pub struct DockSplitSlot {
    /// All split neighbors (except the panel itself), used as anchors to find the correct split on
    /// return; any one present in the dock is sufficient. Stored in canonical split order.
    #[serde(default)]
    pub siblings: Vec<String>,
    /// Panels in the NEIGHBORING slot (beside which the panel stood). That slot may have been a nested
    /// split (column), so it is wrapped as a whole when recreating the split. Empty → use siblings.
    #[serde(default)]
    pub slot_panels: Vec<String>,
    /// Panel index in the split at detach time, used to insert it back in the same position
    /// (clamped to the number of slots). Important for splits with 3+ panels.
    #[serde(default)]
    pub index: usize,
    /// Panel side relative to its neighbor in a COLLAPSED split (2 panels): 0=Left, 1=Right,
    /// 2=Top, 3=Bottom (matches `moon_ui::DockSplitPlacement`).
    pub placement: u8,
    /// Pixel size of the PANEL slot along the split axis at detach time. 0.0 = flex (no fixed size).
    #[serde(default)]
    pub size: f32,
    /// Pixel size of the NEIGHBOR slot along the split axis (for a collapsed split). 0.0 = flex.
    #[serde(default)]
    pub sibling_size: f32,
}

/// Header price-ticker source selection. The core is stored by stable server `uid`
/// (survives configuration reordering), and the market by the core's canonical name.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderTicker {
    // wire-id-exempt: terminal-issued, never a core id — see `config::wire_id`.
    pub core_uid: u64,
    pub market: String,
}
