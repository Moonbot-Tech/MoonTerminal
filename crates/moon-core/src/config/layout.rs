//! Window layout in the portable `layout.toml` file in the config directory. Stores
//! group-window geometry and shared window, chart, and table settings. Live dock and
//! detached-window state lives in `docks.json` and `detached.json`; legacy compatibility
//! fields remain readable. A corrupt or missing file yields the default.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::paths;

mod empty;
mod lines_carry;
mod serde_compat;

pub use lines_carry::stamp_carried_lines;

pub use empty::{EmptyBlock, EmptyPlaces, EmptySlot};

use serde_compat::{
    de_arrow_scale, de_auto_workspace_rail_width, de_candle_volume_alpha, de_candle_volume_height,
    de_candle_volume_scale, de_candle_volume_style, de_candle_volume_tf_s, de_clock_zone,
    de_connector_thickness, de_hvol_price_frame_pct, de_hvol_side, de_hvol_tf_s, de_hvol_width,
    de_lenient_chart_labels, de_lenient_false, de_lenient_graphics, de_lenient_map,
    de_lenient_seed, de_lenient_true, de_lenient_u32, de_marker_scale,
    de_strategies_tree_text_step, de_table_order_map, de_table_sort_map, de_tick_ranges,
    de_trade_history_style, de_trade_volume_alpha,
};
pub use serde_compat::{de_lenient, de_lenient_bool};

mod bounds;
mod first_run;
mod geometry;
mod graphics;
mod panels;
mod per_tab;
mod table_prefs;
mod warnings;

pub use bounds::*;
pub use first_run::*;
pub use geometry::*;
pub use graphics::*;
pub use panels::*;
pub use table_prefs::*;
pub use warnings::*;

use super::{ProfileAge, chart_defaults, chart_labels};

fn def_true() -> bool {
    true
}

/// Default multiplier on the closed-trade-history arrow size: the sizes the layer shipped with.
fn def_trade_arrow_scale() -> f32 {
    1.0
}

/// Default entry-to-exit connector thickness, matching `moon_chart::trade_marks::CONNECTOR_THICKNESS`.
fn def_connector_thickness_px() -> f32 {
    2.0
}

/// Default multiplier on the trade-cross marker size.
///
/// This is the ONE home of that default. It shipped as `1.0` while the value lived on `ChartTheme`,
/// which reproduced the historical 7x7 "Normal Trade X" exactly; `0.70` is the deliberate product
/// change that came with the move into the per-tab popup, because a chart dense with closed trades
/// reads better with smaller crosses. `0.7` and `1.0` are both selectable steps in the popup, so
/// the old size is one click away.
///
/// Note that `chartdx::view::ViewStyle::default()` carries its own `1.0`. That is NOT a second copy
/// of this default: it is the neutral element of a multiplier on a struct the per-frame sync
/// overwrites before any draw. See the comment there.
fn def_marker_scale() -> f32 {
    0.70
}

/// Default opacity of the per-TRADE volume bars. Was the compile-time `DEFAULT_VOLUME_ALPHA` in
/// `chartdx` before it became configurable.
fn def_trade_volume_alpha() -> f32 {
    0.34
}

/// Default bottom-volume display style: the band is on. The only other live id is OFF — see
/// `crate::market::candles::VOLUME_STYLE_OFF`.
fn def_candle_volume_style() -> u8 {
    crate::market::candles::VOLUME_STYLE_HILLS
}

/// Default bottom-volume band height, as a fraction of the plot height.
///
/// The same fraction the per-trade band has always used, so the two bands line up.
fn def_candle_volume_height() -> f32 {
    0.18
}

/// Default bottom-volume opacity.
fn def_candle_volume_alpha() -> f32 {
    0.30
}

/// Default colour of the volume scale's max and average reference lines, sRGB.
fn def_candle_volume_scale() -> [u8; 3] {
    [110, 110, 110]
}

/// Default horizontal-volume price window, percent of price — Moonbot's `PriceFrame` at its
/// shipped `0.1%`. The chart floors it at the market's own tick.
fn def_hvol_price_frame_pct() -> f32 {
    0.1
}

