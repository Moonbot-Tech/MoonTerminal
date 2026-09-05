//! Independent Classic/shared-Auto dock layouts, all-core navigation rail, and chart reveal.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::{
    AUTO_WORKSPACE_RAIL_WIDTH_MAX, AUTO_WORKSPACE_RAIL_WIDTH_MIN, WorkspaceMode,
};
use moon_core::feed::{ConnStatus, CoreStartupStatus};
use moon_core::venue::CoreVenue;
use moon_ui::{
    DockTopologyByName, DockTopologyNode, MoonBackgroundPolicy, MoonBadge, MoonBadgeSize,
    MoonBadgeVariant, MoonPalette, MoonScrollbarVisibility, MoonTooltipView, MoonVirtualList,
    PanelView, h_flex, moon_h_resizable, moon_resizable_panel, v_flex,
};
use rust_i18n::t;

use super::Shell;
use crate::workspace::{
    WorkspaceCoreStatus, WorkspaceNavigationAction, WorkspaceRailDensity, WorkspaceRosterInput,
    WorkspaceRosterRow,
};
use crate::{Backend, design};

/// Stable first-run order for the shared Auto topology, with pinned Charts leading operations.
const AUTO_PANEL_ORDER: &[&str] = &[
    "ChartTabs",
    "Report",
    "Orders",
    "Assets",
    "CoreStatus",
    "Log",
    "Detects",
];

/// Auto-only top tabs whose real activation may become the group's restart preference.
const AUTO_WORKSPACE_TAB_NAMES: &[&str] = &[
    "ChartTabs",
    "Report",
    "Assets",
    "CoreStatus",
    "Log",
    "Detects",
];

/// Stable panel names retained exclusively for the Classic workspace.
const AUTO_CLASSIC_ONLY_PANEL_NAMES: &[&str] = &["News", "Alerts"];

/// Return the complete stable-name policy for panels unavailable in Auto.
///
/// Returns:
///     Classic-only panel names shared by dock extraction, detached exclusion, and tests.
fn auto_classic_only_panel_names() -> &'static [&'static str] {
    AUTO_CLASSIC_ONLY_PANEL_NAMES
}

/// Return whether a stable panel name is eligible for Auto top-tab persistence.
///
/// Args:
///     panel_name: Stable dock panel name emitted by MoonUI.
///
/// Returns:
///     `true` for top operational surfaces; Orders and both Classic-only panels are excluded.
pub(super) fn auto_workspace_tab_is_eligible(panel_name: &str) -> bool {
    AUTO_WORKSPACE_TAB_NAMES.contains(&panel_name)
}

/// Resolve a saved Auto tab to an eligible stable name with a deterministic Report fallback.
///
/// Args:
///     saved: Raw persisted name, including possible stale or hand-edited values.
///
/// Returns:
///     The saved eligible name, or `Report` without rewriting the persisted source value.
pub(super) fn resolved_auto_workspace_tab(saved: Option<&str>) -> &str {
    saved
        .filter(|name| auto_workspace_tab_is_eligible(name))
        .unwrap_or("Report")
}

/// Return the deterministic fallback after a requested Auto panel was absent from the live dock.
///
/// Args:
///     activated: Whether the requested saved or resolved panel was found and activated.
///
/// Returns:
///     `Report` only after a failed activation; successful activation needs no second request.
fn auto_workspace_activation_fallback(activated: bool) -> Option<&'static str> {
    (!activated).then_some("Report")
}

/// Return an eligible activation only when it represents a user-visible Auto transition.
///
/// Args:
///     auto: Whether Auto workspace mode currently owns the dock.
///     applying_topology: Whether programmatic topology effects are still being delivered.
///     panel_name: Stable panel name carried by the activation event.
///
/// Returns:
///     The eligible name to persist, or `None` for Classic, programmatic, or ineligible events.
pub(super) fn auto_workspace_tab_to_persist<'a>(
    auto: bool,
    applying_topology: bool,
    panel_name: &'a str,
) -> Option<&'a str> {
    (auto && !applying_topology && auto_workspace_tab_is_eligible(panel_name)).then_some(panel_name)
}

/// Return whether a dock-layout event may update the shared Auto topology.
///
/// Args:
///     auto: Whether Auto workspace mode currently owns the dock.
///     applying_topology: Whether programmatic topology effects are still being delivered.
///
/// Returns:
///     `true` only for a user-driven Auto layout mutation.
pub(super) fn auto_workspace_topology_is_persistable(auto: bool, applying_topology: bool) -> bool {
    auto && !applying_topology
}

/// Return the insertion index that keeps `panel_name` in [`AUTO_PANEL_ORDER`] among `names`.
///
/// Names absent from the first-run order stay where they are; the requested panel is placed
/// before the first present successor, or at the end when none remains.
fn auto_panel_insert_index(names: &[String], panel_name: &str) -> usize {
    let Some(desired) = AUTO_PANEL_ORDER.iter().position(|name| *name == panel_name) else {
        return names.len();
    };
    names
        .iter()
        .position(|existing| {
            AUTO_PANEL_ORDER
                .iter()
                .position(|name| *name == existing.as_str())
                .is_some_and(|position| position > desired)
        })
        .unwrap_or(names.len())
}

/// Insert `panel_name` into the first non-empty Auto strip so a reveal can activate it.
///
/// Split children are walked in order so the upper operational tabs win over the Orders leaf.
/// Empty nodes are skipped so a leftover split slot is not turned into a singleton panel.
fn insert_auto_panel_name(node: &mut DockTopologyNode, panel_name: &str) -> bool {
    match node {
        DockTopologyNode::Empty => false,
        DockTopologyNode::Panel { name } => {
            let existing = name.clone();
            let mut names = vec![existing];
            let at = auto_panel_insert_index(&names, panel_name);
            names.insert(at, panel_name.to_string());
            *node = DockTopologyNode::Tabs { names };
            true
        }
        DockTopologyNode::Tabs { names } => {
            let at = auto_panel_insert_index(names, panel_name);
            names.insert(at, panel_name.to_string());
            true
        }
        DockTopologyNode::Tiles { .. } => false,
        DockTopologyNode::Split { items, .. } => items
            .iter_mut()
            .any(|item| insert_auto_panel_name(item, panel_name)),
    }
}

