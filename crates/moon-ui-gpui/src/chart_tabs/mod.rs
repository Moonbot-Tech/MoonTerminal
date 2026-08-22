//! Custom chart tab strip ported from the egui chart tabs: Main plus AddToChart-N.
//! It owns active-tab selection without automatically switching on a detect, chart double-click
//! routing to Main, and detaching tabs into OS windows when Classic owns the workspace. Auto keeps
//! its chart tabs inside the group window. The center `DockArea` panel contains this strip and the
//! active `ChartPanel`; detects, orders, and lower tabs are separate dock panels.
//!
//! The detached-tab OS-window subsystem, including detach, restore, repin, persistence, and
//! `DetachedChartHost`, lives in [`windows`].

mod add_stack;
// `pub(crate)` because Backend queues a detached window's ⧉ press as `ApplyAllRequest`.
pub(crate) mod apply_all;
pub(crate) mod apply_row;
mod candle_popup;
mod graphics_popup;
// `pub(crate)` because the header price ticker reuses `search` and `render_popup`.
pub(crate) mod coin_search;
mod common;
mod custom;
mod detached_host;
mod fig_tools;
mod ingest;
mod labels_popup;
mod layout_popup;
mod main_stack;
mod popup_slot;
mod settings;
mod sig;
mod stack;
mod strip;
#[cfg(test)]
mod tests;
mod windows;

use std::collections::HashMap;

pub(crate) use add_stack::AddChartStack;
use common::LayoutPopupHost;
pub(crate) use common::seg_row;
pub(crate) use main_stack::MainChartStack;
use popup_slot::ChartPopup;
use sig::chart_tabs_sig;

use crate::persistence::chart_persist::StackLayoutMode;

use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonColorPickerEvent, MoonColorPickerState, MoonInputEvent,
    MoonInputState, Panel, PanelEvent, PanelState,
};
use rust_i18n::t;

use crate::Backend;
use crate::persistence::chart_persist;
use moon_core::config::{ChartBucket, ChartTheme, WorkspaceMode};
use moon_core::market::MarketLabel;
use moon_core::session::CoreId;

/// Maximum candidates inspected while preserving a Main chart's market across Auto cores.
const AUTO_MARKET_SEARCH_LIMIT: usize = 256;

/// Return the concrete Auto core that may retarget one group's Main chart.
///
/// Args:
///     backend: Shared state holding persisted mode and validated workspace selection.
///     group: Group whose chart controller consumes the selection.
///
/// Returns:
///     A live selected core in Auto mode, or `None` for Classic and Auto Overview.
fn auto_workspace_chart_core(backend: &Backend, group: &str) -> Option<CoreId> {
    (backend.workspace_mode(group) == WorkspaceMode::AutoTrading)
        .then(|| backend.valid_auto_workspace_core(group))
        .flatten()
}

/// Resolve the market universe one tab's coin field may search.
///
/// Auto binds the whole window to one core, so Main and custom tabs narrow to it instead of
/// offering the group. The narrowing is expressed as a bucket rather than filtered further down
/// because the bucket is also the suggestion cache key: a scope living outside it would keep
/// serving the previous core's suggestions for the rest of that cache's lifetime.
///
/// Args:
///     active: Tab currently owning the coin field.
///     auto_core: Concrete Auto rail selection, or `None` for Classic and Auto Overview.
///
/// Returns:
///     The bucket to search, or `None` for the whole group.
fn coin_search_bucket(active: &Tab, auto_core: Option<CoreId>) -> Option<ChartBucket> {
    match active {
        Tab::Main | Tab::Custom(..) => auto_core.map(ChartBucket::Core),
        Tab::Add(_, bucket) => Some(bucket.clone()),
    }
}

/// Drop accumulated coin checkboxes that fall outside the current search scope, in place.
///
/// The dropdown accumulates a selection across separate searches, so a rail move would otherwise
/// carry invisible markets of the previous core into "Open in new tab" — and into the footer count,
/// which reads the raw selection length.
///
/// Prunes in place and reports whether it removed anything, because the caller sits on the
/// feed-drain path: the steady state is "nothing to prune", and that state must allocate nothing.
///
/// Args:
///     selected: Currently accumulated core and market pairs, pruned in place.
///     scope_core: Concrete Auto core owning the search, or `None` when the group is in scope.
///
/// Returns:
///     `true` when at least one pair was dropped; an unscoped search never drops any.
fn prune_coin_selection_to_scope(
    selected: &mut std::collections::HashSet<(CoreId, String)>,
    scope_core: Option<CoreId>,
) -> bool {
    let Some(scope_core) = scope_core else {
        return false;
    };
    if !selected.iter().any(|(core, _)| *core != scope_core) {
        return false;
    }
    selected.retain(|(core, _)| *core == scope_core);
    true
}

/// Retry identity for one unresolved Auto chart target.
type AutoWorkspaceChartAttempt = (CoreId, Option<(CoreId, u64)>, Option<(CoreId, String)>);

/// Tracks only successfully applied Auto targets while deduplicating unresolved retries.
#[derive(Debug, Default)]
struct AutoWorkspaceChartState {
    applied_core: Option<CoreId>,
    last_attempt: Option<AutoWorkspaceChartAttempt>,
}

impl AutoWorkspaceChartState {
    /// Seed startup state from the restored Auto selection and actual Main target.
    ///
    /// Args:
    ///     selected_core: Concrete Auto core restored for the owning group.
    ///     current_target: Actual restored Main target, if Main has an active chart.
    ///
    /// Returns:
    ///     A tracker with no pending market-resolution attempt.
    fn seeded(selected_core: Option<CoreId>, current_target: Option<(CoreId, String)>) -> Self {
        Self {
            applied_core: selected_core.filter(|selected| {
                current_target
                    .as_ref()
                    .is_some_and(|(current, _)| current == selected)
            }),
            last_attempt: None,
        }
    }

