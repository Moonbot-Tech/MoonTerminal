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
    /// Last active trading-core UID in each Main window group.
    ///
    /// A live session with the same stable UID must still belong to the group before the UI uses
    /// the value. Stale entries remain references for the durable UID high-water mark.
    #[serde(default)]
    pub active_trade_core_by_group: HashMap<String, u64>,
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
    /// Standalone "Report" window geometry opened from Analytics.
    #[serde(default, deserialize_with = "de_lenient")]
    pub report_window: Option<GeomRect>,
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
    /// `db::tuner::threshold_search` when the tuner loads.
    #[serde(default, deserialize_with = "de_lenient_u32")]
    pub analytics_tuner_iters: Option<u32>,
    /// Quantile depth of the "By filter" tuner's threshold search. None or a value absent from
    /// the dropdown selects the tuner's default.
    #[serde(default, deserialize_with = "de_lenient_u32")]
    pub analytics_tuner_edges: Option<u32>,
    /// Percentage of the period the "By filter" search may fit on, the rest being held back as a
    /// holdout. None or a value absent from the dropdown means the whole period, i.e. no split.
    #[serde(default, deserialize_with = "de_lenient_u32")]
    pub analytics_tuner_train: Option<u32>,
    /// Base seed of the "By filter" tuner's random restarts, so a chosen seed survives a restart.
    /// None = draw a fresh seed per search, which is what an empty box has always meant.
    ///
    /// Held as text because a seed can exceed what TOML integers hold, and read through
    /// [`de_lenient_seed`] because it must not be able to break anything else — see there.
    #[serde(default, deserialize_with = "de_lenient_seed")]
    pub analytics_tuner_seed: Option<String>,
    /// Fields taking part in the "By filter" tuner's automatic search — the grid checkboxes —
    /// stored as report-column ids (`db::tuner::FieldSpec::col`).
    ///
    /// Column ids rather than a positional mask because the field table's order is PRESENTATION
    /// order (Base → Ping → Volume → Delta) and free to change; a saved mask would then tick
    /// different boxes than the ones the user ticked.
    ///
    /// `None` = no usable saved list, so the tuner applies its own default (every field whose
    /// threshold a strategy can actually store). An EMPTY list is a different statement — the
    /// user unchecked everything — and must stay empty, or the next open would silently re-arm a
    /// search they deliberately disarmed. An id no longer in the table is ignored; a field not yet
    /// in the list opens unchecked, so a newly added one cannot join a search unannounced.
    #[serde(default, deserialize_with = "de_lenient")]
    pub analytics_tuner_fields: Option<Vec<String>>,
    /// Previous visible-column masks, superseded by `analytics_strat_cols_modes2`.
    ///
    /// Retained only as a migration seed so historical choices keep their semantic fields.
    #[serde(default)]
    pub analytics_strat_cols_modes: Option<StratColsByMode>,
    /// Versioned strategy-list masks whose bit layout includes Avg order and Profit %.
    #[serde(default, deserialize_with = "de_lenient")]
    pub analytics_strat_cols_modes2: Option<StratColsByMode>,
    /// Strategy-list sort as `(stable column key, descending)`.
    ///
    /// `None` means the UI's profit-descending default. Read leniently because this
    /// hand-editable field must never make one malformed value discard the complete layout.
    #[serde(default, deserialize_with = "de_lenient")]
    pub analytics_strat_sort: Option<(String, bool)>,
    /// Analytics profit metric: `false` = raw quote money (default for existing configs),
    /// `true` = percent (the report `Profit` column, profit ÷ spent). A per-window display
    /// lens, so it lives here rather than being reset each session.
    #[serde(default)]
    pub analytics_profit_percent: bool,
    /// Analytics "Fact vs variants" KPI matrix: `true` collapses it to its two top rows
    /// (trades + profit), freeing vertical room on short screens where the fields grid below
    /// it would otherwise not fit. A display lens, so it persists rather than resetting each
    /// session. `false` (default, every existing config) shows the full matrix.
    #[serde(default)]
    pub analytics_kpi_collapsed: bool,
    /// Analytics "By filter" distribution card: `true` folds its chart away, keeping the title and
    /// subtitle, so the fields grid and the strategy list above it get the vertical room back.
    /// A display lens like [`Self::analytics_kpi_collapsed`], so it persists rather than resetting
    /// each session. `false` (the default) shows the chart.
    ///
    /// Read leniently because it lands in the hand-edited analytics block: written as `"true"`,
    /// a plain `bool` would reject the whole document and cost the user every window position in
    /// the file. A quoted `"true"`/`"false"` is honoured case-insensitively; anything else at all
    /// answers "not collapsed".
    #[serde(default, deserialize_with = "de_lenient_bool")]
    pub analytics_hist_collapsed: bool,
    /// Analytics "By filter" automatic composition: `true` lets the search choose WHICH fields to
    /// filter on, out of sample, instead of searching every field the checkboxes admit.
    ///
    /// `false` (the default, and every existing config) keeps the plain joint search, which is
    /// still the right tool once the user has decided on a field set themselves. Read leniently
    /// for the same reason as [`Self::analytics_hist_collapsed`]: it lands in the hand-edited
    /// analytics block, and a quoted `"true"` must not cost the user every window position in the
    /// file.
    #[serde(default, deserialize_with = "de_lenient_bool")]
    pub analytics_tuner_compose: bool,
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
    /// inherit it. No entry uses the built-in chart default. Detached windows store their own value
    /// in the tab specification (charts.json).
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
    /// Custom Core Status server display names keyed by endpoint IP string (for example
    /// `45.87.212.44`). No entry means the panel shows the default `Server N` ordinal. Set through
    /// the panel's inline pencil editor; an empty edit removes the entry and restores the default.
    #[serde(default)]
    pub core_server_names: HashMap<String, String>,
    /// Which core-warning axes are actively detected and drawn. A disabled axis stops the engine
    /// opening new episodes for it AND hides its already-recorded episodes from charts and the
    /// Warnings list — "off" means neither written nor shown. Default: every axis on.
    #[serde(default)]
    pub warn_axes: WarnAxesCfg,
    /// Per-axis chart visibility, alert sound, and detection thresholds for the core-warning engine,
    /// set from the Core Status alert popup. Split from `warn_axes` (which keeps only the enable
    /// bools) so an existing `layout.toml` without this key still loads with engine defaults.
    #[serde(default)]
    pub warn_params: WarnParams,
}