/// Name `panel_name` in a saved Auto topology that omitted it.
///
/// `apply_topology_by_name` discards unknown names, so a reveal still has to supply a live
/// instance separately. This only makes the requested name present, once, in first-run order.
fn ensure_auto_topology_contains_panel(topology: &mut DockTopologyByName, panel_name: &str) {
    if topology.panel_names().iter().any(|name| name == panel_name) {
        return;
    }
    if insert_auto_panel_name(&mut topology.center, panel_name) {
        return;
    }
    let existing = std::mem::replace(&mut topology.center, DockTopologyNode::Empty);
    topology.center = match existing {
        DockTopologyNode::Empty => DockTopologyNode::Panel {
            name: panel_name.to_string(),
        },
        other => DockTopologyNode::Split {
            horizontal: false,
            items: vec![
                other,
                DockTopologyNode::Panel {
                    name: panel_name.to_string(),
                },
            ],
            sizes: vec![None, None],
        },
    };
}

/// Build the first-run Auto topology before a shared `auto_dock.json` exists.
///
/// Returns:
///     A vertical operations layout with flexible upper tabs containing Report and Log, plus a
///     taller fixed Orders surface below them. Charts stays first; activation is applied separately.
fn default_auto_workspace_topology() -> DockTopologyByName {
    let primary_names = AUTO_PANEL_ORDER
        .iter()
        .copied()
        .filter(|name| *name != "Orders")
        .map(str::to_string)
        .collect();
    DockTopologyByName {
        center: DockTopologyNode::Split {
            horizontal: false,
            items: vec![
                DockTopologyNode::Tabs {
                    names: primary_names,
                },
                DockTopologyNode::Panel {
                    name: "Orders".to_string(),
                },
            ],
            // The upper slot stays flexible. Orders gains four visible table rows while retaining
            // the same logical-pixel semantics as the previous 260 px first-run preference.
            sizes: vec![None, Some(260.0 + 4.0 * design::TABLE_ROW_H)],
        },
        left: None,
        right: None,
        bottom: None,
    }
    .normalized()
}

/// Return detached panel names that need temporary Auto-only instances.
///
/// The live Classic dock is the name authority. A stale `detached.json` record for a panel already
/// present there must not contribute a second `Rc`; independent debounce can leave exactly that
/// `docks.json` / `detached.json` disagreement until startup reconciliation finishes.
///
/// Args:
///     group: Group whose detached records are being suspended for Auto.
///     classic_panel_names: Stable names already owned by the live Classic dock.
///     detached: Persisted Classic detached-window specifications.
///
/// Returns:
///     Unique detached names absent from the live dock, in persisted order.
fn auto_only_detached_panel_names(
    group: &str,
    classic_panel_names: &[String],
    detached: &[crate::window::detached::DetachedSpec],
) -> Vec<String> {
    let mut accounted = classic_panel_names.iter().cloned().collect::<HashSet<_>>();
    detached
        .iter()
        .filter(|spec| {
            spec.group == group
                && !auto_classic_only_panel_names().contains(&spec.panel.as_str())
                && accounted.insert(spec.panel.clone())
        })
        .map(|spec| spec.panel.clone())
        .collect()
}

/// Minimum body width reserved for the active Auto dock tab while the rail is dragged.
const AUTO_DOCK_MIN_WIDTH: f32 = 420.0;

/// Fit one global rail preference into a particular window without changing the preference.
///
/// Args:
///     preferred: Persisted global logical width.
///     chrome_width: Current viewport width in rendered pixels.
///     scale: Current MoonUI scale used to convert between logical and rendered widths.
///
/// Returns:
///     The window-local logical width that preserves the minimum dock content area.
fn fitted_auto_rail_width(preferred: f32, chrome_width: f32, scale: f32) -> f32 {
    let max_width = (chrome_width / scale.max(0.01) - AUTO_DOCK_MIN_WIDTH)
        .clamp(AUTO_WORKSPACE_RAIL_WIDTH_MIN, AUTO_WORKSPACE_RAIL_WIDTH_MAX);
    preferred.clamp(AUTO_WORKSPACE_RAIL_WIDTH_MIN, max_width)
}

/// Format the bounded Icon-density summary while the full localized status remains in its tooltip.
///
/// Args:
///     configured: Number of configured cores represented by the rail.
///
/// Returns:
///     Decimal configured-core count without the wider ready/problem fractions.
fn icon_workspace_summary(configured: usize) -> String {
    configured.to_string()
}

/// One flattened virtual-list item in the all-core rail.
#[derive(Clone)]
enum RailItem {
    /// Current group aggregate scope.
    Overview { selected: bool },
    /// Venue heading and its logo, both resolved before the virtual row closure.
    Exchange {
        venue: Option<CoreVenue>,
        logo: Option<Arc<RenderImage>>,
    },
    /// Configured core row and whether its branch stem ends at this leaf.
    Core {
        row: WorkspaceRosterRow,
        is_last_in_section: bool,
    },
}

/// Density-specific horizontal budget for one indented core leaf.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CoreRailMetrics {
    horizontal_padding: f32,
    gap: f32,
    connector_width: f32,
    connector_elbow_width: f32,
    dot_size: f32,
}

/// Return the horizontal geometry for a core leaf at one responsive rail density.
///
/// Args:
///     density: Current Full, Compact, or Icon rail rung.
///
/// Returns:
///     Padding, gap, connector, elbow, and status-dot sizes in logical pixels.
fn core_rail_metrics(density: WorkspaceRailDensity) -> CoreRailMetrics {
    match density {
        WorkspaceRailDensity::Icon => CoreRailMetrics {
            horizontal_padding: 4.0,
            gap: 3.0,
            connector_width: 8.0,
            connector_elbow_width: 6.0,
            dot_size: 6.0,
        },
        WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => CoreRailMetrics {
            horizontal_padding: 9.0,
            gap: 7.0,
            connector_width: 13.0,
            connector_elbow_width: 9.0,
            dot_size: 7.0,
        },
    }
}

/// Append one exchange section while marking exactly its final core as the branch endpoint.
///
/// Args:
///     items: Flattened rail destination.
///     rows: Ordered configured cores belonging to one exchange heading.
///
/// Returns:
///     Nothing; rows retain their order and only the last receives the terminal shape.
fn append_core_section_items(items: &mut Vec<RailItem>, rows: Vec<WorkspaceRosterRow>) {
    let last_index = rows.len().checked_sub(1);
    items.extend(
        rows.into_iter()
            .enumerate()
            .map(|(index, row)| RailItem::Core {
                row,
                is_last_in_section: Some(index) == last_index,
            }),
    );
}

/// State capable of releasing a generation-scoped Auto topology guard.
trait DeferredAutoTopologyGuard {
    /// Clear the guard only when no newer topology application superseded this callback.
    ///
    /// Args:
    ///     generation: Generation captured when the deferred callback was scheduled.
    fn release_auto_topology_guard(&mut self, generation: u64);
}