    /// Return a concrete target only for a new or newly resolvable attempt.
    ///
    /// Args:
    ///     next: Current concrete Auto selection, or `None` for Classic/Overview.
    ///     market_revision: Cheap target-core catalog revision used to retry after feed readiness.
    ///     current_target: Current Main core and market, included so an initially empty Main retries.
    ///
    /// Returns:
    ///     The concrete target to resolve. An unchanged successful target or identical failed
    ///     attempt returns `None`.
    fn candidate(
        &mut self,
        next: Option<CoreId>,
        market_revision: Option<(CoreId, u64)>,
        current_target: Option<(CoreId, String)>,
    ) -> Option<CoreId> {
        let Some(target_core) = next else {
            self.applied_core = None;
            self.last_attempt = None;
            return None;
        };
        let main_matches = current_target
            .as_ref()
            .is_some_and(|(current_core, _)| *current_core == target_core);
        if self.applied_core == Some(target_core) && main_matches {
            self.last_attempt = None;
            return None;
        }
        if !main_matches {
            self.applied_core = None;
        }
        let attempt = (target_core, market_revision, current_target);
        if self.last_attempt.as_ref() == Some(&attempt) {
            return None;
        }
        self.last_attempt = Some(attempt);
        Some(target_core)
    }

    /// Commit a target only after Main has accepted its resolved market.
    ///
    /// Args:
    ///     core: Concrete Auto core now represented by Main.
    ///
    /// Returns:
    ///     Nothing; the successful core suppresses focus-only repeats.
    fn commit(&mut self, core: CoreId) {
        self.applied_core = Some(core);
        self.last_attempt = None;
    }
}

/// Pick the target-core market that best preserves the current Main instrument.
///
/// Args:
///     current_market: Canonical market name on the current core.
///     current_label: Catalog-backed coin and quote for that market.
///     candidates: Target-core market names paired with labels from the same market source.
///
/// Returns:
///     Exact market-name availability first, then the same match key and quote, then the same
///     match key alone. `None` leaves the chart unchanged.
fn preferred_auto_workspace_market(
    current_market: &str,
    current_label: &MarketLabel,
    candidates: &[(String, MarketLabel)],
) -> Option<String> {
    if let Some((market, _)) = candidates
        .iter()
        .find(|(market, _)| market == current_market)
    {
        return Some(market.clone());
    }
    let match_key = current_label.match_key();
    candidates
        .iter()
        .find(|(_, label)| {
            label.match_key() == match_key && label.quote.eq_ignore_ascii_case(&current_label.quote)
        })
        .or_else(|| {
            candidates
                .iter()
                .find(|(_, label)| label.match_key() == match_key)
        })
        .map(|(market, _)| market.clone())
}

/// Return the chart-tab strip height in pixels.
/// This must equal `MoonTabStrip`'s tab height (`fit_height(28, 13, 7.5)` in `tab.rs`) so UI and
/// font scaling cannot desynchronize the strip and underline from the tabs. With the default
/// `ui = 1` and `font_delta = 2`, it returns the former constant value of 30.
pub(super) fn chart_tab_strip_h(cx: &App) -> f32 {
    crate::design::fit_h_value(cx, 28.0, 13.0, 7.5)
}

/// Identity of a chart tab, ported from egui's `ContainerKind`.
/// `Main` owns the Main stack; `Add(number, bucket)` identifies an AddToChart tab whose bucket
/// groups core charts as per-core, shared, or named-link collections.
#[derive(Clone, PartialEq, Eq)]
enum Tab {
    Main,
    Add(u32, ChartBucket),
    /// User-created tab built from multi-market selection by Open in New Tab.
    /// It shares the `(number, bucket)` shape with `Add`, uses numbers starting at
    /// `CUSTOM_NUM_BASE`, persists through `custom_coins` specs, and is not populated by detect ingest.
    Custom(u32, ChartBucket),
}

/// Base number for custom tabs, kept above AddToChart detect numbers so their
/// `(number, bucket)` identities cannot collide.
const CUSTOM_NUM_BASE: u32 = 100_000;

pub struct ChartTabs {
    backend: Entity<Backend>,
    group: String,
    epoch: f64,
    theme: ChartTheme,
    /// Main charts: multiple markets as separate `ChartPanel` entries with one active fullscreen.
    main: Entity<MainChartStack>,
    /// AddToChart tabs as `(number, bucket, chart stack)`, sorted by `(number, bucket)`.
    add: Vec<(u32, ChartBucket, Entity<AddChartStack>)>,
    /// Custom tabs from multi-market selection, using the same tuple shape.
    /// Specs persist them while detect ingest does not populate them; `custom_labels` stores labels.
    custom: Vec<(u32, ChartBucket, Entity<AddChartStack>)>,
    /// Custom-tab labels by tab number for display in the strip.
    custom_labels: HashMap<u32, String>,
    /// Next custom-tab number.
    next_custom_num: u32,
    /// Markets checked in the search dropdown for Open in New Tab.
    coin_selected: std::collections::HashSet<(CoreId, String)>,
    /// Order-book gate generation by custom-tab number, invalidating stale five-second suspend
    /// timers so only the latest leave/return/leave cycle applies.
    custom_gate_gen: HashMap<u32, u64>,
    /// Tabs detached into their own OS windows, retained so closing a window can repin its panel
    /// and new detects for the same number continue reaching it.
    detached: Vec<(u32, ChartBucket, Entity<AddChartStack>)>,
    /// Active tab.
    active: Tab,
    /// Number of markets already seen while each `(number, bucket)` tab was active.
    /// The badge is `pane_count - seen`; active tabs catch up and hide it, while inactive tabs
    /// retain their seen count so new detects increase the badge.
    seen: HashMap<(u32, ChartBucket), usize>,
    /// Per-core cursor of processed AddToChart detects.
    add_seq: HashMap<CoreId, u64>,
    /// Signature of inputs that change the tab strip: AddToChart detects, split configuration,
    /// and explicit requests to open a market on Main.
    last_sig: u64,
    /// Last default-setting press this strip has already applied to its own stacks.
    last_defaults_rev: u64,
    /// The ⧉ press this host has armed but not yet performed. See [`apply_row`].
    apply_press: apply_row::ApplyPress,