/// Default horizontal-volume zone width, as a fraction of the pane width.
fn def_hvol_width() -> f32 {
    0.2
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
    // wire-id-exempt: terminal-issued, never a core id — see `config::wire_id`.
    #[serde(default)]
    pub active_trade_core_by_group: HashMap<String, u64>,
    /// Workspace preset selected independently for each group window.
    ///
    /// Absent groups are Classic. The complete map is read leniently because this hand-editable
    /// preference must never discard unrelated geometry or panel state.
    #[serde(default, deserialize_with = "de_lenient_map")]
    pub workspace_mode_by_group: HashMap<String, WorkspaceMode>,
    /// Auto-workspace core selection by group; an absent entry means Overview.
    ///
    /// Stale UIDs remain durable high-water references but are resolved as Overview until that
    /// configured live core returns to the group.
    // wire-id-exempt: terminal-issued, never a core id — see `config::wire_id`.
    #[serde(default, deserialize_with = "de_lenient_map")]
    pub auto_workspace_core_by_group: HashMap<String, u64>,
    /// Last eligible top-level Auto workspace tab selected independently for each group.
    ///
    /// Classic activity remains in `docks.json`. Values are validated by the Shell when read and
    /// written, while lenient map decoding keeps an unknown or wrong-typed hand edit from
    /// discarding unrelated window geometry.
    #[serde(default, deserialize_with = "de_lenient_map")]
    pub auto_workspace_tab_by_group: HashMap<String, String>,
    /// One application-wide Auto rail width shared by every group window.
    ///
    /// The stored logical-pixel value is leniently decoded and clamped so malformed or stale
    /// preferences cannot reject the surrounding layout or produce an unusable rail.
    #[serde(default, deserialize_with = "de_auto_workspace_rail_width")]
    pub auto_workspace_rail_width: Option<f32>,
    /// Workspace preset for any group that has never chosen one, seeded once on a brand-new profile.
    ///
    /// `None` — every layout written before this field existed — resolves to
    /// [`WorkspaceMode::Classic`], so an established user is untouched. It is a persisted scalar
    /// rather than a flipped `#[default]` on the enum because that `Default` is also what serde
    /// substitutes for an absent FIELD in a layout that DOES exist, and rather than pre-seeded
    /// per-group entries because those would forge a preference the user never expressed and would
    /// still miss any group created later.
    #[serde(default, deserialize_with = "de_lenient")]
    pub default_workspace_mode: Option<WorkspaceMode>,
    /// Whether this layout came from a brand-new profile. RUNTIME ONLY, never serialized.
    ///
    /// Placement of the first window is a per-launch decision, not a stored preference, so it must
    /// not appear in `layout.toml`. Skipping it also means every construction path other than
    /// [`WindowLayout::load`] — `default()` included — gets the conservative answer, `false`.
    #[serde(skip)]
    first_run_profile: bool,
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
    /// Strategies: whether core roots are grouped under exchange headings.
    ///
    /// `None` keeps the Strategies-owned default. Read leniently so a malformed hand edit cannot
    /// discard the complete window layout.
    #[serde(default, deserialize_with = "de_lenient")]
    pub strategies_group_by_venue: Option<bool>,
    /// Strategies: whether unchecked live strategies are hidden from the tree.
    ///
    /// `None` keeps the Strategies-owned default. Explicit reveals persist this preference as
    /// disabled so the requested row remains visible after restart.
    #[serde(default, deserialize_with = "de_lenient")]
    pub strategies_active_only: Option<bool>,
    /// Strategies: local text-size step for the tree pane, on top of the global Font slider.
    ///
    /// `None` is the shipped zero — the pane renders at exactly the theme base, identical to
    /// before this field existed. Decoded and clamped like `auto_workspace_rail_width` so a
    /// malformed or out-of-range hand edit cannot discard the surrounding layout or produce a
    /// step the stepper control cannot represent.
    #[serde(default, deserialize_with = "de_strategies_tree_text_step")]
    pub strategies_tree_text_step: Option<f32>,
    /// Strategies: whether the parameters pane shows every section at once instead of one.
    ///
    /// `None` keeps the Strategies-owned default. Read leniently so a malformed hand edit cannot
    /// discard the complete window layout.
    #[serde(default, deserialize_with = "de_lenient")]
    pub strategies_params_full: Option<bool>,
    /// Strategies: whether section and field rows carry the localized human name under
    /// Moonbot's own identifier.
    ///
    /// `None` keeps the Strategies-owned default. Read leniently so a malformed hand edit cannot
    /// discard the complete window layout.
    #[serde(default, deserialize_with = "de_lenient")]
    pub strategies_human_labels: Option<bool>,
    /// Strategies, "WL distribution" tab: the order its core rows were arranged in, as core
    /// uids. The top row receives coins first. Cores it does not name follow in tree order, so
    /// an absent or partial list is complete by construction. Read leniently so a malformed hand
    /// edit cannot discard the complete window layout.
    // wire-id-exempt: terminal-issued core uids, never a core id — see `config::wire_id`.
    #[serde(default, deserialize_with = "de_lenient")]
    pub strategies_dist_order: Option<Vec<u64>>,
    /// Global "Assets" window geometry (singleton), so it reopens in its previous position.
    #[serde(default)]
    pub assets_window: Option<GeomRect>,
    /// "Hide assets worth less than N $" threshold (slider in the "Assets" top bar). Shared by all
    /// "Assets" windows/tabs (one value for every scope, avoiding per-scope keys). `0` = show all.
    /// `None` (old file / field was not written) → panel-side default of $1.
    #[serde(default)]
    pub assets_min_value: Option<f64>,
    /// Assets: whether the wallet section's core list is grouped under exchange headings.
    ///
    /// `None` keeps the Assets-owned default. Read leniently so a malformed hand edit cannot
    /// discard the complete window layout.
    #[serde(default, deserialize_with = "de_lenient")]
    pub assets_group_by_venue: Option<bool>,
    /// "Settings" window geometry (separate window), so it reopens in its previous position.
    #[serde(default)]
    pub settings_window: Option<GeomRect>,
    /// "Screener" window geometry (singleton), so it reopens in its previous position.
    #[serde(default)]
    pub screener_window: Option<GeomRect>,
    /// Expert core-settings window geometry (singleton), so it reopens in its previous position.
    #[serde(default)]
    pub core_expert_window: Option<GeomRect>,
    /// Whether the core-settings gear opens the EXPERT window instead of the compact popup.
    ///
    /// Application-wide rather than per core or per group: it selects a way of working, not a
    /// property of any one MoonBot. `None` — every layout written before this field existed —
    /// resolves to the compact popup, so an established profile keeps what it already has.
    ///
    /// Read leniently, like every other preference in this hand-edited file: one mistyped value
    /// here must not reject the whole document and cost the user every window position in it.
    #[serde(default, deserialize_with = "de_lenient")]
    pub core_settings_expert: Option<bool>,
    /// "Analytics" window geometry (singleton), so it reopens in its previous position.
    #[serde(default)]
    pub analytics_window: Option<GeomRect>,
    /// Independent desktop Profit Monitor geometry.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_window: Option<GeomRect>,
    /// Geometry shared by EVERY trade-detail window, so one reopens where the user left the last.
    ///
    /// One rectangle for all of them rather than one per trade: the user adjusts the window once
    /// and expects that shape back, and a per-trade key would mean the first open of every new coin
    /// ignored every adjustment ever made. Two windows may be open at once, so the second still
    /// cascades off this rectangle instead of landing exactly on the first.
    #[serde(default, deserialize_with = "de_lenient")]
    pub trade_window: Option<GeomRect>,
    /// Price scale shared by EVERY trade-detail window (`None` = Auto).
    ///
    /// Same policy as [`Self::trade_window`]: the user picks a zoom once and expects it back on
    /// the next trade, including after a restart. A per-trade key would mean the first open of
    /// every new coin ignored every choice ever made.
    ///
    /// `None` — every layout written before this field existed, and an explicit Auto pick — is
    /// Auto. Read leniently so a malformed hand edit cannot discard the complete window layout.
    #[serde(default, deserialize_with = "de_lenient")]
    pub trade_window_scale: Option<f32>,
    /// Show neighbouring Report-period trades in trade windows; absent means ON.
    /// Stored alongside the shared scale and read leniently to preserve older layouts.
    #[serde(default, deserialize_with = "de_lenient")]
    pub trade_window_other_trades: Option<bool>,
    /// Frame trade windows on the trade itself — its own span, and the neighbours within it
    /// while they are shown — rather than on the fixed context; absent means OFF.
    #[serde(default, deserialize_with = "de_lenient")]
    pub trade_window_fit: Option<bool>,
    /// Hide the trade window's figures rail; absent means shown.
    #[serde(default, deserialize_with = "de_lenient")]
    pub trade_window_hide_rail: Option<bool>,
    /// Let trade windows fetch the venue's prints (the tick stage); absent means ON.
    #[serde(default, deserialize_with = "de_lenient")]
    pub trade_window_ticks: Option<bool>,
    /// Shade the entry corridor the core saved for a MoonShot trade, from the order's placement
    /// to its fill, in trade windows and in the tuner's trade pane; absent means OFF.
    #[serde(default, deserialize_with = "de_lenient")]
    pub trade_window_moonshot_zone: Option<bool>,
    /// Hide the figures rail of the trade pane under the tuner's deal table; absent means
    /// HIDDEN — the pane shares the tab with the table and the grid, and the chart is what it is
    /// opened for. Apart from [`Self::trade_window_hide_rail`], which a window of its own keeps.
    #[serde(default, deserialize_with = "de_lenient")]
    pub analytics_trade_hide_rail: Option<bool>,
    /// Print the trade's own captions — its strategy, the detect it fired on, why it closed —
    /// at the top of trade windows; absent means ON.
    #[serde(default, deserialize_with = "de_lenient")]
    pub trade_window_labels: Option<bool>,
    /// The same captions in the trade pane under the tuner's deal table; absent means OFF, as
    /// the pane's rail is hidden — the pane is opened for the picture. Apart from
    /// [`Self::trade_window_labels`], which a window of its own keeps.
    #[serde(default, deserialize_with = "de_lenient")]
    pub analytics_trade_labels: Option<bool>,

    /// Selected Profit Monitor period id.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_period: Option<String>,
    /// Selected Profit Monitor grouping id (`core` or `exchange`).
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_group: Option<String>,
    /// Profit Monitor sort as `(stable column key, descending)`.
    ///
    /// `None` preserves the grouping's natural order. Read leniently because a malformed
    /// hand-edited widget preference must never discard the complete window layout.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_sort: Option<(String, bool)>,
    /// Whether the Profit Monitor window was open when the terminal last exited.
    ///
    /// The monitor is a desktop window with no taskbar button of its own, so a restart that
    /// silently drops it leaves no trace that it was ever there. Startup reopens it from this flag.
    #[serde(default, deserialize_with = "de_lenient_bool")]
    pub profit_monitor_open: bool,
    /// What an empty Main draws, layer by layer: the mark, its one line of help, and the three
    /// crowd tables.
    ///
    /// The MARK is the one of them that reaches further than this screen: it is the same brand an
    /// AddToChart stack with no charts and a chart slot waiting for data draw, and one switch
    /// governs all three — a reader who switched it off meant the logo, not the logo here.
    ///
    /// Five independent switches rather than a mode, because a person may want any mixture of
    /// them. `None` means "never chosen" and takes the feature's own default — the logo and the
    /// hint on, every table off — which is what lets a default change later without overriding
    /// somebody who deliberately switched one off. The tables also decide what is READ: each one
    /// carries a connection to a public service, and an unshown table opens none.
    ///
    /// Read leniently for the same reason as every other widget preference here: a hand edit must
    /// not discard the window layout.
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_logo: Option<bool>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_hint: Option<bool>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_minute: Option<bool>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_traders: Option<bool>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_coins: Option<bool>,
    /// Where each of the five blocks of the empty screen is drawn, as one of nine anchors.
    ///
    /// Separate from the switches above because they answer different questions: the switch says
    /// WHETHER a block is drawn, this says WHERE. A person who has switched the minute off still
    /// has a place chosen for it, and switching it back on puts it where they left it.
    ///
    /// `None` means "never chosen" and takes the block's own default, which between the five
    /// reproduces the screen the terminal shipped with. Two blocks may name the SAME anchor — they
    /// stack there, which is exactly how the brand and its hint share the middle — so there is no
    /// combination here that has to be repaired.
    ///
    /// Read leniently for the reason every widget preference here is: a hand edit, or an anchor
    /// written by a newer build, must cost that one block its place and never the window layout.
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_place_logo: Option<EmptySlot>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_place_hint: Option<EmptySlot>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_place_minute: Option<EmptySlot>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_place_traders: Option<EmptySlot>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_place_coins: Option<EmptySlot>,
    /// Whether the crowd's rule is watched, and the two lines it is watched against.
    ///
    /// The rule is not a table: it costs a connection for as long as the terminal is open, because
    /// a detection that only fired while somebody was looking at an empty screen would be useless.
    /// So it is off by default and switched on deliberately, and the two thresholds travel with it
    /// — a figure that is loud on a quiet market is unremarkable during a pump, and only the person
    /// watching knows which they are in.
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_detect: Option<bool>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_detect_profit: Option<f64>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_detect_trades: Option<u32>,
    /// How long one of the rule's cards stays, in seconds, and whether a fresh one may take the
    /// seat of the oldest when every seat is full.
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_detect_keep: Option<u32>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub main_empty_detect_evict: Option<bool>,
    /// Profit Monitor: whether a row shows its exchange logo before the name.
    ///
    /// `None` means the feature's own default. Every monitor preference is read leniently for the
    /// same reason as the sort tuple: a hand-edited widget preference must never discard the
    /// complete window layout.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_exchange_icons: Option<bool>,
    /// Profit Monitor: whether the profit cell appends the latest closed trade in parentheses.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_last_trade: Option<bool>,
    /// Profit Monitor: whether a row lights up and fades when its core closes a new trade.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_flash: Option<bool>,
    /// Profit Monitor: whether clicking a row's core cell filters every main-window panel.
    ///
    /// Only the preference is persisted. The selection itself is process-lifetime state, exactly
    /// like the per-panel core filters it drives — a restart comes back showing every core.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_core_filter: Option<bool>,
    /// Profit Monitor: whether the by-core table splits into the user's saved core groups.
    ///
    /// Only the preference lives here; the groups themselves are application configuration
    /// (`AppConfig.core_groups`), shared with every core picker.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_group_sections: Option<bool>,
    /// Profit Monitor: whether active cores that closed no trade appear as zero rows.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_idle_cores: Option<bool>,
    /// Profit Monitor: whether a row leads with the core's run status, and a restart button when
    /// that core reported a stopped runtime.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_core_status: Option<bool>,
    /// Profit Monitor: whether a row carries the start/stop control for its core's trading.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_trading_buttons: Option<bool>,
    /// Profit Monitor: superseded by [`Self::profit_monitor_group_controls`], which widened this
    /// from "trading on group captions" to "every enabled run control on group captions".
    ///
    /// Still READ, never written: the key shipped, and a profile that set it must keep its group
    /// captions commanding cores across the rename.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_group_trading: Option<bool>,
    /// Profit Monitor: whether a row carries the AutoDetect on/off switch for its own core.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_auto_buttons: Option<bool>,
    /// Profit Monitor: whether a group caption also carries whichever run controls are enabled,
    /// commanding every core the group names with one command each.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_group_controls: Option<bool>,
    /// Profit Monitor: whether the table heading carries the trading and AutoDetect controls for
    /// every core the table commands at once.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_header_controls: Option<bool>,
    /// Auto workspace rail: whether a core row leads with its core's run status, and a restart
    /// button when that core reported a stopped runtime. `None` = the rail's default (ON).
    #[serde(default, deserialize_with = "de_lenient")]
    pub workspace_rail_core_status: Option<bool>,
    /// Auto workspace rail: whether a core row carries the start/stop control for its trading.
    #[serde(default, deserialize_with = "de_lenient")]
    pub workspace_rail_trading_buttons: Option<bool>,
    /// Auto workspace rail: whether a core row carries the AutoDetect on/off switch.
    #[serde(default, deserialize_with = "de_lenient")]
    pub workspace_rail_auto_buttons: Option<bool>,
    /// Auto workspace rail: whether an exchange heading also carries whichever run controls are
    /// enabled, commanding every core under that heading with one command each.
    #[serde(default, deserialize_with = "de_lenient")]
    pub workspace_rail_exchange_controls: Option<bool>,
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
    /// "Analytics" strategy-name mask in the shared syntax (comma = OR, space = AND, `!word`
    /// excludes; see `moon_core::strategy_query`).
    /// None or empty = no filter.
    ///
    /// A flat field rather than an entry in [`Self::report_filters`], because Analytics is a
    /// singleton tool window with no host context to key one by: every other Analytics preference
    /// beside it is flat for the same reason. Read leniently like its neighbours — this block is
    /// hand-edited, and one wrongly typed value must not cost the user the rest of the file.
    #[serde(default, deserialize_with = "de_lenient")]
    pub analytics_strategy_mask: Option<String>,
    /// Bitmask of the visible columns in the Tuning strategy list (the column selector).
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
    /// Analytics money scale: `true` reports every scope in USDT, converting a single-quote scope
    /// too. `false` (default, every existing config) lets the unit follow the period's own quote,
    /// which makes a BTC-quoted core read in BTC for one range and in USDT for another. Ignored in
    /// percent mode, and inert when a scope cannot be fully valued.
    #[serde(default)]
    pub analytics_profit_usdt: bool,
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
    /// Analytics Summary "Profit by core" card: `true` ranks EVERY core, `false` (the default)
    /// shows the compact leaders/outsiders overview.
    ///
    /// A display lens like [`Self::analytics_hist_collapsed`], and persisted for the same reason:
    /// a user who runs two hundred cores picks the full list once and expects it back after a
    /// restart. Read leniently for the same reason as that flag.
    #[serde(default, deserialize_with = "de_lenient_bool")]
    pub analytics_cores_show_all: bool,
    /// Analytics tuner right-hand column ("Fact vs variants" plus the axis-specific tool):
    /// `true` folds the whole column away so the strategy list takes the freed width. One flag
    /// serves every axis (Filters / Coins / Time), because it is the same column
    /// in each — exactly like [`Self::analytics_kpi_collapsed`].
    ///
    /// A display lens like the two flags above, so it persists rather than resetting each
    /// session, and it is deliberately INDEPENDENT of `analytics_kpi_collapsed`: folding the
    /// column away leaves the matrix's own two-row collapse untouched, so restoring the column
    /// restores exactly what the user had inside it. `false` (the default, and every existing
    /// config) shows the column.
    ///
    /// Read leniently for the same reason as [`Self::analytics_hist_collapsed`]: it lands in the
    /// hand-edited analytics block, and a quoted `"true"` must not cost the user every window
    /// position in the file.
    #[serde(default, deserialize_with = "de_lenient_bool")]
    pub analytics_tuner_side_collapsed: bool,
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
    /// The "Entry/Exit" tuner's settings: its search's and its model's. `None` — every config
    /// written before the axis had settings — opens on the defaults. Read leniently: the block
    /// is hand-editable, and a malformed one must cost only itself, never the window positions
    /// around it.
    #[serde(default, deserialize_with = "de_lenient")]
    pub analytics_ticks: Option<TicksAxisLayout>,
    /// Visible screener columns (keys in canonical order). None = all.
    #[serde(default)]
    pub screener_columns: Option<Vec<String>>,
    /// Price ticker in the header (left, after the logo): selected core+market. `None` = default
    /// (first connected core; BTCUSDT, or UBTCUSDC on Hyperliquid-like exchanges).
    #[serde(default)]
    pub header_ticker: Option<HeaderTicker>,
    /// Markets opened from a chart coin search, most recent first, capped at
    /// [`Self::RECENT_COINS_CAP`]. `None` = nothing opened yet.
    ///
    /// Stored by stable core UID like [`HeaderTicker`], so the list survives a configuration
    /// reorder. Entries whose core is gone stay in the file — they cost nothing, and dropping them
    /// on load would silently discard the history of a core that is merely offline right now. They
    /// are filtered at READ time instead, and they still raise the durable UID high-water mark (see
    /// [`Self::max_core_uid`]) so a deleted core's UID can never be reissued to a different server.
    ///
    /// Lenient: this file is one schema-less document, and a single mistyped entry must not discard
    /// every window position along with it.
    #[serde(default, deserialize_with = "de_lenient")]
    pub recent_coins: Option<Vec<HeaderTicker>>,
    /// Application-wide display clock: an exact IANA zone id such as `Europe/Warsaw`.
    /// `None` means an untouched profile; startup detects and persists the operating-system zone.
    /// Existing values always win, including zones outside the clock picker's curated city list.
    ///
    /// The zone id rather than the city's three-letter code: it is canonical, unambiguous and
    /// meaningful to anyone editing this file by hand, while the code is presentation the terminal
    /// derives from its own city table when possible. `de_clock_zone` preserves a present invalid
    /// value as an invalid sentinel: the document remains loadable without mistaking corruption
    /// for a first-run profile and overwriting it from the operating system.
    #[serde(default, deserialize_with = "de_clock_zone")]
    pub header_clock_zone: Option<String>,
    /// Fixed UTC offset in minutes, retained as the migration seed when
    /// [`Self::header_clock_zone`] is absent and as a compatibility mirror when it is present.
    /// Startup refreshes it from the chosen zone's current offset so fixed-offset readers show the
    /// same wall clock. A nonzero value migrates an old profile without consulting the operating
    /// system; zero plus an absent zone marks an untouched profile for system-zone detection.
    #[serde(default)]
    pub header_clock_offset_min: i32,
    /// Candle/trade display on charts (timeframe, mode, trade zone, outline, etc.) —
    /// GLOBAL DEFAULT (tabs can override it in their charts.json specification).
    #[serde(default)]
    pub candle_view: crate::market::candles::CandleViewCfg,
    /// Chart drawing settings from the toolbar's palette popup —
    /// GLOBAL DEFAULT (tabs can override it in their charts.json specification).
    #[serde(default, deserialize_with = "de_lenient_graphics")]
    pub chart_graphics: ChartGraphicsCfg,
    /// One-shot marker: the trade-mark and bottom-volume values have been carried across from the
    /// old `theme.toml` home into [`Self::chart_graphics`] and into every chart tab that held an
    /// override.
    ///
    /// NEVER reset it. Re-running that migration would overwrite whatever the user has since chosen
    /// in the chart-graphics popup with the stale values it reads out of the old theme file.
    ///
    /// The migration does not rewrite `theme.toml`, but that is NOT a recovery copy and must not be
    /// described as one: `AppConfig::save_impl` calls `ChartThemeSet::save` on every settings write,
    /// and once the six fields left `ChartTheme` that write drops the now-unknown keys. The
    /// durable copy is the `.bak` the migration takes before it touches anything.
    ///
    /// It lives here rather than in `theme.toml` because that file is portable — users copy it
    /// between machines — and a marker travelling with it would suppress the migration on a second
    /// machine that still needs it. `charts.json` was not an option either: the migration must run
    /// even when no tab spec exists yet.
    #[serde(default)]
    pub chart_graphics_from_theme_migrated: bool,
    /// One-shot marker: the per-tab `Cancel Buy` / `Panic Sell` POSITIONS have been carried across
    /// into the caption configuration, where the buttons are now drawn from.
    ///
    /// NEVER reset it. The legacy keys are cleared from `charts.json` as they are carried over, so
    /// a second pass would find nothing to read and would append the shipped pair to a caption set
    /// where the reader may since have moved — or removed — those very buttons.
    #[serde(default)]
    pub chart_action_buttons_migrated: bool,
    /// One-shot marker: the strategy-filter skip lines have been carried across from the graphics
    /// checkbox into the caption configuration, where they are now drawn from.
    ///
    /// NEVER reset it. A second pass would append the shipped module to a caption set where the
    /// reader may since have moved — or removed — that very column.
    #[serde(default)]
    pub chart_strategy_filters_migrated: bool,
    /// One-shot marker: the retired global "show path" toggle of `orders.toml` has been folded
    /// into the per-tab `hide_order_move_history` — into [`Self::chart_graphics`], every stored
    /// per-kind default and every tab override — so a user who had the path off keeps it off.
    ///
    /// NEVER reset it. The retired flag stays on disk, so a second pass would re-hide the history
    /// on every tab the user has since shown it on.
    #[serde(default)]
    pub chart_path_visibility_migrated: bool,
    /// Chart caption labels — which figures the chart prints beside its plot, where, and how —
    /// GLOBAL DEFAULT (tabs can override it in their charts.json specification).
    ///
    /// The default reproduces the caption the chart drew before this was configurable, so a profile
    /// written before this key existed opens on exactly the corner it had.
    #[serde(default, deserialize_with = "de_lenient_chart_labels")]
    pub chart_labels: super::chart_labels::ChartLabelsCfg,
    /// Defaults for tabs torn off into their own windows — empty means "follow the fields above".
    ///
    /// The three fields above are the MAIN kind's defaults and keep their keys, so a profile
    /// written before the split opens exactly as it did. See [`super::chart_defaults`].
    #[serde(
        default,
        deserialize_with = "super::chart_defaults::ChartTabDefaults::de_lenient_boxed"
    )]
    pub chart_defaults_addto: Box<super::chart_defaults::ChartTabDefaults>,
    /// Defaults for tabs under the anchor lock, wherever they live. Empty means "follow Main" for
    /// the candles and the graphics, and this kind's OWN shipped set for the captions — see
    /// [`super::chart_labels::ChartLabelsCfg::compare_default`].
    #[serde(
        default,
        deserialize_with = "super::chart_defaults::ChartTabDefaults::de_lenient_boxed"
    )]
    pub chart_defaults_compare: Box<super::chart_defaults::ChartTabDefaults>,
    /// Defaults for the trade-detail window. Empty means "follow this kind's own built-in set" —
    /// which, for the captions, is NOT Main's: see
    /// [`super::chart_labels::ChartLabelsCfg::trade_default`].
    #[serde(
        default,
        deserialize_with = "super::chart_defaults::ChartTabDefaults::de_lenient_boxed"
    )]
    pub chart_defaults_trade: Box<super::chart_defaults::ChartTabDefaults>,
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
    /// Dragged column order per context-qualified table id: the same keys as
    /// [`Self::table_column_widths`].
    ///
    /// No entry, or an empty list, means the table's source order. The list is the user's sequence
    /// of column ids. A panel drops an id that its current columns do not contain and appends an id
    /// they gained; this map only stores what was last written. It is read per entry so one
    /// hand-edited list cannot reject `layout.toml`. The 100 ms coordination drain and
    /// `on_app_quit` both snapshot this struct whole, which is what registers the map on both save
    /// paths.
    #[serde(default, deserialize_with = "de_table_order_map")]
    pub table_column_order: HashMap<String, Vec<String>>,
    /// Generic table-sort persistence: context-qualified table id to validated column/direction.
    ///
    /// Valid entries are salvaged independently, so a hand-edited value for one panel cannot erase
    /// another panel's sort or reject the rest of `layout.toml`. No entry keeps the panel's exact
    /// historical default.
    #[serde(default, deserialize_with = "de_table_sort_map")]
    pub table_sorts: HashMap<String, TableSortPreference>,
    /// Report toolbar filters per host context: `report-filters:dock` / `report-filters:win`.
    ///
    /// Keyed exactly like the column maps above, through `table_persist::ctx_id`, so a docked tab
    /// and a detached window keep their own answers. No entry leaves the panel's own defaults
    /// standing. The map is read leniently for the same reason as its neighbours: a hand edit of a
    /// filter preference must never discard the complete window layout.
    #[serde(default, deserialize_with = "de_lenient_map")]
    pub report_filters: HashMap<String, ReportFilterPrefs>,
    /// Core Status presentation choice per host context: `core-status-mode:dock` /
    /// `core-status-mode:win`.
    ///
    /// Keyed like its neighbours above, through `table_persist::ctx_id`, so a docked tab and a
    /// detached window remember their own mode independently. The value is an OPAQUE stable code
    /// owned by the panel in `moon-ui-gpui`; this crate deliberately does not hold the vocabulary,
    /// exactly as it does not hold [`ReportFilterPrefs`]'s. No entry, or a code this build does not
    /// know, leaves the panel's own first-run default standing rather than failing the load.
    #[serde(default, deserialize_with = "de_lenient_map")]
    pub core_status_mode: HashMap<String, String>,
    /// One-shot Report column migrations already applied to [`Self::table_visible_columns`].
    ///
    /// A saved visible-column set is an EXPLICIT list, so a column added later is simply absent
    /// from it and would stay hidden forever for everyone who ever arranged their columns. The
    /// migration that repairs that must record its completion HERE, in the same document as the
    /// sets it rewrites: a marker in the recoverable report replica would have an independent
    /// write and recovery lifecycle, so an interrupted layout flush could skip the migration
    /// permanently, while a report-replica recovery would re-apply one the user has since undone.
    /// One document, one atomic write, one answer.
    ///
    /// Read leniently like the other hand-editable numbers here; `None` means never migrated.
    #[serde(default, deserialize_with = "de_lenient_u32")]
    pub report_columns_migration: Option<u32>,
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
    /// Custom Core Status server display names keyed by endpoint IP string. No entry means the
    /// panel shows the default `Server N` ordinal. Set through
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
    /// Quiet mode ("sleep"): the schedule, the sound bypasses, and the persisted manual state of
    /// the header toggle. Terminal-wide rather than per group — one operator, one pair of ears.
    #[serde(default)]
    pub quiet: crate::config::quiet::QuietCfg,
    /// Immediate Settings preferences for actual trade edges, keyed by platform and DEX.
    #[serde(default, deserialize_with = "de_lenient_map")]
    pub trade_sounds: HashMap<String, crate::config::trade_sounds::TradeSounds>,
    /// Trade-only loudness in percent; absent preserves the original 100% loudness.
    #[serde(default, deserialize_with = "de_lenient_u32")]
    pub trade_sound_volume: Option<u32>,
    /// Sound stem for a drawn-figure alert whose strategy names no sound, chosen in the Alerts
    /// panel. Empty means the player's built-in default. Persisted here because the choice used to
    /// live only in memory and reset to the default on every start.
    #[serde(default)]
    pub alert_sound: String,
    /// Seconds a strategy-less figure-alert card stays on the Detects feed, chosen in the Alerts
    /// panel. `None` is the MoonBot `KeepTime` default ([`ALERT_DURATION_S_DEFAULT`]); a stored
    /// value is clamped to the stepper range on read. An alert whose strategy named `KeepAlert`
    /// ignores this field.
    #[serde(default, deserialize_with = "de_lenient_u32")]
    pub alert_duration_s: Option<u32>,
    /// How many times the detect player enqueues a figure-alert clip, chosen in the Alerts panel.
    /// `None` is [`ALERT_REPEAT_DEFAULT`]. Zero is silence for that firing; ordinary (non-alert)
    /// detects still play once.
    #[serde(default, deserialize_with = "de_lenient_u32")]
    pub alert_repeat: Option<u32>,
}