/// Defer a weak entity guard release until queued dock-event effects have been delivered.
///
/// Args:
///     entity: Weak owner so a closed window is never retained by the callback.
///     generation: Topology application generation to release.
///     cx: Entity context used to enqueue the callback after current effects.
///
/// Returns:
///     Nothing; a newer generation or dropped entity turns the callback into a no-op.
fn defer_auto_topology_guard_release<T>(entity: WeakEntity<T>, generation: u64, cx: &mut Context<T>)
where
    T: DeferredAutoTopologyGuard + 'static,
{
    cx.defer(move |app| {
        let _ = entity.update(app, |state, _| {
            state.release_auto_topology_guard(generation);
        });
    });
}

impl DeferredAutoTopologyGuard for Shell {
    fn release_auto_topology_guard(&mut self, generation: u64) {
        if self.auto_topology_guard_generation == generation {
            self.applying_auto_topology = false;
        }
    }
}

impl Shell {
    /// Start the blocking exchange-logo cache prewarm off-thread exactly once for this Shell.
    ///
    /// Args:
    ///     cx: Shell context used to spawn work and publish the ready edge on the UI executor.
    ///
    /// Returns:
    ///     Nothing; completion updates only a live weak Shell and requests one repaint.
    fn start_exchange_logo_prewarm(&mut self, cx: &mut Context<Self>) {
        if self.exchange_logo_prewarm_started {
            return;
        }
        self.exchange_logo_prewarm_started = true;
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async { crate::media::exchange_logos::prewarm() })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.exchange_logos_ready = true;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Begin one programmatic Auto topology transition and return its guard generation.
    ///
    /// Returns:
    ///     A generation token that only its deferred completion may release.
    fn begin_auto_topology_application(&mut self) -> u64 {
        self.auto_topology_guard_generation = self.auto_topology_guard_generation.wrapping_add(1);
        self.applying_auto_topology = true;
        self.auto_topology_guard_generation
    }

    /// Keep the current topology guard through queued Dock events, then release it weakly.
    ///
    /// Args:
    ///     generation: Token returned by [`Self::begin_auto_topology_application`].
    ///     cx: Shell context used to enqueue work after dock-event effects.
    ///
    /// Returns:
    ///     Nothing; a newer generation or closed Shell makes the callback a no-op.
    fn finish_auto_topology_application(&mut self, generation: u64, cx: &mut Context<Self>) {
        let shell = cx.entity().downgrade();
        defer_auto_topology_guard_release(shell, generation, cx);
    }