/// Per-axis enable switches for the core-warning engine, set from the Core Status gear popup.
///
/// Each field gates one warning axis end to end: while `false`, the backend engine opens no
/// episodes for that axis (so nothing is persisted and no tab/badge lights up) and the read paths
/// filter its persisted history out of the charts and the Warnings list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarnAxesCfg {
    /// Sustained machine system-CPU warning (per server).
    #[serde(default = "def_true")]
    pub cpu: bool,
    /// Rising process-memory warning (per core).
    #[serde(default = "def_true")]
    pub mem: bool,
    /// Dropped-core connectivity warning (per server).
    #[serde(default = "def_true")]
    pub conn: bool,
    /// Sustained above-baseline client↔core ping/RTT warning (per core).
    #[serde(default = "def_true")]
    pub ping: bool,
    /// Sustained above-baseline core→exchange order-API latency warning (per core).
    #[serde(default = "def_true")]
    pub exch: bool,
}

impl Default for WarnAxesCfg {
    /// Every axis on — the behaviour before the toggles existed, and for every config without the key.
    fn default() -> Self {
        Self {
            cpu: true,
            mem: true,
            conn: true,
            ping: true,
            exch: true,
        }
    }
}

/// Chart visibility, alert sound, and detection thresholds per warning axis. Defaults are the
/// operator-tuned starting point (CPU 70%/5s, memory +15%/30s, latency ×2 yellow / ×10 red over a
/// 15 s baseline / 3 s hold); the engine's `WarnTuning::default()` constants are only a
/// pre-config fallback, so a fresh `layout.toml` opens on these numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WarnParams {
    /// Sustained system-CPU axis.
    pub cpu: CpuWarn,
    /// Rising process-memory axis.
    pub mem: MemWarn,
    /// Dropped-core connectivity axis (no thresholds, just chart + sound).
    pub conn: ConnWarn,
    /// Client↔core ping axis.
    pub ping: LatWarn,
    /// Core→exchange ping axis.
    pub exch: LatWarn,
}

