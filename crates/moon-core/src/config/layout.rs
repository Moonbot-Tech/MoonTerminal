//! Window layout in the portable `layout.toml` file in the config directory. Stores
//! group-window geometry and shared window, chart, and table settings. Live dock and
//! detached-window state lives in `docks.json` and `detached.json`; legacy compatibility
//! fields remain readable. A corrupt or missing file yields the default.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::paths;

/// "Strategies" window panels: widths (tree/versions/sections) + versions collapsed state.
/// Values are clamped by the window when applied.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategiesPanels {
    pub tree_w: f32,
    pub versions_w: f32,
    pub sections_w: f32,
    pub versions_collapsed: bool,
}

impl Default for StrategiesPanels {
    fn default() -> Self {
        Self {
            tree_w: 418.0,
            versions_w: 166.0,
            sections_w: 264.0,
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

fn def_true() -> bool {
    true
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
}

impl Default for StratColsByMode {
    /// Zero is a legitimate mask ("no toggleable column"), so the absent-key default cannot be
    /// `0` — the UI substitutes its own defaults when the whole key is missing instead.
    fn default() -> Self {
        Self {
            filter: 0,
            coins: 0,
            time: 0,
        }
    }
}

/// Window rectangle (outer position + inner size, physical pixels).
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct GeomRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Legacy egui detached-tab compatibility record; live detached state uses `detached.json`.
#[derive(Clone, Serialize, Deserialize)]
pub struct DetachedLayout {
    /// Legacy tab index.
    pub tab: u8,
    /// Legacy owner group name.
    pub owner_group: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Complete window layout.
///
/// Every field is `Option` or carries `#[serde(default)]` on purpose, and prefers a type wider
/// than its values need. This struct is deserialized as a WHOLE, so a single value that does not
/// fit its field's type fails the entire layout — and `load` below passes a no-op corruption
/// handler, so nothing quarantines the file and the first dirty save rewrites it with defaults.
/// One out-of-type integer therefore costs every window position, column width and detached
/// window slot in the file, permanently. Keep that in mind when adding a field.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct WindowLayout {
    /// Group windows by group name.
    #[serde(default)]
    pub groups: HashMap<String, GroupLayout>,
    /// Legacy egui detached-tab records; the live detached-window list uses `detached.json`.
    #[serde(default)]
    pub detached: Vec<DetachedLayout>,
    /// Remembered panel-window geometry after closing, used when the panel is detached again.
    /// Active keys use `panel:<group>/<panel>`; `g:<idx>` and `o:<idx>:<group>` are legacy forms.
    #[serde(default)]
    pub detached_geom: HashMap<String, GeomRect>,
    /// "Strategies" window geometry (separate window), so it reopens in its previous position.
    #[serde(default)]
    pub strategies_window: Option<GeomRect>,
    /// "Strategies" window panels: column widths (logical pixels, resized by splitters)
    /// and "Versions" column collapsed state, persisted like table-column widths.
    #[serde(default)]
    pub strategies_panels: StrategiesPanels,
    /// Global "Assets" window geometry (singleton), so it reopens in its previous position.
    #[serde(default)]
    pub assets_window: Option<GeomRect>,
    /// "Hide assets worth less than N $" threshold (slider in the "Assets" top bar). Shared by all
    /// "Assets" windows/tabs (one value for every scope, avoiding per-scope keys). `0` = show all.
    /// `None` (old file / field was not written) → panel-side default of $1.
    #[serde(default)]
    pub assets_min_value: Option<f64>,
    /// "Settings" window geometry (separate window), so it reopens in its previous position.
    #[serde(default)]
    pub settings_window: Option<GeomRect>,
    /// "Screener" window geometry (singleton), so it reopens in its previous position.
    #[serde(default)]
    pub screener_window: Option<GeomRect>,
    /// "Analytics" window geometry (singleton), so it reopens in its previous position.
    #[serde(default)]
    pub analytics_window: Option<GeomRect>,
    /// Selected "Analytics" period preset (id such as "p-cur-month"), so the window
    /// opens with the previous selection. None = default ("Current month").
    #[serde(default)]
    pub analytics_period: Option<String>,
    /// "Analytics" heatmap mode: "year" (GitHub-style overview) / "month"
    /// (large day cards). None = default ("Month").
    #[serde(default)]
    pub analytics_heat_mode: Option<String>,
    /// Selected period preset for the "Strategy Tuning" tab — its OWN value, independent
    /// from "Summary" (each tab has its own time window). None = default.
    #[serde(default)]
    pub analytics_strat_period: Option<String>,
    /// Bitmask of the visible columns in the Tuning strategy list (the ▦ selector).
    /// None = default (all columns).
    ///
    /// Version 2 of the key. The bit layout is positional (metric columns sit at
    /// `2 + index`), so adding the coin-list columns MOVED every bit above them: a mask
    /// saved under the old layout would silently switch columns on and off rather than
    /// restore what the user chose. A new key is the honest migration — an old config
    /// still loads, and simply falls back to "all columns" once.
    ///
    /// Superseded by [`Self::analytics_strat_cols_modes`], which keeps one mask PER AXIS.
    /// Kept as its seed: a user who already picked their columns carries that pick into all
    /// three axes instead of being reset a second time.
    #[serde(default)]
    pub analytics_strat_cols2: Option<u16>,
    /// Restart count of the "By filter" tuner's threshold search. None = the tuner's default.
    /// Values from an externally edited file are clamped to the range owned by
    /// `db::tuner_smart` when the tuner loads.
    #[serde(default)]
    pub analytics_tuner_iters: Option<u32>,
    /// Quantile depth of the "By filter" tuner's threshold search. None or a value absent from
    /// the dropdown selects the tuner's default.
    #[serde(default)]
    pub analytics_tuner_edges: Option<u32>,
    /// Visible columns of the Tuning strategy list, per axis. None = the UI's own defaults.
    #[serde(default)]
    pub analytics_strat_cols_modes: Option<StratColsByMode>,
    /// Analytics: attribute LIQUIDATION trades to the strategy named in the row.
    ///
    /// Off by default. It moves money between strategies retroactively (measured: 291 of 319
    /// liquidations attach, −4582.89 USDT leaves "Manual"), so it is a decision the user makes
    /// rather than something that quietly changes their history on an update. The Report
    /// window deliberately does NOT follow it.
    #[serde(default)]
    pub analytics_attribute_liq: bool,
    /// The "closed trades the core never dated" banner: the count it was dismissed at.
    ///
    /// `None` — never dismissed, so it shows whenever there is anything to say. Otherwise it
    /// comes back only once MORE such trades appear: the same count is the same news, already
    /// read and put away.
    #[serde(default)]
    pub analytics_undated_hidden_n: Option<i64>,
    /// Visible screener columns (keys in canonical order). None = all.
    #[serde(default)]
    pub screener_columns: Option<Vec<String>>,
    /// Price ticker in the header (left, after the logo): selected core+market. `None` = default
    /// (first connected core; BTCUSDT, or UBTCUSDC on Hyperliquid-like exchanges).
    #[serde(default)]
    pub header_ticker: Option<HeaderTicker>,
    /// Clock in the header's right corner: displayed-time offset from UTC in minutes.
    /// 0 = UTC (default → "(UTC)" label). If it matches the system timezone (displayed time =
    /// system time), the timezone label is hidden. Shared by all windows.
    #[serde(default)]
    pub header_clock_offset_min: i32,
    /// Candle/trade display on charts (timeframe, mode, trade zone, outline, etc.) —
    /// GLOBAL DEFAULT (tabs can override it in their charts.json specification).
    #[serde(default)]
    pub candle_view: crate::market::candles::CandleViewCfg,
    // The former `detect_view_by_group` moved to a separate `detects_view.toml`
    // (see `detect_view::DetectViewFile`); the old layout.toml key is simply ignored.
    /// Chart X time scale (pixels per millisecond) BY GROUP WINDOW: [Shift+middle click] on a chart
    /// synchronizes and saves the scale for charts in ITS OWN window; new charts in that window
    /// inherit it. No entry = 60-second default. Detached windows store their own value in the
    /// tab specification (charts.json).
    #[serde(default)]
    pub chart_x_ppm_by_group: HashMap<String, f32>,
    /// Generic table-column width persistence: `table id → (column key → width in pixels)`.
    /// Every `MoonDataTable` persists its `column_widths` here under a stable id (`orders-table`,
    /// etc.); opening the panel seeds the widths back into it. Empty = default widths.
    #[serde(default)]
    pub table_column_widths: HashMap<String, HashMap<String, f32>>,
    /// Generic persistence for the SET of visible table columns: table id (with `:dock`/`:win`
    /// context) → list of visible-column keys in canonical order. Analogous to
    /// `table_column_widths`, but for field visibility; docked tabs and detached windows have
    /// separate sets. No entry = table default (usually "all visible").
    #[serde(default)]
    pub table_visible_columns: HashMap<String, Vec<String>>,
    /// Panel-tab index in its "home" tab strip at DETACH time, so returning it to the dock restores
    /// THE SAME position rather than the canonical priority position. Key: `group:panel`
    /// (for example, `default:Orders`). No entry → return by priority.
    #[serde(default)]
    pub dock_tab_index: HashMap<String, usize>,
    /// Name of the panel's LEFT NEIGHBOR in the tab strip at DETACH time (empty string = the panel
    /// was leftmost). Returning inserts the panel IMMEDIATELY AFTER that neighbor in the LIVE strip,
    /// so its position remains stable even if the strip changed while it was detached (the raw
    /// [`Self::dock_tab_index`] becomes stale in that case). Key: `group:panel`. Fallback: index.
    #[serde(default)]
    pub dock_tab_left: HashMap<String, String>,
    /// Panel split slot at DETACH time when it occupied a SEPARATE leaf in a split (beside a neighbor,
    /// not in the shared tab row). Detaching such a panel collapses the split, so returning it must
    /// recreate the split beside its neighbor. Key: `group:panel`. Mutually exclusive with
    /// [`Self::dock_tab_index`] (the panel is either in a split or in the tab row).
    #[serde(default)]
    pub dock_split_slot: HashMap<String, DockSplitSlot>,
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
    pub core_uid: u64,
    pub market: String,
}

impl WindowLayout {
    /// Loads layout.toml. A missing file yields the default; a corrupt file is logged and yields the default.
    pub fn load() -> Self {
        super::toml_io::load_or_default(&paths::layout_path(), "layout.toml", |_| {})
    }

    /// Highest core uid this layout still references.
    ///
    /// Feeds the durable uid high-water mark: the header ticker is stored by uid, so reissuing
    /// one a saved layout still names would silently rebind that ticker to the new core.
    pub fn max_core_uid(&self) -> Option<u64> {
        self.header_ticker.as_ref().map(|t| t.core_uid)
    }

    /// Writes layout.toml (non-fatal: errors are only logged).
    pub fn save(&self) {
        if let Err(e) = super::toml_io::save(&paths::layout_path(), self, "layout.toml") {
            log::warn!("{e:#}");
        }
    }
}