impl WindowLayout {
    /// Loads layout.toml. A missing file yields the default; a corrupt file is logged and yields the default.
    ///
    /// Takes the profile's age rather than probing the filesystem for it, so the result stays a
    /// pure function of (bytes, age) and the seeding decision below is reachable from a test. The
    /// age must come from [`super::profile_age`], which reads the disk as it was at launch —
    /// asking `layout_path().exists()` here instead would answer a subtly different question and
    /// would disagree with the theme default that shares the same fact.
    ///
    /// Args:
    ///     age: Whether any file of a configured profile existed at launch.
    ///
    /// Returns:
    ///     The stored layout, with the first-run workspace preset seeded when there is one.
    pub fn load(age: super::ProfileAge) -> Self {
        let mut layout: Self =
            super::toml_io::load_or_default(&paths::layout_path(), "layout.toml", |_| {});
        layout.first_run_profile = age == super::ProfileAge::FirstRun;
        if let Some(mode) = first_run_workspace_mode(age, layout.default_workspace_mode) {
            layout.default_workspace_mode = Some(mode);
        }
        layout
    }

    /// Whether the profile this layout was loaded for had never been configured.
    ///
    /// Consumed by first-window placement. Deliberately NOT derived from `groups.is_empty()`:
    /// that map is persistence data written by the bounds observer long after a window opens, so
    /// an established profile that simply never had geometry recorded would read as brand new.
    ///
    /// Returns:
    ///     `true` only for a layout loaded on a first run.
    pub fn is_first_run_profile(&self) -> bool {
        self.first_run_profile
    }