    /// Schedule one window-aware reconciliation of persisted mode and addressed chart requests.
    ///
    /// Args:
    ///     cx: Shell context used to defer through the owning native window.
    ///
    /// Returns:
    ///     Nothing; repeated notifications coalesce while one reconciliation is queued.
    pub(super) fn defer_workspace_window_sync(&mut self, cx: &mut Context<Self>) {
        if self.workspace_sync_pending {
            return;
        }
        self.workspace_sync_pending = true;
        let shell = cx.entity().downgrade();
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                let _ = shell.update(app, |this, cx| {
                    this.workspace_sync_pending = false;
                    this.reconcile_workspace_window(window, cx);
                });
            });
        });
    }

    /// Apply workspace state and reveal the latest ordered group-local Auto surface when required.
    ///
    /// Args:
    ///     window: Owning group window required by the live DockArea APIs.
    ///     cx: Shell context used to read Backend and update the dock.
    ///
    /// Returns:
    ///     Nothing; Classic observes surface revisions but never changes dock activation for them.
    fn reconcile_workspace_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (mode, surface_request) = {
            let backend = self.backend.read(cx);
            (
                backend.workspace_mode(&self.group),
                backend.auto_workspace_surface_request(&self.group),
            )
        };
        let surface = crate::workspace::resolve_auto_workspace_surface(
            mode,
            &mut self.last_auto_surface_revision,
            surface_request,
        );
        self.apply_workspace_mode(mode, window, cx);
        self.sync_auto_dock_topology(surface.map(|s| s.panel_name()), window, cx);
        self.sync_auto_rail_width(window, cx);

        if let Some(surface) = surface {
            self.dock.update(cx, |dock, cx| {
                dock.activate_panel_by_name(surface.panel_name(), window, cx);
            });
        }
    }

    /// Apply the global persisted Auto rail width to this Shell's live resize state.
    ///
    /// Args:
    ///     window: Owning window required by MoonUI's programmatic resize API.
    ///     cx: Shell context used to read Backend and update the resize entity.
    ///
    /// Returns:
    ///     Nothing; an equal live width, an in-flight drag, and an as-yet-unmeasured first render
    ///     are all no-ops.
    pub(super) fn sync_auto_rail_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A drag owns the width until the user lets go. This runs from the Backend observer, which
        // fires on every 10 Hz coordination tick, while `on_resize` writes the new width only on
        // mouse-up — so mid-drag the stored preference is still the OLD width and reconciling to it
        // would yank the rail back. Worse, `resize_panel_silently` clears the in-flight drag as it
        // applies, so the pointer would be left dragging nothing and mouse-up would never persist
        // the width. The reconcile is not skipped, only deferred: mouse-up publishes a revision
        // that brings this straight back with the width the user actually chose.
        if self.workspace_resize_state.read(cx).is_resizing() {
            return;
        }
        let preferred = self.backend.read(cx).auto_workspace_rail_width();
        self.applied_auto_rail_width = preferred;
        let scale = design::ui_value(cx, 1.0).max(0.01);
        let fitted =
            fitted_auto_rail_width(preferred, f32::from(window.viewport_size().width), scale);
        let Some(current) = self
            .workspace_resize_state
            .read(cx)
            .sizes()
            .first()
            .map(|width| width.as_f32() / scale)
        else {
            return;
        };
        if (current - fitted).abs() < 0.5 {
            return;
        }
        self.workspace_resize_state.update(cx, |state, state_cx| {
            state.resize_panel_silently(0, design::ui_px(state_cx, fitted), window, state_cx);
        });
    }

    /// Transform the single live DockArea between independent Classic and shared Auto layouts.
    ///
    /// Args:
    ///     mode: Persisted desired workspace mode for this group.
    ///     window: Owning window required for panel activation synchronization.
    ///     cx: Shell context used for the dock update.
    ///
    /// Returns:
    ///     Nothing; local panel identities survive both name-based transformations.
    pub(super) fn apply_workspace_mode(
        &mut self,
        mode: WorkspaceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if mode == self.applied_workspace_mode {
            return;
        }
        match mode {
            WorkspaceMode::AutoTrading => {
                self.start_exchange_logo_prewarm(cx);
                let classic = self.dock.read(cx).named_layout(cx);
                let classic_panel_names = classic.panel_names();
                let detached_names = auto_only_detached_panel_names(
                    &self.group,
                    &classic_panel_names,
                    &self.backend.read(cx).detached,
                );
                let mut auto_only_panels = detached_names
                    .iter()
                    .filter_map(|name| {
                        crate::window::detached::build_panel(
                            name,
                            &self.group,
                            &self.backend,
                            window,
                            cx,
                        )
                    })
                    .collect::<Vec<_>>();
                let topology = self
                    .backend
                    .read(cx)
                    .auto_dock_topology()
                    .cloned()
                    .unwrap_or_else(default_auto_workspace_topology);
                self.push_missing_auto_topology_panels(
                    &topology,
                    &mut auto_only_panels,
                    window,
                    cx,
                );
                let active_panel = {
                    let backend = self.backend.read(cx);
                    resolved_auto_workspace_tab(backend.auto_workspace_tab(&self.group)).to_string()
                };
                let guard_generation = self.begin_auto_topology_application();
                let classic_only_panels = self.dock.update(cx, |dock, dock_cx| {
                    let classic_only_panels = auto_classic_only_panel_names()
                        .iter()
                        .filter_map(|panel_name| {
                            dock.take_panel_by_name(panel_name, window, dock_cx)
                        })
                        .collect::<Vec<_>>();
                    dock.set_layout_editable(true, dock_cx);
                    dock.set_detach_allowed(false, dock_cx);
                    dock.set_close_allowed(false, dock_cx);
                    dock.apply_topology_by_name(
                        &topology,
                        auto_only_panels.clone(),
                        window,
                        dock_cx,
                    );
                    dock.set_pinned_leading_panels(vec!["ChartTabs".into()], dock_cx);
                    let activated = dock.activate_panel_by_name(&active_panel, window, dock_cx);
                    if let Some(fallback) = auto_workspace_activation_fallback(activated) {
                        dock.activate_panel_by_name(fallback, window, dock_cx);
                    }
                    classic_only_panels
                });
                let actual = self.dock.read(cx).topology_by_name(cx);
                self.backend.update(cx, |backend, backend_cx| {
                    backend.reconcile_auto_dock_topology(actual, backend_cx);
                });
                let group = self.group.clone();
                let suspended = self.backend.update(cx, |backend, _| {
                    crate::window::detached::take_windows(backend, |owner| owner == group.as_str())
                });
                crate::window::windowing::close_all(suspended, cx);
                self.classic_dock_layout = Some(classic);
                self.classic_only_panels = classic_only_panels;
                self.auto_only_panels = auto_only_panels;
                self.header_core_selector_open = false;
                self.applied_workspace_mode = WorkspaceMode::AutoTrading;
                self.finish_auto_topology_application(guard_generation, cx);
            }
            WorkspaceMode::Classic => {
                let classic = self.classic_dock_layout.take();
                self.dock.update(cx, |dock, dock_cx| {
                    dock.set_pinned_leading_panels(Vec::new(), dock_cx);
                    dock.set_detach_allowed(true, dock_cx);
                    dock.set_close_allowed(true, dock_cx);
                    dock.set_layout_editable(true, dock_cx);
                    if let Some(classic) = classic.as_ref() {
                        dock.apply_named_layout(
                            classic,
                            self.classic_only_panels.clone(),
                            window,
                            dock_cx,
                        );
                    }
                });
                self.classic_only_panels.clear();
                self.auto_only_panels.clear();
                self.applied_workspace_mode = WorkspaceMode::Classic;
                crate::window::detached::respawn_all(&self.backend, cx);
            }
        }
        cx.notify();
    }

    /// Apply the latest shared Auto topology to this Shell's local panel instances.
    ///
    /// Args:
    ///     reveal: Unseen Auto surface to make present before the caller activates it.
    ///     window: Owning group window required by DockArea synchronization.
    ///     cx: Shell context used to read the authority and update the dock.
    ///
    /// Returns:
    ///     Nothing; Classic Shells ignore Auto-layout broadcasts. A reveal that names a panel
    ///     missing from the saved topology inserts that name once and builds a live instance when
    ///     the dock does not already own one, so activation is not a discarded no-op.
    fn sync_auto_dock_topology(
        &mut self,
        reveal: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.applied_workspace_mode != WorkspaceMode::AutoTrading {
            return;
        }
        let Some(original) = self.backend.read(cx).auto_dock_topology().cloned() else {
            return;
        };
        let mut topology = original.clone();
        let mut extra = self.auto_only_panels.clone();
        if let Some(panel_name) = reveal {
            ensure_auto_topology_contains_panel(&mut topology, panel_name);
        }
        self.push_missing_auto_topology_panels(&topology, &mut extra, window, cx);
        let guard_generation = self.begin_auto_topology_application();
        self.dock.update(cx, |dock, dock_cx| {
            dock.apply_topology_by_name(&topology, extra, window, dock_cx);
        });
        let repaired = self.dock.read(cx).topology_by_name(cx);
        if repaired != original {
            self.backend.update(cx, |backend, backend_cx| {
                backend.reconcile_auto_dock_topology(repaired, backend_cx);
            });
        }
        self.finish_auto_topology_application(guard_generation, cx);
    }

    /// Build Auto-eligible panels named by `topology` but missing from this dock.
    ///
    /// MoonUI discards unknown topology names, so a saved or just-injected Assets surface cannot
    /// activate until a live identity exists. Only [`AUTO_PANEL_ORDER`] names are created; Classic-only
    /// surfaces stay suspended.
    fn push_missing_auto_topology_panels(
        &self,
        topology: &DockTopologyByName,
        extra: &mut Vec<Rc<dyn PanelView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let live = self.dock.read(cx).topology_by_name(cx).panel_names();
        for name in topology.panel_names() {
            if !AUTO_PANEL_ORDER.contains(&name.as_str()) {
                continue;
            }
            if live.iter().any(|existing| existing == &name) {
                continue;
            }
            if extra
                .iter()
                .any(|panel| panel.panel_name(cx).as_ref() == name)
            {
                continue;
            }
            if let Some(panel) =
                crate::window::detached::build_panel(&name, &self.group, &self.backend, window, cx)
            {
                extra.push(panel);
            }
        }
    }

    /// Render the current workspace body: unchanged Classic dock or Auto rail plus the same dock.
    ///
    /// Args:
    ///     chrome_width: Current rendered window width used by the responsive rail policy.
    ///     p: Active Moon palette.
    ///     cx: Shell context used to derive the live all-config roster.
    ///
    /// Returns:
    ///     Complete body element between toolbar and status bar.
    pub(super) fn workspace_body(
        &self,
        chrome_width: f32,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.applied_workspace_mode == WorkspaceMode::Classic {
            return dock_host(self.dock.clone()).into_any_element();
        }

        let scale = design::ui_value(cx, 1.0).max(0.01);
        let state_id = ElementId::from(SharedString::from(format!(
            "workspace-resizable-{}",
            self.group
        )));
        let resize_state = self.workspace_resize_state.clone();
        let rail_width = fitted_auto_rail_width(self.applied_auto_rail_width, chrome_width, scale);
        let density = crate::workspace::workspace_rail_density(rail_width);
        let max_rail_width =
            fitted_auto_rail_width(AUTO_WORKSPACE_RAIL_WIDTH_MAX, chrome_width, scale);
        let backend = self.backend.clone();
        moon_h_resizable(state_id)
            .with_state(&resize_state)
            .on_resize(move |state, _, cx| {
                let scale = design::ui_value(cx, 1.0).max(0.01);
                let Some(width) = state
                    .read(cx)
                    .sizes()
                    .first()
                    .map(|width| width.as_f32() / scale)
                else {
                    return;
                };
                backend.update(cx, |backend, backend_cx| {
                    backend.set_auto_workspace_rail_width(width, backend_cx);
                });
            })
            .child(
                moon_resizable_panel()
                    .size(design::ui_px(cx, rail_width))
                    .size_range(
                        design::ui_px(cx, AUTO_WORKSPACE_RAIL_WIDTH_MIN)
                            ..design::ui_px(cx, max_rail_width),
                    )
                    .flex_none()
                    .child(self.workspace_rail(density, p, cx)),
            )
            .child(moon_resizable_panel().child(dock_host(self.dock.clone())))
            .into_any_element()
    }

    /// Render the application-wide virtualized core rail for one Auto group window.
    ///
    /// Args:
    ///     density: Full, compact, or icon rung chosen from the body width.
    ///     p: Active Moon palette.
    ///     cx: Shell context used to read configured and live core state.
    ///
    /// Returns:
    ///     Full-width rail panel with summary, current-group Overview, and exchange-grouped cores.
    fn workspace_rail(
        &self,
        density: WorkspaceRailDensity,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let roster = {
            let backend = self.backend.read(cx);
            let mut servers = backend.config.servers.clone();
            crate::core_order::CoreOrder::new(&backend.config)
                .sort_by(&mut servers, |server| server.id);
            let venues = backend.session.core_venues();
            let store = backend.session.store();
            let is_ready = |id| {
                store
                    .core(id)
                    .is_some_and(|core| core.status == ConnStatus::Ready)
            };
            let configured_total = servers.len();
            let mut inputs = servers
                .iter()
                .map(|server| {
                    let core = store.core(server.id);
                    WorkspaceRosterInput {
                        core: server.id,
                        name: server.name.clone(),
                        group: server.group.clone(),
                        venue: venues.get(&server.id).cloned(),
                        availability: backend.workspace_core_availability(&server.group, server.id),
                        ready: core.is_some_and(|core| core.status == ConnStatus::Ready),
                        connection: core.map(|core| core.status.clone()),
                        startup: core
                            .map(|core| core.startup)
                            .unwrap_or_else(CoreStartupStatus::default),
                        fault: core.and_then(|core| core.fault.clone()),
                        // Fed the WHOLE fleet, not the membership-filtered rows below: a hidden
                        // core still connects, so a transport suggestion computed over a subset
                        // would be wrong evidence (frozen contract §8).
                        mode_suggestion: crate::conn_diag::fleet_mode_suggestion(
                            server.id, &servers, is_ready,
                        ),
                    }
                })
                .collect::<Vec<_>>();
            // The rail only ever renders inside an Auto window, so its viewing preset is a known
            // constant. Rows are RETAINED here, on the already-built inputs, rather than by
            // filtering `servers` itself above — `servers` still feeds `fleet_mode_suggestion`
            // just above, which must see the whole fleet.
            inputs.retain(|input| {
                backend.core_displayed(Some(WorkspaceMode::AutoTrading), input.core)
            });
            crate::workspace::derive_workspace_roster(
                &inputs,
                &self.group,
                backend.valid_auto_workspace_core(&self.group),
                configured_total,
            )
        };

        let mut items = vec![RailItem::Overview {
            selected: roster.overview_selected,
        }];
        for section in roster.sections {
            let logo = if self.exchange_logos_ready {
                section
                    .venue
                    .as_ref()
                    .and_then(|venue| venue.brand())
                    .and_then(crate::media::exchange_logos::exchange_logo)
            } else {
                None
            };
            items.push(RailItem::Exchange {
                venue: section.venue,
                logo,
            });
            append_core_section_items(&mut items, section.rows);
        }
        let item_count = items.len();
        let row_height = design::ui_value(cx, 30.0);
        let backend = self.backend.clone();
        let current_group = self.group.clone();
        let rail = MoonVirtualList::new(
            format!("workspace-rail-{}", self.group),
            item_count,
            row_height,
            move |index, _, app| {
                items
                    .get(index)
                    .cloned()
                    .map(|item| {
                        render_rail_item(
                            item,
                            density,
                            p,
                            backend.clone(),
                            current_group.clone(),
                            app,
                        )
                    })
                    .unwrap_or_else(|| div().into_any_element())
            },
        )
        .surface(false)
        .background_policy(MoonBackgroundPolicy::NoFill)
        .border(false)
        .radius(0.0)
        .scrollbar_visibility(MoonScrollbarVisibility::Hover);

        let mut summary = t!(
            "workspace.summary",
            configured = roster.summary.configured,
            ready = roster.summary.ready,
            problem = roster.summary.problem
        )
        .to_string();
        // The rail's own scope marker: its viewing preset is always Auto, since it only ever
        // renders inside an Auto window (frozen contract §5, the H3 rule's concrete instance).
        let marker = crate::workspace::scope_marker::ScopeMarker::new(
            Some(WorkspaceMode::AutoTrading),
            roster.summary.configured,
            roster.summary.configured_total,
        );
        let marker_facts = marker.facts();
        for fact in &marker_facts {
            summary.push(' ');
            summary.push_str(fact);
        }
        let has_problem = roster.summary.problem > 0;
        let problem_color = if has_problem {
            design::danger_color(p)
        } else {
            p.text_muted
        };
        let marker_tail =
            (!marker_facts.is_empty()).then(|| format!(" {}", marker_facts.join(" ")));
        // Rendered as a LIST of segments that the summary's centered wrapper below takes as its
        // direct children, never as a nested flex row of their own: a nested row measured its
        // truncating children at zero and collapsed the whole line to "...problems..." at full
        // rail width (seen live 2026-09-05). The wrapper's row lays the segments out side by side
        // at their content size, and the shrink priority applies only when it is out of room.
        let summary_content: Vec<AnyElement> = match density {
            WorkspaceRailDensity::Icon => {
                let icon_color = if has_problem {
                    design::danger_color(p)
                } else {
                    p.text_soft
                };
                vec![
                    div()
                        .min_w_0()
                        .truncate()
                        .text_center()
                        .text_color(rgb(icon_color))
                        .child(icon_workspace_summary(roster.summary.configured))
                        .into_any_element(),
                ]
            }
            WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => {
                let cores_ready = format!(
                    "{}{SUMMARY_SEP}{}{SUMMARY_SEP}",
                    t!("workspace.summary_cores", n = roster.summary.configured),
                    t!("workspace.summary_ready", n = roster.summary.ready)
                );
                let problem_seg =
                    t!("workspace.summary_problem", n = roster.summary.problem).to_string();
                let mut segments = vec![
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(p.text_muted))
                        .child(cores_ready)
                        .into_any_element(),
                    // `RAIL_ALARM_SHRINK` keeps this segment's flex-shrink small against
                    // `cores_ready`'s default 1.0, so `cores_ready` absorbs essentially all of
                    // the shrink first and the alarm stays fully visible until the row is
                    // genuinely out of room. It still shrinks a little rather than 0, so the
                    // bar's `overflow_hidden()` never hard-clips it without an ellipsis in that
                    // last resort. The full text is always in the bar's own tooltip.
                    div()
                        .min_w_0()
                        .truncate()
                        .flex_shrink(design::RAIL_ALARM_SHRINK)
                        .text_color(rgb(problem_color))
                        .child(problem_seg)
                        .into_any_element(),
                ];
                if let Some(tail) = marker_tail.clone() {
                    segments.push(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(p.text_muted))
                            .child(tail)
                            .into_any_element(),
                    );
                }
                segments
            }
        };
        // The closing hint line lives in the tooltip only — Icon density's bare count never gets
        // it either, since both densities share this one tooltip.
        let summary = if marker_facts.is_empty() {
            summary
        } else {
            marker.tooltip(std::slice::from_ref(&summary))
        };
        v_flex()
            .size_full()
            .h_full()
            .w_full()
            .min_h_0()
            // The rail reads as its own recessed surface rather than a strip of the window, the way
            // a desktop navigation pane does. `gutter` is the recessed side-strip token and is the
            // one that steps AWAY from the chrome and toolbar above (both `shell_high`) in the same
            // direction in either theme, so the separation survives a runtime theme switch. The
            // divider uses the stronger `border_hover` token, matching the existing chrome-boundary
            // convention without turning the rail into a framed card.
            .border_r_1()
            .border_color(rgb(p.border_hover))
            .bg(rgb(p.gutter))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "workspace-summary-{}",
                        self.group
                    )))
                    .flex_none()
                    .h(design::fit_h_px(cx, 38.0, 11.0, 8.0))
                    .overflow_hidden()
                    .px(design::ui_px(cx, 8.0))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .text_size(design::t_caption(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .border_b_1()
                    .border_color(rgb(p.border_soft))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .justify_center()
                            .children(summary_content),
                    )
                    .tooltip(move |_window, cx| {
                        cx.new(|_| MoonTooltipView::new(summary.clone())).into()
                    }),
            )
            .child(div().flex_1().min_h_0().child(rail))
            .into_any_element()
    }
}

