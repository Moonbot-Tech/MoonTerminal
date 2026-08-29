//! Window layout in the portable `layout.toml` file in the config directory. Stores
//! group-window geometry and shared window, chart, and table settings. Live dock and
//! detached-window state lives in `docks.json` and `detached.json`; legacy compatibility
//! fields remain readable. A corrupt or missing file yields the default.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::paths;

mod serde_compat;

use serde_compat::{
    de_arrow_scale, de_auto_workspace_rail_width, de_candle_volume_alpha, de_candle_volume_height,
    de_candle_volume_scale, de_candle_volume_style, de_clock_zone, de_connector_thickness,
    de_lenient_chart_labels, de_lenient_false, de_lenient_graphics, de_lenient_map, de_lenient_seed,
    de_lenient_true, de_lenient_u32, de_marker_scale, de_strategies_tree_text_step,
    de_table_sort_map, de_trade_volume_alpha,
};
pub use serde_compat::{de_lenient, de_lenient_bool};

/// Narrowest persisted Auto-workspace rail width in logical pixels.
pub const AUTO_WORKSPACE_RAIL_WIDTH_MIN: f32 = 52.0;
/// Widest persisted Auto-workspace rail width in logical pixels.
pub const AUTO_WORKSPACE_RAIL_WIDTH_MAX: f32 = 560.0;
/// First-run Auto-workspace rail width in logical pixels.
pub const AUTO_WORKSPACE_RAIL_WIDTH_DEFAULT: f32 = 340.0;

/// Floor for the Strategies tree's local text-size step. Zero, not negative: a negative step would
/// let the user re-create, as a supported setting, the sub-`t_caption` defect the step's own fix
/// pass corrects (`strategies/tree/moon.rs`).
pub const STRATEGIES_TREE_TEXT_STEP_MIN: f32 = 0.0;
/// Ceiling for the step. Four is where `fit_height`'s line-height term still leaves headroom over
/// its `ui()` term at every global Font-slider setting; higher pushes the row's UI-scaled chrome
/// (checkbox, disclosure caret) past reading as part of the same row.
pub const STRATEGIES_TREE_TEXT_STEP_MAX: f32 = 4.0;
/// Shipped step: the pane renders at exactly the theme base, unchanged by this setting until the
/// user raises it.
pub const STRATEGIES_TREE_TEXT_STEP_DEFAULT: f32 = 0.0;

/// Share of a display's WORK AREA the very first window of a brand-new profile occupies.
///
/// Proportional rather than a pixel size on purpose: monitors differ, and any fixed number is
/// somebody's monitor and nobody else's.
pub const FIRST_RUN_WINDOW_FRACTION: f32 = 0.75;

/// Decide the workspace preset a layout should be seeded with, if any.
///
/// Split out of [`WindowLayout::load`] so the decision is a pure function with a mutation-sensitive
/// test rather than an inline branch reachable only by running startup against a real filesystem.
///
/// Auto is chosen for a brand-new profile because the rail gives a user with no cores something
/// coherent to look at, where Classic opens onto an empty chart. It is a SEED and not an override:
/// once written it is an ordinary stored value, and any group's own entry outranks it.
///
/// Args:
///     age: Whether any file of a configured profile existed at launch.
///     stored: The preset already in the layout, if the file carried one.
///
/// Returns:
///     `Some` preset to write into the layout, or `None` to leave it exactly as loaded — which is
///     the answer for every established profile, and for a first run that somehow already has one.
pub fn first_run_workspace_mode(
    age: super::ProfileAge,
    stored: Option<WorkspaceMode>,
) -> Option<WorkspaceMode> {
    match (age, stored) {
        (super::ProfileAge::FirstRun, None) => Some(WorkspaceMode::AutoTrading),
        (super::ProfileAge::FirstRun, Some(_)) | (super::ProfileAge::Established, _) => None,
    }
}