    /// Effective workspace preset for a group that has no entry of its own.
    ///
    /// Returns:
    ///     The stored layout-wide default, whatever put it there — the first-run seed, or a value
    ///     an earlier launch persisted — and [`WorkspaceMode::Classic`] when none was stored,
    ///     which is every layout written before that preset existed.
    pub fn default_workspace_mode(&self) -> WorkspaceMode {
        self.default_workspace_mode.unwrap_or_default()
    }

    /// Return the effective global Auto rail width for legacy and current layouts.
    ///
    /// Returns:
    ///     Persisted clamped logical-pixel width, or the first-run default when no preference has
    ///     been written yet.
    pub fn auto_workspace_rail_width(&self) -> f32 {
        self.auto_workspace_rail_width
            .unwrap_or(AUTO_WORKSPACE_RAIL_WIDTH_DEFAULT)
    }

    /// Return the effective Strategies tree text step, testable without a GPUI `App`.
    ///
    /// Returns:
    ///     Persisted clamped step, or the shipped default when no preference has been written yet.
    pub fn strategies_tree_text_step(&self) -> f32 {
        self.strategies_tree_text_step
            .unwrap_or(STRATEGIES_TREE_TEXT_STEP_DEFAULT)
    }

    /// Highest core uid this layout still references.
    ///
    /// Feeds the durable UID high-water mark: the header ticker, recent coin history, active
    /// trade-core selections, and Auto workspace selections are stored by UID, so reissuing one
    /// would silently bind saved UI state to a new core.
    ///
    /// Returns:
    ///     The largest stable core UID referenced by layout state, if any.
    pub fn max_core_uid(&self) -> Option<u64> {
        self.header_ticker
            .as_ref()
            .map(|ticker| ticker.core_uid)
            .into_iter()
            .chain(self.active_trade_core_by_group.values().copied())
            .chain(self.auto_workspace_core_by_group.values().copied())
            .chain(
                self.recent_coins
                    .iter()
                    .flatten()
                    .map(|entry| entry.core_uid),
            )
            .max()
    }