/// CPU-warning parameters: drawn-on-chart, sound, sustained-CPU percent, and the sustain seconds.
/// (The averaging window stays a fixed internal 3 s, not a user knob.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuWarn {
    #[serde(default = "def_true")]
    pub chart: bool,
    pub sound: Option<String>,
    /// Machine CPU percent (averaged) that counts toward the warning.
    pub pct: u8,
    /// Consecutive high seconds before it fires.
    pub hold: u8,
}
impl Default for CpuWarn {
    fn default() -> Self {
        Self {
            chart: true,
            sound: None,
            pct: 70,
            hold: 5,
        }
    }
}

/// Memory-growth parameters: percent rise above the window minimum, and the observation window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemWarn {
    #[serde(default = "def_true")]
    pub chart: bool,
    pub sound: Option<String>,
    /// Percent rise above the window minimum that flags growth.
    pub pct: u8,
    /// Observation window in seconds.
    pub window: u16,
}
impl Default for MemWarn {
    fn default() -> Self {
        Self {
            chart: true,
            sound: None,
            pct: 15,
            window: 30,
        }
    }
}

/// Connectivity parameters: chart visibility and sound only (the drop rule has no numeric threshold).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnWarn {
    #[serde(default = "def_true")]
    pub chart: bool,
    pub sound: Option<String>,
}
impl Default for ConnWarn {
    fn default() -> Self {
        Self {
            chart: true,
            sound: None,
        }
    }
}

/// Latency-axis parameters (ping and exch): the baseline MULTIPLIER at which each colour/warning
/// fires, the baseline window, and the sustain seconds. Purely relative — a latency warns when it
/// reaches `red ×` its own rolling mean (default yellow ×2, red ×10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LatWarn {
    #[serde(default = "def_true")]
    pub chart: bool,
    pub sound: Option<String>,
    /// Yellow colour at this multiple of the baseline (e.g. 2 = ×2).
    pub yellow: u8,
    /// Red colour AND warning at this multiple of the baseline (e.g. 10 = ×10).
    pub red: u8,
    /// Baseline (rolling-mean) window in seconds.
    pub window: u16,
    /// Consecutive above-red seconds before it fires.
    pub hold: u8,
}
impl Default for LatWarn {
    fn default() -> Self {
        Self {
            chart: true,
            sound: None,
            yellow: 2,
            red: 10,
            window: 15,
            hold: 3,
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
    pub core_uid: u64,
    pub market: String,
}

/// Read the tuner seed from whatever `layout.toml` happens to hold, never failing.
///
/// This file is deserialized as ONE document with no schema version, so a single field that does
/// not match its declared type discards the ENTIRE saved layout — every window position, every
/// column width. The seed is the field most likely to be typed by hand, and the intuitive way to
/// write it is bare (`analytics_tuner_seed = 123`), which a `String` field rejects. So the field
/// accepts a quoted string, a bare integer, or anything else at all, and answers "no seed" rather
/// than taking the rest of the layout down with it.
fn de_lenient_seed<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// Every shape the seed field might be found in.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Seed {
        /// A quoted decimal seed.
        Text(String),
        /// A bare non-negative integer seed.
        Number(u64),
        /// Anything else — a float, a boolean, a table. Accepted and discarded.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Seed>::deserialize(d)? {
        Some(Seed::Text(s)) => Some(s),
        Some(Seed::Number(n)) => Some(n.to_string()),
        Some(Seed::Other(_)) | None => None,
    })
}