    /// Successfully applied Auto chart target plus one deduplicated unresolved retry identity.
    ///
    /// Construction seeds `applied_core` only when the actual restored Main target already belongs
    /// to the selected core. A mismatched or absent Main remains pending for catalog resolution.
    /// Classic and Overview are both `None`; later concrete selection changes are explicit.
    auto_workspace_chart_state: AutoWorkspaceChartState,
    /// Last observed toolbar `price_scale_rev`; a larger revision applies the selected scale only
    /// to the active panel, while unchanged revisions mirror the active tab's displayed scale.
    last_scale_rev: u64,
    /// Last observed `switch_charts_rev`; only a larger revision addressed to this group advances
    /// the Main stack's active chart, exactly once.
    last_switch_charts_rev: u64,
    /// Last observed `close_all_charts_rev`; a larger revision closes the Main stack.
    last_close_all_charts_rev: u64,
    /// Last observed `close_active_chart_rev`; a larger revision closes Main's active chart.
    last_close_active_chart_rev: u64,
    /// Last observed `chart_x_sync_rev`; Shift+middle-click in this window applies the scale to all
    /// its stacks and persists `layout.chart_x_ppm_by_group`, exactly once.
    last_x_sync_rev: u64,
    /// Detached tabs pending restoration from `charts.json`; regular Add tabs start empty for
    /// ingest, while Custom tabs seed from their specs. The constructor begins restoration and
    /// defers actual OS-window creation through `cx.defer`.
    restore_pending: Vec<(u32, ChartBucket, chart_persist::WinGeom, Option<f32>)>,
    /// Group-window handle used by backend observers that lack `&mut Window`; opening, activating,
    /// and restoring detached windows must happen outside `render()`.
    window_handle: AnyWindowHandle,
    focus: FocusHandle,
    /// The ONE overlay this strip is showing: ⚙ layout, candles, graphics, labels, the armed
    /// tool's defaults panel, or the market-match list.
    ///
    /// One slot rather than a flag each, so opening any of them closes the rest; the popups need no
    /// separate OS window because chart text renders below the regular GPUI scene. What a labels
    /// module PRINTS is edited in its own dialog, which owns its state for as long as it is up.
    popup: popup_slot::PopupSlot,
    /// Fit-mode height field.
    layout_fit_input: Entity<MoonInputState>,
    /// Scroll-mode height field.
    layout_scroll_input: Entity<MoonInputState>,
    /// Custom-tab name field, shown only for `Custom` in the layout popup.
    custom_name_input: Entity<MoonInputState>,
    /// Per-window market-search input in the tab strip; available markets depend on the active
    /// tab's cores.
    coin_input: Entity<MoonInputState>,
    /// Current market-input text mirrored from `coin_input` on `Change`.
    coin_query: String,
    /// Arbitrary drawing-color picker offered at the end of the settings panel's swatch row.
    fig_color_picker: Entity<MoonColorPickerState>,
}

