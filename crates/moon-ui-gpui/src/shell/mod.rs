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
//! - [`quiet`] owns the header quiet-mode ("sleep") popup state and its editors, with
//!   [`quiet_popup`] rendering the content;
//! - [`ticker`] owns the header ticker-source picker.
//! - [`workspace`] owns independent mode layouts, the all-core rail, and chart reveal.

mod actions;
mod core_settings;
pub(crate) mod core_settings_popup;
mod docks;
mod header;
mod init;
mod metrics;
mod quiet;
pub(crate) mod quiet_popup;
mod render;
mod status_bar;
mod ticker;
mod workspace;

use std::rc::Rc;
use std::time::Instant;

use gpui::*;

use moon_core::config::WorkspaceMode;
use moon_ui::{
    DockArea, DockNamedLayout, MoonInputState, MoonResizableState, MoonSliderState, PanelView,
};

use crate::{Backend, controls};

/// Shell for one group and one OS window: fixed header and toolbar, one `DockArea`, and status bar.
///
/// The default dock places `ChartTabs` in the center-left, Detects in the right split, and Orders,
/// Assets, Report, Alerts, and Log in the lower tab strip. Those lower utility panels use the
/// ordinary dock-panel detach/repin subsystem; Detects and `ChartTabs` are excluded from it, while
/// `ChartTabs` manages detached chart windows through its own subsystem.
pub(crate) struct Shell {
    backend: Entity<Backend>,
    /// When the first-run hint on the toolbar Settings gear was armed, if it was.
    ///
    /// A newcomer sees a terminal with no cores and no clue which of two dozen chrome controls
    /// leads to the key field. This points at that control once, for `pulse::ATTENTION`, and only
    /// on an installation where no core has ever been saved.
    settings_hint_at: Option<std::time::Instant>,
    /// Whether the hint's repaint chain is already running, so arming twice cannot stack timers.
    settings_hint_armed: bool,
    /// Process-wide updater observed separately from high-rate backend state.
    updater: Entity<crate::update::UpdateController>,
    group: String,
    dock: Entity<DockArea>,
    /// Full local Classic layout retained while Auto applies the shared name-only topology.
    classic_dock_layout: Option<DockNamedLayout>,
    /// Exact docked Classic panel identities suspended because Auto intentionally excludes them.
    classic_only_panels: Vec<Rc<dyn PanelView>>,
    /// Temporary local instances for panels whose Classic copies are suspended detached windows.
    auto_only_panels: Vec<Rc<dyn PanelView>>,
    /// Whether this Shell has joined the process-wide single-flight exchange-logo prewarm.
    exchange_logo_prewarm_started: bool,
    /// UI-thread edge proving the blocking logo prewarm completed before render-time resolution.
    exchange_logos_ready: bool,
    /// Suppress DockEvent persistence while Auto topology is installed programmatically.
    applying_auto_topology: bool,
    /// Monotonic token preventing an older deferred guard release from exposing newer effects.
    auto_topology_guard_generation: u64,
    /// One persistent resize state per Shell, synchronized through Backend's global rail width.
    workspace_resize_state: Entity<MoonResizableState>,
    /// Last global logical rail width applied to this Shell's resize state.
    applied_auto_rail_width: f32,
    /// Workspace mode currently applied to `dock`, kept separate from persisted desired state.
    applied_workspace_mode: WorkspaceMode,
    /// Latest ordered Auto surface revision observed for this exact group.
    last_auto_surface_revision: u64,
    /// Coalesces backend and workspace-revision notifications into one window-aware dock update.
    workspace_sync_pending: bool,
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
    /// Core-settings blacklist editor, staged into the popup's draft on Blur or Enter.
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
    open_metric_popup: Option<controls::OpenMetricPopup>,
    /// Window-root focus handle that lets root `on_key_down` receive hotkeys even when Main is empty.
    /// It is focused at startup; chart or input clicks take focus, while F-key events bubble back.
    focus: FocusHandle,
    /// Reads Caps Lock and lone-modifier presses out of the modifier-change stream.
    ///
    /// Per window, because one press spans several events and two windows hold their own keyboards
    /// state as far as GPUI is concerned. See [`crate::hotkeys::resolve_modifiers`].
    modifier_watch: moon_ui::MoonHotkeyModifierWatch,
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
    /// Tab selected in the core-settings popup, retained across openings.
    core_settings_tab: core_settings_popup::CoreSettingsTab,
    /// Staged settings both tabs edit, committed by OK and dropped by Cancel or a core switch.
    ///
    /// `None` also covers "the core's full configuration has not arrived yet", which is what the
    /// waiting placeholder renders against.
    core_settings_draft: Option<moon_core::feed::CoreConfig>,
    /// The draft as it was seeded, so an UNTOUCHED popup can follow the core instead of writing a
    /// stale page back over a change made elsewhere while it was open.
    core_settings_seed: Option<moon_core::feed::CoreConfig>,
    /// Popup numeric editors, created on first render of each field and dropped with the draft.
    ///
    /// Each entry remembers the [`Self::core_settings_seed_gen`] it last took a value from, so a
    /// re-seed reaches it exactly once instead of on every repaint.
    core_settings_inputs: std::collections::HashMap<&'static str, (u64, Entity<MoonInputState>)>,
    /// Popup sliders, created on first render of each row and dropped with the draft. Unlike the
    /// editors they follow the draft on every render, so they need no generation of their own.
    core_settings_sliders: std::collections::HashMap<&'static str, Entity<MoonSliderState>>,
    /// Advances whenever the popup's draft is seeded from a core.
    core_settings_seed_gen: u64,
    /// Whether the header quiet-mode ("sleep") settings popover is open.
    quiet_settings_open: bool,
    /// `HH:MM` editor for the quiet-mode schedule start; seeded on open, committed on edit.
    quiet_from_input: Entity<MoonInputState>,
    /// `HH:MM` editor for the quiet-mode schedule end.
    quiet_to_input: Entity<MoonInputState>,
    /// `AddToChart` bypass-number editor (`3, 5`) for quiet mode.
    quiet_charts_input: Entity<MoonInputState>,
    /// Whether the header price-ticker source picker is open.
    ticker_popup_open: bool,
    /// Whether the pointer has entered the ticker popup, enabling dismissal when it later leaves.
    ticker_popup_hovered: bool,
    /// Coin search field used to build the ticker popup's market/core result list.
    ticker_input: Entity<MoonInputState>,
}