/// Read a hand-editable tuner number the same forgiving way as [`de_lenient_seed`].
///
/// The tuner's search settings sit together in this file and are edited by hand together, so they
/// carry the same hazard: one of them written as `"64"` instead of `64`, or as `0.7` instead of a
/// percentage, would take every window position and column width in the document with it. Each
/// answers "unset" instead, and the tuner then applies its own default.
fn de_lenient_u32<'de, D>(d: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// Every shape one of these fields might be found in.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Num {
        /// A bare non-negative integer.
        Number(u32),
        /// A quoted number, which is how a value gets written when copied from elsewhere.
        Text(String),
        /// Anything else — a float, a negative, a boolean, a table. Accepted and discarded.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Num>::deserialize(d)? {
        Some(Num::Number(v)) => Some(v),
        Some(Num::Text(s)) => s.trim().parse().ok(),
        Some(Num::Other(_)) | None => None,
    })
}

/// Read any optional field the same forgiving way as [`de_lenient_u32`], with no coercion.
///
/// The three helpers around this one exist to ACCEPT a neighbouring shape (a bare seed integer, a
/// quoted number, a quoted bool). This one only salvages the document: a value of the wrong type
/// reads as "unset" instead of taking every window position and column width down with it. Reach
/// for it whenever a new `Option<T>` field lands in this hand-edited file and needs no coercion
/// of its own — `analytics_tuner_fields` written as a bare `"lev"` instead of `["lev"]` is the
/// shape it is there for.
///
/// Note that it runs only when the key is PRESENT: `#[serde(default)]` answers an absent key with
/// `None` without deserializing, which is what keeps "absent" and "present but empty" distinct.
fn de_lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    /// The declared shape, or anything else at all.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Or<T> {
        /// The shape the key is written in.
        Val(T),
        /// Anything else. Accepted and discarded.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Or<T>>::deserialize(d)? {
        Some(Or::Val(v)) => Some(v),
        Some(Or::Other(_)) | None => None,
    })
}

/// Read a hand-editable flag the same forgiving way as [`de_lenient_u32`].
///
/// A quoted `"true"` is the natural typo for someone flipping a display lens by hand, and a plain
/// `bool` field would answer it by discarding every window position and column width in the
/// document. So a quoted boolean is READ as that boolean, case-insensitively, and every other
/// shape reads as `false`, matching the field's default when the key is absent.
fn de_lenient_bool<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// Every shape the flag might be found in.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Flag {
        /// A bare boolean, which is the shape this key is written in.
        Bool(bool),
        /// A quoted boolean, which is how one gets typed by hand.
        Text(String),
        /// Anything else — a number, a list, a table. Accepted and discarded.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Flag>::deserialize(d)? {
        Some(Flag::Bool(v)) => v,
        Some(Flag::Text(s)) => s.trim().eq_ignore_ascii_case("true"),
        Some(Flag::Other(_)) | None => false,
    })
}

impl WindowLayout {
    /// Loads layout.toml. A missing file yields the default; a corrupt file is logged and yields the default.
    pub fn load() -> Self {
        super::toml_io::load_or_default(&paths::layout_path(), "layout.toml", |_| {})
    }

    /// Highest core uid this layout still references.
    ///
    /// Feeds the durable uid high-water mark: the header ticker and active trade-core selections
    /// are stored by UID, so reissuing one would silently bind saved UI state to a new core.
    ///
    /// Returns:
    ///     The largest stable core UID referenced by layout state, if any.
    pub fn max_core_uid(&self) -> Option<u64> {
        self.header_ticker
            .as_ref()
            .map(|ticker| ticker.core_uid)
            .into_iter()
            .chain(self.active_trade_core_by_group.values().copied())
            .max()
    }

    /// Writes layout.toml (non-fatal: errors are only logged).
    pub fn save(&self) {
        if let Err(e) = super::toml_io::save(&paths::layout_path(), self, "layout.toml") {
            log::warn!("{e:#}");
        }
    }
}

#[cfg(test)]
mod tests;