/// A screen rectangle in logical pixels, free of any windowing toolkit.
///
/// Deliberately plain `f32` rather than a GPUI `Bounds`: this crate has no `gpui` dependency at
/// all, so the geometry below is unit-testable without a display, and the compiler — not
/// discipline — is what keeps it that way.
///
/// The coordinate SPACE is whatever the caller's is. Platforms disagree (Windows reports global
/// desktop coordinates while macOS reports every display relative to its own origin), and
/// [`first_run_window_rect`] never mixes spaces because it derives its result solely from the
/// rectangle it is handed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Place the first window of a brand-new profile: [`FIRST_RUN_WINDOW_FRACTION`] of the work area,
/// centred on it.
///
/// The minimum is enforced HERE rather than left to the window's min-size hint, because that hint
/// governs live resizing and does not clamp the initial bounds a window is created with.
///
/// Ordering note, and it is the only real trade-off in this function: the minimum is applied
/// first and the work-area clamp SECOND, so on a display too small to hold the minimum the clamp
/// WINS. A window wider than the screen puts its title bar and controls out of reach, which the
/// user cannot recover from; a window narrower than the minimum is merely cramped, and the OS
/// min-size hint re-grows it as soon as there is room.
///
/// Args:
///     work: The display's work area — the monitor minus its taskbar or dock.
///     min_w: Narrowest the window may be created.
///     min_h: Shortest the window may be created.
///
/// Returns:
///     A rectangle wholly inside the sanitized work area. A non-finite ORIGIN falls back to zero
///     and non-finite or non-positive DIMENSIONS collapse to zero — the two are sanitized
///     separately — so a display the platform could not describe yields a degenerate rectangle
///     rather than a `NaN` origin, and never a window nothing can place.
pub fn first_run_window_rect(work: ScreenRect, min_w: f32, min_h: f32) -> ScreenRect {
    let sane = |v: f32| if v.is_finite() && v > 0.0 { v } else { 0.0 };
    let (work_w, work_h) = (sane(work.w), sane(work.h));
    let (min_w, min_h) = (sane(min_w), sane(min_h));
    let (x0, y0) = (
        if work.x.is_finite() { work.x } else { 0.0 },
        if work.y.is_finite() { work.y } else { 0.0 },
    );

    let w = (work_w * FIRST_RUN_WINDOW_FRACTION)
        .round()
        .max(min_w)
        .min(work_w);
    let h = (work_h * FIRST_RUN_WINDOW_FRACTION)
        .round()
        .max(min_h)
        .min(work_h);

    // The clamp is not redundant with the centring: rounding a half-pixel can push the far edge one
    // pixel past the work area, and that pixel is the difference between a window that opens flush
    // against the screen edge and one the compositor may reposition.
    let x = (x0 + ((work_w - w) * 0.5).round()).clamp(x0, x0 + work_w - w);
    let y = (y0 + ((work_h - h) * 0.5).round()).clamp(y0, y0 + work_h - h);

    ScreenRect { x, y, w, h }
}

/// Persisted terminal workspace preset.
///
/// The serialized codes are an external layout contract. Unknown or wrong-typed values fall back
/// to [`Self::Classic`] so a newer or hand-edited preference cannot make the complete layout
/// document unreadable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WorkspaceMode {
    /// The existing chart-first, freely editable terminal workspace.
    #[default]
    Classic,
    /// The shared modular workspace with coordinated rail-owned core navigation.
    AutoTrading,
}

impl WorkspaceMode {
    /// Return the stable code written to `layout.toml`.
    ///
    /// Returns:
    ///     The English machine-readable code for this preset.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::AutoTrading => "auto-trading",
        }
    }

    /// Resolve a persisted code without rejecting the surrounding layout.
    ///
    /// Args:
    ///     code: Value read from the hand-editable layout document.
    ///
    /// Returns:
    ///     The matching preset, or [`Self::Classic`] for an unknown code.
    pub fn from_code(code: &str) -> Self {
        match code.trim() {
            "auto-trading" => Self::AutoTrading,
            _ => Self::Classic,
        }
    }
}