/// Separator between the rail summary's three rendered segments.
///
/// Must match `workspace.summary`'s own spacing byte for byte (space, U+00B7 MIDDLE DOT, space):
/// the tooltip still renders that original one-string value, so a mismatch would put two
/// different-looking separators on screen at once.
const SUMMARY_SEP: &str = " · ";

/// Render the absolute-fill host used by both Classic and Auto around the one DockArea.
///
/// Args:
///     dock: Shared group dock entity.
///
/// Returns:
///     Full-height flexible clipped body that does not create another dock or panel instance. The
///     explicit cross-axis height is required because MoonUI's horizontal flex centers children
///     and the absolute DockArea child contributes no intrinsic height of its own.
fn dock_host(dock: Entity<moon_ui::DockArea>) -> impl IntoElement {
    div()
        .relative()
        .flex_1()
        .h_full()
        .w_full()
        .min_h_0()
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .left_0()
                .child(dock),
        )
}

/// Render one virtualized Overview, group, or configured-core row.
///
/// Args:
///     item: Flattened roster item.
///     density: Current responsive rail rung.
///     p: Active Moon palette.
///     backend: Shared state used by click actions.
///     current_group: Group window that owns this rail.
///     cx: Application context used for scaled geometry and callbacks.
///
/// Returns:
///     Complete fixed-height row with localized status and tooltip behavior.
fn render_rail_item(
    item: RailItem,
    density: WorkspaceRailDensity,
    p: MoonPalette,
    backend: Entity<Backend>,
    current_group: String,
    cx: &mut App,
) -> AnyElement {
    match item {
        RailItem::Overview { selected } => {
            let label = t!("workspace.overview").to_string();
            let tooltip = t!("workspace.overview_tip", group = current_group.clone()).to_string();
            let visible = match density {
                WorkspaceRailDensity::Icon => label.chars().next().unwrap_or('?').to_string(),
                WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => label,
            };
            let click_backend = backend.clone();
            let click_group = current_group.clone();
            rail_row_base("workspace-overview", selected, true, 9.0, 7.0, p, cx)
                // This row is a MODE, not a core: it aggregates every core at once, while every
                // row below it selects exactly one. Weight, a step up in size and a rule beneath
                // separate it from the list it sits on top of, without giving it a surface of its
                // own — the rail already reads as one recessed pane. ONE step up, not `t_title`:
                // the virtual list's row height is fixed and does not track the Font slider, so a
                // three-step jump clips its own text at the top of that slider's range.
                .text_size(design::t_body_lg(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .border_b_1()
                .border_color(rgb(p.border_soft))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .justify_center()
                        .gap(design::ui_px(cx, 7.0))
                        .child(design::status_dot_sized(p.accent, 7.0, cx))
                        .child(div().min_w_0().truncate().child(visible)),
                )
                .tooltip(move |_window, cx| {
                    cx.new(|_| MoonTooltipView::new(tooltip.clone())).into()
                })
                .on_click(move |_, _, cx| {
                    click_backend.update(cx, |backend, backend_cx| {
                        backend.select_auto_workspace_core(&click_group, None, backend_cx);
                    });
                })
                .into_any_element()
        }
        RailItem::Exchange { venue, logo } => {
            let label = crate::controls::venue_section_label(venue.as_ref());
            let tooltip = label.clone();
            // Keyed on the venue IDENTITY, never on the caption: an element id built from rendered
            // text changes with the locale and with a core build's spelling, which makes GPUI treat
            // the same heading as a different element and drop its hover and tooltip state.
            let row_id = SharedString::from(match venue.as_ref() {
                Some(venue) => format!("workspace-exchange-{}-{}", venue.id.code, venue.id.dex),
                None => "workspace-exchange-unknown".to_string(),
            });
            let compact_label = match density {
                WorkspaceRailDensity::Icon if logo.is_none() => {
                    Some(label.chars().next().unwrap_or('?').to_string())
                }
                WorkspaceRailDensity::Icon => None,
                WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => Some(label),
            };
            // The gap is paid out of this 30-unit cell (`design::RAIL_SECTION_GAP`), which is why
            // it is 6 and not 8: at font +6 the caption line box is ~18 px, leaving room inside
            // the remaining 23 units. The outer element stays transparent so the rail's own
            // `p.gutter` shows through as the section gap; the inner row alone paints the solid
            // one-step-up background, so the heading itself reads as elevated, not the gap above
            // it.
            div()
                .id(row_id)
                .size_full()
                .min_w_0()
                .pt(design::ui_px(cx, design::RAIL_SECTION_GAP))
                .child(
                    h_flex()
                        .flex_1()
                        .h_full()
                        .items_center()
                        .gap(design::ui_px(cx, 6.0))
                        .px(design::ui_px(cx, 8.0))
                        .min_w_0()
                        .text_size(design::t_caption(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(p.text_muted))
                        .bg(rgb(p.panel_high))
                        .border_b_1()
                        .border_color(rgb(p.border_soft))
                        .when_some(logo, |row, logo| {
                            row.child(
                                img(logo)
                                    .flex_none()
                                    .w(design::ui_px(cx, 13.0))
                                    .h(design::ui_px(cx, 13.0))
                                    .rounded(design::ui_px(cx, 2.0)),
                            )
                        })
                        .when_some(compact_label, |row, label| {
                            row.child(div().min_w_0().truncate().child(label))
                        }),
                )
                .tooltip(move |_window, cx| {
                    cx.new(|_| MoonTooltipView::new(tooltip.clone())).into()
                })
                .into_any_element()
        }
        RailItem::Core {
            row,
            is_last_in_section,
        } => {
            let status = workspace_status_text(row.status);
            let tooltip = workspace_core_tooltip(&row);
            let dot = workspace_status_color(row.status, p);
            let name = match density {
                WorkspaceRailDensity::Icon => row.name.chars().next().unwrap_or('?').to_string(),
                WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => row.name.clone(),
            };
            let metrics = core_rail_metrics(density);
            // Enlarges for every status the summary's `problem` count includes (`Unavailable` as
            // well as `Problem`, see `workspace.rs`'s roster tally), so a counted row is always
            // findable. Colour still separates severity: `Unavailable` keeps `p.amber` and
            // `Problem` keeps the danger colour via `workspace_status_color`, untouched below —
            // only the pill's danger treatment stays `Problem`-only.
            let dot_size = if matches!(
                row.status,
                WorkspaceCoreStatus::Unavailable | WorkspaceCoreStatus::Problem
            ) {
                metrics.dot_size + design::RAIL_PROBLEM_DOT_STEP
            } else {
                metrics.dot_size
            };
            let vertical_stem = div()
                .absolute()
                .left_0()
                .top_0()
                .w(px(1.0))
                .bg(rgb(p.border_soft))
                .when(is_last_in_section, |stem| stem.h(design::ui_px(cx, 14.5)))
                .when(!is_last_in_section, |stem| stem.bottom_0());
            let mut content = h_flex()
                .size_full()
                .min_w_0()
                .gap(design::ui_px(cx, metrics.gap))
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .h_full()
                        .w(design::ui_px(cx, metrics.connector_width))
                        .child(vertical_stem)
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(design::ui_px(cx, 14.5))
                                .w(design::ui_px(cx, metrics.connector_elbow_width))
                                .h(px(1.0))
                                .bg(rgb(p.border_soft)),
                        ),
                )
                .child(design::status_dot_sized(dot, dot_size, cx))
                .child(div().flex_1().min_w_0().truncate().child(name));
            if workspace_status_label_visible(row.status, density) {
                // The pill stays `Problem`-only, deliberately narrower than the dot above: it is
                // the danger treatment, and `Unavailable` is not danger — it already gets its own
                // plain label below via `workspace_status_label_visible`. Do not widen this back to
                // match the dot; the two signals answer different questions on purpose.
                //
                // `AnyElement` boxing here is load-bearing, not incidental: `rail_problem_pill` now
                // returns `MoonBadge` (`impl IntoElement`) while the other arm returns `Div` — the
                // two arms genuinely differ in type, so this is where the divergence gets erased.
                let label_child: AnyElement = if row.status == WorkspaceCoreStatus::Problem {
                    rail_problem_pill(status, p).into_any_element()
                } else {
                    div()
                        .flex_none()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(status)
                        .into_any_element()
                };
                content = content.child(label_child);
            }
            let action = crate::workspace::plan_workspace_navigation(&current_group, &row);
            let selectable = action.is_some();
            let row_id = format!("workspace-core-{}", row.core);
            let mut root = rail_row_base(
                row_id,
                row.selected,
                selectable,
                metrics.horizontal_padding,
                metrics.gap,
                p,
                cx,
            )
            .child(content)
            .tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(tooltip.clone()).max_width(440.0))
                    .into()
            });
            if let Some(action) = action {
                root = root.on_click(move |_, _, cx| {
                    execute_workspace_navigation(&backend, action.clone(), cx);
                });
            }
            root.into_any_element()
        }
    }
}

