//! Chart-tab persistence, based on the idea in `detached.rs` but using a dedicated schema. Each tab
//! is keyed by group, number, and [`ChartBucket`]. Its `charts.json` specification can store scale
//! and layout settings, display overrides, custom-tab composition and comparison state, plus window
//! geometry when detached. The file lives at `paths::charts_path()` under the configuration directory.
//!
//! At startup, ordinary detached AddToChart tabs reopen empty at their saved geometry and display
//! only the branded logo until `ChartTabs::ingest` populates them by number and bucket. Custom tabs
//! restore their explicit `custom_coins` immediately. Live camera position is not persisted; newly
//! ingested markets begin in live-follow, while the schema restores only its explicit scale settings.

use moon_core::config::{ChartBucket, ServerConfig, paths};
use moon_core::session::CoreId;
use serde::{Deserialize, Serialize};

/// Geometry of a detached chart-tab window.
///
/// Compared as a whole where "did this window move?" is asked: the display is part of the placement,
/// and on macOS the same x/y on another monitor is a real move (coordinates there are relative to
/// the window's own screen).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinGeom {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// Display this window was last seen on, when the platform could name one.
    ///
    /// Without it a restored chart lands on whichever display its group window occupies, because
    /// x/y identify a monitor only where window coordinates are global. Absent from older
    /// `charts.json` files and from a platform that reports no display, so it stays optional and
    /// unresolvable identities fall back to the previous coordinate/owner routes.
    ///
    /// Decoded leniently: charts.json holds every chart tab; a malformed identity must not cost the user all of them.
    #[serde(
        default,
        deserialize_with = "moon_core::config::layout::de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_uuid: Option<uuid::Uuid>,
}

impl WinGeom {
    /// Keep a previously known display when the platform cannot name one right now.
    ///
    /// The `WinGeom` mirror of [`moon_core::config::layout::GeomRect::keeping_display_of`], carrying
    /// the same rule for the same reason: an unknown display means "unknown", not "moved to
    /// nowhere".
    ///
    /// Args:
    ///     previous: Geometry this window was last saved with, if any.
    ///
    /// Returns:
    ///     This geometry, keeping the earlier identity when it has none of its own.
    #[must_use]
    pub fn keeping_display_of(mut self, previous: Option<WinGeom>) -> Self {
        self.display_uuid = self
            .display_uuid
            .or_else(|| previous.and_then(|previous| previous.display_uuid));
        self
    }
}

/// Per-tab chart-stack layout mode:
/// - `Fit`: slot extent zero stretches charts to share the window; an extent of at least 20 enables
///   COMPRESS, using fixed slots without scrolling and shrinking on overflow.
/// - `Scroll`: uses a fixed slot extent and scrolls along the stack orientation.
///
/// Fit and Scroll retain separate values in `layout_height_fit` and `layout_height_scroll`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum StackLayoutMode {
    Fit,
    Scroll,
}

/// Per-tab chart-stack orientation. `Vertical` stacks charts from top to bottom and is the default;
/// `Horizontal` places them left to right. In horizontal mode, the layout popup's slot-height field
/// represents slot width, applying the same FIT, COMPRESS, and SCROLL rules along the X axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum StackOrientation {
    Vertical,
    Horizontal,
}

impl StackOrientation {
    /// Returns whether the orientation is horizontal. A missing specification value means Vertical.
    pub fn is_horizontal(self) -> bool {
        matches!(self, StackOrientation::Horizontal)
    }
}

/// Position of a market action button such as Cancel Buy or Panic Sell within the chart area, not
/// the order-book area. `Hide` suppresses it; a missing specification value defaults to `Right`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ChartBtnPos {
    Hide,
    Left,
    Center,
    Right,
}

impl Default for ChartBtnPos {
    fn default() -> Self {
        ChartBtnPos::Right
    }
}

/// Per-tab price-axis position relative to the chart and order book. `Left` places the gutter left
/// of the plot and is the historical default. `Right` places it to the right beyond the order book,
/// so the axis does not separate the plot from the book. `Hide` removes the axis and returns its
/// space to the plot. A missing specification value means `Left`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PriceAxisPos {
    Hide,
    Left,
    Right,
}

impl Default for PriceAxisPos {
    fn default() -> Self {
        PriceAxisPos::Left
    }
}