impl Serialize for WorkspaceMode {
    /// Serialize through [`Self::code`] so one stable-code authority serves every caller.
    ///
    /// Args:
    ///     serializer: Serde output receiving the machine-readable workspace code.
    ///
    /// Returns:
    ///     Serializer-specific success value.
    ///
    /// Errors:
    ///     Propagates serializer failures while writing the stable string.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for WorkspaceMode {
    /// Deserialize a workspace code leniently so it cannot invalidate `layout.toml`.
    ///
    /// Args:
    ///     deserializer: Serde input positioned at one workspace-mode value.
    ///
    /// Returns:
    ///     The saved preset, defaulting to Classic for every unsupported shape.
    ///
    /// Errors:
    ///     Propagates only input errors that prevent Serde from visiting the value.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// A supported text code or an ignored malformed value.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StoredMode {
            /// Stable workspace code.
            Text(String),
            /// Any future or malformed non-text shape.
            Other(serde::de::IgnoredAny),
        }

        Ok(match StoredMode::deserialize(deserializer)? {
            StoredMode::Text(code) => Self::from_code(&code),
            StoredMode::Other(_) => Self::Classic,
        })
    }
}

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
///
/// Compared as a whole when deciding whether a move is worth persisting: the display is part of the
/// placement, and on macOS — where coordinates are relative to the window's own screen — the same
/// x/y on a different monitor is a real move that a coordinates-only comparison would discard.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeomRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// Whether the window was left MAXIMIZED.
    ///
    /// The rectangle above stays the RESTORE rectangle while this is set: the platform reports
    /// where a maximized window will go once it is un-maximized, and that is what has to survive
    /// a restart. The flag rides beside it rather than replacing it.
    ///
    /// Absent from older config files, and not written out when false, so an untouched file keeps
    /// its previous shape. Decoded leniently for the same reason [`Self::display_uuid`] is: one
    /// mistyped value here would otherwise reject the WHOLE document and cost the user every
    /// window position and column width in it.
    #[serde(
        default,
        deserialize_with = "de_lenient_bool",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub maximized: bool,
    /// macOS fullscreen state (`WindowBounds::Fullscreen`). Separate from [`Self::maximized`]:
    /// the green macOS button produces Fullscreen rather than Maximized, and it must be restored
    /// using its own variant or the window will open normally.
    #[serde(
        default,
        deserialize_with = "de_lenient_bool",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub fullscreen: bool,
    /// Display this window was last seen on, when the platform could name one.
    ///
    /// `x`/`y` alone identify a monitor only where window coordinates are global — Windows and X11.
    /// macOS reports them RELATIVE to the window's own screen and reports a zero origin for every
    /// display, so there the saved point cannot say which monitor it belongs to and the window comes
    /// back on whichever display the app happens to open it on. This is that missing half; it is a
    /// hint, never a requirement — an unplugged or renumbered monitor simply fails to resolve and
    /// the caller falls back to the coordinate and owner-window routes it used before.
    ///
    /// Absent from older config files and from a window whose platform reports no display id, hence
    /// `Option` plus `serde(default)`; it is not written out when absent so an untouched file keeps
    /// its previous shape.
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_uuid: Option<uuid::Uuid>,
}

impl GeomRect {
    /// Keep a previously known display when the platform cannot name one right now.
    ///
    /// `None` from the platform means "unknown", not "moved to nowhere": off macOS it is the normal
    /// answer (the identity is not read there at all), and even on macOS a window mid-move can
    /// briefly report no display. Treating that as a change would erase a good identity — and,
    /// because the comparison that decides whether to save is now whole-struct, would also dirty
    /// the layout on every such blip.
    ///
    /// # Arguments
    ///
    /// * `previous` - Geometry this window was last saved with, if any.
    ///
    /// # Returns
    ///
    /// This rectangle, keeping the earlier identity when it has none of its own.
    #[must_use]
    pub fn keeping_display_of(mut self, previous: Option<GeomRect>) -> Self {
        self.display_uuid = self
            .display_uuid
            .or_else(|| previous.and_then(|previous| previous.display_uuid));
        self
    }