/// Build one Auto core row's hover text, expanding a problem into its live diagnostics.
///
/// Args:
///     row: Derived roster row with typed connection and startup state.
///
/// Returns:
///     The standard core identity line, followed for problem rows by the exact connection detail
///     and every startup fact also exposed by the Core Status panel.
fn workspace_core_tooltip(row: &WorkspaceRosterRow) -> String {
    let status = workspace_status_text(row.status);
    let mut lines = vec![
        t!(
            "workspace.core_tip",
            name = row.name.as_str(),
            group = row.group.as_str(),
            status = status
        )
        .to_string(),
    ];
    if row.status == WorkspaceCoreStatus::Problem
        && let Some(connection) = row.connection.as_ref()
    {
        let diag = moon_core::feed::diagnose(connection, row.fault.as_ref(), &row.startup);
        lines.push(format!(
            "{}: {}",
            t!("core_status.col.status"),
            crate::panels::connection_status_text(connection, diag.as_ref())
        ));
        // The reason and its next step come BEFORE the channel telemetry: a rail row is a glance
        // surface, and the actionable half must not sit under a dozen measurement lines.
        if let Some(diag) = diag.as_ref() {
            lines.push(crate::panels::problem_diagnostic_text(
                diag,
                row.fault.as_ref(),
                &row.startup,
                row.mode_suggestion,
            ));
        } else {
            lines.push(format!(
                "{}:\n{}",
                t!("core_status.col.startup"),
                crate::panels::startup_diagnostic_text(&row.startup)
            ));
        }
    }
    lines.join("\n")
}