/// Persistent state for one chart tab. `num == 0` identifies Main; `num >= 1` identifies numbered
/// AddToChart or custom tabs.
#[derive(Clone, Serialize, Deserialize)]
pub struct ChartTabSpec {
    pub group: String,
    pub num: u32,
    /// Legacy key from `charts.json` before named bundles. Some(core) denotes a split core and None
    /// a shared tab. It remains readable for compatibility; new records write `bucket` and leave
    /// `core` as None.
    #[serde(default)]
    pub core: Option<CoreId>,
    /// Canonical tab key for a dedicated core, shared group, or named bundle. Older files omit it,
    /// in which case [`ChartTabSpec::bucket`] derives it from `core`.
    #[serde(default)]
    pub bucket: Option<ChartBucket>,
    #[serde(default)]
    pub scale: Option<f32>,
    /// Some means the tab is detached into a window with this geometry; None keeps it in the tab strip.
    #[serde(default)]
    pub detached: Option<WinGeom>,
    /// Stack layout mode for this tab. None defaults to Fit.
    #[serde(default)]
    pub layout_mode: Option<StackLayoutMode>,
    /// Fit-mode slot extent in pixels: height for Vertical and width for Horizontal. Zero stretches
    /// charts for normal Fit; at least 20 enables COMPRESS with fixed slots and no scrolling. None
    /// defaults to zero.
    #[serde(default)]
    pub layout_height_fit: Option<u16>,
    /// Scroll-mode slot extent in pixels: height for Vertical and width for Horizontal. None selects
    /// the default.
    #[serde(default)]
    pub layout_height_scroll: Option<u16>,
    /// Whether this tab's charts show the order book. None defaults to enabled. When disabled, the
    /// book and its label are not rendered; Stage 2 also avoids subscribing to book data unless
    /// another window requests it.
    #[serde(default)]
    pub orderbook_enabled: Option<bool>,
    /// Whether this tab's charts render liquidation trades. None defaults to enabled per window/tab.
    #[serde(default)]
    pub liquidations_enabled: Option<bool>,
    /// Whether to fill the management zone when zones are separate and the order book is hidden.
    /// None defaults to enabled per window/tab, like `orderbook_enabled`.
    #[serde(default)]
    pub show_zone: Option<bool>,
    /// Whether placing an order automatically pins its chart. None defaults to disabled per window/tab.
    #[serde(default)]
    pub auto_pin: Option<bool>,
    /// Per-window/tab stack orientation. None defaults to Vertical.
    #[serde(default)]
    pub layout_orientation: Option<StackOrientation>,
    /// Per-window/tab position of Cancel Buy in the chart area. None defaults to Right.
    #[serde(default)]
    pub cancel_buy_pos: Option<ChartBtnPos>,
    /// Per-window/tab position of Panic Sell in the chart area. None defaults to Right.
    #[serde(default)]
    pub panic_sell_pos: Option<ChartBtnPos>,
    /// Explicit `(core, market)` list for a custom multi-market tab created through search. Some
    /// classifies the specification as custom, causing startup to restore exactly these charts
    /// instead of waiting for detect ingest like an ordinary AddToChart tab. None is a regular tab.
    #[serde(default)]
    pub custom_coins: Option<Vec<(CoreId, String)>>,
    /// Editable custom-tab name from the settings popup. None uses the localized default Set N label.
    #[serde(default)]
    pub custom_label: Option<String>,
    /// Comparison-mode `(core, market)` anchor whose chart leads the price scale, shows the lock,
    /// and sits on the left. None disables comparison. Used only by horizontal, usually custom, tabs.
    #[serde(default)]
    pub compare_anchor: Option<(CoreId, String)>,
    /// Whether comparison peers use book-only broom mode, hiding the plot and price axis while
    /// leaving the order book visible.
    #[serde(default)]
    pub compare_orderbook_only: bool,
    /// Per-window/tab price-axis position. None defaults to Left.
    #[serde(default)]
    pub price_axis_pos: Option<PriceAxisPos>,
    /// Whether to show the time axis, including bottom labels and their gutter. None defaults to
    /// enabled per window/tab. When disabled, labels are omitted and the plot uses the full height.
    #[serde(default)]
    pub time_axis_visible: Option<bool>,
    /// Whether to show order-line labels. None defaults to enabled per window/tab.
    #[serde(default)]
    pub line_labels: Option<bool>,
    /// Whether to show crosshair cursor-readout labels. None defaults to enabled per window/tab.
    #[serde(default)]
    pub cursor_labels: Option<bool>,
    /// Per-window/tab candle and trade display settings such as timeframe, mode, and zone from the
    /// candle popup. None inherits the global `layout.candle_view` default.
    #[serde(default)]
    pub candle_view: Option<moon_core::market::CandleViewCfg>,
    /// Per-window/tab chart-drawing settings from the palette popup: trade-arrow size, connector
    /// thickness, closed-trade visibility, the closed order's sell line, trade-mark size, and the
    /// bottom-volume band. None inherits the global `layout.chart_graphics` default, which is what
    /// every file written before this field existed does.
    #[serde(default)]
    ///
    /// A `Some` written before the trade-mark and bottom-volume settings moved onto this struct
    /// carried only the original five values; `startup::graphics_migration` back-filled the other
    /// six once, from the user's old `theme.toml`.
    pub chart_graphics: Option<moon_core::config::ChartGraphicsCfg>,
    /// Per-window/tab chart captions from the labels popup: which figures the chart prints beside
    /// its plot, in which corner and style. None inherits the global `layout.chart_labels` default,
    /// which is what every file written before this field existed does.
    ///
    /// Read LENIENTLY, like `layout.toml` reads its own copy: this file holds every chart tab and
    /// [`load_all`] discards ALL of them on a parse error, so one unreadable caption — a field name
    /// from a newer build, a hand-edited spelling — would cost the user their whole chart layout.
    /// Dropping the captions of one tab costs that tab its captions and nothing else.
    #[serde(default, deserialize_with = "de_lenient_labels")]
    pub chart_labels: Option<moon_core::config::ChartLabelsCfg>,
    /// Detached-window time-axis X scale in pixels per millisecond, synchronized there with
    /// Shift+middle-click. None inherits the group scale or the built-in chart default.
    #[serde(default)]
    pub x_ppm: Option<f32>,
}

