//! Shell for one group: one OS window containing a fixed header and trading toolbar, one
//! `DockArea`, and a fixed status bar.
//! Extracted from `main.rs`. `Backend` remains at the crate root, whose private fields are visible
//! to this descendant module.
//!
//! Responsibilities are split across submodules:
//! - [`init`] constructs `Shell`, builds the dock, and wires observers and subscriptions;
//! - [`actions`] drains inline-edit requests and engine-action toasts and handles window hotkeys;
//! - [`core_settings`] owns the core-settings popup state and commands;
//! - [`render`] composes each frame and the open trading-metric popup content;
//! - [`metrics`] owns toolbar trading-metric popups and commits TP/SL/leverage edits;
//! - [`docks`] detaches, restores, and repins ordinary panels and persists OS-window geometry;
//! - [`status_bar`] renders connection, license, and diagnostic status;
//! - [`ticker`] owns the header ticker-source picker.

mod actions;
mod core_settings;
pub(crate) mod core_settings_popup;
mod docks;
mod header;
mod init;
mod metrics;
mod render;
mod status_bar;
mod ticker;

use std::time::Instant;

use gpui::*;

use moon_ui::{DockArea, MoonInputState, MoonSliderState};

use crate::{Backend, controls};

/// Shell for one group and one OS window: fixed header and toolbar, one `DockArea`, and status bar.
///
/// The default dock places `ChartTabs` in the center-left, Detects in the right split, and Orders,
/// Assets, Report, Alerts, and Log in the lower tab strip. Those lower utility panels use the
/// ordinary dock-panel detach/repin subsystem; Detects and `ChartTabs` are excluded from it, while
/// `ChartTabs` manages detached chart windows through its own subsystem.
pub(crate) struct Shell {
    backend: Entity<Backend>,
    group: String,
    dock: Entity<DockArea>,
    /// Previous-frame time and smoothed render FPS shown in the status bar, as in the egui host.
    last_frame: Option<Instant>,
    fps: f32,
    /// Backend-observer repaint throttle for telemetry such as book, CPU, and FPS values.
    ///
    /// Updating faster than roughly 4 Hz is not useful to a person and triggers a top-down render
    /// that includes the expensive Orders panel. User-driven toolbar changes bypass this throttle.
    last_notify: Option<Instant>,
    /// Last observed Follow state. User-driven Live/Pause changes bypass the 250 ms throttle so
    /// the toolbar button updates immediately.
    last_follow: bool,
    /// Last observed price scale. This is also user-driven, so its toolbar label updates
    /// immediately despite the Shell observer throttle.
    last_price_scale: Option<f32>,
    /// Last observed broad toolbar and order-controls revision.
    ///
    /// It advances for hotkey or direct size selection, size edits and wheel changes, fixed-sell
    /// slot changes, and fixed-sell percentage edits. Every such change bypasses the 250 ms
    /// throttle so the affected controls update immediately.
    last_order_size_rev: u64,
    /// Handle for this Shell's OS window, used by callbacks that lack `&mut Window`.
    /// Window-bound operations are deferred through this handle instead of moving into `render`.
    window_handle: AnyWindowHandle,
    /// Input for inline order-size editing after a toolbar button is double-clicked.
    /// One input per Shell is reused for every F button.
    size_input: Entity<MoonInputState>,
    /// Toolbar order-size editor target as `(group, F1-F6 index)`, or `None` when idle.
    size_edit: Option<(String, usize)>,
    /// Input for inline fixed-sell percentage editing after an S button is double-clicked, plus
    /// its `(group, S1-S6 index)` target. Blur or Enter updates the local group.
    sell_input: Entity<MoonInputState>,
    sell_edit: Option<(String, usize)>,
    /// Sliders and fields for the TP/SL/leverage popups.
    ///
    /// They persist across renders. TP/SL seed and edit the group-local exit generation; leverage
    /// seeds and edits the active core plus Main-chart market. One set serves the window because
    /// only one metric popup can be open; TP uses separate fixed-range sliders for normal and
    /// `x_tmode` ranges.
    tp_slider_normal: Entity<MoonSliderState>,
    tp_slider_ext: Entity<MoonSliderState>,
    /// Fine TP slider for the sub-percent scalp path.
    ///
    /// Its fixed range is created up front, while opening the TP popup seeds the current value.
    tp_fine_slider: Entity<MoonSliderState>,
    sl_slider: Entity<MoonSliderState>,
    lev_slider: Entity<MoonSliderState>,
    tp_input: Entity<MoonInputState>,
    sl_input: Entity<MoonInputState>,
    lev_input: Entity<MoonInputState>,
    /// Core-settings popup sliders for global TP, trailing, and V-Stop.
    /// Their `CORE_*_BOUNDS` ranges are fixed, values are seeded on open, and subscriptions in
    /// `new` commit changes.
    gtp_slider: Entity<MoonSliderState>,
    trailing_slider: Entity<MoonSliderState>,
    vstop_slider: Entity<MoonSliderState>,
    /// Core-settings fields for global TP, trailing, integer-percent V-Stop, and blacklist text.
    /// Values are seeded on open and committed by subscriptions in `new` on Blur or Enter.
    gtp_input: Entity<MoonInputState>,
    trailing_input: Entity<MoonInputState>,
    vstop_input: Entity<MoonInputState>,
    blacklist_input: Entity<MoonInputState>,
    /// Separate multiline state for the expanded blacklist editor.
    ///
    /// It cannot share `blacklist_input`: turning a state into a textarea is irreversible and
    /// would make the single-line field render as a narrow strip. Toggling synchronizes the text.
    blacklist_area: Entity<MoonInputState>,
    /// Search field for the alert-strategy list in the core-settings popup.
    /// The list can contain hundreds of strategies, so it is filtered and scrolled in the popup.
    def_strategy_input: Entity<MoonInputState>,
    /// The open toolbar-metric popup and the address from which its slider and field were seeded.
    ///
    /// One field keeps TP, SL, and leverage mutually exclusive. The address travels with the open
    /// metric because handlers resolve their live target when an event fires, not when the popup
    /// opened; comparing both prevents a stale editor from continuing to write to an old core or
    /// market after the visible context moves.
    /// `None` means all three anchored popovers are closed.
    open_metric_popup: Option<(controls::TradeMetric, controls::MetricTarget)>,
    /// Window-root focus handle that lets root `on_key_down` receive hotkeys even when Main is empty.
    /// It is focused at startup; chart or input clicks take focus, while F-key events bubble back.
    focus: FocusHandle,
    /// Whether this OS window is active.
    ///
    /// Main inactivity tracking calls `Backend::note_main_input` only for an active window, so
    /// mouse movement over an inactive window cannot reset the timer. Set by the window-activation
    /// observer and also used to gate engine-action toasts.
    window_active: bool,
    /// Whether the active-core selector in the header is open.
    ///
    /// Shell owns this state so selecting a core can close the popover while clicks on exchange
    /// labels and the scroll area leave it open.
    header_core_selector_open: bool,
    /// Whether the controlled `MoonPopover` on the core-settings button is open.
    /// The popover handles outside-click dismissal, while opening seeds its editor fields.
    core_settings_open: bool,
    /// The core the core-settings editors were seeded from, and the only core they may write to.
    ///
    /// `None` while the popup is closed, and also while it is open over a core that had nothing to
    /// seed from — in which case every write is refused rather than aimed at whatever core is
    /// active. See `core_settings::resolve_core_settings_write`.
    core_settings_target: Option<moon_core::session::CoreId>,
    /// Confirmation stage for Cancel All Orders: the first click arms it and the second sends it.
    core_settings_cancel_confirm: bool,
    /// Whether the core-settings coin blacklist uses its fixed-height multiline editor instead of
    /// the collapsed single-line field.
    core_settings_bl_expanded: bool,
    /// Whether the header price-ticker source picker is open.
    ticker_popup_open: bool,
    /// Whether the pointer has entered the ticker popup, enabling dismissal when it later leaves.
    ticker_popup_hovered: bool,
    /// Coin search field used to build the ticker popup's market/core result list.
    ticker_input: Entity<MoonInputState>,
}