    /// Whether this rectangle still lands somewhere the user can actually reach.
    ///
    /// A saved geometry outlives the monitors it was saved on: a laptop undocked from a second
    /// screen, a display rearranged, a resolution changed. Restoring such a rectangle opens the
    /// window at coordinates no monitor covers, where it is invisible and — for a window that
    /// hides its taskbar button, as the trade window does — unreachable. The caller falls back to
    /// its own default placement instead.
    ///
    /// The test is OVERLAP AREA, not containment: a window deliberately hanging off the edge of a
    /// screen is a placement the user chose, and demanding full containment would move it back on
    /// every reopen. What it rejects is a rectangle whose intersection with every attached display
    /// is too small to grab — the title bar has to be on a screen for the window to be draggable.
    ///
    /// Pure, and free of any window-system type, so the rule is unit-testable without a display:
    /// `displays` is simply the attached monitors as `(x, y, w, h)` in the same coordinate space
    /// the rectangle was saved in. An EMPTY list means the caller could not enumerate displays at
    /// all, which is "unknown" rather than "nowhere" — the saved rectangle is kept, exactly as
    /// [`Self::keeping_display_of`] keeps an unknown identity.
    ///
    /// # Arguments
    ///
    /// * `displays` - Attached display rectangles as `(x, y, w, h)`.
    /// * `min_visible` - Smallest visible area, in square pixels, that still counts as reachable.
    ///
    /// # Returns
    ///
    /// `true` when the rectangle is usable as-is.
    #[must_use]
    pub fn is_reachable_on(&self, displays: &[(i32, i32, u32, u32)], min_visible: u64) -> bool {
        if self.w == 0 || self.h == 0 {
            return false;
        }
        if displays.is_empty() {
            return true;
        }
        let (left, top) = (i64::from(self.x), i64::from(self.y));
        let (right, bottom) = (left + i64::from(self.w), top + i64::from(self.h));
        displays.iter().any(|&(dx, dy, dw, dh)| {
            let (dleft, dtop) = (i64::from(dx), i64::from(dy));
            let (dright, dbottom) = (dleft + i64::from(dw), dtop + i64::from(dh));
            let overlap_w = right.min(dright) - left.max(dleft);
            let overlap_h = bottom.min(dbottom) - top.max(dtop);
            overlap_w > 0 && overlap_h > 0 && (overlap_w as u64) * (overlap_h as u64) >= min_visible
        })
    }
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

/// One Report toolbar filter set, persisted per host context.
///
/// Holds seven stored members: the six shared Report toolbar filters that decide WHICH TRADES the
/// panel reads — direction, order kind, the deleted-only switch, the open-positions switch, the
/// single-server period preset, and the Auto strategy-name mask — plus the Auto Overview period
/// preset. The comment pane is a
/// display choice and stays in `app_meta` beside the other view preferences; the split is
/// deliberate, so do not "unify" the two stores. These filters belong here because they must
/// survive a quit that a detached preference write would not: the whole layout rides the quit
/// snapshot, and it outlives a report replica that integrity recovery retires.
///
/// Every field is optional and read leniently, so a wrongly-typed member drops only THAT field to
/// `None` and leaves its neighbours, and the rest of the layout, intact. Unknown string ids remain
/// stored here because this crate does not own their vocabulary; the Report decoder treats them as
/// no instruction and keeps the panel's current value. One level up the salvage is coarser: an
/// entry that is not a table at all takes the whole `report_filters` map down to empty with it, the
/// same as every other leniently-read map here. Both outcomes cost only filter preferences.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportFilterPrefs {
    /// Direction filter id.
    ///
    /// Opaque here: this crate stores it, and the Report panel's own encoder in `moon-ui-gpui`
    /// owns the vocabulary. Listing the values in both places is how one copy goes quietly wrong.
    #[serde(default, deserialize_with = "de_lenient")]
    pub side: Option<String>,
    /// Order-kind id, opaque here for the same reason as [`Self::side`].
    #[serde(default, deserialize_with = "de_lenient")]
    pub kind: Option<String>,
    /// Whether the panel showed only soft-deleted trades.
    #[serde(default, deserialize_with = "de_lenient")]
    pub deleted_only: Option<bool>,
    /// Whether the panel admits still-running positions alongside closed trades when its host does
    /// not force closed rows.
    ///
    /// A LIFECYCLE axis, independent of [`Self::kind`], which is about a trade's ORIGIN. Absent
    /// means "no instruction", and the Report decoder then keeps its own default of ON — which is
    /// exactly what every file written before this field existed must continue to mean.
    #[serde(default, deserialize_with = "de_lenient")]
    pub show_open: Option<bool>,
    /// Classic and Auto single-server period preset id — the panel's menu key, opaque here for
    /// the same reason as [`Self::side`].
    ///
    /// Only an explicit menu pick is stored. Typing a manual date also displays "all", but that is
    /// a consequence of the date rather than a chosen preset, so it never reaches this field.
    #[serde(default, deserialize_with = "de_lenient")]
    pub period: Option<String>,
    /// Auto Overview period preset id, falling back to [`Self::period`] when absent or unknown.
    ///
    /// Only an explicit menu pick is stored, matching the manual-date rule on [`Self::period`].
    #[serde(default, deserialize_with = "de_lenient")]
    pub period_overview: Option<String>,
    /// Literal strategy-name substring retained for group Auto mode.
    ///
    /// `Some("")` is a deliberate clear. A missing or malformed value leaves the panel's current
    /// value standing when it changes host context.
    #[serde(default, deserialize_with = "de_lenient")]
    pub strategy_name_mask: Option<String>,
}

/// One user-selected table sort stored under a stable per-context table id.
///
/// Column vocabulary remains panel-owned: this core crate only preserves the stable key and the
/// direction MoonUI reports. Panels validate the key against their current descriptors before
/// adopting it, so a renamed or removed column cannot make a table unusable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSortPreference {
    /// Stable column key defined by the owning table.
    pub column: String,
    /// Whether the selected column is ordered ascending.
    pub ascending: bool,
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
    /// Profit Monitor: whether a group caption carries the start/stop control for every core it
    /// names, sending one command per core.
    #[serde(default, deserialize_with = "de_lenient")]
    pub profit_monitor_group_trading: Option<bool>,
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
    /// "Analytics" strategy-name mask: a literal, case-insensitive part of the strategy name.
    /// None or empty = no filter.
    ///
    /// A flat field rather than an entry in [`Self::report_filters`], because Analytics is a
    /// singleton tool window with no host context to key one by: every other Analytics preference
    /// beside it is flat for the same reason. Read leniently like its neighbours — this block is
    /// hand-edited, and one wrongly typed value must not cost the user the rest of the file.
    #[serde(default, deserialize_with = "de_lenient")]
    pub analytics_strategy_mask: Option<String>,
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
    /// Defaults for tabs under the anchor lock, wherever they live. Empty means "follow Main".
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
    /// Expiring exchange API-key warning (per core).
    #[serde(default = "def_true")]
    pub api: bool,
    /// Exhausting exchange API request quota warning (per core).
    #[serde(default = "def_true")]
    pub api_quota: bool,
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
            api: true,
            api_quota: true,
        }
    }
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