impl ChartTabSpec {
    /// Creates a group/number/bucket specification with schema defaults: optional fields are None
    /// and comparison book-only mode is false. [`upsert`] uses this to avoid duplicating the long
    /// field literal across persistence paths.
    pub fn new(group: String, num: u32, bucket: ChartBucket) -> Self {
        Self {
            group,
            num,
            core: None,
            bucket: Some(bucket),
            scale: None,
            detached: None,
            layout_mode: None,
            layout_height_fit: None,
            layout_height_scroll: None,
            orderbook_enabled: None,
            liquidations_enabled: None,
            show_zone: None,
            auto_pin: None,
            layout_orientation: None,
            cancel_buy_pos: None,
            panic_sell_pos: None,
            custom_coins: None,
            custom_label: None,
            compare_anchor: None,
            compare_orderbook_only: false,
            price_axis_pos: None,
            time_axis_visible: None,
            line_labels: None,
            cursor_labels: None,
            candle_view: None,
            chart_graphics: None,
            chart_labels: None,
            x_ppm: None,
        }
    }

    /// Returns the canonical tab key from `bucket`, or derives it from legacy `core`: Some(core)
    /// becomes Core and None becomes Shared.
    pub fn bucket(&self) -> ChartBucket {
        self.bucket.clone().unwrap_or_else(|| match self.core {
            Some(id) => ChartBucket::Core(id),
            None => ChartBucket::Shared,
        })
    }

    /// Returns whether this specification matches a group, number, and bucket key. This centralizes
    /// the predicate used by `chart_specs.iter().find(...)` calls.
    pub fn matches(&self, group: &str, num: u32, bucket: &ChartBucket) -> bool {
        self.group == group && self.num == num && self.bucket() == *bucket
    }
}

/// Finds a group/number/bucket specification and applies `f`, or creates one with
/// [`ChartTabSpec::new`] before applying `f`. This is the shared upsert for `ChartTabs` and
/// `DetachedChartHost` persistence paths. The caller sets Backend's `chart_specs_dirty` flag.
pub fn upsert(
    specs: &mut Vec<ChartTabSpec>,
    group: &str,
    num: u32,
    bucket: &ChartBucket,
    f: impl FnOnce(&mut ChartTabSpec),
) {
    if let Some(s) = specs.iter_mut().find(|s| s.matches(group, num, bucket)) {
        f(s);
    } else {
        let mut s = ChartTabSpec::new(group.to_string(), num, bucket.clone());
        f(&mut s);
        specs.push(s);
    }
}