    /// Cap on [`Self::recent_coins`]: enough to cover a working set, short enough to stay scannable
    /// in a dropdown that also shows a second section.
    pub const RECENT_COINS_CAP: usize = 12;

    /// Records a market as the most recently opened one.
    ///
    /// Moves an existing entry to the front rather than duplicating it, so re-opening a market
    /// refreshes its position instead of pushing an older copy down the list, and trims to
    /// [`Self::RECENT_COINS_CAP`]. The whole MRU policy lives here, on the type that is persisted,
    /// so it can be exercised without a running UI.
    ///
    /// Args:
    ///     core_uid: Stable UID of the core the market was opened on.
    ///     market: Canonical market name.
    ///
    /// Returns:
    ///     Whether the list changed and therefore needs saving.
    pub fn push_recent_coin(&mut self, core_uid: u64, market: &str) -> bool {
        let entries = self.recent_coins.get_or_insert_with(Vec::new);
        if entries
            .first()
            .is_some_and(|top| top.core_uid == core_uid && top.market == market)
        {
            return false;
        }
        entries.retain(|entry| !(entry.core_uid == core_uid && entry.market == market));
        entries.insert(
            0,
            HeaderTicker {
                core_uid,
                market: market.to_string(),
            },
        );
        entries.truncate(Self::RECENT_COINS_CAP);
        true
    }

    /// Write `layout.toml` without treating persistence failure as fatal.
    ///
    /// Returns:
    ///     `true` only after the atomic write succeeds, allowing callers to retain dirty state and
    ///     retry a transient failure.
    pub fn save(&self) -> bool {
        match super::toml_io::save(&paths::layout_path(), self, "layout.toml") {
            Ok(()) => true,
            Err(error) => {
                log::warn!("{error:#}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests;