impl ChartTabs {
    /// Construct a group's chart-tab controller and restore its persisted chart presentation.
    ///
    /// Args:
    ///     backend: Shared application state and persistence handles.
    ///     group: Main window group owned by this tab controller.
    ///     focus_open: Optional core and market to open initially in Main.
    ///     epoch: Chart time origin passed to child stacks.
    ///     theme: Runtime chart palette and rendering theme.
    ///     window: Owning GPUI window used by tab controls and restored child windows.
    ///     cx: Entity context used to create children and retain observers.
    ///
    /// Returns:
    ///     A ready tab controller with process and durable state wired to its children.
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let main = cx.new(|cx| {
            MainChartStack::new(
                backend.clone(),
                group.clone(),
                focus_open,
                epoch,
                theme.clone(),
                cx,
            )
        });
        let initial_sig = chart_tabs_sig(backend.read(cx), &group);
        let initial_auto_workspace_core = auto_workspace_chart_core(backend.read(cx), &group);
        let initial_main_target = main.read(cx).active_target(cx);
        let initial_x_sync_rev = backend.read(cx).chart_x_sync_rev;
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            if let Some(main_handle) = main.read(cx).debug_data_handle(cx) {
                backend.update(cx, |b, _| {
                    b.register_debug_main_chart(group.clone(), main_handle);
                });
            }
        }
        // Load Main scale (`number == 0`) and this group's detached tabs from `charts.json`.
        // Constructor-deferred restoration leaves Add tabs empty for ingest and seeds Custom from specs.
        #[allow(clippy::type_complexity)]
        let (
            main_scale,
            main_layout,
            main_orderbook,
            main_liquidations,
            main_show_zone,
            main_auto_pin,
            main_action_pos,
            main_axis_pos,
            main_time_axis,
            main_line_labels,
            main_cursor_labels,
            main_candle_view,
            main_chart_graphics,
            main_chart_labels,
            restore_pending,
        ): (
            Option<f32>,
            (Option<StackLayoutMode>, Option<u16>, Option<u16>),
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            (
                Option<chart_persist::ChartBtnPos>,
                Option<chart_persist::ChartBtnPos>,
            ),
            Option<chart_persist::PriceAxisPos>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Option<moon_core::market::CandleViewCfg>,
            Option<moon_core::config::ChartGraphicsCfg>,
            Option<moon_core::config::ChartLabelsCfg>,
            Vec<_>,
        ) = {
            let specs = &backend.read(cx).chart_specs;
            let main_spec = specs.iter().find(|s| s.group == group && s.num == 0);
            let main_scale = main_spec.and_then(|s| s.scale);
            let main_layout = main_spec.map_or((None, None, None), |s| {
                (s.layout_mode, s.layout_height_fit, s.layout_height_scroll)
            });
            let main_orderbook = main_spec.and_then(|s| s.orderbook_enabled);
            let main_liquidations = main_spec.and_then(|s| s.liquidations_enabled);
            let main_show_zone = main_spec.and_then(|s| s.show_zone);
            let main_auto_pin = main_spec.and_then(|s| s.auto_pin);
            let main_action_pos =
                main_spec.map_or((None, None), |s| (s.cancel_buy_pos, s.panic_sell_pos));
            let main_axis_pos = main_spec.and_then(|s| s.price_axis_pos);
            let main_time_axis = main_spec.and_then(|s| s.time_axis_visible);
            let main_line_labels = main_spec.and_then(|s| s.line_labels);
            let main_cursor_labels = main_spec.and_then(|s| s.cursor_labels);
            let main_candle_view = main_spec.and_then(|s| s.candle_view);
            let main_chart_graphics = main_spec.and_then(|s| s.chart_graphics);
            let main_chart_labels = main_spec.and_then(|s| s.chart_labels.clone());
            let pending = specs
                .iter()
                .filter(|s| s.group == group && s.num >= 1 && s.detached.is_some())
                .map(|s| (s.num, s.bucket(), s.detached.unwrap(), s.scale))
                .collect();
            (
                main_scale,
                main_layout,
                main_orderbook,
                main_liquidations,
                main_show_zone,
                main_auto_pin,
                main_action_pos,
                main_axis_pos,
                main_time_axis,
                main_line_labels,
                main_cursor_labels,
                main_candle_view,
                main_chart_graphics,
                main_chart_labels,
                pending,
            )
        };
        if main_scale.is_some() {
            main.update(cx, |p, pcx| p.set_scale(main_scale, pcx));
        }
        if main_layout.0.is_some() || main_layout.1.is_some() || main_layout.2.is_some() {
            main.update(cx, |p, pcx| {
                p.set_layout(main_layout.0, main_layout.1, main_layout.2, pcx)
            });
        }
        if main_orderbook.is_some() {
            main.update(cx, |p, pcx| p.set_orderbook_enabled(main_orderbook, pcx));
        }
        if main_liquidations.is_some() {
            main.update(cx, |p, pcx| {
                p.set_liquidations_enabled(main_liquidations, pcx)
            });
        }
        if main_candle_view.is_some() {
            main.update(cx, |p, pcx| p.set_candle_view(main_candle_view, pcx));
        }
        if main_chart_graphics.is_some() {
            main.update(cx, |p, pcx| p.set_chart_graphics(main_chart_graphics, pcx));
        }
        if main_chart_labels.is_some() {
            main.update(cx, |p, pcx| p.set_chart_labels(main_chart_labels, pcx));
        }
        if main_show_zone.is_some() {
            main.update(cx, |p, pcx| p.set_show_zone(main_show_zone, pcx));
        }
        if main_auto_pin.is_some() {
            main.update(cx, |p, pcx| p.set_auto_pin(main_auto_pin, pcx));
        }
        if main_action_pos.0.is_some() || main_action_pos.1.is_some() {
            main.update(cx, |p, pcx| {
                p.set_action_btn_pos(main_action_pos.0, main_action_pos.1, pcx)
            });
        }
        if main_axis_pos.is_some() {
            main.update(cx, |p, pcx| p.set_price_axis_pos(main_axis_pos, pcx));
        }
        if main_time_axis.is_some() {
            main.update(cx, |p, pcx| p.set_time_axis_visible(main_time_axis, pcx));
        }
        if main_line_labels.is_some() {
            main.update(cx, |p, pcx| p.set_line_labels(main_line_labels, pcx));
        }
        if main_cursor_labels.is_some() {
            main.update(cx, |p, pcx| p.set_cursor_labels(main_cursor_labels, pcx));
        }
        // Seed new charts with the group's persisted window X scale from Shift+middle-click sync.
        let group_x_ppm = backend
            .read(cx)
            .layout
            .chart_x_ppm_by_group
            .get(&group)
            .copied();
        if group_x_ppm.is_some() {
            main.update(cx, |s, c| s.set_x_ppm(group_x_ppm, false, c));
        }
        cx.observe(&main, |this, _main, cx| {
            // Main owns focus changes, while ChartTabs owns the visible anchor-aware group target.
            this.sync_main_chart_target(cx);
            // A selected Auto core may have arrived before Main had an active slot. Include the
            // new target identity in the retry key so that opening Main resumes the pending move.
            this.sync_auto_workspace_chart(cx);
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            this.sync_debug_main_chart(cx);
        })
        .detach();
        cx.observe(&backend, |this, backend, cx| {
            // Drain apply-to-all requests from detached windows before the signature early return.
            this.drain_apply_all(cx);
            // And the defaults a press in ANY window set, which this window's stacks must stop
            // shadowing with overrides of their own.
            this.drain_default_clears(cx);
            // Shift+middle-click in this window applies its scale to every stack and persists it.
            this.drain_x_sync(cx);
            // A target core's catalog can become ready without another workspace-selection edge.
            // The retry tracker makes this cheap unless its snapshot revision actually changed.
            this.sync_auto_workspace_chart(cx);
            // The tool-settings panel belongs to an ARMED tool. Disarming — through the picker's
            // Cursor entry or a per-tool hotkey — closes it HERE, before the signature early return
            // below, so the slot cannot survive to re-open a panel nobody asked for the next time a
            // tool is armed.
            if !backend.read(cx).fig_draw_mode {
                this.close_chart_popup(ChartPopup::FigStyle, cx);
            }
            let sig = chart_tabs_sig(backend.read(cx), &this.group);
            if sig == this.last_sig {
                return;
            }
            this.last_sig = sig;
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            this.drain_debug_fill_main_chart(cx);
            this.handle_open_request(true, cx);
            this.handle_open_compare_request(cx);
            this.ingest(cx);
            this.drain_chart_repin(cx);
            this.sync_active_scale(cx);
            this.sync_switch_charts(cx);
            this.sync_close_all_charts(cx);
            this.sync_close_active_chart(cx);
            this.sync_main_chart_target(cx);
            this.sync_seen_for_active(cx);
            this.persist_scales(cx);
            this.sync_inactive_chart_visibility(cx);
            this.last_sig = chart_tabs_sig(backend.read(cx), &this.group);
            cx.notify();
        })
        .detach();
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            this.sync_auto_workspace_chart(cx);
        })
        .detach();
        let market_data_revision = backend.read(cx).market_data_revision();
        cx.observe(&market_data_revision, |this, _revision, cx| {
            // A failed rail retarget must wake when its target catalog becomes resolvable even if
            // no unrelated Backend or workspace state changes.
            this.sync_auto_workspace_chart(cx);
        })
        .detach();
        let coin_input = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .placeholder(rust_i18n::t!("chart.coin.search").to_string())
        });
        // Mirror market-input changes into `coin_query` and reopen the match list. Cyrillic input
        // indicates an accidentally active Russian keyboard layout because tickers are Latin-only;
        // convert it in place, and the resulting change event will deliver the Latin text.
        cx.subscribe_in(
            &coin_input,
            window,
            |this, input, ev: &MoonInputEvent, window, cx| {
                // Focusing the field opens the list without typing: an empty field then shows
                // recents and the day's movers, which is what makes it browsable rather than a
                // box you must already know the answer to fill in.
                if matches!(ev, MoonInputEvent::Focus) {
                    this.open_coin_popup(cx);
                    return;
                }
                if matches!(ev, MoonInputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    if let std::borrow::Cow::Owned(en) =
                        crate::controls::coin_search::normalize_layout(&value)
                    {
                        input.update(cx, |st, c| st.set_value(en, window, c));
                        return;
                    }
                    if this.coin_query != value {
                        // Clearing the text does not close the list; it falls back to suggestions.
                        this.coin_query = value;
                        this.open_chart_popup(ChartPopup::Coin, cx);
                    }
                }
            },
        )
        .detach();
        // The custom drawing-color picker writes RGB into the SELECTED TOOL's style while
        // preserving alpha, which is controlled by the opacity stepper. It seeds from whichever
        // tool is selected at startup; later selections repaint the wheel from the panel's swatches
        // rather than through this state.
        let fig_color_picker = {
            let b = backend.read(cx);
            let init = b.fig_style(b.fig_tool).color;
            let hsla: Hsla =
                gpui::rgb(crate::design::rgb_to_u32([init[0], init[1], init[2]])).into();
            cx.new(|cx| MoonColorPickerState::new(window, cx).default_value(hsla))
        };
        cx.subscribe(
            &fig_color_picker,
            |this, _st, ev: &MoonColorPickerEvent, cx| {
                let MoonColorPickerEvent::Change(h) = ev;
                let c = crate::design::hsla_to_rgb8(*h);
                this.backend.update(cx, |b, bcx| {
                    let tool = b.fig_tool;
                    let style = b.fig_style_mut(tool);
                    style.color = [c[0], c[1], c[2], style.color[3]];
                    bcx.notify();
                });
            },
        )
        .detach();
        let layout_fit_input = cx.new(|cx| MoonInputState::new(window, cx));
        let layout_scroll_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &layout_fit_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.popup_shows(ChartPopup::Layout)
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &layout_scroll_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.popup_shows(ChartPopup::Layout)
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        // Commit the custom-tab name field from the layout popup on blur or Enter.
        let custom_name_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &custom_name_input,
            |this, input, ev: &MoonInputEvent, cx| {
                if this.popup_shows(ChartPopup::Layout)
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    let name = input.read(cx).value().to_string();
                    this.rename_active_custom(name, cx);
                }
            },
        )
        .detach();
        // A strip that has just been built holds no overrides worth clearing; starting at the
        // current revision keeps it from replaying a press that happened before it existed. Read
        // BEFORE the handle moves into the struct.
        let initial_defaults_rev = backend.read(cx).chart_defaults_rev;
        let mut this = Self {
            backend,
            group,
            epoch,
            theme,
            main,
            add: Vec::new(),
            custom: Vec::new(),
            custom_labels: HashMap::new(),
            next_custom_num: CUSTOM_NUM_BASE,
            coin_selected: std::collections::HashSet::new(),
            custom_gate_gen: HashMap::new(),
            detached: Vec::new(),
            active: Tab::Main,
            seen: HashMap::new(),
            add_seq: HashMap::new(),
            last_sig: initial_sig,
            apply_press: apply_row::ApplyPress::default(),
            last_defaults_rev: initial_defaults_rev,
            auto_workspace_chart_state: AutoWorkspaceChartState::seeded(
                initial_auto_workspace_core,
                initial_main_target,
            ),
            last_scale_rev: 0,
            last_switch_charts_rev: 0,
            last_close_all_charts_rev: 0,
            last_close_active_chart_rev: 0,
            last_x_sync_rev: initial_x_sync_rev,
            restore_pending,
            window_handle: window.window_handle(),
            focus: cx.focus_handle(),
            popup: popup_slot::PopupSlot::default(),
            layout_fit_input,
            layout_scroll_input,
            custom_name_input,
            coin_input,
            coin_query: String::new(),
            fig_color_picker,
        };
        // Read from the window being built rather than looked up later: `group_windows` is filled
        // only after `open_window` returns, so the deferred restore below has no addressable group
        // window and would put every restored chart on the primary display.
        let owner_display = crate::window::windowing::window_display_id(window, cx);
        this.restore_detached(owner_display, cx);
        this.restore_custom_tabs(cx);
        // A request can predate this group window. Consume it after the observers and child stacks
        // exist, but leave startup window/tab presentation to Shell's seeded revision cursor.
        this.handle_open_request(false, cx);
        this.sync_active_scale(cx);
        this.initialize_main_chart_target(cx);
        this.persist_scales(cx);
        this
    }

    /// Retarget Main after a changed concrete Auto rail selection without opening another slot.
    ///
    /// The same scope transition prunes accumulated coin-dropdown selections and closes the popup
    /// when any selected market falls outside the new concrete core.
    ///
    /// The current Main instrument is preserved only when the target core can actually serve it:
    /// exact market name, then catalog match key plus quote, then match key alone. Construction
    /// seeds the applied core only when the actual restored Main belongs to that core. Focus-only
    /// revisions compare equal, while a later Main move away from the selected core becomes a new
    /// retarget attempt. Overview clears the marker but deliberately leaves Main untouched.
    /// Failed resolution remains pending and retries only when the target catalog or Main target
    /// identity changes.
    ///
    /// Args:
    ///     cx: ChartTabs context used to resolve markets and replace or focus Main.
    ///
    /// Returns:
    ///     Nothing; missing current or target markets leave the visible chart and tab unchanged.
    fn sync_auto_workspace_chart(&mut self, cx: &mut Context<Self>) {
        let next = auto_workspace_chart_core(self.backend.read(cx), &self.group);
        // Prune the dropdown's accumulated checkboxes against the current scope, ahead of every
        // early return below. Retargeting can legitimately decline to commit — an unchanged retry
        // identity, an empty Main, an unresolvable market — while the rail has still moved, and a
        // reset sequenced behind the commit would leave invisible foreign markets selected and
        // counted in the popup footer.
        if prune_coin_selection_to_scope(&mut self.coin_selected, next) {
            self.close_chart_popup(ChartPopup::Coin, cx);
        }
        let current_target = self.main.read(cx).active_target(cx);
        let market_revision = next.and_then(|core| {
            self.backend
                .read(cx)
                .session
                .market_source()
                .snapshot_revision(core)
        });
        let Some(target_core) = self.auto_workspace_chart_state.candidate(
            next,
            market_revision,
            current_target.clone(),
        ) else {
            return;
        };
        let Some((current_core, current_market)) = current_target else {
            return;
        };
        let target_market = {
            let backend = self.backend.read(cx);
            let source = backend.session.market_source();
            let current_label = source.market_label(current_core, &current_market);
            let match_key = current_label.match_key();
            let mut market_names = Vec::new();
            for query in [&current_market, &current_label.coin, &match_key] {
                for market in source.search_markets(target_core, query, AUTO_MARKET_SEARCH_LIMIT) {
                    if !market_names.contains(&market) {
                        market_names.push(market);
                    }
                }
            }
            let names: Vec<&str> = market_names.iter().map(String::as_str).collect();
            let labels = source.market_labels(target_core, &names);
            let candidates: Vec<(String, MarketLabel)> =
                market_names.into_iter().zip(labels).collect();
            preferred_auto_workspace_market(&current_market, &current_label, &candidates)
        };
        let Some(target_market) = target_market else {
            return;
        };
        self.main.update(cx, |main, main_cx| {
            main.replace_or_focus(target_core, target_market, main_cx)
        });
        self.auto_workspace_chart_state.commit(target_core);
        self.active = Tab::Main;
        self.sync_inactive_chart_visibility(cx);
        self.sync_seen_for_active(cx);
        self.sync_active_scale(cx);
        self.sync_main_chart_target(cx);
        self.persist_scales(cx);
        cx.notify();
    }

    /// Drain this group's current atomic Main-open request and reveal its target chart.
    ///
    /// Args:
    ///     allow_window_activation: Whether an activating request may raise the native window;
    ///         construction passes `false` so an older pending request cannot steal startup focus.
    ///     cx: ChartTabs context used to update Main and optionally activate the group window.
    ///
    /// Returns:
    ///     Nothing; requests owned by another group or replaced during the read phase stay intact.
    fn handle_open_request(&mut self, allow_window_activation: bool, cx: &mut Context<Self>) {
        let pending = {
            let b = self.backend.read(cx);
            b.pending_open_main_request_for_group(self.group.as_str())
                .cloned()
        };
        let Some((pending_core, pending_market)) = pending else {
            return;
        };
        let req = self.backend.update(cx, |b, bcx| {
            b.take_open_main_request_if_matches(
                self.group.as_str(),
                &(pending_core, pending_market.clone()),
                bcx,
            )
        });
        if let Some((core, market, history, activate)) = req {
            // Captured before `history` moves. A Report-scoped request is the coin click in the
            // Report table, which is the ONE non-activating gesture that hands the chart the
            // keyboard below; every other `activate: false` producer — detect, log, screener,
            // plain table navigation — keeps the documented no-focus-change contract.
            let from_report = matches!(history, crate::backend::ChartHistoryScope::Report { .. });
            self.main
                .update(cx, |p, pcx| p.open_or_focus(core, market, history, pcx));
            self.active = Tab::Main;
            self.last_sig = chart_tabs_sig(self.backend.read(cx), self.group.as_str());
            // Raise and focus Main for a live activating request from a chart double-click or
            // Alerts coin click; construction still consumes the request without stealing focus.
            if activate && allow_window_activation {
                let handle = self.window_handle;
                cx.defer(move |app| {
                    let _ = handle.update(app, |_, window, _| window.activate_window());
                });
            } else if allow_window_activation
                && from_report
                && let Some(focus) = self.main.read(cx).active_focus_handle(cx)
            {
                // The Report coin click must still hand the chart the keyboard. Escape is a
                // window-root hotkey, so it reaches nothing while focus stays in the table that
                // opened the chart — the user had to click the plot before the key did anything.
                //
                // THREE gates, each closing a different way this could misbehave:
                // `allow_window_activation` excludes CONSTRUCTION, where taking focus would fight
                // the seeded startup presentation this call site exists to leave alone;
                // `from_report` keeps every other non-activating producer on the documented
                // no-focus-change contract, so navigating from a detect, the log or the screener
                // still cannot pull focus out of a text input the user is typing in; and
                // `is_window_active` means focus can never be pulled OUT of another window — it
                // only settles where focus lands inside the window the user is already in.
                let handle = self.window_handle;
                cx.defer(move |app| {
                    let _ = handle.update(app, |_, window, app| {
                        if window.is_window_active() {
                            window.focus(&focus, app);
                        }
                    });
                });
            }
            self.sync_inactive_chart_visibility(cx);
            self.sync_seen_for_active(cx);
            self.sync_active_scale(cx);
            self.sync_main_chart_target(cx);
            self.persist_scales(cx);
            let group = self.group.clone();
            self.backend.update(cx, |backend, backend_cx| {
                backend.request_chart_tabs_after_main_open(&group);
                backend_cx.notify();
            });
        }
    }

    /// Drain a detect context-menu request to open a new custom tab in comparison mode.
    /// The Backend atomically revalidates the captured producer authority before consumption.
    fn handle_open_compare_request(&mut self, cx: &mut Context<Self>) {
        let req = self.backend.update(cx, |b, _| {
            b.take_open_compare_request_for_group(self.group.as_str())
        });
        if let Some(((core, market), anchor)) = req {
            match anchor {
                // A comparison BETWEEN two charts: the one it was started from and the one asked
                // for. Anything else — a detect card, a coin menu — names one coin and lets the tab
                // gather the rest.
                Some(anchor) => self.open_compare_with(anchor, (core, market), cx),
                None => self.open_compare_tab(core, market, cx),
            }
            self.last_sig = chart_tabs_sig(self.backend.read(cx), self.group.as_str());
        }
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    fn drain_debug_fill_main_chart(&mut self, cx: &mut Context<Self>) {
        let requested = self.backend.update(cx, |b, _| {
            if b.debug_fill_main_chart_group.as_deref() == Some(self.group.as_str()) {
                b.debug_fill_main_chart_group = None;
                Some(b.debug_fill_main_chart_rev)
            } else {
                None
            }
        });
        if requested.is_some() {
            let rev = requested.unwrap_or_default();
            let filled = self
                .main
                .update(cx, |panel, pcx| panel.debug_fill_history_to_capacity(pcx));
            if filled {
                log::info!(
                    "debug fill main chart: delivered group={} rev={} result=ok",
                    self.group,
                    rev
                );
            } else {
                log::warn!(
                    "debug fill main chart: delivered group={} rev={} result=failed",
                    self.group,
                    rev
                );
            }
            self.active = Tab::Main;
            self.last_sig = chart_tabs_sig(self.backend.read(cx), self.group.as_str());
            cx.notify();
        }
    }

    /// Return the active panel to display: Main or an AddToChart/Custom stack.
    fn active_element(&self) -> AnyElement {
        match &self.active {
            Tab::Main => self.main.clone().into_any_element(),
            Tab::Add(n, bucket) | Tab::Custom(n, bucket) => self
                .add_stack(*n, bucket)
                .map(|p| p.into_any_element())
                .unwrap_or_else(|| self.main.clone().into_any_element()),
        }
    }

    fn active_scale(&self, cx: &App) -> Option<f32> {
        match &self.active {
            Tab::Main => self.main.read(cx).scale(),
            Tab::Add(n, bucket) | Tab::Custom(n, bucket) => self
                .add_stack(*n, bucket)
                .map(|p| p.read(cx).scale())
                .unwrap_or_else(|| self.main.read(cx).scale()),
        }
    }

    /// Resolve the visible trading target, preferring an active comparison anchor over Main.
    ///
    /// Args:
    ///     cx: Application context used to read child stack state.
    ///
    /// Returns:
    ///     The visible core and market, or `None` when no chart supplies a target.
    fn main_chart_target(&self, cx: &App) -> Option<(CoreId, String)> {
        // A locked comparison anchor acts like Main fullscreen for trading: its `(core, market)`
        // becomes the group target for F1-F6, S1-S6, and cancel-buy hotkeys.
        if let Some(stack) = self.active_stack() {
            if let Some(anchor) = stack.read(cx).compare_anchor() {
                return Some(anchor);
            }
        }
        self.main.read(cx).active_target(cx)
    }

    /// Observe an ordinary AddToChart stack as a possible visible trading-target source.
    ///
    /// The observer is installed when either a live detect or startup restoration creates the
    /// stack. Detached stacks are harmless because `active_stack` excludes them; the same observer
    /// becomes effective if the panel is later repinned into the tab strip.
    ///
    /// Args:
    ///     stack: Ordinary AddToChart stack whose comparison anchor may change.
    ///     cx: Parent `ChartTabs` context used to retain the observer.
    ///
    /// Returns:
    ///     Nothing; notifications recompute the target only while this stack is active.
    pub(super) fn watch_regular_stack_target(
        &self,
        stack: &Entity<AddChartStack>,
        cx: &mut Context<Self>,
    ) {
        cx.observe(stack, |this, stack, cx| {
            let is_active = this
                .active_stack()
                .is_some_and(|active| active.entity_id() == stack.entity_id());
            if is_active {
                this.sync_main_chart_target(cx);
            }
        })
        .detach();
    }

    /// Seed Backend with the restored target without replacing a durable header selection.
    ///
    /// Args:
    ///     cx: Parent context used to read the restored visible target and update Backend.
    ///
    /// Returns:
    ///     Nothing; this one-time construction path updates runtime target state only.
    fn initialize_main_chart_target(&self, cx: &mut Context<Self>) {
        let target = self.main_chart_target(cx);
        self.backend.update(cx, |b, _| {
            b.initialize_main_chart_target(&self.group, target)
        });
    }

    /// Publish a runtime target change after construction and update durable selection semantics.
    ///
    /// Args:
    ///     cx: Parent context used to resolve the visible target and update Backend.
    ///
    /// Returns:
    ///     Nothing; Backend decides whether the target core genuinely changed.
    fn sync_main_chart_target(&self, cx: &mut Context<Self>) {
        let target = self.main_chart_target(cx);
        self.backend
            .update(cx, |b, _| b.set_main_chart_target(&self.group, target));
    }

    /// Refresh the debug-only Main chart handle after Main itself changes.
    ///
    /// Args:
    ///     cx: Parent context used to read Main and update Backend diagnostics.
    ///
    /// Returns:
    ///     Nothing; absent debug data leaves the previous handle untouched.
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    fn sync_debug_main_chart(&self, cx: &mut Context<Self>) {
        if let Some(handle) = self.main.read(cx).debug_data_handle(cx) {
            self.backend.update(cx, |b, _| {
                b.register_debug_main_chart(self.group.clone(), handle)
            });
        }
    }

    fn set_active_scale(&self, pct: Option<f32>, cx: &mut Context<Self>) {
        match &self.active {
            Tab::Main => self.main.update(cx, |p, pcx| p.set_scale(pct, pcx)),
            Tab::Add(n, bucket) | Tab::Custom(n, bucket) => {
                if let Some(stack) = self.add_stack(*n, bucket) {
                    stack.update(cx, |p, pcx| p.set_scale(pct, pcx));
                }
            }
        }
    }

    /// Apply a tab-strip scale-dropdown selection only to this window's active tab.
    /// `persist_scales` saves Main, Add, and detached scales but currently omits Custom tabs in the
    /// strip; unlike the former toolbar dropdown, this does not change global `price_scale_rev`.
    pub(crate) fn pick_active_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        self.set_active_scale(pct, cx);
        self.persist_scales(cx);
        cx.notify();
    }

    // Market-input matching lives in `custom.rs`; tab-label construction follows below.

    /// Return a tab label as number-group, number-group-core, or number-group-link.
    fn add_label(&self, n: u32, bucket: &ChartBucket, cx: &App) -> String {
        chart_pane_label(&self.backend, &self.group, n, bucket, cx)
    }

    /// Mark inactive tabs invisible because they are absent from the current GPUI scene and must
    /// not run CPU preparation from chart-data observation. Active or detached panels mark
    /// themselves visible in their own render path.
    fn sync_inactive_chart_visibility(&self, cx: &mut Context<Self>) {
        let active = self.active.clone();
        if matches!(active, Tab::Main) {
            self.main
                .update(cx, |panel, pcx| panel.set_scene_visible(true, pcx));
        } else {
            self.main
                .update(cx, |panel, pcx| panel.set_scene_visible(false, pcx));
        }
        for (n, c, panel) in &self.add {
            if Tab::Add(*n, c.clone()) != active {
                panel.update(cx, |panel, pcx| panel.set_scene_visible(false, pcx));
            }
        }
        for (n, c, panel) in &self.custom {
            let visible = Tab::Custom(*n, c.clone()) == active;
            panel.update(cx, |panel, pcx| panel.set_scene_visible(visible, pcx));
        }
    }

    fn sync_seen_for_active(&mut self, cx: &App) {
        if let Tab::Add(n, c) = self.active.clone() {
            if let Some((_, _, panel)) = self.add.iter().find(|(num, cc, _)| *num == n && *cc == c)
            {
                let cnt = panel.read(cx).pane_count(cx);
                self.seen.insert((n, c), cnt);
            }
        }
    }

    fn sync_active_scale(&mut self, cx: &mut Context<Self>) {
        let (rev, want, ours) = {
            let b = self.backend.read(cx);
            (
                b.price_scale_rev,
                b.price_scale,
                b.price_scale_group.as_deref() == Some(&self.group),
            )
        };
        // Apply a scale command only on a larger revision addressed to this group. Advance the
        // observed revision even for another group's bump so it cannot catch up here later.
        if rev != self.last_scale_rev {
            self.last_scale_rev = rev;
            if ours {
                self.set_active_scale(want, cx);
                return;
            }
        }
        // Otherwise mirror the active tab's scale into the backend for the toolbar label.
        let cur = self.active_scale(cx);
        self.backend.update(cx, |b, _| {
            if b.price_scale != cur {
                b.price_scale = cur;
            }
        });
    }

    /// Advance the Main stack's active chart for a larger `switch_charts_rev` addressed to this group.
    /// Always advance the observed revision so another group's bump cannot apply later. AddToChart
    /// and custom tiled lists have no single active chart and are unaffected.
    fn sync_switch_charts(&mut self, cx: &mut Context<Self>) {
        let (rev, ours) = {
            let b = self.backend.read(cx);
            (
                b.switch_charts_rev,
                b.switch_charts_group.as_deref() == Some(&self.group),
            )
        };
        if rev == self.last_switch_charts_rev {
            return;
        }
        self.last_switch_charts_rev = rev;
        if ours {
            self.main.update(cx, |s, scx| s.cycle_active(scx));
        }
    }

    /// Close this group's Main stack on a larger global `close_all_charts_rev`.
    /// The unaddressed revision reaches every `ChartTabs` simultaneously; AddToChart and named
    /// custom tabs remain open.
    fn sync_close_all_charts(&mut self, cx: &mut Context<Self>) {
        let rev = self.backend.read(cx).close_all_charts_rev;
        if rev == self.last_close_all_charts_rev {
            return;
        }
        self.last_close_all_charts_rev = rev;
        self.main.update(cx, |s, scx| {
            s.close_all(scx);
        });
    }

    /// Close Main's active chart on a larger `close_active_chart_rev` for this group.
    /// This works in fullscreen and tiled-stack presentation and does not alter drawing mode. When
    /// Escape empties an Auto Main stack, publish an ordered request for Shell to reveal Report.
    ///
    /// Args:
    ///     cx: Tab context used to read the addressed revision and update Main.
    ///
    /// Returns:
    ///     Nothing; unrelated or already-observed revisions are ignored.
    fn sync_close_active_chart(&mut self, cx: &mut Context<Self>) {
        let (rev, ours) = {
            let b = self.backend.read(cx);
            (
                b.close_active_chart_rev,
                b.close_active_chart_group.as_deref() == Some(&self.group),
            )
        };
        if rev == self.last_close_active_chart_rev {
            return;
        }
        self.last_close_active_chart_rev = rev;
        if ours {
            let main_surface_active = matches!(self.active, Tab::Main);
            let closed = self.main.update(cx, |s, scx| s.close_active(scx));
            let group = self.group.clone();
            self.backend.update(cx, |backend, backend_cx| {
                if backend.request_report_after_main_close(&group, closed, main_surface_active) {
                    backend_cx.notify();
                }
            });
        }
    }
}