/// Remaps legacy positional CoreIds in `Core(n)` or `core = n` to stable UIDs. This runs exactly
/// once while upgrading a configuration older than `COREID_UID_VERSION`, as indicated by
/// `AppConfig::chart_core_remap_needed`, while server order in `servers.enc` still matches the order
/// used to write `charts.json`. Before v11, `n` was a one-based position in the server list, so it
/// maps to `servers[n - 1].uid`. Out-of-range IDs from orphaned tabs remain unchanged; no live core
/// has that ID, so the tab remains empty.
///
/// Schema versioning provides one-shot safety. Do not call this again on an already remapped file,
/// because a stable UID would be misinterpreted as a position and rebound incorrectly.
pub fn remap_core_ids(specs: &mut [ChartTabSpec], servers: &[ServerConfig]) {
    let pos_to_uid = |n: CoreId| -> Option<CoreId> {
        // Map the old one-based positional ID to the server occupying that position.
        n.checked_sub(1)
            .and_then(|i| servers.get(i as usize))
            .map(|s| s.uid)
    };
    let mut remapped = 0usize;
    for spec in specs.iter_mut() {
        if let Some(ChartBucket::Core(n)) = spec.bucket {
            if let Some(uid) = pos_to_uid(n) {
                spec.bucket = Some(ChartBucket::Core(uid));
                remapped += 1;
            }
        }
        if let Some(n) = spec.core {
            if let Some(uid) = pos_to_uid(n) {
                spec.core = Some(uid);
                remapped += 1;
            }
        }
    }
    log::info!(
        "charts.json: ремап позиционных CoreId → uid ({remapped} вкладок, {} серверов)",
        servers.len()
    );
}

/// Highest core uid any saved chart tab still references.
///
/// Feeds the durable uid high-water mark. Every one of these fields keys a tab, a custom coin
/// list or a comparison anchor by core, so reissuing a uid a saved tab still names would
/// silently reopen the deleted core's chart state against the new core.
///
/// A file written before `COREID_UID_VERSION` holds POSITIONAL ids rather than uids. They are
/// folded in regardless: the floor is monotonic, so these small legacy values can only cause a
/// conservative skip of a few uid numbers before the file is remapped.
pub fn max_core_uid(specs: &[ChartTabSpec]) -> Option<u64> {
    specs
        .iter()
        .flat_map(|s| {
            let bucket = match s.bucket {
                Some(ChartBucket::Core(core)) => Some(core),
                _ => None,
            };
            let coins = s.custom_coins.iter().flatten().map(|(core, _)| *core);
            s.core
                .into_iter()
                .chain(bucket)
                .chain(s.compare_anchor.iter().map(|(core, _)| *core))
                .chain(coins)
        })
        .max()
}

/// Read one tab's captions without letting them reject the file, and repair what they state.
///
/// The salvaging half is `moon_core`'s shared helper — the same one the `display_uuid` field above
/// uses, and the same one `layout.toml` reads its own copy of this configuration with. Only the
/// repair is added here, for the reason that file states: a value read but not sanitized would
/// differ from the stored one on every comparison downstream.
fn de_lenient_labels<'de, D>(d: D) -> Result<Option<moon_core::config::ChartLabelsCfg>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut cfg = moon_core::config::layout::de_lenient::<D, moon_core::config::ChartLabelsCfg>(d)?;
    if let Some(cfg) = cfg.as_mut() {
        cfg.sanitize();
    }
    Ok(cfg)
}

/// Loads `charts.json`, returning an empty list when the file is absent or invalid.
pub fn load_all() -> Vec<ChartTabSpec> {
    match std::fs::read_to_string(paths::charts_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log::warn!("charts.json битый ({e}) → без сохранённых вкладок");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Saves specifications to `charts.json`; failures are logged but nonfatal.
///
/// Returns whether the file was actually replaced. Almost every caller ignores that — a failed
/// autosave is a warning, not a stop — but the one-shot graphics migration must NOT commit its
/// "already migrated" marker unless these specs really reached the disk, or a failure here would
/// permanently skip the tabs it was about to repair.
///
/// Args:
///     list: The specs to write.
///
/// Returns:
///     True when `charts.json` now holds `list`.
pub fn save_all(list: &[ChartTabSpec]) -> bool {
    moon_core::detect_diag::line(&format!(
        "[save] charts.json: {} спек, detached(окна)={}",
        list.len(),
        list.iter().filter(|s| s.detached.is_some()).count()
    ));
    match serde_json::to_string_pretty(list) {
        Ok(s) => {
            if let Err(e) = moon_core::config::write_file_atomic(
                &paths::charts_path(),
                s.as_bytes(),
                "charts.json",
            ) {
                log::warn!("не записал charts.json: {e}");
                false
            } else {
                true
            }
        }
        Err(e) => {
            log::warn!("не сериализовал charts.json: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests;