/// Build the tinted danger pill that replaces the plain status label for a `Problem` core row in
/// Full density.
///
/// `MoonBadge` with `MoonBadgeSize::Status` — sized for exactly this use, through the crate's
/// `tokens.ui(..)`/`tokens.line_height(..)` pipeline rather than raw px — reuses the same tinted
/// idiom this pill hand-rolled before: `danger_color` is a text token, never a fill (`design.rs`'s
/// own docstring), so the badge's background and border stay the raw `p.red` hue and only the text
/// carries the legible per-theme colour, matching the amber warnings in
/// `analytics/tuner/list/table.rs` and `analytics/calendar/day.rs`. Confirmed at the Font slider's
/// +6 ceiling: `MoonBadgeSize::Status` scales to a 23px box (`line_height(12.0)` = 18px, plus
/// `ui(2.5)` pad_y on each side = 5px, for 23px total), well inside the fixed 30-unit row cell.
///
/// Args:
///     status: Localized status text, already resolved to "Problem" in the active locale.
///     p: Active Moon palette.
///
/// Returns:
///     A rounded, tinted badge, well inside the fixed 30-unit cell at every font scale.
fn rail_problem_pill(status: String, p: MoonPalette) -> impl IntoElement {
    MoonBadge::new(status)
        .variant(MoonBadgeVariant::Outline)
        .size(MoonBadgeSize::Status)
        .bg_color(p.red)
        .bg_alpha(design::RAIL_PILL_BG_ALPHA)
        .border_color(p.red)
        .border_alpha(design::RAIL_PILL_BORDER_ALPHA)
        .text_color(design::danger_color(p))
}