impl EventEmitter<PanelEvent> for ChartTabs {}
impl Focusable for ChartTabs {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ChartTabs {
    fn panel_name(&self) -> &'static str {
        "ChartTabs"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        // Do not persist AddToChart tabs; runtime detects recreate them.
        crate::persistence::dock_persist::panel_state_with_group("ChartTabs", &self.group)
    }
    fn background_policy(&self, _cx: &App) -> MoonBackgroundPolicy {
        MoonBackgroundPolicy::NoFill
    }
}

/// Build an AddToChart label for both the tab strip and detached-window title.
/// The base is number-group; per-core buckets append the core, named links append the link, and
/// shared buckets append nothing. An empty group falls back to the number alone.
fn chart_pane_label(
    backend: &Entity<Backend>,
    group: &str,
    n: u32,
    bucket: &ChartBucket,
    cx: &App,
) -> String {
    // A custom multi-market tab uses its `custom_label` or the localized default set label instead
    // of the raw 100000-series number; `custom_coins` in the spec identifies it.
    {
        let specs = &backend.read(cx).chart_specs;
        if let Some(s) = specs.iter().find(|s| s.matches(group, n, bucket)) {
            if s.custom_coins.is_some() {
                return s.custom_label.clone().unwrap_or_else(|| {
                    t!("chart.tab.custom", n = n - CUSTOM_NUM_BASE + 1).to_string()
                });
            }
        }
    }
    let mut label = if group.is_empty() {
        n.to_string()
    } else {
        format!("{n}-{group}")
    };
    let suffix = match bucket {
        ChartBucket::Shared => String::new(),
        ChartBucket::Core(cid) => backend
            .read(cx)
            .session
            .sessions()
            .iter()
            .find(|s| s.id == *cid)
            .map(|s| s.name.clone())
            .unwrap_or_default(),
        ChartBucket::Bundle(name) => name.clone(),
    };
    if !suffix.is_empty() {
        label.push('-');
        label.push_str(&suffix);
    }
    label
}