/// Default bottom-volume display style.
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

/// Chart drawing settings edited from the toolbar's palette popup.
///
/// Stored here as the GLOBAL DEFAULT: each chart tab may hold its own set in `charts.json`, and a
/// tab without one draws with this.
///
/// Deliberately separate from `OrdersStyle` in `orders.toml`: that file describes how each ORDER
/// LINE is painted (colour, dash, marker sizes), while these values decide how order-line repricing
/// and closed-trade history, the trade marks, and the bottom volume band are drawn, and which
/// closed TRADES appear at all. Mixing them would make two unrelated surfaces move together, which
/// is the same reason `trade_marks.rs` refused to read `orders.toml`.
///
/// The last six fields moved here from `ChartTheme` (`theme.toml`). They belong to a CHART TAB, not
/// to a colour scheme: two tabs on one theme routinely want different marker sizes and a different
/// volume band. One consequence is deliberate and worth knowing — `ChartTheme::apply_light_defaults`
/// used to give the light theme its own `candle_volume_alpha` and `candle_volume_scale`, and a
/// per-tab value cannot vary by theme mode, so that pair no longer switches with the mode.
/// `moonterminal::startup::graphics_migration` carries every existing user's values across.
///
/// The numeric fields are NOT clamped here, because `layout.toml` is hand-editable and the drawing
/// path and the hit-test path must clamp IDENTICALLY or the glyph and the region that responds to
/// it drift apart. Every clamp, and the `normalize_chart_graphics` that each storing or comparing
/// site applies, therefore live together in `moon_chart::trade_marks`.
///
/// Every field decodes LENIENTLY and independently, because the whole document is one
/// deserialization (see [`WindowLayout`]): a hand-typed `show_real_trades = "yes"` falls back to
/// that one field's default instead of resetting the others — or the file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChartGraphicsCfg {
    /// Multiplier on the trade-history arrow's half-extents. One is the size the layer shipped with.
    #[serde(default = "def_trade_arrow_scale", deserialize_with = "de_arrow_scale")]
    pub trade_arrow_scale: f32,
    /// Thickness of the dashed entry-to-exit connector, in LOGICAL px.
    #[serde(
        default = "def_connector_thickness_px",
        deserialize_with = "de_connector_thickness"
    )]
    pub connector_thickness_px: f32,
    /// Whether closed trades made by a REAL (non-emulator) order draw their history marks.
    ///
    /// This pair once selected which ORDER LINES were drawn. It selects TRADES now: the order lines
    /// a core reports are few and every one of them is actionable, while closed-trade history is the
    /// crowded layer where telling a live result from an emulated one is what the user needs.
    #[serde(default = "def_true", deserialize_with = "de_lenient_true")]
    pub show_real_trades: bool,
    /// Whether closed trades made by an EMULATOR order draw their history marks.
    #[serde(default = "def_true", deserialize_with = "de_lenient_true")]
    pub show_emulator_trades: bool,
    /// Whether a CLOSED order hides its sell-price line. Live orders always keep theirs.
    ///
    /// On by default: after an order closes, its blue sell line stays on the chart at
    /// `closed_alpha` and reads as a live price the terminal is still tracking.
    #[serde(default = "def_true", deserialize_with = "de_lenient_true")]
    pub hide_closed_sell_line: bool,
    /// Whether an order line hides its repricing history: the server-reported trace, the locally
    /// reconstructed staircase, and the knot marking each reprice. The server's `SetStopPrice`
    /// segment remains visible because it records where a stop sat, rather than a reprice.
    ///
    /// OFF by default, unlike its neighbour above: it removes information a user may rely on, so
    /// it is opt-in rather than opt-out.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub hide_order_move_history: bool,

    // --- Trade marks. Moved here from `ChartTheme` so they are per TAB rather than per theme. ---
    /// Multiplier on the trade-cross marker size. The device pixel ratio is applied separately and
    /// is not part of this number.
    ///
    /// It MULTIPLIES with [`Self::trade_arrow_scale`] and is a different knob: that one sizes the
    /// closed-trade HISTORY arrows, this one the live trade crosses.
    #[serde(default = "def_marker_scale", deserialize_with = "de_marker_scale")]
    pub marker_scale: f32,
    /// Opacity of the per-TRADE volume bars along the plot's bottom edge, 0..1. Distinct from
    /// [`Self::candle_volume_alpha`], which is the per-CANDLE band drawn beneath them.
    #[serde(
        default = "def_trade_volume_alpha",
        deserialize_with = "de_trade_volume_alpha"
    )]
    pub trade_volume_alpha: f32,

    // --- Bottom candle volumes, likewise moved off `ChartTheme`. ---
    /// Display style: `crate::market::candles::VOLUME_STYLE_OFF` / `_BARS` / `_HILLS`.
    #[serde(
        default = "def_candle_volume_style",
        deserialize_with = "de_candle_volume_style"
    )]
    pub candle_volume_style: u8,
    /// Band height as a fraction of the plot height, 0..1. Capped in physical pixels by the
    /// geometry module so the band cannot swallow a tall chart.
    #[serde(
        default = "def_candle_volume_height",
        deserialize_with = "de_candle_volume_height"
    )]
    pub candle_volume_height: f32,
    /// Bottom-volume opacity, 0..1. The band's colours come from the candle colours, which stay on
    /// the theme — only the opacity is per tab.
    #[serde(
        default = "def_candle_volume_alpha",
        deserialize_with = "de_candle_volume_alpha"
    )]
    pub candle_volume_alpha: f32,
    /// Colour of the volume scale's max and average reference lines, sRGB.
    #[serde(
        default = "def_candle_volume_scale",
        deserialize_with = "de_candle_volume_scale"
    )]
    pub candle_volume_scale: [u8; 3],
}