/// Build shared selection, hover, and disabled chrome for one interactive rail row.
///
/// Args:
///     id: Stable identity unique within the virtualized rail.
///     selected: Whether this row owns the current Auto scope.
///     selectable: Whether the row has a live owning group window and session.
///     horizontal_padding: Density-specific left and right inset in logical pixels.
///     gap: Density-specific gap between direct children in logical pixels.
///     p: Active Moon palette.
///     cx: Application context used for scaled padding.
///
/// Returns:
///     Row container ready for content and an optional click callback.
fn rail_row_base(
    id: impl Into<ElementId>,
    selected: bool,
    selectable: bool,
    horizontal_padding: f32,
    gap: f32,
    p: MoonPalette,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(id)
        .size_full()
        .flex()
        .items_center()
        .min_w_0()
        .gap(design::ui_px(cx, gap))
        .px(design::ui_px(cx, horizontal_padding))
        .text_size(design::t_body(cx))
        .text_color(rgb(if selectable { p.text } else { p.text_muted }))
        .when(selected, |row| {
            row.bg(design::moon_alpha(
                p.accent,
                design::RAIL_ROW_SELECTED_ALPHA,
            ))
        })
        .when(selectable, |row| {
            row.cursor_pointer().hover(move |row| {
                row.bg(design::moon_alpha(p.accent, design::RAIL_ROW_HOVER_ALPHA))
            })
        })
}

/// Execute one pure rail navigation action without reparenting group-owned panels.
///
/// Args:
///     backend: Shared workspace authority and group-window registry.
///     action: Same-group selection or cross-group activation plan.
///     cx: Application context used to publish state and activate the destination window.
///
/// Returns:
///     Nothing; unavailable rows never produce an action.
fn execute_workspace_navigation(
    backend: &Entity<Backend>,
    action: WorkspaceNavigationAction,
    cx: &mut App,
) {
    match action {
        WorkspaceNavigationAction::SelectCurrent { group, core } => {
            backend.update(cx, |backend, backend_cx| {
                backend.select_auto_workspace_core(&group, Some(core), backend_cx);
            });
        }
        WorkspaceNavigationAction::ActivateGroup { group, core } => {
            let handle = backend.update(cx, |backend, backend_cx| {
                if !backend
                    .workspace_core_availability(&group, core)
                    .is_available()
                    // The destination is about to SWITCH to Auto, so its viewing preset is that
                    // constant, not the group's current (pre-transition) one (frozen contract
                    // §10.4).
                    || !backend.core_displayed(Some(WorkspaceMode::AutoTrading), core)
                {
                    return None;
                }
                backend.activate_auto_workspace_core(&group, core, backend_cx);
                backend.group_windows.get(&group).copied()
            });
            if let Some(handle) = handle {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
        }
    }
}

/// Return localized status text for one roster state.
///
/// Args:
///     status: Localization-neutral workspace status.
///
/// Returns:
///     Visible status label in the active locale.
fn workspace_status_text(status: WorkspaceCoreStatus) -> String {
    let key = match status {
        WorkspaceCoreStatus::Disabled => "workspace.status.disabled",
        WorkspaceCoreStatus::Unavailable => "workspace.status.unavailable",
        WorkspaceCoreStatus::Problem => "workspace.status.problem",
        WorkspaceCoreStatus::Ready => "workspace.status.ready",
    };
    t!(key).to_string()
}

/// Return the palette role used by one roster status dot.
///
/// Args:
///     status: Derived core status.
///     p: Active Moon palette.
///
/// Returns:
///     Theme-correct status color.
fn workspace_status_color(status: WorkspaceCoreStatus, p: MoonPalette) -> u32 {
    match status {
        WorkspaceCoreStatus::Ready => design::positive_color(p),
        WorkspaceCoreStatus::Problem => design::danger_color(p),
        WorkspaceCoreStatus::Disabled => p.text_muted,
        WorkspaceCoreStatus::Unavailable => p.amber,
    }
}

/// Decide whether a rail row needs a visible status label beside its dot.
///
/// Ready is conveyed by the green dot; `Problem` and `Unavailable` both get an enlarged dot, and
/// `Problem` alone also gets, in Full density, a tinted danger pill. Failure states remain
/// explicit in Full density and stay available in every density through the row tooltip.
///
/// Args:
///     status: Derived core status.
///     density: Current responsive rail presentation.
///
/// Returns:
///     `true` only for a non-ready status in the full-width rail.
fn workspace_status_label_visible(
    status: WorkspaceCoreStatus,
    density: WorkspaceRailDensity,
) -> bool {
    density == WorkspaceRailDensity::Full && status != WorkspaceCoreStatus::Ready
}

#[cfg(test)]
mod tests;