impl Default for ChartGraphicsCfg {
    /// The shipped sizes with every trade kind visible, the closed sell line hidden, and the
    /// order move-history trail shown.
    fn default() -> Self {
        Self {
            trade_arrow_scale: def_trade_arrow_scale(),
            connector_thickness_px: def_connector_thickness_px(),
            show_real_trades: true,
            show_emulator_trades: true,
            hide_closed_sell_line: true,
            hide_order_move_history: false,
            marker_scale: def_marker_scale(),
            trade_volume_alpha: def_trade_volume_alpha(),
            candle_volume_style: def_candle_volume_style(),
            candle_volume_height: def_candle_volume_height(),
            candle_volume_alpha: def_candle_volume_alpha(),
            candle_volume_scale: def_candle_volume_scale(),
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
    /// Expiring exchange API-key axis.
    pub api: ApiWarn,
    /// Exhausting API-request-quota axis.
    #[serde(default)]
    pub api_quota: ApiQuotaWarn,
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

/// Largest API-key warning horizon offered and honoured: the alert popup's stepper range, and the
/// ceiling the engine clamps a hand-edited `layout.toml` to. One constant so the two cannot drift.
pub const API_WARN_MAX_DAYS: u16 = 90;

/// Expiring-API-key parameters: the alert sound and how many days ahead the warning starts.
///
/// No `chart` field, unlike every other axis: this one has no per-second history, so a chart badge
/// would open a card with nothing to draw in it. The warning is a Core Status state, not a moment
/// in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiWarn {
    pub sound: Option<String>,
    /// The warning is on from this many days before expiration and stays on until the key is
    /// replaced. `0` warns on the key's LAST DAY and after — not only once it has expired, because
    /// the count is in whole days and reaches zero while up to a day of life remains.
    pub days: u16,
}
impl Default for ApiWarn {
    fn default() -> Self {
        Self {
            sound: None,
            days: 7,
        }
    }
}

/// Smallest API-request quota the warning can be armed at, and the popup stepper's ceiling.
///
/// The bound is `u16` rather than a round number because two structures downstream are `u16`: the
/// alert popup's stepper (`Param`) and the episode's `peak`. Raising it past that would silently
/// truncate the number an episode records about itself.
pub const API_QUOTA_WARN_MAX: u16 = u16::MAX;

/// Exhausting API-request-quota parameters: the alert sound and the quota the warning starts at.
///
/// Today only HyperLiquid cores report a quota, and the value is address-level rather than
/// per-market. No `chart` field, for the same reason as [`ApiWarn`]: the quota is a standing state
/// the terminal receives every few minutes, not a per-second series a badge could draw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiQuotaWarn {
    pub sound: Option<String>,
    /// The warning is on while the remaining quota is at or below this many requests, and clears
    /// when the quota climbs back above it. Unlike the day counts of [`ApiWarn`] this number is a
    /// COUNT of requests: a HyperLiquid address earns quota with volume, so it moves both ways.
    pub min: u16,
}
impl Default for ApiQuotaWarn {
    fn default() -> Self {
        Self {
            sound: None,
            min: 5000,
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

/// Clamp a persisted or runtime Auto rail width to the globally usable range.
///
/// Args:
///     width: Logical-pixel width from persistence or a resize event.
///
/// Returns:
///     A finite width within the supported range, or the first-run default for non-finite input.
pub fn clamp_auto_workspace_rail_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(AUTO_WORKSPACE_RAIL_WIDTH_MIN, AUTO_WORKSPACE_RAIL_WIDTH_MAX)
    } else {
        AUTO_WORKSPACE_RAIL_WIDTH_DEFAULT
    }
}

/// Clamp a persisted or runtime Strategies tree text step to the supported integer range.
///
/// The `round()` is load-bearing: the stepper control only ever emits integers, and a
/// hand-written fractional value must not produce a half-step the UI cannot represent or
/// return to.
///
/// Args:
///     value: Step from persistence or a stepper change event.
///
/// Returns:
///     A whole-number step within `STRATEGIES_TREE_TEXT_STEP_MIN..=STRATEGIES_TREE_TEXT_STEP_MAX`,
///     or the shipped default for non-finite input.
pub fn clamp_strategies_tree_text_step(value: f32) -> f32 {
    if value.is_finite() {
        value
            .round()
            .clamp(STRATEGIES_TREE_TEXT_STEP_MIN, STRATEGIES_TREE_TEXT_STEP_MAX)
    } else {
        STRATEGIES_TREE_TEXT_STEP_DEFAULT
    }
}

impl WindowLayout {
    /// The candle settings a tab of this kind opens with.
    pub fn candle_view_for(
        &self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> crate::market::candles::CandleViewCfg {
        self.kind_defaults(kind)
            .and_then(|d| d.candle_view)
            .unwrap_or(self.candle_view)
    }

    /// The chart graphics a tab of this kind opens with.
    pub fn chart_graphics_for(
        &self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> ChartGraphicsCfg {
        self.kind_defaults(kind)
            .and_then(|d| d.chart_graphics)
            .unwrap_or(self.chart_graphics)
    }

    /// The captions a tab of this kind opens with.
    pub fn chart_labels_for(
        &self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> &super::chart_labels::ChartLabelsCfg {
        // Stored, else the kind's own shipped set, else Main's — one chain, with "a kind may ship
        // its own captions" living on the KIND rather than as a branch here. The shipped set is
        // built once and shared: this is read on every settings comparison, and a fresh clone per
        // read would make a panel's signature differ from itself.
        self.kind_defaults(kind)
            .and_then(|d| d.chart_labels.as_ref())
            .or_else(|| kind.builtin_labels())
            .unwrap_or(&self.chart_labels)
    }

    /// Store the candle default for one kind, reporting whether it actually moved.
    pub fn set_candle_view_default(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
        value: crate::market::candles::CandleViewCfg,
    ) -> bool {
        // No kind ships its own candles: every one of them follows Main until it is given a value.
        let split =
            self.split_defaults(|d| &mut d.candle_view, |l, k| l.candle_view_for(k), |_| false);
        let moved = match self.kind_defaults_mut(kind) {
            Some(d) => std::mem::replace(&mut d.candle_view, Some(value)) != Some(value),
            None => std::mem::replace(&mut self.candle_view, value) != value,
        };
        split || moved
    }

    /// Store the graphics default for one kind, reporting whether it actually moved.
    pub fn set_chart_graphics_default(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
        value: ChartGraphicsCfg,
    ) -> bool {
        let split = self.split_defaults(
            |d| &mut d.chart_graphics,
            |l, k| l.chart_graphics_for(k),
            |_| false,
        );
        let moved = match self.kind_defaults_mut(kind) {
            Some(d) => std::mem::replace(&mut d.chart_graphics, Some(value)) != Some(value),
            None => std::mem::replace(&mut self.chart_graphics, value) != value,
        };
        split || moved
    }

    /// Store the caption default for one kind, reporting whether it actually moved.
    pub fn set_chart_labels_default(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
        value: super::chart_labels::ChartLabelsCfg,
    ) -> bool {
        let split = self.split_defaults(
            |d| &mut d.chart_labels,
            |l, k| l.chart_labels_for(k).clone(),
            // The trade window ships its own captions; see the guard inside.
            |k| k.builtin_labels().is_some(),
        );
        // `|` rather than `||`: the store must run even when the separation already reported a
        // change, or the pressed value would never be written.
        split | self.store_chart_labels(kind, value)
    }

    /// Store one kind's captions and NOTHING else, reporting whether they moved.
    ///
    /// No separation of the other kinds: the caller is recording what ONE view is showing, not
    /// making a statement about the others. Separating them is a deliberate press with its own
    /// wording in the popup — a right-click toggle inside a window must not perform it silently.
    ///
    /// Args:
    ///     kind: The kind whose captions are being stored.
    ///     value: The set to store.
    ///
    /// Returns:
    ///     Whether the stored value actually changed.
    pub fn store_chart_labels(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
        value: super::chart_labels::ChartLabelsCfg,
    ) -> bool {
        match self.kind_defaults_mut(kind) {
            Some(d) => d.chart_labels.replace(value.clone()) != Some(value),
            None => std::mem::replace(&mut self.chart_labels, value.clone()) != value,
        }
    }

    /// This kind's own defaults, or `None` for Main, whose defaults are the base fields.
    fn kind_defaults(
        &self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> Option<&super::chart_defaults::ChartTabDefaults> {
        match kind {
            super::chart_defaults::ChartTabKind::Main => None,
            super::chart_defaults::ChartTabKind::AddTo => Some(&self.chart_defaults_addto),
            super::chart_defaults::ChartTabKind::Compare => Some(&self.chart_defaults_compare),
            super::chart_defaults::ChartTabKind::Trade => Some(&self.chart_defaults_trade),
        }
    }

    fn kind_defaults_mut(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> Option<&mut super::chart_defaults::ChartTabDefaults> {
        match kind {
            super::chart_defaults::ChartTabKind::Main => None,
            super::chart_defaults::ChartTabKind::AddTo => Some(&mut self.chart_defaults_addto),
            super::chart_defaults::ChartTabKind::Compare => Some(&mut self.chart_defaults_compare),
            super::chart_defaults::ChartTabKind::Trade => Some(&mut self.chart_defaults_trade),
        }
    }

    /// Freeze the kinds this write is NOT addressing at what they currently show.
    ///
    /// Until the first press the two non-Main kinds hold nothing and follow Main, which is what a
    /// profile that never used the feature wants. The moment one kind is given its own value that
    /// stops being true: without this, setting the Main default would still drag the other two
    /// along, and the reader who just separated them would watch them move together anyway.
    ///
    /// Per SETTING, not per kind — separating the captions must not freeze the candles as well —
    /// and only where the value is still absent, so it can never overwrite a stored default.
    ///
    /// Returns whether it wrote anything, and the caller must fold that into its own "changed"
    /// answer: a press that stores a value already in the file still SPLIT the kinds apart, and
    /// reporting "nothing moved" would leave that split in memory only, to be lost on restart.
    fn split_defaults<T: Clone>(
        &mut self,
        slot: impl Fn(&mut super::chart_defaults::ChartTabDefaults) -> &mut Option<T>,
        base: impl Fn(&Self, super::chart_defaults::ChartTabKind) -> T,
        ships_builtin: impl Fn(super::chart_defaults::ChartTabKind) -> bool,
    ) -> bool {
        let mut wrote = false;
        // Every kind: `kind_defaults_mut` answers `None` for Main, whose defaults ARE the base
        // fields, so this needs no hand-kept list of "the others" — a new kind is covered by
        // adding it to `ALL` and nowhere else.
        for kind in super::chart_defaults::ChartTabKind::ALL {
            // A kind that SHIPS its own set does not follow Main and has nothing to be separated
            // from: freezing it here would copy today's built-in value into the profile, and the
            // reader would then be stuck on it — a later build could improve that set and never
            // reach them, because their file now holds a copy made the first time they pressed
            // "make default" on some entirely different kind.
            if ships_builtin(kind) {
                continue;
            }
            // Asked and released BEFORE the value is built, so the two borrows never overlap and
            // the value — a whole caption configuration — is only cloned for a slot that will
            // actually take it.
            let empty = match self.kind_defaults_mut(kind) {
                Some(defaults) => slot(defaults).is_none(),
                None => false,
            };
            if !empty {
                continue;
            }
            // Read per KIND rather than one Main value copied to all: the trade window's captions
            // do not follow Main, so freezing it at Main's set would replace the view the reader is
            // looking at with a different one the first time any OTHER kind's default was set.
            let value = base(self, kind);
            if let Some(defaults) = self.kind_defaults_mut(kind) {
                *slot(defaults) = Some(value);
                wrote = true;
            }
        }
        wrote
    }

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
